use oxigraph::sparql::QueryResults;

use base::config::{BaseConfig, NamespaceConfig};
use base::crud;
use base::scope::ProjectScope;

fn default_ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

#[test]
fn add_project_creates_triples() {
    let tmp = tempfile::tempdir().unwrap();
    let ns = default_ns();

    let slug = crud::project::add(tmp.path(), &ns, "Test Project", "active", None).unwrap();
    assert_eq!(slug, "test-project");

    // Reload and verify
    let trig_path = tmp.path().join(".base").join("graph.nq");
    assert!(trig_path.exists(), "graph.nq should be created");

    let store = base::store::load_graph(&trig_path).unwrap();
    let sparql = format!(
        "PREFIX {p}: <{u}>\n\
         ASK {{ GRAPH ?g {{ <{u}project/test-project> a {p}:Project ; {p}:name \"Test Project\" ; {p}:status \"active\" }} }}",
        p = ns.prefix, u = ns.uri,
    );
    match store.query(&sparql).unwrap() {
        QueryResults::Boolean(yes) => assert!(yes, "Project triples should exist"),
        _ => panic!("Expected boolean"),
    }
}

#[test]
fn add_project_with_custom_path() {
    let tmp = tempfile::tempdir().unwrap();
    let ns = default_ns();

    crud::project::add(tmp.path(), &ns, "PathTest", "active", Some("/custom/path")).unwrap();

    let trig_path = tmp.path().join(".base").join("graph.nq");
    let store = base::store::load_graph(&trig_path).unwrap();
    let sparql = format!(
        "PREFIX {p}: <{u}>\n\
         ASK {{ GRAPH ?g {{ ?proj {p}:path \"/custom/path\" }} }}",
        p = ns.prefix, u = ns.uri,
    );
    match store.query(&sparql).unwrap() {
        QueryResults::Boolean(yes) => assert!(yes, "Custom path should be set"),
        _ => panic!("Expected boolean"),
    }
}

#[test]
fn list_projects_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let ns = default_ns();

    crud::project::add(tmp.path(), &ns, "Alpha", "active", None).unwrap();
    crud::project::add(tmp.path(), &ns, "Beta", "blocked", None).unwrap();

    // Should not error
    let config = BaseConfig { namespace: ns.clone(), ..Default::default() };
    let result = crud::project::list(tmp.path(), &config, ProjectScope::All);
    assert!(result.is_ok());
}

#[test]
fn get_project_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let ns = default_ns();

    crud::project::add(tmp.path(), &ns, "GetMe", "active", None).unwrap();

    let result = crud::project::get(tmp.path(), &ns, "getme");
    assert!(result.is_ok());
}

#[test]
fn update_project_changes_status() {
    let tmp = tempfile::tempdir().unwrap();
    let ns = default_ns();

    crud::project::add(tmp.path(), &ns, "Updatable", "active", None).unwrap();
    crud::project::update(tmp.path(), &ns, "updatable", Some("blocked"), Some("waiting on API"), None).unwrap();

    // Verify new status
    let trig_path = tmp.path().join(".base").join("graph.nq");
    let store = base::store::load_graph(&trig_path).unwrap();

    let sparql = format!(
        "PREFIX {p}: <{u}>\n\
         ASK {{ GRAPH ?g {{ <{u}project/updatable> {p}:status \"blocked\" }} }}",
        p = ns.prefix, u = ns.uri,
    );
    match store.query(&sparql).unwrap() {
        QueryResults::Boolean(yes) => assert!(yes, "Status should be blocked"),
        _ => panic!("Expected boolean"),
    }

    // Verify old status is gone
    let sparql_old = format!(
        "PREFIX {p}: <{u}>\n\
         ASK {{ GRAPH ?g {{ <{u}project/updatable> {p}:status \"active\" }} }}",
        p = ns.prefix, u = ns.uri,
    );
    match store.query(&sparql_old).unwrap() {
        QueryResults::Boolean(yes) => assert!(!yes, "Old status should be removed"),
        _ => panic!("Expected boolean"),
    }

    // Verify updatedAt is set
    let sparql_ts = format!(
        "PREFIX {p}: <{u}>\n\
         ASK {{ GRAPH ?g {{ <{u}project/updatable> {p}:updatedAt ?ts }} }}",
        p = ns.prefix, u = ns.uri,
    );
    match store.query(&sparql_ts).unwrap() {
        QueryResults::Boolean(yes) => assert!(yes, "updatedAt should be set"),
        _ => panic!("Expected boolean"),
    }
}
