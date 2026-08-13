use oxigraph::sparql::QueryResults;

use base::config::NamespaceConfig;
use base::crud;

fn ns() -> NamespaceConfig { NamespaceConfig::default() }

#[test]
fn log_decision_creates_triples() {
    let tmp = tempfile::tempdir().unwrap();
    let slug = crud::decision::log(
        tmp.path(), &ns(), "dev", "Use JWT", "Stateless auth", Some("auth, tokens"),
    ).unwrap();
    assert_eq!(slug, "dev.use-jwt");

    let trig_path = tmp.path().join(".base").join("graph.nq");
    let store = base::store::load_graph(&trig_path).unwrap();
    let sparql = format!(
        "PREFIX {p}: <{u}>\nASK {{ GRAPH ?g {{ ?d a {p}:Decision ; {p}:name \"Use JWT\" ; {p}:rationale \"Stateless auth\" }} }}",
        p = ns().prefix, u = ns().uri,
    );
    match store.query(&sparql).unwrap() {
        QueryResults::Boolean(yes) => assert!(yes, "Decision should exist"),
        _ => panic!("Expected boolean"),
    }
}

#[test]
fn search_finds_matching_decision() {
    let tmp = tempfile::tempdir().unwrap();
    crud::decision::log(tmp.path(), &ns(), "auth", "Use JWT tokens", "Fast auth", None).unwrap();
    crud::decision::log(tmp.path(), &ns(), "db", "Use Postgres", "Reliable", None).unwrap();

    // Search should not error — results go to stdout
    let result = crud::decision::search(tmp.path(), &ns(), "JWT");
    assert!(result.is_ok());
}

#[test]
fn update_decision_changes_fields() {
    let tmp = tempfile::tempdir().unwrap();
    // Decisions carry a stable {domain}.{decision} slug — update addresses it directly.
    let slug = crud::decision::log(tmp.path(), &ns(), "arch", "Use oxigraph", "Embedded RDF", None).unwrap();
    assert_eq!(slug, "arch.use-oxigraph");

    crud::decision::update(
        tmp.path(), &ns(), &slug,
        None, Some("Embedded RDF store, no server"), Some("rdf, graph"), Some("superseded"),
    ).unwrap();

    let hits = crud::decision::search_data(tmp.path(), &ns(), "oxigraph").unwrap();
    let d = hits.iter().find(|d| d.id == slug).expect("decision found");
    assert_eq!(d.rationale.as_deref(), Some("Embedded RDF store, no server"));
    assert_eq!(d.recall.as_deref(), Some("rdf, graph"));
    assert_eq!(d.status.as_deref(), Some("superseded"));
    assert_eq!(d.domain.as_deref(), Some("arch"));
}

#[test]
fn decision_search_json_shape() {
    let tmp = tempfile::tempdir().unwrap();
    crud::decision::log(tmp.path(), &ns(), "db", "Use Postgres", "Reliable", Some("db, sql")).unwrap();

    let rows = crud::decision::search_data(tmp.path(), &ns(), "postgres").unwrap();
    assert_eq!(rows.len(), 1);
    let json = serde_json::to_string(&rows).unwrap();
    for key in [
        "\"id\"", "\"name\"", "\"rationale\"", "\"recall\"", "\"status\"",
        "\"domain\"", "\"created\"", "\"last_active\"",
    ] {
        assert!(json.contains(key), "json missing stable key {key}");
    }
}
