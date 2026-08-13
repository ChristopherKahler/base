use std::path::Path;

use anyhow::Result;
use oxigraph::sparql::QueryResults;

use crate::config::NamespaceConfig;
use crate::crud;

pub fn add(
    cwd: &Path,
    ns: &NamespaceConfig,
    project_slug: &str,
    name: &str,
    description: Option<&str>,
) -> Result<String> {
    let ms_slug_part = crud::slugify(name);
    let slug = format!("{project_slug}.{ms_slug_part}");
    let ms_iri = crud::build_iri(ns, "milestone", &slug);
    let project_iri = crud::build_iri(ns, "project", project_slug);
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);
    let now = crud::now_iso();
    let p = &ns.prefix;
    let name = crud::escape_sparql_literal(name);
    let desc = crud::escape_sparql_literal(description.unwrap_or(""));

    let sparql = format!(
        "INSERT DATA {{\n\
           GRAPH <{graph}> {{\n\
             <{ms_iri}> rdf:type {p}:Milestone ;\n\
               {p}:name \"{name}\" ;\n\
               {p}:status \"active\" ;\n\
               {p}:description \"{desc}\" ;\n\
               {p}:createdAt \"{now}\"^^xsd:dateTime ;\n\
               {p}:lastActive \"{now}\"^^xsd:dateTime ;\n\
               {p}:belongsTo <{project_iri}> .\n\
             <{project_iri}> {p}:hasMilestone <{ms_iri}> .\n\
           }}\n\
         }}"
    );

    crud::load_and_mutate(cwd, ns, &sparql)?;
    Ok(slug)
}

/// One milestone, all fields the graph holds. Stable `--json` contract for the dashboard.
#[derive(Debug, serde::Serialize)]
pub struct MilestoneRecord {
    pub id: String,
    pub name: String,
    pub status: String,
    pub description: Option<String>,
    pub project: Option<String>,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub last_active: Option<String>,
}

/// Query milestone records (typed), optionally scoped to a project. Shared core
/// behind both the human `list` and `--json` `list_json`.
pub fn list_data(cwd: &Path, ns: &NamespaceConfig, project_slug: Option<&str>) -> Result<Vec<MilestoneRecord>> {
    let p = &ns.prefix;
    let anchor = if let Some(ps) = project_slug {
        let project_iri = crud::build_iri(ns, "project", ps);
        format!("<{project_iri}> {p}:hasMilestone ?ms .\n             ")
    } else {
        String::new()
    };

    let sparql = format!(
        "SELECT ?ms ?name ?status ?description ?created ?updated ?lastActive ?proj WHERE {{\n\
           GRAPH ?g {{\n\
             {anchor}?ms a {p}:Milestone ;\n\
               {p}:name ?name ;\n\
               {p}:status ?status .\n\
             OPTIONAL {{ ?ms {p}:description ?description }}\n\
             OPTIONAL {{ ?ms {p}:createdAt ?created }}\n\
             OPTIONAL {{ ?ms {p}:updatedAt ?updated }}\n\
             OPTIONAL {{ ?ms {p}:lastActive ?lastActive }}\n\
             OPTIONAL {{ ?proj a {p}:Project ; {p}:hasMilestone ?ms }}\n\
           }}\n\
         }}\n\
         ORDER BY ?name"
    );

    let results = crud::load_and_query(cwd, ns, &sparql)?;
    let mut out: Vec<MilestoneRecord> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let QueryResults::Solutions(solutions) = results {
        for row in solutions.filter_map(|r| r.ok()) {
            let lit = |k: &str| row.get(k).map(|t| crud::term_display(t.into()));
            let iri = |k: &str| row.get(k).map(|t| crud::slug_of(&crud::term_display(t.into())));
            let Some(id) = iri("ms") else { continue };
            if !seen.insert(id.clone()) {
                continue;
            }
            out.push(MilestoneRecord {
                id,
                name: lit("name").unwrap_or_default(),
                status: lit("status").unwrap_or_default(),
                description: lit("description"),
                project: iri("proj"),
                created: lit("created"),
                updated: lit("updated"),
                last_active: lit("lastActive"),
            });
        }
    }
    Ok(out)
}

pub fn list(cwd: &Path, ns: &NamespaceConfig, project_slug: Option<&str>) -> Result<()> {
    let rows = list_data(cwd, ns, project_slug)?;
    if rows.is_empty() {
        println!("No milestones found.");
        return Ok(());
    }
    println!("| slug | name | status | description |");
    println!("|---|---|---|---|");
    for m in &rows {
        println!(
            "| {} | {} | {} | {} |",
            m.id,
            m.name,
            m.status,
            m.description.as_deref().unwrap_or("-"),
        );
    }
    Ok(())
}

/// `--json` list: valid JSON array on stdout, nothing else.
pub fn list_json(cwd: &Path, ns: &NamespaceConfig, project_slug: Option<&str>) -> Result<()> {
    let rows = list_data(cwd, ns, project_slug)?;
    println!("{}", serde_json::to_string_pretty(&rows)?);
    Ok(())
}

