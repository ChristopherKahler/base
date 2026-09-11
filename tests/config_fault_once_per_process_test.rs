//! #158 leg A, the "once per process" half, proved end to end on the real binary.
//!
//! WHY THIS FILE EXISTS SEPARATELY, and it is the whole point of it.
//!
//! `src/config.rs` proves the latch by passing one in. That proves the LATCH and
//! nothing else: it cannot show that production actually wires the process-global
//! `Once`, because the test supplies its own. Moving the untested thing is not
//! testing it.
//!
//! A `std::sync::Once` cannot be re-armed, and `cargo test` runs every test in one
//! file inside ONE process, so whichever test reported first would decide the
//! result for all the others. Each `tests/*.rs` is its own process and its own
//! fresh `Once`, so this file gets the real latch, untouched, exactly once — which
//! is why the claim lives here on its own rather than beside the other binary
//! tests in `tests/config_fault_surface_test.rs`.
//!
//! THE VEHICLE. `base scaffold` loads the config TWICE in one process: once in
//! `cli::run`, which every subcommand inherits, and again in `scaffold::run`. An
//! unguarded report would therefore print twice, and the operator would be told
//! the same thing about the same file twice in one command. The second assertion
//! in this file pins that two-load shape, so the test cannot quietly degrade into
//! proving nothing if a load site is ever removed.

use std::path::Path;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_base");

/// The consequence clause every fault carries, and the thing to count.
const CONSEQUENCE: &str = "base is running on DEFAULT settings, not yours";

/// Isolated fake home. `BASE_HOME` redirects `home::home_root`, which every
/// `~`-rooted path in the crate resolves through, so the real
/// `~/.base-gbl/base.toml` and `~/.claude/CLAUDE.md` are unreachable from here.
fn isolated_home() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base-gbl").join(".base")).unwrap();
    tmp
}

fn run_scaffold(root: &Path, target: &Path) -> (i32, String, String) {
    let out = Command::new(BIN)
        .args(["scaffold", target.to_str().unwrap()])
        .current_dir(root)
        // Explicit environment: the isolation, and no network from a test.
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

/// One command, two config loads, ONE report.
///
/// Red without the latch: the same sentence about the same file, printed twice in
/// one command. That is the shape that makes an operator stop reading warnings,
/// which would waste the entire leg.
#[test]
fn one_command_that_loads_the_config_twice_reports_the_fault_once() {
    let home = isolated_home();
    let root = home.path();
    std::fs::write(
        root.join(".base-gbl").join("base.toml"),
        "[update]\nauto = \n",
    )
    .unwrap();

    let target = root.join("ws");
    std::fs::create_dir_all(&target).unwrap();

    let (_code, _out, err) = run_scaffold(root, &target);

    let times = err.matches(CONSEQUENCE).count();
    assert!(
        times > 0,
        "precondition: the fault has to be reported at all, or this counts nothing.\nstderr: {err}"
    );
    assert_eq!(
        times, 1,
        "one process, one report — a command that loads the config twice must not \
         say the same thing twice.\nstderr: {err}"
    );
}

/// The control that keeps the test above honest.
///
/// `one_command_..._once` counts to one. If a future change removed the second
/// load site, that count would still be one and the test would pass while proving
/// nothing at all — it would no longer be measuring a latch, just a single call.
/// So pin the two-load shape it depends on.
#[test]
fn the_command_under_test_really_does_load_the_config_more_than_once() {
    let cli = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/cli.rs")).unwrap();
    let scaffold =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/scaffold.rs")).unwrap();

    // Load 1: every subcommand inherits this one from `cli::run`.
    let run_at = cli.find("pub fn run() {").expect("cli::run is the entry point");
    let after_run = &cli[run_at..];
    assert!(
        after_run.contains("let config = BaseConfig::load(&cwd);"),
        "cli::run loads the config for every subcommand — if this moved, \
         the once-per-process test is measuring something else"
    );

    // Load 2: scaffold's own, inside the same process.
    assert!(
        scaffold.contains("BaseConfig::load(target)"),
        "scaffold::run loads the config a second time in the same process"
    );

    // And `base scaffold` really is the command that reaches it.
    assert!(
        cli.contains("base::scaffold::run(&target)"),
        "the Scaffold arm dispatches to scaffold::run"
    );
}

/// The absent control, in this process too. A first run scaffolding a workspace
/// is the single most common way anyone meets this code path, and it must be
/// completely silent about config.
#[test]
fn scaffolding_with_no_config_at_all_reports_nothing() {
    let home = isolated_home();
    let root = home.path();
    assert!(
        !root.join(".base-gbl").join("base.toml").exists(),
        "precondition: no config file anywhere"
    );

    let target = root.join("ws");
    std::fs::create_dir_all(&target).unwrap();

    let (_code, out, err) = run_scaffold(root, &target);

    assert_eq!(
        err.matches(CONSEQUENCE).count(),
        0,
        "a first run is not a fault: {err}"
    );
    assert!(
        !out.trim().is_empty(),
        "precondition: scaffold really ran, so the silence means something"
    );
}
