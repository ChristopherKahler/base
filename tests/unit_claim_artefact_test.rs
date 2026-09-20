//! THE ARTEFACTS BASE PRINTS AND WRITES MUST NOT CARRY A FALSIFIED CLAIM.
//!
//! WHY THIS TESTS ARTEFACTS AND NOT SOURCE. The claim that the host counts UTF-16 units was
//! disproved on 2026-09-20 and corrected where it was found. It had NINE copies. The count went
//! 1 to 2 to 6 to 7 to 8 to 9 across three seats in one day, and every single increment came from
//! WIDENING THE ARTEFACT CLASS, never from looking harder at the class already in view:
//!
//!   1. `emit/mod.rs`      a source comment, corrected the morning the claim was disproved
//!   2. `config.rs:854`    a second source comment, found hours later on the struct itself
//!   3. `signal/memory.rs` REWORDED, NOT DELETED - see below, this one is half true
//!   4. `tests/seed`       the doc on `units()`, false in both of its halves
//!   5. `scaffold.rs`      a STRING LITERAL written into every new workspace's base.toml
//!   6. `install.rs`       a STRING LITERAL written on every install
//!   7. `doctor.rs:763`    a RUNTIME FORMAT STRING printed on the diagnostic screen
//!   8. `qa.md:541`        a SHIPPED DOC FILE copied to the operator's ~/.claude/skills
//!   9. `hook/mod.rs:295`  a runtime notice naming a LEGACY CONFIG KEY as the one to edit
//!
//! A test aimed at one kind of artefact finds one kind of copy. Five were invisible to whoever
//! fixed the first. The eighth was invisible to the test designed in response to the seventh,
//! because that test was scoped to the artefacts its authors had just been looking at.
//!
//! SO EVERY LEG HERE OBSERVES THE SHIPPING CHANNEL: what a child `base` process PRINTED, or what
//! it LEFT ON DISK. No leg calls an internal function. A private call and the stdout an operator
//! reads are two channels in one process, and two reads of the private one corroborate each other
//! exactly as falsely as two reads of the wrong process.
//!
//! AND EVERY LEG THAT WALKS A FILE SET PRINTS THE SIZE OF THE SET IT VISITED. A visited count of
//! zero is a FAIL and never a pass: it means the leg proved nothing, which is not the same as
//! finding nothing. "Grep what install writes" is a rule. "Walk every path install wrote, print
//! the count, grep each" is a gate.

mod seed;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use base::config::RENAMED_BUDGET_KEYS;

// ─────────────────────────────────────────────────────────────────────────────
// THE MATCHERS
// ─────────────────────────────────────────────────────────────────────────────

/// Host tokens: the words that turn a mention of a unit into a CLAIM ABOUT THE HOST.
const HOST_TOKENS: &[&str] = &["Claude Code", "the host", "its limit", "the unit the host"];

/// How far either side of a `UTF-16` mention M1 looks for a host token, in bytes of collapsed text.
///
/// Sized from the widest real instance rather than guessed: `install.rs:547-548` reads
/// "in UTF-16 / # units (the unit Claude Code counts)" with a line break and a comment marker in
/// between, which is 30 characters of collapsed text before the host token begins.
const WINDOW: usize = 80;

/// Whitespace runs collapsed to one space.
///
/// NOT COSMETIC. Two of the nine copies WRAP A LINE - `config.rs:854` breaks between "limit" and
/// "counts", `install.rs:547` between "UTF-16" and "units (the unit Claude Code counts)". A
/// line-oriented matcher misses both, and would have reported the tree clean over two live copies.
fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// M1 - ATTRIBUTION. Every place the text puts `UTF-16` within `WINDOW` of a host token.
///
/// WHY THIS IS NOT A BARE GREP FOR `UTF-16`, AND WHY THAT MATTERS MORE THAN IT SOUNDS.
/// `emit::record::Unit::label` legitimately returns the string "UTF-16 units" as a PER-ROW unit
/// label, and printing it is the no-coercion ruling working: rows written before 2026-09-20 hold
/// UTF-16 units and say so, rows since hold bytes and say so. A matcher keyed on the bare string
/// goes RED on a correct tree, and a test that cries wolf on correct code gets disabled - which
/// would leave the nine copies with no gate at all.
fn m1_attribution(text: &str) -> Vec<String> {
    let flat = collapse(text);
    let lower = flat.to_lowercase();
    let mut hits = Vec::new();
    let mut from = 0;
    while let Some(rel) = lower[from..].find("utf-16") {
        let at = from + rel;
        let lo = at.saturating_sub(WINDOW);
        let hi = (at + "utf-16".len() + WINDOW).min(flat.len());
        // Character boundaries: the captures carry box-drawing characters.
        let lo = (lo..=at).find(|i| flat.is_char_boundary(*i)).unwrap_or(at);
        let hi = (at..=hi).rev().find(|i| flat.is_char_boundary(*i)).unwrap_or(at);
        let window = &flat[lo..hi];
        if HOST_TOKENS.iter().any(|t| window.contains(t)) {
            hits.push(window.to_string());
        }
        from = at + "utf-16".len();
    }
    hits
}

