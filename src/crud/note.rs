use std::path::Path;

use anyhow::Result;
use oxigraph::sparql::QueryResults;

use crate::config::NamespaceConfig;
use crate::crud;

/// Create a note (memory entry) with optional relational edges.
pub fn learn(
    cwd: &Path,
    ns: &NamespaceConfig,
    text: &str,
    note_type: &str,
    domain: Option<&str>,
    project: Option<&str>,
    entity: Option<&str>,
) -> Result<String> {
    let slug = crud::slugify(text);
    let iri = crud::build_iri(ns, "note", &slug);
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);
    let now = crud::now_iso();
    let p = &ns.prefix;

    let escaped_text = crud::escape_sparql_literal(text);

    // Build relatedTo edges
    let mut edge_triples = String::new();
    if let Some(d) = domain {
        let domain_iri = crud::build_iri(ns, "domain", &crud::slugify(d));
        edge_triples.push_str(&format!("      <{iri}> {p}:relatedTo <{domain_iri}> .\n"));
    }
    if let Some(proj) = project {
        let proj_iri = crud::build_iri(ns, "project", &crud::slugify(proj));
        edge_triples.push_str(&format!("      <{iri}> {p}:relatedTo <{proj_iri}> .\n"));
    }
    if let Some(ent) = entity {
        let ent_iri = crud::build_iri(ns, "entity", &crud::slugify(ent));
        edge_triples.push_str(&format!("      <{iri}> {p}:relatedTo <{ent_iri}> .\n"));
    }

    let sparql = format!(
        "INSERT DATA {{\n\
           GRAPH <{graph}> {{\n\
             <{iri}> rdf:type {p}:Note ;\n\
               {p}:noteText \"{escaped_text}\" ;\n\
               {p}:noteType \"{note_type}\" ;\n\
               {p}:status \"active\" ;\n\
               {p}:createdAt \"{now}\"^^xsd:dateTime .\n\
         {edge_triples}\
           }}\n\
         }}"
    );

    crud::load_and_mutate(cwd, ns, &sparql)?;
    Ok(slug)
}

