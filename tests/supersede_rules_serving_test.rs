//! Rank 02 — a superseded RULE is never served, on either hook.
//!
//! `tests/supersede_readers_test.rs` already pins `recall` and the prompt hook's
//! NEIGHBOURHOOD block. It does not pin the rules block, and its own comment says why:
//! the neighbourhood binds `?related` to decisions and projects, never to rules. So the
//! two surfaces that inject RULES were never covered, and neither one filters.
//!
//! Read at `0ace1ba`:
//!   - `crud::rule::fetch` (`src/crud/rule.rs:140`) DOES exclude superseded rules, so
//!     `base rule list` hides one.
//!   - the rules query inside `domain::query::query_domain_from_graph` does NOT.
//!   - `query_rules_from_graph` in `src/hook/pre_tool_use.rs` does NOT.
//!
//! A rule is hidden from the command that lists rules and served by both hooks that
//! inject them.
//!
//! Every test asserts BOTH halves — the superseded text is gone AND the live one is
//! present — because a filter that excludes everything passes "the old one is gone" just
//! as well as a correct one does.
//!
//! NECESSARY AND NOT SUFFICIENT. Measured on the operator's store 2026-09-14: zero
//! `ops:supersededBy` and zero `ops:supersedes` quads exist in either tier, counted at
//! predicate position. So this filter changes nothing a real user sees until the
//! migration sort pass (spec G2, rank 09, lane 3) writes links. It is the half that
//! makes the links mean something; it is not the half that ends the contradiction.

use std::path::Path;

use base::config::{BaseConfig, NamespaceConfig};
use base::crud;
use base::hook::pre_tool_use;

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

const OLD: &str = "Build on Windows natively, WSL is being deprecated";
const NEW: &str = "WSL is in migration, not decommissioned, correcting the earlier rule";

/// A workspace whose domain `probe` holds two CLI rules in the graph, the first
/// superseded by the second. Deliberately no `rules =` key in domains.toml: the
/// graph is the live render path, and a TOML copy would let the fallback
/// (`format_toml_rules`) satisfy an assertion the filter is supposed to satisfy.
fn corrected_workspace(root: &Path) {
    std::fs::create_dir_all(root.join(".base")).unwrap();
    std::fs::write(root.join(".base").join("graph.nq"), "").unwrap();
    std::fs::create_dir_all(root.join("work")).unwrap();
    std::fs::write(
        root.join(".base").join("domains.toml"),
        "[[domain]]\nname = \"probe\"\nmode = \"triggered\"\nprompt_keywords = [\"probe\"]\npaths = [\"work\"]\n",
    )
    .unwrap();

    let idx = crud::rule::add(root, &ns(), "probe", OLD, None).unwrap();
    assert_eq!(idx, 0, "the fixture depends on the first CLI rule being cli-0");
    crud::rule::add_with(root, &ns(), "probe", NEW, None, Some("cli-0")).unwrap();
}

fn probe_domain(root: &Path) -> base::domain::DomainDef {
    base::domain::load_domains(root)
        .into_iter()
        .find(|d| d.name == "probe")
        .expect("the probe domain is declared")
}

/// CONTROL, and it must stay GREEN. It proves the supersession edge was actually
/// written. Without it, a red arm below could mean "the filter is missing" or
/// "the fixture never superseded anything", and those are different bugs.
#[test]
fn control_the_edge_was_written_so_rule_list_already_hides_the_old_one() {
    let tmp = tempfile::tempdir().unwrap();
    corrected_workspace(tmp.path());

    let rules = crud::rule::fetch(tmp.path(), &ns(), "probe", false).unwrap();
    let texts: Vec<&str> = rules.iter().map(|(_, t, _)| t.as_str()).collect();
    assert!(
        texts.iter().any(|t| t.contains("migration")),
        "the live rule is listed: {texts:?}"
    );
    assert!(
        !texts.iter().any(|t| t.contains("deprecated")),
        "crud::rule::fetch already excludes superseded rules at src/crud/rule.rs:140, \
         so a fixture that reaches this assertion has a real edge: {texts:?}"
    );
}

/// CONTROL, and it must stay GREEN through this whole lane. `auk` ruled 2026-09-14,
/// after `petrel` found it: base keeps superseded records ON PURPOSE (`src/cli.rs:376`
/// — the superseded record is the drift evidence). The filter belongs on the three
/// serving surfaces and nowhere else. A blanket filter would delete a deliberate
/// feature, and this test is the guard that catches one.
#[test]
fn non_goal_include_superseded_still_returns_the_whole_chain() {
    let tmp = tempfile::tempdir().unwrap();
    corrected_workspace(tmp.path());

    let rules = crud::rule::fetch(tmp.path(), &ns(), "probe", true).unwrap();
    let texts: Vec<&str> = rules.iter().map(|(_, t, _)| t.as_str()).collect();
    assert!(texts.iter().any(|t| t.contains("migration")), "{texts:?}");
    assert!(
        texts.iter().any(|t| t.contains("deprecated")),
        "--include-superseded is the whole reason the record is kept rather than \
         deleted: {texts:?}"
    );
    assert!(
        rules.iter().any(|(_, t, superseded)| t.contains("deprecated") && *superseded),
        "and it is MARKED superseded, not merely present: {rules:?}"
    );
}

/// RED at `0ace1ba`. The prompt hook's `[DOMAIN: probe]` rules block.
#[test]
fn the_prompt_rules_block_carries_only_the_live_rule() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    corrected_workspace(root);

    let config = BaseConfig { namespace: ns(), ..Default::default() };
    let store = base::store::load_graph(&root.join(".base").join("graph.nq")).unwrap();
    let def = probe_domain(root);
    let (rules, _neighborhood, _served) =
        base::domain::query::query_domain_from_graph(&store, &config, &def);

    assert!(!rules.is_empty(), "nothing was served at all, so nothing below is a test");
    assert!(
        rules.contains("migration"),
        "the live rule must still be injected: {rules}"
    );
    assert!(
        !rules.contains("deprecated"),
        "injecting a rule a later rule corrected is rank 02: the agent is handed both \
         halves of a contradiction and told nothing: {rules}"
    );
}

/// RED at `0ace1ba`. The pre-tool hook's `[FILE MATCH: probe]` rules block.
///
/// Driven through `pre_tool_use::handle`, the path the product actually executes,
/// rather than through the private `query_rules_from_graph`. A knock-out aimed at a
/// wrapper the code never calls reads green and proves nothing.
#[test]
fn the_pre_tool_rules_block_carries_only_the_live_rule() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        corrected_workspace(root);

        let config = BaseConfig { namespace: ns(), ..Default::default() };
        let event = serde_json::json!({
            "tool_name": "Read",
            "tool_input": { "file_path": root.join("work").join("x.md").display().to_string() },
        });
        let (data, context) = pre_tool_use::handle(&config, root, &event).unwrap();

        assert!(
            data.domains_matched.iter().any(|d| d == "probe"),
            "the path trigger has to fire or the rest of this test is vacuous: {:?}",
            data.domains_matched
        );
        assert!(
            context.contains("migration"),
            "the live rule must still be injected: {context}"
        );
        assert!(
            !context.contains("deprecated"),
            "the tool hook is the other serving surface and it has no filter either: \
             {context}"
        );
    });
}
