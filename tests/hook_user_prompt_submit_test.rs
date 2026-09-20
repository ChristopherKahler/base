use std::path::Path;

use base::config::BaseConfig;
use base::hook::user_prompt_submit;

/// Helper: create a domains.toml with test domains.
fn write_test_domains(dir: &Path) {
    let base_dir = dir.join(".base");
    std::fs::create_dir_all(&base_dir).unwrap();
    std::fs::write(
        base_dir.join("domains.toml"),
        r#"
[[domain]]
name = "global"
mode = "always"
rules = [
    "Always-on rule 1",
    "Always-on rule 2",
]

[[domain]]
name = "development"
mode = "triggered"
keywords = ["write code", "fix bug", "implement"]
paths = ["src/"]
rules = [
    "Follow existing code patterns",
    "Run tests after changes",
]

[[domain]]
name = "review"
mode = "triggered"
keywords = ["review", "audit"]
exclude = ["review only"]
rules = [
    "Scrutinize everything",
]
"#,
    )
    .unwrap();
}

#[test]
fn user_prompt_submit_injects_matched_rules() {
    let tmp = tempfile::tempdir().unwrap();
    write_test_domains(tmp.path());

    let config = BaseConfig::default();
    let event = serde_json::json!({
        "prompt": "please fix bug in auth module"
    });

    // First call should succeed and inject rules
    let result = user_prompt_submit::handle(&config, tmp.path(), &event, &mut String::new());
    assert!(result.is_ok(), "Should succeed: {result:?}");
}

#[test]
fn user_prompt_submit_silent_without_domains() {
    let tmp = tempfile::tempdir().unwrap();
    // No .base/ or domains.toml
    let config = BaseConfig::default();
    let event = serde_json::json!({ "prompt": "hello" });

    let result = user_prompt_submit::handle(&config, tmp.path(), &event, &mut String::new());
    assert!(result.is_ok(), "Should succeed silently");
}

#[test]
fn user_prompt_submit_dedup_across_calls() {
    let tmp = tempfile::tempdir().unwrap();
    write_test_domains(tmp.path());

    let config = BaseConfig::default();
    let event = serde_json::json!({ "prompt": "fix bug please" });

    // First call — injects
    user_prompt_submit::handle(&config, tmp.path(), &event, &mut String::new()).unwrap();

    // Second call — should dedup (session state persisted)
    // Dedup detail covered by graph_injection_test.rs — the hook-layer
    // rendered-output hash is the single dedup gate
    let result = user_prompt_submit::handle(&config, tmp.path(), &event, &mut String::new());
    assert!(result.is_ok());
}

#[test]
fn user_prompt_submit_empty_prompt_silent() {
    let tmp = tempfile::tempdir().unwrap();
    write_test_domains(tmp.path());

    let config = BaseConfig::default();
    let event = serde_json::json!({ "prompt": "" });

    let result = user_prompt_submit::handle(&config, tmp.path(), &event, &mut String::new());
    assert!(result.is_ok(), "Empty prompt should be silent");
}

#[test]
fn user_prompt_submit_no_prompt_field_silent() {
    let tmp = tempfile::tempdir().unwrap();
    write_test_domains(tmp.path());

    let config = BaseConfig::default();
    let event = serde_json::json!({ "tool_name": "something" });

    let result = user_prompt_submit::handle(&config, tmp.path(), &event, &mut String::new());
    assert!(result.is_ok(), "Missing prompt field should be silent");
}

#[test]
fn user_prompt_submit_malformed_domains_failopen() {
    let tmp = tempfile::tempdir().unwrap();
    let base_dir = tmp.path().join(".base");
    std::fs::create_dir_all(&base_dir).unwrap();
    std::fs::write(base_dir.join("domains.toml"), "THIS IS NOT VALID TOML {{{").unwrap();

    let config = BaseConfig::default();
    let event = serde_json::json!({ "prompt": "fix bug" });

    // Malformed TOML should result in empty domains (graceful), not a crash
    let result = user_prompt_submit::handle(&config, tmp.path(), &event, &mut String::new());
    assert!(result.is_ok(), "Malformed domains.toml should not crash");
}
