use std::path::Path;

use anyhow::{Context, Result};
use oxigraph::sparql::QueryResults;
use oxigraph::store::Store;

use crate::config::NamespaceConfig;
use crate::crud;
use crate::domain::rules::RuleTests;
use crate::domain::tier::Tier;

/// `base rule add --path` (P7): each path as the full path its place matcher stores, so the rule fires only when
/// that file, or something under that folder, is touched. Refused, naming why, unless the rule can be scoped there.
/// Relative input is the workspace root's (home outside a workspace), the rule every path flag in base follows.
///
/// For the domain of a registered project with a folder, the path must lie inside that folder and belong to that
/// project: a path inside a project folder deeper in it belongs to that project, whose own domain takes the rule
/// (P2, D13). For any domain the path must not hold a registered project other than its own, since the rule would
/// then fire in that project whatever its `nested` says. Writes nothing.
pub fn scoped_places(cwd: &Path, domain: &str, raw: &[String]) -> std::result::Result<Vec<String>, String> {
    use crate::domain::matcher;
    if raw.iter().all(|r| r.trim().is_empty()) {
        return Ok(Vec::new());
    }
    let ns = crate::config::BaseConfig::load(cwd).namespace;
    let roots = crud::project::PathRoots::new(cwd, &ns);
    let ctx = crate::domain::trigger_context(cwd);
    let own = crud::slugify(domain);
    let spelled = |p: &str| crud::project::absolute_path(p, None, None).unwrap_or_else(|| p.to_string());
    let own_folders: Vec<&matcher::Registered> =
        ctx.registered.iter().filter(|r| r.slug == own && !r.path.is_empty()).collect();
    let mut out = Vec::new();
    for r in raw.iter().map(|r| r.trim()).filter(|r| !r.is_empty()) {
        if r.contains(['*', '?']) {
            return Err(format!("--path {r}: a pattern is not a path; name one file or folder"));
        }
        let Some(full) = roots.from_cli(r) else {
            return Err(format!("--path {r}: cannot be made a full path here"));
        };
        let Some(resolved) = matcher::resolve_trigger(&full, None, ctx.home.as_deref()) else {
            return Err(format!("--path {r}: cannot be made a full path here"));
        };
        if !own_folders.is_empty() {
            if !own_folders.iter().any(|f| matcher::path_under(&resolved, &f.path)) {
                return Err(format!(
                    "{full} is not inside {domain}'s folder {}: a rule's --path lies inside its project",
                    spelled(&own_folders[0].path)
                ));
            }
            if let Some(o) = matcher::owners(&resolved, &ctx.registered).into_iter().find(|o| o.slug != own) {
                return Err(format!(
                    "{full} belongs to {} (its folder is {}): add the rule to that project's domain, base rule add --domain {} ...",
                    o.name,
                    spelled(&o.path),
                    o.slug
                ));
            }
        }
        let mut held: Vec<String> = Vec::new();
        for p in ctx.registered.iter().filter(|p| p.slug != own && !p.path.is_empty() && matcher::path_under(&p.path, &resolved)) {
            if !held.contains(&p.name) {
                held.push(p.name.clone());
            }
        }
        if !held.is_empty() {
            held.sort_by_key(|n| n.to_lowercase());
            return Err(format!(
                "{full} holds {}: a rule's --path is one file, or a folder inside one project",
                matcher::count_projects(&held)
            ));
        }
        out.push(full);
    }
    Ok(out)
}

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
    add_with_matchers(cwd, ns, domain_name, rule_text, rationale, supersedes, &[])
}

/// [`add_with`], plus the rule's own matchers (spec F11): flat literals on the CLI rule, which sync never touches.
pub fn add_with_matchers(
    cwd: &Path,
    ns: &NamespaceConfig,
    domain_name: &str,
    rule_text: &str,
    rationale: Option<&str>,
    supersedes: Option<&str>,
    matchers: &[crate::domain::rules::Matcher],
) -> Result<u32> {
    add_with_tests(cwd, ns, domain_name, rule_text, rationale, supersedes, matchers, &RuleTests::default())
}

