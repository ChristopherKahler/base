//! Rank 01, second defect — the dedup key and the served text came from different
//! sources, so a rule added or edited in the graph was never served again.
//!
//! Found by `petrel`, verified by `auk` at `0ace1ba`, inside four lines of
//! `src/hook/pre_tool_use.rs`:
//!
//! - `:141` computed the key as `rules_hash(&domain_def.rendered_rules())`, which
//!   renders the **TOML**;
//! - `:147-150` then served `query_rules_from_graph(store, ...)` whenever a graph store
//!   exists, falling back to the TOML only when there is none.
//!
//! A domain whose rules live only in the graph — which is every domain whose rules were
//! added with `base rule add` — has an EMPTY `rendered_rules()`, so its key is a
//! constant. The first tool call injects and marks it. Every later tool call in that
//! session computes the same constant and is deduped, no matter how the rules changed.
//!
//! The reverse costs the other way: editing `domains.toml` changes the key and
//! re-injects text the reader has already seen, because the served text came from the
//! graph and did not change.
//!
//! The fix is one reader. What gets hashed IS what gets rendered, because both come
//! from the same list.

use std::path::Path;

use base::config::{BaseConfig, NamespaceConfig};
use base::crud;
use base::domain::session::SessionState;
use base::hook::pre_tool_use;

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

/// A workspace with one path-triggered domain and NO rules in `domains.toml`.
/// Rules go into the graph, the way `base rule add` puts them there.
fn workspace(root: &Path) {
    std::fs::create_dir_all(root.join(".base")).unwrap();
    std::fs::write(root.join(".base").join("graph.nq"), "").unwrap();
    std::fs::create_dir_all(root.join("work")).unwrap();
    std::fs::write(
        root.join(".base").join("domains.toml"),
        "[[domain]]\nname = \"probe\"\nmode = \"triggered\"\npaths = [\"work\"]\n",
    )
    .unwrap();
}

/// One tool call on `work/x.md`; the injected text.
fn touch(config: &BaseConfig, root: &Path, file: &str) -> String {
    let event = serde_json::json!({
        "tool_name": "Read",
        "tool_input": { "file_path": root.join("work").join(file).display().to_string() },
    });
    pre_tool_use::handle(config, root, &event).unwrap().1
}

#[test]
fn a_rule_added_to_the_graph_reaches_a_session_that_already_saw_the_domain() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root);
        let config = BaseConfig { namespace: ns(), ..Default::default() };
        SessionState::clear(&root.join(".base"));

        crud::rule::add(root, &ns(), "probe", "FIRST RULE", None).unwrap();
        let first = touch(&config, root, "x.md");
        assert!(
            first.contains("FIRST RULE"),
            "positive control: the first tool call has to serve something or nothing \
             below is a test: {first}"
        );

        // Same file, same session, nothing changed: correctly deduped.
        let second = touch(&config, root, "x.md");
        assert!(
            !second.contains("FIRST RULE"),
            "an unchanged domain is deduped, which is the behaviour being preserved: \
             {second}"
        );

        // A rule is added to the GRAPH. `domain_def.rendered_rules()` is still empty,
        // because domains.toml still has no rules — so at 0ace1ba the key is the same
        // constant it was two calls ago and this injection never happens.
        crud::rule::add(root, &ns(), "probe", "SECOND RULE", None).unwrap();
        let third = touch(&config, root, "y.md");
        assert!(
            third.contains("SECOND RULE"),
            "a rule added with `base rule add` must reach a session that has already \
             seen its domain. At 0ace1ba the dedup key is computed from the TOML and \
             the payload is read from the graph, so this block is suppressed and the \
             new rule is never served: {third}"
        );
    });
}

#[test]
fn eleven_rules_come_back_in_number_order_not_string_order() {
    // `ORDER BY ?pri` compares "10" against "2" as strings and puts the eleventh rule
    // second (#29). `crud/rule.rs` and `domain/query.rs` both cast to xsd:integer and
    // both carry the comment saying why; the pre-tool query was the one left behind,
    // so the FILE MATCH block and the DOMAIN block disagreed about order on any
    // domain with more than ten rules. base-config has seventeen.
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root);
        let config = BaseConfig { namespace: ns(), ..Default::default() };
        SessionState::clear(&root.join(".base"));

        for i in 0..11 {
            crud::rule::add(root, &ns(), "probe", &format!("RULE-{i:02}"), None).unwrap();
        }

        let block = touch(&config, root, "x.md");
        assert!(!block.is_empty(), "nothing was served at all");

        let order: Vec<usize> = (0..11)
            .map(|i| {
                block
                    .find(&format!("RULE-{i:02}"))
                    .unwrap_or_else(|| panic!("RULE-{i:02} missing from the block:\n{block}"))
            })
            .collect();
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(
            order, sorted,
            "the eleven rules must appear in 0..10 order. A string sort puts RULE-10 \
             second:\n{block}"
        );
    });
}

#[test]
fn a_domain_with_no_rules_anywhere_serves_nothing_rather_than_a_bare_header() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root);
        let config = BaseConfig { namespace: ns(), ..Default::default() };
        SessionState::clear(&root.join(".base"));

        let block = touch(&config, root, "x.md");
        assert!(
            !block.contains("[FILE MATCH: probe]"),
            "a header with no rules under it costs the reader a line and tells them \
             nothing: {block}"
        );
    });
}
