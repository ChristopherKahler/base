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
use crate::graph_query::{load_graph, Node};

/// Resolve a user string to a node id. Tries, in order: exact concept-slug IRI,
/// exact label (case-insensitive), then unique substring. Returns the id, or a
/// disambiguation error listing the candidates.
fn resolve(nodes: &HashMap<String, Node>, ns: &NamespaceConfig, input: &str) -> Result<String> {
    // 1) Treat input as a concept slug → canonical IRI id (`<iri>`).
    let slug_iri = format!("<{}>", crud::build_iri(ns, "concept", &crud::slugify(input)));
    if nodes.contains_key(&slug_iri) {
        return Ok(slug_iri);
    }

    // 2) Exact label match (case-insensitive).
    let want = input.trim().to_lowercase();
    let exact: Vec<&String> = nodes
        .iter()
        .filter(|(_, n)| n.label.to_lowercase() == want)
        .map(|(id, _)| id)
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0].clone());
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

fn label<'a>(nodes: &'a HashMap<String, Node>, id: &'a str) -> &'a str {
    nodes.get(id).map(|n| n.label.as_str()).unwrap_or(id)
}

/// `base graph get-node <node>` — full detail for one node plus its direct edges.
pub fn get_node(cwd: &Path, ns: &NamespaceConfig, input: &str) -> Result<()> {
    let (nodes, adj) = load_graph(cwd, ns, true)?;
    let id = resolve(&nodes, ns, input)?;
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
    let seed = resolve(nodes, ns, input)?;

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
    let src = resolve(&nodes, ns, from)?;
    let dst = resolve(&nodes, ns, to)?;
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
