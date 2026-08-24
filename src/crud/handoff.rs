use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use oxigraph::sparql::QueryResults;
use oxigraph::store::Store;

use crate::config::NamespaceConfig;
use crate::changelog::Change;
use crate::crud;

/// Resolve the target graph file + graph IRI for a write.
///
/// Workspace tier, always. The old global catchall (issue #8) meant a handoff
/// created outside a workspace landed in `~/.base-gbl` and then resurfaced at
/// the start of every unrelated project, forever. Global is now something you
/// opt into with `-g` — which routes cwd to `~/.base-gbl`, so this resolves it
/// as that tier's workspace rather than as a silent fallback.
fn write_tier(cwd: &Path, ns: &NamespaceConfig) -> Result<(PathBuf, String)> {
    let base = crate::config::find_workspace_base(cwd).context(
        "no .base/ directory found — refusing to write outside a workspace. \
         Use --global (-g) to file this against the global tier deliberately, \
         or run `base scaffold` here first.",
    )?;
    let ws_slug = crud::workspace_slug(cwd);
    Ok((base.join("graph.nq"), crud::workspace_graph_iri(ns, &ws_slug)))
}

/// Every existing graph file across tiers — used for tier-agnostic mutations
/// (snooze/archive) so a handoff is updated wherever it lives.
///
/// Takes `gbl_root` rather than resolving the home directory itself. This
/// function is why the fork exists: reaching for the home directory here meant
/// every test that archived a fixture handoff rewrote the operator's real
/// global graph. As a parameter the compiler will not let a caller — test or
/// otherwise — forget to say which root it means.
fn all_tier_files(gbl_root: Option<&Path>, cwd: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Some(home) = gbl_root {
        let gbl = home.join(".base-gbl").join(".base").join("graph.nq");
        if gbl.exists() {
            files.push(gbl);
        }
    }
    if let Some(base) = crate::config::find_workspace_base(cwd) {
        let ws = base.join("graph.nq");
        if ws.exists() {
            files.push(ws);
        }
    }
    files
}

/// Load one graph file, run a SPARQL UPDATE, write back atomically.
fn mutate_file(path: &Path, ns: &NamespaceConfig, sparql: &str) -> Result<()> {
    let store = if path.exists() {
        crate::store::load_graph(path)?
    } else {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("creating tier .base/ directory")?;
        }
        Store::new().context("creating empty store")?
    };
    let full = format!("{}\n{}", crud::prefixes(ns), sparql);
    store
        .update(&full)
        .with_context(|| format!("handoff update failed: {full}"))?;
    crate::store::write_back(&store, path, Change::Sparql(&full))
}

