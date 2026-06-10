use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::BaseConfig;
use crate::crud;
use crate::store;

/// Stats returned from an ingest run.
#[derive(Debug, Default)]
pub struct IngestStats {
    pub files: usize,
    pub entities: usize,
}

/// Ingest all declared sources for an extension into the workspace graph.
/// Resolves state_dir relative to cwd. Missing state_dir → Ok(zero stats).
/// Fail-open: file/parse errors → eprintln + skip, never panic.
pub fn ingest_extension(
    ext: &super::ExtensionDef,
    cwd: &Path,
    config: &BaseConfig,
) -> Result<IngestStats> {
    let hooks = match &ext.hooks {
        Some(h) => h,
        None => return Ok(IngestStats::default()),
    };
    let ss = match &hooks.session_start {
        Some(s) => s,
        None => return Ok(IngestStats::default()),
    };
    if ss.ingest.is_empty() {
        return Ok(IngestStats::default());
    }

    // Resolve state_dir relative to cwd
    let state_dir = match &ext.state_dir {
        Some(sd) => {
            let expanded = expand_tilde(sd);
            if expanded.is_absolute() {
                expanded
            } else {
                cwd.join(expanded)
            }
        }
        None => cwd.to_path_buf(),
    };

    if !state_dir.is_dir() {
        return Ok(IngestStats::default());
    }

    // Load workspace graph
    let (graph_store, trig_path) = crud::load_workspace_store(cwd)
        .context("Failed to load workspace store for ingest")?;
    let ns = &config.namespace;
    let ws_slug = crud::workspace_slug(cwd);
    let graph_iri = crud::workspace_graph_iri(ns, &ws_slug);
    let pfx = crud::prefixes(ns);
    let p = &ns.prefix;

    let ctx = IngestCtx {
        store: &graph_store,
        ns,
        graph_iri: &graph_iri,
        pfx: &pfx,
        p,
        ext_name: &ext.name,
    };

    let mut stats = IngestStats::default();
    let mut graph_dirty = false;

    for source in &ss.ingest {
        // Resolve file paths (handle globs)
        let file_paths = resolve_file_paths(&state_dir, &source.file);

        for file_path in &file_paths {
            match ingest_file(&ctx, source, file_path) {
                Ok(count) => {
                    if count > 0 {
                        stats.files += 1;
                        stats.entities += count;
                        graph_dirty = true;
                    }
                }
                Err(e) => {
                    eprintln!(
                        "base: ext:{} ingest error for {}: {e}",
                        ext.name,
                        file_path.display()
                    );
                }
            }
        }
    }

    // Write back if we changed anything
    if graph_dirty {
        store::write_back(&graph_store, &trig_path)
            .with_context(|| format!("Failed to write back graph after ext:{} ingest", ext.name))?;
    }

    Ok(stats)
}

/// Context for a single ingest operation — avoids passing 8 params.
struct IngestCtx<'a> {
    store: &'a oxigraph::store::Store,
    ns: &'a crate::config::NamespaceConfig,
    graph_iri: &'a str,
    pfx: &'a str,
    p: &'a str,
    ext_name: &'a str,
}

