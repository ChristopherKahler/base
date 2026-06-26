//! Integration tests for `base project move` — end-to-end project re-home composed
//! on the `graph move` primitive. The current workspace (cwd) is the source; the
//! destination is resolved by name through the `[[workspace]]` registry.

use std::fs;
use std::path::Path;

use base::config::{BaseConfig, NamespaceConfig, WorkspaceEntry};
use base::crud;
use base::store::{self, GraphHealth};

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

/// Two registered workspaces under one temp root: `alpha` (source, with a project and
/// its attached subgraph) and `beta` (empty destination). Returns (config, alpha, beta).
fn two_workspaces(root: &Path) -> (BaseConfig, std::path::PathBuf, std::path::PathBuf) {
    let alpha = root.join("alpha");
    let beta = root.join("beta");
    fs::create_dir_all(alpha.join(".base")).unwrap();
    fs::create_dir_all(beta.join(".base")).unwrap();
    fs::write(beta.join(".base").join("graph.nq"), "").unwrap();

    let g = crud::workspace_graph_iri(&ns(), "alpha");
    let u = ns().uri;
    let t = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
    let mut body = String::new();
    body += &format!("<{u}project/demo> <{t}> <{u}Project> <{g}> .\n");
    body += &format!("<{u}project/demo> <{u}name> \"Demo\" <{g}> .\n");
    body += &format!("<{u}project/demo> <{u}status> \"active\" <{g}> .\n");
    body += &format!("<{u}project/demo> <{u}hasDomain> <{u}domain/demo> <{g}> .\n");
    body += &format!("<{u}domain/demo> <{t}> <{u}Domain> <{g}> .\n");
    body += &format!("<{u}task/demo.ship> <{t}> <{u}Task> <{g}> .\n");
    body += &format!("<{u}milestone/demo.v1> <{t}> <{u}Milestone> <{g}> .\n");
    body += &format!("<{u}decision/demo.use-rust> <{t}> <{u}Decision> <{g}> .\n");
    body += &format!("<{u}rule/demo/cli-1> <{t}> <{u}Rule> <{g}> .\n");
    body += &format!("<{u}note/n1> <{u}relatedTo> <{u}domain/demo> <{g}> .\n");
    body += &format!("<{u}handoff/demo-123> <{t}> <{u}Handoff> <{g}> .\n");
    fs::write(alpha.join(".base").join("graph.nq"), body).unwrap();

    let config = BaseConfig {
        namespace: ns(),
        workspace: vec![
            WorkspaceEntry { path: alpha.to_string_lossy().into() },
            WorkspaceEntry { path: beta.to_string_lossy().into() },
        ],
        ..Default::default()
    };
    (config, alpha, beta)
}

#[test]
fn project_move_rehomes_full_subgraph() {
    let root = tempfile::tempdir().unwrap();
    let (config, alpha, beta) = two_workspaces(root.path());

    let report = crud::project::move_project(&alpha, &config, "demo", "beta", false).unwrap();
    assert!(report.applied);
    assert!(report.moved_lines >= 8, "project + domain + task + milestone + decision + rule + note + handoff");

    let src = fs::read_to_string(alpha.join(".base").join("graph.nq")).unwrap();
    let dst = fs::read_to_string(beta.join(".base").join("graph.nq")).unwrap();

    // All entity kinds land in beta, attached under the rewritten stamp; none remain in alpha.
    for iri in [
        "project/demo", "domain/demo", "task/demo.ship", "milestone/demo.v1",
        "decision/demo.use-rust", "rule/demo/cli-1", "note/n1", "handoff/demo-123",
    ] {
        assert!(dst.contains(iri), "{iri} should be in destination");
        assert!(!src.contains(iri), "{iri} should be gone from source");
    }
    assert!(dst.contains("graph/ws/beta"));
    assert!(!dst.contains("graph/ws/alpha"), "no source stamp leaked");

    assert_eq!(store::graph_health(&alpha.join(".base").join("graph.nq")), GraphHealth::Healthy);
    assert_eq!(store::graph_health(&beta.join(".base").join("graph.nq")), GraphHealth::Healthy);
}

#[test]
fn project_move_dry_run_mutates_nothing() {
    let root = tempfile::tempdir().unwrap();
    let (config, alpha, beta) = two_workspaces(root.path());
    let sb = fs::read(alpha.join(".base").join("graph.nq")).unwrap();
    let db = fs::read(beta.join(".base").join("graph.nq")).unwrap();

    let report = crud::project::move_project(&alpha, &config, "demo", "beta", true).unwrap();
    assert!(!report.applied);
    assert!(report.moved_lines >= 8);
    assert_eq!(fs::read(alpha.join(".base").join("graph.nq")).unwrap(), sb);
    assert_eq!(fs::read(beta.join(".base").join("graph.nq")).unwrap(), db);
}

#[test]
fn project_move_unknown_project_errors() {
    let root = tempfile::tempdir().unwrap();
    let (config, alpha, _beta) = two_workspaces(root.path());
    assert!(crud::project::move_project(&alpha, &config, "ghost", "beta", false).is_err());
}
