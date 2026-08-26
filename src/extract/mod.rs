pub mod frontmatter;
pub mod ledger;
pub mod paul_json;
pub mod paul_md;
pub mod paul_toml;

use std::path::Path;
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};

use crate::config::{BaseConfig, NamespaceConfig};
use crate::changelog::Change;
use crate::crud;

/// Report from a sync operation.
pub struct SyncReport {
    pub scanned: usize,
    pub extracted: usize,
    pub skipped: usize,
}

/// Run sync: scan workspace files, extract metadata to graph.
pub fn sync(cwd: &Path, config: &BaseConfig, incremental: bool) -> Result<SyncReport> {
    let ns = &config.namespace;
    let (store, trig_path) = crud::load_workspace_store(cwd)?;
    let ws_slug = crud::workspace_slug(cwd);
    let graph_iri = crud::workspace_graph_iri(ns, &ws_slug);
    // Snapshot the one graph this writer targets, so the record can carry what
    // actually changed instead of only a label. Scoped to the target graph
    // because this runs often and diffing the whole store would not be free.
    let before = crate::store::snapshot_graphs(&store, std::slice::from_ref(&graph_iri));

    let prefixes = crud::prefixes(ns);

    let mut report = SyncReport {
        scanned: 0,
        extracted: 0,
        skipped: 0,
    };

    // Walk workspace for matching files
    let files = discover_files(cwd, &config.sync);

    for file_path in &files {
        report.scanned += 1;

        // Normalize separators at the one seam where the OS hands us a path.
        // Everything downstream — the IRI slug, the `:path` literal, and the
        // `/`-splitting in frontmatter and paul_md — then behaves identically
        // on every platform instead of only on the one that uses `/`.
        let rel_path = crud::normalize_path_sep(
            &file_path
                .strip_prefix(cwd)
                .unwrap_or(file_path)
                .to_string_lossy(),
        );

        let file_iri = file_iri_from_path(ns, &rel_path);

        // Incremental: check mtime vs lastExtracted
        if incremental
            && let Some(true) = is_up_to_date(&store, &file_iri, file_path, ns)
        {
            report.skipped += 1;
            continue;
        }

        // Route to extractor by file type
        // Normalize line endings: strip \r so CRLF files don't break SPARQL literals.
        let content = match std::fs::read_to_string(file_path) {
            Ok(c) => c.replace('\r', ""),
            Err(_) => {
                report.skipped += 1;
                continue;
            }
        };

        // Derive project hint from cwd for .paul/ docs that start at root
        let project_hint = cwd.file_name()
            .map(|n| n.to_string_lossy().to_string());

        let triples = if rel_path.ends_with("paul.json") {
            // Skip paul.json if paul.toml exists in the same dir (toml takes priority)
            let toml_sibling = file_path.with_file_name("paul.toml");
            if toml_sibling.exists() {
                report.skipped += 1;
                continue;
            }
            // Override IRI: paul.json creates a Project entity, not a Document
            if let Some(t) = paul_json::extract(&content, &rel_path, ns) {
                // Extract project name to build project/ IRI
                if let Some((_, name_val)) = t.iter().find(|(p, _)| p.contains(":name")) {
                    let raw_name = name_val.trim_matches('"');
                    let project_slug = crate::crud::slugify(raw_name);
                    let project_iri = crate::crud::build_iri(ns, "project", &project_slug);

                    // Delete old document-style IRI for this file
                    let del_old = format!("{prefixes}\nDELETE WHERE {{ GRAPH <{graph_iri}> {{ <{file_iri}> ?p ?o }} }}");
                    let _ = store.update(&del_old);

                    // Delete existing project triples (idempotent re-extract)
                    let del_proj = format!("{prefixes}\nDELETE WHERE {{ GRAPH <{graph_iri}> {{ <{project_iri}> ?p ?o }} }}");
                    let _ = store.update(&del_proj);

                    // Insert triples under the project IRI
                    let now = crud::now_iso();
                    let p = &ns.prefix;
                    let mut body = String::new();
                    for (pred, val) in &t {
                        body.push_str(&format!("    <{project_iri}> {pred} {val} .\n"));
                    }
                    body.push_str(&format!("    <{project_iri}> {p}:lastExtracted \"{now}\"^^xsd:dateTime .\n"));
                    let ins = format!("{prefixes}\nINSERT DATA {{ GRAPH <{graph_iri}> {{\n{body}}} }}");
                    if store.update(&ins).is_ok() {
                        report.extracted += 1;
                    }
                    continue;
                }
                Some(t)
            } else {
                None
            }
        } else if rel_path.ends_with(".md") && paul_md::is_paul_artifact(&rel_path) {
            paul_md::extract(&content, &rel_path, ns)
        } else if rel_path.ends_with(".md") {
            frontmatter::extract_with_project(&content, &rel_path, ns, project_hint.as_deref())
        } else {
            None
        };

        let Some(triples) = triples else {
            report.skipped += 1;
            continue;
        };

        // DELETE existing triples for this file IRI (idempotent re-extraction)
        let delete_sparql = format!(
            "{prefixes}\nDELETE WHERE {{ GRAPH <{graph_iri}> {{ <{file_iri}> ?p ?o }} }}"
        );
        let _ = store.update(&delete_sparql);

        // INSERT fresh triples
        let now = crud::now_iso();
        let p = &ns.prefix;
        let mut insert_body = String::new();
        let mut entity_iris: Vec<String> = Vec::new();
        for (pred, val) in &triples {
            // ENTITY@@{iri}@@{pred} triples get their own subject IRI
            if let Some(rest) = pred.strip_prefix("ENTITY@@") {
                if let Some((iri, actual_pred)) = rest.split_once("@@") {
                    insert_body.push_str(&format!("    <{iri}> {actual_pred} {val} .\n"));
                    if !entity_iris.contains(&iri.to_string()) {
                        entity_iris.push(iri.to_string());
                    }
                }
            } else {
                insert_body.push_str(&format!("    <{file_iri}> {pred} {val} .\n"));
            }
        }
        insert_body.push_str(&format!(
            "    <{file_iri}> {p}:lastExtracted \"{now}\"^^xsd:dateTime .\n"
        ));
        // Clean up old entity triples for entities owned by this document
        for entity_iri in &entity_iris {
            let del_entity = format!("{prefixes}\nDELETE WHERE {{ GRAPH <{graph_iri}> {{ <{entity_iri}> ?p ?o }} }}");
            let _ = store.update(&del_entity);
        }

        let insert_sparql = format!(
            "{prefixes}\nINSERT DATA {{\n  GRAPH <{graph_iri}> {{\n{insert_body}  }}\n}}"
        );
        store
            .update(&insert_sparql)
            .with_context(|| format!("inserting triples for {rel_path}"))?;

        report.extracted += 1;
    }

    // Extract ledger.toml (append-only session ledger for cost attribution)
    let ledger_count = ledger::extract_ledger(cwd, &store, ns, &graph_iri);
    if ledger_count > 0 {
        report.extracted += ledger_count;
    }

    let delta = crate::store::delta_since(&store, std::slice::from_ref(&graph_iri), before);
    let ops = delta.to_ops();
    crate::store::write_back(&store, &trig_path, Change::OpWithDelta("extract.markdown", &ops))?;
    Ok(report)
}

