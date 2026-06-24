use std::path::Path;

/// Set up a workspace with domains.toml, then run domain sync to populate the graph.
fn setup_synced_workspace(dir: &Path) {
    let base_dir = dir.join(".base");
    std::fs::create_dir_all(&base_dir).unwrap();

    std::fs::write(
        base_dir.join("domains.toml"),
        r#"
[[domain]]
name = "GLOBAL"
mode = "always"
prompt_keywords = []
rules = ["Never lie", "Always verify"]

[[domain]]
name = "DEVELOPMENT"
mode = "triggered"
prompt_keywords = ["write code", "fix bug", "implement"]
file_keywords = ["use crate", "fn main"]
paths = ["src/"]
rules = ["Test everything before claiming done"]
"#,
    )
    .unwrap();

    // Also write a base.toml so config loads cleanly
    std::fs::write(
        base_dir.join("base.toml"),
        "[namespace]\nprefix = \"ops\"\nuri = \"http://ops-sys.local/ontology#\"\n",
    )
    .unwrap();

    // Run domain sync to populate graph
    let config = base::config::BaseConfig::load(dir);
    base::domain::sync::sync_domains_to_graph(&config, dir, None).unwrap();
}

/// Set up workspace with domains + carl.json decisions synced.
fn setup_synced_with_decisions(dir: &Path) {
    setup_synced_workspace(dir);

    let carl_path = dir.join("carl.json");
    std::fs::write(
        &carl_path,
        r#"{
  "domains": [
    {
      "name": "DEVELOPMENT",
      "decisions": [
        {
          "decision": "Use Rust for hot path",
          "rationale": "Performance critical",
          "recall": "When discussing architecture"
        },
        {
          "decision": "Fail-open on all hooks",
          "rationale": "Never block Claude",
          "recall": "When implementing hooks"
        }
      ]
    }
  ]
}"#,
    )
    .unwrap();

    let config = base::config::BaseConfig::load(dir);
    base::domain::sync::sync_domains_to_graph(&config, dir, Some(&carl_path)).unwrap();
}

#[test]
fn graph_injection_returns_rules_from_graph() {
    let tmp = tempfile::tempdir().unwrap();
    setup_synced_workspace(tmp.path());

    // Simulate a user-prompt-submit event that matches GLOBAL (always-on)
    let event = serde_json::json!({
        "prompt": "hello world"
    });

    let config = base::config::BaseConfig::load(tmp.path());

    // Capture stdout
    let result = std::panic::catch_unwind(|| {
        base::hook::user_prompt_submit::handle(&config, tmp.path(), &event).unwrap();
    });
    assert!(result.is_ok());

    // Since we can't easily capture stdout in integration tests,
    // verify the graph has the rules we expect
    let ns = &config.namespace;
    let p = &ns.prefix;
    let domain_iri = format!("{}domain/global", ns.uri);

    let results = base::crud::load_and_query(
        tmp.path(),
        ns,
        &format!(
            "SELECT ?text WHERE {{ GRAPH ?g {{ <{domain_iri}> {p}:hasRule ?r . ?r {p}:ruleText ?text }} }}"
        ),
    )
    .unwrap();

    if let oxigraph::sparql::QueryResults::Solutions(solutions) = results {
        let texts: Vec<String> = solutions
            .filter_map(|r| r.ok())
            .filter_map(|row| {
                row.get("text").map(|t| {
                    use oxigraph::model::TermRef;
                    match t.into() {
                        TermRef::Literal(l) => l.value().to_string(),
                        _ => String::new(),
                    }
                })
            })
            .collect();
        assert_eq!(texts.len(), 2);
        assert!(texts.iter().any(|t| t.contains("Never lie")));
        assert!(texts.iter().any(|t| t.contains("Always verify")));
    } else {
        panic!("Expected solutions from graph query");
    }
}

#[test]
fn graph_injection_includes_neighborhood_decisions() {
    let tmp = tempfile::tempdir().unwrap();
    setup_synced_with_decisions(tmp.path());

    let ns = &base::config::BaseConfig::load(tmp.path()).namespace;
    let p = &ns.prefix;
    let domain_iri = format!("{}domain/development", ns.uri);

    // Verify decisions are linked to the DEVELOPMENT domain
    let results = base::crud::load_and_query(
        tmp.path(),
        ns,
        &format!(
            "SELECT ?name WHERE {{ GRAPH ?g {{ <{domain_iri}> {p}:hasDecision ?d . ?d {p}:name ?name }} }}"
        ),
    )
    .unwrap();

    if let oxigraph::sparql::QueryResults::Solutions(solutions) = results {
        let names: Vec<String> = solutions
            .filter_map(|r| r.ok())
            .filter_map(|row| {
                row.get("name").map(|t| {
                    use oxigraph::model::TermRef;
                    match t.into() {
                        TermRef::Literal(l) => l.value().to_string(),
                        _ => String::new(),
                    }
                })
            })
            .collect();
        assert_eq!(names.len(), 2, "Should have 2 decisions for DEVELOPMENT");
        assert!(names.iter().any(|n| n.contains("Rust")));
        assert!(names.iter().any(|n| n.contains("Fail-open")));
    } else {
        panic!("Expected solutions");
    }
}

