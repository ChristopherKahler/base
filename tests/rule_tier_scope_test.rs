//! `rule remove` must remove what `rule list` showed, or say it did not.
//!
//! #112, reported by sstickel on 0.14.2/Linux. A global `graph.nq` holding rule
//! quads in a named graph other than the tier's own — 460 of them on the
//! reporter's install, origin unknown and unreproducible on 0.14.2 — makes the
//! two halves of `crud::rule` disagree:
//!
//! - `fetch` (`crud/rule.rs:135`) reads `GRAPH ?g`, a wildcard over every named
//!   graph in the tier's file, so `rule -g list` SHOWS the foreign rule.
//! - `remove` (`:265`, `:271`) scoped its DELETE to `GRAPH <graph/ws/{slug}>`,
//!   the tier's own graph, so the statement matched nothing.
//! - `remove` then returned a hardcoded `Ok(1)` (`:280`) without asking the store
//!   what had actually gone, and `cli.rs:2859` printed `Rule 7 removed` and
//!   exited 0.
//!
//! A scripted cleanup loop could report twelve successful deletions and change
//! nothing. That is the defect: not the wildcard read — `GRAPH ?g` is base's
//! normal idiom, 197 occurrences across 37 files, and one tier is one FILE so a
//! wildcard never crosses tiers — but a mutation whose read and write ask
//! different questions and whose return value asks none.
//!
//! Two of these five legs are CONTROLS and pass before the fix. That is
//! deliberate: `the_rule_in_this_tiers_own_graph_still_goes` separates a repair
//! from a silencing (a `remove` that always refused, or that purged the graph,
//! passes the defect leg identically), and
//! `an_index_in_no_graph_at_all_reports_zero` proves #55's `Ok(0)` path — the
//! branch `cli.rs:2851-2858` exits 1 on — is still reached afterwards.

use std::path::{Path, PathBuf};

use base::config::NamespaceConfig;
use base::crud;

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

/// A workspace with an empty graph, which is what `base scaffold` leaves behind
/// and what every read in `crud` expects to open.
fn workspace() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    std::fs::write(tmp.path().join(".base").join("graph.nq"), "").unwrap();
    tmp
}

fn graph_path(root: &Path) -> PathBuf {
    root.join(".base").join("graph.nq")
}

