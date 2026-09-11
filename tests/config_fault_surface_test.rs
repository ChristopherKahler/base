//! #158 leg A at the surface an operator actually sees: the shipped binary, its
//! stderr, and its exit code.
//!
//! The unit tests in `src/config.rs` observe `BaseConfig::load_reporting`, which
//! returns its faults as a value. No operator reads a return value. A fix that is
//! green there and silent here has not fixed anything, so every claim leg A makes
//! is re-proved below against the binary `cargo test` actually builds.
//!
//! ISOLATION. `BASE_HOME` points at a tempdir and the cwd sits inside it, so both
//! config tiers and every `~`-rooted path resolve into the sandbox:
//! `home::home_root` consults `BASE_HOME` before the OS, and
//! `install::ensure_claude_md_current` — the one session-start step that writes a
//! `CLAUDE.md` — resolves its home through that same function. Nothing here can
//! reach the operator's real `~/.base-gbl/base.toml` or `~/.claude/CLAUDE.md`,
//! which is the lane's first hard boundary.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_base");

/// The fault text the product emits, as a consumer would grep for it.
const CONSEQUENCE: &str = "DEFAULT settings, not yours";

/// A fake home carrying a global tier and an operator profile.
///
/// The operator profile is not decoration. It makes `session-start` emit a block
/// on stdout whose content does not depend on `base.toml` in any way, which is
/// what gives the byte-identity test something real to compare. Two empty stdouts
/// compare equal and prove nothing — that silent pass is the same shape as the
/// bug, so `stdout_is_byte_identical_...` asserts the block is present as well.
fn home() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".base-gbl").join(".base")).unwrap();
    std::fs::create_dir_all(root.join(".base")).unwrap();
    std::fs::write(
        root.join(".base-gbl").join("operator.toml"),
        "name = \"probe-158\"\nactive = true\nnorth_star = \"a config that never lies\"\n",
    )
    .unwrap();
    tmp
}

fn global_toml(root: &Path) -> std::path::PathBuf {
    root.join(".base-gbl").join("base.toml")
}

/// One invocation of the real binary, with an EXPLICIT environment.
///
/// `BASE_HOME` is the isolation. `BASE_NO_AUTO_UPDATE` keeps a test off the
/// network; it is checked INSIDE `auto_update`, after the `update.auto` gate, so
/// it cannot mask anything these tests measure.
fn base(root: &Path) -> Command {
    let mut c = Command::new(BIN);
    c.current_dir(root)
        .env("BASE_HOME", root)
        .env("BASE_NO_AUTO_UPDATE", "1");
    c
}

