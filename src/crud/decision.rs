use std::path::Path;

use anyhow::Result;
use oxigraph::sparql::QueryResults;

use crate::config::NamespaceConfig;
use crate::crud;

pub fn log(
    cwd: &Path,
    ns: &NamespaceConfig,
    domain: &str,
    decision_text: &str,
    rationale: &str,
    recall: Option<&str>,
) -> Result<String> {
    let slug = format!("{}.{}", crud::slugify(domain), crud::slugify(decision_text));
    let iri = crud::build_iri(ns, "decision", &slug);
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);
    let now = crud::now_iso();
    let p = &ns.prefix;

    let decision_text = crud::escape_sparql_literal(decision_text);
    let rationale = crud::escape_sparql_literal(rationale);

    let recall_triple = recall
        .map(|r| {
            let r = crud::escape_sparql_literal(r);
            format!("      {p}:recall \"{r}\" ;\n")
        })
        .unwrap_or_default();

    let domain_slug = crud::slugify(domain);
    let domain_iri = crud::build_iri(ns, "domain", &domain_slug);

    let sparql = format!(
        "INSERT DATA {{\n\
           GRAPH <{graph}> {{\n\
             <{iri}> rdf:type {p}:Decision ;\n\
               {p}:name \"{decision_text}\" ;\n\
               {p}:rationale \"{rationale}\" ;\n\
         {recall_triple}\
               {p}:status \"active\" ;\n\
               {p}:createdAt \"{now}\"^^xsd:dateTime ;\n\
               {p}:lastActive \"{now}\"^^xsd:dateTime .\n\
             <{domain_iri}> {p}:hasDecision <{iri}> .\n\
           }}\n\
         }}"
    );

    crud::load_and_mutate(cwd, ns, &sparql)?;
    Ok(slug)
}

/// One decision, all fields the graph holds. Stable `--json` contract for the dashboard.
/// `id` is the stable selector `{domain}.{decision}` that `update`/`delete` address.
#[derive(Debug, serde::Serialize)]
pub struct DecisionRecord {
    pub id: String,
    pub name: String,
    pub rationale: Option<String>,
    pub recall: Option<String>,
    pub status: Option<String>,
    pub domain: Option<String>,
    pub created: Option<String>,
    pub last_active: Option<String>,
}

/// Query decision records (typed) matching a keyword across name/rationale/recall.
/// Shared core behind human `search` and `--json` `search_json`.
pub fn search_data(cwd: &Path, ns: &NamespaceConfig, keyword: &str) -> Result<Vec<DecisionRecord>> {
    let p = &ns.prefix;
    let kw_lower = crud::escape_sparql_literal(&keyword.to_lowercase());
    let sparql = format!(
        "SELECT ?d ?name ?rationale ?recall ?status ?created ?lastActive ?domain WHERE {{\n\
           GRAPH ?g {{\n\
             ?d a {p}:Decision ;\n\
               {p}:name ?name ;\n\
               {p}:rationale ?rationale .\n\
             OPTIONAL {{ ?d {p}:recall ?recall }}\n\
             OPTIONAL {{ ?d {p}:status ?status }}\n\
             OPTIONAL {{ ?d {p}:createdAt ?created }}\n\
             OPTIONAL {{ ?d {p}:lastActive ?lastActive }}\n\
             OPTIONAL {{ ?domain {p}:hasDecision ?d }}\n\
             FILTER(\n\
               CONTAINS(LCASE(STR(?name)), \"{kw_lower}\") ||\n\
               CONTAINS(LCASE(STR(?rationale)), \"{kw_lower}\") ||\n\
               CONTAINS(LCASE(STR(?recall)), \"{kw_lower}\")\n\
             )\n\
           }}\n\
         }}"
    );

    let results = crud::load_and_query(cwd, ns, &sparql)?;
    let mut out: Vec<DecisionRecord> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let QueryResults::Solutions(solutions) = results {
        for row in solutions.filter_map(|r| r.ok()) {
            let lit = |k: &str| row.get(k).map(|t| crud::term_display(t.into()));
            let iri = |k: &str| row.get(k).map(|t| crud::slug_of(&crud::term_display(t.into())));
            let Some(id) = iri("d") else { continue };
            if !seen.insert(id.clone()) {
                continue;
            }
            out.push(DecisionRecord {
                id,
                name: lit("name").unwrap_or_default(),
                rationale: lit("rationale"),
                recall: lit("recall"),
                status: lit("status"),
                domain: iri("domain"),
                created: lit("created"),
                last_active: lit("lastActive"),
            });
        }
    }
    Ok(out)
}

