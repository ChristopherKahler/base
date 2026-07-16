use oxigraph::sparql::QueryResults;

use base::config::NamespaceConfig;
use base::crud;

fn default_ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

#[test]
fn add_task_linked_to_project() {
    let tmp = tempfile::tempdir().unwrap();
    let ns = default_ns();

    // Create project first
    crud::project::add(tmp.path(), &ns, "MyProject", "active", None).unwrap();

    // Add task
    let slug = crud::task::add(tmp.path(), &ns, "myproject", "Fix Auth", Some("high"), None).unwrap();
    assert_eq!(slug, "myproject.fix-auth");

    // Verify task exists and is linked to project
    let trig_path = tmp.path().join(".base").join("graph.nq");
    let store = base::store::load_graph(&trig_path).unwrap();

    let sparql = format!(
        "PREFIX {p}: <{u}>\n\
         ASK {{ GRAPH ?g {{\n\
           <{u}task/myproject.fix-auth> a {p}:Task ;\n\
             {p}:name \"Fix Auth\" ;\n\
             {p}:status \"active\" ;\n\
             {p}:priority \"high\" .\n\
           <{u}project/myproject> {p}:hasTask <{u}task/myproject.fix-auth> .\n\
         }} }}",
        p = ns.prefix,
        u = ns.uri,
    );
    match store.query(&sparql).unwrap() {
        QueryResults::Boolean(yes) => assert!(yes, "Task should exist and be linked to project"),
        _ => panic!("Expected boolean"),
    }
}

#[test]
fn list_tasks_by_project() {
    let tmp = tempfile::tempdir().unwrap();
    let ns = default_ns();

    crud::project::add(tmp.path(), &ns, "Proj", "active", None).unwrap();
    crud::task::add(tmp.path(), &ns, "proj", "Task A", None, None).unwrap();
    crud::task::add(tmp.path(), &ns, "proj", "Task B", None, None).unwrap();

    let result = crud::task::list(tmp.path(), &ns, Some("proj"), None);
    assert!(result.is_ok());
}

#[test]
fn mark_task_done() {
    let tmp = tempfile::tempdir().unwrap();
    let ns = default_ns();

    crud::project::add(tmp.path(), &ns, "Proj", "active", None).unwrap();
    crud::task::add(tmp.path(), &ns, "proj", "DoThis", None, None).unwrap();
    crud::task::done(tmp.path(), &ns, "proj.dothis").unwrap();

    // Verify status is completed
    let trig_path = tmp.path().join(".base").join("graph.nq");
    let store = base::store::load_graph(&trig_path).unwrap();

    let sparql = format!(
        "PREFIX {p}: <{u}>\n\
         ASK {{ GRAPH ?g {{ <{u}task/proj.dothis> {p}:status \"completed\" }} }}",
        p = ns.prefix,
        u = ns.uri,
    );
    match store.query(&sparql).unwrap() {
        QueryResults::Boolean(yes) => assert!(yes, "Task should be completed"),
        _ => panic!("Expected boolean"),
    }

    // Verify updatedAt set
    let sparql_ts = format!(
        "PREFIX {p}: <{u}>\n\
         ASK {{ GRAPH ?g {{ <{u}task/proj.dothis> {p}:updatedAt ?ts }} }}",
        p = ns.prefix,
        u = ns.uri,
    );
    match store.query(&sparql_ts).unwrap() {
        QueryResults::Boolean(yes) => assert!(yes, "updatedAt should be set"),
        _ => panic!("Expected boolean"),
    }
}

#[test]
fn add_task_default_priority() {
    let tmp = tempfile::tempdir().unwrap();
    let ns = default_ns();

    crud::project::add(tmp.path(), &ns, "Proj", "active", None).unwrap();
    crud::task::add(tmp.path(), &ns, "proj", "NoPri", None, None).unwrap();

    let trig_path = tmp.path().join(".base").join("graph.nq");
    let store = base::store::load_graph(&trig_path).unwrap();

    let sparql = format!(
        "PREFIX {p}: <{u}>\n\
         ASK {{ GRAPH ?g {{ <{u}task/proj.nopri> {p}:priority \"medium\" }} }}",
        p = ns.prefix,
        u = ns.uri,
    );
    match store.query(&sparql).unwrap() {
        QueryResults::Boolean(yes) => assert!(yes, "Default priority should be medium"),
        _ => panic!("Expected boolean"),
    }
}

