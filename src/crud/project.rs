use std::path::Path;

use anyhow::{Context, Result};
use oxigraph::sparql::QueryResults;

use crate::config::{NamespaceConfig, ProtocolConfig};
use crate::crud;

pub fn add(
    cwd: &Path,
    ns: &NamespaceConfig,
    name: &str,
    status: &str,
    path: Option<&str>,
) -> Result<String> {
    add_with_stage(cwd, ns, name, status, path, None)
}

/// Like [`add`], but also records the project's protocol lifecycle stage.
pub fn add_with_stage(
    cwd: &Path,
    ns: &NamespaceConfig,
    name: &str,
    status: &str,
    path: Option<&str>,
    stage: Option<&str>,
) -> Result<String> {
    let slug = crud::slugify(name);
    let iri = crud::build_iri(ns, "project", &slug);
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);
    let ws_iri = crud::build_iri(ns, "workspace", &ws_slug);
    let now = crud::now_iso();
    let p = &ns.prefix;
    let project_path = path
        .map(|s| s.to_string())
        .unwrap_or_else(|| cwd.to_string_lossy().to_string());

    let name = crud::escape_sparql_literal(name);
    let project_path = crud::escape_sparql_literal(&project_path);
    let stage_triple = match stage {
        Some(s) => format!("               {p}:stage \"{}\" ;\n", crud::escape_sparql_literal(s)),
        None => String::new(),
    };

    let sparql = format!(
        "INSERT DATA {{\n\
           GRAPH <{graph}> {{\n\
             <{iri}> rdf:type {p}:Project ;\n\
               {p}:name \"{name}\" ;\n\
               {p}:status \"{status}\" ;\n\
               {p}:path \"{project_path}\" ;\n\
{stage_triple}               {p}:createdAt \"{now}\"^^xsd:dateTime ;\n\
               {p}:lastActive \"{now}\"^^xsd:dateTime ;\n\
               {p}:belongsTo <{ws_iri}> .\n\
           }}\n\
         }}"
    );

    crud::load_and_mutate(cwd, ns, &sparql)?;

    // Auto-create domain trigger with path matching (filesystem-first, no keywords by default)
    auto_create_domain(cwd, &name, &project_path)?;

    // Link project to its domain in the graph
    let domain_slug = crud::slugify(&name);
    let domain_iri = crud::build_iri(ns, "domain", &domain_slug);
    let link_sparql = format!(
        "INSERT DATA {{ GRAPH <{graph}> {{ <{iri}> {p}:hasDomain <{domain_iri}> }} }}"
    );
    let _ = crud::load_and_mutate(cwd, ns, &link_sparql);

    Ok(slug)
}

/// Resolve a new project's artifact folder from the protocol, create it (and its
/// optional context doc), and return (workspace-relative folder, stage name).
/// Returns Ok(None) when the protocol is disabled or defines no stages — the caller
/// then falls back to an explicit --path.
pub fn provision_folder(
    cwd: &Path,
    protocol: &ProtocolConfig,
    name: &str,
    slug: &str,
    stage: Option<&str>,
) -> Result<Option<(String, String)>> {
    if !protocol.enabled {
        return Ok(None);
    }
    let Some(stage_def) = protocol.stage_for(stage) else {
        return Ok(None);
    };
    let rel_folder = stage_def.folder.replace("{slug}", slug);

    // Workspace root = the directory containing .base/ (fallback: cwd).
    let ws_root = crate::config::find_workspace_base(cwd)
        .and_then(|b| b.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| cwd.to_path_buf());
    let abs_folder = ws_root.join(&rel_folder);
    std::fs::create_dir_all(&abs_folder)
        .with_context(|| format!("creating project folder {}", abs_folder.display()))?;

    // Context doc — created once, never overwritten.
    if let Some(doc) = &stage_def.context_doc {
        let doc_path = abs_folder.join(doc);
        if !doc_path.exists() {
            let now = crud::now_iso();
            let body = format!(
                "---\ntype: context\nstatus: active\ntags: [{slug}]\n---\n\n\
                 # {name}\n\n\
                 Project context — this folder is the artifact home; touching it keeps the project fresh.\n\n\
                 ## Goal\n\n## Status\nCreated {now} \u{00b7} stage: {stage_name}\n",
                slug = slug,
                name = name,
                now = now,
                stage_name = stage_def.name,
            );
            std::fs::write(&doc_path, body)
                .with_context(|| format!("writing context doc {}", doc_path.display()))?;
        }
    }

    Ok(Some((rel_folder, stage_def.name.clone())))
}

