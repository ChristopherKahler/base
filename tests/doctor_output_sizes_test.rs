//! Rank 10 (lane doc B30): `base doctor` reports what base emitted, from the binary `cargo test` builds.
//!
//! Driven the way Claude Code and an operator drive it: session start with JSON on stdin, then `base doctor`, both
//! with `BASE_HOME` at a seeded fake home so base's write tripwire stays armed.
//!
//! This file uses no API commit F adds, on purpose: it compiles against the head before commit F, so every test here
//! runs red there first. Each test asserts its controls before its claim, and a control that fails says `control:`,
//! so a run that could not measure is never read as a red.
//!
//! The size doctor prints is checked against `units(stdout)` of the session start itself, the channel Claude Code
//! reads (law 42), not against the record doctor read it from.

mod seed;

use seed::{measured, run_base, run_session_start, units};

/// Session start over `seed`, with its controls: hooks fail open, so it exits 0, and it printed something.
fn start(seed: &seed::Seed, session: &str) -> String {
    let (code, stdout, stderr) = run_session_start(seed, Some(session));
    assert_eq!(
        code, 0,
        "control: hooks fail open, so session start exits 0. stderr: {stderr}"
    );
    assert!(
        !stdout.is_empty(),
        "control: session start printed nothing, so this measures nothing. stderr: {stderr}"
    );
    stdout
}

/// `base doctor` over `seed`, with its control: its own header is present, so it ran. Its exit code is not asserted:
/// doctor exits non-zero when unhealthy, and health is not what these tests are about.
fn doctor(seed: &seed::Seed) -> String {
    let (code, stdout, stderr) = run_base(seed, &["doctor"]);
    assert!(
        stdout.contains("base doctor — graph health"),
        "control: doctor did not run. rc={code}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    stdout
}

/// The workspace tier's session-start line that starts with `what`.
fn size_line<'a>(report: &'a str, what: &str) -> Option<&'a str> {
    let key = format!("workspace tier · session-start: {what}");
    report.lines().find(|line| line.contains(&key))
}

/// `N` and `B` from a line reading `... N of B units ...`.
fn of_units(line: &str) -> (usize, usize) {
    let head = line.split(" units").next().unwrap_or("");
    let words: Vec<&str> = head.split_whitespace().collect();
    let n = words.len();
    assert!(
        n >= 3 && words[n - 2] == "of",
        "not an `N of B units` line: {line}"
    );
    let number = |w: &str| {
        w.parse::<usize>()
            .unwrap_or_else(|_| panic!("`{w}` is not a number in: {line}"))
    };
    (number(words[n - 3]), number(words[n - 1]))
}

/// D1. On the real-size seed the size doctor reports for the last session start is the size Claude Code received,
/// against the budget in force, and its trim clause names exactly the record's trim rows.
#[test]
fn doctor_reports_the_last_session_start_at_the_size_claude_code_received() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let seed = seed::write(
        tmp.path(),
        &seed::REAL,
        "[budget]\nsession_start_chars = 9000\n",
    );
    let stdout = start(&seed, "d1-session");
    let base = seed.ws.join(".base");
    let full = std::fs::read_to_string(base.join("last-session-start.md"))
        .unwrap_or_else(|e| panic!("control: session start wrote no last-session-start.md: {e}"));
    println!(
        "stdout {} · full file {}",
        measured(&stdout),
        measured(&full)
    );
    assert!(
        units(&full) > units(&stdout),
        "control: the untrimmed file is not larger than stdout, so this run trimmed nothing"
    );

    let log = std::fs::read_to_string(base.join("hook-output.jsonl"))
        .unwrap_or_else(|e| panic!("no hook-output.jsonl beside last-session-start.md: {e}"));
    let records: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        records.len(),
        1,
        "hook-output.jsonl holds {} records after one session start:\n{log}",
        records.len()
    );
    let record: serde_json::Value = serde_json::from_str(records[0]).expect("the record is JSON");

    let report = doctor(&seed);
    let line = size_line(&report, "last run ")
        .unwrap_or_else(|| panic!("no session-start size line in doctor's output:\n{report}"));
    let (n, budget) = of_units(line);
    assert_eq!(
        n,
        units(&stdout),
        "doctor's last run is not the size session start printed: {line}"
    );
    assert_eq!(
        budget, 9000,
        "doctor's last run is not against the configured budget: {line}"
    );

    let trims: Vec<String> = record["withheld"]
        .as_array()
        .expect("the record carries withheld rows")
        .iter()
        .filter(|w| {
            matches!(
                w["reason"].as_str(),
                Some("collapsed" | "list cut" | "shortened")
            )
        })
        .map(|w| {
            format!(
                "{} {} {}",
                w["block"].as_str().unwrap_or("?"),
                w["items"],
                w["reason"].as_str().unwrap_or("?")
            )
        })
        .collect();
    assert!(
        !trims.is_empty(),
        "the record of a run that trimmed carries no trim row: {}",
        records[0]
    );
    let want = format!(" · trimmed: {}", trims.join(", "));
    assert!(
        line.contains(&want),
        "doctor's trim clause is not the record's trim rows.\nwant: {want}\nline: {line}"
    );
}