/// Like recall() but returns a formatted String instead of printing.
/// Used by hook injection (memory intercept, session-start).
pub fn recall_to_string(
    cwd: &Path,
    ns: &NamespaceConfig,
    keyword: Option<&str>,
    domain: Option<&str>,
) -> String {
    let p = &ns.prefix;

    let sparql = match (keyword, domain) {
        (Some(kw), Some(dom)) => {
            let kw_lower = crud::escape_sparql_literal(&kw.to_lowercase());
            let domain_iri = crud::build_iri(ns, "domain", &crud::slugify(dom));
            format!(
                "SELECT ?text ?type ?created WHERE {{\n\
                   GRAPH ?g {{\n\
                     {{ ?n a {p}:Note ; {p}:noteText ?text ; {p}:noteType ?type .\n\
                        OPTIONAL {{ ?n {p}:createdAt ?created }}\n\
                        FILTER(CONTAINS(LCASE(STR(?text)), \"{kw_lower}\"))\n\
                     }} UNION {{\n\
                        ?n a {p}:Note ; {p}:noteText ?text ; {p}:noteType ?type ; {p}:relatedTo <{domain_iri}> .\n\
                        OPTIONAL {{ ?n {p}:createdAt ?created }}\n\
                     }}\n\
                     ?n {p}:status \"active\" .\n\
                   }}\n\
                 }}"
            )
        }
        (Some(kw), None) => {
            let kw_lower = crud::escape_sparql_literal(&kw.to_lowercase());
            format!(
                "SELECT ?text ?type ?created ?extra WHERE {{\n\
                   {{\n\
                     GRAPH ?g {{\n\
                       ?n a {p}:Note ; {p}:noteText ?text ; {p}:noteType ?type ; {p}:status \"active\" .\n\
                       OPTIONAL {{ ?n {p}:createdAt ?created }}\n\
                       FILTER(CONTAINS(LCASE(STR(?text)), \"{kw_lower}\"))\n\
                     }}\n\
                   }} UNION {{\n\
                     GRAPH ?g {{\n\
                       ?n a {p}:Decision ; {p}:name ?text .\n\
                       BIND(\"decision\" AS ?type)\n\
                       OPTIONAL {{ ?n {p}:rationale ?extra }}\n\
                       OPTIONAL {{ ?n {p}:fromPlan ?created }}\n\
                       FILTER(CONTAINS(LCASE(STR(?text)), \"{kw_lower}\"))\n\
                     }}\n\
                   }} UNION {{\n\
                     GRAPH ?g {{\n\
                       ?n a {p}:Decision ; {p}:rationale ?text .\n\
                       BIND(\"decision\" AS ?type)\n\
                       OPTIONAL {{ ?n {p}:name ?extra }}\n\
                       OPTIONAL {{ ?n {p}:fromPlan ?created }}\n\
                       FILTER(CONTAINS(LCASE(STR(?text)), \"{kw_lower}\"))\n\
                     }}\n\
                   }} UNION {{\n\
                     GRAPH ?g {{\n\
                       ?n a {p}:FileChange ; {p}:filePath ?text .\n\
                       BIND(\"file-change\" AS ?type)\n\
                       OPTIONAL {{ ?n {p}:purpose ?extra }}\n\
                       OPTIONAL {{ ?n {p}:fromPlan ?created }}\n\
                       FILTER(CONTAINS(LCASE(STR(?text)), \"{kw_lower}\"))\n\
                     }}\n\
                   }} UNION {{\n\
                     GRAPH ?g {{\n\
                       ?n a {p}:AcceptanceCriteriaResult ; {p}:criterion ?text .\n\
                       BIND(\"ac-result\" AS ?type)\n\
                       OPTIONAL {{ ?n {p}:status ?extra }}\n\
                       OPTIONAL {{ ?n {p}:fromPlan ?created }}\n\
                       FILTER(CONTAINS(LCASE(STR(?text)), \"{kw_lower}\"))\n\
                     }}\n\
                   }}\n\
                 }}"
            )
        }
        (None, Some(dom)) => {
            let domain_iri = crud::build_iri(ns, "domain", &crud::slugify(dom));
            format!(
                "SELECT ?text ?type ?created WHERE {{\n\
                   GRAPH ?g {{\n\
                     ?n a {p}:Note ; {p}:noteText ?text ; {p}:noteType ?type ; {p}:status \"active\" ;\n\
                       {p}:relatedTo <{domain_iri}> .\n\
                     OPTIONAL {{ ?n {p}:createdAt ?created }}\n\
                   }}\n\
                 }}"
            )
        }
        (None, None) => return String::new(),
    };

    let results = match crud::load_and_query(cwd, ns, &sparql) {
        Ok(r) => r,
        Err(_) => return String::new(),
    };

    if let QueryResults::Solutions(solutions) = results {
        let rows: Vec<Vec<String>> = solutions
            .filter_map(|r| r.ok())
            .map(|row| {
                vec![
                    row.get("type")
                        .map(|t| crud::term_display(t.into()))
                        .unwrap_or_default(),
                    row.get("text")
                        .map(|t| crud::term_display(t.into()))
                        .unwrap_or_default(),
                    row.get("extra")
                        .map(|t| crud::term_display(t.into()))
                        .unwrap_or_else(|| "-".into()),
                    row.get("created")
                        .map(|t| crud::term_display(t.into()))
                        .unwrap_or_else(|| "-".into()),
                ]
            })
            .collect();

        if rows.is_empty() {
            return String::new();
        }

        let mut out = String::from("| type | text | context | plan/date |\n");
        out.push_str("|------|------|---------|----------|\n");
        for row in &rows {
            out.push_str(&format!("| {} | {} | {} | {} |\n", row[0], row[1], row[2], row[3]));
        }
        out
    } else {
        String::new()
    }
}