/// Auto-create a domain trigger entry in the nearest domains.toml.
/// Default: path-based matching. No keywords unless user adds them later.
fn auto_create_domain(cwd: &Path, project_name: &str, project_path: &str) -> Result<()> {
    // Add a path trigger via the existing add_trigger mechanism
    crate::domain::add_trigger(cwd, project_name, None, Some(project_path))?;
    Ok(())
}

pub fn list(cwd: &Path, ns: &NamespaceConfig) -> Result<()> {
    let p = &ns.prefix;
    let sparql = format!(
        "SELECT ?name ?status ?priority ?lastActive WHERE {{\n\
           GRAPH ?g {{\n\
             ?proj a {p}:Project ;\n\
               {p}:name ?name ;\n\
               {p}:status ?status .\n\
             OPTIONAL {{ ?proj {p}:priority ?priority }}\n\
             OPTIONAL {{ ?proj {p}:lastActive ?lastActive }}\n\
           }}\n\
         }}\n\
         ORDER BY ?name"
    );

    let results = crud::load_and_query(cwd, ns, &sparql)?;
    if let QueryResults::Solutions(solutions) = results {
        let vars: Vec<String> = solutions
            .variables()
            .iter()
            .map(|v| v.as_str().to_string())
            .collect();
        let rows: Vec<Vec<String>> = solutions
            .filter_map(|r| r.ok())
            .map(|row| {
                vars.iter()
                    .map(|v| {
                        row.get(v.as_str())
                            .map(|t| crud::term_display(t.into()))
                            .unwrap_or_else(|| "-".into())
                    })
                    .collect()
            })
            .collect();

        if rows.is_empty() {
            println!("No projects found.");
            return Ok(());
        }

        println!("| {} |", vars.join(" | "));
        println!("|{}|", vars.iter().map(|_| "---").collect::<Vec<_>>().join("|"));
        for row in &rows {
            println!("| {} |", row.join(" | "));
        }
    }
    Ok(())
}

pub fn get(cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<()> {
    let iri = crud::build_iri(ns, "project", slug);
    let sparql = format!(
        "SELECT ?pred ?obj WHERE {{\n\
           GRAPH ?g {{\n\
             <{iri}> ?pred ?obj .\n\
           }}\n\
         }}"
    );

    let results = crud::load_and_query(cwd, ns, &sparql)?;
    if let QueryResults::Solutions(solutions) = results {
        let rows: Vec<(String, String)> = solutions
            .filter_map(|r| r.ok())
            .map(|row| {
                let pred = row
                    .get("pred")
                    .map(|t| crud::term_display(t.into()))
                    .unwrap_or_default();
                let obj = row
                    .get("obj")
                    .map(|t| crud::term_display(t.into()))
                    .unwrap_or_default();
                (pred, obj)
            })
            .collect();

        if rows.is_empty() {
            eprintln!("Project '{slug}' not found.");
            return Ok(());
        }

        println!("Project: {slug}");
        for (pred, obj) in &rows {
            // Skip rdf:type display name — show it as "type"
            let label = if pred == "type" {
                "type".to_string()
            } else {
                pred.clone()
            };
            println!("  {label}: {obj}");
        }
    }
    Ok(())
}

pub fn update(
    cwd: &Path,
    ns: &NamespaceConfig,
    slug: &str,
    status: Option<&str>,
    blocked_by: Option<&str>,
    next_action: Option<&str>,
) -> Result<()> {
    let iri = crud::build_iri(ns, "project", slug);
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);
    let now = crud::now_iso();
    let p = &ns.prefix;

    let mut updates = Vec::new();

    if let Some(s) = status {
        updates.push(crud::field_update(&graph, &iri, &format!("{p}:status"), &format!("\"{s}\"")));
    }
    if let Some(b) = blocked_by {
        updates.push(crud::field_update(&graph, &iri, &format!("{p}:blockedBy"), &format!("\"{b}\"")));
    }
    if let Some(n) = next_action {
        updates.push(crud::field_update(&graph, &iri, &format!("{p}:nextAction"), &format!("\"{n}\"")));
    }

    // Always update timestamps
    updates.push(crud::field_update(
        &graph,
        &iri,
        &format!("{p}:updatedAt"),
        &format!("\"{now}\"^^xsd:dateTime"),
    ));
    updates.push(crud::field_update(
        &graph,
        &iri,
        &format!("{p}:lastActive"),
        &format!("\"{now}\"^^xsd:dateTime"),
    ));

    let sparql = updates.join(" ;\n");
    crud::load_and_mutate(cwd, ns, &sparql)
}
