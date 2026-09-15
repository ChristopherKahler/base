//! Rank 06: the reminder lifecycle — snooze, archive, the archive warning, and both tiers.
//!
//! Every test here runs RED at commit C (`65718ae`) plus rank 06's law-11 commit 1, where the
//! subcommands parse and do nothing. They fail on an assertion, never on the build.
//!
//! **Why presence and absence are asserted on the NAME, not the slug.** `base reminder list`
//! prints the name and the surface time and nothing else today; the slug column arrives with this
//! rank. So `!output.contains(slug)` is true at the base whatever the store holds — it reads as
//! "absent" for a reminder that is sitting right there. The first draft of this file used that
//! form and `day_10_archives_and_does_not_delete` passed its real assertion for exactly that wrong
//! reason, failing only on its control. Proved toothless by a knock-out probe before this rewrite:
//! the same assertion passes against `list` output and fails against `add` output, which does
//! print the slug. Presence and absence are therefore asserted on the name, which is printed at
//! both ends, and the slug is asserted separately where the new column is the thing under test.
//!
//! **Why these fixtures are not in `tests/seed`.** The shared seed writes reminders at fixed
//! 2026-08 dates, which is right for a size budget and wrong for a clock: "N days past due" is
//! measured from the moment the command runs, so no fixed date stays at a chosen offset. Every
//! reminder asserted on here is built RELATIVE to the run, through `base reminder add`, which
//! exercises `add` as a control at the same time. Lane 1's seed is left alone on purpose: commit D
//! rewrites the notes section immediately after its reminder loop.

mod seed;

use std::path::Path;

use seed::{run_base, run_session_start};

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const XSD_DATETIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";

/// A reminder this test made. Both handles are kept because they are read in different places:
/// the name is what `list` prints today, the slug is what the commands take and what the new
/// column must show.
struct Fixture {
    name: String,
    slug: String,
}

/// A fresh seeded workspace. `TINY` is the right size here: these tests assert on one reminder's
/// lifecycle, never on a budget, so the real-size seed would only cost time.
fn workspace(tag: &str) -> seed::Seed {
    let root = std::env::temp_dir().join(format!("base-r06-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    seed::write(&root, &seed::TINY, "")
}

/// `YYYY-MM-DD`, `days` before today, in the local zone the clock compares against.
fn days_ago(days: i64) -> String {
    (chrono::Local::now() - chrono::Duration::days(days))
        .format("%Y-%m-%d")
        .to_string()
}

/// `YYYY-MM-DD`, `days` after today.
fn days_ahead(days: i64) -> String {
    (chrono::Local::now() + chrono::Duration::days(days))
        .format("%Y-%m-%d")
        .to_string()
}

/// Add a reminder due on `date`. Fails loudly: a fixture that did not land would otherwise read
/// as the feature under test being absent.
fn add_due(seed: &seed::Seed, name: &str, date: &str) -> Fixture {
    let (code, stdout, stderr) = run_base(seed, &["reminder", "add", "--name", name, "--due", date]);
    assert!(
        !stdout.is_empty(),
        "fixture `add` printed nothing (stderr: {stderr})"
    );
    assert_eq!(code, 0, "fixture `add` failed: {stdout}{stderr}");
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    assert!(
        stdout.contains(&slug),
        "fixture `add` did not report slug {slug}: {stdout}"
    );
    Fixture {
        name: name.to_string(),
        slug,
    }
}

/// Write a reminder straight into the GLOBAL tier's graph, which `base reminder add` cannot
/// target: it always writes the workspace tier. The quad shape is the seed's own.
fn add_global_reminder(seed: &seed::Seed, slug: &str, name: &str, resurface_at: &str) -> Fixture {
    let ns = seed::NS;
    let graph = format!("{ns}graph/global/seed");
    let s = format!("{ns}reminder/{slug}");
    let quads = format!(
        "<{s}> <{RDF_TYPE}> <{ns}Reminder> <{graph}> .\n\
         <{s}> <{ns}name> \"{name}\" <{graph}> .\n\
         <{s}> <{ns}resurfaceAt> \"{resurface_at}\"^^<{XSD_DATETIME}> <{graph}> .\n"
    );
    let path = seed.home.join(".base-gbl").join(".base").join("graph.nq");
    let mut existing = std::fs::read_to_string(&path).expect("global graph");
    existing.push_str(&quads);
    std::fs::write(&path, existing).expect("global graph write");
    assert!(
        std::fs::read_to_string(&path)
            .expect("global graph reread")
            .contains(slug),
        "global fixture {slug} did not land in {}",
        path.display()
    );
    Fixture {
        name: name.to_string(),
        slug: slug.to_string(),
    }
}

/// Midnight-local of `date`, as a `--due` reminder's `resurfaceAt` is stored.
fn midnight(date: &str) -> String {
    format!("{date}T00:00:00-05:00")
}

/// The one line of `haystack` holding `needle`.
fn line_with<'a>(haystack: &'a str, needle: &str) -> Option<&'a str> {
    haystack.lines().find(|l| l.contains(needle))
}

