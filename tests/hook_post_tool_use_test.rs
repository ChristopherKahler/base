use std::path::Path;

use oxigraph::sparql::QueryResults;

use base::config::BaseConfig;
use base::hook::post_tool_use;

/// Helper: create a workspace graph (NQuads) with a project that has ops:path set.
///
/// The path is escaped, not interpolated raw: on Windows a tempdir path is
/// `C:\Users\...`, and an unescaped backslash makes the FIXTURE ITSELF invalid
/// N-Quads, so the strict `load_graph` fails before the assertion under test is
/// ever reached.
fn write_trig_with_path(dir: &Path, project_path: &str) {
    let project_path = base::crud::escape_sparql_literal(project_path);
    let base_dir = dir.join(".base");
    std::fs::create_dir_all(&base_dir).unwrap();
    std::fs::write(
        base_dir.join("graph.nq"),
        format!(
            r#"<http://ops-sys.local/ontology#project/alpha> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ops-sys.local/ontology#Project> <http://ops-sys.local/ontology#graph/ws/test> .
<http://ops-sys.local/ontology#project/alpha> <http://ops-sys.local/ontology#name> "Alpha" <http://ops-sys.local/ontology#graph/ws/test> .
<http://ops-sys.local/ontology#project/alpha> <http://ops-sys.local/ontology#status> "active" <http://ops-sys.local/ontology#graph/ws/test> .
<http://ops-sys.local/ontology#project/alpha> <http://ops-sys.local/ontology#path> "{project_path}" <http://ops-sys.local/ontology#graph/ws/test> .
<http://ops-sys.local/ontology#project/alpha> <http://ops-sys.local/ontology#lastActive> "2026-01-01T00:00:00-06:00"^^<http://www.w3.org/2001/XMLSchema#dateTime> <http://ops-sys.local/ontology#graph/ws/test> .
"#
        ),
    )
    .unwrap();
}

#[test]
fn post_tool_use_updates_last_active() {
    let tmp = tempfile::tempdir().unwrap();
    let project_path = tmp.path().to_str().unwrap();
    write_trig_with_path(tmp.path(), project_path);

    let config = BaseConfig::default();
    let event = serde_json::json!({
        "tool_name": "Edit",
        "tool_input": {
            "file_path": format!("{}/src/main.rs", project_path)
        }
    });

    let result = post_tool_use::handle(&config, tmp.path(), &event);
    assert!(result.is_ok(), "post-tool-use should succeed: {result:?}");

    // Reload the TriG and verify lastActive was updated
    let trig_path = tmp.path().join(".base").join("graph.nq");
    let store = base::store::load_graph(&trig_path).unwrap();

    let sparql = r#"
        PREFIX ops: <http://ops-sys.local/ontology#>
        SELECT ?lastActive WHERE {
            GRAPH ?g {
                ?entity ops:lastActive ?lastActive .
            }
        }
    "#;

    match store.query(sparql).unwrap() {
        QueryResults::Solutions(solutions) => {
            let timestamps: Vec<String> = solutions
                .filter_map(|s| s.ok())
                .filter_map(|s| s.get("lastActive").map(|t| t.to_string()))
                .collect();
            assert!(!timestamps.is_empty(), "Should have a lastActive timestamp");
            // Should NOT still be the old 2026-01-01 value
            assert!(
                !timestamps.iter().any(|t| t.contains("2026-01-01")),
                "lastActive should be updated from original, got: {timestamps:?}"
            );
        }
        _ => panic!("Expected solutions"),
    }
}

