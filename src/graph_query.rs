//! `base graph query "<question>"` — GraphRAG retrieval + synthesis over the
//! workspace graph. Mirrors graphify's retrieval algorithm (IDF-weighted keyword
//! seeding → hub-avoiding BFS → token-budgeted subgraph render) but does the
//! synthesis IN the command via the no-key LLM keystone, over base's unified
//! graph (LLM concepts + curated entities), instead of punting to an external
//! agent over MCP.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Result;
use oxigraph::sparql::QueryResults;

use crate::config::NamespaceConfig;
use crate::crud;

pub struct Node {
    pub label: String,
    pub ntype: String,
    pub source: String,
    pub summary: String,
}

const EXACT: f64 = 1000.0;
const PREFIX: f64 = 100.0;
const SUBSTRING: f64 = 1.0;
const SOURCE: f64 = 0.5;

pub struct Options<'a> {
    pub depth: usize,
    pub token_budget: usize,
    pub model: Option<&'a str>,
    pub raw: bool,
}

pub fn run(cwd: &Path, ns: &NamespaceConfig, question: &str, opts: &Options) -> Result<()> {
    let (nodes, adj) = load_graph(cwd, ns, true)?;
    if nodes.is_empty() {
        println!("The graph has no labelled nodes yet. Run `base graph extract` first.");
        return Ok(());
    }

    let terms = query_terms(question);
    let scored = score_nodes(&nodes, &terms);
    let seeds = pick_seeds(&scored);
    if seeds.is_empty() {
        println!("No matching nodes found for: {question}");
        return Ok(());
    }

    let degree: HashMap<&String, usize> = adj.iter().map(|(k, v)| (k, v.len())).collect();
    let hub = hub_threshold(&degree);
    let (visited, edges) = bfs(&adj, &seeds, opts.depth, hub, &degree);
    let subgraph = render(&nodes, &visited, &edges, &seeds, opts.token_budget);

    let seed_labels: Vec<&str> = seeds
        .iter()
        .filter_map(|s| nodes.get(s).map(|n| n.label.as_str()))
        .collect();
    eprintln!(
        "Traversal: BFS depth={} | seeds={:?} | {} nodes",
        opts.depth,
        seed_labels,
        visited.len()
    );

    if opts.raw {
        println!("{subgraph}");
        return Ok(());
    }

    let prompt = synth_prompt(question, &subgraph);
    match crate::llm::complete(&prompt, opts.model) {
        Ok(answer) => println!("{answer}"),
        Err(e) => {
            eprintln!("(synthesis failed: {e} — showing raw subgraph)");
            println!("{subgraph}");
        }
    }
    Ok(())
}