/// Output is asserted non-empty before anything else: an empty stdout with a zero exit code
/// would otherwise satisfy every `!contains` assertion for the wrong reason.
fn assert_nonempty(what: &str, stdout: &str, stderr: &str) {
    assert!(
        !stdout.is_empty(),
        "{what} printed nothing at all (stderr: {stderr})"
    );
}

// ── R1 ───────────────────────────────────────────────────────────────────────
/// J5.1, R3: from day 8 past due the reminder's DUE NOW line says when it archives and how to
/// reset it. Control: a reminder 3 days past due carries no warning, so the assertion tells the
/// warning apart from the line simply always being there.
#[test]
fn day_8_shows_the_archive_date_and_the_reset_command() {
    let seed = workspace("r1");
    let warned = add_due(&seed, "Renew the cert", &days_ago(8));
    let quiet = add_due(&seed, "Book the room", &days_ago(3));

    let (code, stdout, stderr) = run_session_start(&seed, Some("r06-r1"));
    assert_nonempty("session start", &stdout, &stderr);
    assert_eq!(code, 0, "session start failed: {stderr}");

    let warned_line = line_with(&stdout, &warned.name)
        .unwrap_or_else(|| panic!("the 8-day reminder is not in DUE NOW at all:\n{stdout}"));
    // Ten days from its due date is when it goes: due 8 days ago means 2 days from now.
    let archives_on = days_ahead(2);
    assert!(
        warned_line.contains(&format!("archives {archives_on}")),
        "no archive date on the day-8 line: {warned_line}"
    );
    assert!(
        warned_line.contains(&format!("base reminder snooze {}", warned.slug)),
        "no reset command on the day-8 line: {warned_line}"
    );

    let quiet_line = line_with(&stdout, &quiet.name)
        .unwrap_or_else(|| panic!("the 3-day control is not in DUE NOW at all:\n{stdout}"));
    assert!(
        !quiet_line.contains("archives "),
        "control: a 3-day reminder must carry no warning: {quiet_line}"
    );
}

// ── R2 ───────────────────────────────────────────────────────────────────────
/// J5.2: at 10 whole days past due the reminder is archived and KEPT. Control: a 9-day twin is
/// still live, so the assertion reads the threshold and not "everything got archived".
#[test]
fn day_10_archives_and_does_not_delete() {
    let seed = workspace("r2");
    let gone = add_due(&seed, "Rotate the token", &days_ago(11));
    let staying = add_due(&seed, "Pay the invoice", &days_ago(9));

    let (code, stdout, stderr) = run_session_start(&seed, Some("r06-r2"));
    assert_nonempty("session start", &stdout, &stderr);
    assert_eq!(code, 0, "session start failed: {stderr}");

    let (lcode, live, lerr) = run_base(&seed, &["reminder", "list"]);
    assert_nonempty("reminder list", &live, &lerr);
    assert_eq!(lcode, 0, "reminder list failed: {lerr}");
    assert!(
        !live.contains(&gone.name),
        "the 11-day reminder is still live after the pass:\n{live}"
    );
    assert!(
        live.contains(&staying.name),
        "control: the 9-day twin must still be live:\n{live}"
    );

    let (acode, archived, aerr) = run_base(&seed, &["reminder", "list", "--archived"]);
    assert_nonempty("reminder list --archived", &archived, &aerr);
    assert_eq!(acode, 0, "reminder list --archived failed: {aerr}");
    assert!(
        archived.contains(&gone.name),
        "the 11-day reminder was not kept as archived — archive must never delete:\n{archived}"
    );
    assert!(
        !archived.contains(&staying.name),
        "control: the 9-day twin must not be archived:\n{archived}"
    );
}

