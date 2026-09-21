use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use anyhow::Result;
use oxigraph::sparql::{QueryResults, QuerySolution};

use crate::config::{BaseConfig, WorkspaceEntry};
use crate::crud;
use crate::scope::{self, Home};

/// One working-set entity from the graph: a project, task or milestone in a working state.
struct Wrow {
    /// The entity's IRI as `crud::term_display` gives it, `project/alpha`. Owner links use the same form.
    id: String,
    ty: String,
    name: String,
    status: String,
    next: String,
    blocked_by: String,
    path: String,
    last_active: String,
    /// Every entity linking this one with `hasTask` or `hasMilestone`.
    owners: Vec<String>,
}

/// Active-awareness signal: the working set — every project, task and milestone NOT deferred or
/// terminal. Protocol's reconcile is the single source of "deferred"; this renders what's left in a
/// working state, **scoped to the current workspace**: projects homed elsewhere collapse into a
/// one-line `elsewhere:` count, while the operator's own un-homed (path-less) projects stay visible.
/// Priority 1.
pub fn run(cwd: &Path, config: &BaseConfig) -> Result<String> {
    Ok(run_sections(cwd, config)?
        .into_iter()
        .map(|s| s.text)
        .collect::<Vec<_>>()
        .join("\n"))
}

/// One section of the working set: its block kind at session start, its text, how many rows it
/// lists, and how many exist.
pub struct Section {
    pub kind: &'static str,
    pub text: String,
    pub shown: usize,
    pub total: usize,
}

/// The working set as its sections, in order: projects, tasks, milestones, blocked (spec B6, board
/// ruling R4). Each starts with its count and the command that lists all of it. PROJECTS lists the
/// projects touched within `[session_start] recent_project_days`; TASKS and MILESTONES list the
/// working ones a `hasTask` or `hasMilestone` link ties to one of those projects, which is what
/// "in progress on recently touched projects" means here: no task status says "in progress".
pub fn run_sections(cwd: &Path, config: &BaseConfig) -> Result<Vec<Section>> {
    let ns = &config.namespace;
    let p = &ns.prefix;
    let sparql = format!(
        "SELECT ?entity ?type ?name ?status ?nextAction ?blockedBy ?path ?lastActive ?taskOf ?milestoneOf WHERE {{\n\
           GRAPH ?g {{\n\
             ?entity a ?type ;\n\
               {p}:name ?name ;\n\
               {p}:status ?status ;\n\
               {p}:lastActive ?lastActive .\n\
             OPTIONAL {{ ?entity {p}:nextAction ?nextAction }}\n\
             OPTIONAL {{ ?entity {p}:blockedBy ?blockedBy }}\n\
             OPTIONAL {{ ?entity {p}:path ?path }}\n\
             FILTER(?type IN ({p}:Project, {p}:App, {p}:Framework, {p}:TrackingProject, {p}:Task, {p}:Milestone))\n\
             FILTER(?status NOT IN (\"deferred\", \"complete\", \"completed\", \"archived\"))\n\
           }}\n\
           OPTIONAL {{ GRAPH ?tg {{ ?taskOf {p}:hasTask ?entity }} }}\n\
           OPTIONAL {{ GRAPH ?mg {{ ?milestoneOf {p}:hasMilestone ?entity }} }}\n\
         }}\n\
         ORDER BY DESC(?lastActive)"
    );

    // BOTH tiers. A task or milestone recorded globally lives in a file the
    // workspace-only read never opens, so it rendered as though it did not exist --
    // a shorter list, with nothing saying a tier went unread.
    let results = crud::load_merged_and_query(cwd, ns, &sparql)?;
    let QueryResults::Solutions(solutions) = results else {
        return Ok(Vec::new());
    };

    let cell = |row: &QuerySolution, k: &str| {
        row.get(k).map(|t| crud::term_display(t.into())).unwrap_or_default()
    };
    // One row per link comes back, so an entity with two owners is two rows: gather them.
    let mut rows: Vec<Wrow> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for row in solutions.filter_map(|r| r.ok()) {
        let id = cell(&row, "entity");
        let i = match at.get(&id) {
            Some(&i) => i,
            None => {
                at.insert(id.clone(), rows.len());
                rows.push(Wrow {
                    id,
                    ty: cell(&row, "type"),
                    name: cell(&row, "name"),
                    status: cell(&row, "status"),
                    next: cell(&row, "nextAction"),
                    blocked_by: cell(&row, "blockedBy"),
                    path: cell(&row, "path"),
                    last_active: cell(&row, "lastActive"),
                    owners: Vec::new(),
                });
                rows.len() - 1
            }
        };
        for link in ["taskOf", "milestoneOf"] {
            let owner = cell(&row, link);
            if !owner.is_empty() && !rows[i].owners.contains(&owner) {
                rows[i].owners.push(owner);
            }
        }
    }

    let registry = scope::canonical_registry(&config.workspace);
    let canon_cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    // `[signal] scope = "global"` restores the flat union (current=None → no scoping, Req 5).
    let current = if config.signal.scope == "global" {
        None
    } else {
        scope::current_workspace(&canon_cwd, &registry)
    };
    let days = config.session_start.recent_project_days;
    let since = chrono::Utc::now() - chrono::Duration::days(days);

    Ok(render_sections(&rows, current.as_deref(), &registry, days, since))
}

