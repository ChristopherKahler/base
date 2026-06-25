use std::path::Path;

use anyhow::Result;
use oxigraph::sparql::{QueryResults, QuerySolution};

use crate::config::{BaseConfig, WorkspaceEntry};
use crate::crud;
use crate::scope::{self, Home};

/// One working-set entity row from the graph.
struct Wrow {
    ty: String,
    name: String,
    status: String,
    next: String,
    blocked_by: String,
    path: String,
}

/// Active-awareness signal: the working set — every project/task NOT deferred or terminal.
/// Protocol's reconcile is the single source of "deferred"; this renders what's left in a
/// working state, **scoped to the current workspace**: projects homed elsewhere collapse
/// into a one-line `elsewhere:` pointer, while the operator's own un-homed (path-less)
/// projects stay visible. Priority 1 (highest — never dropped by budget cap).
pub fn run(cwd: &Path, config: &BaseConfig) -> Result<String> {
    let ns = &config.namespace;
    let p = &ns.prefix;
    let sparql = format!(
        "SELECT ?type ?name ?status ?nextAction ?blockedBy ?path WHERE {{\n\
           GRAPH ?g {{\n\
             ?entity a ?type ;\n\
               {p}:name ?name ;\n\
               {p}:status ?status ;\n\
               {p}:lastActive ?lastActive .\n\
             OPTIONAL {{ ?entity {p}:nextAction ?nextAction }}\n\
             OPTIONAL {{ ?entity {p}:blockedBy ?blockedBy }}\n\
             OPTIONAL {{ ?entity {p}:path ?path }}\n\
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

    let cell = |row: &QuerySolution, k: &str| {
        row.get(k).map(|t| crud::term_display(t.into())).unwrap_or_default()
    };
    let rows: Vec<Wrow> = solutions
        .filter_map(|r| r.ok())
        .map(|row| Wrow {
            ty: cell(&row, "type"),
            name: cell(&row, "name"),
            status: cell(&row, "status"),
            next: cell(&row, "nextAction"),
            blocked_by: cell(&row, "blockedBy"),
            path: cell(&row, "path"),
        })
        .collect();

    let registry = scope::canonical_registry(&config.workspace);
    let canon_cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    // `[signal] scope = "global"` restores the flat union (current=None → no scoping, Req 5).
    let current = if config.signal.scope == "global" {
        None
    } else {
        scope::current_workspace(&canon_cwd, &registry)
    };

    Ok(render_working_set(&rows, current.as_deref(), &registry))
}

const PROJECT_TYPES: [&str; 4] = ["Project", "App", "Framework", "TrackingProject"];

/// Pure: group + workspace-scope the rows into the rendered signal.
///
/// Briefing policy: show projects homed in the current workspace OR un-homed (path-less —
/// the operator's own work, not foreign contamination); collapse projects homed in OTHER
/// registered workspaces into a one-line `elsewhere:` pointer. When `current` is None (CWD
/// under no registered workspace) nothing is scoped — the global view (today's behavior).
fn render_working_set(rows: &[Wrow], current: Option<&str>, registry: &[WorkspaceEntry]) -> String {
    let home_of = |path: &str| -> Home {
        let canon = if path.is_empty() { None } else { Some(scope::canonical_str(path)) };
        scope::home(canon.as_deref(), registry)
    };
    let in_briefing = |path: &str| -> bool {
        match current {
            None => true,
            Some(cur) => {
                let h = home_of(path);
                matches!(h, Home::Unscoped) || h == Home::Workspace(cur.to_string())
            }
        }
    };
    let is_project = |ty: &str| PROJECT_TYPES.contains(&ty);

    let mut output = String::new();

    // [Blocked] — scoped projects (tasks pass through; see [Active Tasks] note).
    let blocked: Vec<&Wrow> = rows
        .iter()
        .filter(|r| r.status == "blocked" && (!is_project(&r.ty) || in_briefing(&r.path)))
        .collect();
    if !blocked.is_empty() {
        output.push_str("[Blocked]\n");
        for r in &blocked {
            let reason = if r.blocked_by.is_empty() { "unknown" } else { &r.blocked_by };
            output.push_str(&format!("- {}: {reason}\n", r.name));
        }
        output.push('\n');
    }

    // [Active Projects] — scoped to current workspace + un-homed.
    let projects: Vec<&Wrow> = rows
        .iter()
        .filter(|r| r.status != "blocked" && is_project(&r.ty) && in_briefing(&r.path))
        .collect();
    if !projects.is_empty() {
        output.push_str("[Active Projects]\n");
        for r in &projects {
            if r.next.is_empty() {
                output.push_str(&format!("- {} ({})\n", r.name, r.status));
            } else {
                output.push_str(&format!("- {} ({}) — next: {}\n", r.name, r.status, r.next));
            }
        }
        // Cross-workspace pointer: projects homed in OTHER registered workspaces.
        let mut elsewhere_ws: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut elsewhere_n = 0usize;
        for r in rows.iter().filter(|r| r.status != "blocked" && is_project(&r.ty)) {
            if let Home::Workspace(w) = home_of(&r.path)
                && current.map(|c| w.as_str() != c).unwrap_or(false) {
                    elsewhere_n += 1;
                    elsewhere_ws.insert(w);
                }
        }
        if elsewhere_n > 0 {
            output.push_str(&format!(
                "elsewhere: {elsewhere_n} active across {} workspace(s) — `base project list --all`\n",
                elsewhere_ws.len()
            ));
        }
        output.push('\n');
    }

    // [Active Tasks] — NOT workspace-scoped yet (tasks carry no #path; needs a
    // task→project home join — deferred follow-up).
    let tasks: Vec<&Wrow> = rows
        .iter()
        .filter(|r| r.ty == "Task" && r.status != "blocked")
        .collect();
    if !tasks.is_empty() {
        output.push_str("[Active Tasks]\n");
        for r in &tasks {
            output.push_str(&format!("- {} ({})\n", r.name, r.status));
        }
        output.push('\n');
    }

    output.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(ty: &str, name: &str, path: &str) -> Wrow {
        Wrow {
            ty: ty.into(),
            name: name.into(),
            status: "active".into(),
            next: String::new(),
            blocked_by: String::new(),
            path: path.into(),
        }
    }
    fn reg(paths: &[&str]) -> Vec<WorkspaceEntry> {
        paths.iter().map(|p| WorkspaceEntry { path: (*p).into() }).collect()
    }

    // Current-homed + un-homed shown; foreign-homed hidden and counted in the pointer.
    #[test]
    fn scopes_projects_to_current_and_unhomed_with_pointer() {
        let registry = reg(&["/ws/cur", "/ws/other"]);
        let rows = vec![
            row("Project", "HomeProj", "/ws/cur/a"),   // current → shown
            row("Project", "Planning", ""),            // un-homed → shown
            row("Project", "ForeignA", "/ws/other/x"), // elsewhere → pointer
            row("Project", "ForeignB", "/ws/other/y"), // elsewhere → pointer
        ];
        let out = render_working_set(&rows, Some("cur"), &registry);
        assert!(out.contains("HomeProj"), "current-homed shown");
        assert!(out.contains("Planning"), "un-homed shown");
        assert!(!out.contains("ForeignA"), "foreign hidden from the list");
        assert!(
            out.contains("elsewhere: 2 active across 1 workspace(s)"),
            "pointer counts foreign; got:\n{out}"
        );
    }

    // No current workspace → global view, no pointer (backward compatible).
    #[test]
    fn no_current_workspace_shows_everything() {
        let registry = reg(&["/ws/other"]);
        let rows = vec![row("Project", "ForeignA", "/ws/other/x")];
        let out = render_working_set(&rows, None, &registry);
        assert!(out.contains("ForeignA"), "global fallback shows all");
        assert!(!out.contains("elsewhere:"), "no pointer when unscoped");
    }
}
