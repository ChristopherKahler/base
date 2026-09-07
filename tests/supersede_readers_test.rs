//! Serving surfaces return the live version; `--include-superseded` shows the chain.
//!
//! One fixture, one test per surface. Each test asserts BOTH halves — the superseded
//! text is gone AND the successor is present — because a filter that excludes
//! everything passes a "the old one is gone" assertion just as well as a correct one.

use std::path::Path;

use base::config::{BaseConfig, NamespaceConfig};
use base::crud;

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

/// A workspace holding two notes in domain `probe`, the first superseded by the second.
fn corrected_workspace(root: &Path) -> String {
    std::fs::create_dir_all(root.join(".base")).unwrap();
    std::fs::write(root.join(".base").join("graph.nq"), "").unwrap();

    let old = crud::note::learn(root, &ns(), "the old fact", "insight", Some("probe"), None, None)
        .unwrap();
    crud::note::learn_with(
        root,
        &ns(),
        "the corrected fact",
        "correction",
        Some("probe"),
        None,
        None,
        Some(&old),
    )
    .unwrap();
    old
}

#[test]
fn recall_by_domain_serves_the_correction_and_not_what_it_corrected() {
    let tmp = tempfile::tempdir().unwrap();
    corrected_workspace(tmp.path());

    let out = crud::note::recall_to_string(tmp.path(), &ns(), None, Some("probe"));
    assert!(out.contains("the corrected fact"), "the live version must still be served: {out}");
    assert!(
        !out.contains("the old fact"),
        "a superseded note must not come back beside the note that corrected it: {out}"
    );
}

#[test]
fn recall_by_keyword_drops_it_too() {
    // The keyword arm is a different UNION branch from the domain arm; F16 was exactly
    // a filter that covered some arms and not others.
    let tmp = tempfile::tempdir().unwrap();
    corrected_workspace(tmp.path());

    let out = crud::note::recall_to_string(tmp.path(), &ns(), Some("fact"), None);
    assert!(out.contains("the corrected fact"), "{out}");
    assert!(!out.contains("the old fact"), "{out}");
}

#[test]
fn recall_by_keyword_and_domain_drops_it_too() {
    let tmp = tempfile::tempdir().unwrap();
    corrected_workspace(tmp.path());

    let out = crud::note::recall_to_string(tmp.path(), &ns(), Some("fact"), Some("probe"));
    assert!(out.contains("the corrected fact"), "{out}");
    assert!(!out.contains("the old fact"), "{out}");
}

#[test]
fn include_superseded_shows_both() {
    let tmp = tempfile::tempdir().unwrap();
    corrected_workspace(tmp.path());

    let out = crud::note::recall_to_string_with(tmp.path(), &ns(), None, Some("probe"), true);
    assert!(out.contains("the corrected fact"), "{out}");
    assert!(
        out.contains("the old fact"),
        "--include-superseded is the whole reason the record is kept rather than deleted: {out}"
    );
}

#[test]
fn a_superseded_note_is_not_stamped_as_read_by_a_recall_that_never_showed_it() {
    // `recalled_note_iris` drives the `lastRead` stamp that `graph purge --stale`
    // reads. If it disagreed with what recall printed, a superseded note would be
    // kept alive forever by recalls that never surfaced it.
    let tmp = tempfile::tempdir().unwrap();
    let old = corrected_workspace(tmp.path());

    let iris = crud::note::recalled_note_iris(tmp.path(), &ns(), None, Some("probe"));
    assert!(
        iris.iter().any(|i| i.contains("the-corrected-fact")),
        "the live note is what the recall surfaced: {iris:?}"
    );
    assert!(
        !iris.iter().any(|i| i.ends_with(&format!("/{old}"))),
        "the superseded note was never printed, so it must not be stamped read: {iris:?}"
    );
}

#[test]
fn the_prompt_injection_carries_only_the_live_decision() {
    // `query_domain_from_graph`'s neighbourhood binds ?related to DECISIONS and
    // PROJECTS, never notes — so the fixture supersedes a decision. A note fixture
    // here would pass without the filter existing at all, which is not a test.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".base")).unwrap();
    std::fs::write(root.join(".base").join("graph.nq"), "").unwrap();
    std::fs::write(
        root.join(".base").join("domains.toml"),
        "[[domain]]\nname = \"probe\"\nmode = \"always\"\n",
    )
    .unwrap();

    let old = crud::decision::log(root, &ns(), "probe", "Use the old thing", "it was fine", None)
        .unwrap();
    crud::decision::log_with(
        root,
        &ns(),
        "probe",
        "Use the new thing",
        "the old one was wrong",
        None,
        Some(&old),
    )
    .unwrap();

    let config = BaseConfig { namespace: ns(), ..Default::default() };
    let store = base::store::load_graph(&root.join(".base").join("graph.nq")).unwrap();
    let def = base::domain::load_domains(root)
        .into_iter()
        .find(|d| d.name == "probe")
        .expect("the probe domain is declared");
    let (rules, neighborhood, _extra) =
        base::domain::query::query_domain_from_graph(&store, &config, &def);
    let block = format!("{rules}\n{neighborhood}");

    assert!(
        block.contains("Use the new thing"),
        "the live decision must still be injected: {block}"
    );
    assert!(
        !block.contains("Use the old thing"),
        "injecting a decision a later one superseded is the drift this fork exists to \
         end: {block}"
    );
}