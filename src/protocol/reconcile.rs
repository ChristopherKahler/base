//! Session-start write-reconcile: the mechanical active⇄deferred engine.
//!
//! Every prior hook was read-only. This is the first that MUTATES the graph before
//! signals surface, so the state Claude renders is already true. For each project it:
//!   1. overwrites `lastActive` with the real folder last-touch (de-fakes the
//!      session-stamped timestamp), then
//!   2. decays status mechanically: a *working* project (active / in_progress /
//!      planning / not_started) untouched past `stale_days` becomes `deferred`
//!      ("auto: cold Nd"); a `deferred` project touched within the window revives
//!      to `active`.
//!
//! Pinned projects — those carrying a *future* `resurfaceAt` (an explicit operator
//! "defer until / keep active") — are exempt from mechanical status changes; only
//! their `lastActive` is refreshed. This is the resurfaceAt-pin override: the operator
//! signal wins, everything else decays from the working signals alone.
//!
//! Terminal projects (complete / archived) are skipped entirely.
//!
//! Path resolution is REGISTRY-AWARE: a project's folder is resolved against its own
//! workspace root first, then — if that doesn't exist — searched by basename across
//! every registered workspace (`[[workspace]]` in base.toml). A project whose folder
//! moved to another registered workspace (e.g. a framework promoted to ~/ops-sys/
//! toolbox) is found and flagged for repath instead of being misreported as "gone".
//!
//! Gated on `[protocol] enabled` for the *apply* path. The read-only [`plan`] is
//! ungated so `base reconcile --dry-run` can preview before the protocol is enabled.

use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Local};
use oxigraph::model::Term;
use oxigraph::sparql::QueryResults;
use oxigraph::store::Store;

use crate::config::{BaseConfig, NamespaceConfig};
use crate::crud;
use crate::changelog::Change;
use crate::store;

/// Statuses that count as "working" — eligible to decay to `deferred` when cold.
const WORKING_STATUSES: &[&str] = &["active", "in_progress", "planning", "not_started"];
/// Statuses left strictly alone (done work shouldn't churn the graph).
const TERMINAL_STATUSES: &[&str] = &["complete", "completed", "archived"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Defer,
    Revive,
    Hold,
    Pinned,
    /// Folder not found under its own workspace OR any registered workspace.
    NoFolder,
    Terminal,
}

#[derive(Debug, Clone)]
pub struct Decision {
    pub slug: String,
    pub iri: String,
    pub g: String,
    pub status: String,
    pub path: String,
    pub touch_days: Option<i64>,
    pub touch_iso: Option<String>,
    pub action: Action,
    /// Set when the stored path was stale but the folder was relocated in another
    /// registered workspace (workspace-relative form). Drives a repath nudge.
    pub relocated_to: Option<String>,
    /// Multiple basename matches across registered workspaces — ambiguous, needs a human.
    pub candidates: Vec<String>,
}

#[derive(Debug, Default)]
pub struct ReconcileStats {
    pub scanned: usize,
    pub refreshed: usize,
    pub deferred: usize,
    pub revived: usize,
}

impl ReconcileStats {
    pub fn changed(&self) -> bool {
        self.deferred > 0 || self.revived > 0
    }
}

/// Registered workspace roots (existing dirs only), from `[[workspace]]` in base.toml.
pub fn registered_roots(config: &BaseConfig) -> Vec<PathBuf> {
    config
        .workspace
        .iter()
        .map(|w| PathBuf::from(&w.path))
        .filter(|p| p.is_dir())
        .collect()
}

/// Run the reconcile pass over the workspace graph and APPLY it.
pub fn reconcile(cwd: &Path, config: &BaseConfig) -> Result<ReconcileStats> {
    if !config.protocol.enabled {
        return Ok(ReconcileStats::default());
    }
    let Some((store, trig_path, ws_root)) = open_workspace(cwd) else {
        return Ok(ReconcileStats::default());
    };
    let roots = registered_roots(config);
    let decisions = plan(&store, &config.namespace, &ws_root, &roots, config.protocol.stale_days as i64)?;
    apply(&store, &config.namespace, &trig_path, &decisions)
}

/// Load the workspace graph + resolve the workspace root. `None` when absent.
pub fn open_workspace(cwd: &Path) -> Option<(Store, PathBuf, PathBuf)> {
    let base_dir = crate::config::find_workspace_base(cwd)?;
    let ws_root = base_dir.parent().unwrap_or(cwd).to_path_buf();
    let trig_path = base_dir.join("graph.nq");
    if !trig_path.exists() {
        return None;
    }
    let store = store::load_graph(&trig_path).ok()?;
    Some((store, trig_path, ws_root))
}

