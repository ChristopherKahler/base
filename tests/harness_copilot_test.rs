//! The Copilot CLI binding: the file it writes, the entries it takes back out,
//! and the two payload translations.
//!
//! No Copilot binary is needed, and none is present on the machine this was
//! written on: every claim here is against the documented hook shape. What the
//! tests do exercise is the part base owns — that wiring is idempotent, that
//! un-wiring leaves another tool's entries byte-for-byte in place (as a parsed
//! value), and that a Copilot payload reaches the engine where it looks.
//!
//! ── ISOLATION ──
//!
//! `COPILOT_HOME` is process-global and every test in this binary runs in one
//! process, so every test that touches the filesystem takes `ENV_LOCK` first and
//! restores the variable on drop, including "was not set at all", which is a
//! different state from "was set to empty".

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use base::harness::Binding;
use base::harness::copilot::{BINDING, inbound, outbound};
use serde_json::{Value, json};

/// The six events base wires, host name first.
const EVENTS: [(&str, &str); 6] = [
    ("sessionStart", "session-start"),
    ("userPromptSubmitted", "user-prompt-submit"),
    ("preToolUse", "pre-tool-use"),
    ("postToolUse", "post-tool-use"),
    ("agentStop", "stop"),
    ("sessionEnd", "session-end"),
];

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Holds `ENV_LOCK` for the test's lifetime and puts `COPILOT_HOME` back exactly
/// as it was. A panicking test must not poison the lock for every test after it.
struct CopilotEnv {
    _lock: MutexGuard<'static, ()>,
    saved: Option<OsString>,
}

impl CopilotEnv {
    fn pointing_at(dir: &Path) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let saved = std::env::var_os("COPILOT_HOME");
        unsafe { std::env::set_var("COPILOT_HOME", dir) };
        Self { _lock: lock, saved }
    }

    fn cleared() -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let saved = std::env::var_os("COPILOT_HOME");
        unsafe { std::env::remove_var("COPILOT_HOME") };
        Self { _lock: lock, saved }
    }
}

impl Drop for CopilotEnv {
    fn drop(&mut self) {
        match &self.saved {
            Some(value) => unsafe { std::env::set_var("COPILOT_HOME", value) },
            None => unsafe { std::env::remove_var("COPILOT_HOME") },
        }
    }
}

fn read_json(path: &Path) -> Value {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()))
}

fn write_json(path: &Path, value: &Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
}

#[test]
fn wiring_writes_one_entry_per_event_for_both_shells() {
    let dir = tempfile::tempdir().unwrap();
    let _env = CopilotEnv::pointing_at(dir.path());

    let outcome = BINDING
        .wire(dir.path(), Path::new("/usr/local/bin/base"))
        .unwrap();
    let path = dir.path().join("hooks").join("base.json");
    assert_eq!(outcome.path, path);
    assert_eq!(outcome.changed, EVENTS.len(), "{}", outcome.note);
    assert_eq!(
        outcome.note,
        "added sessionStart, userPromptSubmitted, preToolUse, postToolUse, agentStop, sessionEnd"
    );

    let root = read_json(&path);
    assert_eq!(root["version"], 1);
    let hooks = root["hooks"].as_object().expect("hooks is an object");
    assert_eq!(
        hooks.len(),
        EVENTS.len(),
        "exactly the six events base serves"
    );

    for (host_event, event) in EVENTS {
        let entries = hooks[host_event]
            .as_array()
            .unwrap_or_else(|| panic!("{host_event} is not an array"));
        assert_eq!(entries.len(), 1, "{host_event}");
        let entry = &entries[0];
        let command = format!("base hook {event} --harness copilot");
        assert_eq!(entry["type"], "command", "{host_event}");
        assert_eq!(entry["bash"], command, "{host_event}");
        assert_eq!(entry["powershell"], command, "{host_event}");
        assert_eq!(entry["timeoutSec"], 10, "{host_event}");
    }
}

