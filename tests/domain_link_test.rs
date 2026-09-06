//! `base recall --domain` must return the same notes on both sides of the
//! predicate change, and each note exactly once while both predicates are on disk.
//!
//! `crud/note.rs` had five hardcoded `relatedTo <domain_iri>` reads (hawk C11).
//! Flip the write to `hasDomain` without teaching them both and `recall --domain`
//! returns nothing — silently, because an empty SPARQL result is indistinguishable
//! from "that domain has no notes". These tests plant each era's shape directly in
//! the store so a regression on any one of the five reads fails here.

use std::path::Path;

use base::config::{BaseConfig, NamespaceConfig};
use base::crud;
use base::domain::link;

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

fn workspace() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let base_dir = tmp.path().join(".base");
    std::fs::create_dir_all(&base_dir).unwrap();
    std::fs::write(base_dir.join("graph.nq"), "").unwrap();
    std::fs::write(
        base_dir.join("base.toml"),
        "[namespace]\nprefix = \"ops\"\nuri = \"http://ops-sys.local/ontology#\"\n",
    )
    .unwrap();
    std::fs::write(
        base_dir.join("domains.toml"),
        "[[domain]]\nname = \"probe\"\nmode = \"triggered\"\nkeywords = [\"probe\"]\nrules = [\"a probe rule\"]\n",
    )
    .unwrap();
    let config = BaseConfig::load(tmp.path());
    base::domain::sync::sync_domains_to_graph(&config, tmp.path(), None).unwrap();
    tmp
}

/// A note carrying exactly the predicates given — the era shapes, planted directly.
fn plant_note(cwd: &Path, slug: &str, text: &str, predicates: &[&str]) {
    let ns = ns();
    let p = &ns.prefix;
    let iri = crud::build_iri(&ns, "note", slug);
    let dom = crud::build_iri(&ns, "domain", "probe");
    let graph = crud::workspace_graph_iri(&ns, &crud::workspace_slug(cwd));
    let edges: String = predicates
        .iter()
        .map(|pred| format!("  <{iri}> {p}:{pred} <{dom}> .\n"))
        .collect();
    let sparql = format!(
        "INSERT DATA {{ GRAPH <{graph}> {{\n\
           <{iri}> rdf:type {p}:Note ;\n\
             {p}:noteText \"{text}\" ;\n\
             {p}:noteType \"insight\" ;\n\
             {p}:status \"active\" ;\n\
             {p}:createdAt \"2026-09-06T00:00:00-05:00\"^^xsd:dateTime .\n\
         {edges}\
         }} }}"
    );
    crud::load_and_mutate(cwd, &ns, &sparql).unwrap();
}

