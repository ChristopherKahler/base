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

/// Nothing that carries a clock: the shape of a genuinely fresh install.
const EMPTY: seed::Sizes = seed::Sizes {
    open_handoffs: 0,
    archived_handoffs: 0,
    open_forks: 0,
    archived_forks: 0,
    tasks: 0,
    milestones: 0,
    due_reminders: 0,
    ..seed::TINY
};

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

/// Whether the migration is still pending, read the way the operator would read it.
fn pending(s: &seed::Seed) -> bool {
    let (_, out, _) = run_base(s, &["doctor"]);
    out.contains("deferral upgrade migration PENDING")
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

/// POSITIVE CONTROL for this whole file. Once the migration is complete the deferral pass DOES run
/// on this fixture and DOES defer. If this goes red, every "was not deferred" assertion below is
/// worthless and must be read as VOID, not as a pass.
#[test]
fn control_the_deferral_pass_fires_on_this_fixture_once_the_migration_is_complete() {
    let s = workspace("control");
    let full = waiting(&s);
    assert!(full > 0, "the fixture holds no records that would defer, so nothing here is testable");
    let (rc, out, err) = run_base(&s, &["defer", "migrate", "--apply"]);
    assert_eq!(rc, 0, "a complete apply should succeed:\nstdout:\n{out}\nstderr:\n{err}");
    assert!(!pending(&s), "the migration should be complete after an unstaged --apply");
    // The clocks were just reset to now, so nothing defers TODAY. The control is that the pass is
    // no longer blocked: it runs and reports, rather than returning early.
    let (_, _, err) = run_session_start(&s, None);
    assert!(
        !err.contains("NOTHING HAS BEEN WRITTEN"),
        "the write block should be lifted after a complete migration:\n{err}"
    );
}

// ── DEFECT 1: the blocking one ──────────────────────────────────────────────────

/// THE HEADLINE DEFECT. Staging must not mark the whole migration applied.
///
/// PRE-FIX BEHAVIOUR this is red against: `cli.rs` discarded the full plan, `apply` called
/// `mark_applied` unconditionally, the write block lifted, and every record the operator did NOT
/// stage deferred on the next session start — the mass defer K13 exists to prevent, reached through
/// the staging feature that exists to make the operator safe.
#[test]
fn a_staged_apply_leaves_the_migration_pending_so_the_rest_cannot_defer() {
    let s = workspace("staged");
    let full = waiting(&s);
    assert!(full >= 2, "need at least two waiting records to stage a partial run, got {full}");

    let (rc, out, err) = run_base(&s, &["defer", "migrate", "--apply", "--limit", "1"]);
    assert_eq!(rc, 0, "a partial apply is not an error:\nstdout:\n{out}\nstderr:\n{err}");

    assert!(
        pending(&s),
        "a PARTIAL apply must leave the migration PENDING, or the records the operator did not \
         take lose their protection. doctor said:\n{}",
        run_base(&s, &["doctor"]).1
    );
    assert_eq!(
        deferred_by_session_start(&s),
        0,
        "session start deferred records after a partial migration: this is the mass defer K13 \
         exists to prevent, reached through the staging feature"
    );
}

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
        "refusing must leave the migration PENDING; marking it here would lift the write block \
         having written nothing. stderr was:\n{err}"
    );
    assert_eq!(
        deferred_by_session_start(&s),
        0,
        "session start deferred records after a refused migration"
    );
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