/// Ingest a single JSON file into the graph.
/// Returns the number of entities ingested.
fn ingest_file(
    ctx: &IngestCtx<'_>,
    source: &super::IngestSource,
    file_path: &Path,
) -> Result<usize> {
    let content = std::fs::read_to_string(file_path)
        .with_context(|| format!("Reading {}", file_path.display()))?;

    let json: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("Parsing JSON from {}", file_path.display()))?;

    let entity_type = &source.entity;
    let ext_name = ctx.ext_name;
    let pfx = ctx.pfx;
    let graph_iri = ctx.graph_iri;
    let p = ctx.p;
    let iri_prefix = format!("ext/{ext_name}/{entity_type}");

    // Extract array of objects from JSON
    let objects = extract_objects(&json);

    if objects.is_empty() {
        return Ok(0);
    }

    // For "replace" strategy: delete all existing entities of this type first
    if source.strategy == "replace" {
        let delete_query = format!(
            "{pfx}\n\
             DELETE {{\n\
               GRAPH <{graph_iri}> {{\n\
                 ?entity ?ep ?eo .\n\
               }}\n\
             }}\n\
             WHERE {{\n\
               GRAPH <{graph_iri}> {{\n\
                 ?entity rdf:type {p}:{entity_type} ;\n\
                          {p}:source \"ext:{ext_name}\" ;\n\
                          ?ep ?eo .\n\
               }}\n\
             }}"
        );
        let _ = ctx.store.update(&delete_query);
    }

    let mut count = 0;

    for (idx, obj) in objects.iter().enumerate() {
        let Some(map) = obj.as_object() else {
            continue;
        };

        // Derive entity ID
        let id = map
            .get("id")
            .and_then(|v| match v {
                serde_json::Value::String(s) => Some(crud::slugify(s)),
                serde_json::Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .unwrap_or_else(|| idx.to_string());

        let entity_iri = crud::build_iri(ctx.ns, &iri_prefix, &id);

        // For "upsert" strategy: delete existing triples for this specific entity first
        if source.strategy == "upsert" {
            let delete_entity = format!(
                "{pfx}\n\
                 DELETE {{\n\
                   GRAPH <{graph_iri}> {{\n\
                     <{entity_iri}> ?ep ?eo .\n\
                   }}\n\
                 }}\n\
                 WHERE {{\n\
                   GRAPH <{graph_iri}> {{\n\
                     <{entity_iri}> ?ep ?eo .\n\
                   }}\n\
                 }}"
            );
            let _ = ctx.store.update(&delete_entity);
        }

        // Build INSERT DATA for this entity
        let mut triples = Vec::new();
        triples.push(format!(
            "<{entity_iri}> rdf:type {p}:{entity_type}"
        ));
        triples.push(format!(
            "<{entity_iri}> {p}:source \"ext:{ext_name}\""
        ));

        for (key, value) in map {
            if key == "id" {
                continue;
            }

            let pred = format!("{p}:{key}");

            match value {
                serde_json::Value::String(s) => {
                    triples.push(format!(
                        "<{entity_iri}> {pred} \"{}\"",
                        crud::escape_sparql_literal(s)
                    ));
                }
                serde_json::Value::Number(n) => {
                    triples.push(format!(
                        "<{entity_iri}> {pred} \"{n}\""
                    ));
                }
                serde_json::Value::Bool(b) => {
                    triples.push(format!(
                        "<{entity_iri}> {pred} \"{b}\""
                    ));
                }
                serde_json::Value::Array(arr) => {
                    for item in arr {
                        if let Some(s) = item.as_str() {
                            triples.push(format!(
                                "<{entity_iri}> {pred} \"{}\"",
                                crud::escape_sparql_literal(s)
                            ));
                        } else if let Some(n) = item.as_f64() {
                            triples.push(format!(
                                "<{entity_iri}> {pred} \"{n}\""
                            ));
                        }
                    }
                }
                serde_json::Value::Object(_) => {
                    eprintln!(
                        "base: ext:{ext_name} ingest: skipping nested object field '{key}'"
                    );
                }
                serde_json::Value::Null => {}
            }
        }

        let insert = format!(
            "{pfx}\n\
             INSERT DATA {{\n\
               GRAPH <{graph_iri}> {{\n\
                 {} .\n\
               }}\n\
             }}",
            triples.join(" .\n                 ")
        );

        ctx.store
            .update(&insert)
            .with_context(|| format!("Inserting entity {entity_iri}"))?;

        count += 1;
    }

    Ok(count)
}

/// Extract an array of JSON objects from various JSON shapes.
/// - Array at top level → use directly
/// - Object with a single key whose value is an array → use the array
/// - Single object → wrap in a one-element vec
fn extract_objects(json: &serde_json::Value) -> Vec<&serde_json::Value> {
    match json {
        serde_json::Value::Array(arr) => arr.iter().collect(),
        serde_json::Value::Object(map) => {
            // Check for single-key wrapper: {"items": [...]}
            if map.len() == 1
                && let Some(serde_json::Value::Array(arr)) = map.values().next()
            {
                return arr.iter().collect();
            }
            // Single object entity
            vec![json]
        }
        _ => Vec::new(),
    }
}

/// Resolve file paths, expanding globs if the pattern contains `*`.
fn resolve_file_paths(state_dir: &Path, pattern: &str) -> Vec<PathBuf> {
    if pattern.contains('*') {
        let full_pattern = state_dir.join(pattern).to_string_lossy().to_string();
        match glob::glob(&full_pattern) {
            Ok(paths) => paths.filter_map(|p| p.ok()).collect(),
            Err(e) => {
                eprintln!("base: ingest glob error for '{pattern}': {e}");
                Vec::new()
            }
        }
    } else {
        let path = state_dir.join(pattern);
        if path.exists() {
            vec![path]
        } else {
            Vec::new()
        }
    }
}

/// Expand `~/` prefix to home directory.
fn expand_tilde(path: &str) -> PathBuf {
    if path.starts_with("~/") {
        dirs::home_dir()
            .map(|h| h.join(&path[2..]))
            .unwrap_or_else(|| PathBuf::from(path))
    } else {
        PathBuf::from(path)
    }
}

// ─── Tests ───────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_objects_from_array() {
        let json: serde_json::Value =
            serde_json::from_str(r#"[{"id": "a"}, {"id": "b"}]"#).unwrap();
        let objs = extract_objects(&json);
        assert_eq!(objs.len(), 2);
    }

    #[test]
    fn extract_objects_from_wrapper() {
        let json: serde_json::Value =
            serde_json::from_str(r#"{"items": [{"id": "a"}, {"id": "b"}]}"#).unwrap();
        let objs = extract_objects(&json);
        assert_eq!(objs.len(), 2);
    }

    #[test]
    fn extract_objects_from_single() {
        let json: serde_json::Value =
            serde_json::from_str(r#"{"id": "solo", "name": "Single"}"#).unwrap();
        let objs = extract_objects(&json);
        assert_eq!(objs.len(), 1);
    }

    #[test]
    fn resolve_non_glob_existing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("data.json"), "{}").unwrap();
        let paths = resolve_file_paths(dir.path(), "data.json");
        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn resolve_non_glob_missing() {
        let dir = tempfile::tempdir().unwrap();
        let paths = resolve_file_paths(dir.path(), "missing.json");
        assert_eq!(paths.len(), 0);
    }

    #[test]
    fn resolve_glob_pattern() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("youtube.registry.json"), "[]").unwrap();
        std::fs::write(dir.path().join("tiktok.registry.json"), "[]").unwrap();
        std::fs::write(dir.path().join("other.txt"), "").unwrap();
        let paths = resolve_file_paths(dir.path(), "*.registry.json");
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn expand_tilde_works() {
        let result = expand_tilde("~/foo/bar");
        assert!(result.to_string_lossy().contains("foo/bar"));
        assert!(!result.to_string_lossy().starts_with("~"));
    }

    #[test]
    fn ingest_full_round_trip() {
        let workspace = tempfile::tempdir().unwrap();
        let base_dir = workspace.path().join(".base");
        std::fs::create_dir_all(&base_dir).unwrap();
        std::fs::write(base_dir.join("graph.nq"), "").unwrap();

        // Create state dir with test data
        let state_dir = workspace.path().join(".test-ext");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(
            state_dir.join("items.json"),
            r#"[
                {"id": "item-1", "title": "First", "score": 42},
                {"id": "item-2", "title": "Second", "score": 88}
            ]"#,
        )
        .unwrap();

        // Build extension
        let ext = super::super::ExtensionDef {
            name: "test-ingest".into(),
            version: "0.1.0".into(),
            description: "Test".into(),
            framework_dir: None,
            state_dir: Some(".test-ext".into()),
            hooks: Some(super::super::ExtensionHooks {
                session_start: Some(super::super::SessionStartHook {
                    queries: vec![],
                    ingest: vec![super::super::IngestSource {
                        file: "items.json".into(),
                        entity: "TestItem".into(),
                        strategy: "upsert".into(),
                    }],
                    inject: None,
                }),
                user_prompt: None,
                pre_tool: None,
                post_tool: None,
            }),
            source_path: None,
        };

        let config = BaseConfig::load(workspace.path());

        let stats = ingest_extension(&ext, workspace.path(), &config).unwrap();
        assert_eq!(stats.files, 1);
        assert_eq!(stats.entities, 2);

        // Verify entities exist in graph by querying
        let (store, _) = crud::load_workspace_store(workspace.path()).unwrap();
        let ns = &config.namespace;
        let p = &ns.prefix;
        let query = format!(
            "PREFIX {p}: <{u}>\n\
             PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\n\
             SELECT ?entity ?title WHERE {{\n\
               GRAPH ?g {{\n\
                 ?entity rdf:type {p}:TestItem ;\n\
                         {p}:title ?title .\n\
               }}\n\
             }} ORDER BY ?title",
            u = ns.uri,
        );
        if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&query) {
            let rows: Vec<_> = solutions.filter_map(|r| r.ok()).collect();
            assert_eq!(rows.len(), 2, "Expected 2 TestItem entities in graph");
        } else {
            panic!("Query failed");
        }
    }

    #[test]
    fn ingest_replace_strategy_clears_old() {
        let workspace = tempfile::tempdir().unwrap();
        let base_dir = workspace.path().join(".base");
        std::fs::create_dir_all(&base_dir).unwrap();
        std::fs::write(base_dir.join("graph.nq"), "").unwrap();

        let state_dir = workspace.path().join(".test-ext");
        std::fs::create_dir_all(&state_dir).unwrap();

        let ext = super::super::ExtensionDef {
            name: "test-replace".into(),
            version: "0.1.0".into(),
            description: "Test".into(),
            framework_dir: None,
            state_dir: Some(".test-ext".into()),
            hooks: Some(super::super::ExtensionHooks {
                session_start: Some(super::super::SessionStartHook {
                    queries: vec![],
                    ingest: vec![super::super::IngestSource {
                        file: "items.json".into(),
                        entity: "ReplItem".into(),
                        strategy: "replace".into(),
                    }],
                    inject: None,
                }),
                user_prompt: None,
                pre_tool: None,
                post_tool: None,
            }),
            source_path: None,
        };

        let config = BaseConfig::load(workspace.path());

        // First ingest: 3 items
        std::fs::write(
            state_dir.join("items.json"),
            r#"[{"id": "a"}, {"id": "b"}, {"id": "c"}]"#,
        )
        .unwrap();
        let stats1 = ingest_extension(&ext, workspace.path(), &config).unwrap();
        assert_eq!(stats1.entities, 3);

        // Second ingest: only 1 item — replace should remove the other 2
        std::fs::write(
            state_dir.join("items.json"),
            r#"[{"id": "x"}]"#,
        )
        .unwrap();
        let stats2 = ingest_extension(&ext, workspace.path(), &config).unwrap();
        assert_eq!(stats2.entities, 1);

        // Verify only 1 entity remains
        let (store, _) = crud::load_workspace_store(workspace.path()).unwrap();
        let ns = &config.namespace;
        let p = &ns.prefix;
        let count_query = format!(
            "PREFIX {p}: <{u}>\n\
             PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\n\
             SELECT (COUNT(?e) AS ?c) WHERE {{\n\
               GRAPH ?g {{ ?e rdf:type {p}:ReplItem }}\n\
             }}",
            u = ns.uri,
        );
        if let Ok(oxigraph::sparql::QueryResults::Solutions(mut solutions)) =
            store.query(&count_query)
        {
            let row = solutions.next().unwrap().unwrap();
            let count_str = row.get("c").unwrap().to_string();
            // Oxigraph returns typed literal: "1"^^<...integer>
            assert!(
                count_str.contains("\"1\""),
                "Expected 1 entity after replace, got: {count_str}"
            );
        }
    }

    #[test]
    fn ingest_missing_state_dir_returns_zero() {
        let workspace = tempfile::tempdir().unwrap();
        let base_dir = workspace.path().join(".base");
        std::fs::create_dir_all(&base_dir).unwrap();
        std::fs::write(base_dir.join("graph.nq"), "").unwrap();

        let ext = super::super::ExtensionDef {
            name: "test-missing".into(),
            version: "0.1.0".into(),
            description: "Test".into(),
            framework_dir: None,
            state_dir: Some(".nonexistent/".into()),
            hooks: Some(super::super::ExtensionHooks {
                session_start: Some(super::super::SessionStartHook {
                    queries: vec![],
                    ingest: vec![super::super::IngestSource {
                        file: "data.json".into(),
                        entity: "Thing".into(),
                        strategy: "upsert".into(),
                    }],
                    inject: None,
                }),
                user_prompt: None,
                pre_tool: None,
                post_tool: None,
            }),
            source_path: None,
        };

        let config = BaseConfig::load(workspace.path());
        let stats = ingest_extension(&ext, workspace.path(), &config).unwrap();
        assert_eq!(stats.files, 0);
        assert_eq!(stats.entities, 0);
    }
}
