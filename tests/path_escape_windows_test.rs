//! Regression suite for the Windows path-escape bug (fork
//! `base-sync-windows-path-escape`).
//!
//! `base sync` on Windows died on any frontmattered `.md` in a subdirectory:
//! the extractor embedded the OS-native relative path (`service\CONFIG-KEYS.md`)
//! into a SPARQL literal unescaped, and `\C` is not a valid escape.
//!
//! Every test here passes the backslashed path as a literal `&str` rather than
//! building it from the host separator, so the guard is real on Linux CI too —
//! the platform that cannot reproduce the bug is the one that has to catch it.

use oxigraph::sparql::QueryResults;
use oxigraph::store::Store;

use base::config::{BaseConfig, NamespaceConfig};
use base::crud;
use base::extract;

/// The exact path from the original bug report.
const WIN_REL: &str = r"service\CONFIG-KEYS.md";
const NIX_REL: &str = "service/CONFIG-KEYS.md";

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

fn frontmattered(title: &str) -> String {
    format!("---\ntitle: {title}\nstatus: active\n---\n\n# Body\n")
}

// ─── the helpers themselves ──────────────────────────────────────────

#[test]
fn escape_sparql_literal_escapes_backslash() {
    assert_eq!(crud::escape_sparql_literal(r"a\b"), r"a\\b");
}

#[test]
fn normalize_path_sep_rewrites_and_is_idempotent() {
    assert_eq!(crud::normalize_path_sep(WIN_REL), NIX_REL);
    assert_eq!(crud::normalize_path_sep(NIX_REL), NIX_REL);
}

/// The ordering constraint: normalize BEFORE escaping. Escaping first would
/// yield `service\\CONFIG-KEYS.md` — valid SPARQL, but a literal that never
/// equals the `service/CONFIG-KEYS.md` every other platform stores.
#[test]
fn path_literal_normalizes_before_escaping() {
    assert_eq!(crud::path_literal(WIN_REL), NIX_REL);
    assert!(!crud::path_literal(WIN_REL).contains('\\'));
}

