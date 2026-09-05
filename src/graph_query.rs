//! `base graph query "<question>"` — GraphRAG retrieval + synthesis over the
//! workspace graph. Mirrors graphify's retrieval algorithm (IDF-weighted keyword
//! seeding → hub-avoiding BFS → token-budgeted subgraph render) but does the
//! synthesis IN the command via the no-key LLM keystone, over base's unified
//! graph (LLM concepts + curated entities), instead of punting to an external
//! agent over MCP.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use oxigraph::sparql::QueryResults;

use crate::config::NamespaceConfig;
use crate::crud;

pub struct Node {
    pub label: String,
    pub ntype: String,
    pub source: String,
    pub summary: String,
}

/// The record a bare name denotes when several kinds answer to it, in preference
/// order (see `crud::build_iri` for the kinds). `concept` stays first because it is
/// the only kind 0.13.17 ever resolved: on a store where `graph extract` has run,
/// `neighbors alpha` must keep naming `concept/alpha`, not switch to `domain/alpha`
/// underneath the user. The hub kinds follow for the stores that never ran extract.
/// Shared by `graph_tools::resolve` and the query seeder, so a name means the same
/// record everywhere — and this is a preference order, never an allowlist: a kind
/// missing here still resolves, it just ranks after these six.
pub const RESOLVE_KINDS: [&str; 6] = ["concept", "domain", "project", "entity", "task", "decision"];

/// `<http://…#domain/base>` → `domain`: the kind base addresses a record by, read
/// off its IRI. `None` for an id with no `kind/slug` tail (a code entity, a literal).
pub fn iri_kind(id: &str) -> Option<&str> {
    let bare = id.strip_prefix('<').unwrap_or(id);
    let bare = bare.strip_suffix('>').unwrap_or(bare);
    let (head, _slug) = bare.rsplit_once('/')?;
    let kind = head.rsplit(['#', '/']).next()?;
    if kind.is_empty() { None } else { Some(kind) }
}

