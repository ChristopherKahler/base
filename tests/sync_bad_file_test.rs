//! #162: one file `base sync` cannot extract no longer stops the sync.
//!
//! Before: the `?` on the file's INSERT left the file loop, so every file after it was not read and every file before it
//! was not written (the graph is written once, at the end). Now the file's triples are parsed before anything of it is
//! deleted: a file that does not parse is skipped and named (file, field, value), its earlier triples stay, the rest are
//! written, and `base sync` exits 3, partial.
//!
//! The bad value here is a `related:` entry holding `|`, which no IRI may carry. (Obsidian wikilinks were the reported
//! way in; since #162 they parse, so they cannot be the fixture for a file that does not.)

use std::path::Path;
use std::process::{Command, Stdio};

use oxigraph::sparql::QueryResults;

use base::config::{BaseConfig, NamespaceConfig};
use base::extract;

const BIN: &str = env!("CARGO_BIN_EXE_base");

fn write_md(dir: &Path, rel: &str, frontmatter: &str) {
    let full = dir.join(rel);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(&full, format!("---\n{frontmatter}\n---\n\n# body\n")).unwrap();
}

/// Every `name` in the workspace graph, sorted.
fn names(ws: &Path) -> Vec<String> {
    let store = base::store::load_graph(&ws.join(".base").join("graph.nq")).unwrap();
    let ns = NamespaceConfig::default();
    let q = format!("PREFIX {p}: <{u}>\nSELECT ?n WHERE {{ GRAPH ?g {{ ?d {p}:name ?n }} }}", p = ns.prefix, u = ns.uri);
    let QueryResults::Solutions(rows) = store.query(&q).unwrap() else { panic!("a SELECT") };
    let mut out: Vec<String> = rows
        .map(|r| r.unwrap().get("n").map(|t| base::crud::term_display(t.into())).unwrap_or_default())
        .collect();
    out.sort();
    out
}

/// The `relatedTo` targets of the document at `rel`, sorted.
fn related(ws: &Path, rel: &str) -> Vec<String> {
    let store = base::store::load_graph(&ws.join(".base").join("graph.nq")).unwrap();
    let ns = NamespaceConfig::default();
    let doc = extract::file_iri_from_path(&ns, rel);
    let q = format!("PREFIX {p}: <{u}>\nSELECT ?t WHERE {{ GRAPH ?g {{ <{doc}> {p}:relatedTo ?t }} }}", p = ns.prefix, u = ns.uri);
    let QueryResults::Solutions(rows) = store.query(&q).unwrap() else { panic!("a SELECT") };
    let mut out: Vec<String> = rows.map(|r| r.unwrap().get("t").unwrap().to_string()).collect();
    out.sort();
    out
}

fn workspace() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    tmp
}

#[test]
fn one_bad_file_does_not_stop_the_sync() {
    let tmp = workspace();
    let ws = tmp.path();
    write_md(ws, "a.md", "title: A one");
    write_md(ws, "b.md", "title: B good\nrelated: alpha");
    write_md(ws, "c.md", "title: C one");
    let config = BaseConfig::default();
    let first = extract::sync(ws, &config, false).unwrap();
    assert!(first.unextractable.is_empty(), "control: all three extract");
    assert_eq!(names(ws), ["A one", "B good", "C one"]);

    // b.md, sorted between the other two, now holds a value no IRI can carry; a.md and c.md change too, so the second
    // sync's writes are visible on both sides of it.
    write_md(ws, "a.md", "title: A two");
    write_md(ws, "b.md", "title: B bad\nrelated: bad|value");
    write_md(ws, "c.md", "title: C two");
    let report = extract::sync(ws, &config, false).unwrap();

    assert_eq!(report.extracted, 2, "the files before and after the bad one are written");
    assert_eq!(report.unextractable.len(), 1);
    let bad = &report.unextractable[0];
    assert_eq!(bad.file, "b.md");
    assert_eq!(bad.field, "relatedTo");
    assert_eq!(bad.value, "bad|value", "the value as the file gave it, not the link text");
    assert_eq!(
        bad.to_string(),
        "base sync skipped b.md: its relatedTo value \"bad|value\" cannot be stored in the graph. Change that value in the file, then run base sync again."
    );
    // b.md's earlier triples stay: nothing of a file that does not parse is deleted.
    assert_eq!(names(ws), ["A two", "B good", "C two"]);

    // `lastExtracted` did not move, so an incremental sync retries b.md, and only it.
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(120);
    std::fs::File::options().write(true).open(ws.join("b.md")).unwrap().set_modified(later).unwrap();
    let again = extract::sync(ws, &config, true).unwrap();
    assert_eq!(again.unextractable.len(), 1, "b.md is still reported");
    assert_eq!(again.extracted, 0);
}

#[test]
fn the_162_reproduction_syncs_with_edges_like_bare_values() {
    let tmp = workspace();
    let ws = tmp.path();
    write_md(ws, "note.md", "related:\n  - \"[[alpha]]\"\n  - \"[[beta]]\"");
    write_md(ws, "bare.md", "related:\n  - alpha\n  - beta");
    let report = extract::sync(ws, &BaseConfig::default(), false).unwrap();
    assert!(report.unextractable.is_empty(), "{:?}", report.unextractable.iter().map(ToString::to_string).collect::<Vec<_>>());
    assert_eq!(report.extracted, 2);
    let edges = related(ws, "note.md");
    assert_eq!(edges.len(), 2, "{edges:?}");
    assert_eq!(edges, related(ws, "bare.md"));
}

/// The process, not a return value: the code a script or CI step sees.
fn run_sync(ws: &Path, home: &Path) -> (i32, String, String) {
    let out = Command::new(BIN)
        .arg("sync")
        .current_dir(ws)
        .env("BASE_HOME", home)
        .env("BASE_NO_AUTO_UPDATE", "1")
        .env("BASE_AST_NO_SPAWN", "1")
        .env_remove("BASE_RELAY_AS")
        .env_remove("CLAUDECODE")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn a_partial_sync_exits_3_and_names_the_file() {
    let home = tempfile::tempdir().unwrap();
    let ws = home.path().join("ws");
    std::fs::create_dir_all(ws.join(".base")).unwrap();
    write_md(&ws, "good.md", "title: Good");

    let (code, out, err) = run_sync(&ws, home.path());
    assert_eq!(code, 0, "control: a clean sync exits 0\n{out}\n{err}");
    assert!(out.contains("Sync complete: 1 scanned, 1 extracted, 0 skipped"), "{out}");

    write_md(&ws, "zz-bad.md", "related: bad|value");
    let (code, out, err) = run_sync(&ws, home.path());
    assert_eq!(code, 3, "partial\n{out}\n{err}");
    assert!(
        out.contains(
            "Sync partial: 2 scanned, 1 extracted, 0 skipped. 1 file was not synced (named above); the graph still has what it held for it before. Fix it, then run base sync again."
        ),
        "{out}"
    );
    assert!(err.lines().any(|l| l.starts_with("base sync skipped zz-bad.md: its relatedTo value \"bad|value\"")), "{err}");
}