/// M2 - SOLE-UNIT HEADING. A section heading may not name a unit when the rows beneath it carry
/// their own.
///
/// M1 CANNOT REACH THE SEVENTH COPY AND THAT IS THE WHOLE REASON THIS EXISTS. `doctor.rs:763`
/// prints "hook output (UTF-16 units, measured before printing)" and NAMES NO HOST AT ALL, so no
/// attribution rule can see it. It asserts one unit for every row beneath it - and the row directly
/// underneath reads "6868 of 9000 bytes". The heading contradicts its own rows on adjacent lines,
/// on the operator-facing diagnostic screen, which is the one surface whose entire job is reporting
/// what was measured.
///
/// It is the worst-placed of the nine for a reason worth keeping: the per-row labels are CORRECT
/// because of the no-coercion ruling, and a reader's eye passes the heading first, so the correct
/// labels get read through a false frame. A correct measurement under a wrong heading is not a
/// correct report.
fn m2_sole_unit_heading(text: &str) -> Vec<String> {
    const UNIT_TOKENS: &[&str] = &["UTF-16", "utf-16", "bytes", "chars", "code units"];
    text.lines()
        .filter(|l| l.contains("hook output"))
        .filter(|l| UNIT_TOKENS.iter().any(|u| l.contains(u)))
        .map(|l| l.trim().to_string())
        .collect()
}

/// M3 - NO ARTEFACT NAMES A KEY THE TREE ITSELF CALLS LEGACY.
///
/// KEYED ON `RENAMED_BUDGET_KEYS`, NEVER ON A TYPED LIST, AND THAT IS THE POINT. A hand-written
/// list of "prompt_chars" and "session_start_chars" is correct today and silently blind at the next
/// rename - which is the identical failure this whole file exists to stop. Reading the table the
/// production code reads means a key renamed next month is covered without anyone remembering.
///
/// THE ONE EXEMPTION, AND IT IS NARROW: the deprecation warning itself names the old spelling,
/// because naming it is its entire job. Any OTHER artefact naming it is base pointing an operator
/// at a key base will then warn them about.
fn m3_legacy_key_named(text: &str) -> Vec<String> {
    let mut hits = Vec::new();
    for line in text.lines() {
        // The deprecation warning is the one place a legacy spelling belongs.
        if line.contains("was renamed to") {
            continue;
        }
        for k in RENAMED_BUDGET_KEYS {
            if line.contains(k.old) {
                hits.push(format!("names legacy key `{}`: {}", k.old, line.trim()));
            }
        }
    }
    hits
}

// ─────────────────────────────────────────────────────────────────────────────
// CONTROLS - every leg proves it can SEE before it is allowed to report
// ─────────────────────────────────────────────────────────────────────────────

/// A leg that cannot see REFUSES. It never prints a count.
///
/// A zero is the only result that cannot distinguish "the thing is not there" from "I cannot see",
/// so absence never stands on its own here. On a failed control this panics naming the control, and
/// deliberately reports NO counts: a grader that prints numbers beside a broken anchor has
/// laundered blindness into data.
fn control(leg: &str, name: &str, present: bool) {
    assert!(
        present,
        "{leg}: POSITIVE CONTROL `{name}` DID NOT MATCH, so this leg proved nothing. \
         Refusing rather than reporting a clean result. No counts printed."
    );
}

