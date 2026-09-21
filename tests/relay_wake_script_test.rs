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

/// RANK E: a file that reads empty must NOT be marked consumed.
///
/// Found by auk experiencing it, not by reading for it: its waker printed
/// `RELAY PING from grebe:` with no body. The loop took one `ls` snapshot and
/// then read each file three times, and a REPLY CLEARS INBOUND PINGS — so a
/// file later in the snapshot can be deleted before it is read. Every read
/// returns empty, the header prints with nothing after it, and the file is
/// marked consumed.
///
/// THIS TEST DOES NOT CHASE THE RACE, because a test that has to win a race to
/// fail is a test that passes for the wrong reason. It uses the same code path
/// deterministically: an empty file IS an empty read. It seeds one good ping
/// and one empty file, lets a poll pass over both, then writes content into
/// the empty one and lets another poll run.
///
/// On the old behaviour the empty file is consumed on the first pass and its
/// content is NEVER announced. On the fix it is skipped without being marked,
/// and announced once it has something in it.
#[test]
fn a_file_that_reads_empty_is_not_consumed() {
    let Some(sh) = bash() else {
        eprintln!("SKIPPED: bash not on PATH — this test cannot run here, and is not passing.");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let inbox = tmp.path().join("inbox");
    std::fs::create_dir_all(&inbox).unwrap();

    // One real ping, and one file that is present but empty.
    let mut f = std::fs::File::create(inbox.join("ping-aaa.json")).unwrap();
    write!(f, r#"{{"from": "sender", "summary": "the good one", "doc": null}}"#).unwrap();
    std::fs::File::create(inbox.join("ping-bbb.json")).unwrap();

    let script = base::relay::wake::watch_script_for(&inbox).unwrap();
    let late = inbox.join("ping-bbb.json");
    let late_disp = late.to_string_lossy().replace('\\', "/");

    // Poll once over both, THEN fill the empty file, then poll again.
    let wrapped = format!(
        "( {script} ) & pid=$!; sleep 8;          printf '%s' '{{\"from\": \"sender\", \"summary\": \"the late one\", \"doc\": null}}' > '{late_disp}';          sleep 8; kill $pid 2>/dev/null; wait $pid 2>/dev/null; exit 0"
    );
    let out = Command::new(sh).arg("-c").arg(&wrapped).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("the good one"),
        "the readable ping never announced at all:\n{stdout}"
    );
    assert!(
        stdout.contains("the late one"),
        "a file that read EMPTY was marked consumed, so its content was never \
         announced once it arrived. That is the rank E loss.\n{stdout}"
    );
}

/// Negative control for RANK E: an empty file never announces a PING. Skipping
/// without consuming must not become announcing a blank line — the defect it
/// replaces was a header with no body.
///
/// NARROWED 2026-09-20, because the old wording would now be false. It read
/// "announces NOTHING while it is empty", true when an empty read was silent. The
/// empty branch now prints one `RELAY EMPTY READ:` line. This test guards what it
/// always actually guarded — that no `RELAY PING from` header is emitted with no
/// body — and the assertion is unchanged because it was already keyed on that string.
#[test]
fn an_empty_file_announces_nothing_while_it_is_empty() {
    let Some(sh) = bash() else {
        eprintln!("SKIPPED: bash not on PATH — this test cannot run here, and is not passing.");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let inbox = tmp.path().join("inbox");
    std::fs::create_dir_all(&inbox).unwrap();
    std::fs::File::create(inbox.join("ping-empty.json")).unwrap();

    let script = base::relay::wake::watch_script_for(&inbox).unwrap();
    let wrapped =
        format!("( {script} ) & pid=$!; sleep 8; kill $pid 2>/dev/null; wait $pid 2>/dev/null; exit 0");
    let out = Command::new(sh).arg("-c").arg(&wrapped).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert_eq!(
        stdout.matches("RELAY PING from").count(),
        0,
        "an empty file must announce nothing, not an empty header:\n{stdout}"
    );
}

/// The honest half of RANK E (2026-09-20, raised by `plover`). An empty read used to
/// print nothing at all, so a ping that vanished under the read and an inbox that was
/// simply quiet reached the reader identically.
///
/// TWO ASSERTIONS, AND THE SECOND HAS THE TEETH. That the line appears is the easy
/// half. That it appears EXACTLY ONCE over a run spanning several polls is what proves
/// `reported` works — without it the line repeats every five seconds for as long as the
/// file sits there, which is its own kind of unreadable.
#[test]
fn an_empty_file_says_it_could_not_be_read_exactly_once() {
    let Some(sh) = bash() else {
        eprintln!("SKIPPED: bash not on PATH — this test cannot run here, and is not passing.");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let inbox = tmp.path().join("inbox");
    std::fs::create_dir_all(&inbox).unwrap();
    std::fs::File::create(inbox.join("ping-empty.json")).unwrap();

    let script = base::relay::wake::watch_script_for(&inbox).unwrap();
    // 13s spans at least two 5s polls, so a line repeating per poll would show up twice.
    let wrapped =
        format!("( {script} ) & pid=$!; sleep 13; kill $pid 2>/dev/null; wait $pid 2>/dev/null; exit 0");
    let out = Command::new(sh).arg("-c").arg(&wrapped).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert_eq!(
        stdout.matches("RELAY EMPTY READ:").count(),
        1,
        "an unreadable ping must say so exactly once over several polls:
{stdout}"
    );
    assert!(
        stdout.contains("cannot tell which"),
        "the line must name both states it cannot separate:
{stdout}"
    );
    assert!(
        stdout.contains("ping-empty.json"),
        "the line must name the file, or nobody can go and look:
{stdout}"
    );
}

// ─── The sentinel fingerprint: does a wake fix REACH a running session? ──────
//
// The defect (grebe, verified by auk): the arming block is re-emitted only when the
// sentinel goes STALE, and a live monitor touches it every 5s. So a session already
// running the OLD script never went stale and was never shown the NEW one. Every wake
// fix was undeliverable to exactly the sessions that needed it.
//
// Legs 1-5 test the GATE. Leg 6 tests the SCRIPT, and it is the one with teeth: a gate
// that reads a field nothing writes is green forever.

/// LEG 6 FIRST, because the others are worthless without it. Run the emitted script
/// under real bash and read the sentinel file back off disk. This is the only leg that
/// proves the script holds up its half of the contract.
#[test]
fn the_emitted_script_writes_the_fingerprint_and_title_into_the_sentinel() {
    let Some(sh) = bash() else {
        eprintln!("SKIPPED: bash not on PATH — this test cannot run here, and is not passing.");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let inbox = tmp.path().join("kestrel");
    std::fs::create_dir_all(&inbox).unwrap();

    let script = base::relay::wake::watch_script_for(&inbox).unwrap();
    let wrapped =
        format!("( {script} ) & pid=$!; sleep 3; kill $pid 2>/dev/null; wait $pid 2>/dev/null; exit 0");
    Command::new(sh).arg("-c").arg(&wrapped).output().unwrap();

    let body = std::fs::read_to_string(inbox.join(".watching"))
        .expect("the loop must create the sentinel");
    assert_eq!(
        body.trim(),
        base::relay::wake::template_fingerprint(),
        "the sentinel must hold the template fingerprint and nothing else, got: {body:?}"
    );
}

/// The fingerprint is a property of the TEMPLATE, not of the rendered text. Two
/// different inboxes must stamp the SAME fingerprint — if it moved with the path, the
/// check would depend on path spelling and could go permanently red.
#[test]
fn the_fingerprint_does_not_move_with_the_inbox_path() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("aaa");
    let b = tmp.path().join("bbb");
    let sa = base::relay::wake::watch_script_for(&a).unwrap();
    let sb = base::relay::wake::watch_script_for(&b).unwrap();
    let fp = base::relay::wake::template_fingerprint();

    assert!(sa.contains(&fp) && sb.contains(&fp), "both scripts carry the same fingerprint");
    assert_ne!(sa, sb, "but the scripts differ, so this is not comparing a constant to itself");
    assert!(sa.contains("aaa") && sb.contains("bbb"), "each carries its own title");
}

/// The fingerprint must be 16 hex characters and stable across calls. Without the
/// stability half, a function returning a fresh value each call would make every
/// sentinel read as Outdated forever — permanently red, which is the failure auk ruled
/// against.
#[test]
fn the_fingerprint_is_stable_across_calls() {
    let a = base::relay::wake::template_fingerprint();
    let b = base::relay::wake::template_fingerprint();
    assert_eq!(a, b, "the same template must hash the same way twice");
    assert_eq!(a.len(), 16, "expected 16 hex characters, got {a:?}");
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()), "not hex: {a:?}");
}
