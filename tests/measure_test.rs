//! BO-02 (F6): `base doctor --measure emit`, the hook every measure call registers, driven through the real binary.
//!
//! REAL BINARY, because the emitter's stdout IS the measurement: a byte printed before or after the payload, by a
//! banner, a notice or the hook-wiring repair `cli::run` does before dispatch, changes what is being counted. Only the
//! process's own stdout can prove there is none.
//!
//! ISOLATION. `BASE_HOME` points at a tempdir that holds `.claude/` and `.base-gbl/`, which is exactly the home where
//! `ensure_hooks_wired` would create and wire `.claude/settings.json`. The emitter must leave it untouched.

use std::path::Path;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_base");

fn emit(home: &Path, hook: &str, bytes: usize, nonce: &str) -> std::process::Output {
    Command::new(BIN)
        .args(["doctor", "--measure", "emit", "--hook", hook, "--bytes", &bytes.to_string(), "--nonce", nonce])
        .env("BASE_HOME", home)
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .current_dir(home)
        .output()
        .expect("run base")
}

fn sandbox() -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(home.path().join(".claude")).expect("mkdir .claude");
    std::fs::create_dir_all(home.path().join(".base-gbl")).expect("mkdir .base-gbl");
    home
}

/// The payload a measure hook prints is exactly the size asked for, with the nonce on every line, a BEGIN line, marker
/// lines numbered from 0001 each starting a multiple of 100 bytes in, and an END line. Session start and prompt submit
/// print it as plain stdout; pre-tool prints it as the `additionalContext` of the JSON envelope, the only pre-tool
/// channel that reaches the model.
#[test]
fn measure_payload_is_exact_size_and_marked() {
    let home = sandbox();
    for bytes in [500, 4000, 8192, 10_001, 64_000] {
        let out = emit(home.path(), "user-prompt-submit", bytes, "q7k2");
        assert!(out.status.success(), "rc {:?}: {}", out.status.code(), String::from_utf8_lossy(&out.stderr));
        assert_eq!(out.stdout.len(), bytes, "stdout is exactly the payload, nothing before or after it");
        let text = String::from_utf8(out.stdout).expect("ascii");
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        assert!(lines[0].starts_with(&format!("MEASURE q7k2 BEGIN {bytes} ")), "{:?}", lines[0]);
        assert_eq!(lines.last().copied(), Some(format!("MEASURE q7k2 END {bytes}\n").as_str()));
        let mut offset = lines[0].len();
        assert_eq!(offset, 100, "the BEGIN line is 100 bytes, so marker k starts at byte 100k");
        let mut k = 0;
        for line in &lines[1..lines.len() - 1] {
            if !line.starts_with("MEASURE") {
                assert!(line.trim_end().chars().all(|c| c == '.'), "only the remainder line is unmarked: {line:?}");
                continue;
            }
            k += 1;
            assert_eq!(*line, &format!("MEASURE q7k2 {k:04} {}\n", ".".repeat(99 - 18)), "marker {k}");
            assert_eq!(offset, 100 * k, "marker {k} starts at byte {}", 100 * k);
            offset += line.len();
        }
        assert_eq!(k, base::measure::markers_in(bytes, "q7k2"), "{bytes} bytes");
        if bytes == 8192 {
            assert_eq!(k, 80, "Example 1: 8,192 bytes carry markers 0001 to 0080");
        }
    }

    for hook in ["session-start", "user-prompt-submit"] {
        let out = emit(home.path(), hook, 4000, "a1b2");
        assert_eq!(out.stdout, base::measure::payload(4000, "a1b2").expect("payload").into_bytes(), "{hook}");
    }
    let out = emit(home.path(), "pre-tool-use", 4000, "w3p9");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("one JSON envelope");
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    let ctx = v["hookSpecificOutput"]["additionalContext"].as_str().expect("additionalContext");
    assert_eq!(ctx.len(), 4000);
    assert_eq!(ctx, base::measure::payload(4000, "w3p9").expect("payload"));

    // Nothing else ran: the hook-wiring repair that every other command does first would have created this file.
    assert!(!home.path().join(".claude").join("settings.json").exists(), "the emitter wired hooks");
}

/// A bad call is refused with a reason on stderr and nothing on stdout, so it can never be read as a payload.
#[test]
fn measure_emit_refuses_a_bad_call_with_nothing_on_stdout() {
    let home = sandbox();
    for (hook, bytes, nonce) in [("bogus", 4000, "q7k2"), ("pre-tool-use", 20, "q7k2"), ("pre-tool-use", 4000, "1234")] {
        let out = emit(home.path(), hook, bytes, nonce);
        assert_eq!(out.status.code(), Some(2), "{hook} {bytes} {nonce}");
        assert!(out.stdout.is_empty(), "{hook} {bytes} {nonce}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("base doctor --measure emit:"));
    }
    let out = Command::new(BIN)
        .args(["doctor", "--measure", "emit", "--hook", "pre-tool-use"])
        .env("BASE_HOME", home.path())
        .current_dir(home.path())
        .output()
        .expect("run base");
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
}

/// THE REAL CALL, run by hand: needs Claude Code on PATH and logged in, and spends one Haiku call.
/// `cargo test --test measure_test -- --ignored`. A 4,000-byte prompt-submit payload arrives whole on Claude Code
/// 2.1.287 (the delivery limit measured there is well above it), so the reply reads as every marker plus END.
#[test]
#[ignore = "needs a real Claude Code and spends one Haiku call; run by hand"]
fn measure_real_claude_code_delivers_a_small_payload() {
    use base::measure::{judge, parse_answer, ClaudeRunner, Hook, Runner, Verdict};
    let mut runner = ClaudeRunner::new(BIN.into()).expect("scratch dir");
    let nonce = base::measure::nonce();
    let reply = runner.ask(Hook::UserPromptSubmit, 4000, &nonce).expect("claude -p answered");
    assert_eq!(judge(&parse_answer(&reply), 4000, &nonce), Verdict::Whole, "reply {reply:?}");
}
