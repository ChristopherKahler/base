//! `base graph analyze` never presents the catchall domain as a core abstraction or as a
//! community's name (kite F22, vole 2026-09-06).
//!
//! After the domain backfill every record no source could file points at `domain/unfiled`,
//! so on a real store it is the busiest node in the graph by construction: 1,511 edges on
//! the first migrated copy of Chris's workspace, the #2 god node behind `skyrim-companion`.
//! A hub that means "nobody filed this" is not an abstraction the operator holds, and a
//! community named after it describes the pile rather than the subject.

use std::collections::HashMap;
use std::path::Path;

use base::config::{BaseConfig, NamespaceConfig};
use base::crud;
use base::graph_analyze::{community_sample, rank_god_nodes};
use base::graph_query::load_graph;
use base::migrate::CATCHALL;

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
        "[[domain]]\nname = \"probe\"\nmode = \"triggered\"\nkeywords = [\"probe\"]\nrules = [\"r\"]\n",
    )
    .unwrap();
    let config = BaseConfig::load(tmp.path());
    base::domain::sync::sync_domains_to_graph(&config, tmp.path(), None).unwrap();
    tmp
}

fn insert(cwd: &Path, triples: &str) {
    let g = crud::workspace_graph_iri(&ns(), &crud::workspace_slug(cwd));
    crud::load_and_mutate(cwd, &ns(), &format!("INSERT DATA {{ GRAPH <{g}> {{\n{triples}\n}} }}")).unwrap();
}

/// The shape the migration leaves: a typed, named catchall with many records pointing at
/// it, and a real domain with fewer.
fn plant_migrated_shape(cwd: &Path) {
    let unfiled = crud::build_iri(&ns(), "domain", CATCHALL);
    let probe = crud::build_iri(&ns(), "domain", "probe");
    insert(cwd, &format!("<{unfiled}> rdf:type ops:Domain ; ops:name \"{CATCHALL}\" .\n"));
    for n in 0..6 {
        let d = crud::build_iri(&ns(), "document", &format!("loose-{n}"));
        insert(cwd, &format!("<{d}> rdf:type ops:Document ; ops:name \"loose {n}\" ; ops:hasDomain <{unfiled}> .\n"));
    }
    for n in 0..2 {
        let d = crud::build_iri(&ns(), "document", &format!("filed-{n}"));
        insert(cwd, &format!("<{d}> rdf:type ops:Document ; ops:name \"filed {n}\" ; ops:hasDomain <{probe}> .\n"));
    }
}

#[test]
fn the_catchall_is_never_a_god_node_however_busy_it_is() {
    let tmp = workspace();
    let cwd = tmp.path();
    plant_migrated_shape(cwd);

    let (nodes, adj) = load_graph(cwd, &ns(), false).unwrap();
    let degree: HashMap<&String, usize> =
        nodes.keys().map(|id| (id, adj.get(id).map(|v| v.len()).unwrap_or(0))).collect();
    let unfiled_key = format!("<{}>", crud::build_iri(&ns(), "domain", CATCHALL));
    assert!(
        degree.get(&unfiled_key).copied().unwrap_or(0) >= 6,
        "the fixture must make the catchall the busiest node, or the test proves nothing: {degree:?}"
    );

    let ranked = rank_god_nodes(&ns(), &nodes, &degree);
    assert!(
        !ranked.iter().any(|(id, _)| id == &unfiled_key),
        "the catchall ranked as a god node: {ranked:?}"
    );
    // The control: a real domain still ranks, and the order is still by degree.
    let probe_key = format!("<{}>", crud::build_iri(&ns(), "domain", "probe"));
    assert!(ranked.iter().any(|(id, _)| id == &probe_key), "a real domain must still rank: {ranked:?}");
    assert!(ranked.windows(2).all(|w| w[0].1 >= w[1].1), "highest degree first: {ranked:?}");
}

#[test]
fn a_community_is_never_named_after_the_catchall() {
    let tmp = workspace();
    let cwd = tmp.path();
    plant_migrated_shape(cwd);

    let (nodes, _adj) = load_graph(cwd, &ns(), false).unwrap();
    let members: Vec<&String> = nodes.keys().collect();
    let sample = community_sample(&ns(), &nodes, &members);
    assert!(!sample.is_empty(), "the sample must name something");
    assert!(sample.len() <= 5, "at most five labels: {sample:?}");
    assert!(
        !sample.iter().any(|l| l == CATCHALL),
        "the catchall named a community: {sample:?}"
    );
    // Sorted, so the same community carries the same name on every run.
    let mut sorted = sample.clone();
    sorted.sort();
    assert_eq!(sample, sorted);
}
