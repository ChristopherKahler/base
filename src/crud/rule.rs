use std::path::Path;

use anyhow::{Context, Result};
use oxigraph::sparql::QueryResults;
use oxigraph::store::Store;

use crate::config::NamespaceConfig;
use crate::crud;

/// Add a rule to a domain in the graph, optionally with a rationale (Phase 26).
///
/// Thin delegate to [`add_with`] — see the note on `note::learn`.
pub fn add(
    cwd: &Path,
    ns: &NamespaceConfig,
    domain_name: &str,
    rule_text: &str,
    rationale: Option<&str>,
) -> Result<u32> {
    add_with(cwd, ns, domain_name, rule_text, rationale, None)
}

/// [`add`], plus the rule this one supersedes — rule and edges in one update.
pub fn add_with(
    cwd: &Path,
    ns: &NamespaceConfig,
    domain_name: &str,
    rule_text: &str,
    rationale: Option<&str>,
    supersedes: Option<&str>,
) -> Result<u32> {
    let p = &ns.prefix;
    let domain_slug = crud::slugify(domain_name);
    let domain_iri = crud::build_iri(ns, "domain", &domain_slug);

    // Find next rule index for this domain.
    // CLI rules live in their own IRI namespace (cli-N) — sync rules use
    // rule/{slug}/{i} and are GC'd/renumbered on every sync, so sharing
    // that space would let a sync overwrite or delete CLI-added rules.
    let next_index = next_rule_index(cwd, ns, &domain_iri)?;
    let rule_iri = crud::build_iri(ns, "rule", &format!("{domain_slug}/cli-{next_index}"));
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);

    let escaped = crud::escape_sparql_literal(rule_text);

    // Ensure domain exists
    let ensure_domain = format!(
        "INSERT {{\n\
           GRAPH <{graph}> {{\n\
             <{domain_iri}> rdf:type {p}:Domain ; {p}:name \"{domain_name}\" .\n\
           }}\n\
         }}\n\
         WHERE {{\n\
           FILTER NOT EXISTS {{ GRAPH <{graph}> {{ <{domain_iri}> a {p}:Domain }} }}\n\
         }}"
    );
    // #127. This was `let _ =`. MEASURED by inducing a real rename failure on
    // this exact write and releasing the conflict before the rule write below:
    // `base rule add` exits 0 and prints "Rule 0 added to domain '…'", the rule
    // quads are in the graph, and the domain's `rdf:type Domain` quad is not --
    // a rule hanging off a domain record that was never written, reported as
    // success. Failing here instead means the rule is not written either, which
    // is the honest outcome: the alternative is a rule filed under a domain that
    // does not exist.
    crud::load_and_mutate(cwd, ns, &ensure_domain)
        .with_context(|| format!("ensuring domain '{domain_name}' exists"))?;

    // Optional rationale triple (Phase 26) — "Do X — because Y" on injection.
    let rationale_triple = match rationale.filter(|r| !r.is_empty()) {
        Some(r) => format!("               {p}:rationale \"{}\" ;\n", crud::escape_sparql_literal(r)),
        None => String::new(),
    };

    // Insert rule with edge to domain. {p}:index is what next_rule_index
    // MAXes over — without it every CLI rule would compute index 0 (the
    // original C3 collision, surviving as a predicate mismatch).
    let sparql = format!(
        "INSERT DATA {{\n\
           GRAPH <{graph}> {{\n\
             <{rule_iri}> rdf:type {p}:Rule ;\n\
               {p}:ruleText \"{escaped}\" ;\n\
               {p}:index \"{next_index}\" ;\n\
         {rationale_triple}\
               {p}:priority \"{next_index}\" .\n\
             <{domain_iri}> {p}:hasRule <{rule_iri}> .\n\
           }}\n\
         }}"
    );

    // Named for the refusal: `--supersedes` resolves in the store THIS write
    // loads and in no other, so the message has to say which one it searched.
    let tier = crud::tier_label(cwd);
    crud::load_read_then_mutate(cwd, ns, |store| {
        let clause = crud::supersedes_clause(store, ns, &graph, &rule_iri, supersedes, &tier)?;
        Ok(format!("{sparql}{clause}"))
    })?;
    Ok(next_index)
}

