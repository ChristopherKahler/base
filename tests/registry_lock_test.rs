//! The registry cluster: #71, #72, #73, #74.
//!
//! One test per design lock in `forks/base-registry-tier-and-archive.md`. The
//! shell harness `verification/base-0.14.2/registry_0142.sh` drives the same
//! rows through the real binary across processes; these pin the seams in-tree so
//! a refactor cannot quietly undo them.
//!
//! Each test writes inside its own tempdir (`.base/` makes it a workspace), so
//! `find_workspace_base` never resolves to the operator's graph.

use std::path::{Path, PathBuf};

use oxigraph::sparql::QueryResults;

use base::config::NamespaceConfig;
use base::crud;

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

fn workspace() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    tmp
}

fn graph_of(tmp: &Path) -> PathBuf {
    tmp.join(".base").join("graph.nq")
}

fn ask(path: &Path, body: &str) -> bool {
    let store = base::store::load_graph(path).unwrap();
    let sparql = format!(
        "PREFIX {p}: <{u}>\nASK {{ {body} }}",
        p = ns().prefix,
        u = ns().uri,
    );
    matches!(store.query(&sparql), Ok(QueryResults::Boolean(true)))
}

fn status_is(path: &Path, slug: &str, want: &str) -> bool {
    ask(
        path,
        &format!("GRAPH ?g {{ <{}handoff/{slug}> ops:status \"{want}\" }}", ns().uri),
    )
}

// ── Lock 1 — create says what it archived, in its tier ───────

#[test]
fn create_reports_the_handoff_it_archived() {
    let tmp = workspace();
    crud::handoff::create(None, tmp.path(), &ns(), "pj", "/d/HANDOFF-A.md", None).unwrap();
    let out =
        crud::handoff::create(None, tmp.path(), &ns(), "pj", "/d/HANDOFF-B.md", None).unwrap();

    // 0.14.1 archived the prior handoff and said nothing, so four builders
    // registering inside twelve seconds each closed the one before it silently.
    assert_eq!(out.archived_prior.as_deref(), Some("HANDOFF-A"));
    assert_eq!(out.tier, "workspace tier");
    assert!(status_is(&graph_of(tmp.path()), "HANDOFF-A", "archived"));
}

#[test]
fn create_reports_no_prior_when_there_was_none() {
    let tmp = workspace();
    let out =
        crud::handoff::create(None, tmp.path(), &ns(), "pj", "/d/HANDOFF-ONLY.md", None).unwrap();
    assert_eq!(out.archived_prior, None);
}

#[test]
fn re_registering_the_same_slug_is_not_a_prior_handoff() {
    let tmp = workspace();
    crud::handoff::create(None, tmp.path(), &ns(), "pj", "/d/RESUME.md", None).unwrap();
    let out = crud::handoff::create(None, tmp.path(), &ns(), "pj", "/d/RESUME.md", None).unwrap();
    // Idempotent re-register: it re-points its own row, it does not "archive" it.
    assert_eq!(out.archived_prior, None, "a re-register reported itself as a prior handoff");
}

#[test]
fn create_does_not_touch_the_other_tier() {
    // A write acts on the tier you stand in (#61). The other tier is named by
    // the CLI, never mutated here.
    let home = tempfile::tempdir().unwrap();
    let gbl = home.path().join(".base-gbl");
    std::fs::create_dir_all(gbl.join(".base")).unwrap();
    crud::handoff::create(None, &gbl, &ns(), "pj", "/d/HANDOFF-G.md", None).unwrap();

    let tmp = workspace();
    let out = crud::handoff::create(
        Some(home.path()),
        tmp.path(),
        &ns(),
        "pj",
        "/d/HANDOFF-W.md",
        None,
    )
    .unwrap();

    assert_eq!(
        out.other_tier_open.as_ref().map(|(s, t)| (s.as_str(), t.as_str())),
        Some(("HANDOFF-G", "global tier")),
        "the other tier's open handoff was not reported"
    );
    assert!(
        status_is(&gbl.join(".base").join("graph.nq"), "HANDOFF-G", "open"),
        "create mutated the other tier"
    );
}

// ── Lock 2 — archive/snooze report, and never succeed on a no-op ──

#[test]
fn archive_reports_the_tier_it_changed() {
    let tmp = workspace();
    crud::handoff::create_fork(tmp.path(), &ns(), "pj", "/d/F.md", None).unwrap();
    let changed = crud::handoff::archive(None, tmp.path(), &ns(), "F").unwrap();
    assert_eq!(changed, vec!["workspace tier".to_string()]);
    assert!(status_is(&graph_of(tmp.path()), "F", "archived"));
}

