//! #41: a CLI write made after the dashboard started must survive a dashboard write.
//!
//! The dashboard loaded `graph.nq` once and serialised that snapshot back over the file on
//! every write, so every `base learn` since the start was erased. Red on 0.14.0.

use std::path::Path;

use axum::extract::{Json, State};
use base::config::{BaseConfig, NamespaceConfig};
use base::crud;

/// Drive a handler future to completion without an executor (the dashboard handlers are
/// plain async fns over in-memory state; nothing here awaits I/O).
fn poll_once<F: std::future::Future>(fut: F) -> F::Output {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn clone(_: *const ()) -> RawWaker { RawWaker::new(std::ptr::null(), &VT) }
    fn noop(_: *const ()) {}
    static VT: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VT)) };
    let mut cx = Context::from_waker(&waker);
    let mut fut = Box::pin(fut);
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) { return v; }
    }
}

fn ns() -> NamespaceConfig { NamespaceConfig::default() }

fn workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".base")).unwrap();
    std::fs::write(
        dir.path().join(".base").join("base.toml"),
        "[namespace]\nprefix = \"ops\"\nuri = \"http://ops-sys.local/ontology#\"\n",
    )
    .unwrap();
    dir
}

fn graph(cwd: &Path) -> String {
    std::fs::read_to_string(cwd.join(".base").join("graph.nq")).unwrap_or_default()
}

#[test]
fn a_cli_note_written_after_the_dashboard_started_survives_one_dashboard_post() {
    let dir = workspace();
    let cwd = dir.path();
    crud::note::learn(cwd, &ns(), "seed note before the dashboard", "insight", Some("base"), None, None).unwrap();
    let trig_path = cwd.join(".base").join("graph.nq");
    let state = std::sync::Arc::new(base::dashboard::server::AppState::new(
        BaseConfig::load(cwd),
        cwd.to_path_buf(),
        trig_path.clone(),
        vec![trig_path],
    ));

    // The CLI writes while the server is up.
    crud::note::learn(cwd, &ns(), "cli note after the dashboard started", "insight", Some("base"), None, None).unwrap();
    assert!(graph(cwd).contains("cli note after the dashboard started"), "the CLI write must land first");

    // One dashboard write.
    let out = poll_once(base::dashboard::api::add_rule(
        State(state.clone()),
        Json(base::dashboard::api::AddRuleBody { domain: "base".into(), text: "dashboard rule after the cli write".into() }),
    ));
    assert!(out.is_ok(), "the dashboard write must succeed");

    let g = graph(cwd);
    assert!(g.contains("dashboard rule after the cli write"), "the dashboard's own write must land");
    assert!(
        g.contains("cli note after the dashboard started"),
        "the CLI write made after the server started was erased by the dashboard's write (#41)"
    );
    assert!(g.contains("seed note before the dashboard"));
}

#[test]
fn the_guard_reloads_only_when_a_source_moved() {
    let dir = workspace();
    let cwd = dir.path();
    crud::note::learn(cwd, &ns(), "first", "insight", Some("base"), None, None).unwrap();
    let trig_path = cwd.join(".base").join("graph.nq");
    let state = base::dashboard::server::AppState::new(BaseConfig::load(cwd), cwd.to_path_buf(), trig_path.clone(), vec![trig_path.clone()]);
    let before = base::dashboard::server::file_identity(&trig_path);
    drop(state.store_guard());
    assert_eq!(*state.loaded.lock().unwrap(), vec![before], "an untouched file keeps its identity");
    crud::note::learn(cwd, &ns(), "second", "insight", Some("base"), None, None).unwrap();
    let after = base::dashboard::server::file_identity(&trig_path);
    assert_ne!(before, after, "a CLI write moves the identity");
    drop(state.store_guard());
    assert_eq!(*state.loaded.lock().unwrap(), vec![after], "the guard recorded the new identity after reloading");
}