#[test]
fn fallback_to_toml_when_graph_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let base_dir = tmp.path().join(".base");
    std::fs::create_dir_all(&base_dir).unwrap();

    // Write domains.toml but do NOT run sync — graph doesn't exist
    std::fs::write(
        base_dir.join("domains.toml"),
        r#"
[[domain]]
name = "FALLBACK"
mode = "always"
prompt_keywords = []
rules = ["Fallback rule"]
"#,
    )
    .unwrap();

    // The hook should still work (fail-open, use TOML rules directly)
    let event = serde_json::json!({ "prompt": "test" });
    let config = base::config::BaseConfig::load(tmp.path());
    let result = base::hook::user_prompt_submit::handle(&config, tmp.path(), &event);
    assert!(result.is_ok(), "Should not error even without graph");
}

#[test]
fn dedup_skips_unchanged_graph_injection() {
    let tmp = tempfile::tempdir().unwrap();
    setup_synced_workspace(tmp.path());

    let event = serde_json::json!({ "prompt": "hello" });
    let config = base::config::BaseConfig::load(tmp.path());

    // First call — should inject
    base::hook::user_prompt_submit::handle(&config, tmp.path(), &event).unwrap();

    // Load session state to verify something was marked
    let base_dir = base::config::find_workspace_base(tmp.path()).unwrap();
    let session = base::domain::session::SessionState::load(&base_dir);
    assert!(
        session.injected.contains_key("GLOBAL"),
        "GLOBAL should be marked as injected after first call"
    );

    // Second call with same prompt — GLOBAL should be deduped at the hook
    // layer (single dedup gate: rendered-output hash)
    let result = base::hook::user_prompt_submit::handle(&config, tmp.path(), &event).unwrap();
    assert!(
        result.suppressed >= 1,
        "Second identical call should report suppressed domains, got: {}",
        result.suppressed
    );
}

#[test]
fn rule_change_reinjects_after_dedup() {
    let tmp = tempfile::tempdir().unwrap();
    setup_synced_workspace(tmp.path());

    let event = serde_json::json!({ "prompt": "hello" });
    let config = base::config::BaseConfig::load(tmp.path());

    // First call — injects GLOBAL, marks it in session state
    let first = base::hook::user_prompt_submit::handle(&config, tmp.path(), &event).unwrap();
    assert!(
        first.domains_matched.iter().any(|d| d == "GLOBAL"),
        "First call should inject GLOBAL, got: {:?}",
        first.domains_matched
    );

    // Change GLOBAL's rules and re-sync — rendered output hash changes
    let base_dir = tmp.path().join(".base");
    std::fs::write(
        base_dir.join("domains.toml"),
        r#"
[[domain]]
name = "GLOBAL"
mode = "always"
prompt_keywords = []
rules = ["Never lie", "Always verify", "NEW RULE added later"]
"#,
    )
    .unwrap();
    base::domain::sync::sync_domains_to_graph(&config, tmp.path(), None).unwrap();

    // Second call — changed content must RE-inject, not dedup
    let second = base::hook::user_prompt_submit::handle(&config, tmp.path(), &event).unwrap();
    assert!(
        second.domains_matched.iter().any(|d| d == "GLOBAL"),
        "Changed rules should re-inject GLOBAL, got: {:?} (suppressed: {})",
        second.domains_matched,
        second.suppressed
    );
}

