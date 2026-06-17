use std::path::Path;

use oxigraph::model::TermRef;

use crate::config::BaseConfig;
use crate::crud;
use crate::domain;
use crate::domain::session::{rules_hash, SessionState};

/// Query a domain's rules and 1-hop neighborhood from the graph.
/// Returns (rules_text, neighborhood_text). Falls back to TOML if graph query fails.
pub fn query_domain_from_graph(
    store: &oxigraph::store::Store,
    config: &BaseConfig,
    domain_def: &domain::DomainDef,
) -> (String, String) {
    let ns = &config.namespace;
    let p = &ns.prefix;
    let domain_slug = crud::slugify(&domain_def.name);
    let domain_iri = crud::build_iri(ns, "domain", &domain_slug);
    let pfx = crud::prefixes(ns);

    // Query 1: Get rules ordered by priority
    let rules_sparql = format!(
        "{pfx}\n\
         SELECT ?text WHERE {{\n\
           GRAPH ?g {{\n\
             <{domain_iri}> {p}:hasRule ?rule .\n\
             ?rule {p}:ruleText ?text .\n\
             OPTIONAL {{ ?rule {p}:priority ?pri }}\n\
           }}\n\
         }}\n\
         ORDER BY ?pri"
    );

    let rules_text = match crate::store::query(store, &rules_sparql) {
        Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) => {
            let rules: Vec<String> = solutions
                .filter_map(|r| r.ok())
                .filter_map(|row| {
                    row.get("text").map(|t| match t.into() {
                        TermRef::Literal(l) => l.value().to_string(),
                        _ => String::new(),
                    })
                })
                .filter(|s| !s.is_empty())
                .collect();

            if rules.is_empty() {
                format_toml_rules(domain_def)
            } else {
                let mut out = format!("[DOMAIN: {}]\n", domain_def.name);
                for (i, rule) in rules.iter().enumerate() {
                    out.push_str(&format!("  {i}. {rule}\n"));
                }
                out
            }
        }
        _ => format_toml_rules(domain_def),
    };

    // Query 2: 1-hop neighborhood (decisions linked to this domain, projects with hasDomain)
    let neighborhood_sparql = format!(
        "{pfx}\n\
         SELECT ?name ?type WHERE {{\n\
           GRAPH ?g {{\n\
             {{\n\
               <{domain_iri}> {p}:hasDecision ?related .\n\
               ?related {p}:name ?name .\n\
               BIND({p}:Decision AS ?type)\n\
             }} UNION {{\n\
               ?related {p}:hasDomain <{domain_iri}> ;\n\
                 a {p}:Project ;\n\
                 {p}:name ?name .\n\
               BIND({p}:Project AS ?type)\n\
             }}\n\
           }}\n\
         }}\n\
         ORDER BY ?type ?name"
    );

    let neighborhood_text = match crate::store::query(store, &neighborhood_sparql) {
        Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) => {
            let neighbors: Vec<(String, String)> = solutions
                .filter_map(|r| r.ok())
                .filter_map(|row| {
                    let name = row.get("name").map(|t| match t.into() {
                        TermRef::Literal(l) => l.value().to_string(),
                        _ => String::new(),
                    })?;
                    let type_label = row.get("type").map(|t| crud::term_display(t.into()))?;
                    if name.is_empty() {
                        None
                    } else {
                        Some((type_label, name))
                    }
                })
                .collect();

            if neighbors.is_empty() {
                String::new()
            } else {
                let mut out = format!("[{} CONTEXT]\n", domain_def.name);
                for (type_label, name) in &neighbors {
                    out.push_str(&format!("  - {type_label}: {name}\n"));
                }
                out
            }
        }
        _ => String::new(),
    };

    (rules_text, neighborhood_text)
}

