//! Stop hook: at the end of each Claude turn, keep the current app's AST map
//! live by spawning a backgrounded `base sync --ast` for the cwd's app.
//!
//! Three guards keep this cheap and safe:
//!   1. Never the home directory — an app root that IS the operator's home
//!      would map every project on the machine into one file, so it is the
//!      one root no hook ever syncs. Every other app root gets a map the first
//!      time a session opens in it or a turn edits it (Chris, 2026-09-01:
//!      "it should make a map every time an app is started, period"); before
//!      0.13.9 this was opt-in and a new project stayed unmapped until someone
//!      ran `base sync --ast` by hand.
//!   2. Debounced — at most one refresh per `DEBOUNCE_SECS`, so rapid turns
//!      don't pile up overlapping syncs.
//!   3. Detached — the sync is spawned and never waited on, so the turn never
//!      blocks. Combined with the extractor's atomic temp+rename write, an
//!      in-flight sync can never leave a torn `ast.ttl`.
//!
//! Fail-open: any error surfaces to stderr and the hook still exits 0.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::config::BaseConfig;
use crate::domain::session::SessionState;

const DEBOUNCE_SECS: u64 = 20;

pub fn handle(_config: &BaseConfig, cwd: &Path) -> anyhow::Result<()> {
    // Refresh the session-cwd app AND every app whose files were edited this turn
    // (tracked by the pre-tool-use hook). Without the latter, editing files in a
    // sub-app from the parent workspace would leave that sub-app's map stale — so
    // the next touch's blast radius wouldn't reflect this turn's edits.
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(root) = crate::config::ast_app_root(cwd) {
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

/// What a hook does about an app root's code map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapPlan {
    /// The root is the operator's home directory: never mapped by a hook.
    SkipHome,
    /// A sync ran within the debounce window; the caller may requeue.
    Debounced,
    /// No `.base-ast/ast.ttl` yet: build the first map, registered.
    Build,
    /// A map exists: refresh it in the background, unregistered.
    Refresh,
}

/// The decision, with every input explicit so it can be tested without a
/// filesystem: `home` is the operator's home, `mapped` whether `ast.ttl`
/// exists, `debounced` whether `.last-sync` is fresher than the window.
pub fn plan_map(root: &Path, home: Option<&Path>, mapped: bool, debounced: bool) -> MapPlan {
    if home.is_some_and(|h| h == root) {
        return MapPlan::SkipHome;
    }
    if debounced {
        return MapPlan::Debounced;
    }
    if mapped { MapPlan::Refresh } else { MapPlan::Build }
}

/// Make sure `root` has a code map and that it is fresh: build it on first
/// contact, refresh it afterwards, debounced, never for the home directory.
/// Returns what was decided; `Build` and `Refresh` mean a detached sync is
/// now running. Used by session-start (first contact with an app) and by the
/// Stop hook (apps edited this turn).
pub fn ensure_app_map(root: &Path) -> MapPlan {
    let base_ast = root.join(".base-ast");
    let mapped = base_ast.join("ast.ttl").is_file();
    let marker = base_ast.join(".last-sync");
    let home = crate::home::real_home();
    let plan = plan_map(root, home.as_deref(), mapped, recently_synced(&marker));
    match plan {
        MapPlan::Build | MapPlan::Refresh => {
            let _ = std::fs::create_dir_all(&base_ast);
            let _ = std::fs::write(&marker, b"");
            spawn_sync(root, plan == MapPlan::Build);
        }
        MapPlan::SkipHome | MapPlan::Debounced => {}
    }
    plan
}


/// True if a refresh ran within the debounce window — skip this one.
fn recently_synced(marker: &Path) -> bool {
    std::fs::metadata(marker)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|e| e < Duration::from_secs(DEBOUNCE_SECS))
        .unwrap_or(false)
}

/// Spawn a detached, backgrounded per-app AST sync. Never waited on.
/// `register` is true for a FIRST map, so it lands in `base ast list`; a
/// background refresh skips the workspace-graph registration write so
/// frequent turns don't churn graph.nq.
fn spawn_sync(app_root: &Path, register: bool) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut cmd = Command::new(exe);
    cmd.arg("sync")
        .arg("--ast")
        .arg("--target")
        .arg(app_root);
    if !register {
        cmd.env("BASE_AST_SKIP_REGISTER", "1");
    }
    let _ = cmd
        .current_dir(app_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
