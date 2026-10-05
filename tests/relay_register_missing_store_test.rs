//! RANK A: registering against a relay store that does not exist must SAY SO.
//!
//! Measured 2026-09-20 on Chris's machine. Three seats (`finch`, `grebe` and
//! `auk`) ran `base relay register --as <title> --project base-0160`. No store
//! of that name existed; only `skyrim-companion` did. All three were left off
//! `base relay board`, which is the surface Chris's ping hub renders, and all
//! three were told `Registered '<title>' globally` — a cheerful confirmation of
//! a different and lesser thing.
//!
//! THE DIAGNOSTIC ALREADY EXISTS AND IS DISCARDED. `relay::resolve_store`
//! carries the comment "Fail LOUD on a missing store" and bails with the store
//! name and the list of stores that do exist. `cli.rs`'s register arm matches
//! `Ok(store) if store.exists()` and then a catch-all `_`, and the `_` eats
//! that error. A purpose-built guard, silently disabled by a pattern that looks
//! harmless.
//!
//! Compounding it: base's own relay wake contract instructs the operator to
//! pass `--project`. Following the documentation produces the broken state.

use std::path::PathBuf;
use std::process::Command;

struct Rig {
    home: PathBuf,
    ws: PathBuf,
    _root: tempfile::TempDir,
}

fn rig() -> Rig {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let ws = root.path().join("ws");
    for dir in [home.join(".base-gbl").join(".base"), ws.join(".base")] {
        std::fs::create_dir_all(&dir).unwrap();
    }
    Rig { home, ws, _root: root }
}

fn base(rig: &Rig, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_base"))
        .args(args)
        .current_dir(&rig.ws)
        .env("BASE_HOME", &rig.home)
        .env("BASE_NO_AUTO_UPDATE", "1")
        .env("CLAUDE_CODE_SESSION_ID", "sess-rank-a")
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn registering_against_a_store_that_does_not_exist_names_it_and_the_consequence() {
    let rig = rig();
    base(&rig, &["relay", "init", "--project", "real-store"]);

    let (_, out, err) = base(
        &rig,
        &["relay", "register", "--as", "tern", "--project", "no-such-store"],
    );
    let said = format!("{out}{err}");

    assert!(
        said.contains("no-such-store"),
        "the store that was not found must be named; got:\n{said}"
    );
    assert!(
        said.contains("real-store"),
        "the stores that DO exist must be named so the operator can pick one; got:\n{said}"
    );
    assert!(
        said.to_lowercase().contains("board") || said.to_lowercase().contains("not registered"),
        "the CONSEQUENCE must be stated — the operator is not on the board; got:\n{said}"
    );
}

/// Negative control: the ordinary path must stay quiet. A warning that fires on
/// correct usage is as useless as one that never fires.
#[test]
fn registering_against_a_store_that_exists_says_so_and_does_not_warn() {
    let rig = rig();
    base(&rig, &["relay", "init", "--project", "real-store"]);

    let (_, out, err) = base(
        &rig,
        &["relay", "register", "--as", "tern", "--project", "real-store"],
    );
    let said = format!("{out}{err}");

    assert!(
        said.contains("real-store"),
        "the store joined must be named; got:\n{said}"
    );
    assert!(
        !said.to_lowercase().contains("not found"),
        "a successful register must not warn; got:\n{said}"
    );
}
