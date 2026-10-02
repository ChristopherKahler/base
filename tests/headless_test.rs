//! BO-08 (F27): base's own headless Claude calls run with base's hooks off.
//!
//! `base graph extract` and the other LLM work run headless Claude Code (`claude -p`), and Claude Code runs the user's
//! hooks inside that session: base's own session start, prompt, tool and stop hooks fired inside base's own call. They
//! could take a relay codename (the title the calling session holds, which the child inherits as `BASE_RELAY_AS`, or a
//! Windows Terminal tab's title through `WT_SESSION`: BO-05), write hook log rows and session files, and add base's
//! context to the extraction prompt. Every headless call now carries `BASE_HEADLESS=1`, and every hook returns at once
//! under it.
//!
//! These tests drive the real binary with the environment a headless child inherits.

mod seed;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use seed::{run_base_in_session, run_hook};

const BIN: &str = env!("CARGO_BIN_EXE_base");

/// The five hooks base wires (`install::HOOK_TABLE`), each with the payload Claude Code sends it.
fn events(ws: &Path, session: &str) -> Vec<(&'static str, serde_json::Value)> {
    let cwd = ws.display().to_string();
    let file = ws.join("src").join("lib.rs").display().to_string();
    vec![
        (
            "session-start",
            serde_json::json!({ "cwd": cwd, "hook_event_name": "SessionStart", "source": "startup", "session_id": session }),
        ),
        (
            "user-prompt-submit",
            serde_json::json!({
                "cwd": cwd,
                "hook_event_name": "UserPromptSubmit",
                "prompt": "Extract the concepts and relationships from this document about base relay and graph rules.",
                "session_id": session,
            }),
        ),
        (
            "pre-tool-use",
            serde_json::json!({
                "cwd": cwd,
                "hook_event_name": "PreToolUse",
                "tool_name": "Read",
                "tool_input": { "file_path": file },
                "session_id": session,
            }),
        ),
        (
            "post-tool-use",
            serde_json::json!({
                "cwd": cwd,
                "hook_event_name": "PostToolUse",
                "tool_name": "Read",
                "tool_input": { "file_path": file, "offset": 1, "limit": 5 },
                "tool_response": {},
                "session_id": session,
            }),
        ),
        (
            "stop",
            serde_json::json!({ "cwd": cwd, "hook_event_name": "Stop", "stop_hook_active": false, "session_id": session }),
        ),
    ]
}

