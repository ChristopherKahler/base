//! Which map may answer a read, as a rule — with no filesystem and no real home.
//!
//! `base ast query` answered from a different project's map. From any unmapped
//! directory under the user profile the read walked up into `~/.base-ast` and
//! returned that map's entities: exit 0, no warning, nothing naming the source.
//! Measured 2026-09-08 inside base's own clone — `--contains parakeet` returned
//! 13 rows of an unrelated tool, and `--contains find_ast_ttl` reported no match
//! for a function defined in that very tree.
//!
//! The WRITE path already refused this (`resolve_ast_ttl`'s home filter, added
//! 2026-08-14 after mapping app B erased app A on Windows and Linux). The READ
//! path never got the same rule. That asymmetry is the whole defect.
//!
//! Why these legs pass `home` as a VALUE rather than letting the code call
//! `home::home_root()`: that function resolves differently in a test binary than
//! in the shipped one, in FOUR `cfg(feature = "isolation-guard")` places —
//! `home.rs:26-29` (a per-thread root returned BEFORE `BASE_HOME`),
//! `home.rs:41-44` (a synthetic `test_root()` instead of the OS home),
//! `home.rs:62` (`isolation_active`), and `config.rs:76-77` (`walk_up`'s sandbox
//! ceiling). A leg that consulted it would measure the feature, not the rule —
//! and the `thread_home` branch can be left set by an earlier leg in the same
//! binary, which reads as a clean pass and does not reproduce when run alone.
//! A pure function has no feature-gated input, so there is nothing to leak.

use std::path::Path;

use base::config::ast_map_admissible;

/// U1 — the defect. A read standing below home must not take home's map.
#[test]
fn home_map_is_not_adopted_from_below() {
    let home = Path::new("C:/Users/someone");
    let start = Path::new("C:/Users/someone/.cache/a-clone");

    assert!(
        !ast_map_admissible(start, home, Some(home)),
        "a read from {start:?} must not adopt home's map — this is rank 01"
    );
}

/// U2 — the masking twin (law 24). A rule that refused home ALWAYS would pass
/// U1 identically and would break the intentional workspace-wide map.
#[test]
fn home_still_answers_when_home_is_where_you_are_standing() {
    let home = Path::new("C:/Users/someone");

    assert!(
        ast_map_admissible(home, home, Some(home)),
        "standing IN home, home's own map is exactly the right answer"
    );
}

/// U3 — a directory's own map is always admissible.
#[test]
fn a_directory_may_always_take_its_own_map() {
    let home = Path::new("C:/Users/someone");
    let start = Path::new("C:/Users/someone/dev/app");

    assert!(ast_map_admissible(start, start, Some(home)));
}

/// U4 — no home known. The rule must not start refusing things off-Windows or
/// wherever the home lookup returns nothing; that would be a silent behaviour
/// change dressed as a fix.
#[test]
fn with_no_home_known_nothing_is_refused() {
    let start = Path::new("/srv/app");

    assert!(ast_map_admissible(start, Path::new("/srv"), None));
    assert!(ast_map_admissible(start, start, None));
}

/// U5 — this rule is ONLY about home. Refusing a non-home ancestor is the app
/// boundary's job, in `find_ast_ttl`; keeping the two separate is what stops
/// one of them quietly doing the other's work and both looking correct.
#[test]
fn a_non_home_ancestor_is_not_this_rules_business() {
    let home = Path::new("C:/Users/someone");
    let start = Path::new("C:/work/suite/app");

    assert!(
        ast_map_admissible(start, Path::new("C:/work/suite"), Some(home)),
        "the app boundary refuses this one, not the home rule"
    );
}
