//! Acceptance for #87 step 3, the runtime identity backstop.
//!
//! Five legs, and each one exists because a different way of being wrong would
//! otherwise pass. The first four are the ones the design binds; the fifth is
//! the absent-at-load case, which section 2.4 states as refused and which no leg
//! covered.
//!
//! * **Reach (law 26).** A guard is only proven by a test that REACHES it. The
//!   assertion here is on the BYTES of the target file, never on a refusal
//!   message printing and never on an exit code — an error proves the code
//!   stopped somewhere, it does not prove where.
//! * **Void-FAIL control (law 24).** The same interleaving with the identity NOT
//!   recorded must be ACCEPTED. Without it, a refusal and a harness that cannot
//!   write at all share a result. The control arm is reached through
//!   `write_back_seamed`, which is `#[cfg(feature = "isolation-guard")]` and so
//!   exists in test builds and in nothing else: a safety guard with a runtime
//!   off switch is the next defect, and the seam for turning it off in a test
//!   already existed.
//! * **Masking twin i (law 25).** A fix needs a leg that separates "repaired"
//!   from "silenced". A guard that refused EVERY write would pass the reach leg
//!   identically, so a plain sequential load-then-write with no second writer
//!   must still succeed.
//! * **Masking twin ii.** Two writes from the SAME `LockedGraph` must both
//!   succeed, which is what makes the post-write identity refresh load-bearing.
//!   A guard that skipped the refresh would refuse the second write of a
//!   perfectly legitimate writer.
//!
//! Leg ii is an acceptance for a case NO current caller reaches: measured on this
//! tree, 0 of the 9 Tier C sites write twice within one `LockedGraph`. It stays
//! in on those terms — it fixes the semantics for the first writer that does,
//! and saying so here is the difference between a forward guarantee and a
//! hypothetical presented as a measurement.
//!
//! Every path is inside the test's own tempdir. Two `lock_and_load_graph` calls
//! on one path from one thread RE-ENTER rather than deadlock (`HELD_LOCKS` is
//! thread-local and keyed per path), which is what makes the interleaving
//! expressible in-process at all.

use std::fs;
use std::path::{Path, PathBuf};

use base::changelog::Change;
use base::store;
use oxigraph::model::{GraphName, Literal, NamedNode, Quad};

/// `<root>/ws/.base/graph.nq`, with the directory made — the shape of a real tier.
fn graph_path(root: &Path) -> PathBuf {
    let base = root.join("ws").join(".base");
    fs::create_dir_all(&base).unwrap();
    base.join("graph.nq")
}

/// One distinct quad per tag, so "whose bytes landed" is decidable by substring.
fn quad(tag: &str) -> Quad {
    Quad::new(
        NamedNode::new(format!("http://test.local/s/{tag}")).unwrap(),
        NamedNode::new("http://test.local/p").unwrap(),
        Literal::new_simple_literal(tag),
        GraphName::NamedNode(NamedNode::new("http://test.local/g").unwrap()),
    )
}

/// Write a starting graph the honest way, so every test begins from a real file
/// with a real identity rather than from an absent one.
fn seed(path: &Path) {
    let s = oxigraph::store::Store::new().unwrap();
    s.insert(&quad("seed")).unwrap();
    // Through the seam entry rather than `write_back`, which goes PRIVATE when the
    // nine Tier C sites migrate. `write_back_seamed` is the test-only entry and
    // stays reachable, so these legs do not have to be rewritten by that commit.
    store::write_back_seamed(&s, path, Change::Op("test.seed"), |f| f, None, None).unwrap();
    assert!(path.exists(), "the seed write must leave a file to have an identity of");
}