/// Exit code, stdout, stderr.
fn run(root: &Path, args: &[&str]) -> (i32, String, String) {
    let out = base(root).args(args).output().expect("the base binary runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// One hook event, driven the way Claude Code drives it: JSON on stdin.
fn run_hook(root: &Path, event: &str, payload: &str) -> (i32, String, String) {
    let mut child = base(root)
        .args(["hook", event])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the base binary runs");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn session_start_payload(root: &Path) -> String {
    serde_json::json!({
        "session_id": "probe-158",
        "cwd": root.display().to_string(),
        "hook_event_name": "SessionStart",
    })
    .to_string()
}

// ─── The broken arm: the binary speaks ──────────────────────────────────────

/// An ordinary command, not a hook. `cli::run` loads the config for every
/// subcommand before it dispatches, so a broken file has to announce itself on
/// the very first thing an operator types.
///
/// `hooks manifest` is the cheap one: it reaches that load, writes nothing to the
/// graph, and its own output is a JSON object on stdout, so a fault on stderr
/// cannot be confused with it.
#[test]
fn an_unparseable_config_reaches_stderr_of_an_ordinary_command() {
    let h = home();
    let root = h.path();
    std::fs::write(global_toml(root), "[update]\nauto = \n").unwrap();

    let (_code, _out, err) = run(root, &["hooks", "manifest"]);

    assert!(
        err.contains("cannot parse"),
        "a file that will not parse has to say so: {err}"
    );
    assert!(
        err.contains(&global_toml(root).display().to_string()),
        "and name the file, or the operator cannot act on it: {err}"
    );
    assert!(
        err.contains(CONSEQUENCE),
        "and say what it cost them: {err}"
    );
}

/// The same file, read by the hook that runs on every session start. This is the
/// path #158's data loss travels, so it gets its own proof rather than inheriting
/// the one above.
#[test]
fn an_unparseable_config_reaches_stderr_of_the_session_start_hook() {
    let h = home();
    let root = h.path();
    std::fs::write(global_toml(root), "[update]\nauto = \n").unwrap();

    let (code, _out, err) = run_hook(root, "session-start", &session_start_payload(root));

    assert_eq!(code, 0, "hooks fail open and must keep doing so: {err}");
    assert!(
        err.contains("cannot parse") && err.contains(CONSEQUENCE),
        "the session-start hook reports the fault on stderr: {err}"
    );
}

// ─── The absent control: the binary stays quiet ─────────────────────────────

/// The control. With no `base.toml` at all — a first run — the binary must say
/// nothing about config on any channel. Without this arm, a "fix" that warned
/// unconditionally would pass every test above.
#[test]
fn an_absent_config_says_nothing_on_the_real_binary() {
    let h = home();
    let root = h.path();
    assert!(
        !global_toml(root).exists(),
        "precondition: this run has no config file at all"
    );

    let (_code, out, err) = run(root, &["hooks", "manifest"]);

    assert!(
        !err.contains(CONSEQUENCE),
        "a first run is not a fault and must not be reported as one: {err}"
    );
    assert!(
        !err.contains("cannot parse") && !err.contains("cannot read"),
        "nothing to complain about: {err}"
    );
    assert!(
        !out.trim().is_empty(),
        "precondition: the command really ran and produced its manifest"
    );
}

/// The other control: a good file is silent too, and the setting it carries is
/// the one in force. Proved through the binary, on the channel that ships.
#[test]
fn a_healthy_config_says_nothing_on_the_real_binary() {
    let h = home();
    let root = h.path();
    std::fs::write(global_toml(root), "[update]\nauto = false\n").unwrap();

    let (_code, _out, err) = run(root, &["hooks", "manifest"]);

    assert!(
        !err.contains(CONSEQUENCE) && !err.contains("cannot parse"),
        "a readable config has nothing to report: {err}"
    );
}

// ─── DoD 18: the fault must not move one byte of a hook's stdout ────────────

/// A hook's stdout is a parsed contract. `dispatch` documents the rule — "any
/// error → stderr only, exit 0, empty stdout" — and on session-start the stdout
/// IS the text injected into the model's context. A config warning that leaked
/// into it would be a worse defect than the silence leg A removes.
///
/// The two arms compare ABSENT against BROKEN, not healthy against broken, and
/// that choice is the whole point: both produce `BaseConfig::default()`, so the
/// effective settings are identical and the only difference between the runs is
/// whether a fault exists. Anything that moves in stdout is therefore the fault
/// leaking, and nothing else.
///
/// Each arm gets its OWN fake home so both are first runs and neither inherits
/// warm state from the other. The home path is then normalised out, because the
/// two tempdirs have different names and every absolute path in the block would
/// otherwise differ for a reason that has nothing to do with this test.
#[test]
fn a_hook_stdout_is_byte_identical_with_and_without_a_config_fault() {
    let quiet = home();
    let broken = home();
    std::fs::write(global_toml(broken.path()), "[update]\nauto = \n").unwrap();

    let (code_q, out_q, err_q) = run_hook(
        quiet.path(),
        "session-start",
        &session_start_payload(quiet.path()),
    );
    let (code_b, out_b, err_b) = run_hook(
        broken.path(),
        "session-start",
        &session_start_payload(broken.path()),
    );

    assert_eq!(code_q, 0, "hooks fail open: {err_q}");
    assert_eq!(code_b, 0, "and a config fault must not change that: {err_b}");

    let norm_q = out_q.replace(&quiet.path().display().to_string(), "<HOME>");
    let norm_b = out_b.replace(&broken.path().display().to_string(), "<HOME>");

    // Without this the test would pass over two empty strings and measure nothing,
    // which is exactly the failure shape it exists to catch.
    assert!(
        norm_q.contains("<operator>"),
        "precondition: session-start really did emit a stdout block to compare.\nstdout: {out_q}\nstderr: {err_q}"
    );

    assert_eq!(
        norm_q, norm_b,
        "a config fault must not add, remove or move ONE byte of hook stdout.\n\
         stderr with the fault: {err_b}"
    );

    // And the run that had something to say did say it — on the other channel.
    assert!(
        err_b.contains(CONSEQUENCE),
        "the broken arm has to have actually faulted, or this proved nothing: {err_b}"
    );
    assert!(
        !err_q.contains(CONSEQUENCE),
        "and the quiet arm has to have actually been quiet: {err_q}"
    );
}

// ─── DoD 15: the pinned machine, on the shipped channel ────────────────────

/// N3, the shipped claim. `config.rs` documents `base config set update.auto
/// false` as the way to pin a machine; an unparseable file resets that to `true`
/// and the machine silently resumes updating itself.
///
/// The pin is genuinely lost — a file that will not parse cannot say `false` —
/// so what this proves is the second half of DoD 15: the fault is surfaced before
/// `auto_update` can act. The ORDERING is structural and is pinned separately by
/// `the_config_load_precedes_auto_update_in_the_hook_path`; what is measured here
/// is that the operator is told at all, on the channel they can see.
#[test]
fn a_pinned_machine_that_loses_its_config_is_told_on_the_real_binary() {
    let h = home();
    let root = h.path();

    // Pinned and readable: silent.
    std::fs::write(global_toml(root), "[update]\nauto = false\n").unwrap();
    let (_, _, err_pinned) = run_hook(root, "session-start", &session_start_payload(root));
    assert!(
        !err_pinned.contains(CONSEQUENCE),
        "precondition: a working pin reports nothing: {err_pinned}"
    );

    // The file stops parsing. The pin is lost; the silence must not be.
    std::fs::write(global_toml(root), "[update]\nauto = false\n[[[ broken\n").unwrap();
    let (code, _, err_broken) = run_hook(root, "session-start", &session_start_payload(root));

    assert_eq!(code, 0, "still fails open: {err_broken}");
    assert!(
        err_broken.contains(CONSEQUENCE) && err_broken.contains("cannot parse"),
        "the machine just un-pinned itself and the operator has to hear about it: {err_broken}"
    );
}

/// The ordering DoD 15 depends on, pinned in the source rather than observed.
///
/// It cannot honestly be observed: stdout is block-buffered when piped and stderr
/// is not, so interleaving two streams from one child proves nothing about the
/// order the writes were issued in. Reading the structure is the sound
/// instrument, and `tests/hook_output_channel_test.rs` already uses it in this
/// tree, so this is the established mechanism and not a new one.
///
/// What it pins: `run_event` loads the config — and therefore reports — before it
/// dispatches to any handler, and `auto_update` is reached only from inside
/// `session_start::handle`, which is one of those handlers.
#[test]
fn the_config_load_precedes_auto_update_in_the_hook_path() {
    let hook_mod =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/hook/mod.rs")).unwrap();
    let session_start = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/hook/session_start.rs"
    ))
    .unwrap();

    let run_event = hook_mod
        .find("fn run_event(")
        .expect("run_event is where a hook decides anything");
    let body = &hook_mod[run_event..];

    let load_at = body
        .find("BaseConfig::load(")
        .expect("run_event loads the config");
    let dispatch_at = body
        .find("match event {")
        .expect("run_event dispatches on the event");
    assert!(
        load_at < dispatch_at,
        "the config is loaded, and any fault reported, BEFORE any handler runs"
    );

    let handler_at = body
        .find("session_start::handle(")
        .expect("session-start reaches its handler");
    assert!(
        load_at < handler_at,
        "and specifically before session_start::handle"
    );

    // `auto_update` lives behind that handler, so the report necessarily precedes it.
    assert!(
        session_start.contains("auto_update(config)"),
        "auto_update is still called from session_start::handle"
    );
    let handle_at = session_start
        .find("pub fn handle(")
        .expect("handle is the entry point");
    let auto_at = session_start
        .find("auto_update(config)")
        .expect("auto_update call site");
    assert!(
        handle_at < auto_at,
        "auto_update runs inside handle, which runs after the load"
    );
}
