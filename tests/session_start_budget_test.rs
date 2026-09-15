//! Rank 00 at the surface Claude reads: session start's stdout, from the binary `cargo test` builds.
//!
//! Driven the way Claude Code drives it: JSON on stdin, `BASE_HOME` at a seeded fake home so
//! base's write tripwire stays armed, and the relay variables a caller's shell might carry
//! removed, so the output is the seed's and nobody else's.
//!
//! This file uses no API commit B adds, on purpose: it compiles against the head before commit B,
//! so every test here can be run red there first. The in-process ledger and suppression tests
//! live in `tests/signal_test.rs`.

mod seed;

use std::path::PathBuf;
use std::sync::OnceLock;

use seed::{measured, run_session_start, units};

const REAL_BUDGET: usize = 9000;

/// One session start over the real-size seed, shared by the tests that read it: the run is the
/// slow part, and each test asserts a different property of the same stdout.
fn real_size_run() -> &'static (i32, String, String) {
    static RUN: OnceLock<(i32, String, String)> = OnceLock::new();
    RUN.get_or_init(|| {
        let root: PathBuf =
            std::env::temp_dir().join(format!("base-seed-real-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let seed = seed::write(
            &root,
            &seed::REAL,
            &format!("[budget]\nsession_start_chars = {REAL_BUDGET}\n"),
        );
        run_session_start(&seed, Some("seed-session"))
    })
}

#[test]
fn the_seed_writes_the_measured_note_total() {
    assert_eq!(seed::note_chars_written(&seed::REAL), 970_366);
}

/// J3.1 on the real-size seed. Red before commit B: the same seed printed every block whole.
#[test]
fn session_start_fits_its_budget_on_the_real_size_seed() {
    let (code, stdout, stderr) = real_size_run();
    assert_eq!(*code, 0, "hooks fail open. stderr: {stderr}");
    println!("session start on the real-size seed: {}", measured(stdout));
    assert!(
        !stdout.is_empty(),
        "the hook printed nothing, so this measured nothing. stderr: {stderr}"
    );
    // Commit C: the fork block's header is `FORKS (157 open, ...)` since the B layout, where it
    // was `[Forks]`. The control is unchanged: either the block or its floor carries the seed's 157.
    assert!(
        stdout.contains("FORKS (157 open") || stdout.contains("forks 157 ·"),
        "the seed's 157 open forks are not in the output, so the seed was not read"
    );
    assert!(
        units(stdout) <= REAL_BUDGET,
        "session start printed {} UTF-16 units against a budget of {REAL_BUDGET}",
        units(stdout)
    );
}

/// Spec A8: the legacy `[signal] max_chars = 2000` is read by nothing. As the budget it would cut
/// session start to 2,000 units, the map with everything else.
#[test]
fn the_legacy_max_chars_is_not_the_budget() {
    let (code, stdout, stderr) = real_size_run();
    assert_eq!(*code, 0, "stderr: {stderr}");
    assert!(
        units(stdout) > 2000,
        "session start printed {} units: the legacy max_chars = 2000 is acting as the budget",
        units(stdout)
    );
}

