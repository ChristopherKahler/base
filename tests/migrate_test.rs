//! Rank 09: the upgrade migration (`base defer migrate`), end to end through the CLI.
//!
//! These drive the REAL command surface rather than calling `migrate::apply` directly, because the
//! blocking defect (`auk` G2, 2026-09-18) lived at the CLI/apply boundary: `cli.rs` staged the plan
//! and discarded the full one, so `apply` could not tell "nothing to do" from "you filtered
//! everything out". A unit test that handed `apply` a plan built in the test would have passed
//! while the shipped command was broken.
//!
//! WHAT EACH TEST CAN AND CANNOT SHOW. Every test here asserts on the STATE THE OPERATOR IS LEFT IN,
//! not on the wording of a message. A test pinned to wording goes red when the sentence improves and
//! green when the behaviour breaks, which is backwards.
//!
//! THE CONTROL PROBLEM, and it is the one that matters here. Every assertion below is of the shape
//! "the records were NOT deferred". That is also what you get from a pass that never ran at all. So
//! each migration test is paired with a control that proves the deferral pass DOES fire on this
//! fixture once the migration is complete. Without it, deleting the whole pass would make this file
//! green (law 12's unit-before-number, and the same defect MR3 found in rank 10).

mod seed;

use seed::{run_base, run_session_start};

/// Deferral on, with the default windows. Projects stay behind `[protocol] enabled`, which the seed
/// leaves off, so nothing here can be a project-pass result by accident.
const DEFER_ON: &str = "[defer]\nenabled = true\n";

/// Enough open handoffs that `handoff_created(i) = at(i * 97)` hours spreads them across the
/// deferral window: the oldest are cold, the newest are not. That spread is what makes
/// `--older-than` meaningful rather than all-or-nothing.
const SPREAD: seed::Sizes = seed::Sizes {
    open_handoffs: 8,
    archived_handoffs: 0,
    open_forks: 0,
    archived_forks: 0,
    tasks: 0,
    milestones: 0,
    due_reminders: 0,
    ..seed::TINY
};

// `EMPTY` WAS HERE — a fixture holding nothing that carries a clock, the shape of a genuinely fresh
// install. Its only user was `a_fresh_install_with_no_records_is_marked_applied`, deleted with
// `mark_fresh_install` at the bottom of this file, so it went with it rather than sit unused.

