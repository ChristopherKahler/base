pub mod ast_map;
pub mod ast_query;
pub mod decision;
pub mod entity;
pub mod goal;
pub mod handoff;
pub mod milestone;
pub mod note;
pub mod project;
pub mod rule;
pub mod reminder;
pub mod task;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use oxigraph::sparql::QueryResults;
use oxigraph::store::Store;

use crate::config::NamespaceConfig;

// ─── IRI building ────────────────────────────────────────────

/// Build an entity IRI: `{ns.uri}{entity_type}/{slug}`.
pub fn build_iri(ns: &NamespaceConfig, entity_type: &str, slug: &str) -> String {
    format!("{}{}/{}", ns.uri, entity_type, slug)
}

/// Build the workspace graph IRI: `{ns.uri}graph/ws/{workspace_slug}`.
pub fn workspace_graph_iri(ns: &NamespaceConfig, workspace_slug: &str) -> String {
    format!("{}graph/ws/{}", ns.uri, workspace_slug)
}

/// Derive workspace slug from the workspace root directory name.
pub fn workspace_slug(cwd: &Path) -> String {
    if let Some(base_dir) = crate::config::find_workspace_base(cwd) {
        base_dir
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(slugify)
            .unwrap_or_else(|| "default".into())
    } else {
        cwd.file_name()
            .and_then(|n| n.to_str())
            .map(slugify)
            .unwrap_or_else(|| "default".into())
    }
}

/// Escape a string for safe interpolation into a SPARQL literal.
/// Handles backslashes, double quotes, carriage returns, newlines, and tabs.
pub fn escape_sparql_literal(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', "")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}

/// Convert a name to a URL-safe slug: lowercase, non-alphanumeric→hyphens, deduped.
pub fn slugify(name: &str) -> String {
    let full: String = name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if full.len() <= 80 {
        return full;
    }
    let truncated = &full[..80];
    match truncated.rfind('-') {
        Some(pos) if pos > 20 => truncated[..pos].to_string(),
        _ => truncated.to_string(),
    }
}

// ─── SPARQL helpers ──────────────────────────────────────────

/// Standard PREFIX block for SPARQL operations.
pub fn prefixes(ns: &NamespaceConfig) -> String {
    format!(
        "PREFIX {p}: <{u}>\n\
         PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\n\
         PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>",
        p = ns.prefix,
        u = ns.uri
    )
}

/// Build a DELETE+INSERT operation for a single field.
/// Handles the case where the field doesn't exist yet (OPTIONAL).
pub fn field_update(
    graph_iri: &str,
    subject_iri: &str,
    pred: &str,
    new_value: &str,
) -> String {
    format!(
        "DELETE {{ GRAPH <{g}> {{ <{s}> {pred} ?old }} }}\n\
         INSERT {{ GRAPH <{g}> {{ <{s}> {pred} {val} }} }}\n\
         WHERE {{\n\
           GRAPH <{g}> {{ <{s}> a ?type }}\n\
           OPTIONAL {{ GRAPH <{g}> {{ <{s}> {pred} ?old }} }}\n\
         }}",
        g = graph_iri,
        s = subject_iri,
        val = new_value,
    )
}

/// Current timestamp as ISO 8601 with timezone.
pub fn now_iso() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

// ─── Graph pipeline ──────────────────────────────────────────

/// Find or create the workspace `.base/` directory.
fn find_or_create_base(cwd: &Path) -> Result<PathBuf> {
    if let Some(base) = crate::config::find_workspace_base(cwd) {
        Ok(base)
    } else {
        let base = cwd.join(".base");
        std::fs::create_dir_all(&base).context("creating .base/ directory")?;
        Ok(base)
    }
}

/// Load the workspace store. Creates .base/ and an empty store if graph.nq doesn't exist.
pub fn load_workspace_store(cwd: &Path) -> Result<(Store, PathBuf)> {
    let base_dir = find_or_create_base(cwd)?;
    let trig_path = base_dir.join("graph.nq");

    let store = if trig_path.exists() {
        crate::store::load_graph(&trig_path)?
    } else {
        Store::new().context("creating empty store")?
    };

    Ok((store, trig_path))
}

/// Load store, execute SPARQL UPDATE, write back atomically.
pub fn load_and_mutate(cwd: &Path, ns: &NamespaceConfig, sparql: &str) -> Result<()> {
    let (store, trig_path) = load_workspace_store(cwd)?;
    let full_sparql = format!("{}\n{}", prefixes(ns), sparql);
    store
        .update(&full_sparql)
        .with_context(|| format!("SPARQL update failed: {full_sparql}"))?;
    crate::store::write_back(&store, &trig_path)
}

/// Load workspace graph and run a SPARQL SELECT query.
pub fn load_and_query(cwd: &Path, ns: &NamespaceConfig, sparql: &str) -> Result<QueryResults> {
    let base_dir = crate::config::find_workspace_base(cwd)
        .context("no .base/ directory found. Use --global for global rules, or run `base scaffold` to create a workspace.")?;
    let trig_path = base_dir.join("graph.nq");
    let store = crate::store::load_graph(&trig_path)?;
    let full_sparql = format!("{}\n{}", prefixes(ns), sparql);
    crate::store::query(&store, &full_sparql)
}

// ─── Name resolution ────────────────────────────────────────

/// Capitalize the first character of a string (for RDF class names).
fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
    }
}

