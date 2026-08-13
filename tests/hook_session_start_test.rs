use std::path::Path;

use base::config::{BaseConfig, NamespaceConfig};
use base::hook::session_start;

/// Helper: create a workspace graph (NQuads) with test data.
fn write_test_trig(dir: &Path) {
    let base_dir = dir.join(".base");
    std::fs::create_dir_all(&base_dir).unwrap();
    std::fs::write(
        base_dir.join("graph.nq"),
        r#"<http://ops-sys.local/ontology#project/alpha> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ops-sys.local/ontology#Project> <http://ops-sys.local/ontology#graph/ws/test> .
<http://ops-sys.local/ontology#project/alpha> <http://ops-sys.local/ontology#name> "Alpha" <http://ops-sys.local/ontology#graph/ws/test> .
<http://ops-sys.local/ontology#project/alpha> <http://ops-sys.local/ontology#status> "active" <http://ops-sys.local/ontology#graph/ws/test> .
<http://ops-sys.local/ontology#project/alpha> <http://ops-sys.local/ontology#nextAction> "Ship v1" <http://ops-sys.local/ontology#graph/ws/test> .
<http://ops-sys.local/ontology#project/beta> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ops-sys.local/ontology#Project> <http://ops-sys.local/ontology#graph/ws/test> .
<http://ops-sys.local/ontology#project/beta> <http://ops-sys.local/ontology#name> "Beta" <http://ops-sys.local/ontology#graph/ws/test> .
<http://ops-sys.local/ontology#project/beta> <http://ops-sys.local/ontology#status> "active" <http://ops-sys.local/ontology#graph/ws/test> .
<http://ops-sys.local/ontology#project/gamma> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ops-sys.local/ontology#Project> <http://ops-sys.local/ontology#graph/ws/test> .
<http://ops-sys.local/ontology#project/gamma> <http://ops-sys.local/ontology#name> "Gamma" <http://ops-sys.local/ontology#graph/ws/test> .
<http://ops-sys.local/ontology#project/gamma> <http://ops-sys.local/ontology#status> "blocked" <http://ops-sys.local/ontology#graph/ws/test> .
<http://ops-sys.local/ontology#project/gamma> <http://ops-sys.local/ontology#blockedBy> "Waiting on API keys" <http://ops-sys.local/ontology#graph/ws/test> .
"#,
    )
    .unwrap();
}

/// Helper: create a custom queries.toml in the workspace.
fn write_test_queries(dir: &Path) {
    let base_dir = dir.join(".base");
    std::fs::create_dir_all(&base_dir).unwrap();
    std::fs::write(
        base_dir.join("queries.toml"),
        r#"
[[query]]
name = "test-active"
description = "Test active projects"
order = 1
format = "table"
sparql = """
SELECT ?name ?next WHERE {
  GRAPH ?g {
    ?p a {{prefix}}:Project ;
       {{prefix}}:name ?name ;
       {{prefix}}:status "active" .
    OPTIONAL { ?p {{prefix}}:nextAction ?next }
  }
}
"""
"#,
    )
    .unwrap();
}

#[test]
fn session_start_emits_active_projects() {
    let tmp = tempfile::tempdir().unwrap();
    write_test_trig(tmp.path());
    write_test_queries(tmp.path());

    let config = BaseConfig::default();

    // Capture stdout by calling handle (it prints to stdout)
    // We verify no error; full stdout capture tested via CLI integration
    let result = session_start::handle(&config, tmp.path(), None);
    assert!(result.is_ok(), "session-start should succeed: {result:?}");
}

#[test]
fn session_start_silent_when_no_trig() {
    let tmp = tempfile::tempdir().unwrap();
    // No .base/ directory at all
    let config = BaseConfig::default();

    let result = session_start::handle(&config, tmp.path(), None);
    assert!(
        result.is_ok(),
        "session-start with no TriG should succeed silently"
    );
}

#[test]
fn session_start_failopen_on_malformed_trig() {
    let tmp = tempfile::tempdir().unwrap();
    let base_dir = tmp.path().join(".base");
    std::fs::create_dir_all(&base_dir).unwrap();
    std::fs::write(base_dir.join("graph.nq"), "THIS IS NOT VALID TRIG {{{{").unwrap();

    let config = BaseConfig::default();

    // Should return an error, but the dispatch wrapper catches it (fail-open)
    // At the handler level, an error is expected here
    let result = session_start::handle(&config, tmp.path(), None);
    assert!(result.is_err(), "Malformed TriG should error at handler level");
}

#[test]
fn session_start_with_custom_namespace() {
    let tmp = tempfile::tempdir().unwrap();

    // Write graph (NQuads) with custom namespace
    let base_dir = tmp.path().join(".base");
    std::fs::create_dir_all(&base_dir).unwrap();
    std::fs::write(
        base_dir.join("graph.nq"),
        r#"<http://example.com/ns#project/delta> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.com/ns#Project> <http://example.com/ns#graph/ws/test> .
<http://example.com/ns#project/delta> <http://example.com/ns#name> "Delta" <http://example.com/ns#graph/ws/test> .
<http://example.com/ns#project/delta> <http://example.com/ns#status> "active" <http://example.com/ns#graph/ws/test> .
"#,
    )
    .unwrap();

    // Write matching queries.toml
    std::fs::write(
        base_dir.join("queries.toml"),
        r#"
[[query]]
name = "custom-ns-test"
description = "Custom namespace test"
order = 1
format = "list"
sparql = """
SELECT ?name WHERE {
  GRAPH ?g {
    ?p a {{prefix}}:Project ;
       {{prefix}}:name ?name ;
       {{prefix}}:status "active" .
  }
}
"""
"#,
    )
    .unwrap();

    let config = BaseConfig {
        namespace: NamespaceConfig {
            prefix: "myns".into(),
            uri: "http://example.com/ns#".into(),
        },
        ..BaseConfig::default()
    };

    let result = session_start::handle(&config, tmp.path(), None);
    assert!(
        result.is_ok(),
        "session-start with custom namespace should succeed: {result:?}"
    );
}
