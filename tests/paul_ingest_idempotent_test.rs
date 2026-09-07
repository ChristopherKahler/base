//! F25 — re-ingesting an unchanged `paul.toml` must not touch the store.
//!
//! The ingest used to refresh `updatedAt` on every scanned project and then call
//! `write_back` unconditionally, so a 15 MB `graph.nq` was rewritten on every
//! session start to change four timestamps. That moved the store's identity, which
//! re-opened `migrate_tiers`' delta gate (`changed_since_last_delta` compares
//! `len:mtime_nanos` against `.last-domain-delta`), which bought a full re-plan of
//! the whole store that was then discarded because the plan was empty. Measured on
//! Chris's frozen copy: 18 session-starts produced 18 `extract.paul_toml` changelog
//! entries and 0 `migrate.domain-1.delta` entries.
//!
//! Both assertions below fail on the pre-fix ingest: the changelog grows on every
//! call and the bytes differ every time.

use std::fs;
use std::path::Path;

use base::config::{BaseConfig, NamespaceConfig, WorkspaceEntry};
use base::extract::paul_toml::{ingest_paul_projects, scan_all_workspaces};

fn workspace_with_project(root: &Path, milestone: Option<&str>) -> BaseConfig {
    let ws = root.join("ws");
    fs::create_dir_all(ws.join(".base")).unwrap();
    fs::write(ws.join(".base").join("graph.nq"), "").unwrap();

    let proj = ws.join("demo");
    fs::create_dir_all(proj.join(".paul")).unwrap();
    let mut toml = "name = \"Demo\"\nstatus = \"active\"\npath = \"demo\"\n".to_string();
    if let Some(m) = milestone {
        toml.push_str(&format!(
            "\n[milestone]\nname = \"{m}\"\nversion = \"v1\"\nstatus = \"active\"\n"
        ));
    }
    fs::write(proj.join(".paul").join("paul.toml"), toml).unwrap();

    BaseConfig {
        namespace: NamespaceConfig::default(),
        workspace: vec![WorkspaceEntry { path: ws.to_string_lossy().into() }],
        ..Default::default()
    }
}

fn changelog_lines(ws: &Path) -> usize {
    match fs::read_to_string(ws.join(".base").join("changes.jsonl")) {
        Ok(s) => s.lines().filter(|l| !l.trim().is_empty()).count(),
        Err(_) => 0,
    }
}

fn ingest(config: &BaseConfig, ws: &Path) -> usize {
    let projects = scan_all_workspaces(config);
    assert!(!projects.is_empty(), "the scan must find the project");
    ingest_paul_projects(ws, config, &projects).unwrap().registered
}

#[test]
fn a_second_ingest_of_an_unchanged_paul_toml_touches_nothing() {
    let root = tempfile::tempdir().unwrap();
    let config = workspace_with_project(root.path(), None);
    let ws = root.path().join("ws");
    let graph = ws.join(".base").join("graph.nq");

    // First ingest: the project is new, so it is written and counted.
    assert_eq!(ingest(&config, &ws), 1, "a new project is ingested");
    let after_first = fs::read(&graph).unwrap();
    let log_after_first = changelog_lines(&ws);
    assert!(!after_first.is_empty(), "the first ingest writes the project");
    assert_eq!(log_after_first, 1, "one changelog entry for the first ingest");

    // Second ingest, nothing changed on disk. Not one byte may move, and the
    // changelog must not gain an entry — a no-op write is still a write.
    assert_eq!(ingest(&config, &ws), 0, "an unchanged project is not re-ingested");
    assert_eq!(
        fs::read(&graph).unwrap(),
        after_first,
        "graph.nq must be byte-identical after a no-op ingest"
    );
    assert_eq!(
        changelog_lines(&ws),
        log_after_first,
        "a no-op ingest must not append a changelog entry"
    );

    // A third, also unchanged: the guard must hold across repeats, not just once.
    assert_eq!(ingest(&config, &ws), 0);
    assert_eq!(fs::read(&graph).unwrap(), after_first);
    assert_eq!(changelog_lines(&ws), log_after_first);
}