/// List rules for a domain from the graph.
/// A domain's CLI rules, lowest number first.
///
/// Ordered by the number rather than by its text: `ORDER BY ?pri` compares
/// `"10"` against `"2"` as strings and puts the eleventh rule second. Same
/// comparison that gave every rule past the tenth the index 10 (#29), one query
/// further on, so it outlived that fix and is pinned by `rule_index_test`.
/// The rules of one domain, newest-priority order, with a superseded flag on each.
///
/// THE SHARED READER. `rule list` and the domain block the prompt hook injects both
/// come through here, and auk ruled both are SERVING surfaces: what the agent receives
/// must be the current rule, not the one it replaced. So the exclusion lives here
/// rather than at either call site, where the two would drift apart.
///
/// Indices do not renumber. A rule is addressed by its IRI (`rule/{domain}/cli-N`,
/// built at the `add` above), so hiding one leaves every other index exactly where it
/// was and `rule remove --index` keeps working.
/// One CLI rule as every reader here wants it: its tier-local index, its text,
/// and whether a later record superseded it (#59). Both tiers number from 0
/// independently, so the index only means something beside its own tier.
type CliRule = (u32, String, bool);

pub fn fetch(
    cwd: &Path,
    ns: &NamespaceConfig,
    domain_name: &str,
    include_superseded: bool,
) -> Result<Vec<CliRule>> {
    let p = &ns.prefix;
    let domain_slug = crud::slugify(domain_name);
    let domain_iri = crud::build_iri(ns, "domain", &domain_slug);
    let sup_by = crate::supersede::PRED_SUPERSEDED_BY;

    // INSIDE the GRAPH group, beside the pattern it constrains. F16 shipped once with
    // a filter placed after the last arm, where it constrained nothing and `recall`
    // went on printing what it was meant to hide (`crud/note.rs`, kite, 2026-09-06).
    let no_superseded = if include_superseded {
        String::new()
    } else {
        crate::supersede::sparql_exclude_superseded(ns, "rule")
    };

    let sparql = format!(
        "SELECT ?text ?pri ?sb WHERE {{\n\
           GRAPH ?g {{\n\
             <{domain_iri}> {p}:hasRule ?rule .\n\
             ?rule {p}:ruleText ?text .\n\
             OPTIONAL {{ ?rule {p}:priority ?pri }}\n\
             OPTIONAL {{ ?rule {p}:{sup_by} ?sb }}\n\
             {no_superseded}\
           }}\n\
         }}\n\
         ORDER BY xsd:integer(?pri)"
    );

    let QueryResults::Solutions(solutions) = crud::load_and_query(cwd, ns, &sparql)? else {
        return Ok(Vec::new());
    };
    Ok(solutions
        .filter_map(|r| r.ok())
        .filter_map(|row| {
            let text = row.get("text").map(|t| crud::term_display(t.into()))?;
            let pri = row
                .get("pri")
                .map(|t| crud::term_display(t.into()))
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            Some((pri, text, row.get("sb").is_some()))
        })
        .collect())
}

pub fn list(
    cwd: &Path,
    ns: &NamespaceConfig,
    domain_name: &str,
    include_superseded: bool,
) -> Result<()> {
    let rules = fetch(cwd, ns, domain_name, include_superseded)?;
    if rules.is_empty() {
        println!("No rules for domain '{domain_name}'.");
        return Ok(());
    }

    println!("[{domain_name}] {} rules:", rules.len());
    for (pri, text, superseded) in &rules {
        // The marker appears only under `--include-superseded`, so the default output
        // of a store that never superseded a rule stays byte-identical.
        let mark = if *superseded { "  [superseded]" } else { "" };
        println!("  {pri}. {text}{mark}");
    }
    Ok(())
}