#[test]
fn post_tool_use_no_match_no_mutation() {
    let tmp = tempfile::tempdir().unwrap();
    // Project path is /some/other/dir — won't match the file path
    write_trig_with_path(tmp.path(), "/some/other/dir");

    let config = BaseConfig::default();
    let event = serde_json::json!({
        "tool_name": "Edit",
        "tool_input": {
            "file_path": "/completely/different/path/main.rs"
        }
    });

    let result = post_tool_use::handle(&config, tmp.path(), &event);
    assert!(result.is_ok(), "Should succeed even with no match");

    // Verify lastActive is UNCHANGED (still original value)
    let trig_path = tmp.path().join(".base").join("graph.nq");
    let store = base::store::load_graph(&trig_path).unwrap();

    let sparql = r#"
        PREFIX ops: <http://ops-sys.local/ontology#>
        SELECT ?lastActive WHERE {
            GRAPH ?g {
                ?entity ops:lastActive ?lastActive .
            }
        }
    "#;

    match store.query(sparql).unwrap() {
        QueryResults::Solutions(solutions) => {
            let timestamps: Vec<String> = solutions
                .filter_map(|s| s.ok())
                .filter_map(|s| s.get("lastActive").map(|t| t.to_string()))
                .collect();
            assert!(
                timestamps.iter().any(|t| t.contains("2026-01-01")),
                "lastActive should still be original when no path match, got: {timestamps:?}"
            );
        }
        _ => panic!("Expected solutions"),
    }
}

#[test]
fn post_tool_use_no_trig_silent() {
    let tmp = tempfile::tempdir().unwrap();
    // No .base/ directory
    let config = BaseConfig::default();
    let event = serde_json::json!({
        "tool_name": "Write",
        "tool_input": { "file_path": "/some/file.rs" }
    });

    let result = post_tool_use::handle(&config, tmp.path(), &event);
    assert!(result.is_ok(), "Should succeed silently with no TriG");
}

#[test]
fn post_tool_use_no_file_paths_silent() {
    let tmp = tempfile::tempdir().unwrap();
    write_trig_with_path(tmp.path(), tmp.path().to_str().unwrap());

    let config = BaseConfig::default();
    // WebSearch has no file_path
    let event = serde_json::json!({
        "tool_name": "WebSearch",
        "tool_input": { "query": "something" }
    });

    let result = post_tool_use::handle(&config, tmp.path(), &event);
    assert!(result.is_ok(), "Should succeed silently with no file paths");
}

// ─────────────────────────────────────────────────────────────────────────────
// RANK 03 — the `lastActive` clock on handoff and fork records.
//
// Measured on the operator's machine, 2026-09-14, both tiers: 339 records carry
// `handoffDoc` (120 `kind:handoff`, 219 `kind:fork`) and all 339 have `lastActive`
// exactly equal to `createdAt`. Not one has ever recorded a touch.
//
// The control that makes that a defect and not a guess: in the same pass, 74 records
// carrying `path` HAVE moved. The update works; it is blind to `handoffDoc`.
//
// Every test below asserts the fixture loaded BEFORE asserting anything about the
// behaviour, because a test that silently loaded an empty graph would otherwise report
// "not updated" for both the broken and the fixed build.
// ─────────────────────────────────────────────────────────────────────────────

/// Write a graph holding one handoff record whose doc is `doc_path`.
///
/// `handoffDoc` is stored forward-slashed on the real machine
/// (`"C:/Users/Chris/.base-gbl/handoffs/2026-09-10-1130-auk-base.md"`), unlike `path`,
/// which is stored backslashed. The fixture matches the real storage format, escaped so
/// a Windows tempdir path cannot make the fixture itself invalid N-Quads.
fn write_graph_with_handoff(base_dir: &Path, slug: &str, doc_path: &str) {
    let doc = base::crud::escape_sparql_literal(doc_path);
    std::fs::create_dir_all(base_dir).unwrap();
    std::fs::write(
        base_dir.join("graph.nq"),
        format!(
            r#"<http://ops-sys.local/ontology#handoff/{slug}> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ops-sys.local/ontology#Handoff> <http://ops-sys.local/ontology#graph/ws/test> .
<http://ops-sys.local/ontology#handoff/{slug}> <http://ops-sys.local/ontology#kind> "handoff" <http://ops-sys.local/ontology#graph/ws/test> .
<http://ops-sys.local/ontology#handoff/{slug}> <http://ops-sys.local/ontology#status> "open" <http://ops-sys.local/ontology#graph/ws/test> .
<http://ops-sys.local/ontology#handoff/{slug}> <http://ops-sys.local/ontology#handoffDoc> "{doc}" <http://ops-sys.local/ontology#graph/ws/test> .
<http://ops-sys.local/ontology#handoff/{slug}> <http://ops-sys.local/ontology#createdAt> "2026-01-01T00:00:00-06:00"^^<http://www.w3.org/2001/XMLSchema#dateTime> <http://ops-sys.local/ontology#graph/ws/test> .
<http://ops-sys.local/ontology#handoff/{slug}> <http://ops-sys.local/ontology#lastActive> "2026-01-01T00:00:00-06:00"^^<http://www.w3.org/2001/XMLSchema#dateTime> <http://ops-sys.local/ontology#graph/ws/test> .
"#
        ),
    )
    .unwrap();
}