/// F25's actual cause, found 2026-09-07 with the in-process instrument.
///
/// Chris's `My Documents` junction sits beside `Documents`, so `scan_all_workspaces`
/// returned the same `paul.toml` twice under two paths. The ingest loop ran twice for
/// one IRI and the two passes overwrote each other's `path` quad, so the guard saw a
/// real difference on BOTH passes, both restamped, and every session start rewrote the
/// store. The second pass put `path` back where the first found it, which is why every
/// between-session diff reported "only `updatedAt` moved" and the cause survived three
/// refuted hypotheses.
///
/// unix-only because the reproduction needs a second path to one directory and creating
/// one on Windows needs a privilege a test cannot assume. The defect is not unix-only.
#[cfg(unix)]
#[test]
fn a_project_reachable_by_two_paths_is_ingested_once() {
    let root = tempfile::tempdir().unwrap();
    let config = workspace_with_project(root.path(), None);
    let ws = root.path().join("ws");
    // A second directory in the same workspace reaching the same project, exactly what
    // the junction does on Chris's machine.
    std::os::unix::fs::symlink(ws.join("demo"), ws.join("demo-link")).unwrap();

    let projects = scan_all_workspaces(&config);
    assert_eq!(
        projects.len(),
        1,
        "one paul.toml was scanned twice: {:?}",
        projects.iter().map(|(p, _)| p.display().to_string()).collect::<Vec<_>>()
    );

    let graph = ws.join(".base").join("graph.nq");
    assert_eq!(ingest(&config, &ws), 1, "one project is one registration, not two");
    let after_first = fs::read(&graph).unwrap();
    let log_after_first = changelog_lines(&ws);

    // The whole point: with the duplicate gone the second ingest is a no-op again.
    assert_eq!(ingest(&config, &ws), 0, "the second ingest must touch nothing");
    assert_eq!(
        fs::read(&graph).unwrap(),
        after_first,
        "graph.nq moved on a no-op ingest of a duplicated project"
    );
    assert_eq!(
        changelog_lines(&ws),
        log_after_first,
        "a no-op ingest appended a changelog entry"
    );
}

#[test]
fn a_changed_paul_toml_is_still_ingested_and_restamped() {
    let root = tempfile::tempdir().unwrap();
    let config = workspace_with_project(root.path(), None);
    let ws = root.path().join("ws");
    let graph = ws.join(".base").join("graph.nq");

    assert_eq!(ingest(&config, &ws), 1);
    let before = fs::read_to_string(&graph).unwrap();
    let log_before = changelog_lines(&ws);
    let updated_before = updated_at(&before);

    // Add a milestone: a real change, so the store moves and `updatedAt` is
    // restamped. The guard must not have turned the ingest into a no-op writer.
    let proj = ws.join("demo");
    fs::write(
        proj.join(".paul").join("paul.toml"),
        "name = \"Demo\"\nstatus = \"active\"\npath = \"demo\"\n\
         \n[milestone]\nname = \"Ship\"\nversion = \"v1\"\nstatus = \"active\"\n",
    )
    .unwrap();

    // `updatedAt` is written at second resolution (`crud::now_iso` uses
    // `SecondsFormat::Secs`) and this test runs in ~10 ms, so both ingests would
    // otherwise stamp the SAME literal and the restamp assertion below would fail
    // even though the restamp fired. Cross a real second boundary so the assertion
    // measures the restamp and not the clock resolution; 1100 ms crosses at least
    // one tick from any starting offset.
    std::thread::sleep(std::time::Duration::from_millis(1100));

    assert_eq!(ingest(&config, &ws), 1, "a changed project is re-ingested");
    let after = fs::read_to_string(&graph).unwrap();
    assert_ne!(after, before, "a real change must reach the store");
    assert!(after.contains("Milestone: Ship"), "the new milestone is in the store");
    assert_eq!(changelog_lines(&ws), log_before + 1, "one entry for one real change");
    assert_ne!(updated_at(&after), updated_before, "a real change restamps updatedAt");
}

/// The `updatedAt` literal for `project/demo`, or empty when absent.
fn updated_at(graph: &str) -> String {
    graph
        .lines()
        .find(|l| l.contains("project/demo>") && l.contains("updatedAt>"))
        .unwrap_or_default()
        .to_string()
}
