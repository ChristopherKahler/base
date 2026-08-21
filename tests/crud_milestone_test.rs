use base::config::NamespaceConfig;
use base::crud;

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

#[test]
fn delete_milestone_detaches_tasks_by_default() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    let n = ns();

    crud::project::add(tmp.path(), &n, "Proj", "active", None).unwrap();
    crud::milestone::add(tmp.path(), &n, "proj", "Sprint 1", None).unwrap();
    // Task grouped under the milestone (also gets its project edge at add time).
    crud::task::add(tmp.path(), &n, "proj", "Task A", None, Some("proj.sprint-1")).unwrap();

    // Default delete: milestone gone, task survives detached to project level.
    let removed = crud::milestone::delete(tmp.path(), &n, "proj.sprint-1", false).unwrap();
    assert_eq!(removed, 0, "default detaches, deletes no tasks");
    assert!(crud::milestone::get_data(tmp.path(), &n, "proj.sprint-1").unwrap().is_none(), "milestone gone");

    let task = crud::task::get_data(tmp.path(), &n, "proj.task-a").unwrap().expect("task survives");
    assert_eq!(task.project.as_deref(), Some("proj"), "still homed at the project");
    assert!(task.milestone.is_none(), "milestone edge removed");
}

#[test]
fn delete_milestone_force_cascades_tasks() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    let n = ns();

    crud::project::add(tmp.path(), &n, "Proj", "active", None).unwrap();
    crud::milestone::add(tmp.path(), &n, "proj", "Sprint 2", None).unwrap();
    crud::task::add(tmp.path(), &n, "proj", "Task B", None, Some("proj.sprint-2")).unwrap();

    let removed = crud::milestone::delete(tmp.path(), &n, "proj.sprint-2", true).unwrap();
    assert_eq!(removed, 1, "force cascade-deletes the grouped task");
    assert!(crud::task::get_data(tmp.path(), &n, "proj.task-b").unwrap().is_none(), "task cascaded");
    assert!(crud::milestone::get_data(tmp.path(), &n, "proj.sprint-2").unwrap().is_none(), "milestone gone");
}

#[test]
fn milestone_list_get_json_shape() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    let n = ns();

    crud::project::add(tmp.path(), &n, "Proj", "active", None).unwrap();
    crud::milestone::add(tmp.path(), &n, "proj", "MJ", Some("a milestone")).unwrap();

    let rows = crud::milestone::list_data(tmp.path(), &n, Some("proj")).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "proj.mj");
    assert_eq!(rows[0].project.as_deref(), Some("proj"));
    let json = serde_json::to_string(&rows).unwrap();
    for key in [
        "\"id\"", "\"name\"", "\"status\"", "\"description\"", "\"project\"",
        "\"created\"", "\"updated\"", "\"last_active\"",
    ] {
        assert!(json.contains(key), "json missing stable key {key}");
    }

    let rec = crud::milestone::get_data(tmp.path(), &n, "proj.mj").unwrap().expect("milestone exists");
    assert_eq!(rec.description.as_deref(), Some("a milestone"));
}
