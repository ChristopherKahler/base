pub mod ast_map;
pub mod ast_query;
pub mod decision;
pub mod semantic;
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

/// Normalize an OS path separator to the graph's canonical form: forward slash.
///
/// A relative path reaches the graph twice — as a `:path` literal and as the
/// slugified subject IRI. Windows hands it over backslash-separated, which is
/// both an invalid SPARQL string escape and a *different literal* from the one
/// every other platform stores for the same file. [`slugify`] already collapses
/// `/` and `\` to the same `-`, so normalizing rewrites literals without moving
/// a single subject IRI — which is what makes re-sync a replace, not an append.
pub fn normalize_path_sep(s: &str) -> String {
    s.replace('\\', "/")
}

/// Normalize a path's separators, then escape it for a SPARQL literal.
///
/// Order matters: escaping first turns `\` into `\\` and the separator is no
/// longer there to normalize. Use this for every path that becomes a literal —
/// stored or probed — so both sides of a comparison reduce to the same form.
pub fn path_literal(s: &str) -> String {
    escape_sparql_literal(&normalize_path_sep(s))
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
    // Truncate to at most 80 bytes, backing off to a UTF-8 char boundary so a
    // multi-byte char (e.g. 'μ') straddling byte 80 doesn't panic the slice.
    let mut cut = 80;
    while cut > 0 && !full.is_char_boundary(cut) {
        cut -= 1;
    }
    let truncated = &full[..cut];
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
///
/// The DELETE is graph-AGNOSTIC (`GRAPH ?gg`): it removes the old value wherever it
/// is stamped, then INSERTs the canonical value into `graph_iri`. Scoping the DELETE
/// to a single graph left a stale value stamped under a *different* workspace graph
/// behind, so the new value appended instead of replacing it — the `base project
/// repath` append bug after a cross-workspace move (the moved entity's prior field
/// lived under the old `graph/ws/<name>` stamp, which the single-graph DELETE missed).
pub fn field_update(
    graph_iri: &str,
    subject_iri: &str,
    pred: &str,
    new_value: &str,
) -> String {
    format!(
        "DELETE {{ GRAPH ?gg {{ <{s}> {pred} ?old }} }}\n\
         INSERT {{ GRAPH <{g}> {{ <{s}> {pred} {val} }} }}\n\
         WHERE {{\n\
           GRAPH <{g}> {{ <{s}> a ?type }}\n\
           OPTIONAL {{ GRAPH ?gg {{ <{s}> {pred} ?old }} }}\n\
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

/// Find the workspace `.base/` directory for a tier-bound write.
///
/// Never auto-creates (issue #8): outside a workspace the data would land in
/// a stray `.base/` that no later session resolves, so a success message here
/// is indistinguishable from data loss. Fail loudly and name both escape
/// hatches. Global-tier writers opt in by passing `~/.base-gbl` as cwd.
fn require_base_for_write(cwd: &Path) -> Result<PathBuf> {
    crate::config::find_workspace_base(cwd).context(
        "no .base/ directory found — refusing to write outside a workspace. \
         Use --global (-g) for the global tier, or run `base scaffold` to create a workspace.",
    )
}

/// Load the workspace store. Requires an existing .base/; creates an empty
/// store only when graph.nq itself doesn't exist yet.
pub fn load_workspace_store(cwd: &Path) -> Result<(Store, PathBuf)> {
    let base_dir = require_base_for_write(cwd)?;
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
    // Scope::Target, not Wide: this is the hot path — 40 CRUD callers, and the
    // hooks behind them fire on every tool call. Its SPARQL always names its
    // graph, so the target is derivable and a whole-store diff is never needed.
    crate::store::update_and_write(
        &store,
        &trig_path,
        &full_sparql,
        crate::store::Scope::Target,
        crate::store::Intent::Knowledge,
    )
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

/// Extract the bare entity slug from an IRI display form (`task/foo.bar` → `foo.bar`,
/// `project/foo` → `foo`). Safe on plain slugs (returns them unchanged).
pub fn slug_of(display: &str) -> String {
    display.rsplit('/').next().unwrap_or(display).to_string()
}

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
    // Each repair applies its own INSERT DATA; collect them so the change log
    // carries the actual delta rather than an opaque "a repair happened".
    let mut applied: Vec<String> = Vec::new();

    // 1. Decisions → domain edges (slug format: {domain}.{decision})
    applied.extend(repair_entity_edges(
        &store, &pfx, &graph, ns, p,
        "Decision", "domain", "hasDecision",
    )?);

    // 2. Milestones → project edges (slug format: {project}.{milestone})
    applied.extend(repair_entity_edges(
        &store, &pfx, &graph, ns, p,
        "Milestone", "project", "hasMilestone",
    )?);

    // 3. Tasks → project edges (slug format: {project}.{task})
    applied.extend(repair_entity_edges(
        &store, &pfx, &graph, ns, p,
        "Task", "project", "hasTask",
    )?);

    crate::store::update_and_write(
        &store,
        &trig_path,
        &applied.join(";\n"),
        crate::store::Scope::Target,
        crate::store::Intent::Knowledge,
    )?;
    Ok(applied.len())
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
) -> Result<Vec<String>> {
    // Find all entities of this type (check both named graph and any graph)
    let sparql = format!(
        "{pfx}\nSELECT ?s WHERE {{ {{ GRAPH <{graph}> {{ ?s rdf:type {p}:{type_name} }} }} UNION {{ GRAPH ?g {{ ?s rdf:type {p}:{type_name} }} }} }}"
    );

    let mut applied: Vec<String> = Vec::new();

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
                applied.push(insert);
                let short_slug = slug.split('.').next_back().unwrap_or(slug);
                println!("  + {parent_slug} → {predicate} → {short_slug}");
            }
        }
    }

    Ok(applied)
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
    fn slugify_does_not_panic_on_multibyte_at_truncation_boundary() {
        // Regression: a 2-byte char ('μ') straddling byte 80 used to panic the
        // `&full[..80]` slice ("not a char boundary"). See the fractal-alternate
        // journal concept "...participation measure μ".
        let label = "x".repeat(79) + "μ"; // slug = 79 bytes + μ (bytes 79..81)
        let s = slugify(&label);
        assert!(s.len() <= 80);
        assert!(s.chars().all(|c| c.is_alphanumeric() || c == '-'));
        // and the real-world label that crashed
        let real = "market depth L2 participation measure μ ".repeat(3);
        let _ = slugify(&real); // must not panic
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

    #[test]
    fn field_update_replaces_value_across_named_graphs() {
        // Regression (repath append bug): a field must be REPLACED even when a stale
        // value is stamped under a DIFFERENT workspace graph (the cross-workspace-move
        // case). Before the graph-agnostic DELETE the stale value survived and the new
        // value appended, leaving two values.
        let ns = NamespaceConfig::default();
        let p = &ns.prefix;
        let store = oxigraph::store::Store::new().unwrap();
        let g_a = workspace_graph_iri(&ns, "alpha");
        let g_b = workspace_graph_iri(&ns, "beta");
        let s = build_iri(&ns, "project", "p");

        // Seed: entity + path "alpha" in graph A; a STALE path in graph B.
        let seed = format!(
            "INSERT DATA {{ GRAPH <{g_a}> {{ <{s}> a {p}:Project ; {p}:path \"alpha\" }} \
                            GRAPH <{g_b}> {{ <{s}> {p}:path \"beta-stale\" }} }}"
        );
        store.update(&format!("{}\n{}", prefixes(&ns), seed)).unwrap();

        // Replace path → "new" (canonical graph = A).
        let upd = field_update(&g_a, &s, &format!("{p}:path"), "\"new\"");
        store.update(&format!("{}\n{}", prefixes(&ns), upd)).unwrap();

        // Exactly one path value remains across ALL graphs: "new".
        let q = format!(
            "{}\nSELECT ?v WHERE {{ GRAPH ?g {{ <{s}> {p}:path ?v }} }}",
            prefixes(&ns)
        );
        let mut vals: Vec<String> = Vec::new();
        if let oxigraph::sparql::QueryResults::Solutions(sols) = store.query(&q).unwrap() {
            for r in sols.filter_map(|x| x.ok()) {
                if let Some(t) = r.get("v") {
                    vals.push(term_display(t.into()));
                }
            }
        }
        assert_eq!(
            vals,
            vec!["new".to_string()],
            "stale cross-graph value removed; single canonical value remains"
        );
    }

    // Issue #8: a tier-bound write outside a workspace must fail loudly and
    // leave nothing behind. Silent success with a discarded write is
    // indistinguishable from data loss.
    #[test]
    fn write_outside_workspace_fails_and_creates_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let ns = NamespaceConfig::default();

        let err = load_and_mutate(tmp.path(), &ns, "INSERT DATA { GRAPH <urn:g> { <urn:s> <urn:p> \"v\" } }")
            .expect_err("write outside a workspace must not succeed");
        let msg = format!("{err:#}");
        assert!(msg.contains("refusing to write outside a workspace"), "got: {msg}");
        assert!(msg.contains("--global"), "must name the global escape hatch: {msg}");
        assert!(msg.contains("base scaffold"), "must name the scaffold escape hatch: {msg}");

        assert!(!tmp.path().join(".base").exists(), "no stray .base/ may be created");
    }
}
