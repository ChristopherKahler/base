//! Rank 00 commit C: the session start layout of spec B1-B8, read off the hook's stdout from the
//! binary `cargo test` builds, over the real-size seed.
//!
//! Every test here compiles against commit B's head and runs red there first: the header, the
//! instruction block, DUE NOW and the lettered list do not exist before commit C.

mod seed;

use std::sync::OnceLock;

use base::config::BaseConfig;
use base::hook::session_start::SessionOutput;
use seed::{measured, run_session_start, units};

const REAL_BUDGET: usize = 9000;

/// The same seed and the same run for every test that reads it: the run is the slow part.
fn real(budget: usize) -> &'static (seed::Seed, i32, String, String) {
    static AT_9000: OnceLock<(seed::Seed, i32, String, String)> = OnceLock::new();
    static UNBOUNDED: OnceLock<(seed::Seed, i32, String, String)> = OnceLock::new();
    let cell = if budget == REAL_BUDGET {
        &AT_9000
    } else {
        &UNBOUNDED
    };
    cell.get_or_init(|| {
        let root =
            std::env::temp_dir().join(format!("base-seed-layout-{budget}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let seed = seed::write(
            &root,
            &seed::REAL,
            &format!("[budget]\nsession_start_chars = {budget}\n"),
        );
        let (code, stdout, stderr) = run_session_start(&seed, Some("layout-session"));
        (seed, code, stdout, stderr)
    })
}

/// The first `n` UTF-16 units of `s`.
///
/// THE PREVIEW'S UNIT IS UNKNOWN AND THIS DOC USED TO ASSERT IT. It read "the preview Claude Code
/// hands over when it cuts an output", which claims the host measures that preview in UTF-16. The
/// host's LIMIT was measured as bytes on 2026-09-20. THE PREVIEW LENGTH HAS NEVER BEEN MEASURED,
/// in either unit - it is a separate quantity and that round did not touch it.
fn first_units(s: &str, n: usize) -> String {
    let units: Vec<u16> = s.encode_utf16().take(n).collect();
    String::from_utf16_lossy(&units)
}

/// The BYTE offset just past the end of the line holding `needle`, or `None` when it is absent.
fn end_of_line_bytes(s: &str, needle: &str) -> Option<usize> {
    let at = s.find(needle)?;
    Some(s[at..].find('\n').map_or(s.len(), |i| at + i + 1))
}

/// The UTF-16 offset just past the end of the line holding `needle`, or `None` when it is absent.
fn end_of_line_with(s: &str, needle: &str) -> Option<usize> {
    let at = s.find(needle)?;
    let end = s[at..].find('\n').map_or(s.len(), |i| at + i + 1);
    Some(units(&s[..end]))
}

/// A floor line: `<kind> <count> · all: ...`, `· full text: ...` or `· not shown ...`.
fn is_floor(line: &str) -> bool {
    let mut words = line.splitn(3, ' ');
    let (Some(kind), Some(count), Some(rest)) = (words.next(), words.next(), words.next()) else {
        return false;
    };
    !kind.is_empty()
        && kind
            .chars()
            .all(|c| c.is_ascii_lowercase() || c == '-' || c == '#' || c.is_ascii_digit())
        && count.chars().all(|c| c.is_ascii_digit())
        && (rest.starts_with("· all: ")
            || rest.starts_with("· full text: ")
            || rest.starts_with("· not shown"))
}

/// The seed was read and the hook ran: without these, every assertion below measures nothing.
fn controls(code: i32, stdout: &str, stderr: &str) {
    assert_eq!(code, 0, "hooks fail open. stderr: {stderr}");
    assert!(
        !stdout.is_empty(),
        "the hook printed nothing. stderr: {stderr}"
    );
    assert!(
        stdout.contains("Seed reminder 0 is due") || stdout.contains("reminders 2 ·"),
        "the seed's due reminders are not in the output, so the seed was not read:\n{stdout}"
    );
}

