//! Tests for the `*fork` star command plumbing (crud::handoff fork path) and the
//! unified doc==slug protocol shared by handoffs and forks.
//!
//! Forks reuse the Handoff node type, split by a `kind` field. They flip two
//! behaviors vs handoffs: ADDITIVE (multiple open, no archive-prior) and were the
//! first to adopt the doc==slug naming protocol. Both `create` and `create_fork`
//! now default the slug to the doc basename and accept an explicit `--slug`
//! override, so the doc filename and the summon name always align.
//!
//! Each test creates a `.base/` dir inside its tempdir so `find_workspace_base`
//! resolves the write to the tempdir tier — never the real global graph.

use std::path::Path;

use oxigraph::sparql::QueryResults;

use base::config::NamespaceConfig;
use base::crud;

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

/// Create a workspace tempdir with a `.base/` so writes land in it (hermetic).
fn workspace() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    tmp
}

fn store_of(tmp: &Path) -> oxigraph::store::Store {
    let trig = tmp.join(".base").join("graph.nq");
    base::store::load_graph(&trig).unwrap()
}

fn ask(store: &oxigraph::store::Store, body: &str) -> bool {
    let sparql = format!(
        "PREFIX {p}: <{u}>\nASK {{ {body} }}",
        p = ns().prefix,
        u = ns().uri,
    );
    match store.query(&sparql).unwrap() {
        QueryResults::Boolean(yes) => yes,
        _ => panic!("expected boolean"),
    }
}

/// Count solutions for a SELECT ?h query body (across any graph).
fn count(store: &oxigraph::store::Store, where_body: &str) -> usize {
    let sparql = format!(
        "PREFIX {p}: <{u}>\nSELECT ?h WHERE {{ GRAPH ?g {{ {where_body} }} }}",
        p = ns().prefix,
        u = ns().uri,
    );
    match store.query(&sparql).unwrap() {
        QueryResults::Solutions(s) => s.filter_map(|r| r.ok()).count(),
        _ => panic!("expected solutions"),
    }
}

/// THE PROTOCOL: a fork's graph slug == its doc basename (no extension), verbatim.
/// Mixed case is preserved — the doc filename and the title you summon it by are
/// the SAME string, so a fork is always reliably callable by name.
#[test]
fn fork_slug_equals_doc_basename() {
    let tmp = workspace();
    let doc = "/abs/path/FORK-COMMAND-SPEC.md";
    let slug = crud::handoff::create_fork(tmp.path(), &ns(), "base-v2", doc, None).unwrap();

    assert_eq!(slug, "FORK-COMMAND-SPEC", "slug must equal doc basename verbatim");

    let store = store_of(tmp.path());
    let u = ns().uri;
    assert!(
        ask(
            &store,
            &format!(
                "GRAPH ?g {{ <{u}handoff/FORK-COMMAND-SPEC> a {p}:Handoff ; \
                   {p}:kind \"fork\" ; {p}:status \"open\" ; \
                   {p}:handoffDoc \"{doc}\" }}",
                p = ns().prefix
            )
        ),
        "fork node IRI must be handoff/<doc-basename> with kind=fork, open, pointing at the doc"
    );
}

/// A handoff's slug ALSO defaults to the doc basename now (unified protocol),
/// instead of the old timestamp scheme. kind="handoff" is set explicitly.
#[test]
fn handoff_slug_defaults_to_doc_basename() {
    let tmp = workspace();
    let doc = "/abs/path/2026-06-25-2230-base-v2.md";
    let slug = crud::handoff::create(tmp.path(), &ns(), "base-v2", doc, None).unwrap();
    assert_eq!(slug, "2026-06-25-2230-base-v2");

    let store = store_of(tmp.path());
    let u = ns().uri;
    assert!(
        ask(
            &store,
            &format!(
                "GRAPH ?g {{ <{u}handoff/2026-06-25-2230-base-v2> a {p}:Handoff ; \
                   {p}:kind \"handoff\" ; {p}:status \"open\" }}",
                p = ns().prefix
            )
        ),
        "handoff node IRI must be handoff/<doc-basename> with kind=handoff, open"
    );
}

/// An explicit --slug override wins over the doc basename, for BOTH surfaces —
/// letting the operator pick one consistent summon name regardless of doc path.
#[test]
fn explicit_slug_override_wins_for_both() {
    let tmp = workspace();
    let u = ns().uri;
    let p = ns().prefix;

    let hslug = crud::handoff::create(
        tmp.path(), &ns(), "base-v2", "/d/whatever-timestamped.md", Some("resume-base-v2"),
    ).unwrap();
    assert_eq!(hslug, "resume-base-v2");

    let fslug = crud::handoff::create_fork(
        tmp.path(), &ns(), "base-v2", "/d/some-doc.md", Some("graph-migration"),
    ).unwrap();
    assert_eq!(fslug, "graph-migration");

    let store = store_of(tmp.path());
    assert!(ask(&store, &format!(
        "GRAPH ?g {{ <{u}handoff/resume-base-v2> a {p}:Handoff ; {p}:kind \"handoff\" }}"
    )));
    assert!(ask(&store, &format!(
        "GRAPH ?g {{ <{u}handoff/graph-migration> a {p}:Handoff ; {p}:kind \"fork\" }}"
    )));
}

/// A nested doc path with dots in directories still derives the basename stem.
#[test]
fn fork_slug_strips_dir_and_single_extension() {
    let tmp = workspace();
    let slug = crud::handoff::create_fork(
        tmp.path(), &ns(), "p", "/x/y.z/graph-migration-spec.md", None,
    ).unwrap();
    assert_eq!(slug, "graph-migration-spec");
}

