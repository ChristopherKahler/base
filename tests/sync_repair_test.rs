//! `base sync --repair` must land the edges it computes.
//!
//! 0.13.17 computed the right repairs, applied them to the in-memory store, printed
//! each as `+ parent → predicate → child`, then joined every statement — each with
//! its own `PREFIX` block — into ONE update for the write. SPARQL allows a single
//! prologue, so the batch failed to parse at the second `PREFIX`: exit 1, the `+`
//! lines already on stdout, and the store byte-identical. These tests hold the
//! contract the fix restores: repairs reach disk, a run that finds nothing writes
//! nothing, and the change record names the edges.

use std::fs;
use std::path::{Path, PathBuf};

use base::changelog::LOG_FILE;
use base::config::NamespaceConfig;
use base::crud;
use oxigraph::sparql::QueryResults;

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

/// A workspace whose store holds `n` decisions filed under `domain/alpha` by slug
/// (`alpha.dec-<i>`) but with no `hasDecision` edge — the shape a store written
/// before edges existed left behind, and the one `--repair` is for. Plus one
/// decision whose parent domain does not exist, which must be left alone.
fn workspace(home: &Path, n: usize) -> PathBuf {
    let ws = home.join("proj");
    let base = ws.join(".base");
    fs::create_dir_all(&base).unwrap();
    let u = ns().uri.clone();
    let g = crud::workspace_graph_iri(&ns(), "proj");
    let mut body = String::new();
    body += &format!("<{u}domain/alpha> <{RDF_TYPE}> <{u}Domain> <{g}> .\n");
    body += &format!("<{u}domain/alpha> <{u}name> \"alpha\" <{g}> .\n");
    for i in 0..n {
        body += &format!("<{u}decision/alpha.dec-{i}> <{RDF_TYPE}> <{u}Decision> <{g}> .\n");
        body += &format!("<{u}decision/alpha.dec-{i}> <{u}name> \"decision {i}\" <{g}> .\n");
    }
    body += &format!("<{u}decision/ghost.dec-x> <{RDF_TYPE}> <{u}Decision> <{g}> .\n");
    fs::write(base.join("graph.nq"), body).unwrap();
    ws
}

fn graph_file(ws: &Path) -> PathBuf {
    ws.join(".base").join("graph.nq")
}

/// `hasDecision` edges on disk — read back from the file, never from memory.
fn edges_on_disk(ws: &Path) -> Vec<String> {
    let store = base::store::load_graph(&graph_file(ws)).unwrap();
    let q = format!(
        "PREFIX {p}: <{u}>\nSELECT ?parent ?d WHERE {{ GRAPH ?g {{ ?parent {p}:hasDecision ?d }} }}",
        p = ns().prefix,
        u = ns().uri
    );
    let QueryResults::Solutions(sols) = store.query(&q).unwrap() else { panic!("expected solutions") };
    let mut out: Vec<String> = sols
        .filter_map(|r| r.ok())
        .map(|row| format!("{} {}", row.get("parent").unwrap(), row.get("d").unwrap()))
        .collect();
    out.sort();
    out
}

fn change_records(ws: &Path) -> Vec<serde_json::Value> {
    let log = ws.join(".base").join(LOG_FILE);
    if !log.exists() {
        return Vec::new();
    }
    fs::read_to_string(&log)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("every log line is JSON"))
        .collect()
}

#[test]
fn two_pending_repairs_both_land_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let ws = workspace(tmp.path(), 2);
        let before = fs::read(graph_file(&ws)).unwrap();
        assert!(edges_on_disk(&ws).is_empty(), "precondition: no edges yet");

        let lines = crud::repair_edges(&ws, &ns()).unwrap();

        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines.iter().all(|l| l.starts_with("alpha → hasDecision → dec-")), "{lines:?}");
        let edges = edges_on_disk(&ws);
        assert_eq!(edges.len(), 2, "both repairs must be in the file: {edges:?}");
        assert!(edges.iter().all(|e| e.contains("domain/alpha") && e.contains("decision/alpha.dec-")), "{edges:?}");
        assert_ne!(fs::read(graph_file(&ws)).unwrap(), before, "the store must change on disk");
    });
}

#[test]
fn a_second_run_finds_nothing_and_leaves_the_store_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let ws = workspace(tmp.path(), 2);
        crud::repair_edges(&ws, &ns()).unwrap();
        let after_first = fs::read(graph_file(&ws)).unwrap();
        let records_after_first = change_records(&ws).len();

        let lines = crud::repair_edges(&ws, &ns()).unwrap();

        assert!(lines.is_empty(), "nothing left to repair: {lines:?}");
        assert_eq!(fs::read(graph_file(&ws)).unwrap(), after_first, "an empty repair must not rewrite the store");
        assert_eq!(change_records(&ws).len(), records_after_first, "an empty repair leaves no change record");
    });
}

#[test]
fn an_orphan_without_a_parent_is_skipped_not_invented() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let ws = workspace(tmp.path(), 0);
        let before = fs::read(graph_file(&ws)).unwrap();

        let lines = crud::repair_edges(&ws, &ns()).unwrap();

        assert!(lines.is_empty(), "{lines:?}");
        assert!(edges_on_disk(&ws).is_empty());
        assert_eq!(fs::read(graph_file(&ws)).unwrap(), before);
        assert!(!fs::read_to_string(graph_file(&ws)).unwrap().contains("domain/ghost"), "no parent may be conjured");
    });
}

#[test]
fn a_workspace_without_a_store_is_refused_and_no_store_is_created() {
    // Absent is not empty: 0.13.17 answered "Repair complete: 0 edges backfilled"
    // here and left a new, empty graph.nq behind.
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let ws = tmp.path().join("proj");
        fs::create_dir_all(ws.join(".base")).unwrap();

        let err = crud::repair_edges(&ws, &ns()).unwrap_err().to_string();

        assert!(err.contains("nothing to repair"), "{err}");
        assert!(!graph_file(&ws).exists(), "no store may be conjured by a repair");
    });
}

#[test]
fn the_write_leaves_one_change_record_naming_every_edge() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let ws = workspace(tmp.path(), 3);
        crud::repair_edges(&ws, &ns()).unwrap();

        let records = change_records(&ws);
        assert_eq!(records.len(), 1, "one write, one record: {records:?}");
        let sparql = records[0]["sparql"].as_str().expect("the record carries the statements");
        assert_eq!(sparql.matches("hasDecision").count(), 3, "every edge is named in the record: {sparql}");
        assert_eq!(sparql.matches("INSERT DATA").count(), 3);
    });
}
