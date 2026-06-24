use std::path::Path;

use anyhow::Result;
use oxigraph::sparql::QueryResults;

use crate::config::NamespaceConfig;
use crate::crud;

/// Add a rule to a domain in the graph, optionally with a rationale (Phase 26).
pub fn add(
    cwd: &Path,
    ns: &NamespaceConfig,
    domain_name: &str,
    rule_text: &str,
    rationale: Option<&str>,
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
    let _ = crud::load_and_mutate(cwd, ns, &ensure_domain);

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

    crud::load_and_mutate(cwd, ns, &sparql)?;
    Ok(next_index)
}

/// List rules for a domain from the graph.
pub fn list(cwd: &Path, ns: &NamespaceConfig, domain_name: &str) -> Result<()> {
    let p = &ns.prefix;
    let domain_slug = crud::slugify(domain_name);
    let domain_iri = crud::build_iri(ns, "domain", &domain_slug);

    let sparql = format!(
        "SELECT ?text ?pri WHERE {{\n\
           GRAPH ?g {{\n\
             <{domain_iri}> {p}:hasRule ?rule .\n\
             ?rule {p}:ruleText ?text .\n\
             OPTIONAL {{ ?rule {p}:priority ?pri }}\n\
           }}\n\
         }}\n\
         ORDER BY ?pri"
    );

    let results = crud::load_and_query(cwd, ns, &sparql)?;
    if let QueryResults::Solutions(solutions) = results {
        let rules: Vec<(String, String)> = solutions
            .filter_map(|r| r.ok())
            .filter_map(|row| {
                let text = row.get("text").map(|t| crud::term_display(t.into()))?;
                let pri = row.get("pri").map(|t| crud::term_display(t.into())).unwrap_or_default();
                Some((pri, text))
            })
            .collect();

        if rules.is_empty() {
            println!("No rules for domain '{domain_name}'.");
            return Ok(());
        }

        println!("[{domain_name}] {} rules:", rules.len());
        for (pri, text) in &rules {
            println!("  {pri}. {text}");
        }
    }
    Ok(())
}

/// Remove a rule by index from a domain.
pub fn remove(cwd: &Path, ns: &NamespaceConfig, domain_name: &str, index: u32) -> Result<()> {
    let p = &ns.prefix;
    let domain_slug = crud::slugify(domain_name);
    let domain_iri = crud::build_iri(ns, "domain", &domain_slug);
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);

    // Match by {p}:index predicate, not constructed IRI — CLI rules live at
    // rule/{slug}/cli-N and are the only rules carrying {p}:index. Synced
    // rules are managed by editing domains.toml (sync GC handles them).
    let sparql = format!(
        "DELETE {{\n\
           GRAPH <{graph}> {{\n\
             ?rule ?rp ?ro .\n\
             <{domain_iri}> {p}:hasRule ?rule .\n\
           }}\n\
         }}\n\
         WHERE {{\n\
           GRAPH <{graph}> {{\n\
             <{domain_iri}> {p}:hasRule ?rule .\n\
             ?rule {p}:index \"{index}\" ;\n\
               ?rp ?ro .\n\
           }}\n\
         }}"
    );

    crud::load_and_mutate(cwd, ns, &sparql)?;
    Ok(())
}

/// Find the next available rule index for a domain.
/// Uses MAX(index) + 1 to avoid collisions after deletions.
fn next_rule_index(cwd: &Path, ns: &NamespaceConfig, domain_iri: &str) -> Result<u32> {
    let p = &ns.prefix;
    let sparql = format!(
        "SELECT (MAX(?idx) AS ?max_idx) WHERE {{\n\
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