/// [`add_with_matchers`], plus the rule's test prompts (K2b): flat `firesOn` / `quietOn` literals on the CLI rule, in
/// the same write, so the rule never exists without them.
#[allow(clippy::too_many_arguments)]
pub fn add_with_tests(
    cwd: &Path,
    ns: &NamespaceConfig,
    domain_name: &str,
    rule_text: &str,
    rationale: Option<&str>,
    supersedes: Option<&str>,
    matchers: &[crate::domain::rules::Matcher],
    tests: &RuleTests,
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
    let matcher_triples: String = crate::domain::rules::matcher_literals(matchers)
        .iter()
        .map(|(pred, v)| format!("               {p}:{pred} \"{}\" ;\n", crud::escape_sparql_literal(v)))
        .collect();
    let test_triples: String = crate::domain::rules::test_literals(&tests.fires_on, &tests.quiet_on)
        .iter()
        .map(|(pred, v)| format!("               {p}:{pred} \"{}\" ;\n", crud::escape_sparql_literal(v)))
        .collect();

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
         {matcher_triples}\
         {test_triples}\
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
    let (matchers, mut tests) = by_text(cwd, ns, domain_name);
    for (text, t) in toml_tests_by_text(cwd, domain_name) {
        tests.entry(text).or_default().merge(&t);
    }
    // A retired rule (BO-17) is listed only under `--include-superseded`, as a superseded one is, and says which.
    let retired = if include_superseded { retired_texts(cwd, ns, domain_name) } else { Default::default() };
    for (pri, text, superseded) in &rules {
        // The marker appears only under `--include-superseded`, so the default output
        // of a store that never superseded a rule stays byte-identical.
        let mark = if retired.contains(text) {
            "  [retired: base rule unretire brings it back]"
        } else if *superseded {
            "  [superseded]"
        } else {
            ""
        };
        // K2b: the id `base rule update` and `base rule test --rule` take.
        println!("  {pri}. [{}] {text}{mark}", rule_ref(domain_name, text));
        // F11: the listing shows each rule's kinds and matchers, and only for a rule that has some, so a store
        // with no converted rule prints exactly what it printed before.
        if let Some(m) = matchers.get(text) {
            println!("      match: {}", crate::domain::rules::describe_matchers(m));
        }
        if let Some(t) = tests.get(text) {
            println!("      tests: {}", crate::domain::rule_test::tests_line(t));
        }
    }
    Ok(())
}

/// The wording of `domain_name`'s retired rules in `cwd`'s tier (BO-17).
fn retired_texts(cwd: &Path, ns: &NamespaceConfig, domain_name: &str) -> std::collections::HashSet<String> {
    let p = &ns.prefix;
    let domain_iri = crud::build_iri(ns, "domain", &crud::slugify(domain_name));
    let sparql = format!(
        "SELECT ?text WHERE {{ GRAPH ?g {{ <{domain_iri}> {p}:hasRule ?rule . ?rule {p}:ruleText ?text ; {p}:{} ?when }} }}",
        crate::supersede::PRED_RETIRED_AT
    );
    match crud::load_and_query(cwd, ns, &sparql) {
        Ok(QueryResults::Solutions(rows)) => {
            rows.filter_map(|r| r.ok()).filter_map(|r| r.get("text").map(|t| crud::term_display(t.into()))).collect()
        }
        _ => Default::default(),
    }
}