/// Where a record falls in `RESOLVE_KINDS`: by its IRI kind, else by its RDF class
/// (a `graph extract` concept carries its `conceptType` as `ntype`, which is why the
/// IRI is asked first). Anything outside the list ranks after it.
pub fn kind_rank(id: &str, node: Option<&Node>) -> usize {
    let kind = iri_kind(id)
        .map(|k| k.to_lowercase())
        .or_else(|| node.map(|n| n.ntype.to_lowercase()))
        .unwrap_or_default();
    RESOLVE_KINDS.iter().position(|k| *k == kind).unwrap_or(RESOLVE_KINDS.len())
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
    let degree: HashMap<&String, usize> = adj.iter().map(|(k, v)| (k, v.len())).collect();
    let scored = score_nodes(&nodes, &terms, &degree);
    let seeds = pick_seeds(&scored);
    if seeds.is_empty() {
        println!("No matching nodes found for: {question}");
        return Ok(());
    }

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

/// The concept map and its undirected adjacency list, as `load_graph` returns them.
pub type GraphMaps = (HashMap<String, Node>, HashMap<String, Vec<(String, String)>>);

/// Body predicates, in preference order, that stand in for a missing `ops:name`.
/// A record written by `base learn`/`base rule add` carries its text here and no
/// name; without this it would load as an unlabelled node or not at all.
const BODY_PREDS: [&str; 5] = ["noteText", "ruleText", "description", "text", "summary"];

/// First line of a body value, clipped, as a display label.
fn body_label(s: &str) -> String {
    let line = s.trim().lines().next().unwrap_or("").trim();
    line.chars().take(90).collect()
}

/// `<http://…#entity/renda-group>` → `("entity", "renda group")`. The IRI scheme is
/// `{ns.uri}{kind}/{slug}` (see `crud::build_iri`), so an edge endpoint that carries
/// no triples of its own still names itself.
fn kind_and_label_from_iri(id: &str, ns: &NamespaceConfig) -> Option<(String, String)> {
    let bare = id.strip_prefix('<')?.strip_suffix('>')?;
    let loc = bare.strip_prefix(ns.uri.as_str())?;
    let (kind, slug) = loc.split_once('/')?;
    if kind.is_empty() || slug.is_empty() {
        return None;
    }
    Some((kind.to_string(), slug.replace('-', " ")))
}

/// Pull nodes and the concept edge-set from graph.nq into memory.
///
/// Nodes are every `ops:name`-bearing subject plus every typed subject (labelled
/// from its body text or its IRI slug), plus any edge endpoint that appears
/// nowhere as a subject. Edges are every ops-namespace predicate whose object is
/// an IRI, plus the reified semantic edges written by `graph extract`
/// (`ops:from`/`ops:to`). Traversal is undirected so BFS reaches both sides.
pub fn load_graph(
    cwd: &Path,
    ns: &NamespaceConfig,
    include_ast: bool,
) -> Result<GraphMaps> {
    let p = &ns.prefix;

    // One parse, four queries. `crud::load_and_query` re-reads and re-parses graph.nq
    // on every call, so the old two-query loader paid for the 13 MB store twice and
    // this four-query one would pay four times.
    let base_dir = crate::config::find_workspace_base(cwd)
        .context("no .base/ directory found. Use --global for global rules, or run `base scaffold` to create a workspace.")?;
    let store = crate::store::load_graph(&base_dir.join("graph.nq"))?;
    let pfx = crud::prefixes(ns);
    let ask = |q: &str| crate::store::query(&store, &format!("{pfx}\n{q}"));

    // `conceptType` is written only by `graph extract`; a store that has never run it
    // has none, so fall back to the RDF class every record carries. Without this every
    // node's type is blank and two records sharing a name are indistinguishable.
    let node_q = format!(
        "SELECT ?s ?label ?type ?rdftype ?src ?summary WHERE {{ GRAPH ?g {{\n\
           ?s {p}:name ?label .\n\
           OPTIONAL {{ ?s {p}:conceptType ?type }}\n\
           OPTIONAL {{ ?s a ?rdftype }}\n\
           OPTIONAL {{ ?s {p}:sourceDoc ?src }}\n\
           OPTIONAL {{ ?s {p}:summary ?summary }}\n\
         }} }}"
    );
    let mut nodes: HashMap<String, Node> = HashMap::new();
    if let QueryResults::Solutions(sols) = ask(&node_q)? {
        for row in sols.filter_map(|r| r.ok()) {
            let Some(s) = row.get("s").map(|t| t.to_string()) else { continue };
            nodes.entry(s).or_insert_with(|| Node {
                label: row.get("label").map(|t| crud::term_display(t.into())).unwrap_or_default(),
                ntype: row
                    .get("type")
                    .or_else(|| row.get("rdftype"))
                    .map(|t| crud::term_display(t.into()))
                    .unwrap_or_default(),
                source: row.get("src").map(|t| crud::term_display(t.into())).unwrap_or_default(),
                summary: row.get("summary").map(|t| crud::term_display(t.into())).unwrap_or_default(),
            });
        }
    }

    // Typed subjects that carry no `ops:name`: 58% of a real store (records written
    // by learn/rule/task/decision, plus every PAUL acceptance-criteria and
    // file-change). They are the far end of most edges, so without them the graph
    // loads adjacency into nodes nobody can see.
    let body_opts: String = BODY_PREDS
        .iter()
        .map(|b| format!("           OPTIONAL {{ ?s {p}:{b} ?{b} }}\n"))
        .collect();
    let body_vars: String = BODY_PREDS.iter().map(|b| format!(" ?{b}")).collect();
    let typed_q = format!(
        "SELECT ?s ?type ?src{body_vars} WHERE {{ GRAPH ?g {{\n\
           ?s a ?type .\n\
           FILTER NOT EXISTS {{ ?s {p}:name ?any }}\n\
           OPTIONAL {{ ?s {p}:sourceDoc ?src }}\n\
         {body_opts}\
         }} }}"
    );
    if let QueryResults::Solutions(sols) = ask(&typed_q)? {
        for row in sols.filter_map(|r| r.ok()) {
            let Some(s) = row.get("s").map(|t| t.to_string()) else { continue };
            if nodes.contains_key(&s) {
                continue;
            }
            let body = BODY_PREDS
                .iter()
                .find_map(|b| row.get(*b).map(|t| crud::term_display(t.into())))
                .map(|v| body_label(&v))
                .filter(|v| !v.is_empty());
            let from_iri = kind_and_label_from_iri(&s, ns);
            let label = body
                .or_else(|| from_iri.as_ref().map(|(_, l)| l.clone()))
                .unwrap_or_default();
            if label.is_empty() {
                continue; // nothing to show it under — leave it out rather than print an IRI
            }
            nodes.insert(
                s,
                Node {
                    label,
                    ntype: row
                        .get("type")
                        .map(|t| crud::term_display(t.into()))
                        .or_else(|| from_iri.map(|(k, _)| k))
                        .unwrap_or_default(),
                    source: row.get("src").map(|t| crud::term_display(t.into())).unwrap_or_default(),
                    summary: String::new(),
                },
            );
        }
    }

    let mut adj: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let push = |adj: &mut HashMap<String, Vec<(String, String)>>, a: String, b: String, rel: String| {
        adj.entry(a.clone()).or_default().push((b.clone(), rel.clone()));
        adj.entry(b).or_default().push((a, rel)); // undirected for reachability
    };

    // Direct ontology relations: `?s ops:references ?o`. Any ops-namespace predicate
    // whose object is an IRI is a node→node edge — a literal object can never match
    // isIRI, so this cannot pull in `hasSection`/`hasTag`-style string properties, and
    // it does not go stale when ops.ttl gains a property. The three excluded predicates
    // are the reification scaffolding of a SemanticEdge, loaded as one edge below.
    let direct_q = format!(
        "SELECT ?a ?p ?b WHERE {{ GRAPH ?g {{\n\
           ?a ?p ?b .\n\
           FILTER(isIRI(?a) && isIRI(?b))\n\
           FILTER(STRSTARTS(STR(?p), \"{u}\"))\n\
           FILTER(?p != {p}:from && ?p != {p}:to && ?p != {p}:relation)\n\
         }} }}",
        u = ns.uri
    );
    if let QueryResults::Solutions(sols) = ask(&direct_q)? {
        for row in sols.filter_map(|r| r.ok()) {
            let (Some(a), Some(b)) = (row.get("a").map(|t| t.to_string()), row.get("b").map(|t| t.to_string())) else { continue };
            let Some(rel) = row.get("p").map(|t| crud::term_display(t.into())) else { continue };
            push(&mut adj, a, b, rel);
        }
    }

    let edge_q = format!(
        "SELECT ?a ?b ?rel WHERE {{ GRAPH ?g {{\n\
           ?e a {p}:SemanticEdge ; {p}:from ?a ; {p}:to ?b .\n\
           OPTIONAL {{ ?e {p}:relation ?rel }}\n\
         }} }}"
    );
    if let QueryResults::Solutions(sols) = ask(&edge_q)? {
        for row in sols.filter_map(|r| r.ok()) {
            let (Some(a), Some(b)) = (row.get("a").map(|t| t.to_string()), row.get("b").map(|t| t.to_string())) else { continue };
            let rel = row.get("rel").map(|t| crud::term_display(t.into())).unwrap_or_else(|| "relates_to".into());
            push(&mut adj, a, b, rel);
        }
    }

    // An endpoint that is only ever an object (`entity/…`, `document/…`) carries no
    // triples of its own, so neither node query found it. Its IRI slug is a real name
    // — base addresses the record by it. Without this the other end of ~44% of edges
    // renders as a raw URL.
    let dangling: Vec<String> = adj.keys().filter(|id| !nodes.contains_key(*id)).cloned().collect();
    for id in dangling {
        let Some((kind, label)) = kind_and_label_from_iri(&id, ns) else { continue };
        nodes.insert(
            id,
            Node { label, ntype: kind, source: String::new(), summary: String::new() },
        );
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

    // Adjacency in id order, not store order: every reader (neighbors, get-node,
    // path, the query BFS) prints or walks these lists, and the same store must
    // produce the same output on every run.
    for list in adj.values_mut() {
        list.sort();
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

/// Text-score every node against the question, highest first, in a TOTAL order.
///
/// The score is text only (exact, prefix, substring, IDF-weighted). Ties are the
/// normal case, not the edge case: eleven records on a real store are labelled
/// exactly `basemode` (one Domain hub, one Project, nine Handoffs) and all score
/// the same. `nodes` is a HashMap, so before ties had an order the seed set came
/// out of hash iteration and one question returned anywhere from 3 to 323 nodes
/// on the same store. Ties now break on structure: degree, then hub kind, then id.
fn score_nodes(
    nodes: &HashMap<String, Node>,
    terms: &[String],
    degree: &HashMap<&String, usize>,
) -> Vec<(f64, String)> {
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
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                let da = degree.get(&a.1).copied().unwrap_or(0);
                let db = degree.get(&b.1).copied().unwrap_or(0);
                db.cmp(&da)
            })
            .then_with(|| kind_rank(&a.1, nodes.get(&a.1)).cmp(&kind_rank(&b.1, nodes.get(&b.1))))
            .then_with(|| a.1.cmp(&b.1))
    });
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
        // Walk the frontier in id order so the edge list, and therefore what the
        // budget cut keeps, is the same on every run of the same store.
        let mut order: Vec<&String> = frontier.iter().collect();
        order.sort();
        for n in order {
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
    // Busiest first, id as the tie-break: `visited` is a set, and two nodes with
    // the same edge count used to land in hash order.
    rest.sort_by_cached_key(|n| {
        let touching = edges.iter().filter(|(a, b, _)| *a == **n || *b == **n).count();
        (std::cmp::Reverse(touching), (*n).clone())
    });
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

#[cfg(test)]
mod edge_loading_tests {
    use super::*;
    use std::fs;

    const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

    fn ns() -> NamespaceConfig {
        NamespaceConfig::default()
    }

    /// A workspace whose `.base/graph.nq` holds exactly `body`.
    fn workspace(body: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join(".base");
        fs::create_dir_all(&base).unwrap();
        fs::write(base.join("graph.nq"), body).unwrap();
        dir
    }

    fn load(dir: &tempfile::TempDir) -> GraphMaps {
        load_graph(dir.path(), &ns(), false).unwrap()
    }

    fn id(local: &str) -> String {
        format!("<{}{}>", ns().uri, local)
    }

    /// Relations on `a`'s adjacency list, sorted — the shape every assert reads.
    fn rels(adj: &HashMap<String, Vec<(String, String)>>, a: &str) -> Vec<String> {
        let mut v: Vec<String> = adj
            .get(&id(a))
            .map(|v| v.iter().map(|(_, r)| r.clone()).collect())
            .unwrap_or_default();
        v.sort();
        v
    }

    /// Two named projects wired by a direct ontology predicate, plus the literal
    /// properties that must NOT become edges.
    fn direct_body() -> String {
        let u = ns().uri;
        let g = crate::crud::workspace_graph_iri(&ns(), "t");
        let mut b = String::new();
        b += &format!("<{u}project/alpha> <{u}name> \"alpha\" <{g}> .\n");
        b += &format!("<{u}project/alpha> <{RDF_TYPE}> <{u}Project> <{g}> .\n");
        b += &format!("<{u}domain/beta> <{u}name> \"beta\" <{g}> .\n");
        b += &format!("<{u}domain/beta> <{RDF_TYPE}> <{u}Domain> <{g}> .\n");
        b += &format!("<{u}project/alpha> <{u}hasDomain> <{u}domain/beta> <{g}> .\n");
        // Literal-object properties: heading and tag strings, not node→node edges.
        b += &format!("<{u}project/alpha> <{u}hasSection> \"Overview\" <{g}> .\n");
        b += &format!("<{u}project/alpha> <{u}hasTag> \"launch\" <{g}> .\n");
        b += &format!("<{u}project/alpha> <{u}description> \"a project\" <{g}> .\n");
        b
    }

    /// The reified shape `base graph extract` writes.
    fn semantic_body() -> String {
        let u = ns().uri;
        let g = crate::crud::workspace_graph_iri(&ns(), "t");
        let mut b = String::new();
        b += &format!("<{u}concept/one> <{u}name> \"one\" <{g}> .\n");
        b += &format!("<{u}concept/two> <{u}name> \"two\" <{g}> .\n");
        b += &format!("<{u}edge/e1> <{RDF_TYPE}> <{u}SemanticEdge> <{g}> .\n");
        b += &format!("<{u}edge/e1> <{u}from> <{u}concept/one> <{g}> .\n");
        b += &format!("<{u}edge/e1> <{u}to> <{u}concept/two> <{g}> .\n");
        b += &format!("<{u}edge/e1> <{u}relation> \"explains\" <{g}> .\n");
        b
    }

    // ── AC7 case 1: direct-predicate edges only ──────────────────────────────

    #[test]
    fn direct_predicate_edges_load_and_carry_their_predicate_name() {
        let dir = workspace(&direct_body());
        let (nodes, adj) = load(&dir);
        assert!(nodes.contains_key(&id("project/alpha")));
        assert!(nodes.contains_key(&id("domain/beta")));
        // The relation is the predicate, so output says WHY two things connect.
        assert_eq!(rels(&adj, "project/alpha"), vec!["hasDomain"]);
        // Undirected: reachable from either end.
        assert_eq!(rels(&adj, "domain/beta"), vec!["hasDomain"]);
    }

    #[test]
    fn literal_object_properties_never_become_edges() {
        let dir = workspace(&direct_body());
        let (_, adj) = load(&dir);
        // hasSection is 9.8k triples on a real store and every object is a string.
        // If it ever loads as an edge, centrality becomes "longest document wins".
        let all: Vec<String> = adj.values().flatten().map(|(_, r)| r.clone()).collect();
        assert!(!all.iter().any(|r| r == "hasSection"), "hasSection loaded as an edge");
        assert!(!all.iter().any(|r| r == "hasTag"), "hasTag loaded as an edge");
        assert!(!all.iter().any(|r| r == "description"));
        assert!(!adj.contains_key("\"Overview\""));
    }

    // ── AC7 case 2: SemanticEdge only ────────────────────────────────────────

    #[test]
    fn semantic_edges_still_load_unchanged() {
        let dir = workspace(&semantic_body());
        let (nodes, adj) = load(&dir);
        assert!(nodes.contains_key(&id("concept/one")));
        assert_eq!(rels(&adj, "concept/one"), vec!["explains"]);
        assert_eq!(rels(&adj, "concept/two"), vec!["explains"]);
        // The reification scaffolding is not itself an edge.
        let all: Vec<String> = adj.values().flatten().map(|(_, r)| r.clone()).collect();
        assert!(!all.iter().any(|r| r == "from" || r == "to" || r == "relation"));
        assert!(!adj.contains_key(&id("edge/e1")));
    }

    // ── AC7 case 3: both shapes in one graph (AC6) ───────────────────────────

    #[test]
    fn both_edge_shapes_coexist() {
        let dir = workspace(&format!("{}{}", direct_body(), semantic_body()));
        let (nodes, adj) = load(&dir);
        assert_eq!(rels(&adj, "project/alpha"), vec!["hasDomain"]);
        assert_eq!(rels(&adj, "concept/one"), vec!["explains"]);
        let edges: usize = adj.values().map(|v| v.len()).sum::<usize>() / 2;
        assert_eq!(edges, 2, "one direct edge + one semantic edge");
        assert!(nodes.len() >= 4);
    }

    // ── AC7 case 4: neither shape — must not panic ───────────────────────────

    #[test]
    fn a_graph_with_no_edges_loads_empty_without_panicking() {
        let u = ns().uri;
        let g = crate::crud::workspace_graph_iri(&ns(), "t");
        let dir = workspace(&format!("<{u}project/lonely> <{u}name> \"lonely\" <{g}> .\n"));
        let (nodes, adj) = load(&dir);
        assert_eq!(nodes.len(), 1);
        assert!(adj.is_empty());
    }

    #[test]
    fn an_empty_graph_leaves_nodes_empty_so_the_helpful_message_still_prints() {
        let dir = workspace("");
        let (nodes, adj) = load(&dir);
        assert!(nodes.is_empty(), "run `base graph extract` first' is gated on this");
        assert!(adj.is_empty());
    }

    // ── B1: typed subjects with no ops:name ──────────────────────────────────

    #[test]
    fn a_typed_subject_without_a_name_is_labelled_from_its_body_text() {
        let u = ns().uri;
        let g = crate::crud::workspace_graph_iri(&ns(), "t");
        let body = format!(
            "<{u}note/abc> <{RDF_TYPE}> <{u}Note> <{g}> .\n\
             <{u}note/abc> <{u}noteText> \"first line here\\nsecond line\" <{g}> .\n"
        );
        let dir = workspace(&body);
        let (nodes, _) = load(&dir);
        let n = nodes.get(&id("note/abc")).expect("typed subject loaded as a node");
        assert_eq!(n.label, "first line here", "label is the first line of the body");
        assert_eq!(n.ntype, "Note");
    }

    #[test]
    fn a_typed_subject_with_no_body_falls_back_to_its_iri_slug() {
        let u = ns().uri;
        let g = crate::crud::workspace_graph_iri(&ns(), "t");
        let body = format!("<{u}entity/renda-group> <{RDF_TYPE}> <{u}Entity> <{g}> .\n");
        let dir = workspace(&body);
        let (nodes, _) = load(&dir);
        assert_eq!(nodes.get(&id("entity/renda-group")).unwrap().label, "renda group");
    }

    #[test]
    fn ops_name_still_wins_over_body_text_and_slug() {
        let u = ns().uri;
        let g = crate::crud::workspace_graph_iri(&ns(), "t");
        let body = format!(
            "<{u}note/abc> <{RDF_TYPE}> <{u}Note> <{g}> .\n\
             <{u}note/abc> <{u}name> \"Real Name\" <{g}> .\n\
             <{u}note/abc> <{u}noteText> \"body text\" <{g}> .\n"
        );
        let dir = workspace(&body);
        let (nodes, _) = load(&dir);
        let n = nodes.get(&id("note/abc")).unwrap();
        assert_eq!(n.label, "Real Name");
        // conceptType is absent on every real store, so rdf:type has to fill in.
        assert_eq!(n.ntype, "Note");
    }

    // ── B2: endpoints that appear only as edge objects ───────────────────────

    #[test]
    fn an_endpoint_that_is_only_ever_an_object_is_named_from_its_slug() {
        let u = ns().uri;
        let g = crate::crud::workspace_graph_iri(&ns(), "t");
        // `document/railway-cli` carries no triples of its own — 828 real edges
        // point at IRIs like this, and without B2 the far end prints as a URL.
        let body = format!(
            "<{u}note/abc> <{u}name> \"a note\" <{g}> .\n\
             <{u}note/abc> <{u}references> <{u}document/railway-cli> <{g}> .\n"
        );
        let dir = workspace(&body);
        let (nodes, adj) = load(&dir);
        let far = nodes
            .get(&id("document/railway-cli"))
            .expect("dangling endpoint materialised");
        assert_eq!(far.label, "railway cli");
        assert_eq!(far.ntype, "document");
        assert_eq!(rels(&adj, "note/abc"), vec!["references"]);
    }

    #[test]
    fn an_edge_to_a_foreign_namespace_does_not_load() {
        let u = ns().uri;
        let g = crate::crud::workspace_graph_iri(&ns(), "t");
        let body = format!(
            "<{u}note/abc> <{u}name> \"a note\" <{g}> .\n\
             <{u}note/abc> <http://example.com/links> <http://example.com/thing> <{g}> .\n"
        );
        let dir = workspace(&body);
        let (_, adj) = load(&dir);
        assert!(adj.is_empty(), "only ops-namespace predicates are edges");
    }

    #[test]
    fn adjacency_lists_come_out_in_id_order_whatever_the_store_order() {
        let u = ns().uri;
        let g = crate::crud::workspace_graph_iri(&ns(), "t");
        // Written zebra-first: the loader must not hand the store's order back.
        let body = format!(
            "<{u}project/hub> <{u}name> \"hub\" <{g}> .\n\
             <{u}project/hub> <{u}references> <{u}document/zebra> <{g}> .\n\
             <{u}project/hub> <{u}references> <{u}document/apple> <{g}> .\n\
             <{u}project/hub> <{u}hasDomain> <{u}domain/mango> <{g}> .\n"
        );
        let dir = workspace(&body);
        let (_, adj) = load(&dir);
        let list = &adj[&id("project/hub")];
        let mut sorted = list.clone();
        sorted.sort();
        assert_eq!(list, &sorted);
    }
}

#[cfg(test)]
mod seed_tests {
    use super::*;

    const U: &str = "http://t.local/o#";

    fn node(label: &str, ntype: &str) -> Node {
        Node { label: label.into(), ntype: ntype.into(), source: String::new(), summary: String::new() }
    }

    type Maps = (HashMap<String, Node>, HashMap<String, Vec<(String, String)>>);

    /// Eleven records labelled exactly the same, as on a real store: one Domain
    /// hub, one Project, nine Handoffs. Every one of them scores the same on text.
    /// `rotate` varies insertion order; every map also gets its own hash seed.
    fn tied_store(rotate: usize) -> Maps {
        let mut recs: Vec<(String, Node, usize)> = vec![
            (format!("<{U}domain/basemode>"), node("basemode", "Domain"), 114),
            (format!("<{U}project/basemode>"), node("basemode", "Project"), 18),
        ];
        for i in 0..9 {
            recs.push((format!("<{U}handoff/h{i}-basemode>"), node("basemode", "Handoff"), 1));
        }
        let len = recs.len();
        recs.rotate_left(rotate % len);
        let mut nodes = HashMap::new();
        let mut adj: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for (id, n, deg) in recs {
            for k in 0..deg {
                adj.entry(id.clone()).or_default().push((format!("<{U}note/{k}>"), "relatedTo".into()));
            }
            nodes.insert(id, n);
        }
        (nodes, adj)
    }

    fn seeds_for(nodes: &HashMap<String, Node>, adj: &HashMap<String, Vec<(String, String)>>, q: &str) -> Vec<String> {
        let degree: HashMap<&String, usize> = adj.iter().map(|(k, v)| (k, v.len())).collect();
        pick_seeds(&score_nodes(nodes, &query_terms(q), &degree))
    }

    #[test]
    fn tied_seeds_resolve_to_the_same_set_every_time() {
        let mut seen: HashSet<Vec<String>> = HashSet::new();
        for i in 0..20 {
            let (nodes, adj) = tied_store(i);
            seen.insert(seeds_for(&nodes, &adj, "basemode"));
        }
        assert_eq!(seen.len(), 1, "seed set varied across runs: {seen:?}");
        let only = seen.into_iter().next().unwrap();
        assert_eq!(only[0], format!("<{U}domain/basemode>"), "the busiest record wins the tie");
        assert_eq!(only[1], format!("<{U}project/basemode>"));
    }

    #[test]
    fn a_concept_from_extract_still_outranks_the_hub_on_a_tie() {
        // 0.13.17 resolved names to concepts only; an extract-era store keeps that.
        let mut nodes = HashMap::new();
        nodes.insert(format!("<{U}domain/alpha>"), node("alpha", "Domain"));
        nodes.insert(format!("<{U}concept/alpha>"), node("alpha", "module"));
        let mut adj: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for k in 0..5 {
            adj.entry(format!("<{U}domain/alpha>")).or_default().push((format!("<{U}note/{k}>"), "relatedTo".into()));
            adj.entry(format!("<{U}concept/alpha>")).or_default().push((format!("<{U}concept/c{k}>"), "explains".into()));
        }
        let seeds = seeds_for(&nodes, &adj, "alpha");
        assert_eq!(seeds[0], format!("<{U}concept/alpha>"));
    }

    #[test]
    fn text_score_still_outranks_structure() {
        // An exact label match on a leaf beats a prefix match on a hub.
        let mut nodes = HashMap::new();
        nodes.insert(format!("<{U}domain/basemode-platform>"), node("basemode platform", "Domain"));
        nodes.insert(format!("<{U}note/n1>"), node("basemode", "Note"));
        let mut adj: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for k in 0..50 {
            adj.entry(format!("<{U}domain/basemode-platform>")).or_default().push((format!("<{U}note/{k}>"), "relatedTo".into()));
        }
        let seeds = seeds_for(&nodes, &adj, "basemode");
        assert_eq!(seeds[0], format!("<{U}note/n1>"));
    }

    /// The tied store plus cross-links, so a traversal has real branching and the
    /// render has real ties to break. Every map is built fresh, with its own hash
    /// seed and a rotated insertion order.
    fn branching_store(rotate: usize) -> Maps {
        let (mut nodes, mut adj) = tied_store(rotate);
        let hub = format!("<{U}domain/basemode>");
        for k in 0..6 {
            let n = format!("<{U}note/{k}>");
            nodes.insert(n.clone(), node(&format!("note {k}"), "Note"));
            let d = format!("<{U}decision/d{k}>");
            nodes.insert(d.clone(), node(&format!("decision {k}"), "Decision"));
            adj.entry(n.clone()).or_default().push((d.clone(), "references".into()));
            adj.entry(d.clone()).or_default().push((n.clone(), "references".into()));
            adj.entry(d).or_default().push((hub.clone(), "hasDecision".into()));
        }
        for list in adj.values_mut() {
            list.sort();
        }
        (nodes, adj)
    }

    #[test]
    fn the_whole_rendered_subgraph_is_byte_identical_across_runs() {
        // A stable seed set is not enough: `visited` is a set and the render used to
        // list equal-degree nodes in hash order, so the same query produced three
        // different outputs at one node count. Assert on the bytes, not the count.
        let mut outputs: HashSet<String> = HashSet::new();
        for i in 0..20 {
            let (nodes, adj) = branching_store(i);
            let degree: HashMap<&String, usize> = adj.iter().map(|(k, v)| (k, v.len())).collect();
            let seeds = pick_seeds(&score_nodes(&nodes, &query_terms("basemode"), &degree));
            let hub = hub_threshold(&degree);
            let (visited, edges) = bfs(&adj, &seeds, 3, hub, &degree);
            outputs.insert(render(&nodes, &visited, &edges, &seeds, 400));
        }
        assert_eq!(outputs.len(), 1, "rendered output varied across runs");
        let only = outputs.into_iter().next().unwrap();
        assert!(only.starts_with("NODE basemode type=Domain"), "{only}");
        assert!(only.contains("EDGE "), "the budget must leave room for edges: {only}");
    }

    #[test]
    fn iri_kind_reads_the_segment_before_the_slug() {
        assert_eq!(iri_kind("<http://ops-sys.local/ontology#domain/base>"), Some("domain"));
        assert_eq!(iri_kind("<http://ops-sys.local/ontology#handoff/2026-09-05-magpie-basemode>"), Some("handoff"));
        assert_eq!(iri_kind("<http://example.com/thing>"), Some("example.com"));
        assert_eq!(iri_kind("\"a literal\""), None);
    }
}
