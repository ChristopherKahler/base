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

#[test]
fn sync_persists_good_files_when_one_file_has_invalid_triples() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    // Sorted discovery puts a good file on each side of the failure.
    write_md(tmp.path(), "a-good.md", "title: First good", "");
    write_md(tmp.path(), "b-bad.md", "title: Bad\nrelated: bad<target", "");
    write_md(tmp.path(), "c-good.md", "title: Second good", "");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_base"))
        .arg("sync")
        .current_dir(tmp.path())
        .env("BASE_HOME", home.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("sync status: {}\nstdout: {}\nstderr: {stderr}",
        output.status, String::from_utf8_lossy(&output.stdout));
    assert!(!output.status.success(), "partial sync must exit nonzero");
    assert!(stderr.contains("b-bad.md"), "missing failed file: {stderr}");
    assert!(stderr.contains("IRI"), "missing insertion error: {stderr}");

    // Reload from disk: checking the in-memory store would miss the lost batch.
    let store = base::store::load_graph(&tmp.path().join(".base/graph.nq")).unwrap();
    let p = ns().prefix;
    let u = ns().uri;
    for name in ["First good", "Second good"] {
        let sparql = format!(
            "PREFIX {p}: <{u}>\nASK {{ GRAPH ?g {{ ?doc a {p}:Document ; {p}:name \"{name}\" }} }}"
        );
        match store.query(&sparql).unwrap() {
            QueryResults::Boolean(yes) => assert!(yes, "{name} must persist despite bad file"),
            _ => panic!("Expected boolean"),
        }
    }
    let bad_iri = extract::file_iri_from_path(&ns(), "b-bad.md");
    match store.query(&format!("ASK {{ GRAPH ?g {{ <{bad_iri}> ?p ?o }} }}")).unwrap() {
        QueryResults::Boolean(yes) => assert!(!yes, "bad file must not be inserted"),
        _ => panic!("Expected boolean"),
    }
    assert!(stderr.contains("3 scanned, 2 extracted, 0 skipped, 1 failed"),
        "missing partial summary: {stderr}");
}

#[test]
fn sync_rejected_file_keeps_its_previous_triples() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    write_md(tmp.path(), "note.md", "title: Original", "");
    extract::sync(tmp.path(), &default_config(), false).unwrap();
    let graph = tmp.path().join(".base/graph.nq");
    let before = std::fs::read_to_string(&graph).unwrap();

    write_md(tmp.path(), "note.md", "title: Rejected\nrelated: bad<target", "");
    assert!(extract::sync(tmp.path(), &default_config(), false).is_err());
    let after = std::fs::read_to_string(&graph).unwrap();
    let mut before_lines: Vec<_> = before.lines().collect();
    let mut after_lines: Vec<_> = after.lines().collect();
    before_lines.sort();
    after_lines.sort();
    assert_eq!(after_lines, before_lines);
}

#[test]
fn sync_rejected_paul_json_keeps_its_previous_triples() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    let mut config = default_config();
    config.sync.include.push("**/.paul/paul.json".into());
    let path = "apps/myapp/.paul/paul.json";
    write_paul_json(tmp.path(), path, r#"{"name":"myapp","version":"original"}"#);
    assert_eq!(extract::sync(tmp.path(), &config, false).unwrap().extracted, 1);
    let graph = tmp.path().join(".base/graph.nq");
    // Include a legacy Document subject: both DELETEs must roll back.
    let locked = base::store::lock_and_load_graph(&graph).unwrap();
    let store = locked.store();
    let document = extract::file_iri_from_path(&ns(), path);
    let workspace = base::crud::workspace_graph_iri(&ns(), &base::crud::workspace_slug(tmp.path()));
    store.update(&format!(
        "INSERT DATA {{ GRAPH <{workspace}> {{ <{document}> <{}name> \"legacy\" }} }}", ns().uri
    )).unwrap();
    locked.write(base::changelog::Change::Op("test.seed-legacy")).unwrap();
    drop(locked);
    let before = std::fs::read_to_string(&graph).unwrap();
    assert!(before.contains("original"), "positive control: project must be on disk");
    assert!(before.contains("legacy"), "positive control: legacy document must be on disk");

    // Valid JSON whose decoded quote makes the emitted SPARQL invalid.
    write_paul_json(tmp.path(), path, r#"{"name":"myapp","version":"bad\"version"}"#);
    let result = extract::sync(tmp.path(), &config, false);
    let after = std::fs::read_to_string(&graph).unwrap();
    let mut before_lines: Vec<_> = before.lines().collect();
    let mut after_lines: Vec<_> = after.lines().collect();
    before_lines.sort();
    after_lines.sort();
    assert_eq!(after_lines, before_lines, "rejected paul.json must preserve both subjects");
    match result {
        Err(e) => assert!(e.to_string().contains("1 scanned, 0 extracted, 0 skipped, 1 failed")),
        Ok(_) => panic!("rejected paul.json must report a partial failure"),
    }
}

#[test]
fn sync_reports_invalid_relationship_field_and_value() {
    for key in ["related", "relatedTo"] {
        for value in ["[\"good\", \"bad<target\"]", "\n  - good\n  - bad<target"] {
            let tmp = tempfile::tempdir().unwrap();
            let home = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
            write_md(tmp.path(), "invalid.md", &format!("title: Bad\n{key}: {value}"), "");
            let output = std::process::Command::new(env!("CARGO_BIN_EXE_base"))
                .arg("sync")
                .current_dir(tmp.path())
                .env("BASE_HOME", home.path())
                .output().unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(!output.status.success());
            assert!(stderr.lines().any(|line| {
                line.starts_with("[frontmatter] invalid.md:")
                    && line.contains(&format!("key `{key}`"))
                    && line.contains("value \"bad<target\"")
            }), "missing file, original field, and offending item: {stderr}");
        }
    }
}
