pub mod frontmatter;
pub mod ledger;
pub mod paul_json;
pub mod paul_md;
pub mod paul_toml;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
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
    /// Files whose triples do not parse, so nothing of theirs was written or removed (#162). The sync went on past them;
    /// it is partial when this is not empty.
    pub unextractable: Vec<Unextractable>,
    /// One line per link that leads into a path `sync.exclude` excludes: the files under it were not read (#164).
    pub excluded_links: Vec<String>,
}

/// A file the sync could not extract, and the first value that stopped it.
pub struct Unextractable {
    /// The file, relative to the workspace.
    pub file: String,
    /// The predicate the bad value was for (`relatedTo`), or empty when no single triple fails alone.
    pub field: String,
    /// The value as the file gave it, as near as the triple shows it: an entity's name, or a literal's text.
    pub value: String,
    /// The parser's message, for a reader of the code; the line a person sees does not carry it.
    pub error: String,
}

/// The line `base sync` prints for it.
impl std::fmt::Display for Unextractable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.field.is_empty() {
            write!(
                f,
                "base sync skipped {}: base could not store it in the graph. Check its frontmatter, then run base sync again.",
                self.file
            )
        } else {
            write!(
                f,
                "base sync skipped {}: its {} value \"{}\" cannot be stored in the graph. Change that value in the file, then run base sync again.",
                self.file, self.field, self.value
            )
        }
    }
}