// ── R3 ───────────────────────────────────────────────────────────────────────
/// J5.3, D3: snooze moves the surface time and resets the clock with it, and snoozing an archived
/// reminder brings it back. Control: an untouched twin keeps its own date.
#[test]
fn snooze_moves_it_and_resets_the_clock() {
    let seed = workspace("r3");
    let moved = add_due(&seed, "Chase the reply", &days_ago(3));
    let twin = add_due(&seed, "Read the spec", &days_ago(3));

    let (code, stdout, stderr) = run_base(&seed, &["reminder", "snooze", &moved.slug, "2d"]);
    assert_nonempty("reminder snooze", &stdout, &stderr);
    assert_eq!(code, 0, "snooze failed: {stdout}{stderr}");
    assert!(
        stdout.contains("workspace"),
        "snooze did not name the tier it changed: {stdout}"
    );

    let (lcode, live, lerr) = run_base(&seed, &["reminder", "list"]);
    assert_nonempty("reminder list", &live, &lerr);
    assert_eq!(lcode, 0, "reminder list failed: {lerr}");
    let moved_line = line_with(&live, &moved.name)
        .unwrap_or_else(|| panic!("the snoozed reminder vanished from list:\n{live}"));
    assert!(
        moved_line.contains(&days_ahead(2)),
        "snooze did not move the surface time to now + 2d: {moved_line}"
    );
    assert!(
        !moved_line.contains("overdue"),
        "a snoozed reminder must not read as overdue: {moved_line}"
    );

    let twin_line = line_with(&live, &twin.name)
        .unwrap_or_else(|| panic!("control twin vanished from list:\n{live}"));
    assert!(
        twin_line.contains("overdue"),
        "control: the untouched twin must still be overdue: {twin_line}"
    );

    // Snoozing an archived reminder brings it back.
    let (acode, aout, aerr) = run_base(&seed, &["reminder", "archive", &twin.slug]);
    assert_nonempty("reminder archive", &aout, &aerr);
    assert_eq!(acode, 0, "archive failed: {aout}{aerr}");
    let (_, after_archive, _) = run_base(&seed, &["reminder", "list"]);
    assert!(
        !after_archive.contains(&twin.name),
        "control: the archived twin must leave the live list before the revive:\n{after_archive}"
    );

    let (scode, sout, serr) = run_base(&seed, &["reminder", "snooze", &twin.slug, "1d"]);
    assert_nonempty("reminder snooze (revive)", &sout, &serr);
    assert_eq!(scode, 0, "snooze of an archived reminder failed: {sout}{serr}");
    let (_, live2, _) = run_base(&seed, &["reminder", "list"]);
    assert!(
        live2.contains(&twin.name),
        "snoozing an archived reminder must bring it back live:\n{live2}"
    );
}