/// A backslash is a legal filename character on Linux, so escaping still has to
/// hold for a path that is genuinely not a separator.
#[test]
fn path_literal_survives_a_quote() {
    assert_eq!(crud::path_literal(r#"a"b.md"#), r#"a\"b.md"#);
}

// ─── amendment 3: the idempotence claim, pinned ──────────────────────

/// `slugify` collapses `/` and `\` to the same `-`, so normalizing rewrites
/// literals without moving a subject IRI. This is what makes a re-sync after
/// the fix a replace rather than an append — the DELETE keys on this IRI.
#[test]
fn file_iri_is_separator_independent() {
    assert_eq!(
        extract::file_iri_from_path(&ns(), WIN_REL),
        extract::file_iri_from_path(&ns(), NIX_REL),
    );
}

// ─── the extractors ──────────────────────────────────────────────────

#[test]
fn frontmatter_stores_a_forward_slash_path() {
    let t = extract::frontmatter::extract_with_project(
        &frontmattered("Config Keys"),
        WIN_REL,
        &ns(),
        None,
    )
    .expect("frontmatter should extract");
    let (_, path) = t
        .iter()
        .find(|(p, _)| p.ends_with(":path"))
        .expect("a :path triple");
    assert_eq!(path, &format!("\"{NIX_REL}\""));
}

/// The fallback name is derived by splitting on `/`. On Windows that split
/// found nothing and named the document `service\CONFIG-KEYS`.
#[test]
fn frontmatter_derives_the_name_from_a_windows_path() {
    let no_title = "---\nstatus: active\n---\n\n# Body\n";
    let t = extract::frontmatter::extract_with_project(no_title, WIN_REL, &ns(), None)
        .expect("frontmatter should extract");
    let (_, name) = t
        .iter()
        .find(|(p, _)| p.ends_with(":name"))
        .expect("a :name triple");
    assert_eq!(name, "\"CONFIG-KEYS\"");
}

#[test]
fn paul_json_stores_a_forward_slash_path() {
    let t = extract::paul_json::extract(
        r#"{"name": "myapp", "version": "1.0"}"#,
        r"apps\myapp\.paul\paul.json",
        &ns(),
    )
    .expect("paul.json should extract");
    let (_, path) = t
        .iter()
        .find(|(p, _)| p.ends_with(":path"))
        .expect("a :path triple");
    assert_eq!(path, "\"apps/myapp/.paul/paul.json\"");
}

// ─── the round-trip guard ────────────────────────────────────────────

/// The test that would have caught the original bug on any platform: take a
/// backslashed relative path all the way through to an INSERT and hand it to
/// the parser that rejected it. Before the fix this fails with
/// `error at 7:87: expected ['t'|'b'|'n'|'r'|'f'|'"'|'\''|'\\']`.
#[test]
fn a_backslashed_path_produces_a_parseable_insert() {
    let ns = ns();
    let triples = extract::frontmatter::extract_with_project(
        &frontmattered("Config Keys"),
        WIN_REL,
        &ns,
        None,
    )
    .expect("frontmatter should extract");

    let file_iri = extract::file_iri_from_path(&ns, WIN_REL);
    let graph_iri = crud::workspace_graph_iri(&ns, "test");
    let mut body = String::new();
    for (pred, val) in &triples {
        if !pred.starts_with("ENTITY@@") {
            body.push_str(&format!("    <{file_iri}> {pred} {val} .\n"));
        }
    }
    let sparql = format!(
        "{}\nINSERT DATA {{\n  GRAPH <{graph_iri}> {{\n{body}  }}\n}}",
        crud::prefixes(&ns)
    );

    Store::new().unwrap().update(&sparql).unwrap_or_else(|e| {
        panic!("INSERT built from a backslashed path must parse: {e}\n{sparql}")
    });
}

// ─── amendment 2: read-side probes ───────────────────────────────────

/// A Windows-style probe must find the document whose stored `:filePath` is
/// forward-slash. Without normalizing the probe it parses cleanly and then
/// silently matches nothing — the same bug, one layer up. Driven through
/// `pre_tool_use::handle` rather than a mirrored expression, so it fails if the
/// production call site stops normalizing.
#[test]
fn pre_tool_use_injects_history_for_a_backslashed_probe() {
    let tmp = tempfile::tempdir().unwrap();
    let base_dir = tmp.path().join(".base");
    std::fs::create_dir_all(&base_dir).unwrap();
    let u = "http://ops-sys.local/ontology#";
    std::fs::write(
        base_dir.join("graph.nq"),
        format!(
            r#"<{u}fc/1> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <{u}FileChange> <{u}graph/ws/test> .
<{u}fc/1> <{u}filePath> "{NIX_REL}" <{u}graph/ws/test> .
<{u}fc/1> <{u}fromPlan> "P1" <{u}graph/ws/test> .
<{u}fc/1> <{u}changeType> "modified" <{u}graph/ws/test> .
"#
        ),
    )
    .unwrap();

    let event = serde_json::json!({
        "tool_name": "Edit",
        "tool_input": { "file_path": WIN_REL }
    });
    let (_, out) =
        base::hook::pre_tool_use::handle(&BaseConfig::default(), tmp.path(), &event).unwrap();

    assert!(
        out.contains("<paul-context>") && out.contains("P1"),
        "a backslashed probe should still find the forward-slash file history, got: {out}"
    );
}

/// The `post_tool_use` prefix match reduces BOTH sides to `/`, so it survives a
/// graph whose absolute Project paths were written on either platform.
/// Normalizing only the probe would break every existing Windows graph.
#[test]
fn post_tool_use_matches_a_backslashed_project_path() {
    let tmp = tempfile::tempdir().unwrap();
    let base_dir = tmp.path().join(".base");
    std::fs::create_dir_all(&base_dir).unwrap();
    let u = "http://ops-sys.local/ontology#";
    let dt = "http://www.w3.org/2001/XMLSchema#dateTime";
    // Stored exactly as a Windows machine registered it: escaped backslashes.
    std::fs::write(
        base_dir.join("graph.nq"),
        format!(
            r#"<{u}project/alpha> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <{u}Project> <{u}graph/ws/test> .
<{u}project/alpha> <{u}name> "Alpha" <{u}graph/ws/test> .
<{u}project/alpha> <{u}path> "C:\\Users\\Chris\\ws" <{u}graph/ws/test> .
<{u}project/alpha> <{u}lastActive> "2026-01-01T00:00:00-06:00"^^<{dt}> <{u}graph/ws/test> .
"#
        ),
    )
    .unwrap();

    let event = serde_json::json!({
        "tool_name": "Edit",
        "tool_input": { "file_path": r"C:\Users\Chris\ws\src\main.rs" }
    });
    base::hook::post_tool_use::handle(&BaseConfig::default(), tmp.path(), &event).unwrap();

    let store = base::store::load_graph(&base_dir.join("graph.nq")).unwrap();
    let sparql = format!("SELECT ?t WHERE {{ GRAPH ?g {{ ?e <{u}lastActive> ?t }} }}");
    match store.query(&sparql).unwrap() {
        QueryResults::Solutions(s) => {
            let stamps: Vec<String> = s
                .flatten()
                .filter_map(|r| r.get("t").map(|t| t.to_string()))
                .collect();
            assert!(!stamps.is_empty(), "lastActive should still exist");
            assert!(
                !stamps.iter().any(|t| t.contains("2026-01-01")),
                "a backslashed probe should match the backslashed project path, got: {stamps:?}"
            );
        }
        _ => panic!("expected solutions"),
    }
}

// ─── amendment 1: the AST probes ─────────────────────────────────────

/// `section_entities` splits the probe on `/` and compares it to `sourceFile`.
/// On Windows the split found nothing and the unescaped result made the query
/// unparseable, so the AST hint hooks returned empty on every tool call.
#[test]
fn ast_section_probe_accepts_a_windows_path() {
    let tmp = tempfile::tempdir().unwrap();
    let ast_dir = tmp.path().join(".base-ast");
    std::fs::create_dir_all(&ast_dir).unwrap();
    std::fs::write(
        ast_dir.join("ast.ttl"),
        r#"@prefix ops: <http://ops-sys.local/ontology#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
<http://ops-sys.local/code#entity/handle> a ops:Function ;
  rdfs:label "handle" ;
  ops:sourceFile "main.rs" ;
  ops:sourceLine 10 .
"#,
    )
    .unwrap();

    let out = crud::ast_query::section_entities(tmp.path(), &ns(), r"src\main.rs", 1, 100);
    let out = out.expect("a backslashed probe must still resolve the section");
    assert!(out.contains("handle"), "expected the entity in: {out}");
}

// ─── the whole pipeline ──────────────────────────────────────────────

/// The reported failure, end to end: a frontmattered doc in a subdirectory.
/// On Windows `rel_path` is backslashed here for real; on Linux this is the
/// control that proves the fix did not change existing behaviour.
#[test]
fn sync_extracts_a_subdirectory_doc_and_stays_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    std::fs::create_dir_all(tmp.path().join("service")).unwrap();
    std::fs::write(
        tmp.path().join("service").join("CONFIG-KEYS.md"),
        frontmattered("Config Keys"),
    )
    .unwrap();

    let config = BaseConfig::default();
    let report = extract::sync(tmp.path(), &config, false).unwrap();
    assert!(report.extracted >= 1, "the subdirectory doc should extract");

    let graph = tmp.path().join(".base").join("graph.nq");
    let store = base::store::load_graph(&graph).unwrap();
    let p = ns().prefix;
    let u = ns().uri;

    let ask = format!(
        "PREFIX {p}: <{u}>\nASK {{ GRAPH ?g {{ ?doc a {p}:Document ; {p}:path \"{NIX_REL}\" }} }}"
    );
    match store.query(&ask).unwrap() {
        QueryResults::Boolean(yes) => {
            assert!(yes, "the doc should be stored under a forward-slash path")
        }
        _ => panic!("expected boolean"),
    }

    let count1 = store.len().unwrap();
    extract::sync(tmp.path(), &config, false).unwrap();
    let count2 = base::store::load_graph(&graph).unwrap().len().unwrap();
    assert_eq!(count1, count2, "re-sync must replace, not append");
}
