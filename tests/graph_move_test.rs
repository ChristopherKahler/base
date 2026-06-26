//! Integration tests for `base graph move` — the public subgraph-transfer API.
//! Mirrors the hand-run migration this feature codifies: select a subgraph, rewrite
//! its named-graph stamp source→dest, append to dest, remove from source, atomically.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use base::config::{NamespaceConfig, WorkspaceEntry};
use base::crud;
use base::graph_move::{self, MoveSpec, Selector};
use base::store::{self, GraphHealth};

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}
fn gws(slug: &str) -> String {
    crud::workspace_graph_iri(&ns(), slug)
}

/// Write a source graph stamped `graph/ws/alpha`: a project, its domain, a task, a
/// decision, an edge-attached note, an unrelated referencer, and AST residual.
fn source_fixture(dir: &Path) -> PathBuf {
    let g = gws("alpha");
    let u = ns().uri;
    let t = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
    let mut body = String::new();
    body += &format!("<{u}project/demo> <{t}> <{u}Project> <{g}> .\n");
    body += &format!("<{u}project/demo> <{u}name> \"Demo\" <{g}> .\n");
    body += &format!("<{u}project/demo> <{u}hasDomain> <{u}domain/demo> <{g}> .\n");
    body += &format!("<{u}domain/demo> <{t}> <{u}Domain> <{g}> .\n");
    body += &format!("<{u}task/demo.ship> <{t}> <{u}Task> <{g}> .\n");
    body += &format!("<{u}task/demo.ship> <{u}name> \"Ship\" <{g}> .\n");
    body += &format!("<{u}decision/demo.use-rust> <{t}> <{u}Decision> <{g}> .\n");
    body += &format!("<{u}note/n1> <{t}> <{u}Note> <{g}> .\n");
    body += &format!("<{u}note/n1> <{u}relatedTo> <{u}domain/demo> <{g}> .\n");
    body += &format!("<{u}decision/other.x> <{u}affects> <{u}project/demo> <{g}> .\n");
    body += &format!("<http://ops-sys.local/code#demo_fn> <{t}> <{u}Function> <{g}> .\n");
    body += &format!("<{u}codemap/demo> <{t}> <{u}CodeMap> <{g}> .\n");
    let p = dir.join("source.nq");
    fs::write(&p, body).unwrap();
    p
}

fn spec(src: &Path, dst: &Path, no_ast: bool) -> MoveSpec {
    MoveSpec {
        source_path: src.to_path_buf(),
        dest_path: dst.to_path_buf(),
        source_graph: gws("alpha"),
        dest_graph: gws("beta"),
        source_ws: "alpha".into(),
        dest_ws: "beta".into(),
        no_ast,
    }
}

#[test]
fn move_makes_subgraph_visible_in_dest_and_gone_from_source() {
    let dir = tempfile::tempdir().unwrap();
    let src = source_fixture(dir.path());
    let dst = dir.path().join("dest.nq");
    fs::write(&dst, "").unwrap();

    let report =
        graph_move::graph_move(&spec(&src, &dst, false), &Selector::Domain("Demo".into()), &ns(), false)
            .unwrap();
    assert!(report.applied);
    assert!(report.moved_lines >= 8);

    let dst_txt = fs::read_to_string(&dst).unwrap();
    // Named-graph rewrite landed: dest carries graph/ws/beta, never graph/ws/alpha.
    assert!(dst_txt.contains(&format!("<{}> .", gws("beta"))));
    assert!(!dst_txt.contains(&format!("<{}> .", gws("alpha"))));
    assert!(dst_txt.contains("project/demo"));
    assert!(dst_txt.contains("note/n1"), "edge-attached note moved");

    // Source no longer holds the project's curated subgraph; the unrelated referencer
    // stays (dangling-incoming, logged not moved). Subject-precise: a kept dangling
    // line still mentions project/demo as an OBJECT, so check subjects, not substrings.
    let src_txt = fs::read_to_string(&src).unwrap();
    let u = ns().uri;
    let has_subject = |txt: &str, iri: &str| txt.lines().any(|l| graph_move::parse_subject(l) == Some(iri));
    assert!(!has_subject(&src_txt, &format!("{u}project/demo")));
    assert!(has_subject(&src_txt, &format!("{u}decision/other.x")), "unrelated referencer stays");
    assert!(report.dangling_incoming >= 1);

    // Both tiers parse cleanly — the migration left no corruption.
    assert_eq!(store::graph_health(&src), GraphHealth::Healthy);
    assert_eq!(store::graph_health(&dst), GraphHealth::Healthy);
}