/// Resolve a query name to a `.sparql` file, read it, run it, format results.
/// Resolution: workspace `.base/queries/{name}.sparql` → global `~/.base-gbl/queries/{name}.sparql`.
pub fn resolve_and_run_query(
    store: &oxigraph::store::Store,
    config: &BaseConfig,
    cwd: &Path,
    query_name: &str,
    format: &str,
    domain_name: &str,
) -> String {
    let filename = format!("{query_name}.sparql");

    // Tier 1: workspace
    let sparql_content = crate::config::find_workspace_base(cwd)
        .and_then(|base| std::fs::read_to_string(base.join("queries").join(&filename)).ok())
        // Tier 2: global
        .or_else(|| {
            dirs::home_dir().and_then(|home| {
                std::fs::read_to_string(home.join(".base-gbl").join("queries").join(&filename)).ok()
            })
        });

    let sparql_raw = match sparql_content {
        Some(s) => s,
        None => {
            eprintln!("base: query file not found: queries/{filename}");
            return String::new();
        }
    };

    // Prefix substitution (same pattern as queries.toml)
    let p = &config.namespace.prefix;
    let u = &config.namespace.uri;
    let sparql = sparql_raw
        .replace("{{prefix}}", p)
        .replace("{{uri}}", u);

    match crate::store::query(store, &sparql) {
        Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) => {
            let rows: Vec<_> = solutions.filter_map(|r| r.ok()).collect();
            if rows.is_empty() {
                return String::new();
            }

            let mut out = format!("<base-query name=\"{query_name}\" domain=\"{domain_name}\">\n");

            let known_vars = ["label", "name", "text", "detail", "type", "status", "value", "count", "created"];

            match format {
                "table" => {
                    if let Some(first) = rows.first() {
                        let vars: Vec<&str> = known_vars.iter()
                            .filter(|v| first.get(**v).is_some())
                            .copied()
                            .collect();

                        if !vars.is_empty() {
                            out.push_str(&format!("| {} |\n", vars.join(" | ")));
                            out.push_str(&format!("|{}|\n", vars.iter().map(|_| "---").collect::<Vec<_>>().join("|")));
                            for row in &rows {
                                let vals: Vec<String> = vars.iter()
                                    .map(|v| row.get(*v).map(|t| crud::term_display(t.into())).unwrap_or_default())
                                    .collect();
                                out.push_str(&format!("| {} |\n", vals.join(" | ")));
                            }
                        }
                    }
                }
                "prose" => {
                    for row in &rows {
                        for var in &known_vars[..7] {
                            if let Some(term) = row.get(*var) {
                                let val = crud::term_display(term.into());
                                if !val.is_empty() {
                                    out.push_str(&format!("{var}: {val}\n"));
                                }
                            }
                        }
                        out.push('\n');
                    }
                }
                _ => {
                    // Default: "list"
                    for row in &rows {
                        let primary = row.get("label")
                            .or_else(|| row.get("name"))
                            .or_else(|| row.get("text"))
                            .map(|t| crud::term_display(t.into()))
                            .unwrap_or_default();
                        let detail = row.get("detail")
                            .or_else(|| row.get("value"))
                            .map(|t| crud::term_display(t.into()));

                        if let Some(d) = detail {
                            out.push_str(&format!("- {primary}: {d}\n"));
                        } else {
                            out.push_str(&format!("- {primary}\n"));
                        }
                    }
                }
            }

            out.push_str("</base-query>\n");
            out
        }
        Ok(_) => String::new(),
        Err(e) => {
            eprintln!("base: query '{query_name}' failed: {e}");
            String::new()
        }
    }
}

/// Format rules directly from the DomainDef struct (TOML fallback).
pub fn format_toml_rules(domain_def: &domain::DomainDef) -> String {
    if domain_def.rules.is_empty() {
        return String::new();
    }
    let mut out = format!("[DOMAIN: {}]\n", domain_def.name);
    for (i, rule) in domain_def.rules.iter().enumerate() {
        out.push_str(&format!("  {i}. {rule}\n"));
    }
    out
}

