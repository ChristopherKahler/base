//! RANK D: the wake script base ships must not lose pings.
//!
//! `relay::wake::watch_script` is described in its own doc comment as "the
//! single source of truth every session arms verbatim". Measured 2026-09-20, it
//! drops messages silently:
//!
//! ```text
//! for f in $(ls -1t "$INBOX" | head -5); do ... done
//! seen=$cur          # <- cur is the FULL listing, not the five announced
//! ```
//!
//! With six or more new pings it announces five and marks ALL of them consumed.
//! The remainder are never printed and never will be, because `cur` stops
//! changing so the loop never revisits them. `auk` observed 6 of 8 lost before
//! the mechanism was read.
//!
//! That is silent message loss inside the message-delivery system. Three
//! smaller faults ride along: `seen` starts empty so every re-arm re-announces
//! pings already consumed; `cut -c1-700` truncates with no marker, so a clipped
//! ping is indistinguishable from a short one; and the scan is not narrowed to
//! `ping-*.json`.
//!
//! The behavioural test RUNS the script. If `bash` is not on PATH it SKIPS
//! LOUDLY rather than passing — a test that quietly succeeds where it could not
//! execute is the inert-guard family this round is named after.

use std::io::Write;
use std::process::{Command, Stdio};

fn bash() -> Option<&'static str> {
    ["bash", "/usr/bin/bash", "/bin/bash"].into_iter().find(|candidate| {
        Command::new(candidate)
            .arg("-c")
            .arg("exit 0")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// Seed `n` pings, run the script for a couple of poll cycles, return stdout.
fn run_script(sh: &str, inbox: &std::path::Path, script: &str, n: usize) -> String {
    std::fs::create_dir_all(inbox).unwrap();
    for i in 0..n {
        let mut f = std::fs::File::create(inbox.join(format!("ping-{i:03}.json"))).unwrap();
        write!(
            f,
            r#"{{"from": "sender{i}", "summary": "message number {i}", "doc": null}}"#
        )
        .unwrap();
    }

    // Two poll cycles is enough: the loop sleeps 5s, so 12s covers it with room.
    let wrapped = format!("( {script} ) & pid=$!; sleep 12; kill $pid 2>/dev/null; wait $pid 2>/dev/null; exit 0");
    let out = Command::new(sh).arg("-c").arg(&wrapped).output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn the_shipped_wake_script_announces_every_ping_not_just_the_first_five() {
    let Some(sh) = bash() else {
        eprintln!("SKIPPED: bash not on PATH — this test cannot run here, and is not passing.");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let inbox = tmp.path().join("inbox");
    let script = base::relay::wake::watch_script_for(&inbox).unwrap();

    let stdout = run_script(sh, &inbox, &script, 8);

    let announced = stdout.matches("RELAY PING from").count();
    assert_eq!(
        announced, 8,
        "8 pings were waiting and {announced} were announced. The rest are marked \
         consumed and will never be printed.\nSTDOUT:\n{stdout}"
    );
    for i in 0..8 {
        assert!(
            stdout.contains(&format!("message number {i}")),
            "ping {i} never reached the reader:\n{stdout}"
        );
    }
}

#[test]
fn a_truncated_ping_says_that_it_was_truncated() {
    let Some(sh) = bash() else {
        eprintln!("SKIPPED: bash not on PATH — this test cannot run here, and is not passing.");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let inbox = tmp.path().join("inbox");
    std::fs::create_dir_all(&inbox).unwrap();
    let long = "x".repeat(2000);
    let mut f = std::fs::File::create(inbox.join("ping-long.json")).unwrap();
    write!(f, r#"{{"from": "sender", "summary": "{long}", "doc": null}}"#).unwrap();

    let script = base::relay::wake::watch_script_for(&inbox).unwrap();
    let wrapped =
        format!("( {script} ) & pid=$!; sleep 8; kill $pid 2>/dev/null; wait $pid 2>/dev/null; exit 0");
    let out = Command::new(sh).arg("-c").arg(&wrapped).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.to_uppercase().contains("TRUNCATED"),
        "a clipped ping must say so — otherwise it is indistinguishable from a \
         short one:\n{stdout}"
    );
}

/// Text properties, so the mechanism is pinned even where bash cannot run.
/// These are assertions about the SCRIPT SOURCE, stated as such.
#[test]
fn the_script_never_marks_unannounced_files_as_seen() {
    let tmp = tempfile::tempdir().unwrap();
    let script = base::relay::wake::watch_script_for(tmp.path()).unwrap();

    assert!(
        !script.contains("seen=$cur"),
        "`seen=$cur` marks the WHOLE listing consumed regardless of what was \
         announced. That is the drop.\n{script}"
    );
    assert!(
        !script.contains("head -5"),
        "capping the announce loop while marking everything seen is the drop.\n{script}"
    );
    assert!(
        script.contains("ping-*.json"),
        "the scan must be narrowed to ping files.\n{script}"
    );
}