/// Read-only: compute what reconcile WOULD do for every project-like entity.
pub fn plan(
    store: &Store,
    ns: &NamespaceConfig,
    ws_root: &Path,
    registered_roots: &[PathBuf],
    stale_days: i64,
) -> Result<Vec<Decision>> {
    let p = &ns.prefix;
    let select = format!(
        "{pfx}\n\
         SELECT ?iri ?g ?status ?path ?resurfaceAt WHERE {{\n\
           GRAPH ?g {{\n\
             ?iri a ?type ;\n\
               {p}:status ?status ;\n\
               {p}:path ?path .\n\
             OPTIONAL {{ ?iri {p}:resurfaceAt ?resurfaceAt }}\n\
             FILTER(?type IN ({p}:Project, {p}:App, {p}:Framework, {p}:TrackingProject))\n\
           }}\n\
         }}",
        pfx = crud::prefixes(ns)
    );

    let rows = collect_rows(store, &select)?;
    let now = Local::now();
    let mut out = Vec::with_capacity(rows.len());

    for row in rows {
        let slug = row.iri.rsplit('/').next().unwrap_or(&row.iri).to_string();

        if TERMINAL_STATUSES.contains(&row.status.as_str()) {
            out.push(Decision {
                slug, iri: row.iri, g: row.g, status: row.status, path: row.path,
                touch_days: None, touch_iso: None, action: Action::Terminal,
                relocated_to: None, candidates: Vec::new(),
            });
            continue;
        }

        // Registry-aware resolution.
        let located = locate_folder(&project_root(&row.path), ws_root, registered_roots);
        let (abs, relocated_to, candidates) = match located {
            Located::Here(abs) => (Some(abs), None, Vec::new()),
            Located::Moved { abs, rel } => (Some(abs), Some(rel), Vec::new()),
            Located::Ambiguous(cands) => (None, None, cands),
            Located::Missing => (None, None, Vec::new()),
        };

        let touch = abs.as_deref().and_then(crate::protocol::touch::folder_last_touch);
        let (touch_days, touch_iso) = match &touch {
            Some(t) => (
                Some(now.signed_duration_since(*t).num_days()),
                Some(t.to_rfc3339_opts(chrono::SecondsFormat::Secs, false)),
            ),
            None => (None, None),
        };

        let pinned = row
            .resurface_at
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Local) > now)
            .unwrap_or(false);

        let action = match (touch_days, pinned) {
            (None, _) => Action::NoFolder,
            (Some(_), true) => Action::Pinned,
            (Some(days), false) => {
                if WORKING_STATUSES.contains(&row.status.as_str()) && days >= stale_days {
                    Action::Defer
                } else if row.status == "deferred" && days < stale_days {
                    Action::Revive
                } else {
                    Action::Hold
                }
            }
        };

        out.push(Decision {
            slug, iri: row.iri, g: row.g, status: row.status, path: row.path,
            touch_days, touch_iso, action, relocated_to, candidates,
        });
    }
    Ok(out)
}

/// Apply a plan: refresh lastActive from real touch, flip statuses, write back once.
/// NOTE: relocation is reported, never auto-persisted — basenames can collide across
/// workspaces, so repath is an explicit operator/nudge action.
pub fn apply(store: &Store, ns: &NamespaceConfig, trig_path: &Path, decisions: &[Decision]) -> Result<ReconcileStats> {
    let mut stats = ReconcileStats { scanned: decisions.len(), ..Default::default() };
    let p = &ns.prefix;
    let pfx = crud::prefixes(ns);
    let mut ops: Vec<String> = Vec::new();

    for d in decisions {
        if let Some(iso) = &d.touch_iso
            && d.action != Action::Terminal
        {
            ops.push(crud::field_update(&d.g, &d.iri, &format!("{p}:lastActive"),
                &format!("\"{iso}\"^^xsd:dateTime")));
            stats.refreshed += 1;
        }
        match d.action {
            Action::Defer => {
                let days = d.touch_days.unwrap_or_default();
                ops.push(crud::field_update(&d.g, &d.iri, &format!("{p}:status"), "\"deferred\""));
                ops.push(crud::field_update(&d.g, &d.iri, &format!("{p}:deferredReason"),
                    &format!("\"auto: cold {days}d\"")));
                stats.deferred += 1;
            }
            Action::Revive => {
                ops.push(crud::field_update(&d.g, &d.iri, &format!("{p}:status"), "\"active\""));
                ops.push(format!(
                    "DELETE {{ GRAPH <{g}> {{ <{s}> {p}:deferredReason ?r }} }}\n\
                     WHERE {{ GRAPH <{g}> {{ <{s}> {p}:deferredReason ?r }} }}",
                    g = d.g, s = d.iri
                ));
                stats.revived += 1;
            }
            _ => {}
        }
    }

    if !ops.is_empty() {
        for op in &ops {
            let _ = store.update(&format!("{pfx}\n{op}"));
        }
        store::write_back(store, trig_path, Change::Sparql(&ops.join(";\n")))?;
    }
    Ok(stats)
}

