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
    // Session state is reused for once-per-session "inject" dedup. Loaded
    // lazily-shared across all handlers, saved once at the end if dirty.
    let base_dir = crate::config::find_workspace_base(cwd);
    let mut session = base_dir
        .as_deref()
        .map(crate::domain::session::SessionState::load)
        .unwrap_or_default();
    let mut session_dirty = false;
    // Fourth design signal: styling markers inside the written content
    // (CSS-in-JS, Tailwind, inline styles) — computed once, path-independent.
    let content_design = content_has_design_markers(event);

    for ext in &extensions {
        if let Some(hooks) = &ext.hooks
            && let Some(post) = &hooks.post_tool
        {
            for handler in &post.handlers {
                for fp in &file_paths {
                    let fp_str = fp.to_string_lossy();
                    // Match: "designset" sentinel → built-in design-file heuristic;
                    // any other pattern → substring match (existing behavior).
                    let matched = if handler.pattern == "designset" {
                        is_design_file(&fp_str) || content_design
                    } else {
                        fp_str.contains(&handler.pattern)
                    };
                    if !matched {
                        continue;
                    }
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
                        "inject" => {
                            // Tool gate: default Write/Edit/MultiEdit, never Read.
                            let allowed = handler.on_tools.clone().unwrap_or_else(|| {
                                vec!["Write".into(), "Edit".into(), "MultiEdit".into()]
                            });
                            if !allowed.iter().any(|t| t.as_str() == tool_name) {
                                continue;
                            }
                            let Some(message) = handler.message.as_deref() else {
                                continue;
                            };
                            // Once-per-session dedup keyed on ext name + message hash.
                            // Re-fires only if the message text changes.
                            let once = handler.once_per_session.unwrap_or(true);
                            let key = format!("ext-inject:{}", ext.name);
                            let msg_hash =
                                crate::domain::session::rules_hash(&[message.to_string()]);
                            if once && session.is_injected(&key, msg_hash) {
                                continue;
                            }
                            print!("{}", message.trim_end());
                            data.nudged = true;
                            if once {
                                session.mark_injected(&key, msg_hash);
                                session_dirty = true;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    if session_dirty
        && let Some(bd) = base_dir.as_deref()
    {
        let _ = session.save(bd);
    }

    Ok(data)
}

/// Design/frontend file heuristic for the "designset" post-tool inject pattern.
/// Broad by design — once-per-session dedup caps the cost of a false positive
/// at a single nudge, so we favor coverage over precision. Three signal classes:
/// file extension, path segment, and filename. Content markers are a fourth
/// class handled separately (see `content_has_design_markers`).
fn is_design_file(path: &str) -> bool {
    let lower = path.to_lowercase();

    // ── 1. Extensions: style, markup, component, template, vector ──
    const EXTS: &[&str] = &[
        // stylesheets
        ".css", ".scss", ".sass", ".less", ".pcss", ".postcss", ".styl",
        // markup / components
        ".html", ".htm", ".vue", ".svelte", ".astro", ".tsx", ".jsx", ".mdx",
        // template engines / server views
        ".liquid", ".hbs", ".handlebars", ".mustache", ".ejs", ".pug", ".jade",
        ".haml", ".slim", ".erb", ".twig", ".njk", ".blade.php", ".razor",
        ".cshtml", ".vbhtml",
        // native / app UI markup
        ".xaml", ".axaml", ".qml",
        // vector / visual
        ".svg",
        // CSS-in-TS (vanilla-extract / treat)
        ".css.ts", ".css.js",
    ];
    if EXTS.iter().any(|e| lower.ends_with(e)) {
        return true;
    }

    // ── 2. Path segments: folders that imply UI/design work ──
    const SEGMENTS: &[&str] = &[
        "/components/", "/component/", "/ui/", "/styles/", "/style/", "/css/",
        "/scss/", "/sass/", "/less/", "/stylesheets/", "/design/", "/design-system/",
        "/theme/", "/themes/", "/tokens/", "/layout/", "/layouts/", "/pages/",
        "/views/", "/templates/", "/partials/", "/sections/", "/blocks/", "/widgets/",
        "/screens/", "/elements/", "/atoms/", "/molecules/", "/organisms/",
        "/landing/", "/marketing/", "/brand/", "/branding/", "/motion/",
        "/animations/", "/fonts/", "/icons/", "/svg/", "/assets/",
        "/storybook/", "/.storybook/", "/stories/",
    ];
    if SEGMENTS.iter().any(|s| lower.contains(s)) {
        return true;
    }

    // ── 3. Filenames: framework configs + token/theme files ──
    const FILES: &[&str] = &[
        "tailwind.config", "postcss.config", "unocss.config", "uno.config",
        "windi.config", "panda.config", "stitches.config", "components.json",
        "globals.css", ".module.css", ".module.scss", ".styles.", ".styled.",
        "theme.", "tokens.", "palette.", "typography.", "design-tokens.", "stylelint",
    ];
    FILES.iter().any(|f| lower.contains(f))
}

/// Extract the text being written: Write.content, Edit.new_string,
/// or MultiEdit.edits[].new_string (concatenated).
fn extract_written_content(event: &serde_json::Value) -> String {
    let Some(ti) = event.get("tool_input") else {
        return String::new();
    };
    let mut out = String::new();
    if let Some(c) = ti.get("content").and_then(|v| v.as_str()) {
        out.push_str(c);
    }
    if let Some(c) = ti.get("new_string").and_then(|v| v.as_str()) {
        out.push('\n');
        out.push_str(c);
    }
    if let Some(edits) = ti.get("edits").and_then(|v| v.as_array()) {
        for e in edits {
            if let Some(c) = e.get("new_string").and_then(|v| v.as_str()) {
                out.push('\n');
                out.push_str(c);
            }
        }
    }
    out
}

/// Fourth signal class: design work hidden inside a non-design extension
/// (CSS-in-JS, Tailwind, inline styles in a .ts/.js/etc). Scans the written
/// content for high-signal styling markers.
fn content_has_design_markers(event: &serde_json::Value) -> bool {
    let content = extract_written_content(event);
    if content.is_empty() {
        return false;
    }
    let lower = content.to_lowercase();
    const MARKERS: &[&str] = &[
        // JSX / HTML styling attributes
        "classname=", "class=", "style=", "<style",
        // CSS-in-JS libraries
        "styled.", "styled(", "css`", "tw`", "createglobalstyle", "createtheme",
        "makestyles", "usestyles", "sx={", "cva(", "clsx(",
        // Tailwind / utility
        "@apply", "@tailwind", "tailwind",
        // raw CSS properties / at-rules
        "@media", "@keyframes", "keyframes", "box-shadow", "border-radius",
        "linear-gradient", "backdrop-filter", "font-family", "transition:",
        "flex-direction", ":root",
        // color functions
        "rgba(", "hsl(",
    ];
    MARKERS.iter().any(|m| lower.contains(m))
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

    #[test]
    fn design_file_extensions_and_paths() {
        // extensions
        assert!(is_design_file("/p/hero.css"));
        assert!(is_design_file("/p/Button.tsx"));
        assert!(is_design_file("/p/page.svelte"));
        assert!(is_design_file("/p/views/home.blade.php"));
        assert!(is_design_file("/p/icon.svg"));
        assert!(is_design_file("/p/styles/theme.css.ts")); // vanilla-extract
        assert!(is_design_file("/p/Page.HTML")); // case-insensitive
        // path segments
        assert!(is_design_file("/p/components/Logic.ts"));
        assert!(is_design_file("/p/theme/colors.ts"));
        assert!(is_design_file("/p/.storybook/preview.ts"));
        // filenames
        assert!(is_design_file("/p/tailwind.config.js"));
        assert!(is_design_file("/p/components.json"));
        assert!(is_design_file("/p/Card.styles.ts"));
        // negatives
        assert!(!is_design_file("/p/src/api.ts"));
        assert!(!is_design_file("/p/server/db.py"));
        assert!(!is_design_file("/p/README.md"));
        assert!(!is_design_file("/p/lib/util.rs"));
    }

    #[test]
    fn content_markers_catch_hidden_design() {
        let styled = serde_json::json!({
            "tool_name": "Write",
            "tool_input": {"file_path": "/p/Widget.ts", "content": "const C = styled.div`color:red`"}
        });
        assert!(content_has_design_markers(&styled));

        let tailwind_edit = serde_json::json!({
            "tool_name": "Edit",
            "tool_input": {"file_path": "/p/x.ts", "new_string": "return <div className=\"flex\">"}
        });
        assert!(content_has_design_markers(&tailwind_edit));

        let logic = serde_json::json!({
            "tool_name": "Write",
            "tool_input": {"file_path": "/p/util.ts", "content": "export const add = (a, b) => a + b;"}
        });
        assert!(!content_has_design_markers(&logic));
    }
}