// ── R4 ───────────────────────────────────────────────────────────────────────
/// J5.4, D5: an archived reminder never shows at session start, and `remove` is still the hard
/// delete. The remove half holds at commit C and is a guard, proven by its mutation rather than
/// counted as red; the archive half is the new behaviour.
#[test]
fn archived_never_shows_at_session_start_and_remove_still_deletes() {
    let seed = workspace("r4");
    let hidden = add_due(&seed, "Silence this one", &days_ago(2));
    let shown = add_due(&seed, "Keep this one", &days_ago(2));

    let (acode, aout, aerr) = run_base(&seed, &["reminder", "archive", &hidden.slug]);
    assert_nonempty("reminder archive", &aout, &aerr);
    assert_eq!(acode, 0, "archive failed: {aout}{aerr}");

    let (code, stdout, stderr) = run_session_start(&seed, Some("r06-r4"));
    assert_nonempty("session start", &stdout, &stderr);
    assert_eq!(code, 0, "session start failed: {stderr}");
    assert!(
        !stdout.contains(&hidden.name),
        "an archived reminder reached session start:\n{stdout}"
    );
    assert!(
        stdout.contains(&shown.name),
        "control: the live reminder must still reach session start:\n{stdout}"
    );

    // Guard: `remove` still deletes outright, out of every listing.
    let (rcode, rout, rerr) = run_base(&seed, &["reminder", "remove", &shown.slug]);
    assert_nonempty("reminder remove", &rout, &rerr);
    assert_eq!(rcode, 0, "remove failed: {rout}{rerr}");
    let (_, live, _) = run_base(&seed, &["reminder", "list"]);
    let (_, archived, _) = run_base(&seed, &["reminder", "list", "--archived"]);
    assert!(
        !live.contains(&shown.name),
        "remove left it live:\n{live}"
    );
    assert!(
        !archived.contains(&shown.name),
        "remove must delete, not archive:\n{archived}"
    );
}

// ── R5 ───────────────────────────────────────────────────────────────────────
/// D5: `list` reads BOTH tiers (today it reads one), names each reminder's tier and slug, and
/// archived reminders need the flag.
#[test]
fn list_reads_both_tiers_and_archived_needs_the_flag() {
    let seed = workspace("r5");
    let local = add_due(&seed, "Local reminder", &days_ago(1));
    let global = add_global_reminder(
        &seed,
        "global-reminder",
        "Global reminder",
        &midnight(&days_ago(1)),
    );

    let (code, stdout, stderr) = run_base(&seed, &["reminder", "list"]);
    assert_nonempty("reminder list", &stdout, &stderr);
    assert_eq!(code, 0, "reminder list failed: {stderr}");
    assert!(
        stdout.contains(&local.name),
        "the workspace reminder is missing from list:\n{stdout}"
    );
    assert!(
        stdout.contains(&global.name),
        "list read one tier: the global reminder is missing:\n{stdout}"
    );
    // The new columns, asserted on the slug because the slug column IS the thing under test.
    assert!(
        stdout.contains(&local.slug) && stdout.contains(&global.slug),
        "list does not show the slug column:\n{stdout}"
    );
    assert!(
        stdout.contains("global") && stdout.contains("workspace"),
        "list does not name each reminder's tier:\n{stdout}"
    );

    let (acode, aout, aerr) = run_base(&seed, &["reminder", "archive", &local.slug]);
    assert_nonempty("reminder archive", &aout, &aerr);
    assert_eq!(acode, 0, "archive failed: {aout}{aerr}");

    let (_, live, _) = run_base(&seed, &["reminder", "list"]);
    assert!(
        !live.contains(&local.name),
        "an archived reminder shows without the flag:\n{live}"
    );
    let (fcode, flagged, ferr) = run_base(&seed, &["reminder", "list", "--archived"]);
    assert_nonempty("reminder list --archived", &flagged, &ferr);
    assert_eq!(fcode, 0, "reminder list --archived failed: {ferr}");
    assert!(
        flagged.contains(&local.name),
        "--archived did not list the archived reminder:\n{flagged}"
    );
    assert!(
        !flagged.contains(&global.name),
        "--archived must list only archived ones:\n{flagged}"
    );
}

