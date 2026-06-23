use base::config::{BaseConfig, NamespaceConfig};
use base::crud;
use base::signal;

fn test_config() -> BaseConfig {
    BaseConfig::default()
}

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

/// Helper: populate a workspace with test entities at various timestamps.
fn seed_workspace(dir: &std::path::Path) {
    let ns = ns();

    // Active project (recent)
    crud::project::add(dir, &ns, "Active Project", "active", None).unwrap();

    // Blocked project
    crud::project::add(dir, &ns, "Blocked Project", "blocked", None).unwrap();
    crud::project::update(dir, &ns, "blocked-project", Some("blocked"), Some("waiting on API"), None).unwrap();

    // Active task
    crud::task::add(dir, &ns, "active-project", "Fix Auth", Some("high"), None).unwrap();

    // Deferred project — excluded from active-awareness (protocol's call, not a time window).
    crud::project::add(dir, &ns, "Deferred Project", "deferred", None).unwrap();

    // Decision
    crud::decision::log(dir, &ns, "dev", "Use JWT", "Stateless", None).unwrap();
}

#[test]
fn active_awareness_surfaces_recent_entities() {
    let tmp = tempfile::tempdir().unwrap();
    seed_workspace(tmp.path());

    let config = test_config();
    let output = signal::active_awareness::run(tmp.path(), &config.namespace).unwrap();

    assert!(output.contains("Active Project"), "Should include active project");
    assert!(output.contains("Blocked Project"), "Should include blocked project");
    assert!(output.contains("Fix Auth"), "Should include active task");
    // Deferred project must NOT appear — protocol's deferral is the gate, not a time window.
    assert!(!output.contains("Deferred Project"), "Deferred project should not appear in active-awareness");
}

#[test]
fn pulse_shows_counts() {
    let tmp = tempfile::tempdir().unwrap();
    seed_workspace(tmp.path());

    let config = test_config();
    let output = signal::pulse::run(tmp.path(), &config.namespace, &config.signal).unwrap();

    assert!(output.contains("Pulse"), "Should have Pulse header");
    assert!(output.contains("active"), "Should mention active count");
    assert!(output.contains("blocked"), "Should mention blocked count");
}

#[test]
fn suppression_skips_unchanged_signals() {
    let tmp = tempfile::tempdir().unwrap();
    seed_workspace(tmp.path());

    let config = test_config();

    // First run — should produce output
    let output1 = signal::run_signals(tmp.path(), &config, "test").unwrap();
    assert!(!output1.content.is_empty(), "First run should produce output");

    // Second run — nothing changed, should be suppressed
    let output2 = signal::run_signals(tmp.path(), &config, "test").unwrap();
    assert!(output2.content.is_empty(), "Second run should be suppressed (no changes)");
}

#[test]
fn suppression_re_emits_on_change() {
    let tmp = tempfile::tempdir().unwrap();
    seed_workspace(tmp.path());

    let config = test_config();

    // First run
    signal::run_signals(tmp.path(), &config, "test").unwrap();

    // Change data — add a new project
    crud::project::add(tmp.path(), &ns(), "New Project", "active", None).unwrap();

    // Third run — data changed, should re-emit
    let output3 = signal::run_signals(tmp.path(), &config, "test").unwrap();
    assert!(!output3.content.is_empty(), "Should re-emit after data change");
}

#[test]
fn budget_cap_truncates() {
    let tmp = tempfile::tempdir().unwrap();
    seed_workspace(tmp.path());

    let mut config = test_config();
    config.signal.max_chars = 50; // Very small budget

    let output = signal::run_signals(tmp.path(), &config, "test").unwrap();
    // Active-awareness (priority 1) should always appear regardless of budget
    // Lower-priority signals may be dropped
    assert!(!output.content.is_empty(), "Priority 1 signal should always emit");
}

#[test]
fn disabled_signals_emit_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    seed_workspace(tmp.path());

    let mut config = test_config();
    config.signal.enabled = false;

    let output = signal::run_signals(tmp.path(), &config, "test").unwrap();
    assert!(output.content.is_empty(), "Disabled signals should emit nothing");
    assert!(output.diagnostics.is_empty(), "Disabled signals should emit no diagnostics");
}
