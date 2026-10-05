//! Rank 04: the memory block's own budget, read off what the session-start hook writes, from the
//! binary `cargo test` builds.
//!
//! The first test runs red at commit C's head, where the block ignores `[budget] memory_chars`,
//! caps its text at 4,000 bytes and ends with `... and N more notes`. The other two guard the
//! signal's early return, which no test read before this file (lane doc B19, fact 4), one branch
//! each.

mod seed;

use std::sync::OnceLock;

use seed::{measured, run_session_start, units};

/// Below the 3,147 units commit C's block takes on the real-size seed, above what two whole notes
/// and the count line take.
const MEMORY_BUDGET: usize = 3000;

/// The memory section `seed::write` puts in the global `base.toml`.
const SEEDED_MEMORY: &str = "[memory]\nenabled = true\nmode = \"both\"\n";

/// The lines of the memory block in `text`, from `<base-memory>` to `</base-memory>` inclusive.
fn memory_block(text: &str) -> Option<Vec<&str>> {
    let lines: Vec<&str> = text.lines().collect();
    let open = lines.iter().position(|l| *l == "<base-memory>")?;
    let close = open + lines[open..].iter().position(|l| *l == "</base-memory>")?;
    Some(lines[open..=close].to_vec())
}

/// T11 (lane doc G0.7 and B19). On the real-size seed the memory block fits `[budget]
/// memory_chars`, prints whole notes, corrections newest first, stops at the first note that does
/// not fit, and ends with a line counting the notes left out of all the notes that exist, never of
/// what one query read. Red at commit C's head: the block takes about 3,147 units there.
#[test]
fn the_memory_block_fits_its_own_budget_in_whole_notes_and_counts_the_rest() {
    let root = std::env::temp_dir().join(format!("base-seed-memory-real-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let seed = seed::write(
        &root,
        &seed::REAL,
        &format!("[budget]\nmemory_chars = {MEMORY_BUDGET}\n"),
    );
    let (code, stdout, stderr) = run_session_start(&seed, None);
    assert_eq!(code, 0, "hooks fail open. stderr: {stderr}");
    assert!(
        !stdout.is_empty(),
        "the hook printed nothing. stderr: {stderr}"
    );
    let full_path = seed.ws.join(".base").join("last-session-start.md");
    let full = std::fs::read_to_string(&full_path).unwrap_or_else(|e| {
        panic!(
            "no full-output file at {}: {e}. stderr: {stderr}",
            full_path.display()
        )
    });
    let block = memory_block(&full).unwrap_or_else(|| {
        panic!("the seeded notes printed no memory block. stderr: {stderr}\n{full}")
    });
    let text = block.join("\n");
    let notes: Vec<&str> = block
        .iter()
        .copied()
        .filter(|l| l.starts_with("- ["))
        .collect();
    assert!(
        !notes.is_empty(),
        "control: the seed's notes were read and none printed:\n{text}"
    );

    assert!(
        units(&text) <= MEMORY_BUDGET,
        "the memory block is over [budget] memory_chars = {MEMORY_BUDGET}: {}\n{text}",
        measured(&text)
    );

    // Every note line is one seeded note, whole, under its own label.
    let mut shown: Vec<usize> = Vec::new();
    for line in &notes {
        let (label, rest) = line[3..]
            .split_once("] ")
            .unwrap_or_else(|| panic!("a note line without a label: {line}"));
        let i: usize = rest
            .strip_prefix("note ")
            .and_then(|r| r.split(' ').next())
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("a note line that names no seeded note: {line}"));
        assert_eq!(
            label,
            seed::note_type(i),
            "note {i} is printed under the wrong label"
        );
        assert!(
            rest == seed::note_text(&seed::REAL, i),
            "note {i} is not printed whole: {} printed, {} seeded",
            units(rest),
            units(&seed::note_text(&seed::REAL, i))
        );
        shown.push(i);
    }

    // Corrections, newest first, ties to the lower IRI: the notes the block reaches first.
    let mut corrections: Vec<usize> = (0..seed::REAL.notes)
        .filter(|&i| seed::note_type(i) == "correction")
        .collect();
    corrections.sort_by(|&a, &b| {
        seed::note_created(b)
            .cmp(&seed::note_created(a))
            .then(a.cmp(&b))
    });
    assert_eq!(
        shown,
        corrections[..shown.len()],
        "the block did not print the newest corrections in order"
    );

    // The fill stopped at the first note that did not fit.
    let next = corrections[shown.len()];
    let next_line = format!("- [correction] {}\n", seed::note_text(&seed::REAL, next));
    assert!(
        units(&text) + units(&next_line) > MEMORY_BUDGET,
        "note {next} would still have fit: {} units printed, its line {}\n{text}",
        units(&text),
        units(&next_line)
    );

    // The last line counts what was left out, of every note, and names the command that lists them.
    assert_eq!(
        block[block.len() - 2],
        format!(
            "{} notes withheld · all: base learn --list",
            seed::REAL.notes - shown.len()
        ),
        "the count line\n{text}"
    );
}

