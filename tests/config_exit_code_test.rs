//! #158 leg B: a `base config` command that fails has to EXIT non-zero.
//!
//! REAL BINARY FROM THE FIRST LINE, and that is not a stylistic preference. Leg B
//! is entirely about what the PROCESS EXITS WITH, and a unit test cannot observe a
//! process exit code at all — it observes a return value. "rc 0 over a parse
//! failure" is precisely the gap between those two things.
//!
//! Leg A measured that gap rather than assuming it: under the mutation that made
//! leg A's fault report a no-op, all nine of its unit tests stayed GREEN while
//! both of its real-binary files went RED. A unit layer does not guard the
//! shipping channel, so this leg does not have one.
//!
//! THE INVARIANT, which settles every branch uniformly instead of making twelve
//! separate judgement calls: **the exit code follows the message.** A branch that
//! prints an error exits non-zero; a branch that succeeds exits 0. `exit_code_
//! follows_the_message_on_every_failing_branch` is that invariant as a sweep, and
//! the named tests below it pin the ones with extra consequences.
//!
//! WHY THESE ALL USED TO EXIT 0. Every branch was `eprintln!(...)` followed by a
//! bare `return`, inside `cli::run()`, which returns `()`. A bare return from a
//! function that yields `()` is a NORMAL exit. So `base config set x.y v && echo
//! ok` printed the error AND then printed ok, and every CI step, shell chain and
//! script treated a broken config as a success.
//!
//! ONE DOCUMENTED EXCLUSION, ruled rather than missed. `base config get
//! <section>.<field>` for a key that exists nowhere and has no compiled-in
//! default still prints "Key not found" and exits 0. A missing key is not a parse
//! failure, so it sits outside this lane under the backlog's standing order 1 and
//! is logged to the work order's unranked findings instead. It is deliberately
//! NOT asserted either way here: pinning rc 0 would turn a deferral into a rule
//! and make the eventual fix look like a regression.
//!
//! ISOLATION. `BASE_HOME` points at a tempdir and the cwd sits inside it, so
//! every `~`-rooted path resolves into the sandbox. Nothing here can reach the
//! operator's real `~/.base-gbl/base.toml`.

use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_base");

/// What state the global `base.toml` is in when the command runs.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Config {
    /// No file at all — a first run.
    Absent,
    /// Present, readable, valid.
    Healthy,
    /// Present, readable, not TOML.
    Unparseable,
    /// Present and cannot be read: a directory sits where the file belongs.
    /// Chosen over `chmod 000` because that is a no-op for root, which would make
    /// the test silently stop measuring rather than fail.
    Unreadable,
    /// Valid TOML whose `[update]` is a string, so `set update.auto` finds a
    /// section that is not a table.
    SectionIsScalar,
    /// Present, readable, valid, and not writable.
    ReadOnly,
}

fn home_with(state: Config) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let gbl = tmp.path().join(".base-gbl");
    std::fs::create_dir_all(gbl.join(".base")).unwrap();
    let path = gbl.join("base.toml");
    match state {
        Config::Absent => {}
        Config::Healthy => std::fs::write(&path, "[update]\nauto = false\n").unwrap(),
        Config::Unparseable => std::fs::write(&path, "[update]\nauto = \n").unwrap(),
        Config::Unreadable => std::fs::create_dir_all(&path).unwrap(),
        Config::SectionIsScalar => std::fs::write(&path, "update = \"not a table\"\n").unwrap(),
        Config::ReadOnly => {
            std::fs::write(&path, "[update]\nauto = false\n").unwrap();
            set_read_only(&path);
        }
    }
    tmp
}

#[cfg(unix)]
fn set_read_only(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o444)).unwrap();
    // A read-only file is only a failure mechanism for a user the kernel actually
    // stops. Root writes straight through it, and this test would then pass while
    // measuring nothing — the same silent-skip shape leg B exists to remove. So
    // prove the mechanism works in THIS process before relying on it.
    if std::fs::write(path, "probe").is_ok() {
        panic!(
            "this test cannot measure anything: writing to a 0444 file succeeded, \
             so the suite is running with permission to ignore the mode bits (root \
             or CAP_DAC_OVERRIDE). Run it as an ordinary user."
        );
    }
}

#[cfg(not(unix))]
fn set_read_only(path: &Path) {
    let mut p = std::fs::metadata(path).unwrap().permissions();
    p.set_readonly(true);
    std::fs::set_permissions(path, p).unwrap();
    if std::fs::write(path, "probe").is_ok() {
        panic!("this test cannot measure anything: a read-only file was still writable");
    }
}

fn global_toml(root: &Path) -> PathBuf {
    root.join(".base-gbl").join("base.toml")
}