/// Run sync: scan workspace files, extract metadata to graph.
pub fn sync(cwd: &Path, config: &BaseConfig, incremental: bool) -> Result<SyncReport> {
    let ns = &config.namespace;
    // #87: lock, THEN load. The guard lives on `locked`, so the critical section
    // runs to the end of this function — and this is the WIDEST load-to-write
    // window in the tree, which is why both forks lost on 2026-09-07 went through
    // it: `fork create` succeeded, then `base sync` overwrote the file from a
    // snapshot taken before it.
    let locked = crud::lock_and_load_workspace(cwd)?;
    let store = locked.store();
    let ws_slug = crud::workspace_slug(cwd);
    let graph_iri = crud::workspace_graph_iri(ns, &ws_slug);
    // Snapshot the one graph this writer targets, so the record can carry what
    // actually changed instead of only a label. Scoped to the target graph
    // because this runs often and diffing the whole store would not be free.
    let before = crate::store::snapshot_graphs(store, std::slice::from_ref(&graph_iri));

    let prefixes = crud::prefixes(ns);

    // Walk workspace for matching files
    let (files, excluded_links) = discover_files(cwd, &config.sync);

    let mut report = SyncReport {
        scanned: 0,
        extracted: 0,
        skipped: 0,
        unextractable: Vec::new(),
        excluded_links,
    };

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
            && let Some(true) = is_up_to_date(store, &file_iri, file_path, ns)
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

                    // Delete existing project triples (idempotent re-extract), except the parent link and `nested`,
                    // which the operator sets and nothing re-derives (D13).
                    let p = &ns.prefix;
                    let del_proj = format!(
                        "{prefixes}\nDELETE {{ GRAPH <{graph_iri}> {{ <{project_iri}> ?p ?o }} }} \
                         WHERE {{ GRAPH <{graph_iri}> {{ <{project_iri}> ?p ?o \
                         FILTER(?p NOT IN ({p}:parentProject, {p}:nested)) }} }}"
                    );
                    let _ = store.update(&del_proj);

                    // Insert triples under the project IRI. The project's folder is the one holding `.paul`, stored
                    // absolute (F25b): the paul.json file's own path is no folder, and storing it made
                    // `project paths --suggest` flag the project and a sync undo the folder it set.
                    let now = crud::now_iso();
                    let p = &ns.prefix;
                    let folder = rel_path
                        .replace('\\', "/")
                        .strip_suffix("/.paul/paul.json")
                        .and_then(|f| crate::crud::project::PathRoots::new(cwd, ns).from_cli(f));
                    let path_pred = format!("{p}:path");
                    let mut body = String::new();
                    for (pred, val) in &t {
                        let val = match (&folder, *pred == path_pred) {
                            (Some(f), true) => format!("\"{}\"", crate::crud::escape_sparql_literal(f)),
                            _ => val.clone(),
                        };
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

        // INSERT fresh triples
        let now = crud::now_iso();
        let p = &ns.prefix;
        let mut insert_body = String::new();
        let mut entity_iris: Vec<String> = Vec::new();
        // Each triple as written, so a file that does not parse can be traced to the value that stopped it.
        let mut lines: Vec<(&str, &str, String)> = Vec::new();
        for (pred, val) in &triples {
            // ENTITY@@{iri}@@{pred} triples get their own subject IRI
            if let Some(rest) = pred.strip_prefix("ENTITY@@") {
                if let Some((iri, actual_pred)) = rest.split_once("@@") {
                    let line = format!("    <{iri}> {actual_pred} {val} .\n");
                    insert_body.push_str(&line);
                    lines.push((actual_pred, val.as_str(), line));
                    if !entity_iris.contains(&iri.to_string()) {
                        entity_iris.push(iri.to_string());
                    }
                }
            } else {
                let line = format!("    <{file_iri}> {pred} {val} .\n");
                insert_body.push_str(&line);
                lines.push((pred.as_str(), val.as_str(), line));
            }
        }
        insert_body.push_str(&format!(
            "    <{file_iri}> {p}:lastExtracted \"{now}\"^^xsd:dateTime .\n"
        ));
        let insert_sparql = format!(
            "{prefixes}\nINSERT DATA {{\n  GRAPH <{graph_iri}> {{\n{insert_body}  }}\n}}"
        );
        // #162: parsed BEFORE anything of this file is deleted. A file whose triples do not parse is skipped and named,
        // and its earlier triples stay; one such file used to end the whole sync here, with every file before it
        // unwritten. A store error after a clean parse is not the file's fault and still stops the sync.
        let update = match oxigraph::sparql::Update::parse(&insert_sparql, None) {
            Ok(update) => update,
            Err(e) => {
                report.unextractable.push(unextractable(&rel_path, &prefixes, &graph_iri, &lines, &e.to_string()));
                continue;
            }
        };

        // DELETE existing triples for this file IRI (idempotent re-extraction)
        let delete_sparql = format!(
            "{prefixes}\nDELETE WHERE {{ GRAPH <{graph_iri}> {{ <{file_iri}> ?p ?o }} }}"
        );
        let _ = store.update(&delete_sparql);

        // Clean up old entity triples for entities owned by this document
        for entity_iri in &entity_iris {
            let del_entity = format!("{prefixes}\nDELETE WHERE {{ GRAPH <{graph_iri}> {{ <{entity_iri}> ?p ?o }} }}");
            let _ = store.update(&del_entity);
        }

        store
            .update(update)
            .with_context(|| format!("inserting triples for {rel_path}"))?;

        report.extracted += 1;
    }

    // Extract ledger.toml (append-only session ledger for cost attribution)
    let ledger_count = ledger::extract_ledger(cwd, store, ns, &graph_iri);
    if ledger_count > 0 {
        report.extracted += ledger_count;
    }

    let delta = crate::store::delta_since(store, std::slice::from_ref(&graph_iri), before);
    let ops = delta.to_ops();
    locked.write(Change::OpWithDelta("extract.markdown", &ops))?;
    Ok(report)
}

/// Name the first triple of a file that does not parse on its own: its predicate's local name and its value.
fn unextractable(file: &str, prefixes: &str, graph_iri: &str, lines: &[(&str, &str, String)], error: &str) -> Unextractable {
    let alone = |line: &str| format!("{prefixes}\nINSERT DATA {{ GRAPH <{graph_iri}> {{\n{line}}} }}");
    // What a person wrote, as near as the triple shows it: the name at the end of an entity's link, or a literal's text.
    let shown = |val: &str| match (val.strip_prefix('<').and_then(|v| v.strip_suffix('>')), val.strip_prefix('"')) {
        (Some(iri), _) => iri.rsplit('/').next().unwrap_or(iri).to_string(),
        (None, Some(lit)) => lit.split('"').next().unwrap_or(lit).to_string(),
        (None, None) => val.to_string(),
    };
    let bad = lines.iter().find_map(|(pred, val, line)| {
        oxigraph::sparql::Update::parse(&alone(line), None)
            .err()
            .map(|e| (pred.rsplit([':', '#', '/']).next().unwrap_or(pred).to_string(), shown(val), e.to_string()))
    });
    match bad {
        Some((field, value, error)) => Unextractable { file: file.to_string(), field, value, error },
        None => Unextractable { file: file.to_string(), field: String::new(), value: String::new(), error: error.to_string() },
    }
}

