//! Integration tests for the graph change log.
//!
//! The feature exists so an external reader (the Electron app) sees every graph
//! write. These tests hold the four properties that claim rests on: one line per
//! write, the delta round-trips, a failed mutation logs nothing, and both tiers
//! are covered.

use std::fs;
use std::path::Path;

use base::changelog::{self, Change, LOG_FILE};
use base::config::NamespaceConfig;
use base::crud;

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

/// A workspace with an empty `.base/`, the shape `load_and_mutate` requires.
fn workspace(root: &Path, name: &str) -> std::path::PathBuf {
    let ws = root.join(name);
    fs::create_dir_all(ws.join(".base")).unwrap();
    ws
}

fn log_of(ws: &Path) -> std::path::PathBuf {
    ws.join(".base").join(LOG_FILE)
}

fn lines(ws: &Path) -> Vec<serde_json::Value> {
    let path = log_of(ws);
    if !path.exists() {
        return Vec::new();
    }
    fs::read_to_string(&path)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("every log line must be valid JSON"))
        .collect()
}

// ─── (a) one write, one line ─────────────────────────────────

#[test]
fn learn_decision_and_rule_each_append_exactly_one_line() {
    let root = tempfile::tempdir().unwrap();
    let ws = workspace(root.path(), "proj");

    crud::note::learn(&ws, &ns(), "graph writes are logged", "insight", None, None, None).unwrap();
    assert_eq!(lines(&ws).len(), 1, "learn writes one line");

    crud::decision::log(&ws, &ns(), "basemode", "log every graph write", "so the app can tail it", None).unwrap();
    assert_eq!(lines(&ws).len(), 2, "decision writes one more line");

    // `rule add` genuinely performs TWO graph writes — it ensures the domain node
    // exists, then inserts the rule. Two writes, two lines: the invariant is one
    // line per graph WRITE, not one per command, and collapsing them would hide a
    // write the reader needs to see.
    crud::rule::add(&ws, &ns(), "basemode", "never log a write that did not land", None).unwrap();
    let after_rule = lines(&ws);
    assert_eq!(after_rule.len(), 4, "rule add = ensure-domain write + insert-rule write");
    assert!(after_rule[2]["sparql"].as_str().unwrap().contains("Domain"));
    assert!(after_rule[3]["sparql"].as_str().unwrap().contains("never log a write that did not land"));

    // Every record carries the tier's workspace slug and a timestamp with an offset.
    for rec in lines(&ws) {
        assert_eq!(rec["ws"], serde_json::json!("proj"));
        let at = rec["at"].as_str().unwrap();
        assert!(at.contains('T'), "RFC3339: {at}");
        assert!(
            at.contains('+') || at.contains('-') || at.ends_with('Z'),
            "timestamp needs a numeric offset: {at}"
        );
    }
}

// ─── (b) the logged sparql round-trips ───────────────────────

#[test]
fn logged_sparql_is_the_update_that_was_applied() {
    let root = tempfile::tempdir().unwrap();
    let ws = workspace(root.path(), "proj");

    crud::note::learn(&ws, &ns(), "round trip me", "insight", Some("basemode"), None, None).unwrap();

    let rec = lines(&ws).pop().unwrap();
    let sparql = rec["sparql"].as_str().expect("a real delta carries sparql");
    assert!(!rec.get("sparql_truncated").is_some_and(|v| v == true));

    // It is the actual update text, not a summary of it.
    assert!(sparql.contains("INSERT DATA"), "{sparql}");
    assert!(sparql.contains("round trip me"), "{sparql}");
    assert!(sparql.contains("PREFIX"), "prefixes are part of what was applied");

    // And it re-applies against a fresh store, which is the strongest form of
    // "this is the delta": a reader can replay it.
    let store = oxigraph::store::Store::new().unwrap();
    store.update(sparql).expect("logged sparql must be valid, applicable SPARQL");

    // The named graph it targeted is recorded alongside it.
    let expected_graph = crud::workspace_graph_iri(&ns(), &crud::workspace_slug(&ws));
    assert_eq!(rec["graph"], serde_json::json!(expected_graph));
}

// ─── (c) a failed mutation logs nothing ──────────────────────

