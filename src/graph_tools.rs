//! Agentic retrieval primitives — `base graph get-node`, `neighbors`, `path`.
//!
//! `base graph query` does the whole retrieve-and-synthesize pipeline in one shot.
//! These are the finer-grained, read-only graph ops a LIVE Claude session drives
//! across multiple turns: look up a node, expand its neighborhood, trace how two
//! nodes connect — deciding its own traversal instead of trusting base's BFS. This
//! is the graphify-parity agentic-retrieval surface, exposed as plain subcommands
//! (callable from any session via Bash) rather than an MCP server. Output is plain
//! text, formatted to be read back by the model that called it.

use std::collections::{HashMap, VecDeque};
use std::path::Path;

use anyhow::{bail, Result};

use crate::config::NamespaceConfig;
use crate::crud;
use crate::graph_query::{iri_kind, kind_rank, load_graph, Node};

type Adjacency = HashMap<String, Vec<(String, String)>>;

/// Resolve a user string to a node id. Tries, in order: the input as a record slug
/// under any kind, exact label (case-insensitive), then unique substring. Only
/// `concept/<slug>` was ever tried before, so on a store that has not run
/// `graph extract` the slug step could never hit and every lookup fell through to
/// substring matching and its ambiguity error. When several records answer to one
/// slug or name, `pick` decides in a total order, so the same input names the same
/// record on every run. Returns the id, or a disambiguation error listing candidates.
fn resolve(nodes: &HashMap<String, Node>, adj: &Adjacency, ns: &NamespaceConfig, input: &str) -> Result<String> {
    // 1) The input as a slug: `<ns>{kind}/{slug}` under ANY kind. No kind list here
    //    — a milestone or rule slug must hit too — the list only orders a tie.
    let slug = crud::slugify(input);
    let (head, tail) = (format!("<{}", ns.uri), format!("/{slug}>"));
    let by_slug: Vec<&String> = nodes.keys().filter(|id| id.starts_with(&head) && id.ends_with(&tail)).collect();
    if let Some(hit) = pick(by_slug, nodes, adj, input) {
        return Ok(hit);
    }

    // 2) Exact label match (case-insensitive). Several records legitimately share a
    //    name — `domain/base`, `project/base` and every handoff named "base".
    let want = input.trim().to_lowercase();
    let exact: Vec<&String> = nodes
        .iter()
        .filter(|(_, n)| n.label.to_lowercase() == want)
        .map(|(id, _)| id)
        .collect();
    if let Some(hit) = pick(exact, nodes, adj, input) {
        return Ok(hit);
    }

    // 3) Substring match.
    let subs: Vec<&String> = nodes
        .iter()
        .filter(|(_, n)| n.label.to_lowercase().contains(&want))
        .map(|(id, _)| id)
        .collect();
    match subs.len() {
        0 => bail!("No node matches '{input}'. Try `base graph query \"{input}\"` for a fuzzy answer."),
        1 => Ok(subs[0].clone()),
        _ => {
            let mut names: Vec<&str> = subs.iter().filter_map(|id| nodes.get(*id)).map(|n| n.label.as_str()).collect();
            names.sort();
            names.truncate(12);
            bail!("'{input}' is ambiguous ({} matches): {}", subs.len(), names.join(", "));
        }
    }
}

/// The record a name denotes when several answer to it: preferred kind first
/// (`RESOLVE_KINDS`), then the busiest, then the lowest id. A total order, so the
/// choice cannot come out of hash iteration — before this, two domains both named
/// `alpha` resolved to either one, run to run. A tie inside one kind is still a
/// data smell, so it is said on stderr; the caller still gets an answer.
fn pick(mut cands: Vec<&String>, nodes: &HashMap<String, Node>, adj: &Adjacency, input: &str) -> Option<String> {
    if cands.is_empty() {
        return None;
    }
    cands.sort_by_cached_key(|id| {
        let busy = adj.get(*id).map(|v| v.len()).unwrap_or(0);
        (kind_rank(id, nodes.get(*id)), std::cmp::Reverse(busy), (*id).clone())
    });
    let chosen = cands[0];
    let kind = iri_kind(chosen).unwrap_or("record");
    let same_kind = cands.iter().filter(|c| iri_kind(c) == iri_kind(chosen)).count();
    if same_kind > 1 {
        eprintln!(
            "note: {same_kind} {kind} records are named '{input}'; using the busiest ({}). Name the slug to pick another.",
            chosen.trim_matches(['<', '>'])
        );
    }
    Some(chosen.clone())
}