/// Pull labelled nodes and the concept edge-set from graph.nq into memory.
/// Edges come from reified semantic edges (ops:from/ops:to); traversal is
/// undirected so BFS reaches both callers and callees of a concept.
pub fn load_graph(
    cwd: &Path,
    ns: &NamespaceConfig,
    include_ast: bool,
) -> Result<(HashMap<String, Node>, HashMap<String, Vec<(String, String)>>)> {
    let p = &ns.prefix;

    let node_q = format!(
        "SELECT ?s ?label ?type ?src ?summary WHERE {{ GRAPH ?g {{\n\
           ?s {p}:name ?label .\n\
           OPTIONAL {{ ?s {p}:conceptType ?type }}\n\
           OPTIONAL {{ ?s {p}:sourceDoc ?src }}\n\
           OPTIONAL {{ ?s {p}:summary ?summary }}\n\
         }} }}"
    );
    let mut nodes: HashMap<String, Node> = HashMap::new();
    if let QueryResults::Solutions(sols) = crud::load_and_query(cwd, ns, &node_q)? {
        for row in sols.filter_map(|r| r.ok()) {
            let Some(s) = row.get("s").map(|t| t.to_string()) else { continue };
            nodes.entry(s).or_insert_with(|| Node {
                label: row.get("label").map(|t| crud::term_display(t.into())).unwrap_or_default(),
                ntype: row.get("type").map(|t| crud::term_display(t.into())).unwrap_or_default(),
                source: row.get("src").map(|t| crud::term_display(t.into())).unwrap_or_default(),
                summary: row.get("summary").map(|t| crud::term_display(t.into())).unwrap_or_default(),
            });
        }
    }

    let edge_q = format!(
        "SELECT ?a ?b ?rel WHERE {{ GRAPH ?g {{\n\
           ?e a {p}:SemanticEdge ; {p}:from ?a ; {p}:to ?b .\n\
           OPTIONAL {{ ?e {p}:relation ?rel }}\n\
         }} }}"
    );
    let mut adj: HashMap<String, Vec<(String, String)>> = HashMap::new();
    if let QueryResults::Solutions(sols) = crud::load_and_query(cwd, ns, &edge_q)? {
        for row in sols.filter_map(|r| r.ok()) {
            let (Some(a), Some(b)) = (row.get("a").map(|t| t.to_string()), row.get("b").map(|t| t.to_string())) else { continue };
            let rel = row.get("rel").map(|t| crud::term_display(t.into())).unwrap_or_else(|| "relates_to".into());
            adj.entry(a.clone()).or_default().push((b.clone(), rel.clone()));
            adj.entry(b).or_default().push((a, rel)); // undirected for reachability
        }
    }

    // Federate the per-app AST map so code entities (functions/structs/modules)
    // and their call/import relationships traverse alongside semantic concepts.
    // Edges are kept only when both endpoints are real nodes (drops dangling
    // import targets). Code-entity IRIs (code:*) never collide with concept IRIs.
    if include_ast {
        let (code_nodes, code_edges) = crate::crud::ast_query::code_graph(cwd, ns);
        for (id, label, ntype, file) in code_nodes {
            nodes.entry(id).or_insert(Node {
                label,
                ntype,
                source: file,
                summary: String::new(),
            });
        }
        for (a, b, rel) in code_edges {
            if nodes.contains_key(&a) && nodes.contains_key(&b) {
                adj.entry(a.clone()).or_default().push((b.clone(), rel.clone()));
                adj.entry(b).or_default().push((a, rel));
            }
        }
    }

    Ok((nodes, adj))
}

/// Split a question into searchable terms — drop short stopword-ish tokens.
fn query_terms(question: &str) -> Vec<String> {
    question
        .split(|c: char| !c.is_alphanumeric())
        .filter_map(|w| {
            let w = w.trim().to_lowercase();
            let english = w.chars().all(|c| c.is_ascii_lowercase());
            if !w.is_empty() && (!english || w.len() > 2) { Some(w) } else { None }
        })
        .collect()
}

/// IDF per term: common terms (match many node labels) get low weight; rare
/// identifiers get high weight. log(1 + N/(1+df)).
fn idf(nodes: &HashMap<String, Node>, terms: &[String]) -> HashMap<String, f64> {
    let n = nodes.len().max(1) as f64;
    let mut out = HashMap::new();
    for t in terms {
        let df = nodes.values().filter(|node| node.label.to_lowercase().contains(t)).count() as f64;
        out.insert(t.clone(), (1.0 + n / (1.0 + df)).ln());
    }
    out
}