/// `base.9f2c1a7b`: a rule's domain and the start of its id, as `base rule update` takes it.
fn rule_ref(domain_name: &str, text: &str) -> String {
    crate::domain::rule_test::short_ref(domain_name, &crate::domain::rules::rule_id(domain_name, text))
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
    let (mut matchers, mut tests) = by_text(&ws_cwd, ns, domain_name);
    let (gbl_matchers, gbl_tests) = by_text(&gbl_cwd, ns, domain_name);
    matchers.extend(gbl_matchers);
    // A rule in both tiers, or in a graph and a file, is one rule (its id is its text): it carries the tests of all.
    for (text, t) in gbl_tests.into_iter().chain(toml_tests_by_text(cwd, domain_name)) {
        tests.entry(text).or_default().merge(&t);
    }
    for (label, rules) in &shown {
        if rules.is_empty() {
            println!("  ({label}: none)");
            continue;
        }
        // A retired rule (BO-17) is listed only under `--include-superseded`, and says which.
        let tier_cwd = if *label == "workspace" { &ws_cwd } else { &gbl_cwd };
        let retired = if include_superseded { retired_texts(tier_cwd, ns, domain_name) } else { Default::default() };
        for (pri, text, superseded) in rules {
            let mark = if retired.contains(text) {
                "  [retired: base rule unretire brings it back]"
            } else if *superseded {
                "  [superseded]"
            } else {
                ""
            };
            println!("  {label:<9} {pri}. [{}] {text}{mark}", rule_ref(domain_name, text));
            if let Some(m) = matchers.get(text) {
                println!("              match: {}", crate::domain::rules::describe_matchers(m));
            }
            if let Some(t) = tests.get(text) {
                println!("              tests: {}", crate::domain::rule_test::tests_line(t));
            }
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

/// Each rule's matchers in THIS tier, by its text (F11: `rule list` shows each rule's kinds and matchers).
/// Keyed on text because the text is a rule's identity (`domain::rules::rule_id`); a rule with none is absent.
pub fn matchers_by_text(
    cwd: &Path,
    ns: &NamespaceConfig,
    domain_name: &str,
) -> std::collections::HashMap<String, Vec<crate::domain::rules::Matcher>> {
    by_text(cwd, ns, domain_name).0
}

/// Each rule's matchers and its CLI test prompts in THIS tier, by its text, over one load of the tier's graph (F11,
/// K2b): what `rule list` prints under each rule. A synced copy's test literals are not read: a `domains.toml` rule's
/// tests come from the file ([`toml_tests_by_text`]).
fn by_text(
    cwd: &Path,
    ns: &NamespaceConfig,
    domain_name: &str,
) -> (std::collections::HashMap<String, Vec<crate::domain::rules::Matcher>>, std::collections::HashMap<String, RuleTests>) {
    let p = &ns.prefix;
    let domain_iri = crud::build_iri(ns, "domain", &crud::slugify(domain_name));
    let matchers_sparql = format!(
        "{}\nSELECT ?text ?mp ?mv WHERE {{\n\
           GRAPH ?g {{\n\
             <{domain_iri}> {p}:hasRule ?rule .\n\
             ?rule {p}:ruleText ?text ; ?mp ?mv .\n\
             FILTER(?mp IN ({p}:matchKind, {p}:matchPlace, {p}:matchTool, {p}:matchCommand, {p}:matchWord))\n\
           }}\n\
         }}",
        crud::prefixes(ns)
    );
    let tests_sparql = format!(
        "{}\nSELECT ?text ?tp ?tv WHERE {{\n\
           GRAPH ?g {{\n\
             <{domain_iri}> {p}:hasRule ?rule .\n\
             ?rule {p}:ruleText ?text ; ?tp ?tv .\n\
             FILTER(?tp IN ({p}:firesOn, {p}:quietOn))\n\
             FILTER NOT EXISTS {{ ?rule {p}:source ?source }}\n\
           }}\n\
         }}",
        crud::prefixes(ns)
    );
    let mut pairs: std::collections::HashMap<String, Vec<(String, String)>> = std::collections::HashMap::new();
    let mut tests: std::collections::HashMap<String, RuleTests> = std::collections::HashMap::new();
    if let Ok(store) = crud::load_workspace_graph(cwd) {
        if let Ok(QueryResults::Solutions(rows)) = crate::store::query(&store, &matchers_sparql) {
            for row in rows.filter_map(|r| r.ok()) {
                let (Some(t), Some(mp), Some(mv)) = (row.get("text"), row.get("mp"), row.get("mv")) else {
                    continue;
                };
                let pred_full = crud::term_display(mp.into());
                let pred = pred_full.trim_end_matches('>').rsplit(['#', '/']).next().unwrap_or_default().to_string();
                pairs.entry(crud::term_display(t.into())).or_default().push((pred, crud::term_display(mv.into())));
            }
        }
        if let Ok(QueryResults::Solutions(rows)) = crate::store::query(&store, &tests_sparql) {
            for row in rows.filter_map(|r| r.ok()) {
                let (Some(t), Some(tp), Some(tv)) = (row.get("text"), row.get("tp"), row.get("tv")) else {
                    continue;
                };
                tests
                    .entry(crud::term_display(t.into()))
                    .or_default()
                    .add(&crud::term_display(tp.into()), &crud::term_display(tv.into()));
            }
        }
    }
    let matchers = pairs
        .into_iter()
        .map(|(text, ps)| {
            let m = crate::domain::rules::matchers_from_literals(ps.iter().map(|(p, v)| (p.as_str(), v.as_str())));
            (text, m)
        })
        .filter(|(_, m)| !m.is_empty())
        .collect();
    (matchers, tests)
}

// ─── Test prompts (K2b, BO-14) ───────────────────────────────────────────────

/// The test prompts each of `domain_name`'s `domains.toml` rules carries, by its text, as `load_domains` reads the
/// files from `cwd`. A file read, no graph.
fn toml_tests_by_text(cwd: &Path, domain_name: &str) -> std::collections::HashMap<String, RuleTests> {
    let want = crud::slugify(domain_name);
    let mut out: std::collections::HashMap<String, RuleTests> = std::collections::HashMap::new();
    for d in crate::domain::load_domains(cwd).into_iter().filter(|d| crud::slugify(&d.name) == want) {
        for r in &d.rules {
            let (fires_on, quiet_on) = r.tests();
            if fires_on.is_empty() && quiet_on.is_empty() {
                continue;
            }
            let t = RuleTests { fires_on: fires_on.to_vec(), quiet_on: quiet_on.to_vec() };
            out.entry(r.text().to_string()).or_default().merge(&t);
        }
    }
    out
}

/// One place a rule's test prompts are stored (K2a): its entry in a `domains.toml`, or the rule `base rule add` wrote
/// in one tier's graph. A synced copy of a `domains.toml` rule is not one: sync rewrites it from the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestHome {
    Toml { file: std::path::PathBuf, tier: Tier },
    Graph { cwd: std::path::PathBuf, tier: Tier, iri: String },
}

impl TestHome {
    /// `domains.toml (workspace tier)`, `graph (global tier)`: where `rule update` says it wrote.
    pub fn label(&self) -> String {
        match self {
            TestHome::Toml { tier, .. } => format!("domains.toml ({} tier)", tier.label()),
            TestHome::Graph { tier, .. } => format!("graph ({} tier)", tier.label()),
        }
    }

    pub fn tier(&self) -> Tier {
        match self {
            TestHome::Toml { tier, .. } | TestHome::Graph { tier, .. } => *tier,
        }
    }
}

/// A rule [`find`] matched: its id, and each place its tests can be stored with the tests stored there now.
#[derive(Debug, Clone)]
pub struct FoundRule {
    /// The full [`crate::domain::rules::rule_id`].
    pub id: String,
    pub domain: String,
    pub text: String,
    pub homes: Vec<(TestHome, RuleTests)>,
    /// The `source` of each synced copy seen: `domains.toml`, or `ext:<name>` for an extension's rule. A rule seen
    /// only as copies has no home: its line is in an extension, or gone from `domains.toml`.
    pub copies: Vec<String>,
}

impl FoundRule {
    /// The homes `base rule update` writes: one tier, never both, so one tier's prompts never land in the other. With
    /// `-g` the global tier's; otherwise the workspace tier's, or the global tier's when the workspace holds none. Empty
    /// when `-g` is given and only the workspace holds the rule.
    pub fn homes_for(&self, global: bool) -> Vec<&(TestHome, RuleTests)> {
        let of = |t: Tier| self.homes.iter().filter(|(h, _)| h.tier() == t).collect::<Vec<_>>();
        if global {
            return of(Tier::Global);
        }
        let ws = of(Tier::Workspace);
        if ws.is_empty() { of(Tier::Global) } else { ws }
    }
}

/// `base rule update`'s and `base rule test --rule`'s argument: `<domain>.<id>`, or a bare `<id>`, where the id is a
/// rule's id or the start of one (at least 4 hex characters, as `base rule list` prints them).
pub fn parse_rule_ref(spec: &str) -> std::result::Result<(Option<String>, String), String> {
    let spec = spec.trim();
    let (domain, id) = match spec.rsplit_once('.') {
        Some((d, i)) => (Some(d.trim().to_string()).filter(|d| !d.is_empty()), i.trim()),
        None => (None, spec),
    };
    let id = id.to_ascii_lowercase();
    if id.len() < 4 || id.len() > 16 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "'{spec}' is not a rule id: write <domain>.<id>, with the id (or its first 4 or more characters) from \
             base rule list --domain <domain>"
        ));
    }
    Ok((domain, id))
}