/// Every non-blank line of the tier's graph file.
///
/// Returned rather than counted so each leg can print the set it actually
/// visited: an assertion that looped over nothing proved nothing, and must fail
/// rather than pass.
fn quad_lines(root: &Path) -> Vec<String> {
    std::fs::read_to_string(graph_path(root))
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// Every line mentioning one rule's IRI, in any named graph.
///
/// Matched on the IRI rather than on the bytes I appended, because a write
/// re-serializes the whole store and the reporter's hand-written spacing does
/// not survive it. The IRI does.
fn lines_for_rule(root: &Path, ns: &NamespaceConfig, domain: &str, tail: &str) -> Vec<String> {
    let iri = format!("<{}rule/{}/{}>", ns.uri, domain, tail);
    quad_lines(root).into_iter().filter(|l| l.contains(&iri)).collect()
}

/// The reporter's stray: five quads for one rule, in a named graph this tier does
/// not own, appended to the tier's own file exactly as #112's reproduction does.
/// Returns how many quads it wrote.
///
/// Built from `ns.uri`, never from a pasted literal. A hardcoded IRI that drifted
/// from the configured namespace would make the stray inert, the wildcard reader
/// would never see it, and every leg below would pass vacuously against a fixture
/// that was never there.
///
/// `tail` is separate from `index` on purpose: two rules at the same index must be
/// two distinct subjects, or the store folds them into one and a count cannot tell
/// a read-back from a constant.
fn append_foreign_rule(
    root: &Path,
    ns: &NamespaceConfig,
    domain: &str,
    tail: &str,
    index: u32,
    text: &str,
) -> usize {
    let u = &ns.uri;
    let g = format!("{u}graph/ws/otherws");
    let d = format!("{u}domain/{domain}");
    let r = format!("{u}rule/{domain}/{tail}");
    let quads = [
        format!("<{d}> <{u}hasRule> <{r}> <{g}> ."),
        format!("<{r}> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <{u}Rule> <{g}> ."),
        format!("<{r}> <{u}index> \"{index}\" <{g}> ."),
        format!("<{r}> <{u}priority> \"{index}\" <{g}> ."),
        format!("<{r}> <{u}ruleText> \"{text}\" <{g}> ."),
    ];

    let mut body = std::fs::read_to_string(graph_path(root)).unwrap();
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    for q in &quads {
        body.push_str(q);
        body.push('\n');
    }
    std::fs::write(graph_path(root), body).unwrap();
    quads.len()
}

/// The reporter's starting state: one real rule in the tier's own graph, and one
/// stray at index 7 in a graph the tier does not own.
fn tier_with_a_stray(root: &Path) -> usize {
    crud::rule::add(root, &ns(), "demoapp", "real global rule", None).unwrap();
    append_foreign_rule(
        root,
        &ns(),
        "demoapp",
        "cli-7",
        7,
        "stray rule from another workspace graph",
    )
}

/// FIXTURE CONTROL. Separates "the assertion failed" from "the assertion could
/// not run": if the stray never reaches the wildcard reader, every leg below is
/// measuring an empty graph and would pass no matter what `remove` did.
#[test]
fn the_stray_is_really_there_and_the_listing_really_shows_it() {
    let tmp = workspace();
    let written = tier_with_a_stray(tmp.path());
    assert_eq!(written, 5, "the reporter's stray is five quads");

    let on_disk = lines_for_rule(tmp.path(), &ns(), "demoapp", "cli-7");
    assert_eq!(
        on_disk.len(),
        5,
        "the stray must be on disk before any leg runs; found {}: {on_disk:?}",
        on_disk.len()
    );

    let seen = crud::rule::fetch(tmp.path(), &ns(), "demoapp", true).unwrap();
    assert_eq!(
        seen.len(),
        2,
        "the wildcard reader must see BOTH rules — at 1 the fixture is inert and \
         every leg in this file would pass vacuously: {seen:?}"
    );
    assert!(
        seen.iter().any(|(n, _, _)| *n == 7),
        "index 7 is what the operator reads and what they will type: {seen:?}"
    );
}

/// THE DEFECT. Red before the fix: `remove` reports success and the five quads it
/// claimed to delete are still on disk.
#[test]
fn removing_a_rule_the_listing_showed_actually_removes_it() {
    let tmp = workspace();
    tier_with_a_stray(tmp.path());

    let removed = crud::rule::remove(tmp.path(), &ns(), "demoapp", 7).unwrap();

    let left_on_disk = lines_for_rule(tmp.path(), &ns(), "demoapp", "cli-7");
    assert!(
        left_on_disk.is_empty(),
        "`remove` returned {removed} and left {} of the rule's quads on disk: {left_on_disk:?}",
        left_on_disk.len()
    );
    assert_eq!(removed, 1, "one rule was addressed and one rule went");

    let after = crud::rule::fetch(tmp.path(), &ns(), "demoapp", true).unwrap();
    assert_eq!(after.len(), 1, "the listing must agree with the delete: {after:?}");
    assert_eq!(
        after[0].1, "real global rule",
        "the surviving rule is this tier's own: {after:?}"
    );
}

/// THE COUNT IS READ BACK, NOT ASSUMED. Two distinct rules sit at index 7 — one in
/// this tier's own graph, one in a graph it does not own — so the honest answer is
/// 2. A hardcoded `Ok(1)`, or any fix that returns what it hoped for instead of
/// what left the store, reports 1 here.
#[test]
fn the_count_comes_back_from_the_store() {
    let tmp = workspace();
    // Eight rules, so index 7 is legitimately this tier's own.
    for i in 1..=8 {
        crud::rule::add(tmp.path(), &ns(), "demoapp", &format!("rule number {i}"), None).unwrap();
    }
    append_foreign_rule(
        tmp.path(),
        &ns(),
        "demoapp",
        "stray-7",
        7,
        "a stray sharing index 7",
    );

    let both = crud::rule::fetch(tmp.path(), &ns(), "demoapp", true).unwrap();
    let at_seven = both.iter().filter(|r| r.0 == 7).count();
    assert_eq!(at_seven, 2, "precondition: two distinct rules share index 7: {both:?}");

    let removed = crud::rule::remove(tmp.path(), &ns(), "demoapp", 7).unwrap();
    assert_eq!(
        removed, 2,
        "two rules were at index 7 and both had to go; a constant return says 1 here"
    );

    for tail in ["cli-7", "stray-7"] {
        let left = lines_for_rule(tmp.path(), &ns(), "demoapp", tail);
        assert!(left.is_empty(), "{tail} survived a reported removal: {left:?}");
    }
    let after = crud::rule::fetch(tmp.path(), &ns(), "demoapp", true).unwrap();
    assert_eq!(after.len(), 7, "the other seven rules were not addressed: {after:?}");
}

/// CONTROL, passes before the fix. A `remove` "fixed" by always refusing, or by
/// purging every rule it could reach, passes the defect leg above identically.
/// This is the leg that separates a repair from a silencing: the addressed rule
/// goes, and nothing else does.
#[test]
fn the_rule_in_this_tiers_own_graph_still_goes_and_nothing_else_does() {
    let tmp = workspace();
    tier_with_a_stray(tmp.path());
    crud::rule::add(tmp.path(), &ns(), "otherdomain", "a rule on another domain", None).unwrap();

    let removed = crud::rule::remove(tmp.path(), &ns(), "demoapp", 0).unwrap();
    assert_eq!(removed, 1, "the tier's own rule 0 was addressed and went");

    let gone = lines_for_rule(tmp.path(), &ns(), "demoapp", "cli-0");
    assert!(gone.is_empty(), "rule 0 was reported removed and is still here: {gone:?}");

    let stray = lines_for_rule(tmp.path(), &ns(), "demoapp", "cli-7");
    assert_eq!(
        stray.len(),
        5,
        "index 7 was never addressed and must survive — a purge takes it too: {stray:?}"
    );

    let other = crud::rule::fetch(tmp.path(), &ns(), "otherdomain", true).unwrap();
    assert_eq!(other.len(), 1, "another domain's rules were collateral: {other:?}");
}

/// CONTROL, passes before the fix. #55's zero path must still be REACHED after the
/// change — it is the branch `cli.rs:2851-2858` prints the tier-scoped error and
/// calls `std::process::exit(1)` on. A fix that returned a non-zero count
/// unconditionally passes every leg above and fails this one.
#[test]
fn an_index_in_no_graph_at_all_reports_zero_and_writes_nothing() {
    let tmp = workspace();
    tier_with_a_stray(tmp.path());

    let before = quad_lines(tmp.path());
    assert!(
        !before.is_empty(),
        "this leg visited zero quads, so it proved nothing about writing none"
    );

    let removed = crud::rule::remove(tmp.path(), &ns(), "demoapp", 99).unwrap();
    assert_eq!(removed, 0, "index 99 is in no graph in this file, and 0 is not success");

    let after = quad_lines(tmp.path());
    assert_eq!(
        after.len(),
        before.len(),
        "a refused removal wrote to the graph: {} lines before, {} after",
        before.len(),
        after.len()
    );
}
