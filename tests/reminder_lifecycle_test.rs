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

use seed::{run_base, run_base_in_session, run_session_start};

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

/// The workspace graph's lines for reminder `slug` and predicate `pred`, as the store wrote them.
fn quads(seed: &seed::Seed, slug: &str, pred: &str) -> Vec<String> {
    let path = seed.ws.join(".base").join("graph.nq");
    let key = format!("<{ns}reminder/{slug}> <{ns}{pred}> ", ns = seed::NS);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .lines()
        .filter(|l| l.starts_with(&key))
        .map(str::to_string)
        .collect()
}

/// Move the time session start recorded for reminder `slug`'s first shown warning (BO-27, V4) `days` into the past,
/// as if that session start had run `days` ago. The one way to reach a later day in a test.
fn backdate_warned(seed: &seed::Seed, slug: &str, days: i64) {
    let path = seed.ws.join(".base").join("graph.nq");
    let key = format!("<{ns}reminder/{slug}> <{ns}warnedAt> \"", ns = seed::NS);
    let when = (chrono::Local::now() - chrono::Duration::days(days)).to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let text = std::fs::read_to_string(&path).expect("workspace graph");
    let mut moved = 0;
    let out: Vec<String> = text
        .lines()
        .map(|l| match l.strip_prefix(&key).and_then(|rest| rest.find('"').map(|end| &rest[end..])) {
            Some(tail) => {
                moved += 1;
                format!("{key}{when}{tail}")
            }
            None => l.to_string(),
        })
        .collect();
    assert_eq!(moved, 1, "one warnedAt line for {slug} in the workspace graph:\n{text}");
    std::fs::write(&path, out.join("\n") + "\n").expect("workspace graph write");
}

