use std::path::Path;

use anyhow::Result;
use oxigraph::sparql::QueryResults;

use crate::config::NamespaceConfig;
use crate::crud;

/// Active-awareness signal: the working set — every project/task NOT deferred or
/// terminal. Protocol's reconcile is the single source of "deferred" (it decays cold
/// projects), so this just renders whatever protocol left in a working state.
/// Priority 1 (highest — never dropped by budget cap).
pub fn run(cwd: &Path, ns: &NamespaceConfig) -> Result<String> {
    let p = &ns.prefix;
    let sparql = format!(
        "SELECT ?type ?name ?status ?nextAction ?blockedBy WHERE {{\n\
           GRAPH ?g {{\n\
             ?entity a ?type ;\n\
               {p}:name ?name ;\n\
               {p}:status ?status ;\n\
               {p}:lastActive ?lastActive .\n\
             OPTIONAL {{ ?entity {p}:nextAction ?nextAction }}\n\
             OPTIONAL {{ ?entity {p}:blockedBy ?blockedBy }}\n\
             FILTER(?type IN ({p}:Project, {p}:App, {p}:Framework, {p}:TrackingProject, {p}:Task))\n\
             FILTER(?status NOT IN (\"deferred\", \"complete\", \"completed\", \"archived\"))\n\
           }}\n\
         }}\n\
         ORDER BY DESC(?lastActive)"
    );

    let results = crud::load_and_query(cwd, ns, &sparql)?;
    let QueryResults::Solutions(solutions) = results else {
        return Ok(String::new());
    };

    let rows: Vec<(String, String, String, String, String)> = solutions
        .filter_map(|r| r.ok())
        .map(|row| {
            (
                row.get("type").map(|t| crud::term_display(t.into())).unwrap_or_default(),
                row.get("name").map(|t| crud::term_display(t.into())).unwrap_or_default(),
                row.get("status").map(|t| crud::term_display(t.into())).unwrap_or_default(),
                row.get("nextAction").map(|t| crud::term_display(t.into())).unwrap_or_default(),
                row.get("blockedBy").map(|t| crud::term_display(t.into())).unwrap_or_default(),
            )
        })
        .collect();

    if rows.is_empty() {
        return Ok(String::new());
    }

    let mut output = String::new();

    // Group: blocked items first
    let blocked: Vec<_> = rows.iter().filter(|r| r.2 == "blocked").collect();
    if !blocked.is_empty() {
        output.push_str("[Blocked]\n");
        for (_, name, _, _, blocker) in &blocked {
            let reason = if blocker.is_empty() { "unknown" } else { blocker };
            output.push_str(&format!("- {name}: {reason}\n"));
        }
        output.push('\n');
    }

    // Group: active projects
    let projects: Vec<_> = rows
        .iter()
        .filter(|r| {
            r.2 != "blocked"
                && (r.0 == "Project" || r.0 == "App" || r.0 == "Framework" || r.0 == "TrackingProject")
        })
        .collect();
    if !projects.is_empty() {
        output.push_str("[Active Projects]\n");
        for (_, name, status, next, _) in &projects {
            if next.is_empty() {
                output.push_str(&format!("- {name} ({status})\n"));
            } else {
                output.push_str(&format!("- {name} ({status}) — next: {next}\n"));
            }
        }
        output.push('\n');
    }

    // Group: active tasks
    let tasks: Vec<_> = rows
        .iter()
        .filter(|r| r.0 == "Task" && r.2 != "blocked")
        .collect();
    if !tasks.is_empty() {
        output.push_str("[Active Tasks]\n");
        for (_, name, status, _, _) in &tasks {
            output.push_str(&format!("- {name} ({status})\n"));
        }
        output.push('\n');
    }

    Ok(output.trim_end().to_string())
}