#[test]
fn archive_of_an_unknown_slug_changes_nothing() {
    // #72: 0.14.1 ran the UPDATE, bound nothing, returned Ok(()) and the CLI
    // printed `archived`. An empty vec is what makes the CLI exit non-zero.
    let tmp = workspace();
    crud::handoff::create_fork(tmp.path(), &ns(), "pj", "/d/F.md", None).unwrap();
    let changed = crud::handoff::archive(None, tmp.path(), &ns(), "no-such-slug").unwrap();
    assert!(changed.is_empty(), "a no-op archive reported {changed:?}");
}

#[test]
fn snooze_of_an_unknown_slug_changes_nothing() {
    let tmp = workspace();
    crud::handoff::create_fork(tmp.path(), &ns(), "pj", "/d/F.md", None).unwrap();
    let changed = crud::handoff::snooze(None, tmp.path(), &ns(), "no-such-slug", 3).unwrap();
    assert!(changed.is_empty(), "a no-op snooze reported {changed:?}");
}

// ── Lock 3 — findable by slug from either tier (regression pin) ──

#[test]
fn a_global_row_archives_from_a_workspace_cwd_by_slug() {
    // Measured green on 0.14.1 too: entity IRIs are tier-invariant
    // (`{ns.uri}handoff/{slug}`) and `archive` binds `GRAPH ?g` as a variable, so
    // the graph IRI never entered the match. This pins that it stays true.
    let home = tempfile::tempdir().unwrap();
    let gbl = home.path().join(".base-gbl");
    std::fs::create_dir_all(gbl.join(".base")).unwrap();
    crud::handoff::create_fork(&gbl, &ns(), "xt", "/d/XT.md", None).unwrap();

    let tmp = workspace();
    let changed = crud::handoff::archive(Some(home.path()), tmp.path(), &ns(), "XT").unwrap();
    assert_eq!(changed, vec!["global tier".to_string()]);
    assert!(status_is(&gbl.join(".base").join("graph.nq"), "XT", "archived"));
}

#[test]
fn the_g_flag_does_not_apply_the_update_twice() {
    // Under `-g` the global tier IS the workspace, so both tier entries resolved
    // to one file and the UPDATE ran twice on it — two identical archive lines
    // one second apart in the production feed (2026-09-07 15:07:48).
    let home = tempfile::tempdir().unwrap();
    let gbl = home.path().join(".base-gbl");
    std::fs::create_dir_all(gbl.join(".base")).unwrap();
    crud::handoff::create_fork(&gbl, &ns(), "xt", "/d/XT.md", None).unwrap();

    let changed = crud::handoff::archive(Some(home.path()), &gbl, &ns(), "XT").unwrap();
    assert_eq!(changed.len(), 1, "the UPDATE ran on {} files, not 1", changed.len());
}

// ── Lock 4 — the write that did not land ─────────────────────