/// Resolve a user-provided identifier (slug, display name, or mixed case) to a canonical slug.
/// Tries: 1) exact match as slug, 2) slugify the input, 3) SPARQL lookup by display name.
/// Loads the graph once and runs all checks against it.
pub fn resolve_slug(cwd: &Path, ns: &NamespaceConfig, entity_type: &str, input: &str) -> Result<String> {
    let base_dir = crate::config::find_workspace_base(cwd)
        .context("no .base/ directory found. Use --global for global rules, or run `base scaffold` to create a workspace.")?;
    let trig_path = base_dir.join("graph.nq");
    let store = crate::store::load_graph(&trig_path)?;
    let pfx = prefixes(ns);
    let p = &ns.prefix;
    let type_name = capitalize_first(entity_type);

    // Try 1: Input as-is is already a valid slug (skip if contains spaces — invalid IRI)
    if !input.contains(' ') {
        let iri = build_iri(ns, entity_type, input);
        let ask = format!("{pfx}\nASK WHERE {{ GRAPH ?g {{ <{iri}> a {p}:{type_name} }} }}");
        if let Ok(QueryResults::Boolean(true)) = crate::store::query(&store, &ask) {
            return Ok(input.to_string());
        }
    }

    // Try 2: Slugify the input and check
    let slugified = slugify(input);
    if slugified != input {
        let iri2 = build_iri(ns, entity_type, &slugified);
        let ask2 = format!("{pfx}\nASK WHERE {{ GRAPH ?g {{ <{iri2}> a {p}:{type_name} }} }}");
        if let Ok(QueryResults::Boolean(true)) = crate::store::query(&store, &ask2) {
            return Ok(slugified);
        }
    }

    // Try 3: Name lookup (case-insensitive)
    let escaped = input.replace('"', "\\\"");
    let sel = format!(
        "{pfx}\nSELECT ?iri WHERE {{\n\
           GRAPH ?g {{\n\
             ?iri a {p}:{type_name} ;\n\
               {p}:name ?name .\n\
             FILTER(LCASE(?name) = LCASE(\"{escaped}\"))\n\
           }}\n\
         }} LIMIT 1"
    );
    if let QueryResults::Solutions(solutions) = crate::store::query(&store, &sel)? {
        for row in solutions.filter_map(|r| r.ok()) {
            if let Some(term) = row.get("iri") {
                let display = term_display(term.into());
                return Ok(display);
            }
        }
    }

    anyhow::bail!(
        "No {entity_type} found matching '{input}'. Try the slug (e.g., 'my-project') or display name (e.g., 'My Project')."
    )
}

// ─── Display helpers ─────────────────────────────────────────

/// Extract a human-readable string from an RDF term.
pub fn term_display(term: oxigraph::model::TermRef<'_>) -> String {
    use oxigraph::model::TermRef;
    match term {
        TermRef::Literal(l) => l.value().to_string(),
        TermRef::NamedNode(n) => {
            let iri = n.as_str();
            iri.rfind('#')
                .or_else(|| iri.rfind('/'))
                .map(|pos| iri[pos + 1..].to_string())
                .unwrap_or_else(|| iri.to_string())
        }
        TermRef::BlankNode(b) => format!("_:{}", b.as_str()),
        #[allow(unreachable_patterns)]
        _ => term.to_string(),
    }
}