/// Render a human-readable dry-run report grouped by action.
pub fn format_report(decisions: &[Decision], ws_root: &Path, stale_days: i64) -> String {
    use Action::*;
    let by = |a: Action| -> Vec<&Decision> {
        let mut v: Vec<&Decision> = decisions.iter().filter(|d| d.action == a).collect();
        v.sort_by(|x, y| y.touch_days.unwrap_or(i64::MIN).cmp(&x.touch_days.unwrap_or(i64::MIN)));
        v
    };
    let line = |d: &Decision, note: &str| {
        let moved = d.relocated_to.as_ref().map(|r| format!("  ⟲ moved → {r} (repath)")).unwrap_or_default();
        format!("  {:<26} {:<13} {}{}\n", d.slug, d.status, note, moved)
    };

    let mut s = format!("Reconcile dry-run — {} (stale_days = {})\n\n", ws_root.display(), stale_days);

    let defer = by(Defer);
    s.push_str(&format!("WOULD DEFER — working but cold ({}):\n", defer.len()));
    for d in &defer { s.push_str(&line(d, &format!("{}d cold", d.touch_days.unwrap_or(0)))); }
    if defer.is_empty() { s.push_str("  (none)\n"); }

    let revive = by(Revive);
    s.push_str(&format!("\nWOULD REVIVE — deferred but recently touched ({}):\n", revive.len()));
    for d in &revive { s.push_str(&line(d, &format!("touched {}d ago", d.touch_days.unwrap_or(0)))); }
    if revive.is_empty() { s.push_str("  (none)\n"); }

    let hold = by(Hold);
    s.push_str(&format!("\nSTAYS PUT — warm, or deferred+still-cold ({}):\n", hold.len()));
    for d in &hold { s.push_str(&line(d, &format!("{}d", d.touch_days.unwrap_or(0)))); }
    if hold.is_empty() { s.push_str("  (none)\n"); }

    let pinned = by(Pinned);
    if !pinned.is_empty() {
        s.push_str(&format!("\nPINNED — future resurfaceAt, exempt ({}):\n", pinned.len()));
        for d in &pinned { s.push_str(&line(d, &format!("{}d (pinned)", d.touch_days.unwrap_or(0)))); }
    }

    let nofolder = by(NoFolder);
    s.push_str(&format!("\nUNRESOLVED — folder not found in any registered workspace ({}):\n", nofolder.len()));
    for d in &nofolder {
        if d.candidates.is_empty() {
            s.push_str(&format!("  {:<26} {:<13} stored: {}\n", d.slug, d.status, d.path));
        } else {
            s.push_str(&format!("  {:<26} {:<13} AMBIGUOUS — candidates: {}\n", d.slug, d.status, d.candidates.join(", ")));
        }
    }
    if nofolder.is_empty() { s.push_str("  (none)\n"); }

    let terminal = by(Terminal).len();
    s.push_str(&format!("\nTERMINAL — complete/archived, skipped: {terminal} project(s)\n"));

    let moved = decisions.iter().filter(|d| d.relocated_to.is_some()).count();
    s.push_str(&format!(
        "\nSummary: {} defer · {} revive · {} stay · {} unresolved · {} terminal · {} moved-needs-repath  (no graph writes)\n",
        defer.len(), revive.len(), hold.len(), nofolder.len(), terminal, moved
    ));
    s
}

enum Located {
    /// Found under its own workspace root at the stored path.
    Here(PathBuf),
    /// Found in another registered workspace (stale path); rel = workspace-relative form vs that root.
    Moved { abs: PathBuf, rel: String },
    /// Multiple registered workspaces hold a folder with this basename.
    Ambiguous(Vec<String>),
    Missing,
}