#[test]
fn wiring_twice_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let _env = CopilotEnv::pointing_at(dir.path());

    let first = BINDING.wire(dir.path(), Path::new("base")).unwrap();
    let bytes = std::fs::read(&first.path).unwrap();

    let second = BINDING.wire(dir.path(), Path::new("base")).unwrap();
    assert_eq!(second.changed, 0);
    assert_eq!(second.note, "already wired");
    assert_eq!(
        std::fs::read(&second.path).unwrap(),
        bytes,
        "a second wire rewrote the file"
    );
}

#[test]
fn unwire_deletes_a_file_that_is_only_base() {
    let dir = tempfile::tempdir().unwrap();
    let _env = CopilotEnv::pointing_at(dir.path());
    BINDING.wire(dir.path(), Path::new("base")).unwrap();
    let path = BINDING.config_path(dir.path());
    assert!(path.exists());

    let outcome = BINDING.unwire(dir.path()).unwrap();
    assert_eq!(outcome.changed, EVENTS.len(), "{}", outcome.note);
    assert!(
        !path.exists(),
        "a file holding only base's hooks is deleted"
    );
    assert_eq!(
        BINDING.unwire(dir.path()).unwrap().changed,
        0,
        "unwire is idempotent"
    );
}

#[test]
fn unwire_on_a_missing_file_is_a_no_op() {
    let dir = tempfile::tempdir().unwrap();
    let _env = CopilotEnv::pointing_at(dir.path());

    let outcome = BINDING.unwire(dir.path()).unwrap();
    assert_eq!(outcome.changed, 0);
    assert_eq!(outcome.note, "not wired");
    assert!(
        !dir.path().join("hooks").exists(),
        "unwire must not create the directory"
    );
}

#[test]
fn unwire_keeps_another_tools_entries_and_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let _env = CopilotEnv::pointing_at(dir.path());
    let path = dir.path().join("hooks").join("base.json");
    let foreign_start = json!({
        "type": "command",
        "bash": "other-tool start",
        "powershell": "other-tool start",
        "timeoutSec": 5,
    });
    let foreign_notice = json!({ "type": "command", "command": "other-tool notify" });
    write_json(
        &path,
        &json!({
            "version": 1,
            "hooks": {
                "sessionStart": [foreign_start],
                "notification": [foreign_notice],
            }
        }),
    );

    let wired = BINDING.wire(dir.path(), Path::new("base")).unwrap();
    assert_eq!(wired.changed, EVENTS.len(), "{}", wired.note);

    let outcome = BINDING.unwire(dir.path()).unwrap();
    assert_eq!(outcome.changed, EVENTS.len(), "{}", outcome.note);
    assert!(path.exists(), "another tool's entries keep the file alive");

    let root = read_json(&path);
    let hooks = root["hooks"].as_object().unwrap();
    assert_eq!(
        hooks["sessionStart"],
        json!([foreign_start]),
        "the other tool's hook must survive the round trip"
    );
    assert_eq!(hooks["notification"], json!([foreign_notice]));
    assert!(
        !hooks.contains_key("agentStop"),
        "an array base emptied is dropped"
    );
    assert!(
        !serde_json::to_string(&root).unwrap().contains("base hook"),
        "no base entry may remain: {root}"
    );
}

#[test]
fn wire_refuses_a_file_it_cannot_merge_without_clobbering() {
    let dir = tempfile::tempdir().unwrap();
    let _env = CopilotEnv::pointing_at(dir.path());
    let path = dir.path().join("hooks").join("base.json");
    write_json(&path, &json!({ "version": 1, "hooks": [] }));
    let before = std::fs::read(&path).unwrap();

    let err = BINDING.wire(dir.path(), Path::new("base")).unwrap_err();
    assert!(format!("{err:#}").contains("non-object"), "{err:#}");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "a refused wire must not write"
    );
}