/// Forks are ADDITIVE — creating a second fork does NOT archive the first.
/// Multiple forks for the same project coexist, all open.
#[test]
fn forks_are_additive_multiple_open() {
    let tmp = workspace();
    crud::handoff::create_fork(tmp.path(), &ns(), "base-v2", "/d/FEATURE-A.md", None).unwrap();
    crud::handoff::create_fork(tmp.path(), &ns(), "base-v2", "/d/FEATURE-B.md", None).unwrap();

    let store = store_of(tmp.path());
    let open_forks = count(
        &store,
        &format!(
            "?h a {p}:Handoff ; {p}:kind \"fork\" ; {p}:status \"open\"",
            p = ns().prefix
        ),
    );
    assert_eq!(open_forks, 2, "both forks must stay open (no archive-prior)");
}

/// Re-creating a flow-doc at the same slug is idempotent — re-points, never
/// duplicates triples. Verified for both fork and handoff.
#[test]
fn recreate_same_slug_is_idempotent() {
    let tmp = workspace();
    let p = ns().prefix;

    crud::handoff::create_fork(tmp.path(), &ns(), "p", "/d/FEATURE-A.md", None).unwrap();
    crud::handoff::create_fork(tmp.path(), &ns(), "p", "/d/FEATURE-A.md", None).unwrap();
    crud::handoff::create(tmp.path(), &ns(), "p", "/d/RESUME.md", None).unwrap();
    crud::handoff::create(tmp.path(), &ns(), "p", "/d/RESUME.md", None).unwrap();

    let store = store_of(tmp.path());
    let forks = count(&store, &format!("?h a {p}:Handoff ; {p}:kind \"fork\" ; {p}:status \"open\""));
    assert_eq!(forks, 1, "re-create fork at same slug must not duplicate");

    // Exactly one status triple on the handoff node (no layered open+archived).
    let statuses = count(&store, &format!(
        "?h a {p}:Handoff ; {p}:kind \"handoff\" ; {p}:handoffDoc \"/d/RESUME.md\" ; {p}:status ?s"
    ));
    assert_eq!(statuses, 1, "re-create handoff at same slug must not layer status triples");
}

/// Forks and the continuity handoff are independent: neither archives the other.
/// And `*handoff` stays one-open-archives-prior — a second handoff (different
/// doc/slug) archives the first while every fork remains open.
#[test]
fn fork_and_handoff_are_independent() {
    let tmp = workspace();
    let p = ns().prefix;

    crud::handoff::create(tmp.path(), &ns(), "base-v2", "/d/HANDOFF-1.md", None).unwrap();
    crud::handoff::create_fork(tmp.path(), &ns(), "base-v2", "/d/FEATURE-A.md", None).unwrap();

    // Both open: the fork did not archive the handoff, the handoff did not archive the fork.
    let store = store_of(tmp.path());
    let open_handoffs = count(
        &store,
        &format!(
            "?h a {p}:Handoff ; {p}:status \"open\" . OPTIONAL {{ ?h {p}:kind ?k }} \
             FILTER(!BOUND(?k) || ?k != \"fork\")"
        ),
    );
    let open_forks = count(
        &store,
        &format!("?h a {p}:Handoff ; {p}:kind \"fork\" ; {p}:status \"open\""),
    );
    assert_eq!(open_handoffs, 1, "the handoff is open");
    assert_eq!(open_forks, 1, "the fork is open alongside the handoff");

    // *handoff unchanged: a SECOND handoff archives the first, fork untouched.
    crud::handoff::create(tmp.path(), &ns(), "base-v2", "/d/HANDOFF-2.md", None).unwrap();
    let store = store_of(tmp.path());
    let open_handoffs = count(
        &store,
        &format!(
            "?h a {p}:Handoff ; {p}:status \"open\" . OPTIONAL {{ ?h {p}:kind ?k }} \
             FILTER(!BOUND(?k) || ?k != \"fork\")"
        ),
    );
    let open_forks = count(
        &store,
        &format!("?h a {p}:Handoff ; {p}:kind \"fork\" ; {p}:status \"open\""),
    );
    assert_eq!(open_handoffs, 1, "archive-prior: still exactly one open handoff");
    assert_eq!(open_forks, 1, "fork remains open through handoff archive-prior");
}

/// A fork can be archived by its title (== slug), reusing the shared plumbing.
#[test]
fn fork_archive_by_title() {
    let tmp = workspace();
    let p = ns().prefix;
    let slug =
        crud::handoff::create_fork(tmp.path(), &ns(), "p", "/d/FEATURE-A.md", None).unwrap();
    crud::handoff::archive(tmp.path(), &ns(), &slug).unwrap();

    let store = store_of(tmp.path());
    let still_open = count(
        &store,
        &format!("?h a {p}:Handoff ; {p}:kind \"fork\" ; {p}:status \"open\""),
    );
    assert_eq!(still_open, 0, "archived fork must stop being open");
}

/// list_forks and the (fork-excluding) handoff list both run cleanly.
#[test]
fn list_surfaces_run() {
    let tmp = workspace();
    crud::handoff::create_fork(tmp.path(), &ns(), "p", "/d/FEATURE-A.md", None).unwrap();
    crud::handoff::create(tmp.path(), &ns(), "p", "/d/HANDOFF-1.md", None).unwrap();
    assert!(crud::handoff::list_forks(tmp.path(), &ns()).is_ok());
    assert!(crud::handoff::list(tmp.path(), &ns()).is_ok());
}