// ─── CLI: `base context` subcommand ─────────────────────────

/// Pull targeted graph context on demand. Same engine as hook injection.
/// Explicit call BYPASSES dedup but REGISTERS hash for hook-side dedup.
pub fn context_pull(config: &BaseConfig, cwd: &Path, text: &str) {
    let domains = domain::load_domains(cwd);
    if domains.is_empty() {
        return;
    }

    crate::hook::user_prompt_submit::ensure_domain_sync_pub(config, cwd);

    let graph_store = crate::store::load_merged(cwd);

    let matched = domain::matcher::match_domains(text, &domains, &[]);
    if matched.is_empty() {
        return;
    }

    let base_dir = crate::config::find_workspace_base(cwd);
    let mut session = base_dir
        .as_deref()
        .map(SessionState::load)
        .unwrap_or_default();
    let mut session_dirty = false;

    for dm in &matched {
        let domain_def = dm.domain;

        let (rules_text, neighborhood_text) = match &graph_store {
            Some(store) => query_domain_from_graph(store, config, domain_def),
            None => (format_toml_rules(domain_def), String::new()),
        };

        let query_text = match (&graph_store, &domain_def.query) {
            (Some(store), Some(query_name)) => {
                let fmt = domain_def.query_format.as_deref().unwrap_or("list");
                resolve_and_run_query(store, config, cwd, query_name, fmt, &domain_def.name)
            }
            _ => String::new(),
        };

        let mut output = String::new();
        if !rules_text.is_empty() {
            output.push_str(&rules_text);
        }
        if !neighborhood_text.is_empty() {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(&neighborhood_text);
        }
        if !query_text.is_empty() {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(&query_text);
        }

        if !output.is_empty() {
            print!("{output}\n");

            // Register hash so hook-side dedup suppresses same content later
            let combined_hash = {
                let mut lines: Vec<String> = output.lines().map(String::from).collect();
                lines.sort();
                rules_hash(&lines)
            };
            session.mark_injected(&domain_def.name, combined_hash);
            session_dirty = true;
        }
    }

    if session_dirty {
        if let Some(bd) = base_dir.as_deref() {
            let _ = session.save(bd);
        }
    }
}

/// List all available context triggers (domains with keywords or queries).
pub fn context_list(cwd: &Path) {
    let domains = domain::load_domains(cwd);
    if domains.is_empty() {
        println!("No domains configured.");
        return;
    }

    println!("Available context triggers:\n");
    for d in &domains {
        let keywords = if d.prompt_keywords.is_empty() {
            "(no keywords)".to_string()
        } else {
            d.prompt_keywords.join(", ")
        };
        let query_info = match &d.query {
            Some(q) => format!(" → {q}"),
            None => " (rules only)".to_string(),
        };
        println!("  {}: {}{}", d.name, keywords, query_info);
    }
}

/// Compact cheat-sheet for SessionStart injection. Shows query-bearing
/// and keyword-triggered domains so Claude knows what's pullable.
pub fn context_triggers_block(cwd: &Path) -> String {
    let domains = domain::load_domains(cwd);
    let triggerable: Vec<_> = domains
        .iter()
        .filter(|d| !d.prompt_keywords.is_empty() || d.query.is_some())
        .collect();

    if triggerable.is_empty() {
        return String::new();
    }

    let mut out = String::from(
        "<base-context-triggers>\n\
         Run `base context <keyword>` to pull targeted graph context on demand:\n",
    );
    for d in &triggerable {
        let kws = if d.prompt_keywords.is_empty() {
            "(path-only)".to_string()
        } else {
            d.prompt_keywords
                .iter()
                .take(4)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        };
        let query_suffix = match &d.query {
            Some(q) => format!(" → {q}"),
            None => String::new(),
        };
        out.push_str(&format!("  {}: {}{}\n", d.name, kws, query_suffix));
    }
    out.push_str("</base-context-triggers>");
    out
}