/// Fetch one milestone as a typed record. `None` when no node matches the slug.
pub fn get_data(cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<Option<MilestoneRecord>> {
    let iri = crud::build_iri(ns, "milestone", slug);
    let p = &ns.prefix;
    let sparql = format!(
        "SELECT ?name ?status ?description ?created ?updated ?lastActive ?proj WHERE {{\n\
           GRAPH ?g {{\n\
             <{iri}> a {p}:Milestone ;\n\
               {p}:name ?name ;\n\
               {p}:status ?status .\n\
             OPTIONAL {{ <{iri}> {p}:description ?description }}\n\
             OPTIONAL {{ <{iri}> {p}:createdAt ?created }}\n\
             OPTIONAL {{ <{iri}> {p}:updatedAt ?updated }}\n\
             OPTIONAL {{ <{iri}> {p}:lastActive ?lastActive }}\n\
             OPTIONAL {{ ?proj a {p}:Project ; {p}:hasMilestone <{iri}> }}\n\
           }}\n\
         }}\n\
         LIMIT 1"
    );

    let results = crud::load_and_query(cwd, ns, &sparql)?;
    if let QueryResults::Solutions(solutions) = results {
        for row in solutions.filter_map(|r| r.ok()) {
            let lit = |k: &str| row.get(k).map(|t| crud::term_display(t.into()));
            let iri_s = |k: &str| row.get(k).map(|t| crud::slug_of(&crud::term_display(t.into())));
            return Ok(Some(MilestoneRecord {
                id: slug.to_string(),
                name: lit("name").unwrap_or_default(),
                status: lit("status").unwrap_or_default(),
                description: lit("description"),
                project: iri_s("proj"),
                created: lit("created"),
                updated: lit("updated"),
                last_active: lit("lastActive"),
            }));
        }
    }
    Ok(None)
}

pub fn get(cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<()> {
    match get_data(cwd, ns, slug)? {
        None => {
            eprintln!("Milestone '{slug}' not found.");
            Ok(())
        }
        Some(m) => {
            println!("Milestone: {}", m.id);
            println!("  name: {}", m.name);
            println!("  status: {}", m.status);
            if let Some(v) = &m.description { println!("  description: {v}"); }
            if let Some(v) = &m.project { println!("  project: {v}"); }
            if let Some(v) = &m.created { println!("  created: {v}"); }
            if let Some(v) = &m.updated { println!("  updated: {v}"); }
            if let Some(v) = &m.last_active { println!("  lastActive: {v}"); }
            Ok(())
        }
    }
}

/// `--json` get: one JSON document (record or `null`) on stdout.
pub fn get_json(cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<()> {
    let rec = get_data(cwd, ns, slug)?;
    println!("{}", serde_json::to_string_pretty(&rec)?);
    Ok(())
}

pub fn update(
    cwd: &Path,
    ns: &NamespaceConfig,
    slug: &str,
    status: Option<&str>,
    description: Option<&str>,
) -> Result<()> {
    let iri = crud::build_iri(ns, "milestone", slug);
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);
    let now = crud::now_iso();
    let p = &ns.prefix;

    let mut updates = Vec::new();

    if let Some(s) = status {
        updates.push(crud::field_update(
            &graph,
            &iri,
            &format!("{p}:status"),
            &format!("\"{s}\""),
        ));
    }
    if let Some(d) = description {
        updates.push(crud::field_update(
            &graph,
            &iri,
            &format!("{p}:description"),
            &format!("\"{d}\""),
        ));
    }

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

/// Count tasks currently grouped under this milestone (its `hasTask` edges).
pub fn task_count(cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<usize> {
    let iri = crud::build_iri(ns, "milestone", slug);
    let p = &ns.prefix;
    let sparql = format!("SELECT ?t WHERE {{ GRAPH ?g {{ <{iri}> {p}:hasTask ?t }} }}");
    let mut n = 0;
    if let QueryResults::Solutions(sols) = crud::load_and_query(cwd, ns, &sparql)? {
        n = sols.filter_map(|r| r.ok()).count();
    }
    Ok(n)
}

/// Delete a milestone. By DEFAULT its tasks are DETACHED to project-level, not
/// deleted: removing the milestone node drops its `hasTask` edges, and every task
/// keeps the `<project> hasTask <task>` edge stamped at creation — so no task is
/// orphaned. With `force = true` the grouped tasks are cascade-deleted first.
/// Returns the number of tasks cascade-deleted (0 when detaching). Atomic,
/// backup-first via the shared store primitive.
pub fn delete(cwd: &Path, ns: &NamespaceConfig, slug: &str, force: bool) -> Result<usize> {
    let iri = crud::build_iri(ns, "milestone", slug);
    let p = &ns.prefix;
    let mut removed = 0;

    if force {
        // Cascade: delete each grouped task node + its edges.
        let find = format!("SELECT ?t WHERE {{ GRAPH ?g {{ <{iri}> {p}:hasTask ?t }} }}");
        let mut task_slugs: Vec<String> = Vec::new();
        if let QueryResults::Solutions(sols) = crud::load_and_query(cwd, ns, &find)? {
            for row in sols.filter_map(|r| r.ok()) {
                if let Some(t) = row.get("t") {
                    task_slugs.push(crud::slug_of(&crud::term_display(t.into())));
                }
            }
        }
        for ts in &task_slugs {
            crate::crud::task::delete(cwd, ns, ts)?;
            removed += 1;
        }
    }

    // Delete the milestone node + every edge touching it (detaches surviving tasks).
    let del = format!(
        "DELETE WHERE {{ GRAPH ?g {{ <{iri}> ?p ?o }} }};\n\
         DELETE WHERE {{ GRAPH ?g {{ ?s ?p <{iri}> }} }}"
    );
    crud::load_and_mutate(cwd, ns, &del)?;
    Ok(removed)
}
