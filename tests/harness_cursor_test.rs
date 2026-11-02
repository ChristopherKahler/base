//! The Cursor binding: the merge into the operator's `~/.cursor/hooks.json`, the
//! exact inverse un-wiring, the double-injection guard, and the two payload
//! translations.
//!
//! No Cursor binary is needed, and none is exercised here: what is tested is the
//! part base owns — that wiring is idempotent, that un-wiring leaves another
//! tool's entries byte-for-byte in place (in fact byte-for-byte in the whole
//! file, round trip), that the guard keeps a second copy of every hook out of a
//! machine where Cursor already imports Claude Code's, and that a Cursor payload
//! reaches the engine where it looks.

use std::path::Path;

use base::harness::Binding;
use base::harness::cursor::{BINDING, inbound, outbound};
use serde_json::{Value, json};

/// The six events base wires, host name first.
const EVENTS: [(&str, &str); 6] = [
    ("sessionStart", "session-start"),
    ("beforeSubmitPrompt", "user-prompt-submit"),
    ("preToolUse", "pre-tool-use"),
    ("postToolUse", "post-tool-use"),
    ("stop", "stop"),
    ("sessionEnd", "session-end"),
];

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&read(path)).unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()))
}

/// A file as this machine actually has it: another tool's hooks under three of
/// the events, in an order and with a formatting of their own.
const EXISTING: &str = r#"{
  "version": 1,
  "hooks": {
    "preToolUse": [
      {
        "command": "rtk hook cursor",
        "matcher": "Shell"
      }
    ],
    "sessionStart": [
      {
        "command": "node \"/home/vkretz/Programming/ponytail/hooks/ponytail-activate.js\"",
        "timeout": 5
      }
    ],
    "beforeSubmitPrompt": [
      {
        "command": "node \"/home/vkretz/Programming/ponytail/hooks/ponytail-mode-tracker.js\"",
        "timeout": 5
      }
    ]
  }
}
"#;

fn write_hooks(home: &Path, text: &str) -> std::path::PathBuf {
    let dir = home.join(".cursor");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("hooks.json");
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn wiring_adds_one_entry_per_event_under_cursor_names() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".cursor")).unwrap();

    let outcome = BINDING.wire(home.path(), Path::new("/usr/local/bin/base")).unwrap();
    let path = home.path().join(".cursor").join("hooks.json");
    assert_eq!(outcome.path, path);
    assert_eq!(outcome.changed, EVENTS.len(), "{}", outcome.note);
    assert_eq!(
        outcome.note,
        "added sessionStart, beforeSubmitPrompt, preToolUse, postToolUse, stop, sessionEnd"
    );

    let root = read_json(&path);
    assert_eq!(root["version"], 1);
    let hooks = root["hooks"].as_object().expect("hooks is an object");
    assert_eq!(hooks.len(), EVENTS.len(), "exactly the six events base serves");

    for (host_event, event) in EVENTS {
        let entries = hooks[host_event]
            .as_array()
            .unwrap_or_else(|| panic!("{host_event} is not an array"));
        assert_eq!(entries.len(), 1, "{host_event}");
        assert_eq!(
            entries[0]["command"],
            format!("base hook {event} --harness cursor"),
            "{host_event}"
        );
        assert_eq!(entries[0]["timeout"], 10, "{host_event}");
    }
}

#[test]
fn wiring_twice_changes_nothing() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".cursor")).unwrap();

    let first = BINDING.wire(home.path(), Path::new("base")).unwrap();
    let bytes = std::fs::read(&first.path).unwrap();

    let second = BINDING.wire(home.path(), Path::new("base")).unwrap();
    assert_eq!(second.changed, 0);
    assert_eq!(second.note, "already wired");
    assert_eq!(
        std::fs::read(&second.path).unwrap(),
        bytes,
        "a second wire rewrote the file"
    );
}

#[test]
fn wire_then_unwire_is_byte_identical_and_keeps_the_other_tools() {
    let home = tempfile::tempdir().unwrap();
    let path = write_hooks(home.path(), EXISTING);

    let wired = BINDING.wire(home.path(), Path::new("base")).unwrap();
    assert_eq!(wired.changed, EVENTS.len(), "{}", wired.note);
    assert_ne!(read(&path), EXISTING, "wire changed nothing");

    let outcome = BINDING.unwire(home.path()).unwrap();
    assert_eq!(outcome.changed, EVENTS.len(), "{}", outcome.note);
    assert_eq!(
        read(&path),
        EXISTING,
        "unwire must restore the file byte for byte"
    );
}

#[test]
fn unwire_keeps_another_tools_entries_in_place() {
    let home = tempfile::tempdir().unwrap();
    let path = write_hooks(home.path(), EXISTING);
    BINDING.wire(home.path(), Path::new("base")).unwrap();
    BINDING.unwire(home.path()).unwrap();

    let root = read_json(&path);
    let hooks = root["hooks"].as_object().unwrap();
    assert_eq!(hooks["sessionStart"].as_array().unwrap().len(), 1);
    assert_eq!(hooks["sessionStart"][0]["timeout"], 5);
    assert_eq!(hooks["preToolUse"][0]["matcher"], "Shell");
    assert!(
        !read(&path).contains("base hook"),
        "no base entry may remain: {root}"
    );
}