fn workspace(tag: &str) -> seed::Seed {
    let root = std::env::temp_dir().join(format!("base-r09-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    seed::write(&root, &SPREAD, DEFER_ON)
}

/// How many records the migration says are waiting, read from its own preview.
fn waiting(s: &seed::Seed) -> usize {
    let (rc, out, err) = run_base(s, &["defer", "migrate"]);
    assert_eq!(rc, 0, "the preview should never fail:\nstdout:\n{out}\nstderr:\n{err}");
    if out.contains("nothing to reset") {
        return 0;
    }
    out.split_whitespace()
        .find_map(|w| w.parse::<usize>().ok())
        .unwrap_or_else(|| panic!("the preview named no count:\n{out}"))
}

/// Whether the migration is still pending.
///
/// IT NO LONGER READS THIS FROM AN OPERATOR-VISIBLE SURFACE, BECAUSE THERE ISN'T ONE. Until
/// 2026-09-19 this scraped `base doctor` for the string "deferral upgrade migration PENDING". Item 1
/// removed both of doctor's migration warnings, so after that change no command anywhere states
/// whether the migration ran. That loss is deliberate and recorded at the removal site in
/// `doctor.rs`; this helper reads the marker directly because nothing else is left to read.
///
/// WHY THIS REWRITE IS THE DANGEROUS PART OF ITEM 1, stated where the next reader will find it.
/// Leaving the old body in place would NOT have failed loudly. `out.contains(..)` on output that can
/// no longer contain it returns `false` forever, so `pending()` becomes a constant. The five
/// `assert!(pending(..))` sites then go red and get noticed; the three `assert!(!pending(..))` sites
/// PASS NO MATTER WHAT THE COMMAND DID. Three assertions about whether `base defer migrate` marked
/// the migration would have stopped testing anything, silently, inside the change that removed the
/// display they depended on. This is the same shape as the `tree_after` defect: a value that reads
/// the same in two different states and gets believed.
///
/// THE MARKER ROOT COMES FROM THIS SEED, never from `migrate::marker_root`. `marker_root` resolves
/// through `home_root()`, which under the `isolation-guard` feature returns a path belonging to the
/// TEST PROCESS rather than to the seed — the order-dependence measured in this file on 2026-09-18,
/// where a pair read GREEN in a full-suite run and RED in a targeted run on the same commit.
fn pending(s: &seed::Seed) -> bool {
    let root = s.home.join(".base-gbl").join(".base");
    matches!(
        base::protocol::migrate::state(&root),
        base::protocol::migrate::State::Pending
    )
}

/// How many records a session start deferred. This is the number every test here is really about.
fn deferred_by_session_start(s: &seed::Seed) -> usize {
    let (_, _, err) = run_session_start(s, None);
    err.lines()
        .find(|l| l.starts_with("base: defer —") && l.contains("deferred"))
        .and_then(|l| {
            l.split_whitespace()
                .nth(3)
                .and_then(|w| w.parse::<usize>().ok())
        })
        .unwrap_or(0)
}

// ── THE CONTROL ─────────────────────────────────────────────────────────────────
// Everything below asserts records were NOT deferred. This proves that is a result and not the
// silence of a pass that never fires.

/// POSITIVE CONTROL for this whole file: the deferral pass DOES fire on this fixture and DOES
/// defer. If this goes red, every "was not deferred" assertion below is worthless and must be read
/// as VOID rather than as a pass.
///
/// REWRITTEN 2026-09-19, and the old version is why this comment is long. It used to run
/// `--apply` first and then assert that session-start stderr did NOT contain "NOTHING HAS BEEN
/// WRITTEN" — that is, that the write block had lifted. Item 1 deleted the line carrying that
/// string, so the assertion could never fail again: a control that cannot go red is not a control
/// (lane rule 63). Carrying it would have left this file's "was not deferred" assertions resting on
/// an instrument that had quietly stopped measuring.
///
/// It now asserts the thing directly, with no migration run at all: cold records exist, and a
/// session start defers them. That is also the headline behaviour change of item 1 — before the
/// removal this fixture deferred NOTHING until an operator previewed, and the count here was zero.
#[test]
fn control_the_deferral_pass_fires_on_this_fixture_and_defers() {
    let s = workspace("control");
    let full = waiting(&s);
    assert!(full > 0, "the fixture holds no records that would defer, so nothing here is testable");
    assert!(
        pending(&s),
        "control: this fixture must start un-migrated, or the count below proves nothing about a \
         machine that never ran the migration"
    );
    let deferred = deferred_by_session_start(&s);
    assert!(
        deferred > 0,
        "session start deferred NOTHING on a fixture holding {full} cold records with no migration \
         run. Either the defer pass is not firing, in which case every 'was not deferred' assertion \
         in this file is VOID, or something still gates it — which is the gate item 1 removed"
    );
}

// ── DEFECT 1: the blocking one ──────────────────────────────────────────────────

// `a_staged_apply_leaves_the_migration_pending_so_the_rest_cannot_defer` WAS HERE and DIED WITH
// THE GATE, 2026-09-19.
//
// It was the headline test of the blocking defect: a staged `--apply` must not mark the whole
// migration applied, because the records the operator did not stage would then defer on the next
// session start. Its first assertion — that a partial apply leaves the state PENDING — is still
// true and is still covered, by `rollback_works_after_a_partial_run_even_though_the_state_is_still_pending`
// below. Its second assertion required that session start defer NOTHING afterwards, and that is
// exactly what item 1 removed: cold records now defer whether or not a migration ran.
//
// So this test did not weaken, it INVERTED, and the surviving half is not lost.
/// The sharper form, which writes nothing at all and is therefore not even recoverable.
///
/// PRE-FIX BEHAVIOUR: the staged plan was empty, `apply` took the `is_empty` branch, marked Applied
/// with no snapshots, lifted the block — and `--rollback` then had nothing to restore, so the state
/// flip could not be undone through the CLI at all.
#[test]
fn a_filter_that_matches_nothing_refuses_and_does_not_mark_the_migration_applied() {
    let s = workspace("filtered");
    let full = waiting(&s);
    assert!(full > 0, "need waiting records for the filter to exclude");

    let (rc, out, err) = run_base(&s, &["defer", "migrate", "--apply", "--older-than", "9999"]);
    assert_ne!(rc, 0, "a filter that matched nothing must REFUSE, not succeed:\nstdout:\n{out}");
    assert!(
        pending(&s),
        "refusing must leave the migration PENDING; marking it here would record a migration as \
         done having written nothing. stderr was:\n{err}"
    );
    // THE SESSION-START ASSERTION THAT USED TO CLOSE THIS TEST WAS REMOVED 2026-09-19. It required
    // `deferred_by_session_start(&s) == 0` after a refused migration — true only while the gate
    // withheld `Defer` on an un-migrated machine. Item 1 removed the gate, so cold records now defer
    // regardless, and that assertion INVERTED rather than weakened. What this test is actually about
    // — that a filter matching nothing refuses and records nothing — is asserted above, and is
    // unaffected.
}

/// The one empty case that MAY mark: there is genuinely nothing to reset. Same count as the test
/// above, opposite fact, and the pair is the whole defect.
#[test]
fn an_empty_plan_marks_applied_because_there_is_genuinely_nothing_to_reset() {
    let s = workspace("empty");
    // Take everything, so the second run has nothing left to do.
    let (rc, _, err) = run_base(&s, &["defer", "migrate", "--apply"]);
    assert_eq!(rc, 0, "the first apply should succeed: {err}");
    assert_eq!(waiting(&s), 0, "everything should have been reset by the unstaged apply");

    let (rc, out, _) = run_base(&s, &["defer", "migrate", "--apply"]);
    assert_eq!(rc, 0, "an empty plan is not a refusal — there is nothing to filter out");
    assert!(out.contains("nothing to reset"), "expected the nothing-to-do wording:\n{out}");
    assert!(!pending(&s), "an empty plan may mark the migration applied");
}

/// A complete run marks, and leaves nothing waiting. The other half of the pair above.
///
/// RENAMED 2026-09-19 from `a_complete_apply_marks_applied_and_lifts_the_write_block`. There is no
/// write block left to lift; both assertions in the body are unchanged and both still hold.
#[test]
fn a_complete_apply_marks_applied_and_leaves_nothing_waiting() {
    let s = workspace("complete");
    let full = waiting(&s);
    assert!(full > 0);
    let (rc, out, err) = run_base(&s, &["defer", "migrate", "--apply"]);
    assert_eq!(rc, 0, "stdout:\n{out}\nstderr:\n{err}");
    assert!(!pending(&s), "a complete apply must mark the migration applied");
    assert_eq!(waiting(&s), 0, "nothing should be left waiting after a complete apply");
}

// ── G4 steps 1 and 5: the snapshot and the rollback ─────────────────────────────

/// A PARTIAL run must still be rollbackable. Its snapshots are recorded even though the state stays
/// Pending — if they were not, the half already written could never be undone.
#[test]
fn rollback_works_after_a_partial_run_even_though_the_state_is_still_pending() {
    let s = workspace("partial-rollback");
    let full = waiting(&s);
    assert!(full >= 2, "need at least two waiting records, got {full}");

    let (rc, _, err) = run_base(&s, &["defer", "migrate", "--apply", "--limit", "1"]);
    assert_eq!(rc, 0, "partial apply failed: {err}");
    assert!(pending(&s), "control: a partial run leaves the migration pending");

    let (rc, out, err) = run_base(&s, &["defer", "migrate", "--rollback"]);
    assert_eq!(rc, 0, "rollback after a partial run must work:\nstdout:\n{out}\nstderr:\n{err}");
    assert_eq!(
        waiting(&s),
        full,
        "rollback should put every record back, so the full count returns to what it was"
    );
}

/// `--apply` and `--rollback` together are two writes in opposite directions. Guessing which wins is
/// how an operator loses a tier, so the command refuses and names both.
#[test]
fn apply_and_rollback_together_refuse_rather_than_guessing() {
    let s = workspace("both-flags");
    let before = waiting(&s);
    let (_, _, err) = run_base(&s, &["defer", "migrate", "--apply", "--rollback"]);
    assert!(
        err.contains("opposite"),
        "expected a refusal naming both flags, got:\n{err}"
    );
    assert_eq!(waiting(&s), before, "the refused command must not have written anything");
    assert!(pending(&s), "the refused command must not have changed the migration state");
}

// ── THE LEGACY-INSTALL PAIR WAS HERE. BOTH TESTS DIED WITH `mark_fresh_install`, 2026-09-19 ──
//
//   a_legacy_install_with_records_and_no_marker_is_not_marked_applied
//   a_fresh_install_with_no_records_is_marked_applied
//
// They were a COMPLEMENT and only worked as a pair: same call, same absent marker, opposite record
// populations, opposite required answers. Both called `base::protocol::migrate::mark_fresh_install`
// directly, and item 1 deleted that function. They cannot be adapted, because the condition they
// pinned — mark APPLIED on an empty plan, stay PENDING on a non-empty one — exists nowhere now:
// `base install` no longer touches migration state at all.
//
// WHAT THE PAIR WAS GUARDING, and why losing it costs nothing HERE: it stopped a legacy install
// being recorded as migrated having never migrated, because that lifted the write gate and let the
// next session start mass-defer every record. There is no write gate to lift. The mass defer it
// feared is now the designed behaviour, recoverable per record, and `deferral_test`'s renamed
// survivor is the test that pins it.
//
// THE ORDER-DEPENDENCE LESSON THEY CARRIED IS NOT LOST. Both spelled their marker root out of the
// seed rather than calling `migrate::marker_root`, because `marker_root` resolves through
// `home_root()` and under `isolation-guard` that is the TEST PROCESS's path, not the seed's — so the
// pair shared one marker root and read GREEN in a full-suite run and RED in a targeted run on the
// same commit. That reasoning now lives on `pending()` above, which is the only thing in this file
// still reading a marker.