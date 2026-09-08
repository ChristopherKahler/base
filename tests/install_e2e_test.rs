//! End-to-end legs for #92 and #93, driven through the real binary.
//!
//! The unit legs in `install_scripts_source_test.rs` and `hooks_self_heal_test.rs`
//! prove the two seams. These prove the WIRING: that `install_scripts` actually
//! calls the candidate seam, and that `ensure_hooks_wired` actually has a caller
//! outside the hook pipeline. A seam nothing calls is the whole shape of #93.
//!
//! Platform scope: these spawn `base` as a child process, the shape that #129
//! (`STATUS_STACK_OVERFLOW`, debug profile only) crashes on Windows. Ubuntu CI is
//! the acceptance surface; `cargo test --release` corroborates on Windows.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The binary under test. Cargo builds it for this test target, so it is always
/// this tree's binary and never something left in the target dir by another
/// branch.
const BASE: &str = env!("CARGO_BIN_EXE_base");

/// This repo's real `scripts/ast/`, the directory the release archive stages.
fn repo_scripts_ast() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts").join("ast")
}

fn mkdir(p: impl AsRef<Path>) -> PathBuf {
    let p = p.as_ref().to_path_buf();
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Lay out what `tar xzf base-linux-x86_64.tar.gz -C <into>` leaves behind:
/// the binary and `scripts/ast/` as siblings. Returns the binary's path.
///
/// `requirements.txt` is deliberately NOT staged. `install_scripts` shells out
/// to `pip install -r` when it lands one, and this leg is about where the
/// scripts come from, not about pip.
fn unpack_fake_archive(into: &Path) -> PathBuf {
    mkdir(into);
    let binary = into.join(format!("base{}", std::env::consts::EXE_SUFFIX));
    // Hard link where the filesystem allows it: a debug binary is large and this
    // runs twice. Content is identical either way, which is all that matters.
    if std::fs::hard_link(BASE, &binary).is_err() {
        std::fs::copy(BASE, &binary).unwrap();
    }

    let dest = mkdir(into.join("scripts").join("ast"));
    let mut staged = 0;
    for entry in std::fs::read_dir(repo_scripts_ast()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "py") {
            std::fs::copy(&path, dest.join(path.file_name().unwrap())).unwrap();
            staged += 1;
        }
    }
    // Law 23: a loop that asserts over a set prints what it visited, and zero
    // visited is a failure. A silently empty archive would make every assertion
    // below meaningless.
    assert!(staged >= 2, "staged {staged} .py files into the fake archive");
    assert!(dest.join("onto_ast.py").is_file(), "the marker install probes for");
    binary
}

/// A skills source beside the archive, so `install_skills` resolves locally and
/// this test never reaches the network. `find_local_skills_dir` probes
/// `<binary>/../../claude/skills` first, which is `<parent of archive>`.
fn seed_local_skill(archive_parent: &Path) {
    let dir = mkdir(archive_parent.join("claude").join("skills").join("base-help"));
    std::fs::write(dir.join("SKILL.md"), "---\nname: base-help\n---\n").unwrap();
}

/// Windows `STATUS_STACK_OVERFLOW` (`0xC00000FD`) as a process exit code. This
/// is #129, and it is debug-profile only — tern measured `ovf=0` on every one of
/// four targets with a release arm.
const STATUS_STACK_OVERFLOW: i32 = 0xC000_00FD_u32 as i32;

