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
    priority: Option<&str>,
    milestone_slug: Option<&str>,
) -> Result<String> {
    let task_slug_part = crud::slugify(name);
    let slug = format!("{project_slug}.{task_slug_part}");
    let task_iri = crud::build_iri(ns, "task", &slug);
    let project_iri = crud::build_iri(ns, "project", project_slug);
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);
    let now = crud::now_iso();
    let p = &ns.prefix;
    let pri = priority.unwrap_or("medium");

    // Build milestone edge if provided
    let milestone_edge = if let Some(ms) = milestone_slug {
        let ms_iri = crud::build_iri(ns, "milestone", ms);
        format!("<{ms_iri}> {p}:hasTask <{task_iri}> .\n")
    } else {
        String::new()
    };

    let name = crud::escape_sparql_literal(name);

    let sparql = format!(
        "INSERT DATA {{\n\
           GRAPH <{graph}> {{\n\
             <{task_iri}> rdf:type {p}:Task ;\n\
               {p}:name \"{name}\" ;\n\
               {p}:status \"active\" ;\n\
               {p}:priority \"{pri}\" ;\n\
               {p}:createdAt \"{now}\"^^xsd:dateTime ;\n\
               {p}:lastActive \"{now}\"^^xsd:dateTime .\n\
             <{project_iri}> {p}:hasTask <{task_iri}> .\n\
             {milestone_edge}\
           }}\n\
         }}"
    );

    crud::load_and_mutate(cwd, ns, &sparql)?;
    Ok(slug)
}

/// One task, all fields the graph holds. The stable `--json` data contract the
/// `base-board` dashboard reads. Absent predicates serialize as `null` so the key
/// set is stable across invocations.
#[derive(Debug, serde::Serialize)]
pub struct TaskRecord {
    pub id: String,
    pub name: String,
    pub status: String,
    pub priority: Option<String>,
    pub project: Option<String>,
    pub milestone: Option<String>,
    pub description: Option<String>,
    pub assignee: Option<String>,
    pub due: Option<String>,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub last_active: Option<String>,
    /// Free-form labels (multi-valued `{p}:label`) — the dashboard's tagging facet.
    /// Serializes as a (possibly empty) array; always present, never null.
    pub labels: Vec<String>,
}

/// All task→labels in the workspace, one query. Key = task slug (== `TaskRecord.id`).
fn labels_map(cwd: &Path, ns: &NamespaceConfig) -> Result<std::collections::HashMap<String, Vec<String>>> {
    let p = &ns.prefix;
    let sparql = format!(
        "SELECT ?task ?label WHERE {{\n\
           GRAPH ?g {{ ?task a {p}:Task ; {p}:label ?label }}\n\
         }}"
    );
    let results = crud::load_and_query(cwd, ns, &sparql)?;
    let mut map: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    if let QueryResults::Solutions(solutions) = results {
        for row in solutions.filter_map(|r| r.ok()) {
            let id = row.get("task").map(|t| crud::slug_of(&crud::term_display(t.into())));
            let label = row.get("label").map(|t| crud::term_display(t.into()));
            if let (Some(id), Some(label)) = (id, label) {
                map.entry(id).or_default().push(label);
            }
        }
    }
    for v in map.values_mut() {
        v.sort();
        v.dedup();
    }
    Ok(map)
}