/// Backfill missing relationship edges for existing entities.
/// Parses entity slugs to infer parent relationships and creates edges where missing.
pub fn repair_edges(cwd: &Path, ns: &NamespaceConfig) -> Result<usize> {
    let (store, trig_path) = load_workspace_store(cwd)?;
    let ws_slug = workspace_slug(cwd);
    let graph = workspace_graph_iri(ns, &ws_slug);
    let p = &ns.prefix;
    let pfx = prefixes(ns);
    let mut count = 0;

    // 1. Decisions → domain edges (slug format: {domain}.{decision})
    count += repair_entity_edges(
        &store, &pfx, &graph, ns, p,
        "Decision", "domain", "hasDecision",
    )?;

    // 2. Milestones → project edges (slug format: {project}.{milestone})
    count += repair_entity_edges(
        &store, &pfx, &graph, ns, p,
        "Milestone", "project", "hasMilestone",
    )?;

    // 3. Tasks → project edges (slug format: {project}.{task})
    count += repair_entity_edges(
        &store, &pfx, &graph, ns, p,
        "Task", "project", "hasTask",
    )?;

    crate::store::write_back(&store, &trig_path)?;
    Ok(count)
}

#[allow(clippy::too_many_arguments)] // internal helper, params are query fragments
fn repair_entity_edges(
    store: &Store,
    pfx: &str,
    graph: &str,
    ns: &NamespaceConfig,
    p: &str,
    type_name: &str,
    parent_type: &str,
    predicate: &str,
) -> Result<usize> {
    // Find all entities of this type (check both named graph and any graph)
    let sparql = format!(
        "{pfx}\nSELECT ?s WHERE {{ {{ GRAPH <{graph}> {{ ?s rdf:type {p}:{type_name} }} }} UNION {{ GRAPH ?g {{ ?s rdf:type {p}:{type_name} }} }} }}"
    );

    let mut edges_added = 0;

    if let Ok(QueryResults::Solutions(solutions)) = store.query(&sparql) {
        let iris: Vec<String> = solutions
            .filter_map(|r| r.ok())
            .filter_map(|row| row.get("s").map(|t| {
                match t.into() {
                    oxigraph::model::TermRef::NamedNode(n) => n.as_str().to_string(),
                    other => term_display(other),
                }
            }))
            .collect();

        for iri in &iris {
            // Extract slug from IRI (everything after the last /)
            let slug = iri.rsplit('/').next().unwrap_or("");

            // Parse parent slug from dot notation (first segment before the dot)
            let parent_slug = match slug.split_once('.') {
                Some((parent, _)) => parent,
                None => continue, // No dot = can't determine parent
            };

            let parent_iri = build_iri(ns, parent_type, parent_slug);

            // Check if edge already exists (any graph)
            let check = format!(
                "{pfx}\nASK WHERE {{ GRAPH ?g {{ <{parent_iri}> {p}:{predicate} <{iri}> }} }}"
            );

            if let Ok(QueryResults::Boolean(true)) = store.query(&check) {
                continue; // Edge exists
            }

            // Check parent entity exists (any graph)
            let parent_check = format!(
                "{pfx}\nASK WHERE {{ GRAPH ?g {{ <{parent_iri}> a ?type }} }}"
            );

            if let Ok(QueryResults::Boolean(false)) | Err(_) = store.query(&parent_check) {
                continue; // Parent doesn't exist
            }

            // Create edge
            let insert = format!(
                "{pfx}\nINSERT DATA {{ GRAPH <{graph}> {{ <{parent_iri}> {p}:{predicate} <{iri}> }} }}"
            );

            if store.update(&insert).is_ok() {
                edges_added += 1;
                let short_slug = slug.split('.').next_back().unwrap_or(slug);
                println!("  + {parent_slug} → {predicate} → {short_slug}");
            }
        }
    }

    Ok(edges_added)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_works() {
        assert_eq!(slugify("My Cool Project"), "my-cool-project");
        assert_eq!(slugify("CaseGate v2"), "casegate-v2");
        assert_eq!(slugify("hello--world"), "hello-world");
        assert_eq!(slugify("  spaced  "), "spaced");
    }

    #[test]
    fn build_iri_follows_scheme() {
        let ns = NamespaceConfig::default();
        assert_eq!(
            build_iri(&ns, "project", "casegate-v2"),
            "http://ops-sys.local/ontology#project/casegate-v2"
        );
    }

    #[test]
    fn workspace_graph_iri_correct() {
        let ns = NamespaceConfig::default();
        assert_eq!(
            workspace_graph_iri(&ns, "chris-ai-systems"),
            "http://ops-sys.local/ontology#graph/ws/chris-ai-systems"
        );
    }
}