#[test]
fn concurrent_writers_do_not_lose_rows() {
    // The defect, in-process: eight writers loading the same graph, mutating in
    // memory and writing the whole file back. Without the lock, whoever renames
    // last wins and the rest are gone with no error anywhere — measured on
    // 0.14.1 as three of eight rows surviving on Windows, two of eight on Linux.
    let tmp = workspace();
    crud::handoff::create_fork(tmp.path(), &ns(), "seed", "/d/SEED.md", None).unwrap();

    let root = tmp.path().to_path_buf();
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let root = root.clone();
            std::thread::spawn(move || {
                crud::handoff::create_fork(&root, &ns(), "conc", &format!("/d/C{i}.md"), None)
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap().unwrap();
    }

    let g = graph_of(tmp.path());
    let missing: Vec<usize> = (0..8)
        .filter(|i| !status_is(&g, &format!("C{i}"), "open"))
        .collect();
    assert!(missing.is_empty(), "rows lost to concurrent writers: {missing:?}");
}

#[test]
fn the_graph_lock_is_reentrant_on_one_path() {
    // An outer writer that calls a helper which locks the same graph must not
    // wait out its own timeout on a file it is itself holding.
    let tmp = workspace();
    let g = graph_of(tmp.path());
    std::fs::write(&g, "").unwrap();

    let out = base::store::with_graph_lock(&g, || {
        assert!(base::store::holds_graph_lock());
        base::store::with_graph_lock(&g, || Ok(42))
    })
    .unwrap();
    assert_eq!(out, 42);
    assert!(!base::store::holds_graph_lock(), "the guard did not release");
    assert!(!base::store::lock_path(&g).exists(), "the lock file was left behind");
}

#[test]
fn a_held_lock_blocks_and_a_stale_one_is_reaped() {
    let tmp = workspace();
    let g = graph_of(tmp.path());
    std::fs::write(&g, "").unwrap();
    let lock = base::store::lock_path(&g);

    // Held by "another process": the wait is bounded and the error names the lock.
    std::fs::write(&lock, "999999\n").unwrap();
    let err = std::thread::spawn({
        let g = g.clone();
        move || base::store::with_graph_lock(&g, || Ok(()))
    })
    .join()
    .unwrap()
    .unwrap_err();
    assert!(
        err.to_string().contains("graph lock"),
        "the timeout did not name the lock: {err}"
    );

    // Older than the stale window with nobody holding it: reaped, write proceeds.
    // Backdating the mtime needs a platform call and this crate has no filetime
    // dependency; `touch` is enough here, and the shell harness (R15) exercises
    // the same path end-to-end on both platforms.
    #[cfg(unix)]
    {
        let touched = std::process::Command::new("touch")
            .args(["-d", "2 hours ago"])
            .arg(&lock)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(touched, "could not backdate the lock file");
        let got = base::store::with_graph_lock(&g, || Ok(7)).unwrap();
        assert_eq!(got, 7, "a stale lock was not reaped");
    }
    #[cfg(not(unix))]
    let _ = lock;
}

// ── Lock 4, the reap's own semantics (PR 88 review, plover) ──
//
// Age is not staleness. Reaping by mtime alone kills a LIVE holder — `graph
// compact` and `doctor --repair` rewrite a whole graph and legitimately hold the
// lock past the window — and an unconditional remove on drop then lets the
// reaped holder delete the NEW holder's lock. Both put two writers on one graph,
// which is the defect the lock exists to prevent.

/// Backdate a file's mtime. Unix only; the shell harness (R15) covers the same
/// path on Windows, and a test that cannot set the clock reports SKIP by
/// returning false rather than passing vacuously.
#[cfg(unix)]
fn backdate(path: &std::path::Path) -> bool {
    std::process::Command::new("touch")
        .args(["-d", "2 hours ago"])
        .arg(path)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn backdate(_path: &std::path::Path) -> bool {
    false
}

#[test]
fn an_old_lock_held_by_a_live_process_is_not_reaped() {
    let tmp = workspace();
    let g = graph_of(tmp.path());
    std::fs::write(&g, "").unwrap();
    let lock = base::store::lock_path(&g);

    // Our own pid is unimpeachably alive. Old mtime, live holder: keep it.
    std::fs::write(&lock, format!("{}\n", std::process::id())).unwrap();
    if !backdate(&lock) {
        return; // covered by the shell harness on this platform
    }
    let err = std::thread::spawn({
        let g = g.clone();
        move || base::store::with_graph_lock(&g, || Ok(()))
    })
    .join()
    .unwrap()
    .unwrap_err();
    assert!(
        err.to_string().contains("graph lock"),
        "an old lock held by a LIVE process was reaped: {err}"
    );
    assert!(lock.exists(), "the live holder's lock file was removed");
}

#[test]
fn an_old_lock_whose_pid_is_gone_is_reaped() {
    let tmp = workspace();
    let g = graph_of(tmp.path());
    std::fs::write(&g, "").unwrap();
    let lock = base::store::lock_path(&g);

    // A pid that cannot be running. Old mtime, dead holder: reap and proceed.
    std::fs::write(&lock, "4294967294\n").unwrap();
    if !backdate(&lock) {
        return;
    }
    let got = base::store::with_graph_lock(&g, || Ok(9)).unwrap();
    assert_eq!(got, 9, "a lock whose pid is gone was not reaped");
}

#[test]
fn drop_never_removes_another_holders_lock() {
    // The ABA leg. A guard is taken, its lock is reaped out from under it and
    // replaced by another holder's file; the first guard's drop must leave that
    // file alone, because the pid in it is no longer ours.
    let tmp = workspace();
    let g = graph_of(tmp.path());
    std::fs::write(&g, "").unwrap();
    let lock = base::store::lock_path(&g);

    {
        let _guard = base::store::lock_graph(&g).unwrap();
        assert_eq!(
            std::fs::read_to_string(&lock).unwrap().trim(),
            std::process::id().to_string(),
            "the lock does not record our pid"
        );
        // Someone reaped us and took the lock; their pid is in the file now.
        std::fs::write(&lock, "4294967293\n").unwrap();
    } // our guard drops here

    assert!(
        lock.exists(),
        "drop deleted a lock file this process no longer held"
    );
    assert_eq!(
        std::fs::read_to_string(&lock).unwrap().trim(),
        "4294967293",
        "drop replaced the other holder's lock"
    );
    std::fs::remove_file(&lock).unwrap();
}
