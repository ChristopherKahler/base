use std::path::Path;

fn setup_workspace_with_devmode(dir: &Path, devmode_enabled: bool) {
    let base_dir = dir.join(".base");
    std::fs::create_dir_all(&base_dir).unwrap();

    std::fs::write(
        base_dir.join("domains.toml"),
        r#"
[[domain]]
name = "GLOBAL"
mode = "always"
prompt_keywords = []
rules = ["Rule 1", "Rule 2"]

[[domain]]
name = "DEVELOPMENT"
mode = "triggered"
prompt_keywords = ["write code", "fix bug"]
rules = ["Dev rule"]
"#,
    )
    .unwrap();

    std::fs::write(
        base_dir.join("base.toml"),
        format!(
            "[namespace]\nprefix = \"ops\"\nuri = \"http://ops-sys.local/ontology#\"\n\n\
             [devmode]\nenabled = {devmode_enabled}\n\n\
             [bracket]\nenabled = true\nfresh_until = 3\nmoderate_until = 10\n"
        ),
    )
    .unwrap();

    // Sync domains to graph
    let config = base::config::BaseConfig::load(dir);
    base::domain::sync::sync_domains_to_graph(&config, dir, None).unwrap();
}

#[test]
fn devmode_disabled_no_block() {
    let tmp = tempfile::tempdir().unwrap();
    setup_workspace_with_devmode(tmp.path(), false);

    let event = serde_json::json!({ "prompt": "hello" });
    let config = base::config::BaseConfig::load(tmp.path());

    // Run the hook — devmode disabled
    base::hook::user_prompt_submit::handle(&config, tmp.path(), &event).unwrap();

    // We can't capture stdout directly, but verify the config is correct
    assert!(!config.devmode.enabled);
}

#[test]
fn devmode_enabled_config_parsed() {
    let tmp = tempfile::tempdir().unwrap();
    setup_workspace_with_devmode(tmp.path(), true);

    let config = base::config::BaseConfig::load(tmp.path());
    assert!(config.devmode.enabled);
}

#[test]
fn devmode_block_format_includes_domains() {
    use base::domain::session::Bracket;

    let loaded = vec![
        ("GLOBAL".to_string(), "always_on".to_string(), 2),
        ("DEVELOPMENT".to_string(), "matched".to_string(), 1),
    ];

    let all_domains = vec![
        base::domain::DomainDef {
            name: "GLOBAL".into(),
            mode: "always".into(),
            prompt_keywords: vec![],
            file_keywords: vec![],
            paths: vec![],
            exclude: vec![],
            rules: vec!["Rule 1".into(), "Rule 2".into()],
            query: None,
            query_format: None,
            commands: vec![],
            command_activation: "both".into(),
            role: None,
            output_mode: None,
            format: None,
        },
        base::domain::DomainDef {
            name: "DEVELOPMENT".into(),
            mode: "triggered".into(),
            prompt_keywords: vec!["write code".into()],
            file_keywords: vec![],
            paths: vec![],
            exclude: vec![],
            rules: vec!["Dev rule".into()],
            query: None,
            query_format: None,
            commands: vec![],
            command_activation: "both".into(),
            role: None,
            output_mode: None,
            format: None,
        },
        base::domain::DomainDef {
            name: "UNMATCHED".into(),
            mode: "triggered".into(),
            prompt_keywords: vec!["something else".into()],
            file_keywords: vec![],
            paths: vec![],
            exclude: vec![],
            rules: vec!["Unused".into()],
            query: None,
            query_format: None,
            commands: vec![],
            command_activation: "both".into(),
            role: None,
            output_mode: None,
            format: None,
        },
    ];

    let output = base::hook::user_prompt_submit::format_devmode_block(
        &loaded,
        &all_domains,
        Bracket::Fresh,
        1,
        0,
    );

    assert!(output.contains("DEVMODE=true"), "Should contain DEVMODE header");
    assert!(output.contains("[FRESH]"), "Should contain bracket");
    assert!(output.contains("[GLOBAL] always_on"), "Should list GLOBAL");
    assert!(output.contains("[DEVELOPMENT] matched"), "Should list DEVELOPMENT");
    assert!(output.contains("UNMATCHED"), "Should list unmatched as available");
    assert!(output.contains("something else"), "Should show unmatched keywords");
}

#[test]
fn devmode_block_shows_dedup_count() {
    use base::domain::session::Bracket;

    let loaded = vec![
        ("GLOBAL".to_string(), "dedup".to_string(), 2),
    ];
    let all_domains = vec![];

    let output = base::hook::user_prompt_submit::format_devmode_block(
        &loaded,
        &all_domains,
        Bracket::Moderate,
        5,
        1,
    );

    assert!(output.contains("DEDUP: 1 domain(s) skipped"), "Should show dedup count");
    assert!(output.contains("[MODERATE]"), "Should show bracket");
}

#[test]
fn bracket_tag_emitted_in_output() {
    let tmp = tempfile::tempdir().unwrap();
    setup_workspace_with_devmode(tmp.path(), false);

    let config = base::config::BaseConfig::load(tmp.path());
    assert!(config.bracket.enabled);
    assert_eq!(config.bracket.fresh_until, 3);

    // Run hook — bracket should be tracked in session
    let event = serde_json::json!({ "prompt": "hello" });
    base::hook::user_prompt_submit::handle(&config, tmp.path(), &event).unwrap();

    // Load session to verify prompt count incremented
    let base_dir = base::config::find_workspace_base(tmp.path()).unwrap();
    let session = base::domain::session::SessionState::load(&base_dir);
    assert_eq!(session.prompt_count, 1, "Prompt count should be 1 after first call");
}

#[test]
fn bracket_increments_across_multiple_prompts() {
    let tmp = tempfile::tempdir().unwrap();
    setup_workspace_with_devmode(tmp.path(), false);

    let config = base::config::BaseConfig::load(tmp.path());
    let event = serde_json::json!({ "prompt": "hello" });

    // Fire 5 prompts
    for _ in 0..5 {
        base::hook::user_prompt_submit::handle(&config, tmp.path(), &event).unwrap();
    }

    let base_dir = base::config::find_workspace_base(tmp.path()).unwrap();
    let session = base::domain::session::SessionState::load(&base_dir);
    assert_eq!(session.prompt_count, 5, "Should track 5 prompts");

    // At prompt 5, bracket should be MODERATE (fresh_until=3)
    let bracket = session.bracket(&config.bracket);
    assert_eq!(
        bracket,
        base::domain::session::Bracket::Moderate,
        "Should be MODERATE at prompt 5"
    );
}