/// A complete run marks, and the block lifts. The other half of the pair above.
#[test]
fn a_complete_apply_marks_applied_and_lifts_the_write_block() {
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

// ── THE LEGACY-INSTALL PAIR ─────────────────────────────────────────────────────
// Added on auk's condition after he found a hole in my first fix. These two are a COMPLEMENT: same
// call, same absent marker, opposite record populations, opposite required answers. Neither is worth
// much alone — the first could pass because the function never marks anything, and the second could
// pass because it marks everything. Only the pair pins the condition.

/// A LEGACY install must NOT be marked applied. This is the one Chris gated the release on.
///
/// The hole this pins: my first `mark_fresh_install` marked APPLIED whenever the marker was ABSENT.
/// A legacy user has no marker — that IS the population — so running `base install` after upgrading
/// (routine, to rewire hooks) would have recorded their migration as done having never run. The
/// block lifts, their records still carry `lastActive == createdAt`, and the next session start
/// mass-defers all of them. An absent marker was a PROXY for "nothing to migrate"; the empty plan is
/// that fact.
#[test]
fn a_legacy_install_with_records_and_no_marker_is_not_marked_applied() {
    let s = workspace("legacy-install");
    let full = waiting(&s);
    assert!(full > 0, "control: the fixture must hold records that would defer, got {full}");

    // THE MARKER ROOT COMES FROM THIS SEED, never from `migrate::marker_root`.
    //
    // `marker_root` resolves through `home_root()`, which under the `isolation-guard` feature
    // returns a path belonging to the TEST PROCESS rather than to the seed. Both tests in this pair
    // then shared ONE marker root: whichever ran first left a marker, and the second one's
    // `mark_fresh_install` returned early at its `if base_dir.join(MARKER).exists()` line and read
    // the other test's state as its own.
    //
    // Measured 2026-09-18: this pair read GREEN in a full-suite run and RED in a targeted run on the
    // SAME commit. A test whose result depends on execution order was never evidence in either
    // direction, and this is the test guarding the data-loss scenario 0.16.0 is gated on.
    //
    // It is the same defect as the production one this file exists to check: A VALUE WHOSE SCOPE IS
    // WIDER THAN THE THING IT IS MEANT TO DESCRIBE. There, a machine-wide marker written from
    // one-directory evidence. Here, a process-wide marker root standing in for a per-test seed.
    //
    // Every other test file in this tree already derives it from its own root — 26 places across 20
    // files, including `deferral_test::mark_migrated` written by the same hand for this same hazard.
    // This file was the only outlier.
    let root = s.home.join(".base-gbl").join(".base");
    std::fs::create_dir_all(&root).expect("the seed's global tier must exist");
    assert!(
        matches!(base::protocol::migrate::state(&root), base::protocol::migrate::State::Pending),
        "control: a fixture that has never migrated must start Pending"
    );

    let cfg = base::config::BaseConfig::load(&s.home.join(".base-gbl"));
    base::protocol::migrate::mark_fresh_install(&root, Some(&s.home), &s.ws, &cfg)
        .expect("mark_fresh_install must not error on a legacy install");

    assert!(
        matches!(base::protocol::migrate::state(&root), base::protocol::migrate::State::Pending),
        "a legacy install was marked APPLIED having never migrated: the write block would lift and \
         {full} records would defer on the next session start"
    );
    assert!(pending(&s), "and doctor must still report it pending");
}

/// The twin, and it is what stops the test above passing for the wrong reason. A GENUINELY fresh
/// install — no records at all, so an empty plan — MUST be marked applied, or deferral stays off
/// forever for someone who has nothing to migrate.
#[test]
fn a_fresh_install_with_no_records_is_marked_applied() {
    let root_dir = std::env::temp_dir().join(format!("base-r09-fresh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root_dir);
    let s = seed::write(&root_dir, &EMPTY, DEFER_ON);
    assert_eq!(waiting(&s), 0, "control: this fixture must hold nothing that would defer");

    // THE MARKER ROOT COMES FROM THIS SEED, never from `migrate::marker_root`.
    //
    // `marker_root` resolves through `home_root()`, which under the `isolation-guard` feature
    // returns a path belonging to the TEST PROCESS rather than to the seed. Both tests in this pair
    // then shared ONE marker root: whichever ran first left a marker, and the second one's
    // `mark_fresh_install` returned early at its `if base_dir.join(MARKER).exists()` line and read
    // the other test's state as its own.
    //
    // Measured 2026-09-18: this pair read GREEN in a full-suite run and RED in a targeted run on the
    // SAME commit. A test whose result depends on execution order was never evidence in either
    // direction, and this is the test guarding the data-loss scenario 0.16.0 is gated on.
    //
    // It is the same defect as the production one this file exists to check: A VALUE WHOSE SCOPE IS
    // WIDER THAN THE THING IT IS MEANT TO DESCRIBE. There, a machine-wide marker written from
    // one-directory evidence. Here, a process-wide marker root standing in for a per-test seed.
    //
    // Every other test file in this tree already derives it from its own root — 26 places across 20
    // files, including `deferral_test::mark_migrated` written by the same hand for this same hazard.
    // This file was the only outlier.
    let root = s.home.join(".base-gbl").join(".base");
    std::fs::create_dir_all(&root).expect("the seed's global tier must exist");
    let cfg = base::config::BaseConfig::load(&s.home.join(".base-gbl"));
    base::protocol::migrate::mark_fresh_install(&root, Some(&s.home), &s.ws, &cfg)
        .expect("mark_fresh_install must succeed on a fresh install");

    assert!(
        matches!(base::protocol::migrate::state(&root), base::protocol::migrate::State::Applied(_)),
        "a fresh install with nothing to migrate was left Pending, so deferral would stay blocked \
         forever for someone who has nothing to migrate"
    );
}
