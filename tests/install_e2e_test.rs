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

    // An install from before the Stop hook existed, so before SessionEnd (BO-17) too: four of the six.
    let four: Vec<String> = base::install::HOOK_TABLE
        .iter()
        .filter(|(event, _)| *event != "Stop" && *event != "SessionEnd")
        .map(|(event, cmd)| format!(r#""{event}":[{{"hooks":[{{"type":"command","command":"{cmd}"}}]}}]"#))
        .collect();
    assert_eq!(four.len(), 4, "seeded four of the six");
    std::fs::write(
        home.join(".claude").join("settings.json"),
        format!("{{\"hooks\":{{{}}}}}", four.join(",")),
    )
    .unwrap();

    let cwd = mkdir(tmp.path().join("cwd"));
    let out = run_base(Path::new(BASE), &cwd, &home, &["hook", "session-start"]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("[hooks] wired base hook Stop, SessionEnd"),
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

#[test]
fn skip_hooks_holds_when_claude_code_is_already_installed() {
    // The shape the leg above CANNOT reach, and the only one an ordinary user is
    // ever in: Claude Code got here first, so `~/.claude` exists BEFORE base runs.
    //
    // `cli::run` calls `ensure_hooks_wired` once, before it dispatches to any
    // subcommand. A seam that excludes only `Commands::Hook` therefore reaches
    // `install` too: with a config tier already present it wires all five hooks
    // and stamps the version, and step 3's `⊘ Hook wiring skipped (--skip-hooks)`
    // then prints over work that has already happened. The leg above passes
    // against that bug, because its home has no `~/.claude` at entry and the seam
    // returns empty for a reason that has nothing to do with the flag.
    //
    // Found by raven reading the tree, not by any test here. Law 31: a guard is
    // only proven against the shapes the CODEBASE has, never against its own.
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("unpacked");
    let binary = unpack_fake_archive(&archive);
    seed_local_skill(tmp.path());

    let home = mkdir(tmp.path().join("home"));
    mkdir(home.join(".claude"));
    // The stamp needs somewhere to land, or a write that "did not happen" may
    // only have failed, and this leg would pass without proving anything.
    mkdir(home.join(".base-gbl"));

    let settings = home.join(".claude").join("settings.json");
    let stamp = home
        .join(".base-gbl")
        .join(base::install::hooks_wired_stamp());
    assert!(!settings.exists(), "the precondition, stated rather than assumed");
    assert!(!stamp.exists(), "and this version is unstamped, which is what arms the seam");

    let out = run_base(
        &binary,
        &archive,
        &home,
        &["install", "--no-starter-commands", "--skip-hooks"],
    );

    assert!(
        !settings.exists(),
        "--skip-hooks wrote hooks anyway, into a home that already had ~/.claude.\nstdout:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !stamp.exists(),
        "--skip-hooks stamped the version, which disarms the repair for the rest of it.\nstdout:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn uninstall_does_not_create_a_settings_file_on_its_way_to_emptying_one() {
    // The same shape as the leg above, one command over, and found by sweeping
    // all 39 `Commands` variants against every caller of the two settings.json
    // mutators rather than by trusting a list. Exactly three commands own hook
    // state: `Hook` (session_start), `Install` (step 3 and the deferred block)
    // and `Uninstall` (`remove_hooks`, whose only caller is `install::uninstall`).
    //
    // `~/.claude/` exists and `settings.json` does not — a Claude Code install
    // that has never been configured. The CLI seam runs before the dispatch, so
    // an exclusion list missing `Uninstall` lets `wire_hooks_quiet` take the
    // "parent is a dir" branch, CREATE the file, wire five hooks and stamp the
    // version — and `remove_hooks` then strips them and reports that it removed
    // hooks from settings.json. The user asked base to leave, and it left a
    // config file behind that was never there.
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("unpacked");
    let binary = unpack_fake_archive(&archive);

    let home = mkdir(tmp.path().join("home"));
    mkdir(home.join(".claude"));
    mkdir(home.join(".base-gbl"));

    let settings = home.join(".claude").join("settings.json");
    let stamp = home
        .join(".base-gbl")
        .join(base::install::hooks_wired_stamp());
    assert!(!settings.exists(), "the precondition, stated rather than assumed");
    assert!(!stamp.exists(), "and this version is unstamped, which is what arms the seam");

    let out = run_base(&binary, &archive, &home, &["uninstall"]);

    assert!(
        !settings.exists(),
        "uninstall created a settings.json that was never there.\nstdout:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !stamp.exists(),
        "uninstall stamped the hooks version on its way out.\nstdout:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn a_second_install_over_the_first_leaves_settings_json_byte_identical() {
    // Law 22: an installer gets a first-run leg AND an over-the-top-of-an-
    // existing-install leg, and the assertion is on CONTENT, not presence.
    // Every other leg in this file uses a fresh home, so none of them would
    // notice a re-install that duplicated the hook entries or rewrote the file.
    //
    // The assertion is byte identity rather than "the hooks are still there":
    // `wire_hooks_quiet` returns early when every command in HOOK_TABLE is
    // already present, so a correct second install must not touch the file at
    // all. A version that re-serialised it — same hooks, reordered keys — would
    // pass a presence check and fail this one.
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("unpacked");
    let binary = unpack_fake_archive(&archive);
    seed_local_skill(tmp.path());

    let home = mkdir(tmp.path().join("home"));
    mkdir(home.join(".claude"));
    let settings = home.join(".claude").join("settings.json");

    let first = run_base(&binary, &archive, &home, &["install", "--no-starter-commands"]);
    assert!(
        settings.is_file(),
        "the first install did not wire anything, so the second proves nothing.\nstdout:\n{}",
        String::from_utf8_lossy(&first.stdout)
    );
    let before = std::fs::read(&settings).unwrap();
    for (event, cmd) in base::install::HOOK_TABLE {
        let text = String::from_utf8_lossy(&before);
        assert_eq!(text.matches(cmd).count(), 1, "{event}: wired exactly once by the first run");
    }

    let second = run_base(&binary, &archive, &home, &["install", "--no-starter-commands"]);
    // Byte identity is an assertion on an ABSENCE of change, so it passes just
    // as well against a second install that died at step 1 and never reached the
    // wiring. Prove it ran the whole way first.
    let stdout = String::from_utf8_lossy(&second.stdout).into_owned();
    assert!(
        stdout.contains("Install complete"),
        "the second install did not finish, so byte identity proves nothing.\n\
         status:{:?}\nstdout:\n{stdout}\nstderr:\n{}",
        second.status.code(),
        String::from_utf8_lossy(&second.stderr)
    );

    let after = std::fs::read(&settings).unwrap();

    assert_eq!(
        before.len(),
        after.len(),
        "the second install changed settings.json's length.\nstdout:\n{}",
        String::from_utf8_lossy(&second.stdout)
    );
    assert!(
        before == after,
        "the second install rewrote settings.json.\nbefore:\n{}\nafter:\n{}",
        String::from_utf8_lossy(&before),
        String::from_utf8_lossy(&after)
    );
}

// ─── BO-15: the CORRECTED line, C3 for every user ───────────

/// Example 5. `base install` offers one line for the user's CLAUDE.md so the AI marks a corrected reply. Yes (here
/// `--corrections-line`, the unattended answer) puts it just above the BASE CLI section, outside the span a section
/// refresh rewrites, and a second install leaves one copy. No (`--no-corrections-line`) writes nothing and records the
/// answer, and session start carries the line instead. With no terminal and no flag nothing is written or recorded. A
/// CLAUDE.md that already asks for a marker (Chris's T2 holds `UPDATED:`, D10) is never given the line.
#[test]
fn install_offers_corrected_line() {
    const LINE: &str = "When the user corrects you, start that reply with \"CORRECTED: <what you got wrong>\".";
    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("unpacked");
    let binary = unpack_fake_archive(&archive);
    seed_local_skill(tmp.path());
    let claude_md = |home: &Path| std::fs::read_to_string(home.join(".claude").join("CLAUDE.md")).unwrap_or_default();
    let answer = |home: &Path| std::fs::read_to_string(home.join(".base-gbl").join(".corrections-line")).ok();
    let shown = |out: &std::process::Output| {
        format!("stdout:\n{}\nstderr:\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
    };

    // Yes.
    let yes = mkdir(tmp.path().join("yes"));
    let out = run_base(&binary, &archive, &yes, &["install", "--no-starter-commands", "--corrections-line"]);
    let md = claude_md(&yes);
    let line_at = md.find(LINE).unwrap_or_else(|| panic!("the line was not written\n{md}\n{}", shown(&out)));
    let section_at = md.find("## BASE CLI").expect("the BASE CLI section");
    assert!(line_at < section_at, "the line sits above the section, outside what a refresh rewrites:\n{md}");
    assert_eq!(answer(&yes).as_deref().map(str::trim), Some("added"));
    let again = run_base(&binary, &archive, &yes, &["install", "--no-starter-commands", "--corrections-line"]);
    assert_eq!(claude_md(&yes).matches(LINE).count(), 1, "one copy after a second install\n{}", shown(&again));

    // No: nothing written, the answer kept, and session start carries the line.
    let no = mkdir(tmp.path().join("no"));
    let out = run_base(&binary, &archive, &no, &["install", "--no-starter-commands", "--no-corrections-line"]);
    assert!(!claude_md(&no).contains(LINE), "declined, yet written\n{}", shown(&out));
    assert_eq!(answer(&no).as_deref().map(str::trim), Some("declined"));
    let start = run_base(&binary, &mkdir(no.join("work")), &no, &["hook", "session-start"]);
    assert!(
        String::from_utf8_lossy(&start.stdout).contains(LINE),
        "a declined user's session start carries the line\n{}",
        shown(&start)
    );

    // No terminal, no flag: nothing written, nothing recorded.
    let quiet = mkdir(tmp.path().join("quiet"));
    let out = run_base(&binary, &archive, &quiet, &["install", "--no-starter-commands"]);
    assert!(!claude_md(&quiet).contains(LINE), "written without an answer\n{}", shown(&out));
    assert_eq!(answer(&quiet), None, "recorded without an answer");

    // Covered: Chris's T2 asks for UPDATED:, so the line is never added (D10).
    let t2 = mkdir(tmp.path().join("t2"));
    mkdir(t2.join(".claude"));
    std::fs::write(t2.join(".claude").join("CLAUDE.md"), "When you change position, print UPDATED: <why>.\n").unwrap();
    let out = run_base(&binary, &archive, &t2, &["install", "--no-starter-commands", "--corrections-line"]);
    assert!(!claude_md(&t2).contains(LINE), "a CLAUDE.md that asks for a marker was given the line\n{}", shown(&out));
    assert!(String::from_utf8_lossy(&out.stdout).contains("already asks"), "{}", shown(&out));
    let start = run_base(&binary, &mkdir(t2.join("work")), &t2, &["hook", "session-start"]);
    assert!(!String::from_utf8_lossy(&start.stdout).contains(LINE), "covered, yet carried\n{}", shown(&start));
}
