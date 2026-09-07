use std::path::Path;

/// Helper: create .base/ directory with an initial domains.toml.
fn write_initial_domains(dir: &Path) {
    let base_dir = dir.join(".base");
    std::fs::create_dir_all(&base_dir).unwrap();
    std::fs::write(
        base_dir.join("domains.toml"),
        r#"
[[domain]]
name = "dev"
mode = "triggered"
keywords = ["write code"]
rules = ["Dev rule 1"]
"#,
    )
    .unwrap();
}

#[test]
fn add_keyword_trigger_to_existing_domain() {
    let tmp = tempfile::tempdir().unwrap();
    write_initial_domains(tmp.path());

    base::domain::add_trigger(tmp.path(), false, "dev", Some("fix bug"), None).unwrap();

    let domains = base::domain::load_domains(tmp.path());
    let dev = domains.iter().find(|d| d.name == "dev").unwrap();
    assert!(
        dev.prompt_keywords.contains(&"write code".to_string()),
        "Original keyword preserved"
    );
    assert!(
        dev.prompt_keywords.contains(&"fix bug".to_string()),
        "New keyword added"
    );
}

#[test]
fn add_trigger_creates_new_domain() {
    let tmp = tempfile::tempdir().unwrap();
    write_initial_domains(tmp.path());

    base::domain::add_trigger(tmp.path(), false, "review", Some("audit"), None).unwrap();

    let domains = base::domain::load_domains(tmp.path());
    let review = domains.iter().find(|d| d.name == "review").unwrap();
    assert_eq!(review.mode, "triggered");
    assert!(review.prompt_keywords.contains(&"audit".to_string()));
}

#[test]
fn add_path_trigger() {
    let tmp = tempfile::tempdir().unwrap();
    write_initial_domains(tmp.path());

    base::domain::add_trigger(tmp.path(), false, "dev", None, Some("src/")).unwrap();

    let domains = base::domain::load_domains(tmp.path());
    let dev = domains.iter().find(|d| d.name == "dev").unwrap();
    assert!(dev.paths.contains(&"src/".to_string()));
}

#[test]
fn add_trigger_no_duplicate_keywords() {
    let tmp = tempfile::tempdir().unwrap();
    write_initial_domains(tmp.path());

    // Add the same keyword twice
    base::domain::add_trigger(tmp.path(), false, "dev", Some("write code"), None).unwrap();

    let domains = base::domain::load_domains(tmp.path());
    let dev = domains.iter().find(|d| d.name == "dev").unwrap();
    let count = dev
        .prompt_keywords
        .iter()
        .filter(|k| *k == "write code")
        .count();
    assert_eq!(count, 1, "Should not duplicate existing keyword");
}

#[test]
fn add_trigger_creates_base_dir_if_missing() {
    let tmp = tempfile::tempdir().unwrap();
    // No .base/ directory at all

    base::domain::add_trigger(tmp.path(), false, "new-domain", Some("test"), None).unwrap();

    let domains = base::domain::load_domains(tmp.path());
    let new = domains.iter().find(|d| d.name == "new-domain");
    assert!(new.is_some(), "new-domain should exist in loaded domains");
    assert_eq!(new.unwrap().prompt_keywords, vec!["test"]);
}