/// Every `lastActive` value in a graph file, as strings.
///
/// Returns a Vec so the caller can assert on the SIZE it visited. An empty Vec means the
/// fixture did not load, which is never a pass.
fn last_active_values(trig_path: &Path) -> Vec<String> {
    let store = base::store::load_graph(trig_path).unwrap();
    let sparql = r#"
        PREFIX ops: <http://ops-sys.local/ontology#>
        SELECT ?lastActive WHERE { GRAPH ?g { ?entity ops:lastActive ?lastActive . } }
    "#;
    match store.query(sparql).unwrap() {
        QueryResults::Solutions(solutions) => solutions
            .filter_map(|s| s.ok())
            .filter_map(|s| s.get("lastActive").map(|t| t.to_string()))
            .collect(),
        _ => panic!("expected solutions"),
    }
}

fn read_event(file_path: &str) -> serde_json::Value {
    serde_json::json!({
        "tool_name": "Read",
        "tool_input": { "file_path": file_path }
    })
}

/// RANK 03, the core defect: opening a handoff doc must move its `lastActive`.
///
/// RED at `30eb876`: `post_tool_use.rs` matches `?entity ops:path ?path` only, and the
/// string `handoffDoc` does not occur in the file. The record is found, the doc is
/// opened, and the clock does not move.
#[test]
fn reading_a_handoff_doc_updates_its_last_active() {
    let tmp = tempfile::tempdir().unwrap();
    let doc = tmp.path().join("handoffs").join("2026-09-14-plover-base.md");
    std::fs::create_dir_all(doc.parent().unwrap()).unwrap();
    std::fs::write(&doc, "# handoff\n").unwrap();

    let base_dir = tmp.path().join(".base");
    write_graph_with_handoff(&base_dir, "2026-09-14-plover-base", doc.to_str().unwrap());
    let trig = base_dir.join("graph.nq");

    // POSITIVE CONTROL: the fixture is there and carries the original timestamp. Without
    // this, "no 2026-01-01 found" would pass on an empty graph.
    let before = last_active_values(&trig);
    assert_eq!(before.len(), 1, "fixture must load exactly one record, got {before:?}");
    assert!(
        before[0].contains("2026-01-01"),
        "fixture must start at the original timestamp, got {before:?}"
    );

    let config = BaseConfig::default();
    let result = post_tool_use::handle(&config, tmp.path(), &read_event(doc.to_str().unwrap()));
    assert!(result.is_ok(), "post-tool-use should succeed: {result:?}");

    let after = last_active_values(&trig);
    assert_eq!(after.len(), 1, "still exactly one record, got {after:?}");
    assert!(
        !after[0].contains("2026-01-01"),
        "opening the handoff doc must move lastActive off its created value, got {after:?}"
    );
}