/// Run `base` with an explicit environment. Nothing is inherited implicitly:
/// `BASE_HOME` is the only tier this touches, and `HOME` is left alone so the
/// write tripwire stays armed (it compares against the real profile).
///
/// The crash check is not politeness. When #129 kills the child at startup it
/// writes nothing, so an assertion on an ABSENCE — "stdout does not mention
/// hooks", "settings.json was not created" — passes for the wrong reason, and
/// an assertion on a presence fails with a message about the product rather
/// than about the corpse. Law 24: a leg whose "could not run" and "failed"
/// states share a shape is not a leg. Measured 2026-09-08: all seven legs in
/// this file died this way on Windows debug, and one of them reported PASS.
fn run_base(binary: &Path, cwd: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    let out = Command::new(binary)
        .args(args)
        .current_dir(cwd)
        .env("BASE_HOME", home)
        .env("BASE_NO_AUTO_UPDATE", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|e| panic!("spawning {} {args:?}: {e}", binary.display()));

    assert_ne!(
        out.status.code(),
        Some(STATUS_STACK_OVERFLOW),
        "VOID, not a result: the child died with STATUS_STACK_OVERFLOW before it ran \
         anything (#129, debug profile on Windows). Ubuntu CI is this file's acceptance \
         surface; `cargo test --release` corroborates locally.\nargs: {args:?}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

// ─── #92 ────────────────────────────────────────────────────

#[test]
fn an_unpacked_archive_installs_the_ast_scripts_from_an_unrelated_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("unpacked");
    let binary = unpack_fake_archive(&archive);
    seed_local_skill(tmp.path());

    let home = mkdir(tmp.path().join("home"));
    // The reproduction from the issue: run the unpacked binary from anywhere
    // that is not the unpack directory.
    let elsewhere = mkdir(tmp.path().join("elsewhere"));

    let out = run_base(&binary, &elsewhere, &home, &["install", "--no-starter-commands"]);
    let installed = home.join(".base-gbl").join("scripts").join("ast");

    assert!(
        installed.join("extractor.py").is_file(),
        "AST extraction is silently absent.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(installed.join("onto_ast.py").is_file());
}

#[test]
fn the_same_archive_run_from_its_own_directory_also_installs_them() {
    // The control. Before the fix this arm passed while the one above failed,
    // which is the only reason either reading means anything: same script, same
    // binary, one variable — the working directory.
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("unpacked");
    let binary = unpack_fake_archive(&archive);
    seed_local_skill(tmp.path());

    let home = mkdir(tmp.path().join("home"));

    let out = run_base(&binary, &archive, &home, &["install", "--no-starter-commands"]);
    assert!(
        home.join(".base-gbl").join("scripts").join("ast").join("extractor.py").is_file(),
        "the cwd candidate still answers.\nstdout:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

// ─── #93 ────────────────────────────────────────────────────

#[test]
fn an_ordinary_command_wires_the_hooks_when_claude_code_arrived_later() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    // The state #93 describes, after Claude Code has been installed: base's
    // global tier exists, `~/.claude` exists, and settings.json does not,
    // because hook wiring was skipped at install time.
    mkdir(home.join(".base-gbl"));
    mkdir(home.join(".claude"));
    let settings = home.join(".claude").join("settings.json");
    assert!(!settings.exists(), "the precondition, stated rather than assumed");

    let cwd = mkdir(tmp.path().join("cwd"));
    let out = run_base(Path::new(BASE), &cwd, &home, &["recall", "--keyword", "anything"]);

    assert!(
        settings.is_file(),
        "an ordinary command left base inert.\nstatus:{:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&settings).unwrap();
    for (event, cmd) in base::install::HOOK_TABLE {
        assert_eq!(text.matches(cmd).count(), 1, "{event}: wired exactly once");
    }

    // And it does it silently: `base recall` must not grow a line of install
    // chatter. This assertion is on an ABSENCE, so it only means anything
    // beside the presence assertion above — on its own it would pass just as
    // well against a process that printed nothing because it never ran.
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("[hooks]"),
        "the repair announced itself on an ordinary command:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn the_session_start_hook_still_announces_what_it_wired() {
    // The control for excluding `Commands::Hook` from the CLI-level call. The
    // hook path owns the `[hooks]` notice; if the CLI seam consumed the wiring
    // first, this notice would silently disappear and nothing else would fail.
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    mkdir(home.join(".base-gbl"));
    mkdir(home.join(".claude"));

    // An install from before the Stop hook existed: four of the five.
    let four: Vec<String> = base::install::HOOK_TABLE
        .iter()
        .filter(|(event, _)| *event != "Stop")
        .map(|(event, cmd)| format!(r#""{event}":[{{"hooks":[{{"type":"command","command":"{cmd}"}}]}}]"#))
        .collect();
    assert_eq!(four.len(), 4, "seeded four of the five");
    std::fs::write(
        home.join(".claude").join("settings.json"),
        format!("{{\"hooks\":{{{}}}}}", four.join(",")),
    )
    .unwrap();

    let cwd = mkdir(tmp.path().join("cwd"));
    let out = run_base(Path::new(BASE), &cwd, &home, &["hook", "session-start"]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("[hooks] wired base hook Stop"),
        "the session-start notice named nothing.\nstdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ─── #93, the reproduction in the issue ─────────────────────

#[test]
fn one_install_into_a_home_with_no_claude_code_still_leaves_the_hooks_wired() {
    // The issue's own repro: "Install base into a home with no ~/.claude, then
    // install Claude Code, then start a session."
    //
    // That session finds base's hooks or it finds nothing — there is no third
    // path, because without settings.json base does not run inside a session at
    // all. Step 3 cannot wire (no directory yet); steps 7 and 8 then create
    // ~/.claude for base's own skill and CLAUDE.md. The wiring is deferred to
    // after them rather than abandoned.
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("unpacked");
    let binary = unpack_fake_archive(&archive);
    seed_local_skill(tmp.path());

    let home = mkdir(tmp.path().join("home"));
    assert!(!home.join(".claude").exists(), "no Claude Code, stated not assumed");

    let out = run_base(&binary, &archive, &home, &["install", "--no-starter-commands"]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    // Step 3 still says what it saw. Losing that message would trade one silent
    // state for another.
    assert!(
        stdout.contains("is Claude Code installed?"),
        "step 3 stopped reporting what it found.\nstdout:\n{stdout}"
    );

    let settings = home.join(".claude").join("settings.json");
    assert!(
        settings.is_file(),
        "the install left nothing for a later Claude Code to read.\nstdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&settings).unwrap();
    for (event, cmd) in base::install::HOOK_TABLE {
        assert_eq!(text.matches(cmd).count(), 1, "{event}: wired exactly once");
    }
}

#[test]
fn skip_hooks_still_means_skip_hooks_even_after_base_creates_the_directory() {
    // The control that separates "the wiring is deferred" from "the wiring is
    // now unconditional". Without it the fix above could pass by quietly
    // breaking --skip-hooks, and nothing else would fail.
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("unpacked");
    let binary = unpack_fake_archive(&archive);
    seed_local_skill(tmp.path());

    let home = mkdir(tmp.path().join("home"));
    let out = run_base(
        &binary,
        &archive,
        &home,
        &["install", "--no-starter-commands", "--skip-hooks"],
    );

    // base still creates ~/.claude for its own skill, so the directory existing
    // is not the thing under test — settings.json is.
    assert!(
        home.join(".claude").is_dir(),
        "base created its own skill directory regardless.\nstdout:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !home.join(".claude").join("settings.json").exists(),
        "--skip-hooks wrote hooks anyway.\nstdout:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}