/// J3.2 and A5. The header, the whole instruction block and the whole DUE NOW block sit inside the
/// first 2,000 UTF-16 units, so a spill never costs Claude the map or the commands. Red before
/// commit C: there is no header and no instruction block.
#[test]
fn the_header_instructions_and_due_now_fit_the_first_screen() {
    let (_, code, stdout, stderr) = real(REAL_BUDGET);
    controls(*code, stdout, stderr);
    println!("session start on the real-size seed: {}", measured(stdout));

    let line1 = stdout.lines().next().unwrap_or_default();
    assert!(
        line1.starts_with("[BASE START · 2 due · handoffs 24 open (10 shown) · forks 157 · projects 24 · tasks 115 · milestones 52 · withheld "),
        "line 1 is not the B2 header: {line1}"
    );
    let screen = first_units(stdout, 2000);
    for (what, needle) in [
        ("the header", "[BASE START ·"),
        (
            "the instruction block's first line",
            "DO THIS FIRST, BEFORE ANYTHING ELSE IN YOUR FIRST REPLY:",
        ),
        ("the instruction block's last line", "Letters: A="),
        (
            "DUE NOW's first line",
            "DUE NOW (2) · all: base reminder list",
        ),
        (
            "DUE NOW's last item",
            // `archive`, not `remove`, since lane 3 rank 06: a handled reminder is archived
            // and kept, and `remove` is a hard delete. Both commands exist; session start
            // changed which one it RECOMMENDS. The assertion below is unchanged - this
            // still pins DUE NOW's last item inside the first 2,000 UTF-16 units.
            "  2 Seed reminder 1 is due · clear: base reminder archive seed-reminder-1",
        ),
    ] {
        let end = end_of_line_with(stdout, needle)
            .unwrap_or_else(|| panic!("{what} is not in the output:\n{stdout}"));
        let end_bytes = end_of_line_bytes(stdout, needle)
            .unwrap_or_else(|| panic!("{what} is not in the output:\n{stdout}"));

        // BOTH UNITS, AND THIS IS A HEDGE AGAINST AN OPEN QUESTION RATHER THAN BELT AND BRACES.
        // DO NOT SIMPLIFY IT AWAY AS REDUNDANT.
        //
        // The host's LIMIT was measured as bytes on 2026-09-20. THE PREVIEW'S LENGTH WAS NEVER
        // MEASURED, in either unit. This assertion used to check UTF-16 alone, which is too weak in
        // exactly the direction that hides a real cut: 2,000 UTF-16 units of the box-drawing text
        // session start actually emits is up to 6,000 BYTES, so a byte-counted preview would cut
        // the map and the commands while this test stayed green.
        //
        // Asserting bytes INSTEAD would repeat the original error with the sign flipped. The defect
        // was never that anyone picked UTF-16 - it was that nobody measured and the code asserted
        // anyway. Both units is the only form that is true under either answer, and it needs no
        // measurement to justify. When the preview is finally measured, drop the other arm and say
        // which reading retired it.
        assert!(
            end <= 2000,
            "{what} ends at UTF-16 unit {end}, past the 2,000-unit preview. The preview:\n{screen}"
        );
        assert!(
            end_bytes <= 2000,
            "{what} ends at BYTE {end_bytes}, past a 2,000-BYTE preview. The preview unit is \
             unmeasured, so this arm holds if the host counts bytes. The preview:\n{screen}"
        );
    }
}

