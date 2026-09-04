//! Rule numbers are numbers, not text.
//!
//! `next_rule_index` took `MAX(?idx)` over `:index` literals stored as strings,
//! so once `"0"`..`"9"` existed the maximum stayed `"9"` and every later rule was
//! handed the index 10: they landed on one IRI, `rule/<domain>/cli-10`, and the
//! hook injection rendered the cross-product of their texts. Reported as #29 by
//! Marc Swindle, fixed in #30, measured on the released 0.13.16 binary — twelve
//! rules produced eleven IRIs and six quads on cli-10.
//!
//! The same string comparison sat one query further on, in the listing's
//! `ORDER BY`, and outlived that fix: a domain past its tenth rule printed
//! 0, 1, 10, 11, 2. Both are pinned here, because both look right until a
//! domain crosses ten rules.

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

/// `count` rules on one domain, in order, returning the index each was given.
fn add_rules(root: &std::path::Path, count: u32) -> Vec<u32> {
    (1..=count)
        .map(|i| crud::rule::add(root, &ns(), "probe", &format!("rule number {i}"), None).unwrap())
        .collect()
}

/// Twelve rules, twelve numbers. Fails on the old code at the eleventh.
#[test]
fn every_rule_past_the_tenth_gets_its_own_number() {
    let tmp = workspace();
    let indices = add_rules(tmp.path(), 12);

    assert_eq!(
        indices,
        (0..12).collect::<Vec<u32>>(),
        "each rule takes the next number; the old MAX over strings stuck at 10"
    );

    let graph = std::fs::read_to_string(tmp.path().join(".base").join("graph.nq")).unwrap();
    let mut iris: Vec<&str> = graph
        .match_indices("rule/probe/cli-")
        .map(|(at, _)| {
            let rest = &graph[at..];
            &rest[..rest.find(['>', ' ']).unwrap_or(rest.len())]
        })
        .collect();
    iris.sort_unstable();
    iris.dedup();
    assert_eq!(iris.len(), 12, "twelve rules, twelve IRIs — no two rules share one");
}

/// The listing counts up, not alphabetically.
#[test]
fn the_listing_orders_by_the_number_not_its_text() {
    let tmp = workspace();
    add_rules(tmp.path(), 12);

    let rules = crud::rule::fetch(tmp.path(), &ns(), "probe").unwrap();
    let numbers: Vec<u32> = rules.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        numbers,
        (0..12).collect::<Vec<u32>>(),
        "10 comes after 9, not after 1 — `ORDER BY ?pri` sorted the digits as text"
    );
    assert_eq!(rules[10].1, "rule number 11", "and the text travels with its number");
}

/// A rule added past the tenth is still removable by its number.
#[test]
fn a_rule_past_the_tenth_can_still_be_removed() {
    let tmp = workspace();
    add_rules(tmp.path(), 12);
    crud::rule::remove(tmp.path(), &ns(), "probe", 10).unwrap();

    let left = crud::rule::fetch(tmp.path(), &ns(), "probe").unwrap();
    assert_eq!(left.len(), 11, "one rule gone");
    assert!(
        !left.iter().any(|(_, t)| t == "rule number 11"),
        "the eleventh rule is the one that went"
    );
}