/// Labels on one task (sorted, deduped).
fn labels_of(cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<Vec<String>> {
    let iri = crud::build_iri(ns, "task", slug);
    let p = &ns.prefix;
    let sparql = format!("SELECT ?label WHERE {{ GRAPH ?g {{ <{iri}> {p}:label ?label }} }}");
    let results = crud::load_and_query(cwd, ns, &sparql)?;
    let mut labels: Vec<String> = Vec::new();
    if let QueryResults::Solutions(solutions) = results {
        for row in solutions.filter_map(|r| r.ok()) {
            if let Some(l) = row.get("label").map(|t| crud::term_display(t.into())) {
                labels.push(l);
            }
        }
    }
    labels.sort();
    labels.dedup();
    Ok(labels)
}

/// Query task records (typed), optionally scoped to a project or milestone, and
/// filtered to tasks carrying ALL of `label_filter` (empty slice = no filter).
/// Shared core behind both the human `list` and the `--json` `list_json`.
pub fn list_data(
    cwd: &Path,
    ns: &NamespaceConfig,
    project_slug: Option<&str>,
    milestone_slug: Option<&str>,
    label_filter: &[String],
) -> Result<Vec<TaskRecord>> {
    let p = &ns.prefix;
    // Scope anchor: a milestone or project `hasTask` edge, else all tasks.
    let anchor = if let Some(ms) = milestone_slug {
        let ms_iri = crud::build_iri(ns, "milestone", ms);
        format!("<{ms_iri}> {p}:hasTask ?task .\n             ")
    } else if let Some(ps) = project_slug {
        let project_iri = crud::build_iri(ns, "project", ps);
        format!("<{project_iri}> {p}:hasTask ?task .\n             ")
    } else {
        String::new()
    };

    let sparql = format!(
        "SELECT ?task ?name ?status ?priority ?description ?assignee ?due ?created ?updated ?lastActive ?proj ?ms WHERE {{\n\
           GRAPH ?g {{\n\
             {anchor}?task a {p}:Task ;\n\
               {p}:name ?name ;\n\
               {p}:status ?status .\n\
             OPTIONAL {{ ?task {p}:priority ?priority }}\n\
             OPTIONAL {{ ?task {p}:description ?description }}\n\
             OPTIONAL {{ ?task {p}:assignee ?assignee }}\n\
             OPTIONAL {{ ?task {p}:due ?due }}\n\
             OPTIONAL {{ ?task {p}:createdAt ?created }}\n\
             OPTIONAL {{ ?task {p}:updatedAt ?updated }}\n\
             OPTIONAL {{ ?task {p}:lastActive ?lastActive }}\n\
             OPTIONAL {{ ?proj a {p}:Project ; {p}:hasTask ?task }}\n\
             OPTIONAL {{ ?ms a {p}:Milestone ; {p}:hasTask ?task }}\n\
           }}\n\
         }}\n\
         ORDER BY ?name"
    );

    let results = crud::load_and_query(cwd, ns, &sparql)?;
    let mut out: Vec<TaskRecord> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let QueryResults::Solutions(solutions) = results {
        for row in solutions.filter_map(|r| r.ok()) {
            let lit = |k: &str| row.get(k).map(|t| crud::term_display(t.into()));
            let iri = |k: &str| row.get(k).map(|t| crud::slug_of(&crud::term_display(t.into())));
            let Some(id) = iri("task") else { continue };
            if !seen.insert(id.clone()) {
                continue; // one row per task (OPTIONAL edges can multiply rows)
            }
            out.push(TaskRecord {
                id,
                name: lit("name").unwrap_or_default(),
                status: lit("status").unwrap_or_default(),
                priority: lit("priority"),
                project: iri("proj"),
                milestone: iri("ms"),
                description: lit("description"),
                assignee: lit("assignee"),
                due: lit("due"),
                created: lit("created"),
                updated: lit("updated"),
                last_active: lit("lastActive"),
                labels: Vec::new(),
            });
        }
    }
    // Attach labels (one extra query), then apply the --label AND-filter.
    let lmap = labels_map(cwd, ns)?;
    for t in out.iter_mut() {
        t.labels = lmap.get(&t.id).cloned().unwrap_or_default();
    }
    if !label_filter.is_empty() {
        out.retain(|t| label_filter.iter().all(|f| t.labels.iter().any(|l| l == f)));
    }
    Ok(out)
}

pub fn list(cwd: &Path, ns: &NamespaceConfig, project_slug: Option<&str>, milestone_slug: Option<&str>, label_filter: &[String]) -> Result<()> {
    let rows = list_data(cwd, ns, project_slug, milestone_slug, label_filter)?;
    if rows.is_empty() {
        println!("No tasks found.");
        return Ok(());
    }
    println!("| slug | name | status | priority | milestone |");
    println!("|---|---|---|---|---|");
    for t in &rows {
        println!(
            "| {} | {} | {} | {} | {} |",
            t.id,
            t.name,
            t.status,
            t.priority.as_deref().unwrap_or("-"),
            t.milestone.as_deref().unwrap_or("-"),
        );
    }
    Ok(())
}

/// `--json` list: valid JSON array on stdout, nothing else.
pub fn list_json(cwd: &Path, ns: &NamespaceConfig, project_slug: Option<&str>, milestone_slug: Option<&str>, label_filter: &[String]) -> Result<()> {
    let rows = list_data(cwd, ns, project_slug, milestone_slug, label_filter)?;
    println!("{}", serde_json::to_string_pretty(&rows)?);
    Ok(())
}

pub fn done(cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<()> {
    let task_iri = crud::build_iri(ns, "task", slug);
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);
    let now = crud::now_iso();
    let p = &ns.prefix;

    let updates = [
        crud::field_update(&graph, &task_iri, &format!("{p}:status"), "\"completed\""),
        crud::field_update(
            &graph,
            &task_iri,
            &format!("{p}:updatedAt"),
            &format!("\"{now}\"^^xsd:dateTime"),
        ),
        crud::field_update(
            &graph,
            &task_iri,
            &format!("{p}:lastActive"),
            &format!("\"{now}\"^^xsd:dateTime"),
        ),
    ];

    let sparql = updates.join(" ;\n");
    crud::load_and_mutate(cwd, ns, &sparql)
}

/// Fetch one task as a typed record (all fields + project/milestone edges).
/// `None` when no task node matches the slug.
pub fn get_data(cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<Option<TaskRecord>> {
    let iri = crud::build_iri(ns, "task", slug);
    let p = &ns.prefix;
    let sparql = format!(
        "SELECT ?name ?status ?priority ?description ?assignee ?due ?created ?updated ?lastActive ?proj ?ms WHERE {{\n\
           GRAPH ?g {{\n\
             <{iri}> a {p}:Task ;\n\
               {p}:name ?name ;\n\
               {p}:status ?status .\n\
             OPTIONAL {{ <{iri}> {p}:priority ?priority }}\n\
             OPTIONAL {{ <{iri}> {p}:description ?description }}\n\
             OPTIONAL {{ <{iri}> {p}:assignee ?assignee }}\n\
             OPTIONAL {{ <{iri}> {p}:due ?due }}\n\
             OPTIONAL {{ <{iri}> {p}:createdAt ?created }}\n\
             OPTIONAL {{ <{iri}> {p}:updatedAt ?updated }}\n\
             OPTIONAL {{ <{iri}> {p}:lastActive ?lastActive }}\n\
             OPTIONAL {{ ?proj a {p}:Project ; {p}:hasTask <{iri}> }}\n\
             OPTIONAL {{ ?ms a {p}:Milestone ; {p}:hasTask <{iri}> }}\n\
           }}\n\
         }}\n\
         LIMIT 1"
    );

    let results = crud::load_and_query(cwd, ns, &sparql)?;
    if let QueryResults::Solutions(solutions) = results
        && let Some(row) = solutions.filter_map(|r| r.ok()).next()
    {
        let lit = |k: &str| row.get(k).map(|t| crud::term_display(t.into()));
        let iri_s = |k: &str| row.get(k).map(|t| crud::slug_of(&crud::term_display(t.into())));
        return Ok(Some(TaskRecord {
            id: slug.to_string(),
            name: lit("name").unwrap_or_default(),
            status: lit("status").unwrap_or_default(),
            priority: lit("priority"),
            project: iri_s("proj"),
            milestone: iri_s("ms"),
            description: lit("description"),
            assignee: lit("assignee"),
            due: lit("due"),
            created: lit("created"),
            updated: lit("updated"),
            last_active: lit("lastActive"),
            labels: labels_of(cwd, ns, slug)?,
        }));
    }
    Ok(None)
}

pub fn get(cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<()> {
    match get_data(cwd, ns, slug)? {
        None => {
            eprintln!("Task '{slug}' not found.");
            Ok(())
        }
        Some(t) => {
            println!("Task: {}", t.id);
            println!("  name: {}", t.name);
            println!("  status: {}", t.status);
            if let Some(v) = &t.priority { println!("  priority: {v}"); }
            if let Some(v) = &t.project { println!("  project: {v}"); }
            if let Some(v) = &t.milestone { println!("  milestone: {v}"); }
            if let Some(v) = &t.description { println!("  description: {v}"); }
            if let Some(v) = &t.assignee { println!("  assignee: {v}"); }
            if let Some(v) = &t.due { println!("  due: {v}"); }
            if let Some(v) = &t.created { println!("  created: {v}"); }
            if let Some(v) = &t.updated { println!("  updated: {v}"); }
            if let Some(v) = &t.last_active { println!("  lastActive: {v}"); }
            Ok(())
        }
    }
}

/// `--json` get: one JSON document (the record, or `null` if not found) on stdout.
pub fn get_json(cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<()> {
    let rec = get_data(cwd, ns, slug)?;
    println!("{}", serde_json::to_string_pretty(&rec)?);
    Ok(())
}

/// Push a string-literal field update onto the batch (mirrors `milestone::update`).
fn push_field(updates: &mut Vec<String>, graph: &str, iri: &str, p: &str, pred: &str, val: &str) {
    updates.push(crud::field_update(
        graph,
        iri,
        &format!("{p}:{pred}"),
        &format!("\"{}\"", crud::escape_sparql_literal(val)),
    ));
}

/// Update a task's mutable fields. Scalar fields go through the shared atomic
/// `field_update`; `project`/`milestone` are edge reassignments that rewrite ONLY
/// the matching-type `hasTask` edge (a project reassignment never disturbs the
/// milestone edge and vice versa). `status` is a free string — the canonical task
/// vocabulary is `active`/`completed` (`base task done` writes `completed`); no
/// validation is imposed here, matching `milestone`/`project update`.
#[allow(clippy::too_many_arguments)]
pub fn update(
    cwd: &Path,
    ns: &NamespaceConfig,
    slug: &str,
    name: Option<&str>,
    status: Option<&str>,
    priority: Option<&str>,
    description: Option<&str>,
    assignee: Option<&str>,
    due: Option<&str>,
    project: Option<&str>,
    milestone: Option<&str>,
) -> Result<()> {
    let iri = crud::build_iri(ns, "task", slug);
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);
    let now = crud::now_iso();
    let p = &ns.prefix;

    // 1. Scalar field updates + timestamps (single atomic mutation).
    let mut updates: Vec<String> = Vec::new();
    if let Some(v) = name { push_field(&mut updates, &graph, &iri, p, "name", v); }
    if let Some(v) = status { push_field(&mut updates, &graph, &iri, p, "status", v); }
    if let Some(v) = priority { push_field(&mut updates, &graph, &iri, p, "priority", v); }
    if let Some(v) = description { push_field(&mut updates, &graph, &iri, p, "description", v); }
    if let Some(v) = assignee { push_field(&mut updates, &graph, &iri, p, "assignee", v); }
    if let Some(v) = due { push_field(&mut updates, &graph, &iri, p, "due", v); }
    updates.push(crud::field_update(&graph, &iri, &format!("{p}:updatedAt"), &format!("\"{now}\"^^xsd:dateTime")));
    updates.push(crud::field_update(&graph, &iri, &format!("{p}:lastActive"), &format!("\"{now}\"^^xsd:dateTime")));
    let sparql = updates.join(" ;\n");
    crud::load_and_mutate(cwd, ns, &sparql)?;

    // 2. Project reassignment — swap the Project→hasTask edge only.
    if let Some(new_proj) = project {
        let new_iri = crud::build_iri(ns, "project", new_proj);
        let reassign = format!(
            "DELETE {{ GRAPH ?gg {{ ?old {p}:hasTask <{iri}> }} }}\n\
             WHERE {{ GRAPH ?gg {{ ?old a {p}:Project ; {p}:hasTask <{iri}> }} }}"
        );
        crud::load_and_mutate(cwd, ns, &reassign)?;
        let insert = format!("INSERT DATA {{ GRAPH <{graph}> {{ <{new_iri}> {p}:hasTask <{iri}> }} }}");
        crud::load_and_mutate(cwd, ns, &insert)?;
    }

    // 3. Milestone reassignment — swap the Milestone→hasTask edge only.
    if let Some(new_ms) = milestone {
        let new_iri = crud::build_iri(ns, "milestone", new_ms);
        let reassign = format!(
            "DELETE {{ GRAPH ?gg {{ ?old {p}:hasTask <{iri}> }} }}\n\
             WHERE {{ GRAPH ?gg {{ ?old a {p}:Milestone ; {p}:hasTask <{iri}> }} }}"
        );
        crud::load_and_mutate(cwd, ns, &reassign)?;
        let insert = format!("INSERT DATA {{ GRAPH <{graph}> {{ <{new_iri}> {p}:hasTask <{iri}> }} }}");
        crud::load_and_mutate(cwd, ns, &insert)?;
    }

    Ok(())
}

/// Delete a task node and every edge touching it (atomic, backup-first via the
/// shared store primitive — same shape as `decision::delete`).
pub fn delete(cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<()> {
    let iri = crud::build_iri(ns, "task", slug);
    let sparql = format!(
        "DELETE WHERE {{ GRAPH ?g {{ <{iri}> ?p ?o }} }};\n\
         DELETE WHERE {{ GRAPH ?g {{ ?s ?p <{iri}> }} }}"
    );
    crud::load_and_mutate(cwd, ns, &sparql)
}

/// Attach / detach free-form labels on a task (multi-valued `{p}:label` triples) —
/// the `base-board` dashboard's tagging facet. Removals apply before additions;
/// `--add` is idempotent (INSERT DATA of an existing triple is a no-op in RDF).
/// base assigns NO meaning to label strings — conventions (`mine`, `extendly`, …)
/// live in the consumer. Touches `updatedAt`/`lastActive` like `update`/`done`.
pub fn tag(cwd: &Path, ns: &NamespaceConfig, slug: &str, add: &[String], remove: &[String]) -> Result<()> {
    let iri = crud::build_iri(ns, "task", slug);
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);
    let now = crud::now_iso();
    let p = &ns.prefix;

    for label in remove {
        let esc = crud::escape_sparql_literal(label);
        let sparql = format!("DELETE WHERE {{ GRAPH ?g {{ <{iri}> {p}:label \"{esc}\" }} }}");
        crud::load_and_mutate(cwd, ns, &sparql)?;
    }
    for label in add {
        let esc = crud::escape_sparql_literal(label);
        let sparql = format!("INSERT DATA {{ GRAPH <{graph}> {{ <{iri}> {p}:label \"{esc}\" }} }}");
        crud::load_and_mutate(cwd, ns, &sparql)?;
    }

    let bump = [
        crud::field_update(&graph, &iri, &format!("{p}:updatedAt"), &format!("\"{now}\"^^xsd:dateTime")),
        crud::field_update(&graph, &iri, &format!("{p}:lastActive"), &format!("\"{now}\"^^xsd:dateTime")),
    ]
    .join(" ;\n");
    crud::load_and_mutate(cwd, ns, &bump)
}
