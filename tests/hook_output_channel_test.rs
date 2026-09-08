//! #75 and #76: what a hook writes has to go out on a channel the host actually delivers.
//!
//! Claude Code adds plain stdout to the model's context on `UserPromptSubmit`,
//! `UserPromptExpansion`, `SessionStart` and `PostModelSwitch`. On every other event it
//! captures stdout into the transcript and drops it. So on `PostToolUse` the text has to
//! travel in `hookSpecificOutput.additionalContext`, and on `Stop` — which has no
//! additive model channel at all — the only honest destination is `systemMessage`, which
//! speaks to the operator.
//!
//! These tests pin the CHANNEL, not the content. Content is what the handlers already
//! test; the channel is what made every one of those handlers a no-op.

use std::io::Write;
use std::path::Path;
use std::process::Command;

fn isolated_home() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".base-gbl").join(".base")).unwrap();
    home
}

/// A workspace whose graph knows one project, living at `project_path`. Moving that
/// directory is what makes `emit_move_nudge` speak.
fn workspace_with_project(root: &Path, project_path: &str) -> std::path::PathBuf {
    let ws = root.join("ws");
    std::fs::create_dir_all(ws.join(".base")).unwrap();
    let escaped = base::crud::escape_sparql_literal(project_path);
    let g = "<http://ops-sys.local/ontology#graph/ws/test>";
    let p = "<http://ops-sys.local/ontology#project/alpha>";
    std::fs::write(
        ws.join(".base").join("graph.nq"),
        format!(
            "{p} <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ops-sys.local/ontology#Project> {g} .\n\
             {p} <http://ops-sys.local/ontology#name> \"Alpha\" {g} .\n\
             {p} <http://ops-sys.local/ontology#status> \"active\" {g} .\n\
             {p} <http://ops-sys.local/ontology#path> \"{escaped}\" {g} .\n"
        ),
    )
    .unwrap();
    ws
}

fn run_hook(event: &str, cwd: &Path, home: &Path, payload: &str) -> (i32, String, String) {
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
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The block this pins is one of the two the issue did NOT name. `emit_move_nudge`'s
/// `<base:path-drift>` text tells the model to run `base project repath`, so it was a
/// model-facing instruction on a dead channel for exactly as long as the two nudges #75
/// counted — and a fix that moved only those two would have closed the issue with this
/// still broken.
#[test]
fn post_tool_use_context_travels_in_the_json_envelope() {
    let root = tempfile::tempdir().unwrap();
    let home = isolated_home();
    let old_dir = root.path().join("alpha-old");
    std::fs::create_dir_all(&old_dir).unwrap();
    let new_dir = root.path().join("alpha-new");
    std::fs::create_dir_all(&new_dir).unwrap();
    let ws = workspace_with_project(root.path(), old_dir.to_str().unwrap());

    let payload = serde_json::json!({
        "session_id": "probe-75",
        "cwd": ws.to_string_lossy(),
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": format!("mv {} {}", old_dir.display(), new_dir.display()) },
        "tool_response": { "stdout": "" }
    })
    .to_string();

    let (code, stdout, stderr) = run_hook("post-tool-use", &ws, home.path(), &payload);
    assert_eq!(code, 0, "hooks fail open. stderr: {stderr}");

    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        // The nudge did not fire, so there is no channel to test. Say so loudly rather
        // than passing: a silent skip here is the same shape as the bug.
        panic!("the move nudge did not fire, so this test measured nothing. stderr: {stderr}");
    }

    let v: serde_json::Value = serde_json::from_str(trimmed).unwrap_or_else(|e| {
        panic!("PostToolUse stdout must be the JSON envelope, not a bare block ({e}):\n{trimmed}")
    });
    assert_eq!(
        v["hookSpecificOutput"]["hookEventName"].as_str(),
        Some("PostToolUse"),
        "the envelope names its own event, or the host will not route it: {v}"
    );
    let ctx = v["hookSpecificOutput"]["additionalContext"].as_str().expect("additionalContext");
    assert!(ctx.contains("<base:path-drift>"), "the nudge belongs inside the envelope: {ctx}");
    assert!(ctx.contains("base project repath"), "and it still carries its instruction: {ctx}");
}