/// J3.3 and board ruling R4. Every B1 block is present at least as one line with its count and its
/// command, milestones included; on an unbounded budget each carries the seed's recent subsets.
/// Red before commit C: none of these block lines exist.
#[test]
fn every_b1_block_is_present_with_its_count_and_command() {
    let (_, code, stdout, stderr) = real(REAL_BUDGET);
    controls(*code, stdout, stderr);
    for (whole, floor) in [
        (
            "DUE NOW (2) · all: base reminder list",
            "reminders 2 · all: base reminder list",
        ),
        (
            "HANDOFFS (24 open, newest 10 shown) · all: base handoff list",
            "handoffs 24 · all: base handoff list",
        ),
        (
            "FORKS (157 open, newest 3 shown) · all: base fork list",
            "forks 157 · all: base fork list",
        ),
        (
            "PROJECTS (24 active, touched in 7 days: 5) · all: base project list --all",
            "projects 24 · all: base project list --all",
        ),
        (
            "TASKS (115 active, on projects touched in 7 days: 25) · all: base task list",
            "tasks 115 · all: base task list",
        ),
        (
            "MILESTONES (52 active, on projects touched in 7 days: 14) · all: base milestone list",
            "milestones 52 · all: base milestone list",
        ),
    ] {
        assert!(
            stdout.lines().any(|l| l == whole || l == floor),
            "neither the block nor its floor is present. wanted {whole:?} or {floor:?}:\n{stdout}"
        );
    }

    let (_, code, wide, stderr) = real(1_000_000);
    assert_eq!(*code, 0, "stderr: {stderr}");
    assert!(
        !wide.lines().any(is_floor),
        "an unbounded budget collapsed something:\n{wide}"
    );
    let listed = |header: &str| -> Vec<String> {
        let mut lines = wide.lines().skip_while(|l| !l.starts_with(header));
        assert!(lines.next().is_some(), "{header} is missing:\n{wide}");
        lines
            .take_while(|l| l.starts_with("  "))
            .map(str::to_string)
            .collect()
    };
    assert_eq!(
        listed("PROJECTS (").len(),
        5,
        "the five recent projects are listed"
    );
    assert!(
        listed("PROJECTS (")
            .iter()
            .all(|l| l.contains("— next: ship slice "))
    );
    let tasks = listed("TASKS (");
    assert_eq!(
        tasks.len(),
        25,
        "tasks on the five recent projects: {tasks:?}"
    );
    assert!(
        tasks.iter().all(|l| [
            "project 00",
            "project 01",
            "project 02",
            "project 03",
            "project 04"
        ]
        .iter()
        .any(|p| l.contains(&format!("for {p} · Project {}", &p[8..])))),
        "a listed task is not on a recent project: {tasks:?}"
    );
    assert_eq!(
        listed("MILESTONES (").len(),
        14,
        "milestones on the five recent projects"
    );
    assert_eq!(listed("FORKS (").len(), 3, "the three newest forks");
}

/// J3.4, B4 and B5. At most ten handoffs, lettered A to J in order, newest created first, one per
/// project with the S/W pair folded, and no path on any line. Red before commit C: 24 lines,
/// oldest first, a path on each.
#[test]
fn handoffs_are_ten_lettered_newest_first_one_per_project_without_paths() {
    let (seed, code, stdout, stderr) = real(REAL_BUDGET);
    controls(*code, stdout, stderr);
    let lines: Vec<&str> = stdout
        .lines()
        .skip_while(|l| !l.starts_with("HANDOFFS ("))
        .skip(1)
        .take_while(|l| l.starts_with("  "))
        .collect();
    assert_eq!(lines.len(), 10, "handoff lines:\n{}", lines.join("\n"));

    // The seed's open handoffs sorted by their written creation time, newest first, with the older
    // of the S/W pair (15, under 16) folded away. Worked out from `seed::handoff_created`.
    let expected = [22, 21, 20, 19, 18, 17, 16, 23, 7, 14];
    let letters_line = stdout
        .lines()
        .find(|l| l.starts_with("Letters: "))
        .expect("the instruction block carries the letters");
    let want_letters: Vec<String> = expected
        .iter()
        .zip('A'..='J')
        .map(|(i, l)| format!("{l}={}", seed::handoff_slug(*i, "handoff")))
        .collect();
    assert_eq!(letters_line, format!("Letters: {}", want_letters.join(" ")));

    let root = seed.ws.parent().expect("seed root").display().to_string();
    let mut projects = std::collections::HashSet::new();
    let mut ages = Vec::new();
    for ((line, i), letter) in lines.iter().zip(expected).zip('A'..='J') {
        let project = seed::handoff_project(i, &seed::REAL);
        let head = format!("  {letter}) {project} · seed · ");
        assert!(line.starts_with(&head), "line {line:?} is not {head:?}…");
        assert!(
            projects.insert(project.clone()),
            "{project} is listed twice"
        );
        assert!(
            !line.contains('/') && !line.contains(&root),
            "a path on {line:?}"
        );
        let tail = &line[head.len()..];
        let days: i64 = tail
            .split('d')
            .next()
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("no age in {line:?}"));
        ages.push(days);
        let folded = i == seed::SW_PAIR.1;
        assert_eq!(
            tail.ends_with(" (+1 older)"),
            folded,
            "{line:?}: only the newer half of the S/W pair carries the fold"
        );
    }
    assert!(
        ages.windows(2).all(|w| w[0] <= w[1]),
        "not newest first: ages {ages:?}"
    );
}

