//! #158 leg C: a registry base cannot read must never become a registry base
//! reports as empty.
//!
//! THE DEFECT. `registered_workspace_paths()` returned `Vec<String>` and answered
//! four different states with the same empty vec: no home directory, no
//! `base.toml`, a `base.toml` that could not be read, and one that could not be
//! parsed. That empty vec went to `build_workspace_block`, which writes
//! `- (none registered)`, and `sync_claude_md_registry` then wrote that over the
//! managed block in `~/.claude/CLAUDE.md`. One unparseable character in
//! `base.toml` — a legacy Windows path pasted raw is the reported cause — and the
//! operator's entire registered-workspace list was replaced with the words "none
//! registered", by a command whose whole job is to keep that list current.
//!
//! An empty list is a CLAIM that nothing is registered. It must never be the way
//! base says "I could not tell".
//!
//! HOW THIS LEG CLOSES, and it closes no other way: every test here starts from a
//! `CLAUDE.md` that already holds real registrations and a `base.toml` that is
//! deliberately broken, and asserts the file is **byte-identical** afterwards. A
//! test that starts from a good config and asserts the good path proves nothing
//! about this leg.
//!
//! BYTE-IDENTICAL, not "still mentions three workspaces". A count survives a
//! rewrite that happened to produce the same count, and the block is not the only
//! thing in that file — the prose around it is the operator's own and has to come
//! through untouched too. Comparing the whole file byte for byte is the only
//! assertion that covers both.
//!
//! ISOLATION. `BASE_HOME` is a tempdir and the cwd sits inside it. The
//! `~/.claude/CLAUDE.md` every one of these tests writes to is
//! `<tempdir>/.claude/CLAUDE.md`; `install::ensure_claude_md_current` and
//! `scaffold::sync_claude_md_registry` both resolve their home through
//! `home::home_root`, which consults `BASE_HOME` before the OS. Nothing here can
//! reach the operator's real file, which is the lane's first hard boundary.

use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_base");

/// A newline, as a constant, because a char literal for it does not survive
/// being written through a shell heredoc and a Python string on the way here.
const NEWLINE: char = 10u8 as char;

/// Three workspaces an operator would lose. Real-looking, and one of them is the
/// Windows shape whose raw paste is what breaks the file in the first place.
const REGISTERED: [&str; 3] = [
    "/home/operator/lifestyle",
    "/home/operator/ops-sys/toolbox",
    "C:/Users/operator/Documents/grazer",
];

/// The `base.toml` that breaks, and why it breaks.
///
/// `\\?\` is a real Windows extended-length prefix and `\?` is not a TOML escape,
/// so a registry that recorded one verbatim stops parsing. This is the reported
/// cause of #158, not an invented corruption — and note that the file still
/// plainly contains the other two paths, which is exactly why erasing them is
/// data loss rather than an honest empty answer.
fn broken_registry() -> String {
    format!(
        "[[workspace]]\npath = \"\\\\?\\E:\\legacy\"\n\n\
         [[workspace]]\npath = \"{}\"\n\n\
         [[workspace]]\npath = \"{}\"\n",
        REGISTERED[0], REGISTERED[1]
    )
}