/// Exit code, stdout, stderr — with an explicit environment.
fn run(root: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(BIN)
        .args(args)
        .current_dir(root)
        .env("BASE_HOME", root)
        .env("BASE_NO_AUTO_UPDATE", "1")
        .output()
        .expect("the base binary runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

// ─── The invariant, swept ──────────────────────────────────────────────────

/// Every failing branch of `base config`, the state that reaches it, and a
/// fragment of the message it owes the operator.
///
/// This is the enumeration, not a sample. Each row is one of the branches that
/// used to print an error and exit 0.
///
/// THE EXPECTED FRAGMENT IS NOT DECORATION, and it was added because designing
/// this leg's mutations exposed the gap. Make the parse failure return "no file"
/// instead of dying, and `list` still exits non-zero — it just exits non-zero
/// with the WRONG cause, sending the operator to run `base install` over a
/// `base.toml` that is sitting right there with a typo in it. An exit code alone
/// cannot tell those two apart, so the row pins the cause as well.
const FAILING: &[(&str, Config, &[&str], &str)] = &[
    // `list` reads the file, so all three bad states are failures for it.
    ("list / unparseable", Config::Unparseable, &["config", "list"], "Failed to parse"),
    ("list / unreadable", Config::Unreadable, &["config", "list"], "Cannot read"),
    ("list / absent", Config::Absent, &["config", "list"], "base install"),
    // `get` fails on a file it cannot use, and on a malformed key.
    ("get / unparseable", Config::Unparseable, &["config", "get", "update.auto"], "Failed to parse"),
    ("get / unreadable", Config::Unreadable, &["config", "get", "update.auto"], "Cannot read"),
    ("get / malformed key", Config::Healthy, &["config", "get", "nodot"], "section.field"),
    // `set` fails on all of those, plus the states only a write can reach.
    ("set / unparseable", Config::Unparseable, &["config", "set", "update.auto", "true"], "Failed to parse"),
    ("set / unreadable", Config::Unreadable, &["config", "set", "update.auto", "true"], "Cannot read"),
    ("set / absent", Config::Absent, &["config", "set", "update.auto", "true"], "base install"),
    ("set / malformed key", Config::Healthy, &["config", "set", "nodot", "true"], "section.field"),
    ("set / section is not a table", Config::SectionIsScalar, &["config", "set", "update.auto", "true"], "not a table"),
    ("set / read-only file", Config::ReadOnly, &["config", "set", "update.auto", "true"], "Failed to write"),
];

/// THE leg B test. Every branch that writes an error string exits non-zero, and
/// the message names the cause it actually hit.
#[test]
fn exit_code_follows_the_message_on_every_failing_branch() {
    let mut wrong = Vec::new();
    for (name, state, args, expected) in FAILING {
        let h = home_with(*state);
        let (code, out, err) = run(h.path(), args);
        if code == 0 {
            wrong.push(format!(
                "  {name}: rc 0\n    stderr: {}\n    stdout: {}",
                err.trim(),
                out.trim()
            ));
            continue;
        }
        // A non-zero code with nothing said is not the fix either: the operator
        // has to be able to tell what went wrong.
        if err.trim().is_empty() {
            wrong.push(format!("  {name}: rc {code} but said NOTHING on stderr"));
            continue;
        }
        // And it must not be a panic. A panic clears non-zero by accident, which
        // is not the same as answering correctly.
        if code == 101 || err.contains("panicked at") {
            wrong.push(format!("  {name}: PANICKED (rc {code}) instead of reporting: {}", err.trim()));
            continue;
        }
        // Right code, right channel, WRONG CAUSE is still a wrong answer.
        if !err.contains(expected) {
            wrong.push(format!(
                "  {name}: rc {code} but the message names the wrong cause\n    wanted to see: {expected}\n    stderr: {}",
                err.trim()
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "these branches do not report their failure correctly:\n{}",
        wrong.join("\n")
    );
}

/// The control, and without it the sweep above proves nothing: a `config` that
/// always exited non-zero would pass every row and be useless.
#[test]
fn the_succeeding_branches_still_exit_zero() {
    // list
    let h = home_with(Config::Healthy);
    let (code, out, err) = run(h.path(), &["config", "list"]);
    assert_eq!(code, 0, "a readable config lists fine: {err}");
    assert!(out.contains("update.auto"), "and really did list it: {out}");

    // get, against a key the file sets
    let h = home_with(Config::Healthy);
    let (code, out, err) = run(h.path(), &["config", "get", "update.auto"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("false"), "the file's value is the one reported: {out}");

    // set
    let h = home_with(Config::Healthy);
    let (code, out, err) = run(h.path(), &["config", "set", "update.auto", "true"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("Updated"), "{out}");
    let after = std::fs::read_to_string(global_toml(h.path())).unwrap();
    assert!(after.contains("auto = true"), "and the write actually landed: {after}");
}

// ─── The branches with consequences beyond the exit code ───────────────────

/// The headline of leg B: `config set` must not report success over a write it
/// did not make.
///
/// This branch was the worst of the twelve because it had no `return` AT ALL —
/// it printed the error and fell through to `println!("Updated {key} = {new_val}")`.
/// So `base config set update.auto false` against a read-only file told the
/// operator, in as many words, that it had updated a setting it had not written.
#[test]
fn config_set_never_claims_success_over_a_write_it_did_not_make() {
    let h = home_with(Config::ReadOnly);
    let path = global_toml(h.path());
    let before = std::fs::read(&path).unwrap();

    let (code, out, err) = run(h.path(), &["config", "set", "update.auto", "true"]);

    assert_ne!(code, 0, "a write that did not happen is not a success: {out} {err}");
    assert!(
        !out.contains("Updated"),
        "it must not SAY it updated anything — this is the sentence that lied: {out}"
    );
    assert!(!err.trim().is_empty(), "and it has to say what went wrong: {err}");

    let after = std::fs::read(&path).unwrap();
    assert_eq!(before, after, "the file is byte-identical, as it must be");
}

/// `set` against an absent config refuses and NAMES THE REMEDY.
///
/// `base install` is what creates this file (`install::create_global_tier`), so
/// absent means install never ran. Creating a bare one here would hand the
/// operator a silently degraded config they would later read as theirs — the same
/// class of harm as the silent default this whole lane removes. The wording
/// mirrors `scaffold::register_workspace` rather than being a second spelling.
#[test]
fn set_against_an_absent_config_refuses_and_names_base_install() {
    let h = home_with(Config::Absent);
    let (code, _out, err) = run(h.path(), &["config", "set", "update.auto", "true"]);

    assert_ne!(code, 0, "a discarded write is not a success");
    assert!(
        err.contains("base install"),
        "a bare refusal leaves the operator stuck; name the fix: {err}"
    );
    assert!(
        !global_toml(h.path()).exists(),
        "and it must NOT have created the file it refused to write"
    );
}

/// `list` against an absent config refuses too, and for a reason the old code had
/// already half-decided: it printed "Cannot read base.toml at ..." and then exited
/// 0 anyway. The message was right; the exit code contradicted it.
#[test]
fn list_against_an_absent_config_refuses_and_names_base_install() {
    let h = home_with(Config::Absent);
    let (code, _out, err) = run(h.path(), &["config", "list"]);

    assert_ne!(code, 0, "there is no file to list");
    assert!(err.contains("base install"), "name the fix: {err}");
}

/// The asymmetry between `get` and `set` on an ABSENT file is deliberate, and
/// this test is what makes it a decision rather than an inconsistency.
///
/// Nearly every setting has a compiled-in default that is what the machine
/// actually does, so a machine with no `base.toml` can still be asked what it
/// will do. Reading a default is honest. Silently discarding a WRITE is not.
#[test]
fn get_against_an_absent_config_answers_from_the_default_and_exits_zero() {
    let h = home_with(Config::Absent);
    let (code, out, err) = run(h.path(), &["config", "get", "update.auto"]);

    assert_eq!(code, 0, "absent is a read-state for get, not an error: {err}");
    assert!(
        out.contains("true") && out.contains("default"),
        "it answers with the default, and says that is what it is: {out}"
    );
}

/// N5. The `Config` arm used to `.expect()` on the home directory — a PANIC at
/// rc 101 — for the same condition its 113 sibling call sites report through
/// `die` at rc 1. One fault, two exit codes, depending on which command you ran.
///
/// WHAT THIS TEST CAN AND CANNOT PROVE, said plainly rather than overclaimed:
/// the panic cannot be reached from a test binary at all. `home::home_root` is
/// compiled with the `isolation-guard` feature under `cargo test`, and that build
/// ends `let home = Some(test_root())` — it never returns `None`, by design, so no
/// test process can drive that branch. So this pins it STRUCTURALLY: no `.expect(`
/// survives in the arm, and the sweep above separately asserts that no failing
/// branch answers with rc 101.
#[test]
fn the_config_arm_no_longer_panics_where_its_siblings_die() {
    let cli = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/cli.rs")).unwrap();
    let start = cli
        .find("Some(Commands::Config { action }) => {")
        .expect("the Config arm");
    let end = cli[start..]
        .find("Some(Commands::Context {")
        .expect("the arm after it")
        + start;
    let arm = &cli[start..end];

    assert!(
        !arm.contains(".expect("),
        "the Config arm must report failures, not panic on them"
    );
    assert_eq!(
        arm.matches("return;").count(),
        0,
        "a bare `return` from `run() -> ()` is a NORMAL exit — that is the whole defect"
    );
    assert!(
        arm.contains("die("),
        "and it uses the mechanism the rest of the file already uses"
    );
}