/// auk's ruling on flag 2: a slug without the `YYYY-MM-DD-HHMM-<codename>-<rest>` shape prints the
/// slug unchanged where the codename goes, never a guessed name. Red before commit C: the line is
/// `A) grazer · <doc path> · 0d` under `[Pick up where you left off]`.
#[test]
fn a_slug_without_the_codename_shape_prints_the_slug_unchanged() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let seed = seed::write(tmp.path(), &seed::TINY, "");
    let now = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let ns = seed::NS;
    let graph = seed.ws.join(".base").join("graph.nq");
    let s = format!("<{ns}handoff/HANDOFF-grazer-mantis>");
    let g = format!("<{ns}graph/ws/seed>");
    let dt = "<http://www.w3.org/2001/XMLSchema#dateTime>";
    let mut quads = std::fs::read_to_string(&graph).expect("seeded graph");
    let facts: [(String, String); 7] = [
        (
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#type".to_string(),
            format!("<{ns}Handoff>"),
        ),
        (format!("{ns}status"), "\"open\"".to_string()),
        (format!("{ns}project"), "\"grazer\"".to_string()),
        (format!("{ns}kind"), "\"handoff\"".to_string()),
        (
            format!("{ns}handoffDoc"),
            "\"/docs/HANDOFF-grazer-mantis.md\"".to_string(),
        ),
        (format!("{ns}createdAt"), format!("\"{now}\"^^{dt}")),
        (format!("{ns}resurfaceAt"), format!("\"{now}\"^^{dt}")),
    ];
    for (p, o) in &facts {
        quads.push_str(&format!("{s} <{p}> {o} {g} .\n"));
    }
    std::fs::write(&graph, quads).expect("graph written");

    let (code, stdout, stderr) = run_session_start(&seed, None);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(
        !stdout.is_empty(),
        "the hook printed nothing. stderr: {stderr}"
    );
    assert!(
        stdout
            .lines()
            .any(|l| l == "  A) grazer · HANDOFF-grazer-mantis · 0d"),
        "the newest handoff is not lettered A with its slug in the codename place:\n{stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|l| l.starts_with("  B) project-00 · seed · ")),
        "control: the shaped slug still yields its codename:\n{stdout}"
    );
}

/// T6's line-1 half and T8. Line 1 names the full-output file, which exists and holds more than was
/// printed, and carries the withheld total, which is non-zero whenever a floor was printed. Red
/// before commit C: line 1 is not a header.
#[test]
fn line_one_names_the_full_output_file_and_the_withheld_total() {
    let (seed, code, stdout, stderr) = real(REAL_BUDGET);
    controls(*code, stdout, stderr);
    let line1 = stdout.lines().next().unwrap_or_default();
    let file = seed.ws.join(".base").join("last-session-start.md");
    assert!(
        line1.ends_with(&format!(" · full: {}]", file.display())),
        "line 1 does not name the full-output file: {line1}"
    );
    let full = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    assert!(units(&full) > 0, "the full-output file is empty");

    let withheld: usize = line1
        .split(" · withheld ")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no withheld total on line 1: {line1}"));
    let floors: Vec<&str> = stdout.lines().filter(|l| is_floor(l)).collect();
    println!(
        "withheld {withheld}, floors {floors:?}, stdout {}",
        measured(stdout)
    );
    assert!(
        !floors.is_empty(),
        "control: 9,000 units cannot hold the real-size seed whole"
    );
    assert!(
        withheld > 0,
        "floors were printed and line 1 says nothing was withheld: {floors:?}"
    );
}

/// T7, the header half. In process: the withheld total on line 1 is the ledger's total. Red before
/// commit C: line 1 carries no total.
#[test]
fn the_withheld_total_on_line_one_is_the_ledgers() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join(".base")).expect(".base");
    let mut config = BaseConfig::default();
    config.budget.session_start_bytes = 300;
    let mut out = SessionOutput::new();
    out.push("pulse", &format!("[Pulse]\n{}", "p".repeat(2000)), 1);
    out.push(
        "triggers",
        &format!("<base-context-triggers>\n{}", "t".repeat(2000)),
        1,
    );
    let rendered = out.finish(&config, tmp.path());
    assert!(!rendered.text.is_empty(), "nothing rendered");
    let total = rendered.withheld_total();
    assert!(
        total > 0,
        "control: a 300-unit budget must collapse a 2,000-unit block: {}",
        rendered.text
    );
    let line1 = rendered.text.lines().next().unwrap_or_default();
    assert!(
        line1.contains(&format!(" · withheld {total} · ")),
        "line 1 does not carry the ledger's total {total}: {line1}"
    );
}