pub fn search(cwd: &Path, ns: &NamespaceConfig, keyword: &str) -> Result<()> {
    let rows = search_data(cwd, ns, keyword)?;
    if rows.is_empty() {
        println!("No decisions matching '{keyword}'.");
        return Ok(());
    }
    println!("| decision | rationale | recall |");
    println!("|----------|-----------|--------|");
    for d in &rows {
        println!(
            "| {} | {} | {} |",
            d.name,
            d.rationale.as_deref().unwrap_or("-"),
            d.recall.as_deref().unwrap_or("-"),
        );
    }
    Ok(())
}

/// `--json` search: valid JSON array on stdout, nothing else.
pub fn search_json(cwd: &Path, ns: &NamespaceConfig, keyword: &str) -> Result<()> {
    let rows = search_data(cwd, ns, keyword)?;
    println!("{}", serde_json::to_string_pretty(&rows)?);
    Ok(())
}

/// Update a decision in place. Decisions carry a STABLE selector — the slug
/// `{domain}.{decision}` minted at `log` time and never mutated — so they are NOT
/// append-only and support update the same way milestones/projects do. Mutates only
/// the provided fields, through the shared atomic `field_update`. Changing `name`
/// updates the display text without moving the node (the slug/IRI is the identity).
pub fn update(
    cwd: &Path,
    ns: &NamespaceConfig,
    slug: &str,
    name: Option<&str>,
    rationale: Option<&str>,
    recall: Option<&str>,
    status: Option<&str>,
) -> Result<()> {
    let iri = crud::build_iri(ns, "decision", slug);
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);
    let now = crud::now_iso();
    let p = &ns.prefix;

    let mut updates: Vec<String> = Vec::new();
    let mut field = |pred: &str, val: &str| {
        updates.push(crud::field_update(
            &graph,
            &iri,
            &format!("{p}:{pred}"),
            &format!("\"{}\"", crud::escape_sparql_literal(val)),
        ));
    };
    if let Some(v) = name { field("name", v); }
    if let Some(v) = rationale { field("rationale", v); }
    if let Some(v) = recall { field("recall", v); }
    if let Some(v) = status { field("status", v); }

    updates.push(crud::field_update(&graph, &iri, &format!("{p}:updatedAt"), &format!("\"{now}\"^^xsd:dateTime")));
    updates.push(crud::field_update(&graph, &iri, &format!("{p}:lastActive"), &format!("\"{now}\"^^xsd:dateTime")));

    let sparql = updates.join(" ;\n");
    crud::load_and_mutate(cwd, ns, &sparql)
}

pub fn delete(cwd: &Path, ns: &NamespaceConfig, keyword: &str) -> Result<usize> {
    let p = &ns.prefix;
    let kw_lower = crud::escape_sparql_literal(&keyword.to_lowercase());

    // Find matching decisions first
    let find_sparql = format!(
        "SELECT ?d ?name WHERE {{\n\
           GRAPH ?g {{\n\
             ?d a {p}:Decision ;\n\
               {p}:name ?name .\n\
             FILTER(CONTAINS(LCASE(STR(?name)), \"{kw_lower}\"))\n\
           }}\n\
         }}"
    );

    let results = crud::load_and_query(cwd, ns, &find_sparql)?;
    let mut iris: Vec<String> = Vec::new();
    if let QueryResults::Solutions(solutions) = results {
        for row in solutions.filter_map(|r| r.ok()) {
            if let Some(d) = row.get("d") {
                iris.push(d.to_string());
            }
        }
    }

    if iris.is_empty() {
        return Ok(0);
    }

    // Delete all triples where the decision is subject or object
    for iri in &iris {
        let iri_clean = iri.trim_matches(|c| c == '<' || c == '>');
        let delete_sparql = format!(
            "DELETE WHERE {{ GRAPH ?g {{ <{iri_clean}> ?p ?o }} }};\n\
             DELETE WHERE {{ GRAPH ?g {{ ?s ?p <{iri_clean}> }} }}"
        );
        crud::load_and_mutate(cwd, ns, &delete_sparql)?;
    }

    Ok(iris.len())
}
