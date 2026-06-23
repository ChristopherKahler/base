use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::config::NamespaceConfig;
use crate::crud;

// ─── paul.toml schema ───────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct PaulToml {
    pub name: String,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub milestone: Option<Milestone>,
    #[serde(default)]
    pub phase: Option<Phase>,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_status() -> String {
    "active".into()
}

#[derive(Debug, Deserialize)]
pub struct Milestone {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct Phase {
    #[serde(default)]
    pub number: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub status: String,
}

// ─── Scanner ────────────────────────────────────────────────

/// Scan all registered workspaces for .paul/paul.toml files.
/// Workspaces come from [[workspace]] entries in ~/.base-gbl/base.toml + cwd.
/// Checks root, every immediate subdirectory, and one level deeper.
pub fn scan_all_workspaces(config: &crate::config::BaseConfig) -> Vec<(PathBuf, PaulToml)> {
    let mut results = Vec::new();
    let mut scanned = std::collections::HashSet::new();

    let mut workspace_roots: Vec<PathBuf> = config
        .workspace
        .iter()
        .map(|w| PathBuf::from(&w.path))
        .collect();

    // Also include cwd's workspace if not already registered
    if let Ok(cwd) = std::env::current_dir()
        && let Some(base_dir) = crate::config::find_workspace_base(&cwd)
        && let Some(root) = base_dir.parent()
    {
        workspace_roots.push(root.to_path_buf());
    }

    // Scan each workspace: root + every subdirectory + one level deeper
    for root in &workspace_roots {
        if !root.is_dir() || !scanned.insert(root.clone()) {
            continue;
        }

        // Check workspace root
        results.extend(try_load_paul_toml(root));

        // Check every immediate subdirectory
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    results.extend(try_load_paul_toml(&path));

                    // One level deeper
                    if let Ok(sub_entries) = std::fs::read_dir(&path) {
                        for sub_entry in sub_entries.flatten() {
                            if sub_entry.path().is_dir() {
                                results.extend(try_load_paul_toml(&sub_entry.path()));
                            }
                        }
                    }
                }
            }
        }
    }

    results
}

fn try_load_paul_toml(project_dir: &Path) -> Option<(PathBuf, PaulToml)> {
    let toml_path = project_dir.join(".paul").join("paul.toml");
    if !toml_path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&toml_path).ok()?;
    let parsed: PaulToml = toml::from_str(&content).ok()?;
    Some((toml_path, parsed))
}

// ─── Graph ingestion ────────────────────────────────────────

pub struct IngestStats {
    pub scanned: usize,
    pub registered: usize,
}

/// Ingest all scanned paul.toml projects into the graph. Idempotent: delete + re-insert.
pub fn ingest_paul_projects(
    cwd: &Path,
    ns: &NamespaceConfig,
    projects: &[(PathBuf, PaulToml)],
) -> Result<IngestStats> {
    let (store, trig_path) = crud::load_workspace_store(cwd)?;
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);
    let pfx = crud::prefixes(ns);
    let p = &ns.prefix;
    let now = crud::now_iso();

    let mut registered = 0usize;

    for (_toml_path, paul) in projects {
        let slug = crud::slugify(&paul.name);
        let iri = crud::build_iri(ns, "project", &slug);

        // Re-ingest the volatile paul-derived fields idempotently, but PRESERVE the
        // mechanical-state predicates: lastActive, status, deferredReason, resurfaceAt,
        // createdAt, and rdf:type. The old "delete everything + re-insert with
        // lastActive=now()" wiped these every session — that was the freshness-faking
        // bug (N projects sharing one identical lastActive) and it also reverted any
        // mechanical deferral straight back to active. Truth for those fields now comes
        // from the reconcile pass (real folder touch), not from ingest.
        let delete = format!(
            "{pfx}\n\
             DELETE {{ GRAPH <{graph}> {{ <{iri}> ?pp ?oo }} }}\n\
             WHERE {{ GRAPH <{graph}> {{ <{iri}> ?pp ?oo .\n\
               FILTER(?pp NOT IN (\
                 rdf:type, {p}:status, {p}:lastActive, {p}:deferredReason, \
                 {p}:resurfaceAt, {p}:createdAt)) }} }}"
        );
        let _ = store.update(&delete);

        // Build milestone/phase/loop description
        let mut extra_triples = String::new();

        if !paul.path.is_empty() {
            extra_triples.push_str(&format!(
                "      <{iri}> {p}:path \"{}\" .\n",
                escape(&paul.path)
            ));
        }

        if let Some(ref ms) = paul.milestone {
            extra_triples.push_str(&format!(
                "      <{iri}> {p}:description \"Milestone: {} ({}) [{}]\" .\n",
                escape(&ms.name),
                escape(&ms.version),
                escape(&ms.status)
            ));
        }

        if let Some(ref phase) = paul.phase {
            extra_triples.push_str(&format!(
                "      <{iri}> {p}:nextAction \"Phase {}: {} [{}]\" .\n",
                phase.number,
                escape(&phase.name),
                escape(&phase.status)
            ));
        }

        // Tag edges → domain association
        for tag in &paul.tags {
            let domain_iri = crud::build_iri(ns, "domain", &crud::slugify(tag));
            extra_triples.push_str(&format!(
                "      <{iri}> {p}:hasDomain <{domain_iri}> .\n"
            ));
        }

        let insert = format!(
            "{pfx}\n\
             INSERT DATA {{\n\
               GRAPH <{graph}> {{\n\
                 <{iri}> rdf:type {p}:Project ;\n\
                   {p}:name \"{}\" ;\n\
                   {p}:updatedAt \"{now}\"^^xsd:dateTime .\n\
             {extra_triples}\
               }}\n\
             }}",
            escape(&paul.name),
        );

        store.update(&insert)
            .with_context(|| format!("Failed to ingest paul project '{}'", paul.name))?;

        // Seed status + lastActive + createdAt ONLY on first registration. On
        // re-ingest these are absent from the WHERE match (they were preserved, not
        // deleted), so this no-ops and the mechanical state set by reconcile stands.
        let seed = format!(
            "{pfx}\n\
             INSERT {{ GRAPH <{graph}> {{\n\
               <{iri}> {p}:status \"{}\" ;\n\
                 {p}:lastActive \"{now}\"^^xsd:dateTime ;\n\
                 {p}:createdAt \"{now}\"^^xsd:dateTime .\n\
             }} }}\n\
             WHERE {{ FILTER NOT EXISTS {{ GRAPH <{graph}> {{ <{iri}> {p}:status ?s }} }} }}",
            escape(&paul.status),
        );
        let _ = store.update(&seed);
        registered += 1;
    }

    crate::store::write_back(&store, &trig_path)?;

    Ok(IngestStats {
        scanned: projects.len(),
        registered,
    })
}

fn escape(s: &str) -> String {
    crate::crud::escape_sparql_literal(s)
}
