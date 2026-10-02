use oxigraph::sparql::QueryResults;

use base::config::{BaseConfig, NamespaceConfig};
use base::extract;

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

fn default_config() -> BaseConfig {
    BaseConfig::default()
}

/// Helper: create a markdown file with frontmatter in a temp workspace.
fn write_md(dir: &std::path::Path, rel_path: &str, frontmatter: &str, body: &str) {
    let full = dir.join(rel_path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&full, format!("---\n{frontmatter}\n---\n\n{body}")).unwrap();
}

/// Helper: create a paul.json file.
fn write_paul_json(dir: &std::path::Path, rel_path: &str, content: &str) {
    let full = dir.join(rel_path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&full, content).unwrap();
}

#[test]
fn sync_extracts_markdown_frontmatter() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    write_md(
        tmp.path(),
        "docs/readme.md",
        "title: My Readme\nstatus: active",
        "# Hello",
    );

    let config = default_config();
    let report = extract::sync(tmp.path(), &config, false).unwrap();
    assert!(report.extracted >= 1, "Should extract at least 1 file");

    // Verify in graph
    let trig = tmp.path().join(".base").join("graph.nq");
    let store = base::store::load_graph(&trig).unwrap();
    let p = ns().prefix;
    let u = ns().uri;
    let sparql = format!(
        "PREFIX {p}: <{u}>\nASK {{ GRAPH ?g {{ ?doc a {p}:Document ; {p}:name \"My Readme\" }} }}"
    );
    match store.query(&sparql).unwrap() {
        QueryResults::Boolean(yes) => assert!(yes, "Document should exist with name"),
        _ => panic!("Expected boolean"),
    }
}

#[test]
fn sync_extracts_paul_json() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    write_paul_json(
        tmp.path(),
        "apps/myapp/.paul/paul.json",
        r#"{"name": "myapp", "version": "1.0", "phase": {"name": "Build", "status": "active"}}"#,
    );

    let mut config = default_config();
    config.sync.include.push("**/.paul/paul.json".into());
    let report = extract::sync(tmp.path(), &config, false).unwrap();
    assert!(report.extracted >= 1);

    let trig = tmp.path().join(".base").join("graph.nq");
    let store = base::store::load_graph(&trig).unwrap();
    let p = ns().prefix;
    let u = ns().uri;
    let sparql = format!(
        "PREFIX {p}: <{u}>\nASK {{ GRAPH ?g {{ ?proj a {p}:Project ; {p}:name \"myapp\" }} }}"
    );
    match store.query(&sparql).unwrap() {
        QueryResults::Boolean(yes) => assert!(yes, "Project from paul.json should exist"),
        _ => panic!("Expected boolean"),
    }
}

/// BO-09 (D13): a re-sync of a paul.json project rewrites what the file says and keeps the parent link and
/// `nested` the operator set, which nothing re-derives.
#[test]
fn paul_json_resync_keeps_parent_and_nested() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    let json = |v: &str| format!(r#"{{"name": "myapp", "version": "{v}", "phase": {{"name": "Build", "status": "active"}}}}"#);
    write_paul_json(tmp.path(), "apps/myapp/.paul/paul.json", &json("1.0"));
    let mut config = default_config();
    config.sync.include.push("**/.paul/paul.json".into());
    extract::sync(tmp.path(), &config, false).unwrap();

    base::crud::project::add(tmp.path(), &ns(), "Holder", "active", Some("/nope")).unwrap();
    let change = base::crud::project::ProjectUpdate {
        parent: Some(base::crud::project::ParentChange::Set("holder".into())),
        nested: Some(true),
        ..Default::default()
    };
    base::crud::project::apply_update(tmp.path(), &ns(), "myapp", &change).unwrap();

    write_paul_json(tmp.path(), "apps/myapp/.paul/paul.json", &json("1.1"));
    extract::sync(tmp.path(), &config, false).unwrap();
    let rec = base::crud::project::get_data(tmp.path(), &ns(), "myapp").unwrap().expect("myapp");
    assert_eq!(rec.parent.as_deref(), Some("holder"), "parent kept: {rec:?}");
    assert!(rec.nested, "nested kept: {rec:?}");
    let trig = tmp.path().join(".base").join("graph.nq");
    let text = std::fs::read_to_string(trig).unwrap();
    assert!(text.contains("\"version: 1.1\""), "the re-sync did rewrite the record");
    assert!(!text.contains("\"version: 1.0\""), "and dropped what the file no longer says");
}

#[test]
fn sync_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    write_md(tmp.path(), "test.md", "title: Test\nstatus: done", "Body");

    let config = default_config();

    // First sync
    extract::sync(tmp.path(), &config, false).unwrap();
    let trig = tmp.path().join(".base").join("graph.nq");
    let store1 = base::store::load_graph(&trig).unwrap();
    let count1 = store1.len().unwrap();

    // Second sync (no changes)
    extract::sync(tmp.path(), &config, false).unwrap();
    let store2 = base::store::load_graph(&trig).unwrap();
    let count2 = store2.len().unwrap();

    assert_eq!(count1, count2, "Triple count should be identical after re-sync");
}

#[test]
fn sync_incremental_skips_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    write_md(tmp.path(), "stable.md", "title: Stable", "Content");

    let config = default_config();

    // Full sync first
    let r1 = extract::sync(tmp.path(), &config, false).unwrap();
    assert_eq!(r1.extracted, 1);

    // Incremental — file unchanged
    let r2 = extract::sync(tmp.path(), &config, true).unwrap();
    assert_eq!(r2.skipped, 1, "Unchanged file should be skipped");
    assert_eq!(r2.extracted, 0);
}

#[test]
fn sync_skips_file_without_frontmatter() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    let plain = tmp.path().join("plain.md");
    std::fs::write(&plain, "# No frontmatter\n\nJust text").unwrap();

    let config = default_config();
    let report = extract::sync(tmp.path(), &config, false).unwrap();
    assert_eq!(report.extracted, 0, "File without frontmatter should not be extracted");
    assert_eq!(report.skipped, 1);
}

#[test]
fn sync_respects_exclude_patterns() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    write_md(tmp.path(), "good.md", "title: Good", "Yes");
    write_md(
        tmp.path(),
        "node_modules/dep/readme.md",
        "title: Dep",
        "No",
    );

    let config = default_config();
    let report = extract::sync(tmp.path(), &config, false).unwrap();
    assert_eq!(report.extracted, 1, "Only non-excluded file should be extracted");
}