/// Nothing to say means nothing on stdout. An empty envelope is not harmless: the host
/// parses stdout on this event, and a bare `{}` invites it to route an empty context.
#[test]
fn post_tool_use_says_nothing_when_it_has_nothing_to_say() {
    let root = tempfile::tempdir().unwrap();
    let home = isolated_home();
    let ws = root.path().join("quiet");
    std::fs::create_dir_all(ws.join(".base")).unwrap();

    let payload = serde_json::json!({
        "session_id": "probe-quiet",
        "cwd": ws.to_string_lossy(),
        "hook_event_name": "PostToolUse",
        "tool_name": "WebSearch",
        "tool_input": { "query": "nothing to do with files" }
    })
    .to_string();

    let (code, stdout, _) = run_hook("post-tool-use", &ws, home.path(), &payload);
    assert_eq!(code, 0);
    assert!(stdout.trim().is_empty(), "expected silence, got: {stdout}");
}

/// #76. Stop must never write a bare block again. Whatever it emits is either nothing or
/// a JSON object — and if it carries text, that text is under `systemMessage`, which is
/// the one Stop channel that produces a record (`hook_system_message`) rather than being
/// captured and dropped.
#[test]
fn stop_never_writes_a_bare_block_to_stdout() {
    let root = tempfile::tempdir().unwrap();
    let home = isolated_home();
    let ws = root.path().join("ws");
    std::fs::create_dir_all(ws.join(".base")).unwrap();

    let payload = serde_json::json!({
        "session_id": "probe-76",
        "cwd": ws.to_string_lossy(),
        "hook_event_name": "Stop"
    })
    .to_string();

    let (code, stdout, stderr) = run_hook("stop", &ws, home.path(), &payload);
    assert_eq!(code, 0, "stderr: {stderr}");

    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return; // no open task, nothing to announce — correct and common
    }
    let v: serde_json::Value = serde_json::from_str(trimmed)
        .unwrap_or_else(|e| panic!("Stop stdout must be JSON, never a bare block ({e}):\n{trimmed}"));
    assert!(
        v.get("systemMessage").is_some(),
        "the only Stop key that reaches anybody is systemMessage: {v}"
    );
    assert!(
        !trimmed.starts_with('<'),
        "a relay block on raw stdout is the #76 defect exactly: {trimmed}"
    );
}

/// The events where plain stdout IS delivered keep it. This is the control: if the fix
/// had been "wrap everything in an envelope", these two would have broken, and the
/// injections that already worked would have stopped working.
#[test]
fn the_events_that_deliver_plain_stdout_still_use_it() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/hook/mod.rs")).unwrap();
    let arm_of = |needle: &str| -> String {
        let start = src.find(needle).unwrap_or_else(|| panic!("arm {needle} not found"));
        let rest = &src[start..];
        let end = rest.find("\n        }\n").map(|i| i + start).unwrap_or(src.len());
        src[start..end].to_string()
    };
    for arm in ["\"session-start\" =>", "\"user-prompt-submit\" =>"] {
        let body = arm_of(arm);
        assert!(body.contains("print!(\"{block}\")"), "{arm} must keep plain stdout — the host delivers it there");
    }
    for arm in ["\"pre-tool-use\" =>", "\"post-tool-use\" =>"] {
        let body = arm_of(arm);
        assert!(body.contains("hookSpecificOutput"), "{arm} must use the envelope");
        assert!(!body.contains("print!(\"{block}\")"), "{arm} must not write bare blocks");
    }
    let stop = arm_of("\"stop\" =>");
    assert!(stop.contains("systemMessage"), "stop speaks to the operator");
    assert!(!stop.contains("print!(\"{block}\")"), "stop must not write a bare block");
}