/// Build a file IRI from a relative path.
pub fn file_iri_from_path(ns: &NamespaceConfig, rel_path: &str) -> String {
    let slug = crate::crud::slugify(rel_path);
    format!("{}document/{}", ns.uri, slug)
}

/// Discover files matching include patterns, excluding exclude patterns.
fn discover_files(cwd: &Path, sync_config: &crate::config::SyncConfig) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();

    for pattern in &sync_config.include {
        let full_pattern = format!("{}/{}", cwd.display(), pattern);
        if let Ok(paths) = glob::glob(&full_pattern) {
            for entry in paths.flatten() {
                // Check excludes
                // Same seam: `exclude` patterns are written with `/`, so the
                // candidate has to be normalized or nothing is ever excluded
                // below the workspace root on Windows.
                let rel = crate::crud::normalize_path_sep(
                    &entry.strip_prefix(cwd).unwrap_or(&entry).to_string_lossy(),
                );
                let excluded = sync_config
                    .exclude
                    .iter()
                    .any(|ex| rel.contains(ex.trim_end_matches('/')));
                if !excluded && entry.is_file() {
                    files.push(entry);
                }
            }
        }
    }

    files.sort();
    files.dedup();
    files
}

/// Check if a file is up-to-date (mtime <= lastExtracted).
fn is_up_to_date(
    store: &oxigraph::store::Store,
    file_iri: &str,
    file_path: &Path,
    ns: &NamespaceConfig,
) -> Option<bool> {
    // Get file mtime
    let metadata = std::fs::metadata(file_path).ok()?;
    let mtime = metadata.modified().ok()?;
    let mtime_secs = mtime.duration_since(UNIX_EPOCH).ok()?.as_secs();

    // Query lastExtracted from graph
    let p = &ns.prefix;
    let sparql = format!(
        "{}\nSELECT ?ts WHERE {{ GRAPH ?g {{ <{file_iri}> {p}:lastExtracted ?ts }} }}",
        crud::prefixes(ns)
    );

    if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&sparql) {
        for row in solutions.flatten() {
            if let Some(term) = row.get("ts") {
                let ts_str = crud::term_display(term.into());
                // Parse ISO 8601 timestamp to compare
                if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&ts_str) {
                    let extracted_secs = dt.timestamp() as u64;
                    return Some(mtime_secs <= extracted_secs);
                }
            }
        }
    }

    Some(false) // No lastExtracted found → needs extraction
}