#[test]
fn sync_gc_removes_stale_synced_rules_keeps_cli_rules() {
    let tmp = tempfile::tempdir().unwrap();
    setup_synced_workspace(tmp.path()); // GLOBAL: ["Never lie", "Always verify"]
    let config = base::config::BaseConfig::load(tmp.path());
    let ns = &config.namespace;

    // Add a CLI rule (no source marker) — must survive sync GC
    base::crud::rule::add(tmp.path(), ns, "GLOBAL", "CLI-added rule", None).unwrap();

    // Change toml rules: drop "Always verify", add "New rule C"
    let base_dir = tmp.path().join(".base");
    std::fs::write(
        base_dir.join("domains.toml"),
        r#"
[[domain]]
name = "GLOBAL"
mode = "always"
prompt_keywords = []
rules = ["Never lie", "New rule C"]
"#,
    )
    .unwrap();
    base::domain::sync::sync_domains_to_graph(&config, tmp.path(), None).unwrap();
    // Sync twice — idempotency
    base::domain::sync::sync_domains_to_graph(&config, tmp.path(), None).unwrap();

    let store = base::store::load_graph(&base_dir.join("graph.nq")).unwrap();
    let p = &ns.prefix;
    let u = &ns.uri;
    let sparql = format!(
        "PREFIX {p}: <{u}>\nSELECT ?text WHERE {{ GRAPH ?g {{ ?r a {p}:Rule ; {p}:ruleText ?text . }} }}"
    );
    let texts: Vec<String> = match store.query(&sparql).unwrap() {
        oxigraph::sparql::QueryResults::Solutions(sols) => sols
            .filter_map(|r| r.ok())
            .filter_map(|row| row.get("text").map(|t| t.to_string()))
            .collect(),
        _ => panic!("Expected solutions"),
    };

    assert!(texts.iter().any(|t| t.contains("Never lie")), "kept synced rule: {texts:?}");
    assert!(texts.iter().any(|t| t.contains("New rule C")), "new synced rule: {texts:?}");
    assert!(texts.iter().any(|t| t.contains("CLI-added rule")), "CLI rule survives GC: {texts:?}");
    assert!(!texts.iter().any(|t| t.contains("Always verify")), "stale synced rule GC'd: {texts:?}");
    assert_eq!(texts.len(), 3, "exactly 3 rules after double sync (idempotent): {texts:?}");
}

#[test]
fn carl_migration_real_dict_format_imports_rules_and_decisions() {
    let tmp = tempfile::tempdir().unwrap();
    setup_synced_workspace(tmp.path());
    let config = base::config::BaseConfig::load(tmp.path());
    let ns = &config.namespace;

    let carl_path = tmp.path().join("carl.json");
    std::fs::write(
        &carl_path,
        r#"{
  "version": "2.0",
  "domains": {
    "DEVELOPMENT": {
      "state": "active",
      "always_on": false,
      "rules": [
        {"id": 0, "text": "Carl rule one", "added": "2026-03-23", "source": "manual"},
        {"id": 1, "text": "Carl rule \"quoted\" two", "added": "2026-03-23"}
      ],
      "decisions": [
        {"id": 0, "decision": "Use Rust", "rationale": "Fast", "recall": "arch", "status": "active"},
        {"id": 1, "decision": "Old idea", "rationale": "Superseded", "status": "archived"}
      ]
    }
  }
}"#,
    )
    .unwrap();

    base::domain::sync::sync_domains_to_graph(&config, tmp.path(), Some(&carl_path)).unwrap();
    // Re-run — idempotent for rules (dedup by text), decisions delete+reinsert
    base::domain::sync::sync_domains_to_graph(&config, tmp.path(), Some(&carl_path)).unwrap();

    let base_dir = tmp.path().join(".base");
    let store = base::store::load_graph(&base_dir.join("graph.nq")).unwrap();
    let p = &ns.prefix;
    let u = &ns.uri;

    let rules_q = format!(
        "PREFIX {p}: <{u}>\nSELECT ?text WHERE {{ GRAPH ?g {{ <{u}domain/development> {p}:hasRule ?r . ?r {p}:ruleText ?text . ?r {p}:index ?i . }} }}"
    );
    let rule_texts: Vec<String> = match store.query(&rules_q).unwrap() {
        oxigraph::sparql::QueryResults::Solutions(sols) => sols
            .filter_map(|r| r.ok())
            .filter_map(|row| row.get("text").map(|t| t.to_string()))
            .collect(),
        _ => panic!("Expected solutions"),
    };
    assert_eq!(rule_texts.len(), 2, "2 carl rules imported once (idempotent): {rule_texts:?}");
    assert!(rule_texts.iter().any(|t| t.contains("quoted")), "escaped rule imported: {rule_texts:?}");

    let dec_q = format!(
        "PREFIX {p}: <{u}>\nSELECT ?name ?status WHERE {{ GRAPH ?g {{ ?d a {p}:Decision ; {p}:name ?name ; {p}:status ?status . }} }}"
    );
    let decs: Vec<(String, String)> = match store.query(&dec_q).unwrap() {
        oxigraph::sparql::QueryResults::Solutions(sols) => sols
            .filter_map(|r| r.ok())
            .filter_map(|row| {
                Some((row.get("name")?.to_string(), row.get("status")?.to_string()))
            })
            .collect(),
        _ => panic!("Expected solutions"),
    };
    assert!(decs.iter().any(|(n, s)| n.contains("Use Rust") && s.contains("active")));
    assert!(
        decs.iter().any(|(n, s)| n.contains("Old idea") && s.contains("archived")),
        "import-all: archived decision imported with its status: {decs:?}"
    );
}