fn label<'a>(nodes: &'a HashMap<String, Node>, id: &'a str) -> &'a str {
    nodes.get(id).map(|n| n.label.as_str()).unwrap_or(id)
}

/// `base graph get-node <node>` — full detail for one node plus its direct edges.
pub fn get_node(cwd: &Path, ns: &NamespaceConfig, input: &str) -> Result<()> {
    let (nodes, adj) = load_graph(cwd, ns, true)?;
    let id = resolve(&nodes, &adj, ns, input)?;
    let n = &nodes[&id];

    println!("NODE {}", n.label);
    if !n.ntype.is_empty() { println!("  type: {}", n.ntype); }
    if !n.source.is_empty() { println!("  source: {}", n.source); }
    if !n.summary.is_empty() { println!("  summary: {}", n.summary); }
    let edges = adj.get(&id).map(|v| v.as_slice()).unwrap_or(&[]);
    println!("  degree: {}", edges.len());
    if !edges.is_empty() {
        println!("  edges:");
        let mut seen = std::collections::HashSet::new();
        for (nb, rel) in edges {
            if seen.insert((nb.clone(), rel.clone())) {
                println!("    --{rel}--> {}", label(&nodes, nb));
            }
        }
    }
    Ok(())
}

/// `base graph neighbors <node> [-d N]` — the n-hop neighborhood as EDGE lines.
pub fn neighbors(cwd: &Path, ns: &NamespaceConfig, input: &str, depth: usize) -> Result<()> {
    let g = load_graph(cwd, ns, true)?;
    let (nodes, adj) = &g;
    let seed = resolve(nodes, adj, ns, input)?;

    let mut visited = std::collections::HashSet::from([seed.clone()]);
    let mut frontier = vec![seed.clone()];
    let mut printed = std::collections::HashSet::new();
    println!("NEIGHBORS of {} (depth {depth})", label(nodes, &seed));
    for hop in 1..=depth.max(1) {
        let mut next = Vec::new();
        for n in &frontier {
            for (nb, rel) in adj.get(n).map(|v| v.as_slice()).unwrap_or(&[]) {
                let key = if n < nb { (n.clone(), nb.clone(), rel.clone()) } else { (nb.clone(), n.clone(), rel.clone()) };
                if printed.insert(key) {
                    println!("  [{hop}] {} --{rel}--> {}", label(nodes, n), label(nodes, nb));
                }
                if visited.insert(nb.clone()) {
                    next.push(nb.clone());
                }
            }
        }
        if next.is_empty() { break; }
        frontier = next;
    }
    if printed.is_empty() {
        println!("  (no edges — isolated node)");
    }
    Ok(())
}

/// `base graph path <from> <to>` — shortest undirected path between two nodes.
pub fn shortest_path(cwd: &Path, ns: &NamespaceConfig, from: &str, to: &str) -> Result<()> {
    let (nodes, adj) = load_graph(cwd, ns, true)?;
    let src = resolve(&nodes, &adj, ns, from)?;
    let dst = resolve(&nodes, &adj, ns, to)?;
    if src == dst {
        println!("Same node: {}", label(&nodes, &src));
        return Ok(());
    }

    // BFS with parent + the relation used to reach each node.
    let mut prev: HashMap<String, (String, String)> = HashMap::new();
    let mut visited = std::collections::HashSet::from([src.clone()]);
    let mut q = VecDeque::from([src.clone()]);
    let mut found = false;
    while let Some(n) = q.pop_front() {
        if n == dst { found = true; break; }
        for (nb, rel) in adj.get(&n).map(|v| v.as_slice()).unwrap_or(&[]) {
            if visited.insert(nb.clone()) {
                prev.insert(nb.clone(), (n.clone(), rel.clone()));
                q.push_back(nb.clone());
            }
        }
    }
    if !found && !prev.contains_key(&dst) {
        println!("No path between {} and {}.", label(&nodes, &src), label(&nodes, &dst));
        return Ok(());
    }

    // Walk back from dst to src, then reverse.
    let mut chain: Vec<(String, String)> = Vec::new(); // (node, relation-into-it)
    let mut cur = dst.clone();
    while cur != src {
        let (p, rel) = prev[&cur].clone();
        chain.push((cur.clone(), rel));
        cur = p;
    }
    chain.reverse();

    println!("PATH {} → {} ({} hop(s))", label(&nodes, &src), label(&nodes, &dst), chain.len());
    print!("  {}", label(&nodes, &src));
    for (node, rel) in &chain {
        print!(" --{rel}--> {}", label(&nodes, node));
    }
    println!();
    Ok(())
}