#[test]
fn inbound_maps_the_copilot_field_names_and_keeps_the_rest() {
    let raw = json!({
        "sessionId": "session-1",
        "toolName": "bash",
        "toolArgs": "{\"command\":\"ls -la\"}",
        "cwd": "/work/tree",
        "timestamp": "2026-10-30T09:00:00Z",
        "prompt": "carry on",
    });

    let got = inbound("pre-tool-use", raw);
    assert_eq!(got["session_id"], "session-1");
    assert_eq!(
        got["tool_name"], "Bash",
        "the engine's matchers compare against Claude's tool names literally"
    );
    assert_eq!(got["tool_input"]["command"], "ls -la");
    assert_eq!(
        got["cwd"], "/work/tree",
        "the workspace tier depends on cwd surviving"
    );
    assert_eq!(got["timestamp"], "2026-10-30T09:00:00Z");
    assert_eq!(got["prompt"], "carry on");
    assert_eq!(
        got["sessionId"], "session-1",
        "the host's own key is not destroyed"
    );
    assert_eq!(got["toolName"], "bash");
}

#[test]
fn inbound_accepts_an_object_tool_args_and_keeps_a_canonical_field_already_there() {
    let got = inbound(
        "pre-tool-use",
        json!({ "toolName": "edit", "toolArgs": { "path": "src/lib.rs" } }),
    );
    assert_eq!(got["tool_input"]["path"], "src/lib.rs");

    let got = inbound(
        "pre-tool-use",
        json!({ "toolArgs": { "path": "ignored" }, "tool_input": { "path": "kept" } }),
    );
    assert_eq!(
        got["tool_input"]["path"], "kept",
        "an adapter must not overwrite the engine's key"
    );
}

#[test]
fn inbound_passes_a_non_json_string_and_non_objects_through() {
    let got = inbound("pre-tool-use", json!({ "toolArgs": "ls -la" }));
    assert_eq!(got["tool_input"], "ls -la");

    assert_eq!(inbound("stop", json!("plain")), json!("plain"));
    assert_eq!(inbound("stop", json!([1, 2])), json!([1, 2]));
}

#[test]
fn outbound_emits_one_line_of_additional_context() {
    let printed = outbound("pre-tool-use", "line one\nline two").unwrap();
    assert_eq!(
        printed.lines().count(),
        1,
        "Copilot reads one line: {printed}"
    );
    let parsed: Value = serde_json::from_str(&printed).unwrap();
    assert_eq!(parsed["additionalContext"], "line one\nline two");
    assert_eq!(
        parsed.as_object().unwrap().len(),
        1,
        "only the field Copilot reads"
    );
}

#[test]
fn outbound_unwraps_the_engines_claude_envelope() {
    let envelope =
        r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":"ctx"}}"#;
    let parsed: Value = serde_json::from_str(&outbound("pre-tool-use", envelope).unwrap()).unwrap();
    assert_eq!(
        parsed["additionalContext"], "ctx",
        "the model must see context, not an envelope"
    );
}

#[test]
fn detection_honours_copilot_home() {
    let dir = tempfile::tempdir().unwrap();
    let _env = CopilotEnv::pointing_at(dir.path());
    let elsewhere = Path::new("/definitely/not/a/home");

    assert!(
        BINDING.detected(elsewhere),
        "the env var is the location, not the home argument"
    );
    assert_eq!(
        BINDING.config_path(elsewhere),
        dir.path().join("hooks").join("base.json")
    );
}

#[test]
fn detection_falls_back_to_the_home_directory() {
    let _env = CopilotEnv::cleared();
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join(".copilot").join("hooks").join("base.json");

    assert!(!BINDING.detected(home.path()));
    std::fs::create_dir_all(home.path().join(".copilot")).unwrap();
    assert!(BINDING.detected(home.path()));
    assert_eq!(BINDING.config_path(home.path()), config);
}

#[test]
fn the_config_path_is_the_one_the_file_lands_at() {
    let dir = tempfile::tempdir().unwrap();
    let _env = CopilotEnv::pointing_at(dir.path());
    let outcome = BINDING.wire(dir.path(), Path::new("base")).unwrap();
    let expected: PathBuf = dir.path().join("hooks").join("base.json");
    assert_eq!(outcome.path, expected);
    assert!(expected.exists());
}
