//! The Antigravity binding: plugin and hooks wiring in `~/.gemini/config/plugins/base/`,
//! exact unwiring, and payload translations.

use std::path::Path;

use base::harness::Binding;
use base::harness::antigravity::{BINDING, inbound, outbound};
use serde_json::{Value, json};

fn hooks_path(home: &Path) -> std::path::PathBuf {
    home.join(".gemini").join("config").join("plugins").join("base").join("hooks.json")
}

fn plugin_path(home: &Path) -> std::path::PathBuf {
    home.join(".gemini").join("config").join("plugins").join("base").join("plugin.json")
}

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()))
}

#[test]
fn wiring_writes_plugin_and_hooks_files() {
    let dir = tempfile::tempdir().unwrap();
    let outcome = BINDING.wire(dir.path(), Path::new("/usr/local/bin/base")).unwrap();

    let hp = hooks_path(dir.path());
    let pp = plugin_path(dir.path());

    assert_eq!(outcome.path, hp);
    assert_eq!(outcome.changed, 5);
    assert!(hp.exists());
    assert!(pp.exists());

    let plugin = read_json(&pp);
    assert_eq!(plugin["name"], "base");

    let hooks = read_json(&hp);
    assert!(hooks["base"]["enabled"].as_bool().unwrap_or(false));
    assert!(hooks["base"]["SessionStart"].is_array());
    assert!(hooks["base"]["PreInvocation"].is_array());
    assert!(hooks["base"]["PreToolUse"].is_array());
    assert!(hooks["base"]["PostToolUse"].is_array());
    assert!(hooks["base"]["Stop"].is_array());
}

#[test]
fn wiring_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let first = BINDING.wire(dir.path(), Path::new("base")).unwrap();
    assert_eq!(first.changed, 5);

    let second = BINDING.wire(dir.path(), Path::new("base")).unwrap();
    assert_eq!(second.changed, 0);
    assert_eq!(second.note, "already wired");
}

#[test]
fn unwiring_cleans_up_files_and_dir() {
    let dir = tempfile::tempdir().unwrap();
    BINDING.wire(dir.path(), Path::new("base")).unwrap();

    let hp = hooks_path(dir.path());
    let pp = plugin_path(dir.path());
    assert!(hp.exists());

    let outcome = BINDING.unwire(dir.path()).unwrap();
    assert_eq!(outcome.changed, 5);
    assert!(!hp.exists());
    assert!(!pp.exists());
    assert!(!hp.parent().unwrap().exists());
}

#[test]
fn unwiring_when_not_wired_is_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let outcome = BINDING.unwire(dir.path()).unwrap();
    assert_eq!(outcome.changed, 0);
    assert_eq!(outcome.note, "not wired");
}

#[test]
fn inbound_maps_conversation_id_and_workspace() {
    let raw = json!({
        "conversationId": "conv-123",
        "workspacePaths": ["/home/user/project", "/other"],
        "lastUserInput": "explain this code"
    });

    let mapped = inbound("session-start", raw);
    assert_eq!(mapped["session_id"], "conv-123");
    assert_eq!(mapped["conversationId"], "conv-123");
    assert_eq!(mapped["cwd"], "/home/user/project");
    assert_eq!(mapped["prompt"], "explain this code");
}

#[test]
fn inbound_maps_run_command_to_bash() {
    let raw = json!({
        "conversationId": "c1",
        "workspacePaths": ["/repo"],
        "toolCall": {
            "name": "run_command",
            "args": {
                "CommandLine": "cargo test --lib",
                "WaitMsBeforeAsync": 5000
            }
        }
    });

    let mapped = inbound("pre-tool-use", raw);
    assert_eq!(mapped["tool_name"], "Bash");
    assert_eq!(mapped["tool_input"]["CommandLine"], "cargo test --lib");
    assert_eq!(mapped["tool_input"]["command"], "cargo test --lib");
    assert_eq!(mapped["tool_input"]["WaitMsBeforeAsync"], 5000);
}

#[test]
fn inbound_maps_read_and_edit_tools() {
    let raw = json!({
        "toolCall": {
            "name": "edit_file",
            "args": {
                "Path": "/repo/src/main.rs",
                "Target": "old",
                "Replacement": "new"
            }
        }
    });

    let mapped = inbound("pre-tool-use", raw);
    assert_eq!(mapped["tool_name"], "Edit");
    assert_eq!(mapped["tool_input"]["file_path"], "/repo/src/main.rs");
    assert_eq!(mapped["tool_input"]["Path"], "/repo/src/main.rs");
}

#[test]
fn outbound_session_start_formats_inject_steps() {
    let out = outbound("session-start", "Startup instructions").unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["injectSteps"][0]["systemMessage"]["systemMessage"],
        "Startup instructions"
    );
}

#[test]
fn outbound_user_prompt_submit_formats_ephemeral_message() {
    let out = outbound("user-prompt-submit", "Domain hint: auth").unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["injectSteps"][0]["ephemeralMessage"],
        "Domain hint: auth"
    );
}

#[test]
fn outbound_pre_tool_use_allows() {
    let out = outbound("pre-tool-use", "any context").unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["decision"], "allow");
}

#[test]
fn outbound_unwraps_claude_envelope() {
    let envelope = r#"{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"extracted text"}}"#;
    let out = outbound("user-prompt-submit", envelope).unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["injectSteps"][0]["ephemeralMessage"], "extracted text");
}
