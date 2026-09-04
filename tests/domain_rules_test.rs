//! A domain's rules live in two stores, and every reader has to see both.
//!
//! `base rule add` writes to the graph at `rule/<slug>/cli-N`, in its own IRI
//! namespace, so that a `base sync` rebuilding a domain from `domains.toml`
//! cannot delete a rule someone added by hand. The readers only ever looked at
//! the file, so `base domain get` answered `Rules (0)` for a domain whose rules
//! were injecting on every matching tool call — and the obvious reading of that
//! is that `base rule add` had silently failed (#38).

use std::path::Path;

use base::config::NamespaceConfig;
use base::crud;
use base::domain;

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

/// A workspace with a domain declared in `domains.toml` and an empty graph.
fn workspace(declared: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let base_dir = tmp.path().join(".base");
    std::fs::create_dir_all(&base_dir).unwrap();
    std::fs::write(base_dir.join("graph.nq"), "").unwrap();
    std::fs::write(
        base_dir.join("domains.toml"),
        format!(
            "[[domain]]\nname = \"probe\"\nmode = \"triggered\"\nkeywords = [\"probe\"]\nrules = [{declared}]\n"
        ),
    )
    .unwrap();
    tmp
}

fn probe(cwd: &Path) -> base::domain::DomainDef {
    domain::load_domains(cwd)
        .into_iter()
        .find(|d| d.name == "probe")
        .expect("the probe domain is declared")
}

/// The reader returns both stores, not just the file.
#[test]
fn a_rule_added_from_the_cli_reaches_the_domain_readers() {
    let tmp = workspace("\"a rule from the file\"");
    crud::rule::add(tmp.path(), &ns(), "probe", "a rule from the cli", None).unwrap();

    let (declared, added) = domain::rules_of(tmp.path(), &ns(), &probe(tmp.path()));
    assert_eq!(declared, vec!["a rule from the file".to_string()]);
    assert_eq!(
        added,
        vec![(0, "a rule from the cli".to_string())],
        "the graph half is what `domain get` used to drop"
    );
    assert_eq!(
        declared.len() + added.len(),
        2,
        "the count `base domain get` prints — it said 1 before this fix, and 0 with no file rules"
    );
}

/// The shape from #38: nothing in the file, everything from the CLI.
#[test]
fn a_domain_whose_rules_are_all_from_the_cli_does_not_read_as_empty() {
    let tmp = workspace("");
    for text in ["first", "second", "third"] {
        crud::rule::add(tmp.path(), &ns(), "probe", text, None).unwrap();
    }

    let (declared, added) = domain::rules_of(tmp.path(), &ns(), &probe(tmp.path()));
    assert!(declared.is_empty(), "the file declares none");
    assert_eq!(added.len(), 3, "and `Rules (0)` was the bug");
    assert_eq!(added[2].1, "third");
}

/// Outside a workspace the declared half still shows rather than erroring.
#[test]
fn a_missing_graph_yields_the_declared_rules_alone() {
    let tmp = workspace("\"a rule from the file\"");
    std::fs::remove_file(tmp.path().join(".base").join("graph.nq")).unwrap();

    let (declared, added) = domain::rules_of(tmp.path(), &ns(), &probe(tmp.path()));
    assert_eq!(declared.len(), 1, "a display command still shows what it can");
    assert!(added.is_empty());
}