/// RANK 03, the tier trap: the record lives in the GLOBAL tier, the cwd is a workspace.
///
/// RED at `30eb876` for a second, independent reason: the update loaded only
/// `find_workspace_trig(cwd)`. Measured 2026-09-14, 64 of the 339 affected records live
/// in the global tier, and a workspace cwd is exactly how they are opened.
///
/// This test would still fail on a fix that added the `handoffDoc` predicate but kept a
/// single tier, which is the whole reason it is separate from the test above.
#[test]
fn reading_a_global_tier_handoff_from_a_workspace_cwd_updates_it() {
    let tmp = tempfile::tempdir().unwrap();

    // The workspace tier: present, and deliberately holding no handoff at all.
    let ws = tmp.path().join("workspace");
    let ws_base = ws.join(".base");
    std::fs::create_dir_all(&ws_base).unwrap();
    std::fs::write(ws_base.join("graph.nq"), "").unwrap();

    // The global tier, where the record actually lives.
    let gbl_base = tmp.path().join(".base-gbl").join(".base");
    let doc = tmp.path().join("handoffs").join("2026-09-14-auk-base.md");
    std::fs::create_dir_all(doc.parent().unwrap()).unwrap();
    std::fs::write(&doc, "# handoff\n").unwrap();
    write_graph_with_handoff(&gbl_base, "2026-09-14-auk-base", doc.to_str().unwrap());
    let gbl_trig = gbl_base.join("graph.nq");

    let before = last_active_values(&gbl_trig);
    assert_eq!(before.len(), 1, "global fixture must load, got {before:?}");
    assert!(before[0].contains("2026-01-01"));

    let config = BaseConfig::default();
    // `with_thread_home` points `home_root()` at the tempdir for this thread only, so the
    // global tier under test is the fixture's and never the operator's.
    let result = base::home::with_thread_home(tmp.path(), || {
        post_tool_use::handle(&config, &ws, &read_event(doc.to_str().unwrap()))
    });
    assert!(result.is_ok(), "post-tool-use should succeed: {result:?}");

    let after = last_active_values(&gbl_trig);
    assert_eq!(after.len(), 1, "still exactly one record, got {after:?}");
    assert!(
        !after[0].contains("2026-01-01"),
        "a global-tier handoff opened from a workspace cwd must be updated in the tier \
         that holds it, got {after:?}"
    );
}

/// RANK 03, the discriminator: `handoffDoc` stores a FILE, so the match is EQUALITY.
///
/// This is the control that separates a real fix from a prefix match. `path` stores a
/// directory and is matched with STRSTARTS, which is correct for a directory; copying
/// that shape onto `handoffDoc` would let `<doc>.md.bak` register as a touch of
/// `<doc>.md`. A fix built with STRSTARTS passes the two tests above and fails this one.
#[test]
fn a_neighbouring_file_does_not_touch_the_handoff_record() {
    let tmp = tempfile::tempdir().unwrap();
    let doc = tmp.path().join("handoffs").join("2026-09-14-plover-base.md");
    std::fs::create_dir_all(doc.parent().unwrap()).unwrap();
    std::fs::write(&doc, "# handoff\n").unwrap();

    let base_dir = tmp.path().join(".base");
    write_graph_with_handoff(&base_dir, "2026-09-14-plover-base", doc.to_str().unwrap());
    let trig = base_dir.join("graph.nq");

    let before = last_active_values(&trig);
    assert_eq!(before.len(), 1, "fixture must load, got {before:?}");

    // A DIFFERENT file whose path has the record's doc as a prefix.
    let sibling = format!("{}.bak", doc.to_str().unwrap());
    std::fs::write(&sibling, "backup\n").unwrap();

    let config = BaseConfig::default();
    let result = post_tool_use::handle(&config, tmp.path(), &read_event(&sibling));
    assert!(result.is_ok(), "post-tool-use should succeed: {result:?}");

    let after = last_active_values(&trig);
    assert_eq!(after.len(), 1, "still exactly one record, got {after:?}");
    assert!(
        after[0].contains("2026-01-01"),
        "touching <doc>.md.bak must NOT move <doc>.md's clock, got {after:?}"
    );
}