fn score_nodes(nodes: &HashMap<String, Node>, terms: &[String]) -> Vec<(f64, String)> {
    let weights = idf(nodes, terms);
    let mut scored: Vec<(f64, String)> = Vec::new();
    for (id, node) in nodes {
        let label = node.label.to_lowercase();
        let source = node.source.to_lowercase();
        let mut score = 0.0;
        for t in terms {
            let w = weights.get(t).copied().unwrap_or(1.0);
            if label == *t {
                score += EXACT * w;
            } else if label.starts_with(t.as_str()) {
                score += PREFIX * w;
            } else if label.contains(t.as_str()) {
                score += SUBSTRING * w;
            }
            if source.contains(t.as_str()) {
                score += SOURCE * w;
            }
        }
        if score > 0.0 {
            scored.push((score, id.clone()));
        }
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored
}

/// Top-k seeds, dropping any whose score falls below 20% of the top — so a
/// dominant identifier match isn't diluted by high-frequency noise terms.
fn pick_seeds(scored: &[(f64, String)]) -> Vec<String> {
    let Some((top, _)) = scored.first() else { return Vec::new() };
    let mut seeds = Vec::new();
    for (score, id) in scored.iter().take(3) {
        if !seeds.is_empty() && *score < top * 0.2 {
            break;
        }
        seeds.push(id.clone());
    }
    seeds
}

/// p99 of the degree distribution, floored at 50 — nodes above this are hubs we
/// don't expand through (prevents god-nodes from exploding the traversal).
fn hub_threshold(degree: &HashMap<&String, usize>) -> usize {
    if degree.is_empty() {
        return 50;
    }
    let mut d: Vec<usize> = degree.values().copied().collect();
    d.sort_unstable();
    let idx = ((d.len() as f64) * 0.99) as usize;
    d.get(idx.min(d.len() - 1)).copied().unwrap_or(50).max(50)
}

fn bfs(
    adj: &HashMap<String, Vec<(String, String)>>,
    seeds: &[String],
    depth: usize,
    hub: usize,
    degree: &HashMap<&String, usize>,
) -> (HashSet<String>, Vec<(String, String, String)>) {
    let seed_set: HashSet<&String> = seeds.iter().collect();
    let mut visited: HashSet<String> = seeds.iter().cloned().collect();
    let mut frontier: HashSet<String> = seeds.iter().cloned().collect();
    let mut edges: Vec<(String, String, String)> = Vec::new();
    for _ in 0..depth {
        let mut next: HashSet<String> = HashSet::new();
        for n in &frontier {
            let deg = degree.get(n).copied().unwrap_or(0);
            if !seed_set.contains(n) && deg >= hub {
                continue; // don't transit through hubs
            }
            for (nb, rel) in adj.get(n).into_iter().flatten() {
                edges.push((n.clone(), nb.clone(), rel.clone()));
                if !visited.contains(nb) {
                    next.insert(nb.clone());
                }
            }
        }
        for n in &next {
            visited.insert(n.clone());
        }
        frontier = next;
        if frontier.is_empty() {
            break;
        }
    }
    (visited, edges)
}

/// Render the subgraph as NODE/EDGE text, seeds first then by degree, cut at the
/// token budget (~3 chars/token).
fn render(
    nodes: &HashMap<String, Node>,
    visited: &HashSet<String>,
    edges: &[(String, String, String)],
    seeds: &[String],
    token_budget: usize,
) -> String {
    let char_budget = token_budget * 3;
    let seed_set: HashSet<&String> = seeds.iter().collect();
    let mut ordered: Vec<&String> = seeds.iter().filter(|s| visited.contains(*s)).collect();
    let mut rest: Vec<&String> = visited.iter().filter(|n| !seed_set.contains(n)).collect();
    rest.sort_by_key(|n| std::cmp::Reverse(edges.iter().filter(|(a, b, _)| *a == **n || *b == **n).count()));
    ordered.extend(rest);

    let mut lines: Vec<String> = Vec::new();
    for id in &ordered {
        if let Some(n) = nodes.get(*id) {
            let src = if n.source.is_empty() { String::new() } else { format!(" src={}", n.source) };
            let ty = if n.ntype.is_empty() { String::new() } else { format!(" type={}", n.ntype) };
            lines.push(format!("NODE {}{ty}{src}", n.label));
        }
    }
    let mut seen_edges: HashSet<(String, String)> = HashSet::new();
    for (a, b, rel) in edges {
        if !visited.contains(a) || !visited.contains(b) {
            continue;
        }
        let key = if a < b { (a.clone(), b.clone()) } else { (b.clone(), a.clone()) };
        if !seen_edges.insert(key) {
            continue;
        }
        let (la, lb) = (
            nodes.get(a).map(|n| n.label.as_str()).unwrap_or(a),
            nodes.get(b).map(|n| n.label.as_str()).unwrap_or(b),
        );
        lines.push(format!("EDGE {la} --{rel}--> {lb}"));
    }

    let output = lines.join("\n");
    if output.len() > char_budget {
        let cut = output[..char_budget].rfind('\n').unwrap_or(char_budget);
        format!("{}\n... (truncated at ~{token_budget}-token budget)", &output[..cut])
    } else {
        output
    }
}

fn synth_prompt(question: &str, subgraph: &str) -> String {
    format!(
        "Answer the question using ONLY the knowledge-graph context below. The context is NODE and EDGE lines from a codebase/document knowledge graph. Cite the `src=` source for any claim. If the context is insufficient to answer, say so plainly — do not invent.\n\n\
         QUESTION: {question}\n\n\
         GRAPH CONTEXT:\n{subgraph}"
    )
}