fn body(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

// ─── leg 1: reach ────────────────────────────────────────────

#[test]
fn identity_guard_refuses_a_stale_snapshot_overwrite() {
    let root = tempfile::tempdir().unwrap();
    let path = graph_path(root.path());
    seed(&path);

    // Two writers, each having loaded the SAME file, which is the shape that
    // loses updates: both hold a pre-write snapshot and both write the whole file.
    let first = store::lock_and_load_graph(&path).unwrap();
    let second = store::lock_and_load_graph(&path).unwrap();
    assert_eq!(
        first.identity(),
        second.identity(),
        "both writers must start from the same identity, or this is not the \
         interleaving under test"
    );

    first.store().insert(&quad("first")).unwrap();
    first.write(Change::Op("test.first")).unwrap();

    // The bytes after the correctly-ordered write. This, and not an error
    // message, is what the guard has to protect.
    let after_first = fs::read(&path).unwrap();
    assert!(
        body(&path).contains("/s/first"),
        "the first writer's change must be on disk before the second writer runs"
    );

    second.store().insert(&quad("second")).unwrap();
    let refused = second.write(Change::Op("test.second"));

    assert!(
        refused.is_err(),
        "the second writer overwrote a file that changed under its snapshot"
    );

    // THE acceptance: the file is byte-identical to what the first writer left.
    assert_eq!(
        fs::read(&path).unwrap(),
        after_first,
        "the refused write still changed the file — the guard fired late or not \
         at all, and the first writer's change is gone"
    );
    assert!(
        body(&path).contains("/s/first"),
        "the first writer's change was erased by a write that reported failure"
    );
    assert!(
        !body(&path).contains("/s/second"),
        "the second writer's bytes reached the file despite the refusal"
    );

    // Diagnostic quality, deliberately NOT the acceptance: a refusal nobody can
    // triage is a bad refusal, but a message is not evidence that the bytes
    // survived — the assertions above are.
    let msg = format!("{:#}", refused.unwrap_err());
    assert!(
        msg.contains("changed while this writer held the lock"),
        "the refusal must say what happened, got: {msg}"
    );
    assert!(
        msg.contains(store::LOCK_SEAM_MARKER),
        "the refusal must name the seam so a harness can attribute it, got: {msg}"
    );
}

// ─── leg 2: void-FAIL control ────────────────────────────────

#[test]
fn the_same_interleaving_with_no_identity_recorded_is_accepted() {
    let root = tempfile::tempdir().unwrap();
    let path = graph_path(root.path());
    seed(&path);

    let first = store::lock_and_load_graph(&path).unwrap();
    let second = store::lock_and_load_graph(&path).unwrap();

    first.store().insert(&quad("first")).unwrap();
    first.write(Change::Op("test.first")).unwrap();

    // Identical interleaving, identical stale snapshot, one variable changed:
    // `expect` is None, so no identity is recorded and there is nothing to
    // compare. This write MUST land. If it did not, leg 1's refusal would be
    // indistinguishable from a harness that cannot write in this state at all.
    second.store().insert(&quad("second")).unwrap();
    store::write_back_seamed(
        second.store(),
        &path,
        Change::Op("test.second-unchecked"),
        |file| file,
        None,
        None,
    )
    .expect("with no identity recorded there is nothing to refuse, so this must land");

    assert!(
        body(&path).contains("/s/second"),
        "the control arm did not write, so leg 1 proves nothing about the guard"
    );
    // And it demonstrates the damage the guard prevents, from the same fixture:
    // the stale snapshot's whole-file write erased the first writer's change,
    // and nothing failed on either side.
    assert!(
        !body(&path).contains("/s/first"),
        "the unchecked write was expected to erase the first writer's change — if \
         it did not, the fixture is not reproducing the lost update and leg 1 is \
         guarding nothing"
    );
}

// ─── leg 3: masking twin i ───────────────────────────────────

#[test]
fn a_lone_writer_with_no_second_writer_succeeds() {
    let root = tempfile::tempdir().unwrap();
    let path = graph_path(root.path());
    seed(&path);

    // The ordinary case. A guard that refused everything would pass leg 1
    // identically and fail here, which is the whole point of having this leg.
    let only = store::lock_and_load_graph(&path).unwrap();
    only.store().insert(&quad("only")).unwrap();
    only.write(Change::Op("test.only"))
        .expect("a writer with no competitor must not be refused");

    assert!(body(&path).contains("/s/only"), "the lone writer's change is on disk");
    assert!(body(&path).contains("/s/seed"), "and it did not lose the seed");
}

// ─── leg 4: masking twin ii ──────────────────────────────────

#[test]
fn two_writes_from_one_locked_graph_both_succeed() {
    let root = tempfile::tempdir().unwrap();
    let path = graph_path(root.path());
    seed(&path);

    // A guard that recorded the identity at load and never refreshed it would
    // accept the first write and refuse the second — refusing a writer that is
    // its own most recent writer. No current caller does this; the semantics are
    // fixed here for the first one that does.
    let g = store::lock_and_load_graph(&path).unwrap();

    g.store().insert(&quad("one")).unwrap();
    g.write(Change::Op("test.one")).expect("first write");
    let after_one = g.identity();

    g.store().insert(&quad("two")).unwrap();
    g.write(Change::Op("test.two"))
        .expect("the second write from the same LockedGraph must be legal — the \
                 post-write identity refresh is what makes it so");

    assert_ne!(
        after_one,
        g.identity(),
        "the identity did not move across the second write, so the refresh is not \
         happening and this leg passed for the wrong reason"
    );
    let text = body(&path);
    assert!(text.contains("/s/one"), "the first write survived the second");
    assert!(text.contains("/s/two"), "the second write landed");
    assert!(text.contains("/s/seed"), "and neither lost the seed");
}

// ─── the absent-at-load case, which is its own refusal ───────

#[test]
fn a_graph_created_after_an_absent_load_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let path = graph_path(root.path());
    // Deliberately NOT seeded: the writer loads a tier that has no graph file,
    // which is a real state and not an error.
    let writer = store::lock_and_load_graph(&path).unwrap();
    assert_eq!(
        writer.identity(),
        store::FileIdentity::Absent,
        "a tier with no graph file must record Absent, not a zero-length Present"
    );

    // Someone else creates it while we hold our absent snapshot.
    let other = oxigraph::store::Store::new().unwrap();
    other.insert(&quad("other")).unwrap();
    store::write_back_seamed(&other, &path, Change::Op("test.other"), |f| f, None, None).unwrap();
    let after_other = fs::read(&path).unwrap();

    writer.store().insert(&quad("ours")).unwrap();
    let refused = writer.write(Change::Op("test.ours"));

    assert!(refused.is_err(), "absent at load and present at write must be refused");
    assert_eq!(
        fs::read(&path).unwrap(),
        after_other,
        "the refused write changed a graph it never loaded"
    );
    assert!(
        !body(&path).contains("/s/ours"),
        "our bytes reached a file we had no snapshot of"
    );
    let msg = format!("{:#}", refused.unwrap_err());
    assert!(msg.contains("absent"), "the refusal must say it started from absent, got: {msg}");
}
