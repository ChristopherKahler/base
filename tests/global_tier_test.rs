//! `--global` must write the global tier, not the workspace tier above it.
//!
//! `cli::tier_cwd` swaps cwd for `<home>/.base-gbl` and hands that to
//! `config::find_workspace_base`. While that was a walk-up, a machine whose
//! `<home>/.base-gbl/.base` did not exist yet — every machine that has only run
//! `base install`, which creates `.base-gbl` and stops — climbed one level too
//! far and wrote `<home>/.base`, the WORKSPACE tier, reporting success. It hit
//! every `-g` verb, on both platforms.
//!
//! Each test below builds the exact shape that triggered it: the tier absent,
//! and a decoy `<home>/.base` waiting one level up. The decoy is what makes
//! these fire on Linux as well as Windows, where the original report landed.

use std::fs;
use std::path::{Path, PathBuf};

use base::config::{self, NamespaceConfig};

/// A home with a workspace tier already in it and the global tier NOT created.
fn home_with_decoy_workspace(root: &Path) -> PathBuf {
    let decoy = root.join(".base");
    fs::create_dir_all(&decoy).unwrap();
    fs::write(decoy.join("graph.nq"), "<urn:decoy/s> <urn:decoy/p> \"untouched\" <urn:decoy/g> .\n")
        .unwrap();

    let tier_root = root.join(".base-gbl");
    fs::create_dir_all(&tier_root).unwrap();
    assert!(!tier_root.join(".base").exists(), "precondition: the global tier is not created yet");
    tier_root
}

fn decoy_bytes(root: &Path) -> Vec<u8> {
    fs::read(root.join(".base").join("graph.nq")).unwrap()
}

fn assert_op(fact_id: &str) -> String {
    serde_json::json!({
        "named_graph": "https://basemode.ai/g/6f1c",
        "type": "assert",
        "fact_id": fact_id,
        "payload": {
            "quads": ["<urn:s/1> <urn:p/1> \"inbound fact\" <urn:g/team> ."],
            "ws": "team",
            "at": "2026-08-25T13:00:00Z",
        },
    })
    .to_string()
}

#[test]
fn apply_ops_global_writes_the_global_tier_not_a_walked_up_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let tier_root = home_with_decoy_workspace(tmp.path());
        let before = decoy_bytes(tmp.path());

        // Exactly what `cli.rs` does for `base graph apply-ops --global`.
        let base_dir = config::find_workspace_base(&tier_root).expect("the global tier resolves");
        let graph = base_dir.join("graph.nq");
        let (out, code) = base::apply_ops::run(&graph, &format!("[{}]", assert_op("01GLOBAL0001")));

        assert_eq!(code, 0, "got {out}");
        assert_eq!(out["applied"], 1);
        assert_eq!(
            graph,
            tmp.path().join(".base-gbl").join(".base").join("graph.nq"),
            "the inbound fact must land in the global tier"
        );
        assert!(fs::read_to_string(&graph).unwrap().contains("inbound fact"));
        assert_eq!(decoy_bytes(tmp.path()), before, "the workspace graph must be byte-identical");
    });
}

#[test]
fn learn_global_writes_the_global_tier_and_leaves_the_workspace_graph_byte_identical() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let tier_root = home_with_decoy_workspace(tmp.path());
        let before = decoy_bytes(tmp.path());

        // `base learn -g` — the defect is a class, and the verb that first shipped
        // with it is not the verb that reported it.
        base::crud::note::learn(
            &tier_root,
            &NamespaceConfig::default(),
            "global tier note",
            "insight",
            None,
            None,
            None,
        )
        .unwrap();

        let global_graph = tmp.path().join(".base-gbl").join(".base").join("graph.nq");
        assert!(global_graph.exists(), "the global tier graph must be created by the write");
        assert!(fs::read_to_string(&global_graph).unwrap().contains("global tier note"));
        assert_eq!(decoy_bytes(tmp.path()), before, "the workspace graph must be byte-identical");
    });
}

#[test]
fn changes_global_reads_the_global_tier() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let tier_root = home_with_decoy_workspace(tmp.path());

        base::crud::note::learn(
            &tier_root,
            &NamespaceConfig::default(),
            "global tier note",
            "insight",
            None,
            None,
            None,
        )
        .unwrap();

        // Exactly what `cli.rs` does for `base changes --global`.
        let base_dir = config::find_workspace_base(&tier_root).expect("the global tier resolves");
        let log = base::changelog::log_path_for(&base_dir.join("graph.nq"));
        assert_eq!(
            log.parent().unwrap(),
            tmp.path().join(".base-gbl").join(".base"),
            "the reader and the writer must agree on which tier they are on"
        );

        let page = base::changelog::read_since(&log, 0).unwrap();
        assert_eq!(page.lines.len(), 1, "the global write is visible to the global reader");
        let record: serde_json::Value = serde_json::from_str(&page.lines[0]).unwrap();
        assert_eq!(record["ws"], serde_json::json!("base-gbl"), "and it labels the right tier");
    });
}

/// The tier resolves before it exists, so a first pull on a brand-new machine —
/// the case `apply-ops` was built for — is not the one case that cannot work.
#[test]
fn the_global_tier_resolves_on_a_home_where_nothing_has_been_created() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let tier_root = tmp.path().join(".base-gbl");
        assert!(!tier_root.exists(), "precondition: bare home, no base directories at all");

        let base_dir = config::find_workspace_base(&tier_root).expect("the global tier resolves");
        let graph = base_dir.join("graph.nq");
        let (out, code) = base::apply_ops::run(&graph, &format!("[{}]", assert_op("01FRESH0001")));

        assert_eq!(code, 0, "got {out}");
        assert_eq!(out["applied"], 1);
        assert!(graph.exists(), "the first pull creates the tier it writes to");
    });
}
