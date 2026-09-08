//! #77: the hook event line lands in the tier of the cwd the HOST reported, not the tier
//! of wherever the hook process happens to be standing.
//!
//! The line always recorded the payload's cwd in its own body, so before this it could
//! name workspace A and sit in workspace B's log — and `base doctor`'s hook trail, which
//! reads one tier, then reported on the wrong one. A failing hook in A was invisible
//! from A.

use std::path::Path;
use std::process::Command;

/// A workspace is a directory with a `.base/`. Two of them plus an isolated home is the
/// whole fixture — `BASE_HOME` points outside both so the global tier cannot absorb the
/// line and make the test pass for the wrong reason.
fn workspace(root: &Path, name: &str) -> std::path::PathBuf {
    let ws = root.join(name);
    std::fs::create_dir_all(ws.join(".base")).unwrap();
    ws
}

fn log_lines(ws: &Path) -> Vec<serde_json::Value> {
    let p = ws.join(".base").join("hook-events.jsonl");
    match std::fs::read_to_string(&p) {
        Ok(s) => s.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).unwrap()).collect(),
        Err(_) => Vec::new(),
    }
}

fn run_hook(event: &str, cwd: &Path, home: &Path, payload: &str) -> std::process::Output {
    use std::io::Write;
    let mut child = Command::new(env!("CARGO_BIN_EXE_base"))
        .args(["hook", event])
        .current_dir(cwd)
        .env("BASE_HOME", home)
        .env("BASE_NO_AUTO_UPDATE", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.as_mut().unwrap().write_all(payload.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn the_line_lands_in_the_workspace_the_payload_named() {
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".base-gbl").join(".base")).unwrap();
    let a = workspace(root.path(), "wsA");
    let b = workspace(root.path(), "wsB");

    // Shell stands in B; the host says the event happened in A.
    let payload = serde_json::json!({
        "session_id": "probe-f3",
        "cwd": a.to_string_lossy(),
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": "echo f3" },
        "tool_response": { "stdout": "f3" }
    })
    .to_string();
    let out = run_hook("post-tool-use", &b, home.path(), &payload);
    assert_eq!(out.status.code(), Some(0), "hooks fail open: {out:?}");

    let in_a = log_lines(&a);
    let in_b = log_lines(&b);
    assert_eq!(in_a.len(), 1, "the event belongs to A, whose cwd the host reported");
    assert!(in_b.is_empty(), "nothing belongs to B — the shell standing there is not the event's workspace");

    let line = &in_a[0];
    assert_eq!(line["cwd"].as_str().unwrap(), a.to_string_lossy(), "the body already named A");
    assert_eq!(line["cwd_source"].as_str(), Some("payload"), "and the log says which input chose the tier");
}

#[test]
fn a_payload_with_no_cwd_says_so_rather_than_implying_the_host_named_one() {
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".base-gbl").join(".base")).unwrap();
    let b = workspace(root.path(), "wsB");

    // No `cwd` key at all: falling back to the process cwd is correct here, and the only
    // wrong answer is a line that looks the same as one the host actually reported.
    let payload = r#"{"session_id":"probe","hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{"command":"echo x"}}"#;
    let out = run_hook("post-tool-use", &b, home.path(), payload);
    assert_eq!(out.status.code(), Some(0));

    let lines = log_lines(&b);
    assert_eq!(lines.len(), 1, "with no payload cwd the process cwd is the only input there is");
    assert_eq!(lines[0]["cwd_source"].as_str(), Some("process"), "absent is not the same as empty");
}

/// The arm the issue exists for. `hook_failure_summary` reports failures, so a FAILING
/// hook is the one that most needs to be findable from its own workspace — and it is the
/// arm where `HookEventData` is `None`, which is why the cwd could not come from there.
#[test]
fn a_failing_hook_still_logs_into_the_payloads_workspace() {
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".base-gbl").join(".base")).unwrap();
    let a = workspace(root.path(), "wsA");
    let b = workspace(root.path(), "wsB");

    // A graph that cannot be parsed makes the handler fail. The hook still exits 0 —
    // hooks fail open — and the failure has to be recorded where it happened.
    std::fs::write(a.join(".base").join("graph.nq"), "this is not n-quads {{{\n").unwrap();

    let payload = serde_json::json!({
        "session_id": "probe-fail",
        "cwd": a.to_string_lossy(),
        "hook_event_name": "PostToolUse",
        "tool_name": "Edit",
        "tool_input": { "file_path": a.join("src").join("main.rs").to_string_lossy() }
    })
    .to_string();
    let out = run_hook("post-tool-use", &b, home.path(), &payload);
    assert_eq!(out.status.code(), Some(0), "hooks fail open even when the handler errors");

    let in_a = log_lines(&a);
    let in_b = log_lines(&b);
    assert!(in_b.is_empty(), "the failure did not happen in B");
    assert_eq!(in_a.len(), 1, "the failure happened in A and is recorded in A");
    assert_eq!(in_a[0]["cwd_source"].as_str(), Some("payload"));
}

/// The two fields that were set on the struct and never written out (#75). Without them
/// the log said an extension nudge had not fired when it had — invisible in both
/// directions at once.
#[test]
fn the_log_line_carries_every_field_the_struct_sets() {
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".base-gbl").join(".base")).unwrap();
    let a = workspace(root.path(), "wsA");

    let payload = serde_json::json!({
        "session_id": "probe",
        "cwd": a.to_string_lossy(),
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": "echo hello" }
    })
    .to_string();
    run_hook("post-tool-use", &a, home.path(), &payload);

    let line = log_lines(&a).pop().expect("one event");
    for key in ["nudged", "standards_injected", "cwd_source", "section_context"] {
        assert!(!line[key].is_null(), "{key} must reach the log, not just the struct: {line}");
    }
}
