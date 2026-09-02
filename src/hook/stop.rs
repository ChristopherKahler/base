//! Stop hook: at the end of each Claude turn, keep every code map this turn
//! touched live — the cwd's app (or the bare folder the session adopted) and
//! every app whose files were edited, as tracked by the pre-tool-use hook.
//!
//! Which folder is an app, and whether to build, refresh, or wait, is decided
//! in `hook::automap`; this hook only drains the marks. Three properties hold
//! for every sync it starts:
//!   1. Never the home directory or a workspace of apps — see `automap`.
//!   2. Debounced — at most one refresh per app per window, so rapid turns
//!      don't pile up overlapping syncs; a debounced app is requeued for the
//!      next Stop instead of dropped.
//!   3. Detached — spawned and never waited on, so the turn never blocks.
//!      With the extractor's atomic temp+rename write, an in-flight sync can
//!      never leave a torn `ast.ttl`.
//!
//! Fail-open: any error surfaces to stderr and the hook still exits 0.

use std::path::{Path, PathBuf};

use crate::config::BaseConfig;
use crate::domain::session::SessionState;
use crate::hook::automap::{self, RootPlan};

pub use crate::hook::automap::{ensure_app_map, plan_map, MapPlan};

pub fn handle(_config: &BaseConfig, cwd: &Path) -> anyhow::Result<()> {
    // Refresh the session-cwd app AND every app whose files were edited this turn
    // (tracked by the pre-tool-use hook). Without the latter, editing files in a
    // sub-app from the parent workspace would leave that sub-app's map stale — so
    // the next touch's blast radius wouldn't reflect this turn's edits.
    let mut roots: Vec<PathBuf> = Vec::new();
    if let RootPlan::Marked(root) | RootPlan::Adopt(root) = automap::session_root(cwd) {
        roots.push(root);
    }

    let base_dir = crate::config::find_workspace_base(cwd);
    let mut session = base_dir.as_deref().map(SessionState::load);
    if let Some(s) = session.as_mut() {
        for app in s.take_dirty_apps() {
            roots.push(PathBuf::from(app));
        }
    }
    // And the global-tier copy: a mark written while cwd was inside the app is
    // invisible to the cwd-scoped drain above once cwd has moved (0.13.8).
    for app in SessionState::take_dirty_apps_global() {
        roots.push(PathBuf::from(app));
    }

    roots.sort();
    roots.dedup();

    // Requeue apps that are mapped but debounced, so they refresh next Stop
    // instead of being silently dropped.
    let mut requeue: Vec<String> = Vec::new();
    for root in &roots {
        if ensure_app_map(root) == MapPlan::Debounced
            && let Some(s) = root.to_str()
        {
            requeue.push(s.to_string());
        }
    }

    // Requeue into the global tier, which is where the next Stop looks
    // whatever its cwd is; the cwd-scoped copy is kept for older binaries
    // that still drain only there.
    for app in &requeue {
        SessionState::mark_dirty_app_global(app);
    }
    if let (Some(s), Some(bd)) = (session.as_mut(), base_dir.as_deref()) {
        for app in requeue {
            s.mark_dirty_app(&app);
        }
        let _ = s.save(bd);
    }
    Ok(())
}