/// T16. A shown-once block marks itself shown while it renders: the welcome stamps its marker, the
/// wake contract stamps its nudge. Collapsed, it must leave its full text in the full-output file
/// and a floor that names the file. Red before commit B: no file, no floor.
#[test]
fn a_collapsed_shown_once_block_leaves_its_text_in_the_full_output_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let seed = seed::write(
        tmp.path(),
        &seed::TINY,
        "[budget]\nsession_start_chars = 1\n",
    );
    let (code, stdout, stderr) = run_session_start(&seed, Some("t16-session"));
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(
        !stdout.is_empty(),
        "the hook printed nothing. stderr: {stderr}"
    );

    let file = seed.ws.join(".base").join("last-session-start.md");
    let full = std::fs::read_to_string(&file).unwrap_or_else(|e| {
        panic!(
            "no full-output file at {}: {e}\nstdout:\n{stdout}",
            file.display()
        )
    });
    println!("stdout {} · file {}", measured(&stdout), measured(&full));
    assert!(
        units(&full) > units(&stdout),
        "the file is the untrimmed output, so it is the larger"
    );

    let shown = file.display().to_string();
    // Commit C: the relay tick is two blocks since the B layout, the tasks delivered to the
    // session and the wake contract. The seed delivers no task, so the wake contract carries the case
    // the single `relay-tick` block carried.
    let cases = [
        ("first-run", "base is installed."),
        ("relay-wake", "=== RELAY WAKE CONTRACT"),
    ];
    for (kind, marker) in cases {
        assert!(
            full.contains(marker),
            "{kind}: the file does not hold its full text"
        );
        assert!(
            !stdout.contains(marker),
            "{kind}: a 1-unit budget still printed it whole"
        );
        let floor = format!("{kind} 1 · full text: {shown}");
        assert!(
            stdout.lines().any(|line| line == floor),
            "{kind}: no floor naming the file. wanted:\n{floor}\nstdout:\n{stdout}"
        );
    }
}

/// T17. Nothing in session start writes to stdout: everything it says goes through the one
/// measured emission. Shapes from the census (lane doc B6, B7): `println!`, `print!(` and a direct
/// `stdout()` handle. `eprintln!` is stderr, outside the budget by construction, and a shape counts
/// only when no word character precedes it, so `eprintln!` never reads as `println!`. Red before
/// commit B: 32 sites.
#[test]
fn no_stdout_write_remains_in_session_start() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/hook/session_start.rs"
    ))
    .expect("session_start.rs");
    let mut visited = 0usize;
    let mut hits: Vec<String> = Vec::new();
    for (n, line) in src.lines().enumerate() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        visited += 1;
        for shape in ["println!", "print!(", "stdout()"] {
            if occurs_unprefixed(line, shape) {
                hits.push(format!("{}: {shape}: {}", n + 1, line.trim()));
            }
        }
    }
    assert!(
        visited > 100,
        "read {visited} code lines of session_start.rs: this measured nothing"
    );
    assert!(
        hits.is_empty(),
        "stdout writes left in session_start.rs:\n{}",
        hits.join("\n")
    );
}

/// True when `shape` occurs in `line` with no letter, digit or `_` right before it.
fn occurs_unprefixed(line: &str, shape: &str) -> bool {
    line.match_indices(shape).any(|(i, _)| {
        line[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
    })
}

#[test]
fn the_stdout_shape_matcher_tells_stdout_from_stderr() {
    assert!(occurs_unprefixed("    println!(\"x\");", "println!"));
    assert!(occurs_unprefixed(
        "let _ = std::io::stdout().lock();",
        "stdout()"
    ));
    assert!(
        !occurs_unprefixed("    eprintln!(\"x\");", "println!"),
        "stderr is not stdout"
    );
    assert!(
        !occurs_unprefixed("    eprint!(\"x\");", "print!("),
        "stderr is not stdout"
    );
}

/// T7, the drop path that is gone. `run_signals` dropped whole signals past `[signal] max_chars`
/// and counted them in a `dropped` counter, reported at the tail. This guard is keyed on names, so
/// it catches that path coming back as it was, and nothing else; the removal paths are enumerated
/// in the lane doc (B14). Red before commit B: all three names are in `src/signal/mod.rs` code.
#[test]
fn the_max_chars_drop_path_is_gone_from_the_signals() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/signal/mod.rs"))
        .expect("signal/mod.rs");
    let code: Vec<&str> = src
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect();
    assert!(
        code.len() > 50,
        "read {} code lines of signal/mod.rs: this measured nothing",
        code.len()
    );
    for gone in ["max_chars", "dropped", "budget cap"] {
        let lines: Vec<&&str> = code.iter().filter(|l| l.contains(gone)).collect();
        assert!(
            lines.is_empty(),
            "`{gone}` is back in signal/mod.rs code: {lines:?}"
        );
    }
}