/// A TINY seed whose global `base.toml` carries `memory` in place of the seeded memory section.
/// Exit code, stdout, stderr and the full-output file.
fn tiny_run(tag: &str, memory: &str) -> (i32, String, String, String) {
    let root = std::env::temp_dir().join(format!("base-seed-memory-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let seed = seed::write(&root, &seed::TINY, "");
    let toml_path = seed.home.join(".base-gbl").join("base.toml");
    let written = std::fs::read_to_string(&toml_path).expect("the seed's global base.toml");
    assert_eq!(
        written.matches(SEEDED_MEMORY).count(),
        1,
        "the seed's memory section is not where this test replaces it:\n{written}"
    );
    std::fs::write(&toml_path, written.replace(SEEDED_MEMORY, memory))
        .expect("the memory section is replaced");
    let (code, stdout, stderr) = run_session_start(&seed, None);
    let full = std::fs::read_to_string(seed.ws.join(".base").join("last-session-start.md"))
        .unwrap_or_default();
    (code, stdout, stderr, full)
}

/// The control both guards compare against: the TINY seed with its memory section as seeded
/// prints a memory block.
fn memory_on() -> &'static (i32, String, String, String) {
    static ON: OnceLock<(i32, String, String, String)> = OnceLock::new();
    ON.get_or_init(|| tiny_run("on", SEEDED_MEMORY))
}

/// The memory section in `memory` prints no memory block, in stdout or in the full-output file,
/// while the control run prints one.
fn assert_no_memory_block(tag: &str, memory: &str) {
    let (code, _, stderr, full) = memory_on();
    assert_eq!(*code, 0, "control: hooks fail open. stderr: {stderr}");
    assert!(
        memory_block(full).is_some(),
        "control: the memory section as seeded printed no block. stderr: {stderr}\n{full}"
    );
    let (code, stdout, stderr, full) = tiny_run(tag, memory);
    assert_eq!(code, 0, "hooks fail open. stderr: {stderr}");
    assert!(
        !stdout.is_empty(),
        "the hook printed nothing. stderr: {stderr}"
    );
    assert!(
        !full.is_empty(),
        "the full-output file was not written. stderr: {stderr}"
    );
    assert!(
        !stdout.contains("<base-memory>") && !full.contains("<base-memory>"),
        "{memory:?} printed a memory block:\n{full}"
    );
}

/// C22 (lane doc B19): memory switched off prints no memory block. Holds at commit C's head, so it
/// is proven by mutation: drop `!enabled` from the early return.
#[test]
fn memory_switched_off_prints_no_memory_block() {
    assert_no_memory_block("off", "[memory]\nenabled = false\nmode = \"both\"\n");
}

/// C22: memory in `claude` mode prints no memory block, because Claude's own memory is in use.
/// Holds at commit C's head, so it is proven by mutation: drop the mode check from the early
/// return.
#[test]
fn memory_in_claude_mode_prints_no_memory_block() {
    assert_no_memory_block("claude", "[memory]\nenabled = true\nmode = \"claude\"\n");
}