const PROJECT_TYPES: [&str; 4] = ["Project", "App", "Framework", "TrackingProject"];

/// Pure: group + workspace-scope the rows into the rendered sections.
///
/// Briefing policy: count projects homed in the current workspace OR un-homed (path-less — the
/// operator's own work, not foreign contamination); projects homed in OTHER registered workspaces
/// become a one-line `elsewhere:` count. When `current` is None (CWD under no registered workspace)
/// nothing is scoped — the global view. Tasks and milestones are not workspace-scoped: they carry no
/// path of their own, so they reach a workspace only through a recent project they belong to.
fn render_sections(
    rows: &[Wrow],
    current: Option<&str>,
    registry: &[WorkspaceEntry],
    days: i64,
    since: chrono::DateTime<chrono::Utc>,
) -> Vec<Section> {
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
    let touched = |r: &Wrow| {
        chrono::DateTime::parse_from_rfc3339(&r.last_active)
            .is_ok_and(|dt| dt.with_timezone(&chrono::Utc) >= since)
    };

    let mut sections: Vec<Section> = Vec::new();

    // PROJECTS — scoped to the current workspace + un-homed; the recent ones listed.
    let projects: Vec<&Wrow> = rows
        .iter()
        .filter(|r| r.status != "blocked" && is_project(&r.ty) && in_briefing(&r.path))
        .collect();
    let recent: Vec<&Wrow> = projects.iter().copied().filter(|r| touched(r)).collect();
    let recent_name: HashMap<&str, &str> =
        recent.iter().map(|r| (r.id.as_str(), r.name.as_str())).collect();
    if !projects.is_empty() {
        let mut output = format!(
            "PROJECTS ({} active, touched in {days} days: {}) · all: base project list --all\n",
            projects.len(),
            recent.len()
        );
        for r in &recent {
            if r.next.is_empty() {
                output.push_str(&format!("  {} ({})\n", r.name, r.status));
            } else {
                output.push_str(&format!("  {} ({}) — next: {}\n", r.name, r.status, r.next));
            }
        }
        // Cross-workspace count: projects homed in OTHER registered workspaces.
        let mut elsewhere_ws: BTreeSet<String> = BTreeSet::new();
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
                "  elsewhere: {elsewhere_n} active across {} workspace(s)\n",
                elsewhere_ws.len()
            ));
        }
        sections.push(Section {
            kind: "projects",
            text: output.trim_end().to_string(),
            shown: recent.len(),
            total: projects.len(),
        });
    }

    // TASKS and MILESTONES — every working one counted, the ones on a recent project listed.
    for (kind, ty, title, command) in [
        ("tasks", "Task", "TASKS", "base task list"),
        ("milestones", "Milestone", "MILESTONES", "base milestone list"),
    ] {
        let all: Vec<&Wrow> = rows
            .iter()
            .filter(|r| r.ty == ty && r.status != "blocked")
            .collect();
        if all.is_empty() {
            continue;
        }
        let listed: Vec<(&Wrow, &str)> = all
            .iter()
            .filter_map(|r| {
                r.owners
                    .iter()
                    .find_map(|o| recent_name.get(o.as_str()))
                    .map(|project| (*r, *project))
            })
            .collect();
        let mut output = format!(
            "{title} ({} active, on projects touched in {days} days: {}) · all: {command}\n",
            all.len(),
            listed.len()
        );
        for (r, project) in &listed {
            output.push_str(&format!("  {} · {project}\n", r.name));
        }
        sections.push(Section {
            kind,
            text: output.trim_end().to_string(),
            shown: listed.len(),
            total: all.len(),
        });
    }

    // BLOCKED — scoped projects; tasks and milestones pass through.
    let blocked: Vec<&Wrow> = rows
        .iter()
        .filter(|r| r.status == "blocked" && (!is_project(&r.ty) || in_briefing(&r.path)))
        .collect();
    if !blocked.is_empty() {
        let mut output = format!("BLOCKED ({})\n", blocked.len());
        for r in &blocked {
            let reason = if r.blocked_by.is_empty() { "unknown" } else { &r.blocked_by };
            output.push_str(&format!("  {}: {reason}\n", r.name));
        }
        sections.push(Section {
            kind: "blocked",
            text: output.trim_end().to_string(),
            shown: blocked.len(),
            total: blocked.len(),
        });
    }

    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(days_ago: i64) -> String {
        (chrono::Utc::now() - chrono::Duration::days(days_ago)).to_rfc3339()
    }

    fn week() -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now() - chrono::Duration::days(7)
    }

    fn joined(rows: &[Wrow], current: Option<&str>, registry: &[WorkspaceEntry]) -> String {
        render_sections(rows, current, registry, 7, week())
            .into_iter()
            .map(|s| s.text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn row(ty: &str, name: &str, path: &str) -> Wrow {
        Wrow {
            id: format!("{}/{}", ty.to_lowercase(), name.to_lowercase()),
            ty: ty.into(),
            name: name.into(),
            status: "active".into(),
            next: String::new(),
            blocked_by: String::new(),
            path: path.into(),
            last_active: stamp(0),
            owners: Vec::new(),
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
        let out = joined(&rows, Some("cur"), &registry);
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
        let out = joined(&rows, None, &registry);
        assert!(out.contains("ForeignA"), "global fallback shows all");
        assert!(!out.contains("elsewhere:"), "no pointer when unscoped");
    }

    /// Board R4 and spec B6. Every working item is counted; only the ones on a project touched
    /// within the window are listed. Old, T2 and M2 are the controls: counted, never listed.
    #[test]
    fn sections_count_everything_and_list_what_is_on_recent_projects() {
        let registry = reg(&[]);
        let alpha = row("Project", "Alpha", "");
        let mut old = row("Project", "Old", "");
        old.last_active = stamp(30);
        let mut stuck = row("Project", "Stuck", "");
        stuck.status = "blocked".into();
        stuck.blocked_by = "API keys".into();
        let mut t1 = row("Task", "T1", "");
        t1.owners = vec![alpha.id.clone()];
        let mut t2 = row("Task", "T2", "");
        t2.owners = vec![old.id.clone()];
        let t3 = row("Task", "T3", "");
        let mut m1 = row("Milestone", "M1", "");
        m1.owners = vec![alpha.id.clone()];
        let mut m2 = row("Milestone", "M2", "");
        m2.owners = vec![old.id.clone()];
        let rows = vec![alpha, old, stuck, t1, t2, t3, m1, m2];

        let sections = render_sections(&rows, None, &registry, 7, week());
        let counts: Vec<(&str, usize, usize)> =
            sections.iter().map(|s| (s.kind, s.shown, s.total)).collect();
        assert_eq!(
            counts,
            [("projects", 1, 2), ("tasks", 1, 3), ("milestones", 1, 2), ("blocked", 1, 1)]
        );
        let text = joined(&rows, None, &registry);
        assert!(text.contains("PROJECTS (2 active, touched in 7 days: 1) · all: base project list --all"), "{text}");
        assert!(text.contains("  Alpha (active)") && !text.contains("  Old (active)"), "{text}");
        assert!(text.contains("TASKS (3 active, on projects touched in 7 days: 1) · all: base task list"), "{text}");
        assert!(text.contains("  T1 · Alpha") && !text.contains("T2 ·") && !text.contains("T3 ·"), "{text}");
        assert!(text.contains("MILESTONES (2 active, on projects touched in 7 days: 1) · all: base milestone list"), "{text}");
        assert!(text.contains("  M1 · Alpha") && !text.contains("M2 ·"), "{text}");
        assert!(text.contains("BLOCKED (1)\n  Stuck: API keys"), "{text}");
    }
}
