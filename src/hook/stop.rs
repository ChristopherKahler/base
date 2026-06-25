//! Stop hook: at the end of each Claude turn, keep the current app's AST map
//! live by spawning a backgrounded `base sync --ast` for the cwd's app.
//!
//! Three guards keep this cheap and safe:
//!   1. Opt-in — only apps already mapped once (`.base-ast/ast.ttl` exists) are
//!      auto-refreshed, so a Stop in a never-mapped dir (or the workspace root,
//!      which trips the file-count threshold) does nothing.
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

    roots.sort();
    roots.dedup();

    // Requeue apps that are mapped but debounced, so they refresh next Stop
    // instead of being silently dropped.
    let mut requeue: Vec<String> = Vec::new();
    for root in &roots {
        if !refresh_app(root) && root.join(".base-ast").join("ast.ttl").is_file()
            && let Some(s) = root.to_str()
        {
            requeue.push(s.to_string());
        }
    }

    if let (Some(s), Some(bd)) = (session.as_mut(), base_dir.as_deref()) {
        for app in requeue {
            s.mark_dirty_app(&app);
        }
        let _ = s.save(bd);
    }
    Ok(())
}

/// Refresh one app's AST map (opt-in + debounced). Returns true if a sync was
/// spawned, false if skipped (not mapped, or within the debounce window).
fn refresh_app(root: &Path) -> bool {
    let base_ast = root.join(".base-ast");
    let ttl = base_ast.join("ast.ttl");
    if !ttl.is_file() {
        return false; // opt-in: only keep already-mapped apps fresh
    }
    let marker = base_ast.join(".last-sync");
    if recently_synced(&marker) {
        return false;
    }
    let _ = std::fs::write(&marker, b"");
    spawn_sync(root);
    true
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
fn spawn_sync(app_root: &Path) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let _ = Command::new(exe)
        .arg("sync")
        .arg("--ast")
        .arg("--target")
        .arg(app_root)
        // Background refresh only touches the ttl — skip the workspace-graph
        // registration write so frequent turns don't churn graph.nq.
        .env("BASE_AST_SKIP_REGISTER", "1")
        .current_dir(app_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