#[test]
fn move_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let src = source_fixture(dir.path());
    let dst = dir.path().join("dest.nq");
    fs::write(&dst, "").unwrap();

    graph_move::graph_move(&spec(&src, &dst, false), &Selector::Domain("Demo".into()), &ns(), false).unwrap();
    let second =
        graph_move::graph_move(&spec(&src, &dst, false), &Selector::Domain("Demo".into()), &ns(), false).unwrap();
    assert_eq!(second.moved_lines, 0, "re-running moves nothing");
}

#[test]
fn dry_run_mutates_nothing_but_counts_the_move() {
    let dir = tempfile::tempdir().unwrap();
    let src = source_fixture(dir.path());
    let dst = dir.path().join("dest.nq");
    fs::write(&dst, "").unwrap();
    let (sb, db) = (fs::read(&src).unwrap(), fs::read(&dst).unwrap());

    let report =
        graph_move::graph_move(&spec(&src, &dst, false), &Selector::Domain("Demo".into()), &ns(), true).unwrap();
    assert!(!report.applied);
    assert!(report.moved_lines >= 8);
    assert_eq!(fs::read(&src).unwrap(), sb);
    assert_eq!(fs::read(&dst).unwrap(), db);
}

#[test]
fn no_ast_excludes_code_and_codemap() {
    let dir = tempfile::tempdir().unwrap();
    let src = source_fixture(dir.path());
    let dst = dir.path().join("dest.nq");
    fs::write(&dst, "").unwrap();

    let mut subjects: HashSet<String> =
        graph_move::resolve_selector(&src, &Selector::Prefix("demo".into()), &gws("alpha"), &ns()).unwrap();
    subjects.insert("http://ops-sys.local/code#demo_fn".to_string());
    subjects.insert(format!("{}codemap/demo", ns().uri));

    let report = graph_move::graph_move_subjects(&spec(&src, &dst, true), &subjects, &ns(), false).unwrap();
    assert!(report.ast_excluded >= 2);
    let dst_txt = fs::read_to_string(&dst).unwrap();
    assert!(!dst_txt.contains("code#demo_fn"));
    assert!(!dst_txt.contains("codemap/demo"));
}

#[test]
fn selector_parse_covers_documented_forms() {
    assert_eq!(
        Selector::parse("domain:base-v2").unwrap(),
        Selector::Domain("base-v2".into())
    );
    assert_eq!(
        Selector::parse("prefix:proj").unwrap(),
        Selector::Prefix("proj".into())
    );
    assert_eq!(
        Selector::parse("node:http://x#project/p").unwrap(),
        Selector::Node("http://x#project/p".into())
    );
    // A bare full IRI is a node.
    assert_eq!(
        Selector::parse("http://ops-sys.local/ontology#project/p").unwrap(),
        Selector::Node("http://ops-sys.local/ontology#project/p".into())
    );
    // A bare non-IRI token is rejected with guidance.
    assert!(Selector::parse("base-v2").is_err());
}

#[test]
fn resolve_workspace_picks_existing_graph_and_errors_on_unknown() {
    let root = tempfile::tempdir().unwrap();
    let alpha = root.path().join("alpha");
    fs::create_dir_all(alpha.join(".base")).unwrap();
    fs::write(alpha.join(".base").join("graph.nq"), "").unwrap();
    let registry = vec![WorkspaceEntry { path: alpha.to_string_lossy().into() }];

    let (path, slug) = graph_move::resolve_workspace("alpha", &registry).unwrap();
    assert_eq!(slug, "alpha");
    assert!(path.ends_with(Path::new(".base/graph.nq")));

    assert!(graph_move::resolve_workspace("nope", &registry).is_err());
}