/// Every tier's rules for one domain, labelled, with each tier's OWN index.
///
/// #53. `list` prints one tier and says nothing about the other, while the hook
/// injects the union -- so a rule removed from one tier kept arriving in every
/// prompt and no command explained why. Measured on a fake home: workspace said
/// 2, global said 3, the prompt got 5, interleaved and renumbered 0-4.
///
/// The per-tier index is deliberate and is the reason this is not just a merged
/// list: `rule remove --index N` takes a tier-local index, both tiers start at
/// 0, and the renumbered figure in the injected block belongs to neither.
pub fn list_all_tiers(
    cwd: &Path,
    ns: &NamespaceConfig,
    domain_name: &str,
    include_superseded: bool,
) -> Result<()> {
    let ws_cwd = cwd.to_path_buf();
    let gbl_cwd = crate::home::home_root()
        .map(|h| h.join(".base-gbl"))
        .unwrap_or_else(|| cwd.to_path_buf());

    let mut total = 0usize;
    let mut shown: Vec<(&str, Vec<CliRule>)> = Vec::new();
    for (label, c) in [("workspace", &ws_cwd), ("global", &gbl_cwd)] {
        let rules = fetch(c, ns, domain_name, include_superseded).unwrap_or_default();
        total += rules.len();
        shown.push((label, rules));
    }

    if total == 0 {
        println!("No rules for domain '{domain_name}' in either tier.");
        return Ok(());
    }

    println!("[{domain_name}] {total} rules across both tiers:");
    for (label, rules) in &shown {
        if rules.is_empty() {
            println!("  ({label}: none)");
            continue;
        }
        for (pri, text, superseded) in rules {
            let mark = if *superseded { "  [superseded]" } else { "" };
            println!("  {label:<9} {pri}. {text}{mark}");
        }
    }
    println!("\nIndices are per tier; `rule remove` takes the index shown beside its own tier.");
    Ok(())
}

/// Remove a rule by index from a domain.
/// Remove one CLI rule from THIS tier. Returns how many went — READ BACK from the
/// store after the write, not assumed: 0 means the index is not in this tier,
/// which is not the same as success.
///
/// #55. This ran the DELETE and returned `Ok(())` whatever matched, so
/// `rule remove --domain X --index 10` against a tier with no rules at all
/// printed "Rule 10 removed from domain 'X'" and exited 0. The index is
/// tier-local and both tiers start at 0, so the number a user reads in their
/// injected context routinely names a different rule here.
///
/// #112. #55 gave `cli.rs:2851-2858` an `Ok(0)` branch that exits non-zero, and
/// this function could never reach it. The guard read `GRAPH ?g` — a wildcard over
/// every named graph in this tier's FILE — while the DELETE was scoped to
/// `GRAPH <graph/ws/{slug}>`, the tier's own graph. A rule sitting in a foreign
/// named graph (460 quads of them on the reporting install) passed the guard,
/// matched nothing in the DELETE, and the hardcoded `Ok(1)` reported success. Both
/// sides now ask the wildcard's question, which is what six of the seven other
/// removers in `crud` already ask, and the return value asks the store.
///
/// A wildcard cannot cross a tier: `lock_and_load` resolves ONE path through
/// `find_workspace_base(cwd)`, so `GRAPH ?g` ranges over the named graphs inside
/// one tier's own `graph.nq` and reaches nothing else.
pub fn remove(cwd: &Path, ns: &NamespaceConfig, domain_name: &str, index: u32) -> Result<usize> {
    let p = &ns.prefix;
    let domain_slug = crud::slugify(domain_name);
    let domain_iri = crud::build_iri(ns, "domain", &domain_slug);

    // One lock, one load, and the load is INSIDE the lock. The old guard was a
    // second `fetch`, which parsed the whole graph through `load_and_query` before
    // `load_and_mutate` parsed it again.
    let (store, trig_path, _lock) = crud::lock_and_load(cwd)?;

    // Counted as DISTINCT rules, which is the unit every other remover here
    // reports (`decision::delete` returns decisions, `milestone::delete` returns
    // cascade-deleted tasks). Superseded rules are counted deliberately: one still
    // occupies its index, and refusing to remove it because the default view hides
    // it would be a fresh false "no rule N" of exactly the kind #55 is about.
    let count = format!(
        "SELECT (COUNT(DISTINCT ?rule) AS ?n) WHERE {{\n\
           GRAPH ?g {{\n\
             <{domain_iri}> {p}:hasRule ?rule .\n\
             ?rule {p}:index \"{index}\" .\n\
           }}\n\
         }}"
    );
    let before = count_rules(&store, ns, &count)?;
    if before == 0 {
        return Ok(0);
    }

    // Match by {p}:index predicate, not constructed IRI — CLI rules live at
    // rule/{slug}/cli-N and are the only rules carrying {p}:index. Synced
    // rules are managed by editing domains.toml (sync GC handles them).
    let sparql = format!(
        "{}\n\
         DELETE {{\n\
           GRAPH ?g {{\n\
             ?rule ?rp ?ro .\n\
             <{domain_iri}> {p}:hasRule ?rule .\n\
           }}\n\
         }}\n\
         WHERE {{\n\
           GRAPH ?g {{\n\
             <{domain_iri}> {p}:hasRule ?rule .\n\
             ?rule {p}:index \"{index}\" ;\n\
               ?rp ?ro .\n\
           }}\n\
         }}",
        crud::prefixes(ns)
    );

    // Wide, not Target: `changelog::derive_graph_iri` cannot name a target for a
    // `GRAPH ?g` update, so Target would record a MultiGraph delta GAP where a
    // retraction owes a delta. `note::remove` takes Wide for the same statement
    // shape, and `store.rs` documents the scope for exactly this case.
    crate::store::update_and_write(
        &store,
        &trig_path,
        &sparql,
        crate::store::Scope::Wide,
        crate::store::Intent::Knowledge,
    )?;

    // READ BACK, which is the only thing that separates a repaired write from a
    // silenced one. `update_and_write` applies the update to THIS store and only
    // then serialises it to `trig_path`, propagating any failure — so on `Ok` the
    // file on disk was written FROM the store being re-queried here, and the two
    // cannot disagree. A count taken before the write, or assumed from the guard,
    // would report exactly what #112 reported.
    let after = count_rules(&store, ns, &count)?;
    if after > before {
        anyhow::bail!(
            "removing rule {index} from domain '{domain_name}' left MORE rules at that \
             index than it started with ({before} before, {after} after)"
        );
    }
    Ok(before - after)
}

