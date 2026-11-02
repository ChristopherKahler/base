//! The OpenCode binding: the plugin file it writes, the file it refuses to
//! touch, and the two payload translations.
//!
//! No OpenCode session is driven here — the plugin is JavaScript that the host
//! loads, and there is no host in a `cargo test` process. What these tests do
//! exercise is the part base owns: that wiring is a single idempotent file
//! write, that a user's own `base.ts` is never overwritten or deleted, and that
//! the generated plugin is the transport base documented (`--harness opencode`,
//! no shell interpolation, `BASE_HEADLESS` honoured).

use std::path::Path;

use base::harness::Binding;
use base::harness::opencode::{BINDING, MARKER, inbound, outbound};
use serde_json::{Value, json};

/// An absolute `exe`, which is what an install embeds. A relative one falls back
/// to `base` on PATH and is covered by the module's own unit tests.
const EXE: &str = "/usr/local/bin/base";

fn plugin_path(home: &Path) -> std::path::PathBuf {
    home.join(".config").join("opencode").join("plugins").join("base.ts")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

#[test]
fn wiring_writes_the_plugin_and_names_its_transport() {
    let dir = tempfile::tempdir().unwrap();
    let outcome = BINDING.wire(dir.path(), Path::new(EXE)).unwrap();

    let path = plugin_path(dir.path());
    assert_eq!(outcome.path, path);
    assert_eq!(outcome.changed, 1, "{}", outcome.note);
    assert!(path.exists());

    let source = read(&path);
    assert!(source.starts_with(MARKER), "the marker is the first line");
    for needle in [
        "Plugin.define",
        "execFile",
        "BASE_HEADLESS",
        "\"hook\", event, \"--harness\", \"opencode\"",
        "\"session-start\"",
        "\"user-prompt-submit\"",
        "\"pre-tool-use\"",
        "\"post-tool-use\"",
        "\"stop\"",
        "\"session-end\"",
    ] {
        assert!(source.contains(needle), "the plugin lost {needle}");
    }
    assert!(
        source.contains(EXE),
        "the plugin must be able to reach the binary that wrote it"
    );
    assert!(
        !source.contains("$("),
        "Electron has no Bun `$`; the plugin must spawn through execFile"
    );
}

#[test]
fn wiring_creates_the_plugin_directory_on_a_forced_install() {
    // `--harness opencode` on a machine where OpenCode has never run: the
    // operator named this host, so refusing to create the path would make the
    // command a silent no-op.
    let dir = tempfile::tempdir().unwrap();
    assert!(!dir.path().join(".config").exists());

    BINDING.wire(dir.path(), Path::new(EXE)).unwrap();

    assert!(plugin_path(dir.path()).exists());
}

#[test]
fn wiring_twice_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let first = BINDING.wire(dir.path(), Path::new(EXE)).unwrap();
    let bytes = std::fs::read(&first.path).unwrap();

    let second = BINDING.wire(dir.path(), Path::new(EXE)).unwrap();
    assert_eq!(second.changed, 0);
    assert_eq!(second.note, "already wired");
    assert_eq!(
        std::fs::read(&second.path).unwrap(),
        bytes,
        "a second wire rewrote the plugin"
    );
}

#[test]
fn wiring_refuses_a_plugin_it_did_not_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = plugin_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mine = "export default { id: \"mine\" }\n";
    std::fs::write(&path, mine).unwrap();

    let err = BINDING.wire(dir.path(), Path::new(EXE)).unwrap_err();
    assert!(format!("{err:#}").contains("refusing to overwrite"), "{err:#}");
    assert_eq!(
        read(&path),
        mine,
        "someone else's plugin must survive a failed wire byte for byte"
    );
}