/// Build a file IRI from a relative path.
pub fn file_iri_from_path(ns: &NamespaceConfig, rel_path: &str) -> String {
    let slug = crate::crud::slugify(rel_path);
    format!("{}document/{}", ns.uri, slug)
}

/// Discover files matching include patterns, excluding exclude patterns. Also returns one warning line per link that
/// led into an excluded path.
///
/// #164: the glob follows links and junctions, so the exclude test on the path as walked let `notes/alias -> ../private`
/// carry `private/` into the graph. Each file's resolved path, relative to the resolved workspace, gets the same test.
/// A link that resolves outside the workspace is followed as before: exclude patterns name paths inside it.
fn discover_files(cwd: &Path, sync_config: &crate::config::SyncConfig) -> (Vec<PathBuf>, Vec<String>) {
    let mut files = Vec::new();
    let excluded = |rel: &str| sync_config.exclude.iter().any(|ex| rel.contains(ex.trim_end_matches('/')));
    let root = crate::config::resolve(cwd);
    // Each directory is resolved once; a file costs one `lstat`, and only a file that is itself a link is resolved.
    let mut dirs: HashMap<PathBuf, Option<PathBuf>> = HashMap::new();
    // link as walked → (where it leads, whether it is a folder, the files not read through it)
    let mut through: BTreeMap<String, (String, bool, BTreeSet<PathBuf>)> = BTreeMap::new();

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
                if excluded(&rel) || !entry.is_file() {
                    continue;
                }
                if let Some(root) = &root
                    && let Some(real) = resolved_rel(&entry, root, &mut dirs)
                    && real != rel
                    && excluded(&real)
                {
                    let (link, leads_to) = first_link(cwd, &rel, root).unwrap_or_else(|| (rel.clone(), real.clone()));
                    let folder = cwd.join(&link).is_dir();
                    through.entry(link).or_insert((leads_to, folder, BTreeSet::new())).2.insert(entry);
                    continue;
                }
                files.push(entry);
            }
        }
    }

    files.sort();
    files.dedup();
    let warnings = through
        .into_iter()
        .map(|(link, (to, folder, skipped))| match (folder, skipped.len()) {
            (false, _) => format!(
                "base sync skipped {link}: it links to {to}, which sync.exclude keeps out of the graph, so it was not read."
            ),
            (true, 1) => format!(
                "base sync skipped {link}: it links into {to}, which sync.exclude keeps out of the graph, so the 1 file \
                 under it was not read."
            ),
            (true, n) => format!(
                "base sync skipped {link}: it links into {to}, which sync.exclude keeps out of the graph, so the {n} files \
                 under it were not read."
            ),
        })
        .collect();
    (files, warnings)
}

/// Where `file` really is, relative to the resolved workspace `root`, with `/` separators. `None` when it resolves outside
/// the workspace (or cannot be resolved).
fn resolved_rel(file: &Path, root: &Path, dirs: &mut HashMap<PathBuf, Option<PathBuf>>) -> Option<String> {
    let real = if std::fs::symlink_metadata(file).is_ok_and(|m| m.file_type().is_symlink()) {
        crate::config::resolved_under(file, root)?
    } else {
        let dir = file.parent()?;
        let real_dir = dirs.entry(dir.to_path_buf()).or_insert_with(|| crate::config::resolved_under(dir, root));
        real_dir.as_ref()?.join(file.file_name()?)
    };
    Some(crate::crud::normalize_path_sep(&real.to_string_lossy()))
}

/// The first link (or junction) on the walked path `rel`, and where it leads, for the warning. `None` when nothing on
/// the way reads as a link.
fn first_link(cwd: &Path, rel: &str, root: &Path) -> Option<(String, String)> {
    let mut walked = PathBuf::new();
    for part in rel.split('/') {
        walked.push(part);
        let at = cwd.join(&walked);
        if std::fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink()) {
            let to = crate::config::resolved_under(&at, root)
                .map(|p| crate::crud::normalize_path_sep(&p.to_string_lossy()))
                .or_else(|| crate::config::resolve(&at).map(|p| p.display().to_string()))?;
            return Some((crate::crud::normalize_path_sep(&walked.to_string_lossy()), to));
        }
    }
    None
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