fn hits(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

#[test]
fn recall_by_domain_finds_a_legacy_only_note() {
    let tmp = workspace();
    plant_note(tmp.path(), "legacy-era", "written by 0.13.x", &[link::LEGACY]);

    let out = crud::note::recall_to_string(tmp.path(), &ns(), None, Some("probe"));
    assert_eq!(
        hits(&out, "written by 0.13.x"),
        1,
        "a note written before the migration must still recall — got {out:?}"
    );
}

#[test]
fn recall_by_domain_finds_a_canonical_only_note() {
    let tmp = workspace();
    plant_note(tmp.path(), "canonical-era", "written after the contract", &[link::CANONICAL]);

    let out = crud::note::recall_to_string(tmp.path(), &ns(), None, Some("probe"));
    assert_eq!(
        hits(&out, "written after the contract"),
        1,
        "a note carrying only the new predicate must recall — got {out:?}"
    );
}

#[test]
fn a_note_carrying_both_predicates_recalls_exactly_once() {
    // The overlap window's own shape: 0.14.0 writes both, so a naive
    // `?n (a|b) <dom>` triple pattern yields one solution per matching path and
    // prints every note twice. The reads use FILTER EXISTS for that reason.
    let tmp = workspace();
    plant_note(tmp.path(), "overlap-era", "written during the overlap", &[link::CANONICAL, link::LEGACY]);

    let out = crud::note::recall_to_string(tmp.path(), &ns(), None, Some("probe"));
    assert_eq!(
        hits(&out, "written during the overlap"),
        1,
        "both predicates on one note is ONE note — got {out:?}"
    );

    let both = crud::note::recall_to_string(tmp.path(), &ns(), Some("overlap"), Some("probe"));
    assert_eq!(
        hits(&both, "written during the overlap"),
        1,
        "keyword + domain must not double-count either — got {both:?}"
    );

    let iris = crud::note::recalled_note_iris(tmp.path(), &ns(), None, Some("probe"));
    assert_eq!(iris.len(), 1, "the lastRead stamp must see one note, not two — got {iris:?}");
}

#[test]
fn learn_writes_both_predicates_and_recalls_once() {
    // The real write path, end to end: `base learn --domain` during the overlap.
    let tmp = workspace();
    let cwd = tmp.path();
    crud::note::learn(cwd, &ns(), "a note through the write path", "insight", Some("probe"), None, None)
        .unwrap();

    let ns = ns();
    let p = &ns.prefix;
    let dom = crud::build_iri(&ns, "domain", "probe");
    for pred in [link::CANONICAL, link::LEGACY] {
        let q = format!("ASK {{ GRAPH ?g {{ ?n a {p}:Note ; {p}:{pred} <{dom}> }} }}");
        let r = crud::load_and_query(cwd, &ns, &q).unwrap();
        let oxigraph::sparql::QueryResults::Boolean(yes) = r else { panic!("expected ASK") };
        assert!(yes, "0.14.0 must write {pred} — a rolled-back 0.13.x binary reads the legacy one");
    }

    let out = crud::note::recall_to_string(cwd, &ns, None, Some("probe"));
    assert_eq!(hits(&out, "a note through the write path"), 1, "one note, one row — got {out:?}");
}

#[test]
fn list_notes_by_domain_sees_both_eras() {
    let tmp = workspace();
    let cwd = tmp.path();
    plant_note(cwd, "legacy-era", "legacy listed", &[link::LEGACY]);
    plant_note(cwd, "canonical-era", "canonical listed", &[link::CANONICAL]);
    plant_note(cwd, "overlap-era", "overlap listed", &[link::CANONICAL, link::LEGACY]);
    // A note in no domain at all — the control the filter must exclude.
    plant_note(cwd, "unlinked", "unlinked note", &[]);

    // `list_notes` prints; the filter it builds is the fifth read. Exercise it
    // through the IRI resolver, which builds the same domain clause.
    let iris = crud::note::recalled_note_iris(cwd, &ns(), None, Some("probe"));
    let mut slugs: Vec<String> =
        iris.iter().filter_map(|i| i.rsplit_once("note/").map(|(_, s)| s.to_string())).collect();
    slugs.sort();
    assert_eq!(
        slugs,
        vec!["canonical-era".to_string(), "legacy-era".to_string(), "overlap-era".to_string()],
        "both eras in, once each, and the unlinked note out"
    );
}

#[test]
fn a_notes_project_and_entity_links_are_not_domain_links() {
    // `relatedTo` is generic: 532 document→entity and 30 document→project links in
    // Chris's store use it (hawk C12). The object IRI kind is the discriminator, so
    // reading the domain link must never pull a project link in.
    let tmp = workspace();
    let cwd = tmp.path();
    crud::note::learn(cwd, &ns(), "a note about a project", "insight", None, Some("probe"), None)
        .unwrap();

    // `project/probe` and `domain/probe` differ only in IRI kind.
    let out = crud::note::recall_to_string(cwd, &ns(), None, Some("probe"));
    assert!(
        !out.contains("a note about a project"),
        "a relatedTo→project link is not a domain link — got {out:?}"
    );
}