#[test]
fn get_update_delete_task_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let ns = default_ns();

    crud::project::add(tmp.path(), &ns, "Proj", "active", None).unwrap();
    crud::project::add(tmp.path(), &ns, "Other", "active", None).unwrap();
    crud::milestone::add(tmp.path(), &ns, "proj", "M1", None).unwrap();
    let slug = crud::task::add(tmp.path(), &ns, "proj", "Round Trip", Some("low"), None).unwrap();
    assert_eq!(slug, "proj.round-trip");

    // get: all fields present, project edge resolved
    let rec = crud::task::get_data(tmp.path(), &ns, &slug).unwrap().expect("task exists");
    assert_eq!(rec.name, "Round Trip");
    assert_eq!(rec.status, "active");
    assert_eq!(rec.priority.as_deref(), Some("low"));
    assert_eq!(rec.project.as_deref(), Some("proj"));
    assert!(rec.milestone.is_none());

    // update: scalar fields + reassign project + attach milestone
    crud::task::update(
        tmp.path(), &ns, &slug,
        Some("Renamed"), Some("completed"), Some("high"),
        Some("a note"), Some("chris"), Some("2026-08-01"),
        Some("other"), Some("proj.m1"),
    ).unwrap();
    let rec2 = crud::task::get_data(tmp.path(), &ns, &slug).unwrap().unwrap();
    assert_eq!(rec2.name, "Renamed");
    assert_eq!(rec2.status, "completed");
    assert_eq!(rec2.priority.as_deref(), Some("high"));
    assert_eq!(rec2.description.as_deref(), Some("a note"));
    assert_eq!(rec2.assignee.as_deref(), Some("chris"));
    assert_eq!(rec2.due.as_deref(), Some("2026-08-01"));
    assert_eq!(rec2.project.as_deref(), Some("other"), "reassigned to other project");
    assert_eq!(rec2.milestone.as_deref(), Some("proj.m1"), "attached to milestone");

    // delete: node + reverse edges gone
    crud::task::delete(tmp.path(), &ns, &slug).unwrap();
    assert!(crud::task::get_data(tmp.path(), &ns, &slug).unwrap().is_none(), "task removed");
    let trig = tmp.path().join(".base").join("graph.nq");
    let store = base::store::load_graph(&trig).unwrap();
    let sparql = format!(
        "PREFIX {p}: <{u}>\nASK {{ GRAPH ?g {{ ?s {p}:hasTask <{u}task/proj.round-trip> }} }}",
        p = ns.prefix, u = ns.uri,
    );
    match store.query(&sparql).unwrap() {
        QueryResults::Boolean(yes) => assert!(!yes, "no dangling hasTask edge should remain"),
        _ => panic!("Expected boolean"),
    }
}

#[test]
fn task_list_json_shape() {
    let tmp = tempfile::tempdir().unwrap();
    let ns = default_ns();

    crud::project::add(tmp.path(), &ns, "Proj", "active", None).unwrap();
    crud::task::add(tmp.path(), &ns, "proj", "Alpha", Some("high"), None).unwrap();

    let rows = crud::task::list_data(tmp.path(), &ns, Some("proj"), None).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "proj.alpha");
    let json = serde_json::to_string(&rows).unwrap();
    for key in [
        "\"id\"", "\"name\"", "\"status\"", "\"priority\"", "\"project\"",
        "\"milestone\"", "\"description\"", "\"assignee\"", "\"due\"",
        "\"created\"", "\"updated\"", "\"last_active\"",
    ] {
        assert!(json.contains(key), "json missing stable key {key}: {json}");
    }
}