#[test]
fn a_plugin_base_wrote_is_rewritten_when_the_binary_moves() {
    let dir = tempfile::tempdir().unwrap();
    BINDING.wire(dir.path(), Path::new("/old/path/base")).unwrap();

    let outcome = BINDING.wire(dir.path(), Path::new("/new/path/base")).unwrap();
    assert_eq!(outcome.changed, 1, "{}", outcome.note);
    let source = read(&outcome.path);
    assert!(source.contains("/new/path/base"), "the binary path was not updated");
    assert!(!source.contains("/old/path/base"));
}

#[test]
fn unwiring_removes_base_plugin_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    BINDING.wire(dir.path(), Path::new(EXE)).unwrap();
    let path = BINDING.config_path(dir.path());

    let outcome = BINDING.unwire(dir.path()).unwrap();
    assert_eq!(outcome.changed, 1, "{}", outcome.note);
    assert!(!path.exists(), "base's plugin is base's to delete");

    let again = BINDING.unwire(dir.path()).unwrap();
    assert_eq!(again.changed, 0);
    assert!(
        !dir.path().join(".config").join("opencode").join("plugins").exists()
            || dir
                .path()
                .join(".config")
                .join("opencode")
                .join("plugins")
                .read_dir()
                .unwrap()
                .next()
                .is_none(),
        "unwire leaves no empty file behind"
    );
}

#[test]
fn unwiring_a_missing_file_is_a_no_op() {
    let dir = tempfile::tempdir().unwrap();
    let outcome = BINDING.unwire(dir.path()).unwrap();
    assert_eq!(outcome.changed, 0);
    assert_eq!(outcome.note, "not wired");
    assert!(
        !dir.path().join(".config").exists(),
        "unwire must not create the directory it looked in"
    );
}

#[test]
fn unwiring_leaves_a_plugin_base_did_not_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = plugin_path(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mine = "export default { id: \"mine\" }\n";
    std::fs::write(&path, mine).unwrap();

    let outcome = BINDING.unwire(dir.path()).unwrap();
    assert_eq!(outcome.changed, 0);
    assert!(outcome.note.contains("not written by base"), "{}", outcome.note);
    assert_eq!(read(&path), mine);
}

#[test]
fn detection_counts_the_config_directory() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!BINDING.detected(dir.path()));

    std::fs::create_dir_all(dir.path().join(".config").join("opencode")).unwrap();
    assert!(BINDING.detected(dir.path()));
    assert_eq!(BINDING.config_path(dir.path()), plugin_path(dir.path()));
}

#[test]
fn inbound_passes_the_engine_shape_through() {
    // The plugin normalizes on the JavaScript side, so the payload that reaches
    // `base hook` is already the engine's dialect.
    let raw = json!({
        "cwd": "/work/tree",
        "session_id": "s1",
        "tool_name": "Bash",
        "tool_input": { "command": "ls" },
    });
    assert_eq!(inbound(raw.clone()), raw);
}

#[test]
fn outbound_emits_one_line_of_additional_context() {
    let printed = outbound("session-start", "line one\nline two").unwrap();
    assert_eq!(printed.lines().count(), 1, "OpenCode parses one line: {printed}");
    let parsed: Value = serde_json::from_str(&printed).unwrap();
    assert_eq!(parsed["additionalContext"], "line one\nline two");
    assert_eq!(parsed.as_object().unwrap().len(), 1);
}

#[test]
fn outbound_unwraps_the_engines_claude_envelope() {
    let envelope =
        r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":"ctx"}}"#;
    let parsed: Value = serde_json::from_str(&outbound("pre-tool-use", envelope).unwrap()).unwrap();
    assert_eq!(
        parsed["additionalContext"], "ctx",
        "the model must see context, never JSON inside JSON"
    );
}

#[test]
fn outbound_prints_nothing_for_an_empty_emission() {
    assert_eq!(outbound("stop", ""), None);
    assert_eq!(outbound("stop", "   \n"), None);
    assert_eq!(outbound("stop", r#"{"additionalContext":"  "}"#), None);
}
