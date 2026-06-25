//! Semantic layer: LLM-extracted concept nodes + relationship edges, stored in
//! per-document named graphs so re-extracting one doc cleanly DROPs and rebuilds
//! just that doc's contribution. Concept IRIs are global (derived from the name)
//! so the same concept across docs merges at query time. Every triple carries
//! provenance + confidence (edges) and a sourceDoc tag.

use std::path::Path;

use anyhow::Result;
use oxigraph::sparql::QueryResults;
use serde::Deserialize;

use crate::config::NamespaceConfig;
use crate::crud;

#[derive(Debug, Deserialize, Default)]
pub struct Extraction {
    #[serde(default)]
    pub concepts: Vec<Concept>,
    #[serde(default)]
    pub edges: Vec<Edge>,
}

#[derive(Debug, Deserialize)]
pub struct Concept {
    pub name: String,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub relation: String,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default)]
    pub provenance: String,
}

/// Concepts the semantic graph derived from a given source file (matched by
/// filename, so it works regardless of the root a doc was extracted under).
/// Returns `(name, summary)` pairs — the "linked concepts" for file-touch context.
pub fn concepts_for_file(cwd: &Path, ns: &NamespaceConfig, file_name: &str) -> Vec<(String, String)> {
    let p = &ns.prefix;
    let needle = crud::escape_sparql_literal(&file_name.to_lowercase());
    let q = format!(
        "SELECT DISTINCT ?name ?summary WHERE {{ GRAPH ?g {{\n\
           ?c a {p}:Concept ; {p}:name ?name ; {p}:sourceDoc ?doc .\n\
           OPTIONAL {{ ?c {p}:summary ?summary }}\n\
           FILTER(CONTAINS(LCASE(STR(?doc)), \"{needle}\"))\n\
         }} }} LIMIT 12"
    );
    let mut out = Vec::new();
    if let Ok(QueryResults::Solutions(sols)) = crud::load_and_query(cwd, ns, &q) {
        for row in sols.filter_map(|r| r.ok()) {
            let Some(name) = row.get("name").map(|t| crud::term_display(t.into())) else { continue };
            let summary = row.get("summary").map(|t| crud::term_display(t.into())).unwrap_or_default();
            out.push((name, summary));
        }
    }
    out
}

/// Per-document named graph IRI: `{uri}graph/semantic/{ws}/{doc-slug}`.
fn doc_graph_iri(ns: &NamespaceConfig, ws: &str, doc: &str) -> String {
    format!("{}graph/semantic/{}/{}", ns.uri, ws, crud::slugify(doc))
}

/// Idempotently replace a document's semantic contribution: DROP its named
/// graph, then INSERT the freshly extracted concepts + edges.
pub fn upsert(cwd: &Path, ns: &NamespaceConfig, source_doc: &str, ex: &Extraction) -> Result<()> {
    let ws = crud::workspace_slug(cwd);
    let g = doc_graph_iri(ns, &ws, source_doc);
    let p = &ns.prefix;
    let doc = crud::escape_sparql_literal(source_doc);

    let mut triples = String::new();

    for c in &ex.concepts {
        if c.name.trim().is_empty() {
            continue;
        }
        let iri = crud::build_iri(ns, "concept", &crud::slugify(&c.name));
        let label = crud::escape_sparql_literal(&c.name);
        let kind = crud::escape_sparql_literal(if c.kind.is_empty() { "concept" } else { &c.kind });
        let summary = crud::escape_sparql_literal(&c.summary);
        triples.push_str(&format!(
            "    <{iri}> rdf:type {p}:Concept ;\n      {p}:name \"{label}\" ;\n      {p}:conceptType \"{kind}\" ;\n      {p}:summary \"{summary}\" ;\n      {p}:sourceDoc \"{doc}\" .\n"
        ));
    }

    for (i, e) in ex.edges.iter().enumerate() {
        if e.from.trim().is_empty() || e.to.trim().is_empty() {
            continue;
        }
        let from = crud::build_iri(ns, "concept", &crud::slugify(&e.from));
        let to = crud::build_iri(ns, "concept", &crud::slugify(&e.to));
        let rel = crud::escape_sparql_literal(if e.relation.is_empty() { "relates_to" } else { &e.relation });
        let prov = crud::escape_sparql_literal(if e.provenance.is_empty() { "EXTRACTED" } else { &e.provenance });
        let conf = if e.confidence > 0.0 && e.confidence <= 1.0 { e.confidence } else { 0.5 };
        let edge_iri = crud::build_iri(
            ns,
            "semedge",
            &format!("{}-{i}", crud::slugify(&format!("{}-{}-{}", source_doc, e.from, e.to))),
        );
        triples.push_str(&format!(
            "    <{edge_iri}> rdf:type {p}:SemanticEdge ;\n      {p}:from <{from}> ;\n      {p}:to <{to}> ;\n      {p}:relation \"{rel}\" ;\n      {p}:confidence {conf} ;\n      {p}:provenance \"{prov}\" ;\n      {p}:sourceDoc \"{doc}\" .\n"
        ));
    }

    // Empty extraction: just clear any prior contribution for this doc.
    let sparql = if triples.trim().is_empty() {
        format!("DROP SILENT GRAPH <{g}>")
    } else {
        format!("DROP SILENT GRAPH <{g}>;\nINSERT DATA {{\n  GRAPH <{g}> {{\n{triples}  }}\n}}")
    };

    crud::load_and_mutate(cwd, ns, &sparql)
}
