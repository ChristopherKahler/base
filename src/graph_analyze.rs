//! `base graph analyze` — emergent structure over the workspace graph, no LLM:
//! god nodes (degree centrality / the core abstractions), communities (label
//! propagation), surprising cross-community connections (bridges), and stats.
//! Mirrors graphify's cluster + analyze, computed over base's unified graph.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;

use crate::config::NamespaceConfig;
use crate::graph_query::load_graph;

pub fn run(cwd: &Path, ns: &NamespaceConfig, top_n: usize) -> Result<()> {
    // Concept-level analysis: AST federation is left to query/agentic tools so
    // community/centrality output stays about ideas, not code structure.
    let (nodes, adj) = load_graph(cwd, ns, false)?;
    if nodes.is_empty() {
        println!("The graph has no labelled nodes yet. Run `base graph extract` first.");
        return Ok(());
    }

    let degree: HashMap<&String, usize> = nodes
        .keys()
        .map(|id| (id, adj.get(id).map(|v| v.len()).unwrap_or(0)))
        .collect();

    // ── God nodes: highest degree = core abstractions ──
    let mut by_deg: Vec<(&String, usize)> = degree.iter().map(|(k, v)| (*k, *v)).collect();
    by_deg.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| label_of(&nodes, a.0).cmp(&label_of(&nodes, b.0))));

    // ── Communities: label propagation ──
    let comm = label_propagation(&nodes, &adj);
    let mut groups: HashMap<usize, Vec<&String>> = HashMap::new();
    for (id, c) in &comm {
        if degree.get(id).copied().unwrap_or(0) == 0 {
            continue; // isolated nodes (no edges) aren't part of any community
        }
        groups.entry(*c).or_default().push(id);
    }
    let mut group_vec: Vec<(usize, Vec<&String>)> = groups.into_iter().collect();
    for (_, members) in group_vec.iter_mut() {
        members.sort();
    }
    // Largest first; equal sizes by their first member, so `[3]` names the same
    // community on every run instead of whichever the map iterated first.
    group_vec.sort_by(|x, y| y.1.len().cmp(&x.1.len()).then_with(|| x.1.first().cmp(&y.1.first())));

    // ── Surprising connections: edges that bridge two communities ──
    let bridges = bridge_pairs(&adj, &comm, &degree);

    // ── Report ──
    let edge_count: usize = adj.values().map(|v| v.len()).sum::<usize>() / 2;
    let connected: usize = degree.values().filter(|d| **d > 0).count();
    println!("# Graph analysis\n");
    println!(
        "{} nodes ({} connected) · {} edges · {} communities\n",
        nodes.len(), connected, edge_count, group_vec.len()
    );

    println!("## God nodes (most connected — the core abstractions)");
    for (id, d) in by_deg.iter().filter(|(_, d)| *d > 0).take(top_n) {
        println!("  {:>3}  {}", d, label_of(&nodes, id));
    }

    println!("\n## Communities (by size)");
    for (i, (_, members)) in group_vec.iter().enumerate().take(top_n) {
        let mut sample: Vec<String> = members.iter().map(|m| label_of(&nodes, m)).collect();
        sample.sort();
        let sample: Vec<String> = sample.into_iter().take(5).collect();
        println!("  [{}] {} nodes — {}", i, members.len(), sample.join(", "));
    }

    if !bridges.is_empty() {
        println!("\n## Surprising connections (bridges across communities)");
        for (a, b) in bridges.iter().take(top_n) {
            let (la, lb) = (label_of(&nodes, a), label_of(&nodes, b));
            if la == lb {
                // Two records really do share this name (`domain/base` ↔ `project/base`).
                // The edge is real; printing it twice under one name reads as a bug.
                println!("  {}  <->  {}", typed_label(&nodes, a), typed_label(&nodes, b));
            } else {
                println!("  {la}  <->  {lb}");
            }
        }
    }
    Ok(())
}

/// Edges whose ends sit in different communities, busiest pair first. Each pair is
/// stored low id first and ties break on the ids, so the list — and which end
/// prints on the left — is the same on every run. The edge set came out of a
/// HashMap walk before, and the same bridge printed as `A <-> B` or `B <-> A`
/// depending on which end the map visited first.
fn bridge_pairs(
    adj: &HashMap<String, Vec<(String, String)>>,
    comm: &HashMap<String, usize>,
    degree: &HashMap<&String, usize>,
) -> Vec<(String, String)> {
    let mut seen = std::collections::HashSet::new();
    let mut bridges: Vec<(String, String)> = Vec::new();
    for (a, neighbors) in adj {
        for (b, _) in neighbors {
            if comm.get(a) != comm.get(b) {
                let pair = if a < b { (a.clone(), b.clone()) } else { (b.clone(), a.clone()) };
                if seen.insert(pair.clone()) {
                    bridges.push(pair);
                }
            }
        }
    }
    let weight = |p: &(String, String)| degree.get(&p.0).unwrap_or(&0) + degree.get(&p.1).unwrap_or(&0);
    bridges.sort_by(|x, y| weight(y).cmp(&weight(x)).then_with(|| x.cmp(y)));
    bridges
}

