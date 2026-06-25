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
    let (nodes, adj) = load_graph(cwd, ns)?;
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
    group_vec.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    // ── Surprising connections: edges that bridge two communities ──
    let mut bridges: Vec<(String, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (a, neighbors) in &adj {
        for (b, _) in neighbors {
            if comm.get(a) != comm.get(b) {
                let key = if a < b { (a.clone(), b.clone()) } else { (b.clone(), a.clone()) };
                if seen.insert(key) {
                    bridges.push((a.clone(), b.clone()));
                }
            }
        }
    }
    bridges.sort_by(|x, y| {
        let dx = degree.get(&x.0).unwrap_or(&0) + degree.get(&x.1).unwrap_or(&0);
        let dy = degree.get(&y.0).unwrap_or(&0) + degree.get(&y.1).unwrap_or(&0);
        dy.cmp(&dx)
    });

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
            println!("  {}  <->  {}", label_of(&nodes, a), label_of(&nodes, b));
        }
    }
    Ok(())
}

fn label_of(nodes: &HashMap<String, crate::graph_query::Node>, id: &str) -> String {
    nodes.get(id).map(|n| n.label.clone()).unwrap_or_else(|| id.to_string())
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
            if let Some((best, _)) = ranked.first() {
                if label.get(*k) != Some(best) {
                    label.insert((*k).clone(), *best);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    label
}
