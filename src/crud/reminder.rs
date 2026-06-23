use std::path::Path;

use anyhow::Result;
use oxigraph::sparql::QueryResults;

use crate::config::NamespaceConfig;
use crate::crud;

pub fn add(
    cwd: &Path,
    ns: &NamespaceConfig,
    name: &str,
    surface_at: &str,
    due_date: Option<&str>,
) -> Result<String> {
    let slug = crud::slugify(name);
    let iri = crud::build_iri(ns, "reminder", &slug);
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);
    let now = crud::now_iso();
    let p = &ns.prefix;
    let name = crud::escape_sparql_literal(name);

    // Optional dueDate triple — display/back-compat only; surfacing is driven by resurfaceAt.
    let due_triple = match due_date {
        Some(d) => format!(
            "               {p}:dueDate \"{}\"^^xsd:date ;\n",
            crud::escape_sparql_literal(d)
        ),
        None => String::new(),
    };

    let sparql = format!(
        "INSERT DATA {{\n\
           GRAPH <{graph}> {{\n\
             <{iri}> rdf:type {p}:Reminder ;\n\
               {p}:name \"{name}\" ;\n\
               {p}:resurfaceAt \"{surface_at}\"^^xsd:dateTime ;\n\
{due_triple}               {p}:createdAt \"{now}\"^^xsd:dateTime ;\n\
               {p}:lastActive \"{now}\"^^xsd:dateTime .\n\
           }}\n\
         }}"
    );

    crud::load_and_mutate(cwd, ns, &sparql)?;
    Ok(slug)
}

pub fn list(cwd: &Path, ns: &NamespaceConfig) -> Result<()> {
    let p = &ns.prefix;
    let sparql = format!(
        "SELECT ?name ?resurfaceAt WHERE {{\n\
           GRAPH ?g {{\n\
             ?r a {p}:Reminder ;\n\
               {p}:name ?name ;\n\
               {p}:resurfaceAt ?resurfaceAt .\n\
           }}\n\
         }}\n\
         ORDER BY ?resurfaceAt"
    );

    let results = crud::load_and_query(cwd, ns, &sparql)?;
    if let QueryResults::Solutions(solutions) = results {
        let rows: Vec<Vec<String>> = solutions
            .filter_map(|r| r.ok())
            .map(|row| {
                vec![
                    row.get("name").map(|t| crud::term_display(t.into())).unwrap_or_default(),
                    row.get("resurfaceAt").map(|t| crud::term_display(t.into())).unwrap_or_default(),
                ]
            })
            .collect();

        if rows.is_empty() {
            println!("No reminders.");
            return Ok(());
        }

        println!("| name | surfaces at |");
        println!("|------|-------------|");
        for row in &rows {
            println!("| {} | {} |", row[0], row[1]);
        }
    }
    Ok(())
}

pub fn remove(cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<()> {
    let iri = crud::build_iri(ns, "reminder", slug);

    // Hard delete: remove all triples about this reminder
    let sparql = format!(
        "DELETE WHERE {{ GRAPH ?g {{ <{iri}> ?p ?o }} }}"
    );

    crud::load_and_mutate(cwd, ns, &sparql)
}