/// How many distinct rules a counting query binds, asked of a store already loaded.
///
/// Split out because `remove` asks it twice with the same text — once before the
/// write and once after — and the two readings are only comparable if they are the
/// same question put to the same store.
fn count_rules(store: &Store, ns: &NamespaceConfig, sparql: &str) -> Result<usize> {
    let full = format!("{}\n{}", crud::prefixes(ns), sparql);
    let QueryResults::Solutions(mut solutions) = crate::store::query(store, &full)? else {
        anyhow::bail!("rule count query did not return solutions: {sparql}");
    };
    let Some(row) = solutions.next().transpose()? else {
        return Ok(0);
    };
    let Some(term) = row.get("n") else { return Ok(0) };
    let text = crud::term_display(term.into());
    // Parsed, never defaulted: a count that failed to parse is not a count of zero,
    // and zero is the value the caller treats as "no such rule".
    text.parse()
        .with_context(|| format!("rule count returned an unreadable value: {text:?}"))
}

/// Find the next available rule index for a domain.
/// Uses MAX(index) + 1 to avoid collisions after deletions.
fn next_rule_index(cwd: &Path, ns: &NamespaceConfig, domain_iri: &str) -> Result<u32> {
    let p = &ns.prefix;
    let sparql = format!(
        "SELECT (MAX(xsd:integer(?idx)) AS ?max_idx) WHERE {{\n\
           GRAPH ?g {{\n\
             <{domain_iri}> {p}:hasRule ?rule .\n\
             ?rule {p}:index ?idx .\n\
           }}\n\
         }}"
    );

    let results = crud::load_and_query(cwd, ns, &sparql)?;
    if let QueryResults::Solutions(solutions) = results
        && let Some(Ok(row)) = solutions.into_iter().next()
        && let Some(term) = row.get("max_idx")
    {
        let max_str = crud::term_display(term.into());
        if let Ok(max_val) = max_str.parse::<u32>() {
            return Ok(max_val + 1);
        }
    }
    Ok(0)
}