/// Everything the three matchers found, formatted for one assertion.
fn scan(leg: &str, artefact: &str, text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for h in m1_attribution(text) {
        out.push(format!("{leg} · {artefact} · M1 attribution · ...{h}..."));
    }
    for h in m2_sole_unit_heading(text) {
        out.push(format!("{leg} · {artefact} · M2 sole-unit heading · {h}"));
    }
    for h in m3_legacy_key_named(text) {
        out.push(format!("{leg} · {artefact} · M3 · {h}"));
    }
    out
}

/// Every file under `root`, with its text. Directories walked depth-first; unreadable and binary
/// files are COUNTED AND NAMED rather than skipped in silence, because a file this cannot read is
/// a hole in the very claim the leg is making.
fn walk_text(root: &Path) -> (Vec<(PathBuf, String)>, Vec<PathBuf>) {
    let mut text = Vec::new();
    let mut unreadable = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            unreadable.push(dir);
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                match std::fs::read_to_string(&p) {
                    Ok(s) => text.push((p, s)),
                    Err(_) => unreadable.push(p),
                }
            }
        }
    }
    (text, unreadable)
}

// ─────────────────────────────────────────────────────────────────────────────
// THE MATCHERS ARE PROVEN BEFORE THEY ARE TRUSTED
// ─────────────────────────────────────────────────────────────────────────────