/// The tiers to search, each as a working directory, never the same graph twice: standing inside `~/.base-gbl` with
/// no other workspace, both tiers are one file.
pub(crate) fn tier_cwds(cwd: &Path) -> Vec<(Tier, std::path::PathBuf)> {
    let mut out: Vec<(Tier, std::path::PathBuf)> = vec![(Tier::Workspace, cwd.to_path_buf())];
    if let Some(h) = crate::home::home_root() {
        out.push((Tier::Global, h.join(".base-gbl")));
    }
    let mut seen: Vec<std::path::PathBuf> = Vec::new();
    out.retain(|(_, c)| match crate::config::find_workspace_base(c) {
        Some(b) if !seen.contains(&b) => {
            seen.push(b);
            true
        }
        _ => false,
    });
    out
}

/// Every rule whose id starts with `id` (in `domain`, when given), with where its tests are stored, across both tiers'
/// `domains.toml` and both tiers' graphs. Superseded rules are left out: no prompt serves them. One entry per id.
pub fn find(cwd: &Path, ns: &NamespaceConfig, domain: Option<&str>, id: &str) -> Result<Vec<FoundRule>> {
    use crate::domain::rules::rule_id;
    let want = domain.map(crud::slugify);
    let in_domain = |name: &str| want.as_deref().is_none_or(|w| crud::slugify(name) == w);
    let mut found: Vec<FoundRule> = Vec::new();
    let mut add = |name: &str, text: &str, home: Option<TestHome>, copy: Option<String>, tests: &[(&str, String)]| {
        let rid = rule_id(name, text);
        if !rid.starts_with(id) || !in_domain(name) {
            return;
        }
        let i = match found.iter().position(|f| f.id == rid) {
            Some(i) => i,
            None => {
                found.push(FoundRule {
                    id: rid,
                    domain: name.to_string(),
                    text: text.to_string(),
                    homes: Vec::new(),
                    copies: Vec::new(),
                });
                found.len() - 1
            }
        };
        let f = &mut found[i];
        if let Some(h) = home {
            let j = match f.homes.iter().position(|(x, _)| *x == h) {
                Some(j) => j,
                None => {
                    f.homes.push((h, RuleTests::default()));
                    f.homes.len() - 1
                }
            };
            for (pred, v) in tests {
                f.homes[j].1.add(pred, v);
            }
        }
        if let Some(c) = copy
            && !f.copies.contains(&c)
        {
            f.copies.push(c);
        }
    };

    // The domains.toml files `load_domains` reads: the workspace one first, since it replaces a global domain of the
    // same name. Read whether or not the tier has a graph yet.
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    for tier in [Tier::Workspace, Tier::Global] {
        let Some(file) = crate::domain::tier::domains_toml_for(cwd, tier).filter(|f| f.is_file() && !files.contains(f))
        else {
            continue;
        };
        let tier = &tier;
        files.push(file.clone());
        for d in crate::domain::load_domains_file(&file, None) {
            for r in &d.rules {
                let (fires_on, quiet_on) = r.tests();
                let tests = crate::domain::rules::test_literals(fires_on, quiet_on);
                add(&d.name, r.text(), Some(TestHome::Toml { file: file.clone(), tier: *tier }), None, &tests);
            }
        }
    }

    // The graphs: a rule with no `source` is one `base rule add` wrote, and holds its own tests.
    let p = &ns.prefix;
    let no_superseded = crate::supersede::sparql_exclude_superseded(ns, "rule");
    let sparql = format!(
        "{}\nSELECT ?domain ?rule ?text ?source ?tp ?tv WHERE {{\n\
           GRAPH ?g {{\n\
             ?domain {p}:hasRule ?rule .\n\
             ?rule {p}:ruleText ?text .\n\
             OPTIONAL {{ ?rule {p}:source ?source }}\n\
             OPTIONAL {{ ?rule ?tp ?tv . FILTER(?tp IN ({p}:firesOn, {p}:quietOn)) }}\n\
             {no_superseded}\
           }}\n\
         }}",
        crud::prefixes(ns)
    );
    let names: std::collections::HashMap<String, String> = crate::domain::load_domains(cwd)
        .into_iter()
        .map(|d| (crud::build_iri(ns, "domain", &crud::slugify(&d.name)), d.name))
        .collect();
    for (tier, c) in &tier_cwds(cwd) {
        let Ok(path) = crud::workspace_graph_path(c) else { continue };
        if !path.is_file() {
            continue;
        }
        let store = crate::store::load_graph(&path)?;
        let QueryResults::Solutions(rows) = crate::store::query(&store, &sparql)? else {
            continue;
        };
        for row in rows.filter_map(|r| r.ok()) {
            let named = |k: &str| {
                row.get(k).and_then(|t| match t.into() {
                    oxigraph::model::TermRef::NamedNode(n) => Some(n.as_str().to_string()),
                    _ => None,
                })
            };
            let lit = |k: &str| row.get(k).map(|t| crud::term_display(t.into()));
            let (Some(d), Some(iri), Some(text)) = (named("domain"), named("rule"), lit("text")) else {
                continue;
            };
            let name = names.get(&d).cloned().unwrap_or_else(|| d.rsplit('/').next().unwrap_or_default().to_string());
            let tests: Vec<(&str, String)> = match (lit("tp"), lit("tv")) {
                (Some(tp), Some(tv)) => vec![(if tp == "firesOn" { "firesOn" } else { "quietOn" }, tv)],
                _ => Vec::new(),
            };
            match lit("source") {
                None => add(&name, &text, Some(TestHome::Graph { cwd: c.clone(), tier: *tier, iri }), None, &tests),
                // A synced copy: its tests came from the file and go back there, never here.
                Some(src) => add(&name, &text, None, Some(src), &[]),
            }
        }
    }
    for f in &mut found {
        for (_, t) in &mut f.homes {
            *t = std::mem::take(t).sorted();
        }
    }
    Ok(found)
}

