use oxigraph::sparql::QueryResults;

use base::config::NamespaceConfig;
use base::crud;

fn ns() -> NamespaceConfig { NamespaceConfig::default() }

#[test]
fn entity_add_person() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    let slug = crud::entity::add(tmp.path(), &ns(), "Charlie", "person", "GLOBAL", None).unwrap();
    assert_eq!(slug, "charlie");

    let trig = tmp.path().join(".base").join("graph.nq");
    let store = base::store::load_graph(&trig).unwrap();
    let sparql = format!(
        "PREFIX {p}: <{u}>\nASK {{ GRAPH ?g {{ <{u}entity/charlie> a {p}:Person ; {p}:name \"Charlie\" }} }}",
        p = ns().prefix, u = ns().uri,
    );
    match store.query(&sparql).unwrap() {
        QueryResults::Boolean(yes) => assert!(yes, "Person should exist"),
        _ => panic!("Expected boolean"),
    }
}

#[test]
fn goal_add_and_list() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    crud::goal::add(tmp.path(), &ns(), "Revenue", "$7k/mo").unwrap();

    let trig = tmp.path().join(".base").join("graph.nq");
    let store = base::store::load_graph(&trig).unwrap();
    let sparql = format!(
        "PREFIX {p}: <{u}>\nASK {{ GRAPH ?g {{ ?g2 a {p}:Goal ; {p}:name \"Revenue\" ; {p}:description \"$7k/mo\" }} }}",
        p = ns().prefix, u = ns().uri,
    );
    match store.query(&sparql).unwrap() {
        QueryResults::Boolean(yes) => assert!(yes, "Goal should exist"),
        _ => panic!("Expected boolean"),
    }

    assert!(crud::goal::list(tmp.path(), &ns()).is_ok());
}

#[test]
fn reminder_add_and_remove() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    crud::reminder::add(tmp.path(), &ns(), "Check deploy", "2026-06-15", None).unwrap();

    // Verify exists
    let trig = tmp.path().join(".base").join("graph.nq");
    let store = base::store::load_graph(&trig).unwrap();
    let sparql = format!(
        "PREFIX {p}: <{u}>\nASK {{ GRAPH ?g {{ ?r a {p}:Reminder ; {p}:name \"Check deploy\" }} }}",
        p = ns().prefix, u = ns().uri,
    );
    match store.query(&sparql).unwrap() {
        QueryResults::Boolean(yes) => assert!(yes, "Reminder should exist"),
        _ => panic!("Expected boolean"),
    }

    // Remove
    crud::reminder::remove(tmp.path(), &ns(), "check-deploy").unwrap();

    // Verify gone
    let store2 = base::store::load_graph(&trig).unwrap();
    match store2.query(&sparql).unwrap() {
        QueryResults::Boolean(yes) => assert!(!yes, "Reminder should be removed"),
        _ => panic!("Expected boolean"),
    }
}

#[test]
fn entity_update() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    crud::entity::add(tmp.path(), &ns(), "Charlie", "person", "GLOBAL", None).unwrap();
    crud::entity::update(tmp.path(), &ns(), "charlie", Some("inactive"), None).unwrap();

    let trig = tmp.path().join(".base").join("graph.nq");
    let store = base::store::load_graph(&trig).unwrap();
    let sparql = format!(
        "PREFIX {p}: <{u}>\nASK {{ GRAPH ?g {{ <{u}entity/charlie> {p}:status \"inactive\" }} }}",
        p = ns().prefix, u = ns().uri,
    );
    match store.query(&sparql).unwrap() {
        QueryResults::Boolean(yes) => assert!(yes, "Status should be inactive"),
        _ => panic!("Expected boolean"),
    }
}