#[test]
fn unwire_on_a_missing_file_is_a_no_op() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".cursor")).unwrap();

    let outcome = BINDING.unwire(home.path()).unwrap();
    assert_eq!(outcome.changed, 0);
    assert_eq!(outcome.note, "not wired");
}

#[test]
fn a_foreign_hook_beside_bases_is_left_alone() {
    let home = tempfile::tempdir().unwrap();
    let path = write_hooks(
        home.path(),
        "{\n  \"version\": 1,\n  \"hooks\": {\n    \"stop\": [ { \"command\": \"other-tool stop\" } ]\n  }\n}\n",
    );

    BINDING.wire(home.path(), Path::new("base")).unwrap();
    let outcome = BINDING.unwire(home.path()).unwrap();
    assert_eq!(outcome.changed, EVENTS.len(), "{}", outcome.note);

    let root = read_json(&path);
    assert_eq!(root["hooks"]["stop"], json!([{ "command": "other-tool stop" }]));
    assert!(!read(&path).contains("base hook"));
}

#[test]
fn wire_refuses_a_file_it_cannot_merge_without_clobbering() {
    let home = tempfile::tempdir().unwrap();
    let path = write_hooks(home.path(), "{\n  \"version\": 1,\n  \"hooks\": []\n}\n");
    let before = std::fs::read(&path).unwrap();

    let err = BINDING.wire(home.path(), Path::new("base")).unwrap_err();
    assert!(format!("{err:#}").contains("could not take base's hook entries"), "{err:#}");
    assert_eq!(std::fs::read(&path).unwrap(), before, "a refused wire must not write");
}

#[test]
fn native_wiring_is_skipped_when_cursor_already_imports_claude_hooks() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".cursor")).unwrap();
    let claude = home.path().join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(
        claude.join("settings.json"),
        "{\n  \"hooks\": {\n    \"PreToolUse\": [ { \"hooks\": [ { \"type\": \"command\", \"command\": \"base hook pre-tool-use\" } ] } ]\n  }\n}\n",
    )
    .unwrap();

    let outcome = BINDING.wire(home.path(), Path::new("base")).unwrap();
    assert_eq!(outcome.changed, 0);
    assert!(outcome.note.contains("imports"), "{}", outcome.note);
    assert!(
        !home.path().join(".cursor").join("hooks.json").exists(),
        "a second copy of every hook must not be written"
    );
}

#[test]
fn inbound_maps_the_cursor_tool_names_and_keeps_the_rest() {
    let raw = json!({
        "tool_name": "Shell",
        "tool_input": { "command": "ls -la" },
        "conversation_id": "conv-1",
        "cwd": "/work/tree",
        "hook_event_name": "preToolUse",
        "transcript_path": "/tmp/transcript",
    });

    let got = inbound("pre-tool-use", raw);
    assert_eq!(got["tool_name"], "Bash");
    assert_eq!(got["tool_input"]["command"], "ls -la");
    assert_eq!(got["session_id"], "conv-1", "the engine reads session_id");
    assert_eq!(got["conversation_id"], "conv-1", "the host's own key survives");
    assert_eq!(got["cwd"], "/work/tree", "the workspace tier depends on cwd surviving");
    assert_eq!(got["transcript_path"], "/tmp/transcript");
}

#[test]
fn inbound_maps_write_to_edit_and_leaves_an_unknown_tool_and_session_alone() {
    assert_eq!(inbound("pre-tool-use", json!({ "tool_name": "Write" }))["tool_name"], "Edit");
    assert_eq!(inbound("pre-tool-use", json!({ "tool_name": "Task" }))["tool_name"], "Task");

    // A `session_id` already present wins over `conversation_id`.
    let got = inbound(
        "session-start",
        json!({ "session_id": "keep", "conversation_id": "other" }),
    );
    assert_eq!(got["session_id"], "keep");
}

#[test]
fn inbound_passes_non_objects_through() {
    assert_eq!(inbound("stop", json!("plain")), json!("plain"));
    assert_eq!(inbound("stop", json!([1, 2])), json!([1, 2]));
}

#[test]
fn outbound_is_shaped_per_event_on_one_line() {
    let prompt = outbound("user-prompt-submit", "line one\nline two").unwrap();
    assert_eq!(prompt.lines().count(), 1, "{prompt}");
    assert_eq!(
        serde_json::from_str::<Value>(&prompt).unwrap(),
        json!({ "additional_context": "line one\nline two" })
    );

    let tool = outbound("post-tool-use", "ctx").unwrap();
    let parsed: Value = serde_json::from_str(&tool).unwrap();
    assert_eq!(parsed["hookSpecificOutput"]["hookEventName"], "PostToolUse");
    assert_eq!(parsed["hookSpecificOutput"]["additionalContext"], "ctx");
}

#[test]
fn detection_and_config_path_follow_the_cursor_directory() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join(".cursor").join("hooks.json");

    assert!(!BINDING.detected(home.path()));
    std::fs::create_dir_all(home.path().join(".cursor")).unwrap();
    assert!(BINDING.detected(home.path()));
    assert_eq!(BINDING.config_path(home.path()), config);
}