/// The graph predicate that marks a rule protected (BO-20, K9f): `protected "true"` on its record.
pub const PRED_PROTECTED: &str = "protected";

/// Mark `rule` protected, or clear the mark, in each of `homes` (BO-20, `base rule update --protected`): where its tests
/// are stored, so the mark lives with the rule. A `domains.toml` entry gets `protected = true`; a graph rule
/// `protected "true"`, removed again by `--unprotected`. Says where it wrote; empty when no home still holds the rule.
pub fn store_protected(ns: &NamespaceConfig, rule: &FoundRule, homes: &[&TestHome], protected: bool) -> Result<Vec<String>> {
    let mut wrote: Vec<String> = Vec::new();
    for home in homes.iter().copied() {
        match home {
            TestHome::Toml { file, .. } => {
                if crate::domain::set_rule_protected(file, &rule.domain, &rule.id, protected)? {
                    wrote.push(home.label());
                }
            }
            TestHome::Graph { cwd, iri, .. } => {
                let p = &ns.prefix;
                let iri = iri.trim_start_matches('<').trim_end_matches('>');
                let mut sparql = format!(
                    "{}
DELETE {{ GRAPH ?g {{ <{iri}> {p}:{PRED_PROTECTED} ?v }} }}
                     WHERE {{ GRAPH ?g {{ <{iri}> {p}:{PRED_PROTECTED} ?v }} }}",
                    crud::prefixes(ns)
                );
                if protected {
                    sparql.push_str(&format!(
                        " ;
INSERT {{ GRAPH ?g {{ <{iri}> {p}:{PRED_PROTECTED} \"true\" }} }}
                         WHERE {{ GRAPH ?g {{ <{iri}> {p}:ruleText ?text }} }}"
                    ));
                }
                let (store, trig_path, _lock) = crud::lock_and_load(cwd)?;
                let ask = format!("{}
ASK {{ GRAPH ?g {{ <{iri}> {p}:ruleText ?text }} }}", crud::prefixes(ns));
                if !matches!(crate::store::query(&store, &ask)?, QueryResults::Boolean(true)) {
                    continue;
                }
                crate::store::update_and_write(
                    &store,
                    &trig_path,
                    &sparql,
                    crate::store::Scope::Wide,
                    crate::store::Intent::Knowledge,
                )?;
                wrote.push(home.label());
            }
        }
    }
    Ok(wrote)
}

/// Every rule marked protected, by its id: the `domains.toml` entries of both tiers that say `protected = true`, and
/// the graph rules of both tiers that carry `protected "true"` (BO-20, K9f).
pub fn protected_ids(cwd: &Path, ns: &NamespaceConfig) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for d in crate::domain::load_domains(cwd) {
        for r in d.rules.iter().filter(|r| r.protected()) {
            out.insert(crate::domain::rules::rule_id(&d.name, r.text()));
        }
    }
    let p = &ns.prefix;
    let sparql = format!(
        "{}
SELECT ?domain ?text WHERE {{ GRAPH ?g {{ ?domain {p}:hasRule ?rule . ?rule {p}:ruleText ?text ;          {p}:{PRED_PROTECTED} \"true\" }} }}",
        crud::prefixes(ns)
    );
    let names: std::collections::HashMap<String, String> = crate::domain::load_domains(cwd)
        .into_iter()
        .map(|d| (crud::build_iri(ns, "domain", &crud::slugify(&d.name)), d.name))
        .collect();
    if let Some(store) = crate::store::load_merged(cwd)
        && let Ok(QueryResults::Solutions(rows)) = crate::store::query(&store, &sparql)
    {
        for row in rows.filter_map(|r| r.ok()) {
            let (Some(d), Some(t)) = (row.get("domain"), row.get("text")) else { continue };
            let d = match d {
                oxigraph::model::Term::NamedNode(n) => n.as_str().to_string(),
                _ => continue,
            };
            let name = names.get(&d).cloned().unwrap_or_else(|| d.rsplit('/').next().unwrap_or_default().to_string());
            out.insert(crate::domain::rules::rule_id(&name, &crud::term_display(t.into())));
        }
    }
    out
}

/// Write `tests` as the whole test list of `rule` in each of `homes`, and say where. A `domains.toml` entry is rewritten
/// ([`crate::domain::set_rule_tests`]); a CLI rule's literals are replaced in its own tier's graph, under that tier's
/// lock, in each named graph that holds the rule. A home that no longer holds the rule (it changed after [`find`]
/// read it) is left out of what this returns, so an empty result means nothing was stored.
pub fn store_tests(ns: &NamespaceConfig, rule: &FoundRule, homes: &[&TestHome], tests: &RuleTests) -> Result<Vec<String>> {
    let mut wrote: Vec<String> = Vec::new();
    for home in homes.iter().copied() {
        match home {
            TestHome::Toml { file, .. } => {
                if crate::domain::set_rule_tests(file, &rule.domain, &rule.id, tests)? {
                    wrote.push(home.label());
                }
            }
            TestHome::Graph { cwd, iri, .. } => {
                let p = &ns.prefix;
                let inserts: String = crate::domain::rules::test_literals(&tests.fires_on, &tests.quiet_on)
                    .iter()
                    .map(|(pred, v)| format!("<{iri}> {p}:{pred} \"{}\" .\n", crud::escape_sparql_literal(v)))
                    .collect();
                let mut sparql = format!(
                    "{}\nDELETE {{ GRAPH ?g {{ <{iri}> ?tp ?tv }} }}\n\
                     WHERE {{ GRAPH ?g {{ <{iri}> ?tp ?tv . FILTER(?tp IN ({p}:firesOn, {p}:quietOn)) }} }}",
                    crud::prefixes(ns)
                );
                if !inserts.is_empty() {
                    sparql.push_str(&format!(
                        " ;\nINSERT {{ GRAPH ?g {{\n{inserts}}} }}\nWHERE {{ GRAPH ?g {{ <{iri}> {p}:ruleText ?text }} }}"
                    ));
                }
                let (store, trig_path, _lock) = crud::lock_and_load(cwd)?;
                let ask = format!("{}\nASK {{ GRAPH ?g {{ <{iri}> {p}:ruleText ?text }} }}", crud::prefixes(ns));
                if !matches!(crate::store::query(&store, &ask)?, QueryResults::Boolean(true)) {
                    continue;
                }
                // Wide, as `remove` takes it: `GRAPH ?g` names no single target graph.
                crate::store::update_and_write(
                    &store,
                    &trig_path,
                    &sparql,
                    crate::store::Scope::Wide,
                    crate::store::Intent::Knowledge,
                )?;
                wrote.push(home.label());
            }
        }
    }
    Ok(wrote)
}