// ── R6 ───────────────────────────────────────────────────────────────────────
/// The rank 08 shape: a reminder living in the OTHER tier is snoozed and archived from this
/// workspace, and the command says which tier it changed. Control: the workspace twin is
/// untouched by a command aimed at the global slug.
#[test]
fn a_reminder_in_the_other_tier_is_snoozed_and_archived_from_here() {
    let seed = workspace("r6");
    let twin = add_due(&seed, "Workspace twin", &days_ago(4));
    let other = add_global_reminder(
        &seed,
        "other-tier-reminder",
        "Other tier reminder",
        &midnight(&days_ago(4)),
    );

    let (scode, sout, serr) = run_base(&seed, &["reminder", "snooze", &other.slug, "3d"]);
    assert_nonempty("reminder snooze", &sout, &serr);
    assert_eq!(scode, 0, "snooze of a global reminder failed: {sout}{serr}");
    assert!(
        sout.contains("global"),
        "snooze did not name the global tier it changed: {sout}"
    );

    let (_, live, _) = run_base(&seed, &["reminder", "list"]);
    let moved = line_with(&live, &other.name)
        .unwrap_or_else(|| panic!("the global reminder vanished from list:\n{live}"));
    assert!(
        moved.contains(&days_ahead(3)),
        "the global reminder's surface time did not move: {moved}"
    );
    let twin_line = line_with(&live, &twin.name)
        .unwrap_or_else(|| panic!("control twin vanished from list:\n{live}"));
    assert!(
        twin_line.contains("overdue"),
        "control: the workspace twin must be untouched: {twin_line}"
    );

    let (acode, aout, aerr) = run_base(&seed, &["reminder", "archive", &other.slug]);
    assert_nonempty("reminder archive", &aout, &aerr);
    assert_eq!(acode, 0, "archive of a global reminder failed: {aout}{aerr}");
    assert!(
        aout.contains("global"),
        "archive did not name the global tier it changed: {aout}"
    );

    let (_, live2, _) = run_base(&seed, &["reminder", "list"]);
    assert!(
        !live2.contains(&other.name),
        "the archived global reminder is still live:\n{live2}"
    );
    assert!(
        live2.contains(&twin.name),
        "control: the workspace twin must survive:\n{live2}"
    );

    // A slug in no tier is an error, never a silent success (the #72 shape).
    let (ncode, nout, nerr) = run_base(&seed, &["reminder", "snooze", "no-such-reminder", "1d"]);
    let said = format!("{nout}{nerr}");
    assert!(!said.is_empty(), "a missing slug printed nothing at all");
    assert_ne!(ncode, 0, "a slug in no tier must exit non-zero: {said}");
}

/// The path these tests assume the seed writes the global graph to. A rename in lane 1 would
/// otherwise turn every global-tier assertion above into a silent pass over an unread file.
#[test]
fn the_global_graph_is_where_these_fixtures_write_it() {
    let seed = workspace("r-path");
    let path = seed.home.join(".base-gbl").join(".base").join("graph.nq");
    assert!(
        Path::new(&path).is_file(),
        "the seed's global graph is not at {}",
        path.display()
    );
}

// ── R8 (rank 06 follow-up, flag 6) ──────────────────────────────────────────
/// The instruction block tells Claude what to run for a handled reminder, and it must be the command DUE NOW's
/// own line prints: `base reminder archive <slug>`. Before this follow-up, line 3 still said `remove`, the hard
/// delete R4 keeps separate, while DUE NOW said `archive` two blocks below it.
///
/// Read off the hook's stdout, the channel Claude receives. Controls: the instruction block and DUE NOW both
/// rendered, so an absent line cannot pass for a correct one. Mutation: put `remove` back in
/// `hook::session_start::instruction_block`, and this goes red.
#[test]
fn the_instruction_block_names_archive_for_a_handled_reminder_as_due_now_does() {
    let seed = workspace("r8");
    let due = add_due(&seed, "Renew the domain", &days_ago(1));
    let (code, stdout, stderr) = run_session_start(&seed, None);
    assert_nonempty("session start", &stdout, &stderr);
    assert_eq!(code, 0, "hooks fail open. stderr: {stderr}");
    assert!(
        stdout.contains("DO THIS FIRST, BEFORE ANYTHING ELSE IN YOUR FIRST REPLY:"),
        "control: the instruction block did not render:{NL_MARK}{stdout}",
    );
    assert!(
        stdout.contains(&format!("clear: base reminder archive {}", due.slug)),
        "control: DUE NOW did not print its clear command:{NL_MARK}{stdout}",
    );
    assert!(
        stdout.contains("a handled reminder → `base reminder archive <slug>`."),
        "the instruction block does not name archive for a handled reminder:{NL_MARK}{stdout}",
    );
    assert!(
        !stdout.contains("base reminder remove"),
        "session start still tells Claude to hard-delete a handled reminder:{NL_MARK}{stdout}",
    );
}

/// A line break for the failure messages above, spelled once.
const NL_MARK: &str = "\n";