/// T7, replaced by commit E. DUE NOW shows in full, never collapsed, always first (the four-lane
/// table Chris accepted; `auk`'s cross-lane ruling, lane doc B24). Eighty due reminders push the
/// header, the instruction block and DUE NOW past the 2,000-unit first screen. Nothing inside that
/// prefix may shrink, so the overflow is reported on stderr and what the budget takes comes after
/// DUE NOW. Red before commit E: DUE NOW collapsed to `reminders 80 · all: base reminder list`.
#[test]
fn due_now_stays_whole_and_first_when_it_overflows_the_first_screen() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let crowded = seed::Sizes {
        due_reminders: 80,
        ..seed::REAL
    };
    let seed = seed::write(
        tmp.path(),
        &crowded,
        "[budget]\nsession_start_chars = 9000\n",
    );
    let (code, stdout, stderr) = run_session_start(&seed, None);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(
        !stdout.is_empty(),
        "the hook printed nothing. stderr: {stderr}"
    );
    let line1 = stdout.lines().next().unwrap_or_default();
    assert!(
        line1.starts_with("[BASE START · 80 due · "),
        "line 1: {line1}"
    );
    let lines: Vec<&str> = stdout.lines().collect();

    // Whole: the block's first line, then all eighty reminders, oldest due first, one per line.
    // Matched on the line's start, so the `clear:` needle lane 3 rewrites is not pinned here twice.
    let head = lines
        .iter()
        .position(|l| *l == "DUE NOW (80) · all: base reminder list")
        .unwrap_or_else(|| panic!("DUE NOW is not whole:\n{stdout}"));
    for i in 0..80 {
        let want = format!("  {} Seed reminder {i} is due · ", i + 1);
        let got = lines.get(head + 1 + i).copied().unwrap_or_default();
        assert!(
            got.starts_with(&want),
            "DUE NOW line {} is {got:?}, wanted it to start {want:?}",
            i + 1
        );
    }
    assert!(
        !lines.contains(&"reminders 80 · all: base reminder list"),
        "DUE NOW collapsed to its floor:\n{stdout}"
    );

    // The overflow is real. A seed that fits the first screen would pass every line above on a
    // tree that still collapses DUE NOW, which is the vacuity this assertion exists to rule out.
    let due_end = end_of_line_with(&stdout, "  80 Seed reminder 79 is due")
        .expect("the eightieth reminder is present");
    assert!(
        due_end > 2000,
        "control: DUE NOW ends at unit {due_end}, inside the first screen, so nothing overflowed"
    );

    // First: only the header and the instruction block come before it.
    let letters = lines
        .iter()
        .position(|l| l.starts_with("Letters: "))
        .expect("the instruction block carries the letters");
    assert!(
        letters < head,
        "DUE NOW starts before the instruction block ends"
    );
    assert!(
        lines[letters + 1..head].iter().all(|l| l.trim().is_empty()),
        "something sits between the instruction block and DUE NOW: {:?}",
        &lines[letters + 1..head]
    );

    // Reported, not resolved: stderr is the only signal of an overflow nothing can trim away.
    assert!(
        stderr.contains(
            "base: session start's header, instructions and DUE NOW take more than the first 2000 units"
        ),
        "the overflow is not reported. stderr: {stderr}"
    );

    // What fell came after it: the budget still trimmed, and every floor sits below DUE NOW.
    let floors: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| is_floor(l))
        .map(|(n, _)| n)
        .collect();
    assert!(
        !floors.is_empty(),
        "control: 9,000 units cannot hold eighty reminders and the real-size seed whole"
    );
    assert!(
        floors.iter().all(|n| *n > head + 80),
        "a floor sits above DUE NOW's last line: floors at {floors:?}, DUE NOW ends at line {}",
        head + 80
    );
    let withheld: usize = line1
        .split(" · withheld ")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no withheld total on line 1: {line1}"));
    assert!(
        withheld > 0,
        "floors were printed and line 1 says nothing was withheld: {line1}"
    );
    assert!(
        stdout.contains("DO THIS FIRST, BEFORE ANYTHING ELSE IN YOUR FIRST REPLY:"),
        "control: the instruction block never collapses"
    );
}
