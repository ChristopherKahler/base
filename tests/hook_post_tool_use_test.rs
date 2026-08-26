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