/// The rows of one of the workspace's hook logs: `hook-events.jsonl` (one row per hook run, every hook) or
/// `hook-output.jsonl` (what session start and the prompt hook printed).
fn rows(ws: &Path, log: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(ws.join(".base").join(log))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn hooks_in(rows: &[serde_json::Value]) -> Vec<&str> {
    rows.iter().filter_map(|r| r["hook"].as_str()).collect()
}

fn seed_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("base-headless-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    root
}

/// F27b: under `BASE_HEADLESS` each of the five hooks exits 0 with nothing on stdout or stderr and writes nothing:
/// no hook log row, no session file, no relay registration, even with the calling session's relay title and terminal
/// tab in its environment. The control runs the same five hooks without the marker and shows each one writes a log row
/// and changes the store, so the silence above is the marker's doing and not a probe that could not see a write.
#[test]
fn hooks_exit_silently_under_marker() {
    let root = seed_root("silent");
    let s = seed::write(&root, &seed::TINY, "");
    std::fs::create_dir_all(s.ws.join("src")).expect("src");
    std::fs::write(s.ws.join("src").join("lib.rs"), "pub fn f() {}\n").expect("lib.rs");

    // The session that runs `base graph extract` holds a relay title; its headless child inherits that title and tab.
    let (code, _, err) = run_base_in_session(&s, &["relay", "register", "--as", "parent-kite"], "parent-session");
    assert_eq!(code, 0, "the parent registers: {err}");
    let inherited: &[(&str, &str)] =
        &[("BASE_RELAY_AS", "parent-kite"), ("WT_SESSION", "0f1e2d3c-tab"), ("CLAUDE_CODE_SESSION_ID", "headless-child")];

    let before = seed::files_under(&root);
    let (events_before, output_before) = (rows(&s.ws, "hook-events.jsonl").len(), rows(&s.ws, "hook-output.jsonl").len());
    for (event, payload) in events(&s.ws, "headless-child") {
        let mut env = inherited.to_vec();
        env.push(("BASE_HEADLESS", "1"));
        let (code, stdout, stderr) = run_hook(&s, event, &payload, &env);
        assert_eq!(code, 0, "{event} under the marker exited {code}: {stderr}");
        assert_eq!(stdout, "", "{event} under the marker printed to stdout");
        assert_eq!(stderr, "", "{event} under the marker printed to stderr");
    }
    let after = seed::files_under(&root);
    let changed: Vec<_> = after
        .iter()
        .filter(|(p, v)| before.get(*p) != Some(*v))
        .map(|(p, _)| p.display().to_string())
        .chain(before.keys().filter(|p| !after.contains_key(*p)).map(|p| format!("{} (removed)", p.display())))
        .collect();
    assert!(changed.is_empty(), "hooks under the marker wrote: {changed:?}");
    assert_eq!(rows(&s.ws, "hook-events.jsonl").len(), events_before, "no hook-events row under the marker");
    assert_eq!(rows(&s.ws, "hook-output.jsonl").len(), output_before, "no hook-output row under the marker");

    // Control: the same five hooks, same environment, with BASE_HEADLESS=0, which is not the marker (only 1 is).
    for (event, payload) in events(&s.ws, "headless-child") {
        let mut env = inherited.to_vec();
        env.push(("BASE_HEADLESS", "0"));
        let (code, _, stderr) = run_hook(&s, event, &payload, &env);
        assert_eq!(code, 0, "{event} without the marker exited {code}: {stderr}");
    }
    let events = rows(&s.ws, "hook-events.jsonl");
    assert_eq!(
        hooks_in(&events[events_before..]),
        ["session-start", "user-prompt-submit", "pre-tool-use", "post-tool-use", "stop"],
        "with BASE_HEADLESS=0 every hook logs a hook-events row, so the check above could see one"
    );
    let output = rows(&s.ws, "hook-output.jsonl");
    assert_eq!(
        hooks_in(&output[output_before..]),
        ["session-start", "user-prompt-submit"],
        "without the marker session start and the prompt hook keep their output records"
    );
    assert_ne!(seed::files_under(&root), after, "without the marker the hooks write to the store");
    let _ = std::fs::remove_dir_all(&root);
}

/// `base doctor --measure` (BO-02) runs its probes as headless calls too, so they carry the marker, and the hook those
/// calls register is `base doctor --measure emit`, not `base hook`. It must still print its payload under the marker,
/// or every measurement would read zero.
#[test]
fn measure_emit_still_prints_under_the_marker() {
    let home = tempfile::tempdir().expect("tempdir");
    let out = Command::new(BIN)
        .args(["doctor", "--measure", "emit", "--hook", "session-start", "--bytes", "700", "--nonce", "hx7k"])
        .env("BASE_HOME", home.path())
        .env("BASE_NO_AUTO_UPDATE", "1")
        .env("BASE_HEADLESS", "1")
        .stdin(Stdio::null())
        .output()
        .expect("base runs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.stdout.len(), 700, "the measure payload is exactly its size under the marker");
}

/// F27a end to end: `base graph extract` starts `claude` with `BASE_HEADLESS=1` in its environment. A stand-in
/// `claude` on `PATH` records what it was given and answers with an empty extraction.
///
/// Unix only: on Windows `Command` finds `claude` only as `claude.exe`, and a stand-in would have to be a compiled
/// program. The builder this goes through is checked on every platform by `llm::tests::headless_calls_set_marker`.
#[test]
fn graph_extract_starts_claude_with_the_marker() {
    if cfg!(windows) {
        eprintln!("graph_extract_starts_claude_with_the_marker: Unix only by design (see the doc comment); returns here.");
        return;
    }
    let root = seed_root("extract");
    let s = seed::write(&root, &seed::TINY, "");
    let docs = s.ws.join("docs");
    std::fs::create_dir_all(&docs).expect("docs");
    std::fs::write(docs.join("one.md"), "# Relay\n\nA relay title names one session.\n").expect("one.md");
    std::fs::write(docs.join("two.md"), "# Graph\n\nThe graph holds notes and rules.\n").expect("two.md");

    let bin = root.join("fake-bin");
    std::fs::create_dir_all(&bin).expect("fake-bin");
    let log = root.join("claude-env.log");
    let fake = bin.join("claude");
    std::fs::write(
        &fake,
        "#!/bin/sh\nprintf '%s\\n' \"${BASE_HEADLESS-unset}\" >> \"$FAKE_CLAUDE_LOG\"\necho '{\"concepts\":[],\"edges\":[]}'\n",
    )
    .expect("fake claude");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let path = std::env::join_paths(
        std::iter::once(bin.clone()).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())),
    )
    .expect("PATH");

    let out = Command::new(BIN)
        .args(["graph", "extract", "--target", "docs"])
        .current_dir(&s.ws)
        .env("PATH", path)
        .env("FAKE_CLAUDE_LOG", &log)
        .env("BASE_HOME", &s.home)
        .env("BASE_NO_AUTO_UPDATE", "1")
        .env("BASE_AST_NO_SPAWN", "1")
        .env_remove("BASE_HEADLESS")
        .stdin(Stdio::null())
        .output()
        .expect("base runs");
    let calls = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(
        calls.lines().collect::<Vec<_>>(),
        ["1", "1"],
        "one call per document, each with BASE_HEADLESS=1; base said: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(&root);
}