/// The lines session start printed about reminders the auto-archive pass archived.
fn archive_lines(stdout: &str) -> Vec<&str> {
    stdout.lines().filter(|l| l.starts_with("reminder: archived ")).collect()
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
    // By its DUE NOW number since BO-00 B4: DUE NOW sorts oldest due first, so the 8-day reminder is 1.
    assert!(
        warned_line.starts_with("  1 ") && warned_line.contains("base reminder snooze 1 <duration>"),
        "no reset command naming the line's own number on the day-8 line: {warned_line}"
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
///
/// BO-27 (V4): only once its warning has been shown for two days. The first session start shows
/// both warnings and archives nothing; the archive is read at a session start two days after it.
#[test]
fn day_10_archives_and_does_not_delete() {
    let seed = workspace("r2");
    let gone = add_due(&seed, "Rotate the token", &days_ago(11));
    let staying = add_due(&seed, "Pay the invoice", &days_ago(9));

    let (code, warned, stderr) = run_session_start(&seed, Some("r06-r2"));
    assert_eq!(code, 0, "session start failed: {stderr}");
    assert!(warned.contains(&gone.name), "the first start shows the 11-day reminder, warned:\n{warned}");
    backdate_warned(&seed, &gone.slug, 2);
    backdate_warned(&seed, &staying.slug, 2);

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
/// The instruction block tells Claude what to run for a handled reminder: `base reminder archive <number>`, the
/// number DUE NOW prints on the reminder's line. Before rank 06's follow-up, line 3 still said `remove`, the hard
/// delete R4 keeps separate, while DUE NOW said `archive` two blocks below it. Since BO-00 B4 DUE NOW's lines
/// carry the number and no command, so line 3 is the only place the command is named.
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
        stdout.contains("DO THIS FIRST IN YOUR FIRST REPLY:"),
        "control: the instruction block did not render:{NL_MARK}{stdout}",
    );
    assert!(
        stdout.lines().any(|l| l == format!("  1 {}", due.name)),
        "control: DUE NOW did not print the reminder as number 1:{NL_MARK}{stdout}",
    );
    assert!(
        !stdout.contains(&due.slug),
        "DUE NOW still spends the first screen on the reminder's slug:{NL_MARK}{stdout}",
    );
    assert!(
        // BO-06 (D16b) reworded line 3 shorter; the command it names is unchanged.
        stdout.contains("A handled reminder: `base reminder archive <number>`."),
        "the instruction block does not name archive for a handled reminder:{NL_MARK}{stdout}",
    );
    assert!(
        !stdout.contains("base reminder remove"),
        "session start still tells Claude to hard-delete a handled reminder:{NL_MARK}{stdout}",
    );
}

// ── R9 (BO-00 B4) ─────────────────────────────────────────────────────────────
/// `base reminder archive <number>` and `snooze <number>` act on the reminder the last session start printed under
/// that number, and say which. A number session start did not print is read as a slug, so it finds nothing and
/// says so rather than acting on a guess.
///
/// Controls: each number is checked against the reminder it must reach AND the one it must not, so a resolver that
/// ignored the number and took the first reminder fails on 2.
#[test]
fn a_due_now_number_archives_and_snoozes_the_reminder_printed_under_it() {
    let seed = workspace("r9");
    let first = add_due(&seed, "Pay the invoice", &days_ago(3));
    let second = add_due(&seed, "Call the bank", &days_ago(2));
    let (code, stdout, stderr) = run_session_start(&seed, None);
    assert_eq!(code, 0, "session start failed: {stderr}");
    assert!(
        stdout.lines().any(|l| l == format!("  1 {}", first.name))
            && stdout.lines().any(|l| l == format!("  2 {}", second.name)),
        "control: DUE NOW did not number the two reminders oldest first:{NL_MARK}{stdout}",
    );

    let (acode, aout, aerr) = run_base(&seed, &["reminder", "archive", "2"]);
    assert_nonempty("reminder archive 2", &aout, &aerr);
    assert_eq!(acode, 0, "archive by number failed: {aout}{aerr}");
    assert!(
        aout.contains(&format!("DUE NOW 2 is '{}'", second.slug)),
        "archive by number did not say which reminder it reached: {aout}"
    );
    let (_, live, _) = run_base(&seed, &["reminder", "list"]);
    assert!(!live.contains(&second.name), "number 2 was not archived:\n{live}");
    assert!(live.contains(&first.name), "number 2 reached number 1 as well:\n{live}");

    let (scode, sout, serr) = run_base(&seed, &["reminder", "snooze", "1", "2d"]);
    assert_nonempty("reminder snooze 1", &sout, &serr);
    assert_eq!(scode, 0, "snooze by number failed: {sout}{serr}");
    assert!(
        sout.contains(&format!("DUE NOW 1 is '{}'", first.slug)),
        "snooze by number did not say which reminder it reached: {sout}"
    );
    let (_, live, _) = run_base(&seed, &["reminder", "list"]);
    let moved = line_with(&live, &first.name).unwrap_or_else(|| panic!("number 1 vanished:\n{live}"));
    assert!(moved.contains(&days_ahead(2)), "number 1 was not snoozed: {moved}");

    let (ncode, nout, nerr) = run_base(&seed, &["reminder", "archive", "7"]);
    assert_ne!(ncode, 0, "a number session start never printed must not succeed: {nout}{nerr}");
    assert!(
        format!("{nout}{nerr}").contains("no reminder '7'"),
        "an unprinted number is not reported as not found: {nout}{nerr}"
    );
}

// ── R10 (BO-00 code review) ───────────────────────────────────────────────────
/// Inside a session a DUE NOW number reaches only the reminder THAT session's start printed under it. Another
/// session's later start rewrites the workspace's letters file and must not move this session's numbers, and a
/// session with no numbers on file resolves none, whatever the workspace file says.
///
/// The failure this closes: A sees 1 = pay, pay is archived, B starts and the workspace file says 1 = call, and
/// A's `base reminder archive 1` archived call.
#[test]
fn a_due_now_number_belongs_to_the_session_that_printed_it() {
    let seed = workspace("r10");
    let pay = add_due(&seed, "Pay the invoice", &days_ago(3));
    let call = add_due(&seed, "Call the bank", &days_ago(2));
    let (code, stdout, stderr) = run_session_start(&seed, Some("r10-session-a"));
    assert_eq!(code, 0, "session A's start failed: {stderr}");
    assert!(stdout.lines().any(|l| l == format!("  1 {}", pay.name)), "control: A sees pay as 1:{NL_MARK}{stdout}");

    let (code, out, err) = run_base(&seed, &["reminder", "archive", &pay.slug]);
    assert_eq!(code, 0, "archive by slug failed: {out}{err}");
    let (code, stdout, stderr) = run_session_start(&seed, Some("r10-session-b"));
    assert_eq!(code, 0, "session B's start failed: {stderr}");
    assert!(
        stdout.lines().any(|l| l == format!("  1 {}", call.name)),
        "control: B's start renumbered, so the workspace file now says 1 = call:{NL_MARK}{stdout}"
    );

    let (code, out, err) = run_base_in_session(&seed, &["reminder", "archive", "1"], "r10-session-a");
    assert_eq!(code, 0, "A's archive 1 failed: {out}{err}");
    assert!(out.contains(&format!("DUE NOW 1 is '{}'", pay.slug)), "A's 1 is not the reminder A saw: {out}");
    let (_, live, _) = run_base(&seed, &["reminder", "list"]);
    assert!(live.contains(&call.name), "A's number reached the reminder B numbered 1:\n{live}");

    let (code, out, err) = run_base_in_session(&seed, &["reminder", "archive", "1"], "r10-never-started");
    assert_ne!(code, 0, "a session with no numbers on file resolved one: {out}{err}");
    assert!(format!("{out}{err}").contains("no reminder '1'"), "not reported as not found: {out}{err}");
    let (_, live, _) = run_base(&seed, &["reminder", "list"]);
    assert!(live.contains(&call.name), "a session with no numbers archived from the workspace file:\n{live}");
}

/// A line break for the failure messages above, spelled once.
const NL_MARK: &str = "\n";

// ── BO-27, V4 ────────────────────────────────────────────────────────────────
/// V4: a reminder 15 days overdue whose warning no session start has shown (a 0.15.2 user, or one who opened no session
/// from day 8 to day 10) is NOT archived at the next start: it stays in DUE NOW with the warning, dated two days out,
/// and the start records `warnedAt`. Still live at a second start that day and at one a day later; archived only at a
/// start two days after the warning was first shown. Control: a 3-day reminder carries no warning and no record.
#[test]
fn overdue_reminder_not_archived_before_its_warning_is_seen() {
    let seed = workspace("bo27-v4a");
    let late = add_due(&seed, "Pay the insurance premium", &days_ago(15));
    let fresh = add_due(&seed, "Water the plants", &days_ago(3));
    assert!(quads(&seed, &late.slug, "warnedAt").is_empty(), "control: no warning recorded before any start");

    let (code, first, stderr) = run_session_start(&seed, Some("bo27-v4a"));
    assert_eq!(code, 0, "session start failed: {stderr}");
    let line = line_with(&first, &late.name).unwrap_or_else(|| panic!("the 15-day reminder left DUE NOW:\n{first}"));
    assert!(
        line.contains(&format!("archives {} unless reset", days_ahead(2))),
        "warned, with the two days still ahead of it: {line}"
    );
    assert_eq!(quads(&seed, &late.slug, "warnedAt").len(), 1, "the shown warning is recorded");
    assert!(quads(&seed, &fresh.slug, "warnedAt").is_empty(), "control: a 3-day reminder is not warned");
    assert!(archive_lines(&first).is_empty(), "{first}");

    let (_, second, _) = run_session_start(&seed, Some("bo27-v4a"));
    assert!(line_with(&second, &late.name).is_some(), "a second start the same day archives nothing:\n{second}");
    let recorded = quads(&seed, &late.slug, "warnedAt");
    assert_eq!(recorded.len(), 1, "the first showing is kept, never moved or doubled: {recorded:?}");

    backdate_warned(&seed, &late.slug, 1);
    let (_, day_one, _) = run_session_start(&seed, Some("bo27-v4a"));
    assert!(line_with(&day_one, &late.name).is_some(), "one day after the warning it is still live:\n{day_one}");

    backdate_warned(&seed, &late.slug, 2);
    let (_, day_two, _) = run_session_start(&seed, Some("bo27-v4a"));
    let (_, live, _) = run_base(&seed, &["reminder", "list"]);
    assert!(!live.contains(&late.name), "two days after the warning it archives:\n{day_two}\n{live}");
    assert!(live.contains(&fresh.name), "control: the 3-day reminder stays:\n{live}");
}

/// V4: every automatic archive prints one line at the start that archives it, naming the reminder and its undo, in the
/// shape of an upgrade's lines; the next start prints none. The printed undo restores it.
#[test]
fn auto_archive_announces_once_with_undo() {
    let seed = workspace("bo27-v4b");
    let r = add_due(&seed, "Renew the domain", &days_ago(12));
    let (code, _, stderr) = run_session_start(&seed, Some("bo27-v4b"));
    assert_eq!(code, 0, "session start failed: {stderr}");
    backdate_warned(&seed, &r.slug, 2);

    let (_, archiving, _) = run_session_start(&seed, Some("bo27-v4b"));
    let undo = format!("base reminder unarchive {}", r.slug);
    let expected = format!("reminder: archived '{}' (12d overdue, warned {}) · undo: {undo}", r.name, days_ago(2));
    assert_eq!(archive_lines(&archiving), vec![expected.as_str()], "{archiving}");
    let reason = quads(&seed, &r.slug, "archivedReason");
    let why = format!("auto: 12d past due, warned {}", days_ago(2));
    assert!(reason.len() == 1 && reason[0].contains(&why), "the stored reason says when and why: {reason:?}");
    let (_, next, _) = run_session_start(&seed, Some("bo27-v4b"));
    assert!(archive_lines(&next).is_empty(), "said once:\n{next}");
    assert!(line_with(&next, &r.name).is_none(), "control: it is archived:\n{next}");

    let words: Vec<&str> = undo.split(' ').skip(1).collect();
    let (code, out, err) = run_base(&seed, &words);
    assert_eq!(code, 0, "the printed undo failed: {out}{err}");
    let (_, live, _) = run_base(&seed, &["reminder", "list"]);
    assert!(live.contains(&r.name), "the undo restored it:\n{live}");
}

// ── BO-27, V5 ────────────────────────────────────────────────────────────────
/// V5: `base reminder unarchive` brings an archived reminder back. One whose time has passed surfaces from now; one
/// archived before its time keeps it; its archive and its warning record are cleared. A slug no tier holds fails and
/// says so; a live one is left as it is.
#[test]
fn reminder_unarchive_restores() {
    let seed = workspace("bo27-v5a");
    let past = add_due(&seed, "Call the bank about the loan", &days_ago(9));
    let ahead = add_due(&seed, "Send the quarterly pack", &days_ahead(5));
    let (code, _, stderr) = run_session_start(&seed, Some("bo27-v5a"));
    assert_eq!(code, 0, "session start failed: {stderr}");
    assert_eq!(quads(&seed, &past.slug, "warnedAt").len(), 1, "control: the 9-day warning was recorded");
    for r in [&past, &ahead] {
        let (code, out, err) = run_base(&seed, &["reminder", "archive", &r.slug]);
        assert_eq!(code, 0, "archive: {out}{err}");
    }
    let (_, gone, _) = run_base(&seed, &["reminder", "list"]);
    assert!(!gone.contains(&past.name) && !gone.contains(&ahead.name), "control: both archived:\n{gone}");

    for r in [&past, &ahead] {
        let (code, out, err) = run_base(&seed, &["reminder", "unarchive", &r.slug]);
        assert_eq!(code, 0, "unarchive: {out}{err}");
        assert!(out.contains(&format!("Reminder '{}' unarchived (workspace tier)", r.slug)), "{out}");
    }
    let (_, live, _) = run_base(&seed, &["reminder", "list"]);
    let past_line = line_with(&live, &past.name).unwrap_or_else(|| panic!("not restored:\n{live}"));
    assert!(past_line.contains("| today |"), "a passed time surfaces from now: {past_line}");
    let ahead_line = line_with(&live, &ahead.name).unwrap_or_else(|| panic!("not restored:\n{live}"));
    assert!(ahead_line.contains("in 5d"), "a time still ahead is kept: {ahead_line}");
    assert!(quads(&seed, &past.slug, "warnedAt").is_empty(), "the warning record is cleared");
    for pred in ["status", "archivedAt", "archivedReason"] {
        assert!(quads(&seed, &past.slug, pred).is_empty(), "{pred} cleared");
    }

    let (code, out, err) = run_base(&seed, &["reminder", "unarchive", "no-such-reminder"]);
    assert_ne!(code, 0, "an unknown slug must fail: {out}");
    assert!(err.contains("nothing was unarchived"), "{err}");
    let (code, out, _) = run_base(&seed, &["reminder", "unarchive", &past.slug]);
    assert_eq!(code, 0);
    assert!(out.contains("is not archived"), "{out}");
}

/// V5: `base reminder add` with the name of an archived reminder revives it at the new time and says `revived`, and it
/// surfaces at the next session start. Before BO-27 it printed `set` and the record stayed archived. One clock: the
/// record holds one surface time.
#[test]
fn readding_an_archived_reminder_revives_it() {
    let seed = workspace("bo27-v5b");
    let r = add_due(&seed, "Book the venue", &days_ago(4));
    let (code, out, err) = run_base(&seed, &["reminder", "archive", &r.slug]);
    assert_eq!(code, 0, "archive: {out}{err}");

    let (code, out, err) = run_base(&seed, &["reminder", "add", "--name", &r.name, "--due", &days_ago(0)]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains(&format!("Reminder '{}' revived", r.name)), "{out}");
    assert!(!out.contains("' set "), "never a plain set: {out}");
    assert_eq!(quads(&seed, &r.slug, "resurfaceAt").len(), 1, "one surface time");
    assert!(quads(&seed, &r.slug, "status").is_empty(), "the archive is cleared");

    let (_, archived, _) = run_base(&seed, &["reminder", "list", "--archived"]);
    assert!(!archived.contains(&r.name), "{archived}");
    let (code, start, stderr) = run_session_start(&seed, Some("bo27-v5b"));
    assert_eq!(code, 0, "session start failed: {stderr}");
    assert!(line_with(&start, &r.name).is_some(), "it surfaces in DUE NOW:\n{start}");
}

/// G0 Q3: re-adding a LIVE reminder's name moves its one clock and says `moved`. Before BO-27 the record gained a second
/// surface time beside the first.
#[test]
fn readding_a_live_reminder_moves_its_clock() {
    let seed = workspace("bo27-v5c");
    let r = add_due(&seed, "Chase the signed contract", &days_ago(3));
    let (code, out, err) = run_base(&seed, &["reminder", "add", "--name", &r.name, "--due", &days_ahead(2)]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains(&format!("Reminder '{}' moved", r.name)), "{out}");
    assert_eq!(quads(&seed, &r.slug, "resurfaceAt").len(), 1, "one surface time, not two");
    assert_eq!(quads(&seed, &r.slug, "createdAt").len(), 1, "one record, not a second one beside it");
    let (_, live, _) = run_base(&seed, &["reminder", "list"]);
    let line = line_with(&live, &r.name).unwrap_or_else(|| panic!("{live}"));
    assert!(line.contains(&days_ahead(2)) && !line.contains("overdue"), "{line}");
}

/// BO-27 code review: `unarchive` brings back only the copies that are archived. A live copy of the same slug in the
/// other tier keeps its own time, so its overdue age and its warning clock are not reset by someone else's undo.
#[test]
fn unarchive_leaves_a_live_copy_in_the_other_tier_alone() {
    let seed = workspace("bo27-v5d");
    let r = add_due(&seed, "Renew the lease", &days_ago(9));
    let (code, out, err) = run_base(&seed, &["reminder", "archive", &r.slug]);
    assert_eq!(code, 0, "archive: {out}{err}");
    let live_at = midnight(&days_ago(9));
    add_global_reminder(&seed, &r.slug, &r.name, &live_at);
    let global = seed.home.join(".base-gbl").join(".base").join("graph.nq");
    let global_line = || {
        std::fs::read_to_string(&global)
            .expect("global graph")
            .lines()
            .find(|l| l.contains(&format!("reminder/{}> <{}resurfaceAt>", r.slug, seed::NS)))
            .map(str::to_string)
    };
    let before = global_line().expect("control: the live global copy");

    let (code, out, err) = run_base(&seed, &["reminder", "unarchive", &r.slug]);
    assert_eq!(code, 0, "unarchive: {out}{err}");
    assert!(out.contains("(workspace tier)") && !out.contains("global"), "only the archived copy: {out}");
    assert_eq!(global_line().as_deref(), Some(before.as_str()), "the live copy kept its own time");
    assert!(quads(&seed, &r.slug, "status").is_empty(), "the workspace copy is live again");
}
