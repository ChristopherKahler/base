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
    let mut out = session_start::SessionOutput::new();
    let result = session_start::handle(&config, tmp.path(), None, &mut out);
    assert!(result.is_ok(), "session-start should succeed: {result:?}");
}

#[test]
fn session_start_silent_when_no_trig() {
    let tmp = tempfile::tempdir().unwrap();
    // No .base/ directory at all
    let config = BaseConfig::default();

    let mut out = session_start::SessionOutput::new();
    let result = session_start::handle(&config, tmp.path(), None, &mut out);
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

    let mut out = session_start::SessionOutput::new();
    let result = session_start::handle(&config, tmp.path(), None, &mut out);

    // WHAT THIS TEST USED TO ASSERT, AND WHY IT DOES NOT ANY MORE.
    //
    // It asserted `result.is_err()`, with the comment "the dispatch wrapper catches
    // it (fail-open)". That error never came from the signals. It came from
    // `store::load_graphs(&paths)?` in the queries.toml FALLBACK, which sits below
    // the `if any_signal { ... return Ok(()); }` early return. A malformed graph
    // used to leave every signal silent, so the fallback ran and errored.
    //
    // The honesty envelope makes the working-set block always render at least a
    // scope line, so `any_signal` is now always true and the handler returns before
    // reaching that strict load. THE FALLBACK IS NOW UNREACHABLE -- filed as its own
    // item, QTF-1, for the operator to rule on, NOT fixed or deleted by this lane.
    // See findings/2026-09-21-plover-queries-toml-fallback-unreachable.md.
    //
    // THE ASSERTION IS REPLACED, NOT WEAKENED. The contract worth guarding was never
    // "an internal Err the wrapper discards" -- nothing reached the operator from it.
    // It is "a malformed graph does not pass silently", and that is now checked where
    // the operator can actually see it: in the text this session start renders.
    assert!(
        result.is_ok(),
        "the handler no longer reaches the fallback's strict load; see QTF-1: {result:?}"
    );

    // Unchanged from the original: the unhealthy-graph warning must still be collected.
    assert!(
        out.fragments().parts().iter().any(|p| p.kind == "graph-unhealthy"),
        "the unhealthy-graph warning is gone: {:?}",
        out.fragments().parts().iter().map(|p| p.kind.as_str()).collect::<Vec<_>>()
    );

    // NEW AND STRICTLY STRONGER than the Err it replaces: the damaged read has to be
    // reported in rendered text, not merely signalled to a caller that drops it.
    //
    // It has to be read off `finish()`, NOT off `fragments().parts()`.
    // `push_signals` parks the signals in their own field and only merges them in
    // `finish`, so the fragments alone never contain the working-set block. A first
    // draft of this assertion read the fragments, found no block, and would have
    // been taken as proof the envelope does not fire -- when it was the READER that
    // was wrong. Assert on what is actually printed.
    let rendered = out.finish(&config, tmp.path()).text;
    assert!(
        rendered.contains("THIS READ WAS INCOMPLETE"),
        "a malformed graph must be reported in what the operator sees, not only to a \
         caller that discards it; rendered:\n{rendered}"
    );
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

    let mut out = session_start::SessionOutput::new();
    let result = session_start::handle(&config, tmp.path(), None, &mut out);
    assert!(
        result.is_ok(),
        "session-start with custom namespace should succeed: {result:?}"
    );
}


/// QTF-1, option 2. An operator's `queries.toml` must render even when signals speak.
///
/// Before this, the ad-hoc query step sat below `if any_signal { ... return Ok(()); }`,
/// so it ran only when EVERY signal was silent. On a workspace with real data a signal
/// always speaks, so the block never rendered: the file parsed, the config loaded it,
/// `LAYOUT` reserved it a place, and the operator got nothing and was told nothing.
///
/// The workspace below holds a project, so active-awareness WILL speak. That is the
/// whole point of the arm -- a fixture with no signals would pass before the fix and
/// prove nothing.
#[test]
fn an_operator_queries_toml_renders_beside_the_signals() {
    let tmp = tempfile::tempdir().unwrap();
    let base_dir = tmp.path().join(".base");
    std::fs::create_dir_all(&base_dir).unwrap();

    let ns = base::config::NamespaceConfig::default();
    base::crud::project::add(tmp.path(), &ns, "Qtf Project", "active", None).unwrap();

    std::fs::write(
        base_dir.join("queries.toml"),
        "[[query]]\nname = \"qtf_probe\"\ndescription = \"QTF PROBE BLOCK\"\nformat = \"list\"\nsparql = \"\"\"\nSELECT ?name WHERE { GRAPH ?g { ?e {{prefix}}:name ?name } } LIMIT 5\n\"\"\"\n",
    )
    .unwrap();

    let config = BaseConfig::default();
    let mut out = session_start::SessionOutput::new();
    let result = session_start::handle(&config, tmp.path(), None, &mut out);
    assert!(result.is_ok(), "session start must not fail: {result:?}");

    let rendered = out.finish(&config, tmp.path()).text;

    // The signal speaks -- if it did not, this arm would pass before the fix too.
    assert!(
        rendered.contains("Qtf Project"),
        "the fixture must produce a signal, or this arm proves nothing; got:\n{rendered}"
    );
    // ...and the operator's own query renders BESIDE it.
    assert!(
        rendered.contains("QTF PROBE BLOCK"),
        "a queries.toml must render even when signals speak (QTF-1); got:\n{rendered}"
    );
}