/// Derive a flow-doc slug from its doc path basename (no extension).
/// `/abs/path/FORK-COMMAND-SPEC.md` → `FORK-COMMAND-SPEC`. Used VERBATIM (no
/// slugify/lowercase) so the doc filename and the graph slug are the SAME string
/// — that single name is the title a handoff/fork is summoned by (doc==slug protocol).
fn doc_basename_slug(doc_path: &str) -> Result<String> {
    Path::new(doc_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .with_context(|| format!("could not derive a slug from doc path '{doc_path}'"))
}

/// Resolve the slug for a flow-doc: an explicit `--slug` override (verbatim) when
/// given and non-blank, else the doc basename. THE STANDARD for both handoff and
/// fork: doc filename and graph slug always align, so the operator summons the
/// next session by one consistent name.
fn resolve_doc_slug(slug: Option<&str>, doc_path: &str) -> Result<String> {
    match slug {
        Some(s) if !s.trim().is_empty() => Ok(s.trim().to_string()),
        _ => doc_basename_slug(doc_path),
    }
}

/// Register a handoff pointing at a resume document. Archives any prior OPEN
/// continuity handoff for the same project (one open handoff per project), then
/// inserts the new one with `resurfaceAt = now` so it surfaces next session start.
/// Slug defaults to the doc basename (doc==slug protocol); pass `slug` to override.
/// Re-registering the same slug re-points it idempotently (no duplicate triples).
pub fn create(
    cwd: &Path,
    ns: &NamespaceConfig,
    project: &str,
    doc_path: &str,
    slug: Option<&str>,
) -> Result<String> {
    let now = crud::now_iso();
    let slug = resolve_doc_slug(slug, doc_path)?;
    let iri = crud::build_iri(ns, "handoff", &slug);
    let (path, graph) = write_tier(cwd, ns)?;
    let p = &ns.prefix;
    let project = crud::escape_sparql_literal(project);
    let doc = crud::escape_sparql_literal(doc_path);

    // 1. Archive any existing open *continuity* handoff for this project in the
    //    target tier. Forks (kind = "fork") share the Handoff type + project but
    //    are additive side-work — never archive them here.
    let archive_prior = format!(
        "DELETE {{ GRAPH <{graph}> {{ ?h {p}:status \"open\" }} }}\n\
         INSERT {{ GRAPH <{graph}> {{ ?h {p}:status \"archived\" }} }}\n\
         WHERE  {{ GRAPH <{graph}> {{ ?h a {p}:Handoff ; {p}:project \"{project}\" ; {p}:status \"open\" .\n\
           OPTIONAL {{ ?h {p}:kind ?kind }}\n\
           FILTER(!BOUND(?kind) || ?kind != \"fork\") }} }}"
    );

    // 2. Clean any existing node at this exact slug so re-registration re-points
    //    it instead of layering duplicate status/timestamp triples.
    let clean_target = format!(
        "DELETE {{ GRAPH <{graph}> {{ <{iri}> ?dp ?do }} }} WHERE {{ GRAPH <{graph}> {{ <{iri}> ?dp ?do }} }}"
    );

    // 3. Insert the new handoff.
    let insert = format!(
        "INSERT DATA {{ GRAPH <{graph}> {{\n\
           <{iri}> rdf:type {p}:Handoff ;\n\
             {p}:name \"{project}\" ;\n\
             {p}:project \"{project}\" ;\n\
             {p}:handoffDoc \"{doc}\" ;\n\
             {p}:kind \"handoff\" ;\n\
             {p}:status \"open\" ;\n\
             {p}:createdAt \"{now}\"^^xsd:dateTime ;\n\
             {p}:resurfaceAt \"{now}\"^^xsd:dateTime ;\n\
             {p}:lastActive \"{now}\"^^xsd:dateTime .\n\
         }} }}"
    );

    mutate_file(&path, ns, &format!("{archive_prior};\n{clean_target};\n{insert}"))?;
    Ok(slug)
}

/// Register a fork pointing at a build-spec document. Forks are ADDITIVE —
/// creating one does NOT archive sibling forks (contrast `create`, which archives
/// the prior open handoff for the project). Slug defaults to the doc basename
/// (doc==slug protocol), so `handoff/<doc-basename>` is the node IRI; pass `slug`
/// to override. `resurfaceAt = now` so it surfaces next session.
pub fn create_fork(
    cwd: &Path,
    ns: &NamespaceConfig,
    project: &str,
    doc_path: &str,
    slug: Option<&str>,
) -> Result<String> {
    let now = crud::now_iso();
    let slug = resolve_doc_slug(slug, doc_path)?;
    let iri = crud::build_iri(ns, "handoff", &slug);
    let (path, graph) = write_tier(cwd, ns)?;
    let p = &ns.prefix;
    let project = crud::escape_sparql_literal(project);
    let name = crud::escape_sparql_literal(&slug);
    let doc = crud::escape_sparql_literal(doc_path);

    // Additive: no archive-prior. A re-create of the same slug re-points it
    // (idempotent) by deleting any existing node at this IRI first.
    let insert = format!(
        "DELETE {{ GRAPH <{graph}> {{ <{iri}> ?dp ?do }} }} WHERE {{ GRAPH <{graph}> {{ <{iri}> ?dp ?do }} }};\n\
         INSERT DATA {{ GRAPH <{graph}> {{\n\
           <{iri}> rdf:type {p}:Handoff ;\n\
             {p}:name \"{name}\" ;\n\
             {p}:project \"{project}\" ;\n\
             {p}:handoffDoc \"{doc}\" ;\n\
             {p}:kind \"fork\" ;\n\
             {p}:status \"open\" ;\n\
             {p}:createdAt \"{now}\"^^xsd:dateTime ;\n\
             {p}:resurfaceAt \"{now}\"^^xsd:dateTime ;\n\
             {p}:lastActive \"{now}\"^^xsd:dateTime .\n\
         }} }}"
    );

    mutate_file(&path, ns, &insert)?;
    Ok(slug)
}

/// List handoffs (continuity docs only — forks excluded) across both tiers.
pub fn list(cwd: &Path, ns: &NamespaceConfig) -> Result<()> {
    let Some(store) = crate::store::load_merged(cwd) else {
        println!("No handoffs.");
        return Ok(());
    };
    let p = &ns.prefix;
    let sparql = format!(
        "{pfx}\nSELECT ?h ?project ?status ?resurfaceAt WHERE {{\n\
           GRAPH ?g {{\n\
             ?h a {p}:Handoff ;\n\
               {p}:project ?project ;\n\
               {p}:status ?status .\n\
             OPTIONAL {{ ?h {p}:resurfaceAt ?resurfaceAt }}\n\
             OPTIONAL {{ ?h {p}:kind ?kind }}\n\
             FILTER(!BOUND(?kind) || ?kind != \"fork\")\n\
           }}\n\
         }}\n\
         ORDER BY ?status ?project",
        pfx = crud::prefixes(ns)
    );

    if let QueryResults::Solutions(solutions) = crate::store::query(&store, &sparql)? {
        let rows: Vec<Vec<String>> = solutions
            .filter_map(|r| r.ok())
            .map(|row| {
                let get = |k: &str| {
                    row.get(k).map(|t| crud::term_display(t.into())).unwrap_or_default()
                };
                let h = get("h");
                let slug = h.rsplit('/').next().unwrap_or(&h).to_string();
                vec![slug, get("project"), get("status"), get("resurfaceAt")]
            })
            .collect();

        if rows.is_empty() {
            println!("No handoffs.");
            return Ok(());
        }

        println!("| slug | project | status | resurfaceAt |");
        println!("|------|---------|--------|-------------|");
        for row in &rows {
            println!("| {} | {} | {} | {} |", row[0], row[1], row[2], row[3]);
        }
    }
    Ok(())
}

/// List forks (parallel side-work build-specs) across both tiers. Multiple may
/// be open at once. Title == slug == doc basename.
pub fn list_forks(cwd: &Path, ns: &NamespaceConfig) -> Result<()> {
    let Some(store) = crate::store::load_merged(cwd) else {
        println!("No forks.");
        return Ok(());
    };
    let p = &ns.prefix;
    let sparql = format!(
        "{pfx}\nSELECT ?h ?project ?status ?doc WHERE {{\n\
           GRAPH ?g {{\n\
             ?h a {p}:Handoff ;\n\
               {p}:kind \"fork\" ;\n\
               {p}:project ?project ;\n\
               {p}:status ?status ;\n\
               {p}:handoffDoc ?doc .\n\
           }}\n\
         }}\n\
         ORDER BY ?status ?h",
        pfx = crud::prefixes(ns)
    );

    if let QueryResults::Solutions(solutions) = crate::store::query(&store, &sparql)? {
        let rows: Vec<Vec<String>> = solutions
            .filter_map(|r| r.ok())
            .map(|row| {
                let get = |k: &str| {
                    row.get(k).map(|t| crud::term_display(t.into())).unwrap_or_default()
                };
                let h = get("h");
                let slug = h.rsplit('/').next().unwrap_or(&h).to_string();
                vec![slug, get("project"), get("status"), get("doc")]
            })
            .collect();

        if rows.is_empty() {
            println!("No forks.");
            return Ok(());
        }

        println!("| title | project | status | doc |");
        println!("|-------|---------|--------|-----|");
        for row in &rows {
            println!("| {} | {} | {} | {} |", row[0], row[1], row[2], row[3]);
        }
    }
    Ok(())
}

/// Snooze a handoff: push `resurfaceAt` to now + `days`, hiding it until then.
/// Applied to every tier file so it works wherever the handoff lives.
pub fn snooze(
    gbl_root: Option<&Path>,
    cwd: &Path,
    ns: &NamespaceConfig,
    slug: &str,
    days: i64,
) -> Result<()> {
    let iri = crud::build_iri(ns, "handoff", slug);
    let wake = (chrono::Local::now() + chrono::Duration::days(days))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let p = &ns.prefix;
    let sparql = format!(
        "DELETE {{ GRAPH ?g {{ <{iri}> {p}:resurfaceAt ?old }} }}\n\
         INSERT {{ GRAPH ?g {{ <{iri}> {p}:resurfaceAt \"{wake}\"^^xsd:dateTime }} }}\n\
         WHERE  {{ GRAPH ?g {{ <{iri}> a {p}:Handoff }}\n\
           OPTIONAL {{ GRAPH ?g {{ <{iri}> {p}:resurfaceAt ?old }} }} }}"
    );
    for f in all_tier_files(gbl_root, cwd) {
        mutate_file(&f, ns, &sparql)?;
    }
    Ok(())
}

/// Archive a handoff: set status to "archived" so it stops resurfacing.
pub fn archive(gbl_root: Option<&Path>, cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<()> {
    let iri = crud::build_iri(ns, "handoff", slug);
    let p = &ns.prefix;
    let sparql = format!(
        "DELETE {{ GRAPH ?g {{ <{iri}> {p}:status ?old }} }}\n\
         INSERT {{ GRAPH ?g {{ <{iri}> {p}:status \"archived\" }} }}\n\
         WHERE  {{ GRAPH ?g {{ <{iri}> a {p}:Handoff }}\n\
           OPTIONAL {{ GRAPH ?g {{ <{iri}> {p}:status ?old }} }} }}"
    );
    for f in all_tier_files(gbl_root, cwd) {
        mutate_file(&f, ns, &sparql)?;
    }
    Ok(())
}
