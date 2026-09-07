//! #19: outside a workspace, every tier-bound reader says which tier it searched.

use std::process::Command;

const SENTENCE: &str = "(no workspace here: searched the global tier only";

fn run(home: &std::path::Path, cwd: &std::path::Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_base"))
        .args(args)
        .current_dir(cwd)
        .env("BASE_HOME", home)
        .env("BASE_NO_AUTO_UPDATE", "1")
        .output()
        .unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into(), String::from_utf8_lossy(&out.stderr).into())
}

#[test]
fn every_tier_bound_reader_names_the_tier_outside_a_workspace() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".base-gbl").join(".base")).unwrap();
    let nowhere = tempfile::tempdir().unwrap();
    for args in [
        vec!["recall", "--keyword", "anything"],
        vec!["doctor"],
        vec!["commands", "list"],
        vec!["project", "list"],
    ] {
        let (_, _, err) = run(home.path(), nowhere.path(), &args);
        assert!(err.contains(SENTENCE), "{:?} did not name the tier; stderr was: {err}", args);
    }
}

#[test]
fn inside_a_workspace_nothing_is_added() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".base-gbl").join(".base")).unwrap();
    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".base")).unwrap();
    let (_, _, err) = run(home.path(), ws.path(), &["recall", "--keyword", "anything"]);
    assert!(!err.contains(SENTENCE), "inside a workspace the sentence must not print: {err}");
}

#[test]
fn project_list_outside_a_workspace_answers_from_the_global_tier() {
    // It exited 1 with "no .base/ directory found" while recall, doctor and
    // commands list all answered from the global tier. Four behaviours for one
    // situation was the whole of #19; erroring was the odd one out.
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".base-gbl").join(".base")).unwrap();
    let nowhere = tempfile::tempdir().unwrap();
    let (code, _out, err) = run(home.path(), nowhere.path(), &["project", "list"]);
    assert_eq!(code, 0, "project list must answer, not error; stderr was: {err}");
    assert!(err.contains(SENTENCE), "and it must name the tier: {err}");
    assert!(
        !err.contains("no .base/ directory found"),
        "the bail message must be gone: {err}"
    );
}