/// D2. Two runs that printed different sizes: the last run is the second, the largest is the larger, and the window
/// says two runs are on record.
#[test]
fn doctor_tells_the_last_run_from_the_largest() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let seed = seed::write(
        tmp.path(),
        &seed::TINY,
        "[budget]
session_start_chars = 300
",
    );
    let first = start(&seed, "d2-first");

    let toml_path = seed.home.join(".base-gbl").join("base.toml");
    let toml = std::fs::read_to_string(&toml_path).expect("the global base.toml");
    assert_eq!(
        toml.matches("session_start_chars = 300").count(),
        1,
        "control: the budget line to change is not in the global base.toml once:\n{toml}"
    );
    std::fs::write(
        &toml_path,
        toml.replace("session_start_chars = 300", "session_start_chars = 9000"),
    )
    .expect("the global base.toml rewritten");
    let second = start(&seed, "d2-second");

    let (u1, u2) = (units(&first), units(&second));
    println!("run 1 {} · run 2 {}", measured(&first), measured(&second));
    assert_ne!(
        u1, u2,
        "control: both runs printed {u1} units, so the last run and the largest cannot be told apart"
    );

    let report = doctor(&seed);
    let last = size_line(&report, "last run ")
        .unwrap_or_else(|| panic!("no session-start size line in doctor's output:\n{report}"));
    assert_eq!(
        of_units(last),
        (u2, 9000),
        "the last run is not the second run: {last}"
    );
    let largest = size_line(&report, "largest of the last ")
        .unwrap_or_else(|| panic!("doctor printed a last run and no largest:\n{report}"));
    assert!(
        largest.contains("largest of the last 2 run(s) on record: "),
        "the window is not the two runs on record: {largest}"
    );
    // The SECOND run must be the larger one. That is the whole point of the budget going UP between the two
    // runs rather than down. Seeded the other way this test PASSED while `largest` was broken: with run 1
    // already the largest, a `largest` that never updates still returns the right answer, so mutant MR3 -
    // which turns the largest-update branch into `if false` - could not be observed here at all. The adaptive
    // `want` below is good practice and did NOT save it, because the old fixture made u1 > u2 by construction,
    // and the only control asserted the two runs DIFFER. A test whose name claims it tells the last run from
    // the largest has to be able to fail when `largest` is wrong (plover, 2026-09-18, found by MR3).
    assert!(
        u2 > u1,
        "control: the second run ({u2} units) must be the LARGER one, or `largest` can be broken without this          test noticing - the blindness this fixture was inverted to remove"
    );
    let want = if u1 > u2 { (u1, 300) } else { (u2, 9000) };
    assert_eq!(
        of_units(largest),
        want,
        "the largest is not the larger run: {largest}"
    );
}

/// D3. A workspace where session start never ran says so, and never as a run of zero. After one run the reading
/// flips, which is the proof that the reader can see a record at all (law 48).
#[test]
fn a_tier_with_no_session_start_on_record_says_so_and_never_says_zero() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let seed = seed::write(tmp.path(), &seed::TINY, "");

    let before = doctor(&seed);
    assert!(
        before
            .lines()
            .any(|l| l.contains("workspace tier · session-start: no run on record")),
        "doctor printed no 'no run on record' line for a workspace where session start never ran:\n{before}"
    );
    assert!(
        !before.contains("session-start: last run "),
        "a last run was printed before any session start ran:\n{before}"
    );

    let _ = start(&seed, "d3-session");
    let after = doctor(&seed);
    assert!(
        after.contains("workspace tier · session-start: last run "),
        "after one session start the workspace tier still shows no last run:\n{after}"
    );
    assert!(
        !after
            .lines()
            .any(|l| l.contains("workspace tier · session-start: no run on record")),
        "the absence line survived a recorded run:\n{after}"
    );
}

/// D4. Spec A8: the legacy `[signal] max_chars` in the global base.toml is named by doctor, with the file and its
/// replacement. With the key removed from that file, no such line.
#[test]
fn doctor_names_the_legacy_max_chars_with_its_replacement() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let seed = seed::write(tmp.path(), &seed::TINY, "");
    let toml_path = seed.home.join(".base-gbl").join("base.toml");
    let toml = std::fs::read_to_string(&toml_path).expect("the global base.toml");
    assert_eq!(
        toml.matches("[signal]\nmax_chars = 2000\n").count(),
        1,
        "control: the seed's legacy key is not in its global base.toml once:\n{toml}"
    );

    let report = doctor(&seed);
    let head = format!("legacy: [signal] max_chars in {}", toml_path.display());
    let line = report
        .lines()
        .find(|l| l.contains(&head))
        .unwrap_or_else(|| panic!("no legacy line naming {head}:\n{report}"));
    for key in ["[budget] session_start_chars", "[budget] memory_chars"] {
        assert!(
            line.contains(key),
            "the legacy line does not name its replacement {key}: {line}"
        );
    }

    std::fs::write(&toml_path, toml.replace("[signal]\nmax_chars = 2000\n", ""))
        .expect("the global base.toml rewritten");
    let without = doctor(&seed);
    assert!(
        !without.contains("legacy: [signal] max_chars"),
        "a file without the key still printed it as legacy:\n{without}"
    );
}
