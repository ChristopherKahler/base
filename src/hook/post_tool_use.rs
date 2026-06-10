use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::Local;

use crate::config::BaseConfig;
use crate::crud;
use crate::store;

pub fn handle(config: &BaseConfig, cwd: &Path, event: &serde_json::Value) -> Result<super::HookEventData> {
    let mut data = super::HookEventData::default();
    let file_paths = extract_file_paths(event);
    if file_paths.is_empty() {
        return Ok(data);
    }

    // ─── lastActive timestamp update (existing behavior) ─────
    let trig_path = find_workspace_trig(cwd);
    if let Some(ref tp) = trig_path {
        let graph = store::load_graph(tp)?;
        let now = Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        let mut updated = false;

        for file_path in &file_paths {
            let sparql = format!(
                "PREFIX {p}: <{u}>\n\
                 PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\n\
                 DELETE {{ GRAPH ?g {{ ?entity {p}:lastActive ?old }} }}\n\
                 INSERT {{ GRAPH ?g {{ ?entity {p}:lastActive \"{now}\"^^xsd:dateTime }} }}\n\
                 WHERE {{\n\
                   GRAPH ?g {{\n\
                     ?entity {p}:path ?path .\n\
                     FILTER(STRSTARTS(\"{file}\", STR(?path)))\n\
                     OPTIONAL {{ ?entity {p}:lastActive ?old }}\n\
                   }}\n\
                 }}",
                p = config.namespace.prefix,
                u = config.namespace.uri,
                file = file_path.display(),
            );

            if graph.update(&sparql).is_ok() {
                updated = true;
            }
        }

        if updated {
            store::write_back(&graph, tp)?;
        }
    }

    // ─── AST section context for partial reads ──────────────
    let tool_name = event
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if tool_name == "Read"
        && let (Some(offset), Some(limit)) = (extract_offset(event), extract_limit(event)) {
            // Partial read — inject section-specific AST entities
            for fp in &file_paths {
                if let Some(fp_str) = fp.to_str()
                    && is_source_file(fp_str)
                        && let Some(section) =
                            crud::ast_query::section_entities(cwd, &config.namespace, fp_str, offset, limit)
                        {
                            print!("{}", section.trim_end());
                            data.section_context = true;
                        }
            }
        }

    // ─── Extension post-tool handlers ───────────────────────
    let extensions = crate::extension::load_extensions();
    for ext in &extensions {
        if let Some(hooks) = &ext.hooks
            && let Some(post) = &hooks.post_tool
        {
            for handler in &post.handlers {
                for fp in &file_paths {
                    let fp_str = fp.to_string_lossy();
                    if fp_str.contains(&handler.pattern) {
                        match handler.action.as_str() {
                            "log" => {
                                eprintln!(
                                    "base: ext:{} post-tool: {} matched {}",
                                    ext.name, handler.pattern, fp_str
                                );
                            }
                            "reingest" => {
                                match crate::extension::ingest::ingest_extension(ext, cwd, config) {
                                    Ok(stats) if stats.entities > 0 => {
                                        eprintln!(
                                            "base: ext:{} reingest: {} entities from {} file(s)",
                                            ext.name, stats.entities, stats.files
                                        );
                                    }
                                    Err(e) => {
                                        eprintln!("base: ext:{} reingest error: {e}", ext.name);
                                    }
                                    _ => {}
                                }
                            }
                            "query" => {
                                // Future: run a SPARQL query on match
                                eprintln!(
                                    "base: ext:{} query triggered for {} (stub — Phase 24)",
                                    ext.name, handler.pattern
                                );
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    Ok(data)
}

fn is_source_file(path: &str) -> bool {
    let exts = [
        ".rs", ".py", ".js", ".ts", ".go", ".jsx", ".tsx", ".c", ".cpp", ".h", ".hpp",
        ".java", ".rb", ".swift", ".kt", ".kts", ".scala", ".php", ".cs", ".lua", ".zig",
        ".ps1", ".ex", ".exs", ".jl", ".vue", ".svelte", ".astro", ".dart", ".sql", ".r",
        ".f90", ".pas", ".sh", ".bash", ".json", ".toml", ".yaml", ".yml",
    ];
    exts.iter().any(|ext| path.ends_with(ext))
}

fn extract_offset(event: &serde_json::Value) -> Option<u64> {
    event
        .get("tool_input")
        .and_then(|ti| ti.get("offset"))
        .and_then(|v| v.as_u64())
}

fn extract_limit(event: &serde_json::Value) -> Option<u64> {
    event
        .get("tool_input")
        .and_then(|ti| ti.get("limit"))
        .and_then(|v| v.as_u64())
}

/// Extract file paths from the hook event JSON.
/// Handles common Claude Code tool shapes: Edit, Write, Read.
fn extract_file_paths(event: &serde_json::Value) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // tool_input.file_path — used by Edit, Write, Read
    if let Some(fp) = event
        .get("tool_input")
        .and_then(|ti| ti.get("file_path"))
        .and_then(|v| v.as_str())
    {
        paths.push(PathBuf::from(fp));
    }

    // tool_input.path — alternative field name
    if let Some(fp) = event
        .get("tool_input")
        .and_then(|ti| ti.get("path"))
        .and_then(|v| v.as_str())
    {
        paths.push(PathBuf::from(fp));
    }

    // tool_input.command — Bash commands may reference files, but we skip those
    // (too noisy, too hard to parse reliably)

    paths
}

/// Find the workspace .base/graph.nq by walking upward from cwd.
fn find_workspace_trig(cwd: &Path) -> Option<PathBuf> {
    let mut dir = cwd.to_path_buf();
    loop {
        let candidate = dir.join(".base").join("graph.nq");
        if candidate.exists() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_file_path_from_edit_event() {
        let event = serde_json::json!({
            "tool_name": "Edit",
            "tool_input": {
                "file_path": "/home/user/project/src/main.rs",
                "old_string": "foo",
                "new_string": "bar"
            }
        });
        let paths = extract_file_paths(&event);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].to_str().unwrap(), "/home/user/project/src/main.rs");
    }

    #[test]
    fn extract_returns_empty_for_unknown_tool() {
        let event = serde_json::json!({
            "tool_name": "WebSearch",
            "tool_input": {
                "query": "rust oxigraph"
            }
        });
        let paths = extract_file_paths(&event);
        assert!(paths.is_empty());
    }

    #[test]
    fn find_workspace_trig_walks_up() {
        let tmp = tempfile::tempdir().unwrap();
        let base_dir = tmp.path().join(".base");
        std::fs::create_dir_all(&base_dir).unwrap();
        std::fs::write(base_dir.join("graph.nq"), "# test").unwrap();

        let sub = tmp.path().join("deep").join("nested");
        std::fs::create_dir_all(&sub).unwrap();

        let found = find_workspace_trig(&sub);
        assert!(found.is_some());
        assert!(found.unwrap().ends_with(".base/graph.nq"));
    }

    #[test]
    fn find_workspace_trig_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let found = find_workspace_trig(tmp.path());
        assert!(found.is_none());
    }
}