fn healthy_registry() -> String {
    REGISTERED
        .iter()
        .map(|p| format!("[[workspace]]\npath = \"{p}\"\n"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A `CLAUDE.md` that already carries the managed block with all three
/// workspaces in it, plus prose either side that must survive untouched.
fn claude_md_with_registrations() -> String {
    let mut s = String::from(
        "# Operator instructions\n\nThis paragraph is the operator's own and is not managed.\n\n",
    );
    s.push_str("<!-- BASE:WORKSPACES:START — auto-generated from ~/.base-gbl/base.toml; do not edit between markers -->\n");
    s.push_str("## Registered Workspaces (auto — synced from base.toml)\n\n");
    for p in REGISTERED {
        s.push_str(&format!("- `{p}`\n"));
    }
    s.push_str("<!-- BASE:WORKSPACES:END -->\n");
    s.push_str("\n## Something after the block\n\nAlso the operator's own.\n");
    s
}

struct Fake {
    dir: tempfile::TempDir,
}

impl Fake {
    /// A fake home with `.claude/CLAUDE.md` holding three registrations and a
    /// `base.toml` in whatever state the test needs.
    fn new(registry: Option<&str>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".base-gbl").join(".base")).unwrap();
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::write(root.join(".claude").join("CLAUDE.md"), claude_md_with_registrations())
            .unwrap();
        if let Some(r) = registry {
            std::fs::write(root.join(".base-gbl").join("base.toml"), r).unwrap();
        }
        Self { dir }
    }

    /// The same, but `base.toml` is a directory: present and unreadable.
    fn with_unreadable_registry() -> Self {
        let f = Self::new(None);
        std::fs::create_dir_all(f.root().join(".base-gbl").join("base.toml")).unwrap();
        f
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn claude_md(&self) -> PathBuf {
        self.root().join(".claude").join("CLAUDE.md")
    }

    /// The whole file, as bytes.
    ///
    /// Raw bytes rather than a hash on purpose: it is the same assertion, needs
    /// no crate, and when it fails it shows the difference instead of two hex
    /// strings that say only "these are not equal".
    fn claude_md_bytes(&self) -> Vec<u8> {
        std::fs::read(self.claude_md()).unwrap()
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let out = Command::new(BIN)
            .args(args)
            .current_dir(self.root())
            .env("BASE_HOME", self.root())
            .env("BASE_NO_AUTO_UPDATE", "1")
            .output()
            .expect("the base binary runs");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }
}

/// The precondition every test in this file depends on: the file really does
/// start out holding the registrations, so "it survived" means something.
fn assert_registrations_present(f: &Fake, when: &str) {
    let text = std::fs::read_to_string(f.claude_md()).unwrap();
    for p in REGISTERED {
        assert!(
            text.contains(p),
            "{when}: expected {p} in CLAUDE.md, got:\n{text}"
        );
    }
    assert!(
        !text.contains("(none registered)"),
        "{when}: CLAUDE.md must not say (none registered):\n{text}"
    );
}

// ─── The binary has to be the one we just built ────────────────────────────

/// Refuse to measure a `base` binary that does not contain the code under test.
///
/// MEASURED 2026-09-11, and the reason this is BEHAVIOURAL rather than an mtime
/// comparison: `target/debug/base` — the exact path `CARGO_BIN_EXE_base` names —
/// held a build with pre-leg-B behaviour while its mtime (11:28:54) was NEWER
/// than every source file in `src/` (newest 11:26:55). An mtime check would have
/// called it fresh and been wrong. `cargo test --no-run` did not correct it
/// either; only `cargo build --bin base --all-features` did.
///
/// Cargo builds the bin with one feature set for `cargo build` and another for
/// `cargo test` (the self-dev-dependency turns on `isolation-guard`), and both
/// uplift to that same path. Running one after the other can leave it holding
/// the other build.
///
/// Every test in this file then failed for a reason that had nothing to do with
/// the product, which is the same class as a mutation that does not mutate: a
/// test that runs against a stale binary is indistinguishable from a guard that
/// cannot fire. This turns that into a named refusal instead of a confusing red.
///
/// The marker is leg A's fault report, which is committed, pushed, and
/// independent of everything this file asserts: any `base` command run against
/// an unreadable config says so on stderr. A binary without it cannot possibly
/// have leg C.
fn assert_binary_contains_the_code_under_test() {
    let f = Fake::new(Some(&broken_registry()));
    let (_code, _out, err) = f.run(&["hooks", "manifest"]);
    assert!(
        err.contains("base is running on DEFAULT settings, not yours"),
        "STALE BINARY. {BIN} does not carry leg A's config-fault report, so it          cannot carry leg C either and nothing this file measures would mean          anything.

Rebuild it with:
    cargo build --bin base --all-features

         stderr was: {err}"
    );
}

/// Run first by name (tests are alphabetical within a file only by chance, so
/// this is also called from each test that depends on it).
#[test]
fn the_binary_under_test_is_not_stale() {
    assert_binary_contains_the_code_under_test();
}

// ─── The fixture has to be broken, or this whole file measures nothing ─────

/// Every test below starts from a registry that must FAIL to parse. If the
/// fixture ever parses, each of them silently becomes a test of the healthy
/// path — green, and proving nothing about the leg.
///
/// So the fixture asserts its own brokenness before anything relies on it. This
/// is the same rule as "a mutation that does not mutate is indistinguishable
/// from a guard that cannot fire", applied to a test fixture.
#[test]
fn the_broken_fixture_really_does_fail_to_parse() {
    let raw = broken_registry();
    let parsed = raw.parse::<toml::Value>();
    assert!(
        parsed.is_err(),
        "the fixture PARSED, so every other test in this file is measuring the          healthy path.
fixture bytes:
{raw}
parsed as: {parsed:?}"
    );
    // And it still plainly contains real registrations, which is exactly why
    // erasing them is data loss rather than an honest empty answer.
    assert!(raw.contains(REGISTERED[0]), "{raw}");
    assert!(raw.contains(REGISTERED[1]), "{raw}");
}

// ─── THE LEG ───────────────────────────────────────────────────────────────

/// `base workspace sync` against an UNPARSEABLE registry must refuse and touch
/// nothing. This is the leg, stated as a test.
#[test]
fn workspace_sync_refuses_rather_than_erasing_when_the_registry_will_not_parse() {
    assert_binary_contains_the_code_under_test();
    let f = Fake::new(Some(&broken_registry()));
    assert_registrations_present(&f, "precondition");
    let before = f.claude_md_bytes();

    let (code, out, err) = f.run(&["workspace", "sync"]);

    assert_ne!(code, 0, "a sync that could not read the registry has failed: {out}");
    assert_eq!(
        before,
        f.claude_md_bytes(),
        "CLAUDE.md must be BYTE-IDENTICAL. stderr: {err}"
    );
    assert_registrations_present(&f, "after a refused sync");
    assert!(
        err.contains("refusing to rewrite"),
        "and it has to say that it refused, and why: {err}"
    );
}

/// The same, for a registry that is present and cannot be READ. Unreadable and
/// unparseable are different states and each gets its own arm — leg A's law is
/// the same law here.
#[test]
fn workspace_sync_refuses_rather_than_erasing_when_the_registry_cannot_be_read() {
    assert_binary_contains_the_code_under_test();
    let f = Fake::with_unreadable_registry();
    assert_registrations_present(&f, "precondition");
    let before = f.claude_md_bytes();

    let (code, _out, err) = f.run(&["workspace", "sync"]);

    assert_ne!(code, 0, "an unreadable registry is a failure: {err}");
    assert_eq!(before, f.claude_md_bytes(), "CLAUDE.md byte-identical. stderr: {err}");
    assert_registrations_present(&f, "after a refused sync");
}

/// The OTHER reach point, and it needs its own test because it behaves
/// differently on purpose.
///
/// `base scaffold` calls `sync_claude_md_registry` as one step of nine and keeps
/// going afterwards — the write was refused, so the registry is already safe and
/// there is no reason to abandon the rest of the scaffold. What it must NOT do is
/// lose the refusal: it used to `println!` it into the middle of a progress list.
#[test]
fn scaffold_also_refuses_and_reports_the_refusal_on_stderr() {
    assert_binary_contains_the_code_under_test();
    let f = Fake::new(Some(&broken_registry()));
    assert_registrations_present(&f, "precondition");
    let before = f.claude_md_bytes();

    let target = f.root().join("newws");
    std::fs::create_dir_all(&target).unwrap();
    let (_code, out, err) = f.run(&["scaffold", target.to_str().unwrap()]);

    assert_eq!(
        before,
        f.claude_md_bytes(),
        "scaffold must not erase the registry either. stderr: {err}"
    );
    assert_registrations_present(&f, "after a refused scaffold sync");
    assert!(
        err.contains("refusing to rewrite"),
        "the refusal belongs on stderr, where it will not scroll past inside a \
         nine-step progress list. stdout was:\n{out}\nstderr was:\n{err}"
    );
}

// ─── The controls, without which none of the above proves anything ─────────

/// A readable registry still syncs, and all three registrations reach the block.
/// Without this arm, a "fix" that refused unconditionally would pass every test
/// above and break the feature completely.
#[test]
fn a_readable_registry_still_syncs_every_workspace() {
    assert_binary_contains_the_code_under_test();
    let f = Fake::new(Some(&healthy_registry()));
    let (code, out, err) = f.run(&["workspace", "sync"]);

    assert_eq!(code, 0, "a good registry syncs: {err}");
    assert!(out.contains("3 workspace"), "and reports what it wrote: {out}");
    assert_registrations_present(&f, "after a successful sync");
}

/// A registry that is READABLE and genuinely empty writes `(none registered)`,
/// and that is correct rather than data loss: the source of truth says nothing is
/// registered, so the generated block says nothing is registered.
///
/// This is the arm that makes the leg a real distinction instead of a blanket
/// refusal. "Readable and empty" and "unreadable" now produce opposite
/// behaviours, which is the entire point.
#[test]
fn a_readable_but_empty_registry_correctly_writes_none_registered() {
    assert_binary_contains_the_code_under_test();
    let f = Fake::new(Some("[update]\nauto = false\n"));
    let before = f.claude_md_bytes();

    let (code, _out, err) = f.run(&["workspace", "sync"]);

    assert_eq!(code, 0, "an empty registry is not a fault: {err}");
    assert_ne!(before, f.claude_md_bytes(), "the block is regenerated");
    let text = std::fs::read_to_string(f.claude_md()).unwrap();
    assert!(
        text.contains("(none registered)"),
        "a readable registry with no [[workspace]] entries says so: {text}"
    );
    // And the operator's own prose either side is still there, because only the
    // managed block may ever be rewritten.
    assert!(text.contains("This paragraph is the operator's own"), "{text}");
    assert!(text.contains("Also the operator's own."), "{text}");
}

/// An ABSENT `base.toml` also writes `(none registered)`, and that is honest for
/// the same reason: there is no registry, so nothing is registered.
///
/// Absent is deliberately NOT grouped with unreadable here, which is the same
/// distinction leg A's `ConfigFault` encodes by having no `Absent` variant.
#[test]
fn an_absent_registry_is_not_a_fault_and_writes_none_registered() {
    assert_binary_contains_the_code_under_test();
    let f = Fake::new(None);
    let (code, _out, err) = f.run(&["workspace", "sync"]);

    assert_eq!(code, 0, "a first run is not a fault: {err}");
    let text = std::fs::read_to_string(f.claude_md()).unwrap();
    assert!(text.contains("(none registered)"), "{text}");
}

/// The distinction, stated as one assertion rather than left implicit across six
/// tests: the SAME command, on a file that is readable-and-empty versus one that
/// is unreadable, must do opposite things.
#[test]
fn readable_and_empty_is_not_the_same_state_as_unreadable() {
    assert_binary_contains_the_code_under_test();
    let empty = Fake::new(Some("[update]\nauto = false\n"));
    let (empty_code, _, _) = empty.run(&["workspace", "sync"]);

    let broken = Fake::new(Some(&broken_registry()));
    let broken_before = broken.claude_md_bytes();
    let (broken_code, _, _) = broken.run(&["workspace", "sync"]);

    assert_eq!(empty_code, 0, "readable and empty: writes, succeeds");
    assert_ne!(broken_code, 0, "unreadable: refuses, fails");
    assert!(
        std::fs::read_to_string(empty.claude_md())
            .unwrap()
            .contains("(none registered)"),
        "the empty one wrote the honest answer"
    );
    assert_eq!(
        broken_before,
        broken.claude_md_bytes(),
        "the broken one wrote nothing at all"
    );
}

// ─── A count is not a loss ─────────────────────────────────────────────────

/// `✓ synced 0 workspace(s)` is FALSE when three registrations were just
/// removed. It reports a COUNT where the operator needs a LOSS, and the two
/// cases it cannot distinguish — "there were none" and "there were three and now
/// there are none" — are exactly the ones that matter.
///
/// This is the message half of the residual. Whether sync SHOULD clear a
/// populated block when `base.toml` is absent is a separate design question,
/// ruled out of this lane and logged as `N-SYNC-CLEARS-ON-ABSENT-REGISTRY`.
/// Honest reporting does not depend on settling that.
#[test]
fn a_sync_that_removes_registrations_reports_the_loss_not_a_count() {
    assert_binary_contains_the_code_under_test();
    // Absent registry, populated block: the case that produced the false line.
    let f = Fake::new(None);
    assert_registrations_present(&f, "precondition");

    let (code, out, err) = f.run(&["workspace", "sync"]);

    assert_eq!(code, 0, "absent is not a fault: {err}");
    assert!(
        out.contains("REMOVED") && out.contains('3'),
        "it has to say what it took away, and how many: {out}"
    );
    assert!(
        !out.contains("synced 0 workspace(s)"),
        "and must NOT report a bare count, which reads as 'there were none': {out}"
    );
}

/// The control. A sync that removes nothing keeps the plain message — otherwise
/// every ordinary sync would shout about a loss that did not happen.
#[test]
fn a_sync_that_removes_nothing_keeps_the_plain_message() {
    assert_binary_contains_the_code_under_test();
    let f = Fake::new(Some(&healthy_registry()));

    let (code, out, err) = f.run(&["workspace", "sync"]);

    assert_eq!(code, 0, "{err}");
    assert!(out.contains("synced 3 workspace(s)"), "{out}");
    assert!(
        !out.contains("REMOVED"),
        "nothing was removed, so nothing may claim it was: {out}"
    );
}

/// The placeholder must not be counted as a registration, or a block reading
/// `- (none registered)` would report a phantom loss of one on the next sync.
#[test]
fn the_none_registered_placeholder_counts_as_zero_not_one() {
    assert_binary_contains_the_code_under_test();
    let f = Fake::new(None);
    // First sync clears the three and writes the placeholder.
    let (_, out1, _) = f.run(&["workspace", "sync"]);
    assert!(out1.contains("REMOVED"), "precondition: the first sync cleared them: {out1}");

    // Second sync: the block now holds only the placeholder, so nothing is lost.
    let (code, out2, err) = f.run(&["workspace", "sync"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        !out2.contains("REMOVED"),
        "the placeholder is not a registration, so this sync removed nothing: {out2}"
    );
}

// ─── base scaffold: the command the #158 reporter actually ran ─────────────

/// THE measurement for this half of the leg, and it is the reporter's own
/// command. `base scaffold` reported SUCCESS over a refused registry sync: the
/// step printed nothing terminal on stdout, the banner said "✓ Workspace
/// scaffolded", and the process exited 0.
///
/// It needed TWO fixes and either alone leaves rc at 0. `scaffold::run` now
/// returns Err, AND the `cli.rs` Scaffold arm now calls `die` instead of
/// printing the error and falling through — that arm sits inside `run()`, which
/// returns `()`, so a bare `eprintln!` there is a normal exit. Same shape as the
/// twelve rc-0 branches leg B removed.
#[test]
fn scaffold_exits_non_zero_when_the_registry_sync_is_refused() {
    assert_binary_contains_the_code_under_test();
    let f = Fake::new(Some(&broken_registry()));
    let before = f.claude_md_bytes();

    let target = f.root().join("newws");
    std::fs::create_dir_all(&target).unwrap();
    let (code, out, err) = f.run(&["scaffold", target.to_str().unwrap()]);

    assert_ne!(
        code, 0,
        "a refused step is a failed command.
stdout:
{out}
stderr:
{err}"
    );
    assert!(
        !out.contains("✓ Workspace scaffolded"),
        "the banner is a claim about the WHOLE command and one step refused: {out}"
    );
    assert_eq!(before, f.claude_md_bytes(), "and the registry is still untouched");
}

/// The dangling line. Step 4b opens with a `print!` and no newline; the refusal
/// goes to stderr, so without a terminator on stdout the progress list reads
/// "sync CLAUDE.md registry ... " and step 5 continues ON THE SAME LINE. A
/// reader of stdout alone sees the step start and never finish.
#[test]
fn the_refused_step_still_terminates_its_own_stdout_line() {
    assert_binary_contains_the_code_under_test();
    let f = Fake::new(Some(&broken_registry()));
    let target = f.root().join("newws");
    std::fs::create_dir_all(&target).unwrap();

    let (_code, out, _err) = f.run(&["scaffold", target.to_str().unwrap()]);

    let line = out
        .lines()
        .find(|l| l.contains("sync CLAUDE.md registry"))
        .unwrap_or_else(|| panic!("step 4b did not appear on stdout at all:
{out}"));
    assert!(
        line.contains("REFUSED"),
        "the step has to close its own line before the next one starts: {line:?}"
    );
    assert!(
        !line.contains("5."),
        "step 5 must not be sharing this line: {line:?}"
    );
}

/// The control: an ordinary scaffold still succeeds and still exits 0. Without
/// it, a change that made scaffold always fail would pass both tests above.
#[test]
fn an_ordinary_scaffold_still_succeeds_and_exits_zero() {
    assert_binary_contains_the_code_under_test();
    let f = Fake::new(Some(&healthy_registry()));
    let target = f.root().join("newws");
    std::fs::create_dir_all(&target).unwrap();

    let (code, out, err) = f.run(&["scaffold", target.to_str().unwrap()]);

    assert_eq!(code, 0, "a good registry scaffolds fine.
stderr:
{err}");
    assert!(out.contains("✓ Workspace scaffolded"), "{out}");
    assert!(!out.contains("REFUSED"), "{out}");
}

/// C-10. The same defect as C-9, at step 4, on the path C-9 could NOT reach.
///
/// When `base.toml` is UNREADABLE, `register_workspace` fails at step 4 and the
/// `?` carries the error out — so step 4b never runs and C-9's terminator is
/// never exercised. That is the #158 reporter's own path, and it was left
/// reading "4. Register workspace ... " with the next output glued to the end.
///
/// Shipping C-9 while leaving this would mean the acceptance criterion still
/// fails on the case the row is actually about.
#[test]
fn the_aborting_step_four_also_terminates_its_own_stdout_line() {
    assert_binary_contains_the_code_under_test();
    let f = Fake::with_unreadable_registry();
    let target = f.root().join("newws");
    std::fs::create_dir_all(&target).unwrap();

    let (code, out, err) = f.run(&["scaffold", target.to_str().unwrap()]);

    assert_ne!(code, 0, "an unreadable registry is a failed scaffold: {err}");
    let line = out
        .lines()
        .find(|l| l.contains("Register workspace"))
        .unwrap_or_else(|| panic!("step 4 never appeared on stdout:
{out}"));
    assert!(
        line.contains("FAILED"),
        "step 4 must close its own line when it aborts: {line:?}"
    );
    // The real assertion: the line ENDS. `lines()` already split on the newline,
    // so reaching here with a found line proves a terminator followed it — but
    // pin the glue case explicitly, because that is the symptom.
    assert!(
        !line.contains("5."),
        "the next step must not be glued to this line: {line:?}"
    );
    assert!(
        out.ends_with(NEWLINE),
        "stdout must not end mid-line either: {out:?}"
    );
}

/// The control: an ordinary scaffold does not print step 4 as failed.
#[test]
fn step_four_reports_failure_only_when_it_fails() {
    assert_binary_contains_the_code_under_test();
    let f = Fake::new(Some(&healthy_registry()));
    let target = f.root().join("newws");
    std::fs::create_dir_all(&target).unwrap();

    let (code, out, _err) = f.run(&["scaffold", target.to_str().unwrap()]);

    assert_eq!(code, 0);
    let line = out.lines().find(|l| l.contains("Register workspace")).unwrap();
    assert!(!line.contains("FAILED"), "{line:?}");
}