/// Search notes by keyword text match and/or domain linkage.
pub fn recall(
    cwd: &Path,
    ns: &NamespaceConfig,
    keyword: Option<&str>,
    domain: Option<&str>,
) -> Result<()> {
    if keyword.is_none() && domain.is_none() {
        eprintln!("Provide --keyword and/or --domain");
        return Ok(());
    }

    let output = recall_to_string(cwd, ns, keyword, domain);
    if output.is_empty() {
        println!("No results found.");
    } else {
        print!("{output}");
    }
    Ok(())
}

/// Increment mention count on an existing note. Returns the new count.
pub fn mention(
    cwd: &Path,
    ns: &NamespaceConfig,
    slug: &str,
    context: Option<&str>,
) -> Result<u32> {
    let p = &ns.prefix;

    // First, query current mention count
    let iri = crud::build_iri(ns, "note", slug);
    let count_sparql = format!(
        "SELECT ?count WHERE {{\n\
           GRAPH ?g {{\n\
             <{iri}> a {p}:Note .\n\
             OPTIONAL {{ <{iri}> {p}:mentionCount ?count }}\n\
           }}\n\
         }}"
    );

    let results = crud::load_and_query(cwd, ns, &count_sparql)?;
    let current_count = if let QueryResults::Solutions(solutions) = results {
        solutions
            .filter_map(|r| r.ok())
            .next()
            .and_then(|row| {
                row.get("count")
                    .map(|t| crud::term_display(t.into()))
            })
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0)
    } else {
        // Note not found
        anyhow::bail!("Note not found: {slug}");
    };

    let new_count = current_count + 1;
    let now = crud::now_iso();

    // Build update: delete old count/lastMentioned, insert new values
    let mut sparql = format!(
        "DELETE {{\n\
           GRAPH ?g {{\n\
             <{iri}> {p}:mentionCount ?oldCount .\n\
             <{iri}> {p}:lastMentioned ?oldMentioned .\n\
           }}\n\
         }} WHERE {{\n\
           GRAPH ?g {{\n\
             <{iri}> a {p}:Note .\n\
             OPTIONAL {{ <{iri}> {p}:mentionCount ?oldCount }}\n\
             OPTIONAL {{ <{iri}> {p}:lastMentioned ?oldMentioned }}\n\
           }}\n\
         }};\n\
         INSERT DATA {{\n\
           GRAPH <{graph}> {{\n\
             <{iri}> {p}:mentionCount {new_count} .\n\
             <{iri}> {p}:lastMentioned \"{now}\"^^xsd:dateTime .\n\
           }}\n\
         }}",
        graph = {
            let ws_slug = crud::workspace_slug(cwd);
            crud::workspace_graph_iri(ns, &ws_slug)
        },
    );

    // If context provided, append to note text
    if let Some(ctx) = context {
        let escaped = crud::escape_sparql_literal(ctx);
        let append_text = format!("\\n\\n[Mention {new_count}: {escaped}]");
        let ws_slug = crud::workspace_slug(cwd);
        let graph = crud::workspace_graph_iri(ns, &ws_slug);

        sparql.push_str(&format!(
            ";\n\
             DELETE {{\n\
               GRAPH <{graph}> {{\n\
                 <{iri}> {p}:noteText ?oldText .\n\
               }}\n\
             }}\n\
             INSERT {{\n\
               GRAPH <{graph}> {{\n\
                 <{iri}> {p}:noteText ?newText .\n\
               }}\n\
             }}\n\
             WHERE {{\n\
               GRAPH <{graph}> {{\n\
                 <{iri}> {p}:noteText ?oldText .\n\
                 BIND(CONCAT(STR(?oldText), \"{append_text}\") AS ?newText)\n\
               }}\n\
             }}"
        ));
    }

    crud::load_and_mutate(cwd, ns, &sparql)?;
    Ok(new_count)
}