/// Resolve a project folder: its own workspace first, then by basename across every
/// registered workspace. This is what makes reconcile aware of moved/promoted folders.
fn locate_folder(stored_rel: &str, ws_root: &Path, roots: &[PathBuf]) -> Located {
    // 1) Stored path under its own workspace (or absolute) — the happy path.
    let primary = resolve_path(ws_root, stored_rel);
    if primary.is_dir() {
        return Located::Here(primary);
    }

    // 2) Relocate by basename across registered workspaces.
    let Some(base) = Path::new(stored_rel).file_name().and_then(|n| n.to_str()) else {
        return Located::Missing;
    };
    let mut hits: Vec<PathBuf> = Vec::new();
    for root in roots {
        find_dirs_by_basename(root, base, 4, &mut hits);
        if hits.len() > 5 { break; } // cap — ambiguity is ambiguity
    }
    // Don't count the (already-failed) primary location.
    hits.retain(|h| h != &primary);
    hits.sort();
    hits.dedup();

    match hits.len() {
        0 => Located::Missing,
        1 => {
            let abs = hits.remove(0);
            let rel = abs.to_string_lossy().to_string();
            Located::Moved { abs, rel }
        }
        _ => Located::Ambiguous(hits.iter().map(|h| h.to_string_lossy().to_string()).collect()),
    }
}

/// Bounded recursive search for directories named exactly `basename` under `root`.
fn find_dirs_by_basename(root: &Path, basename: &str, max_depth: usize, out: &mut Vec<PathBuf>) {
    const IGNORE: &[&str] = &[".git", ".base", "node_modules", "target", "dist", "build", "vendor"];
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if IGNORE.contains(&name.as_ref()) {
            continue;
        }
        let path = entry.path();
        if name == basename {
            out.push(path.clone());
        }
        if max_depth > 0 {
            find_dirs_by_basename(&path, basename, max_depth - 1, out);
        }
    }
}

/// Normalize a stored path to the project ROOT folder. Old PAUL projects store
/// `apps/x/.paul/paul.json`; we measure the folder `apps/x`, not one state file.
fn project_root(path: &str) -> String {
    if let Some(idx) = path.find("/.paul") {
        return path[..idx].to_string();
    }
    if path == ".paul" || path.starts_with(".paul/") {
        return ".".to_string();
    }
    path.to_string()
}

fn resolve_path(ws_root: &Path, path: &str) -> PathBuf {
    let pb = PathBuf::from(path);
    if pb.is_absolute() { pb } else { ws_root.join(pb) }
}

struct Row { iri: String, g: String, status: String, path: String, resurface_at: Option<String> }

fn collect_rows(store: &Store, sparql: &str) -> Result<Vec<Row>> {
    let QueryResults::Solutions(solutions) = store::query(store, sparql)? else {
        return Ok(Vec::new());
    };
    let full = |t: Option<&Term>| -> Option<String> {
        t.map(|term| match term {
            Term::NamedNode(n) => n.as_str().to_string(),
            Term::Literal(l) => l.value().to_string(),
            other => other.to_string(),
        })
    };
    let mut out = Vec::new();
    for sol in solutions.filter_map(|r| r.ok()) {
        let (Some(iri), Some(g), Some(status), Some(path)) = (
            full(sol.get("iri")), full(sol.get("g")), full(sol.get("status")), full(sol.get("path")),
        ) else { continue };
        out.push(Row { iri, g, status, path, resurface_at: full(sol.get("resurfaceAt")) });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_root_strips_paul_state_path() {
        assert_eq!(project_root("apps/apex/.paul/paul.json"), "apps/apex");
        assert_eq!(project_root("apps/x/.paul/paul.toml"), "apps/x");
        assert_eq!(project_root("apps/x/.paul"), "apps/x");
        assert_eq!(project_root("apps/x"), "apps/x");
        assert_eq!(project_root("planning/y"), "planning/y");
    }

    #[test]
    fn locate_relocates_by_basename_across_roots() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        let other = tmp.path().join("toolbox");
        std::fs::create_dir_all(ws.join("apps")).unwrap();
        std::fs::create_dir_all(other.join("frameworks/widget")).unwrap();

        // Stored path apps/widget doesn't exist under ws, but toolbox has it.
        let roots = vec![ws.clone(), other.clone()];
        match locate_folder("apps/widget", &ws, &roots) {
            Located::Moved { abs, .. } => assert!(abs.ends_with("frameworks/widget")),
            _ => panic!("expected Moved"),
        }
    }

    #[test]
    fn locate_here_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        std::fs::create_dir_all(ws.join("apps/here")).unwrap();
        match locate_folder("apps/here", ws, &[ws.to_path_buf()]) {
            Located::Here(_) => {}
            _ => panic!("expected Here"),
        }
    }
}
