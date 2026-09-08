//! #89: the extractor's notices survive a build nobody was watching.
//!
//! Every background AST refresh discards its child's output on purpose —
//! `spawn_sync`, `first_contact_wait` and `delegate_wsl_contact` all give it
//! `Stdio::null()`, and the git hook `ast_repo` writes redirects to `/dev/null`.
//! That is the right default for a detached process, and it meant the counter
//! #66 added was silent on the path that runs most often: a tree could
//! accumulate skipped files and unplaceable entities for weeks and say nothing.
//!
//! All four of those paths spawn the same `base sync --ast --yes`, so the fix is
//! in the child: it records what it could not do next to the map, the way a
//! failed build already records why it failed in `.last-error`.
//!
//! Filesystem only — no tree-sitter grammar, no python. These ride `cargo test`.

use base::hook::automap::{pending_notices, record_notices};
use std::fs;
use std::path::Path;

fn base_ast(dir: &Path) -> &Path {
    dir
}

/// The two notices worth a person's attention are kept; the one every single
/// successful run writes is not. A line on every turn is noise, and 0.14.1 went
/// out of its way the other direction.
#[test]
fn only_the_notices_worth_saying_are_kept() {
    let tmp = tempfile::tempdir().unwrap();
    let d = base_ast(tmp.path());

    record_notices(
        d,
        b"# Extracting 113 files from /x\n\
          # Skipped /x/a.vue: unparsable\n\
          # 89 entities attributed to the app root (no file node)\n",
    );
    let text = fs::read_to_string(d.join(".last-notices")).unwrap();
    assert!(text.contains("# Skipped /x/a.vue"), "a skipped file is worth saying:\n{text}");
    assert!(
        text.contains("89 entities attributed to the app root"),
        "the #66 counter is the whole point of persisting these:\n{text}"
    );
    assert!(
        !text.contains("Extracting 113 files"),
        "the routine per-run line would become a permanent banner:\n{text}"
    );
}

/// Silence has to stay reachable. A clean run REMOVES the record rather than
/// leaving the last dirty one to be reported forever.
#[test]
fn a_clean_run_clears_the_record() {
    let tmp = tempfile::tempdir().unwrap();
    let d = base_ast(tmp.path());

    record_notices(d, b"# Skipped /x/a.vue: unparsable\n");
    assert!(d.join(".last-notices").is_file());

    record_notices(d, b"# Extracting 4 files from /x\n");
    assert!(
        !d.join(".last-notices").exists(),
        "a build with nothing to report must leave nothing to report"
    );
    assert!(pending_notices(d).is_none(), "and must say nothing at session start");
}

/// Shown once per distinct set. The same eleven skipped files every turn is one
/// fact, not eleven notices; a CHANGED set is new information.
#[test]
fn a_notice_is_shown_once_and_a_changed_one_again() {
    let tmp = tempfile::tempdir().unwrap();
    let d = base_ast(tmp.path());

    record_notices(d, b"# Skipped /x/a.vue: unparsable\n");
    let first = pending_notices(d).expect("a fresh notice is shown");
    assert!(first.contains("Skipped /x/a.vue"), "{first}");
    assert!(!first.starts_with("# "), "the '# ' marker is the extractor's, not a person's: {first}");

    assert!(pending_notices(d).is_none(), "the same set must not repeat");

    record_notices(d, b"# Skipped /x/b.svelte: unparsable\n");
    assert!(pending_notices(d).is_some(), "a different set is new information");
}

/// More than one notice collapses to one line with a count: session start has a
/// one-line budget and the file holds the rest.
#[test]
fn several_notices_become_one_line() {
    let tmp = tempfile::tempdir().unwrap();
    let d = base_ast(tmp.path());

    record_notices(
        d,
        b"# Skipped /x/a.vue: unparsable\n# Skipped /x/b.vue: unparsable\n# 3 entities attributed to the app root (no file node)\n",
    );
    let line = pending_notices(d).unwrap();
    assert!(line.contains("(+2 more)"), "expected a count of the rest, got: {line}");
    assert_eq!(line.lines().count(), 1, "session start gets one line: {line}");
}

/// An absent record says nothing at all, which is the state almost every tree is
/// in almost all of the time.
#[test]
fn no_record_is_silent() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(pending_notices(tmp.path()).is_none());
}

/// A2 (auk's G0 amendment): there are TWO `session_start_notice` functions and
/// `hook::session_start` calls them four lines apart —
/// `crate::update::session_start_notice()` (no arguments, the binary-version
/// notice) and `crate::hook::automap::session_start_notice(cwd)` (the code-map
/// one, which #89 edits). A collision this close is how a fix silently replaces
/// the wrong caller, so both are pinned here by their distinct signatures. This
/// compiles only while both exist and take what they take.
#[test]
fn both_session_start_notices_still_exist() {
    let update_notice: fn() -> Option<String> = base::update::session_start_notice;
    let automap_notice: fn(&Path) -> Option<String> = base::hook::automap::session_start_notice;

    // Called for real, on a directory that is neither an app nor a repo: the
    // automap one must return None there rather than panic, which is also the
    // path every non-code cwd takes at session start.
    let tmp = tempfile::tempdir().unwrap();
    let _ = update_notice();
    let _ = automap_notice(tmp.path());
}