/// Each matcher FIRES on the claim and stays SILENT on the legitimate line it most resembles.
///
/// EXISTING, REACHING AND FIRING ARE THREE DIFFERENT THINGS AND ONLY FIRING IS EVIDENCE. A
/// positive control alone cannot detect a matcher that is merely too loose, so each arm below
/// pairs a must-fire case with the benign case nearest to it.
#[test]
fn the_matchers_fire_on_the_claim_and_not_on_the_truth() {
    // M1 must fire, including across a line break - two of the nine copies wrap.
    assert!(
        !m1_attribution("what a hook may print, in UTF-16 units (Claude Code's unit)").is_empty(),
        "M1 did not fire on the scaffold.rs wording"
    );
    assert!(
        !m1_attribution("about to print, in UTF-16\n# units (the unit Claude Code counts)").is_empty(),
        "M1 did not fire across a line break, so it would miss config.rs:854 and install.rs:547"
    );

    // BENIGN CONTROL. A legitimate per-row unit label, the exact shape a bare grep destroys.
    assert!(
        m1_attribution("workspace tier · session-start: largest 8552 of 9000 UTF-16 units at 06:15")
            .is_empty(),
        "M1 fired on a legitimate per-row label; a matcher that reddens a correct tree gets disabled"
    );

    // M2 must fire on a heading that names a unit, and stay silent on one that does not.
    assert!(
        !m2_sole_unit_heading("─── hook output (UTF-16 units, measured before printing) ───").is_empty(),
        "M2 did not fire on the doctor.rs:763 heading"
    );
    assert!(
        m2_sole_unit_heading("─── hook output (measured before printing) ───").is_empty(),
        "M2 fired on a heading that names no unit, which is the fixed shape"
    );

    // M3 must fire on a legacy key named outside the warning, and stay silent inside it.
    let notice = "[base: user-prompt-submit withheld 8782 bytes against [budget] prompt_chars = 4000.";
    assert!(
        !m3_legacy_key_named(notice).is_empty(),
        "M3 did not fire on the overflow notice naming a legacy key"
    );
    assert!(
        m3_legacy_key_named(
            "base: [budget] prompt_chars was renamed to prompt_bytes when its unit changed."
        )
        .is_empty(),
        "M3 fired on the deprecation warning, whose whole job is to name the old spelling"
    );

    // AND THE PAIR IS A PAIR: no single input fires both M1 and M2. If one did, they would be one
    // rule written twice, and the second would be proving nothing the first did not already.
    let seventh = "─── hook output (UTF-16 units, measured before printing) ───";
    assert!(
        m1_attribution(seventh).is_empty() && !m2_sole_unit_heading(seventh).is_empty(),
        "the seventh copy must be reachable by M2 ALONE - it names no host, so if M1 also fires \
         the two rules are not independent and M2 is untested"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// L1 - WHAT BASE PRINTS
// ─────────────────────────────────────────────────────────────────────────────

fn root_for(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("base-artefact-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    p
}

/// A fixture LOUD ENOUGH TO OVERFLOW. Without it the budget notice never fires and the leg is
/// vacuous - which already happened once on this tree, an isolated session start emitting zero
/// bytes against a 3,000-byte budget and being read as a pass.
fn loud(tag: &str) -> seed::Seed {
    let root = root_for(tag);
    let s = seed::write(&root, &seed::REAL, "[bracket]\nenabled = true\n");
    let ws_base = s.ws.join(".base");
    std::fs::create_dir_all(&ws_base).expect("workspace .base");
    let mut toml = String::from("[[domain]]\nname = \"global\"\nmode = \"always\"\nrules = [\n");
    for i in 0..15 {
        toml.push_str(&format!(
            "  \"Global rule {i}: deliberately long enough to overflow a four thousand byte \
             budget, because a budget test whose fixture fits inside the budget proves nothing \
             about what happens when it does not.\",\n"
        ));
    }
    toml.push_str("]\n");
    std::fs::write(ws_base.join("domains.toml"), toml).expect("domains.toml");
    s
}

/// L1a - what `base doctor` PRINTS. Carries the seventh copy.
#[test]
fn l1a_what_doctor_prints_carries_no_claim() {
    let s = seed::write(&root_for("doctor"), &seed::REAL, "");
    // Give doctor a hook record to report on, so the hook-output section exists to be read.
    let _ = seed::run_session_start(&s, Some("artefact-l1a"));
    let (_, out, err) = seed::run_base(&s, &["doctor"]);
    let text = format!("{out}\n{err}");

    control("L1a", "doctor printed a hook output section", text.contains("hook output"));

    let hits = scan("L1a", "base doctor stdout", &text);
    assert!(hits.is_empty(), "L1a found {} claim(s):\n{}", hits.len(), hits.join("\n"));
}

/// L1b - what the SESSION-START hook prints, driven over budget.
#[test]
fn l1b_what_session_start_prints_carries_no_claim() {
    let s = loud("session-start");
    let (_, out, err) = seed::run_session_start(&s, Some("artefact-l1b"));
    let text = format!("{out}\n{err}");

    control("L1b", "the hook printed something", !text.trim().is_empty());

    let hits = scan("L1b", "session-start stdout", &text);
    assert!(hits.is_empty(), "L1b found {} claim(s):\n{}", hits.len(), hits.join("\n"));
}

/// L1c - what the USER-PROMPT-SUBMIT hook prints when it OVERFLOWS. Carries the ninth copy.
///
/// THE NINTH IS THE ONLY ONE OF THE NINE THAT IS ADVICE. The notice reads "against [budget]
/// prompt_chars = 4000. Raise it in base.toml" - and `prompt_chars` is the LEGACY spelling. An
/// operator who does exactly what base tells them to do sets the legacy key, and base then warns
/// them to rename it. BASE DIRECTS THE OPERATOR INTO THE EXACT STATE IT THEN SCOLDS THEM FOR.
///
/// A stale value sits there. Advice gets acted on.
#[test]
fn l1c_the_overflow_notice_names_no_legacy_key() {
    let s = loud("prompt");
    let (_, out, err) = seed::run_prompt_submit(&s, "an ordinary prompt", Some("artefact-l1c"));
    let text = format!("{out}\n{err}");

    // THE CONTROL THAT MATTERS MOST IN THIS FILE. If the fixture did not overflow, the notice never
    // fired, and every assertion below passes over text that could not have contained the defect.
    control("L1c", "the budget notice fired (the word `withheld`)", text.contains("withheld"));

    let hits = scan("L1c", "user-prompt-submit stdout", &text);
    assert!(hits.is_empty(), "L1c found {} claim(s):\n{}", hits.len(), hits.join("\n"));
}

// ─────────────────────────────────────────────────────────────────────────────
// L2 - WHAT BASE WRITES WHEN IT SCAFFOLDS
// ─────────────────────────────────────────────────────────────────────────────

/// L2 - every file `base scaffold` leaves behind. Carries the fifth copy.
#[test]
fn l2_what_scaffold_writes_carries_no_claim() {
    let s = seed::write(&root_for("scaffold"), &seed::REAL, "");
    let target = s.ws.join("scaffolded");
    std::fs::create_dir_all(&target).expect("target dir");
    let (_, _, _) = seed::run_base(&s, &["scaffold", target.to_str().expect("utf-8 path")]);

    let (files, unreadable) = walk_text(&target);
    control("L2", "scaffold wrote at least one readable file", !files.is_empty());
    control(
        "L2",
        "a base.toml with a [budget] section",
        files.iter().any(|(p, t)| p.ends_with("base.toml") && t.contains("[budget]")),
    );

    let mut hits = Vec::new();
    for (p, t) in &files {
        hits.extend(scan("L2", &p.display().to_string(), t));
    }
    assert!(
        hits.is_empty(),
        "L2 walked {} file(s) ({} unreadable) and found {} claim(s):\n{}",
        files.len(),
        unreadable.len(),
        hits.len(),
        hits.join("\n")
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// L3 - WHAT BASE WRITES WHEN IT INSTALLS
// ─────────────────────────────────────────────────────────────────────────────

/// This repo's real `claude/skills`, the directory the release archive stages.
///
/// THE REAL ONE, NOT A STUB, AND THE EIGHTH COPY IS EXACTLY WHY. `install_e2e_test.rs` seeds a
/// two-line placeholder SKILL.md, which is right for what that file tests and useless here: the
/// eighth copy lives in `references/qa.md`, which a placeholder does not have. A leg that installs
/// a stub proves the installer copies files. It proves nothing about what the files SAY.
fn repo_claude_skills() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("claude").join("skills")
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("dest dir");
    for e in std::fs::read_dir(from).expect("source dir").flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            copy_tree(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).expect("copy file");
        }
    }
}

/// L3 - EVERY FILE `base install` WRITES. Carries the sixth AND the eighth copy.
///
/// THIS LEG IS THE FORK. The agreed shape was "grep what install writes", and both seats wrote
/// that sentence with `base.toml` on either side of it, so the natural build greps ONE FILE and
/// reports green over a claim sitting in `~/.claude/skills/base-help/references/qa.md`.
///
/// So this walks the whole installed tree and PRINTS THE SET SIZE. The count is not decoration:
/// it is the difference between "no claim is present" and "I looked at nothing".
///
/// ISOLATION IS `BASE_HOME`, NOT `HOME`, AND THE FIRST VERSION OF THIS LEG GOT THAT WRONG.
/// `home::home_root` consults `BASE_HOME` and never `$HOME`, because `dirs` does not read `$HOME`
/// on Windows at all - so a `$HOME`-based fake isolates under WSL and SILENTLY FAILS TO ISOLATE ON
/// WINDOWS, which is the worse of the two failure modes and the reason that seam exists.
///
/// Setting the wrong variable sent a real install into `/tmp/base-test-home-<pid>` while this leg
/// walked an empty directory. THE CONTROL BELOW CAUGHT IT AND REFUSED. Without that control the
/// leg would have walked zero files and reported the tree clean over two live copies.
///
/// AND `HOME` IS DELIBERATELY LEFT ALONE, which is the second thing this leg got wrong. Setting
/// `HOME` EQUAL TO `BASE_HOME` makes the fake tier and the real one indistinguishable, and base's
/// own write tripwire correctly panicked: "isolation breach: graph write to .../.base-gbl/.base/
/// graph.nq - the REAL global tier - while isolated." The tripwire compares against
/// `home::real_home`, which on Linux is `$HOME`, so pointing both at one path tells base its fake
/// home IS the real one.
///
/// That is the tripwire doing its job, not a defect - it caught a harness that had disarmed its
/// own isolation. `BASE_HOME` alone isolates AND leaves the tripwire armed, which is the only
/// shape that gets both.
#[test]
fn l3_what_install_writes_carries_no_claim() {
    let root = root_for("install");
    let archive = root.join("archive");
    let home = root.join("home");
    std::fs::create_dir_all(&archive).expect("archive dir");
    std::fs::create_dir_all(&home).expect("home dir");

    // The archive layout `tar xzf` leaves behind: the binary, with claude/skills as a sibling.
    let binary = archive.join(format!("base{}", std::env::consts::EXE_SUFFIX));
    if std::fs::hard_link(env!("CARGO_BIN_EXE_base"), &binary).is_err() {
        std::fs::copy(env!("CARGO_BIN_EXE_base"), &binary).expect("stage the binary");
    }
    copy_tree(&repo_claude_skills(), &archive.join("claude").join("skills"));

    let out = std::process::Command::new(&binary)
        .arg("install")
        .current_dir(&archive)
        .env("BASE_HOME", &home)
        .output()
        .expect("run install");
    let printed = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let (files, unreadable) = walk_text(&home);
    let names: BTreeSet<String> =
        files.iter().map(|(p, _)| p.display().to_string().replace('\\', "/")).collect();

    // THREE CONTROLS, because this leg's whole claim is about COVERAGE.
    control("L3", "install wrote at least one readable file", !files.is_empty());
    control("L3", "a base.toml was written", names.iter().any(|n| n.ends_with("base.toml")));
    control(
        "L3",
        "the base-help coach was written (references/qa.md)",
        names.iter().any(|n| n.ends_with("references/qa.md")),
    );

    let mut hits = scan("L3", "install stdout", &printed);
    for (p, t) in &files {
        hits.extend(scan("L3", &p.display().to_string(), t));
    }
    assert!(
        hits.is_empty(),
        "L3 walked {} file(s) ({} unreadable) under the installed home and found {} claim(s):\n{}",
        files.len(),
        unreadable.len(),
        hits.len(),
        hits.join("\n")
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// F1 - THE BEHAVIOUR DEFECT. NOT A GREP.
// ─────────────────────────────────────────────────────────────────────────────

/// A FRESH INSTALL MUST NOT WRITE A KEY THAT BASE THEN WARNS ABOUT.
///
/// This is a CYCLE, not a string, which is why it gets its own leg rather than riding on M3.
/// `install.rs` writes `session_start_chars` into the operator's base.toml on a fresh install.
/// `config.rs` lists that key as renamed. `BaseConfig::load` reports it on EVERY invocation. So
/// the operator is told, forever, to rename a key inside a file base wrote for them and they have
/// never opened.
///
/// The rename ruling - rename a key in the same commit that changes its unit - was applied to the
/// struct and its alias and NOT to the two sites that write the key onto disk. The migration
/// therefore works perfectly for everyone who already had a base.toml, and greets every new
/// operator with a warning about a file they did not write.
///
/// THE ASSERTION IS ON THE SECOND RUN, NOT THE FIRST. Whether the file contains the string is a
/// text question. Whether base SCOLDS THE OPERATOR FOR ITS OWN FILE is a behaviour question, and
/// only the second invocation can answer it: the second run is the operator's ordinary use of
/// base, with nobody installing anything.
///
/// MEASURED 2026-09-20, and it is worse than the design assumed: the warning fires on the INSTALL
/// RUN ITSELF. A real install into an empty home printed, on its own stderr, "base: [budget]
/// session_start_chars was renamed to session_start_bytes ... Rename the key in base.toml to stop
/// seeing this." The command that creates the file complains about the file while creating it.
/// That is not asserted here - it is one fact seen twice, not a second detector, and dressing it
/// up as corroboration would be exactly the mistake this suite exists to prevent.
#[test]
fn a_fresh_install_does_not_earn_its_own_deprecation_warning() {
    let root = root_for("fresh");
    let archive = root.join("archive");
    let home = root.join("home");
    std::fs::create_dir_all(&archive).expect("archive dir");
    std::fs::create_dir_all(&home).expect("home dir");

    let binary = archive.join(format!("base{}", std::env::consts::EXE_SUFFIX));
    if std::fs::hard_link(env!("CARGO_BIN_EXE_base"), &binary).is_err() {
        std::fs::copy(env!("CARGO_BIN_EXE_base"), &binary).expect("stage the binary");
    }
    copy_tree(&repo_claude_skills(), &archive.join("claude").join("skills"));

    let run = |args: &[&str]| {
        std::process::Command::new(&binary)
            .args(args)
            .current_dir(&archive)
            // BASE_HOME alone. See `l3_what_install_writes_carries_no_claim` for why HOME must not
            // be set to the same path: it disarms base's own write tripwire by making the fake
            // tier and the real one indistinguishable.
            .env("BASE_HOME", &home)
            .output()
            .expect("run base")
    };

    let _installed = run(&["install"]);

    // THE CONTROL THAT DECIDES WHETHER THIS LEG MEANS ANYTHING, and the first version did not have
    // it. "The install printed something" was the original control, and it PASSED while the install
    // wrote into a different directory entirely - so the assertion below ran over a home with no
    // config in it, found no warning, and went GREEN over a live defect.
    //
    // A gate whose failing state and whose could-not-run state produce the same result is not a
    // gate. The control therefore asks for the thing the assertion is ABOUT: a config file, written
    // by this install, carrying the section the key belongs to.
    let written = walk_text(&home)
        .0
        .into_iter()
        .find(|(p, _)| p.ends_with("base.toml"))
        .map(|(_, t)| t);
    control(
        "F1",
        "the install wrote a base.toml containing a [budget] section",
        written.as_deref().is_some_and(|t| t.contains("[budget]")),
    );

    // The SECOND invocation: an ordinary command, reading the config the install just wrote.
    //
    // IT MUST BE A COMMAND THAT LOADS THE CONFIG, AND THE FIRST VERSION WAS NOT. `--version`
    // short-circuits before `BaseConfig::load`, so it emits no advisory whatever the config says -
    // and this leg went GREEN over a live defect for that reason alone. Measured: `--version`
    // prints nothing on stderr; `doctor` and `learn --list` both print the warning.
    //
    // A leg whose subject is "what an ordinary command does" must run a command that reaches the
    // code under test. Picking one that returns early is the same vacuous shape as a fixture that
    // fits inside the budget it claims to test.
    let second = run(&["doctor"]);
    let stderr = String::from_utf8_lossy(&second.stderr).to_string();
    let stdout = String::from_utf8_lossy(&second.stdout).to_string();

    // CONTROL: the second command actually READ the tier this install wrote. Without this, a future
    // change to which command runs here could silently return us to the vacuous green above.
    control(
        "F1",
        "the second command read the installed tier (its output names that home)",
        stdout.contains(&home.display().to_string())
            || stderr.contains(&home.display().to_string()),
    );

    assert!(
        !stderr.contains("was renamed to"),
        "a fresh install wrote a key base immediately deprecates, so the operator is told to \
         rename a key inside a file base wrote for them:\n{stderr}"
    );
}

/// THE OVERFLOW NOTICE NAMES THE KEY THE OPERATOR WOULD ACTUALLY FIND IN THEIR OWN FILE.
///
/// AUK'S REFINEMENT, AND IT IS SHARPER THAN THE OBVIOUS FIX. Swapping the literal to
/// `prompt_bytes` is wrong in the other direction: an operator who legitimately still has
/// `prompt_chars` set would be sent to edit a key that is not in their file. The notice must name
/// THE SPELLING THAT RESOLVED - the same principle as the two matchers, where correct behaviour
/// depends on what is actually there rather than on what we assumed.
///
/// So this leg asserts both halves of that, which no single-case test can:
///   - nothing set:  the notice names the CURRENT spelling and no legacy one
///   - legacy set:   the notice names the spelling the operator actually wrote
#[test]
fn the_overflow_notice_names_the_key_that_resolved() {
    // ARM ONE: nothing set. The notice must name the current spelling.
    let s = loud("resolved-default");
    let (_, out, err) = seed::run_prompt_submit(&s, "a prompt", Some("artefact-resolved-1"));
    let text = format!("{out}\n{err}");
    control("resolved/default", "the notice fired", text.contains("withheld"));
    assert!(
        m3_legacy_key_named(&text).is_empty(),
        "with no budget key set, the notice named a legacy spelling:\n{text}"
    );

    // ARM TWO: the operator legitimately has the legacy key. The notice must name THAT, because
    // that is what is in their file - and the deprecation warning tells them the rest.
    let root = root_for("resolved-legacy");
    let legacy = RENAMED_BUDGET_KEYS[1].old; // prompt_chars
    let s = seed::write(&root, &seed::REAL, &format!("[budget]\n{legacy} = 200\n"));
    let (_, out, err) = seed::run_prompt_submit(&s, "a prompt", Some("artefact-resolved-2"));
    let text = format!("{out}\n{err}");
    control("resolved/legacy", "the notice fired", text.contains("withheld"));
    assert!(
        text.contains(legacy),
        "the operator set `{legacy}`, so that is the key in their file and the one the notice must \
         send them to; naming the new spelling points at a key they do not have:\n{text}"
    );
}