fn label_of(nodes: &HashMap<String, crate::graph_query::Node>, id: &str) -> String {
    nodes.get(id).map(|n| n.label.clone()).unwrap_or_else(|| id.to_string())
}

/// `base (Domain)` — the label plus its RDF class, for the one place two records
/// sharing a name would otherwise print as the same word twice. A store written
/// before records carried `rdf:type` has no class to show, so the kind is read
/// off the IRI instead (`domain/base` → `base (domain)`).
fn typed_label(nodes: &HashMap<String, crate::graph_query::Node>, id: &str) -> String {
    let label = label_of(nodes, id);
    let kind = nodes
        .get(id)
        .map(|n| n.ntype.clone())
        .filter(|t| !t.is_empty())
        .or_else(|| crate::graph_query::iri_kind(id).map(str::to_string));
    match kind {
        Some(k) => format!("{label} ({k})"),
        None => label,
    }
}

/// Label propagation: each node iteratively adopts the most common label among
/// its neighbours until stable. Deterministic (sorted keys, deterministic
/// tie-break) so the same graph yields the same communities.
fn label_propagation(
    nodes: &HashMap<String, crate::graph_query::Node>,
    adj: &HashMap<String, Vec<(String, String)>>,
) -> HashMap<String, usize> {
    let mut keys: Vec<&String> = nodes.keys().collect();
    keys.sort();
    let mut label: HashMap<String, usize> =
        keys.iter().enumerate().map(|(i, k)| ((*k).clone(), i)).collect();

    for _ in 0..20 {
        let mut changed = false;
        for k in &keys {
            let Some(neighbors) = adj.get(*k) else { continue };
            if neighbors.is_empty() {
                continue;
            }
            let mut counts: HashMap<usize, usize> = HashMap::new();
            for (nb, _) in neighbors {
                if let Some(&l) = label.get(nb) {
                    *counts.entry(l).or_insert(0) += 1;
                }
            }
            // Most frequent label; ties broken by smallest label id (determinism).
            let mut ranked: Vec<(usize, usize)> = counts.into_iter().collect();
            ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            if let Some((best, _)) = ranked.first()
                && label.get(*k) != Some(best)
            {
                label.insert((*k).clone(), *best);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    label
}

#[cfg(test)]
mod order_tests {
    use super::*;
    use crate::graph_query::Node;

    const U: &str = "http://t.local/o#";

    fn node(label: &str, ntype: &str) -> Node {
        Node { label: label.into(), ntype: ntype.into(), source: String::new(), summary: String::new() }
    }

    /// Two communities joined by one bridge, built in a varying insertion order and
    /// with a fresh hash seed each time.
    fn two_clusters(rotate: usize) -> crate::graph_query::GraphMaps {
        let ids = ["domain/alpha", "project/alpha", "note/a1", "note/a2", "note/p1", "note/p2"];
        let mut edges: Vec<(&str, &str)> = vec![
            ("domain/alpha", "note/a1"),
            ("domain/alpha", "note/a2"),
            ("project/alpha", "note/p1"),
            ("project/alpha", "note/p2"),
            ("domain/alpha", "project/alpha"),
        ];
        let len = edges.len();
        edges.rotate_left(rotate % len);
        let mut nodes = HashMap::new();
        for id in ids {
            let (kind, _) = id.split_once('/').unwrap();
            nodes.insert(format!("<{U}{id}>"), node("alpha", if kind == "note" { "Note" } else { "" }));
        }
        let mut adj: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for (a, b) in edges {
            let (a, b) = (format!("<{U}{a}>"), format!("<{U}{b}>"));
            adj.entry(a.clone()).or_default().push((b.clone(), "relatedTo".into()));
            adj.entry(b).or_default().push((a, "relatedTo".into()));
        }
        (nodes, adj)
    }

    #[test]
    fn bridge_pairs_print_the_same_way_round_every_run() {
        let mut seen: std::collections::HashSet<Vec<(String, String)>> = std::collections::HashSet::new();
        for i in 0..20 {
            let (nodes, adj) = two_clusters(i);
            let degree: HashMap<&String, usize> =
                nodes.keys().map(|id| (id, adj.get(id).map(|v| v.len()).unwrap_or(0))).collect();
            let comm = label_propagation(&nodes, &adj);
            seen.insert(bridge_pairs(&adj, &comm, &degree));
        }
        assert_eq!(seen.len(), 1, "bridge order varied across runs: {seen:?}");
        for (a, b) in seen.into_iter().next().unwrap() {
            assert!(a < b, "pair stored low id first: {a} {b}");
        }
    }

    #[test]
    fn a_same_name_pair_on_an_untyped_store_is_still_qualified_by_iri_kind() {
        let (nodes, _) = two_clusters(0);
        assert_eq!(typed_label(&nodes, &format!("<{U}domain/alpha>")), "alpha (domain)");
        assert_eq!(typed_label(&nodes, &format!("<{U}project/alpha>")), "alpha (project)");
        // A typed record still shows its class, which is the nicer of the two.
        assert_eq!(typed_label(&nodes, &format!("<{U}note/a1>")), "alpha (Note)");
    }
}