#[cfg(test)]
mod resolve_tests {
    use super::*;

    fn ns() -> NamespaceConfig {
        NamespaceConfig::default()
    }

    fn node(label: &str, ntype: &str) -> Node {
        Node { label: label.into(), ntype: ntype.into(), source: String::new(), summary: String::new() }
    }

    fn id(local: &str) -> String {
        format!("<{}{}>", ns().uri, local)
    }

    /// Records as (iri local part, label, class, degree), inserted in a varying
    /// order so the maps never share an iteration order between builds.
    fn store(recs: &[(&str, &str, &str, usize)], rotate: usize) -> (HashMap<String, Node>, Adjacency) {
        let mut recs: Vec<_> = recs.to_vec();
        let len = recs.len();
        recs.rotate_left(rotate % len);
        let mut nodes = HashMap::new();
        let mut adj: Adjacency = HashMap::new();
        for (local, label, class, deg) in recs {
            let iri = id(local);
            for k in 0..deg {
                adj.entry(iri.clone()).or_default().push((id(&format!("note/{local}-{k}")), "relatedTo".into()));
            }
            nodes.insert(iri, node(label, class));
        }
        (nodes, adj)
    }

    fn resolve_always(recs: &[(&str, &str, &str, usize)], input: &str) -> String {
        let mut seen = std::collections::HashSet::new();
        for i in 0..20 {
            let (nodes, adj) = store(recs, i);
            seen.insert(resolve(&nodes, &adj, &ns(), input).unwrap());
        }
        assert_eq!(seen.len(), 1, "resolution varied across runs: {seen:?}");
        seen.into_iter().next().unwrap()
    }

    #[test]
    fn two_records_of_one_kind_with_one_name_resolve_to_the_busiest_every_time() {
        // hawk's fixture: `get-node alpha` came back with degree 1 or 3, 6/6 over twelve runs.
        let recs = [("domain/alpha-one", "alpha", "Domain", 3), ("domain/alpha-two", "alpha", "Domain", 1)];
        assert_eq!(resolve_always(&recs, "alpha"), id("domain/alpha-one"));
    }

    #[test]
    fn a_concept_from_extract_keeps_winning_a_shared_slug() {
        // 0.13.17 resolved `concept/<slug>` and nothing else; an extract-era store
        // must keep naming the concept, however busy the domain beside it is.
        let recs = [("domain/alpha", "alpha", "Domain", 40), ("concept/alpha", "alpha", "module", 2)];
        assert_eq!(resolve_always(&recs, "alpha"), id("concept/alpha"));
    }

    #[test]
    fn a_hub_beats_a_busier_handoff_on_an_exact_name() {
        let recs = [("domain/base", "base", "Domain", 2), ("handoff/2026-09-05-x-base", "base", "Handoff", 9)];
        assert_eq!(resolve_always(&recs, "base"), id("domain/base"));
    }

    #[test]
    fn a_slug_of_any_kind_hits_the_slug_step_not_substring() {
        // Milestones are outside the preference list; the slug must still resolve
        // directly instead of falling through to a substring match on a note.
        let recs = [("milestone/ship-v1", "Ship v1", "Milestone", 1), ("note/n1", "why ship v1 slipped", "Note", 1)];
        assert_eq!(resolve_always(&recs, "ship-v1"), id("milestone/ship-v1"));
    }

    #[test]
    fn substring_ambiguity_still_errors_with_the_candidates() {
        let recs = [("note/n1", "alpha one", "Note", 1), ("note/n2", "alpha two", "Note", 1)];
        let (nodes, adj) = store(&recs, 0);
        let err = resolve(&nodes, &adj, &ns(), "alpha").unwrap_err().to_string();
        assert!(err.contains("ambiguous (2 matches)"), "{err}");
    }
}