#[test]
fn failed_mutation_logs_nothing() {
    let root = tempfile::tempdir().unwrap();
    let ws = workspace(root.path(), "proj");

    // Establish a baseline so the assertion is about *this* write, not an empty log.
    crud::note::learn(&ws, &ns(), "baseline", "insight", None, None, None).unwrap();
    let before = lines(&ws).len();
    assert_eq!(before, 1);

    // Malformed SPARQL: the update fails, so write_back is never reached.
    let err = crud::load_and_mutate(&ws, &ns(), "INSERT DATA { this is not sparql");
    assert!(err.is_err(), "the mutation must fail for this test to mean anything");
    assert_eq!(lines(&ws).len(), before, "a failed update appends nothing");

    // A write outside a workspace fails before it can touch a store at all.
    let orphan = root.path().join("no-base-here");
    fs::create_dir_all(&orphan).unwrap();
    assert!(crud::note::learn(&orphan, &ns(), "nope", "insight", None, None, None).is_err());
    assert!(!orphan.join(LOG_FILE).exists());
    assert_eq!(lines(&ws).len(), before);
}

// ─── (d) both tiers ──────────────────────────────────────────

#[test]
fn workspace_and_global_tiers_each_get_their_own_log() {
    let root = tempfile::tempdir().unwrap();
    let ws = workspace(root.path(), "proj");
    // The global tier is `~/.base-gbl` handed in as cwd — the same code path with a
    // different resolved root, which is exactly why one hook covers both.
    let gbl = workspace(root.path(), ".base-gbl");

    crud::note::learn(&ws, &ns(), "workspace tier note", "insight", None, None, None).unwrap();
    crud::note::learn(&gbl, &ns(), "global tier note", "insight", None, None, None).unwrap();

    let ws_lines = lines(&ws);
    let gbl_lines = lines(&gbl);
    assert_eq!(ws_lines.len(), 1);
    assert_eq!(gbl_lines.len(), 1);

    // Each log sits beside its own graph.nq and labels its own tier.
    assert!(log_of(&ws).exists() && log_of(&gbl).exists());
    assert_eq!(ws_lines[0]["ws"], serde_json::json!("proj"));
    assert_eq!(gbl_lines[0]["ws"], serde_json::json!("base-gbl"));

    // Neither tier's write leaked into the other's log.
    assert!(ws_lines[0]["sparql"].as_str().unwrap().contains("workspace tier note"));
    assert!(gbl_lines[0]["sparql"].as_str().unwrap().contains("global tier note"));
    assert!(!ws_lines[0]["sparql"].as_str().unwrap().contains("global tier note"));
}

// ─── reader cursor ───────────────────────────────────────────

#[test]
fn byte_offset_cursor_reads_only_what_is_new() {
    let root = tempfile::tempdir().unwrap();
    let ws = workspace(root.path(), "proj");
    let log = log_of(&ws);

    // A reader that starts before anything exists gets offset 0, not an error.
    assert_eq!(changelog::cursor(&log), 0);

    crud::note::learn(&ws, &ns(), "first", "insight", None, None, None).unwrap();
    let page = changelog::read_since(&log, 0).unwrap();
    assert_eq!(page.lines.len(), 1);
    assert!(!page.reset);

    // Resuming from the returned offset sees nothing until another write lands.
    let idle = changelog::read_since(&log, page.offset).unwrap();
    assert!(idle.lines.is_empty());

    crud::note::learn(&ws, &ns(), "second", "insight", None, None, None).unwrap();
    let next = changelog::read_since(&log, page.offset).unwrap();
    assert_eq!(next.lines.len(), 1, "only the new write, not a replay");
    assert!(next.lines[0].contains("second"));
    assert_eq!(next.offset, changelog::cursor(&log));
}

// ─── writes with no SPARQL delta still surface ───────────────

#[test]
fn deltaless_writes_are_logged_with_a_kind_not_dropped() {
    let root = tempfile::tempdir().unwrap();
    let ws = workspace(root.path(), "proj");
    let graph_path = ws.join(".base").join("graph.nq");

    let store = oxigraph::store::Store::new().unwrap();
    base::store::write_back(&store, &graph_path, Change::Op("graph.compact")).unwrap();

    let recs = lines(&ws);
    assert_eq!(recs.len(), 1, "a silent write would be worse than an unlabelled one");
    assert_eq!(recs[0]["kind"], serde_json::json!("graph.compact"));
    assert!(recs[0].get("sparql").is_none());
}
