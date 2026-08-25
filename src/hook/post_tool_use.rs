use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::Local;

use crate::config::BaseConfig;
use crate::crud;
use crate::store;

pub fn handle(config: &BaseConfig, cwd: &Path, event: &serde_json::Value) -> Result<super::HookEventData> {
    let mut data = super::HookEventData::default();

    // Folder-move nudge: a Bash `mv` of a registered project's folder must repath
    // the graph + domain trigger, or the active-state reconcile measures a ghost
    // location and the project looks "gone". Runs before the file-path early-return
    // because Bash carries no file_path.
    if event.get("tool_name").and_then(|v| v.as_str()) == Some("Bash")
        && let Some(cmd) = event
            .get("tool_input")
            .and_then(|i| i.get("command"))
            .and_then(|c| c.as_str())
    {
        emit_move_nudge(config, cwd, cmd);
    }

    let file_paths = extract_file_paths(event);
    if file_paths.is_empty() {
        return Ok(data);
    }

    // ─── lastActive timestamp update (existing behavior) ─────
    let trig_path = find_workspace_trig(cwd);
    if let Some(ref tp) = trig_path {
        let graph = store::load_graph(tp)?;
        let now = Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        // Collect the deltas that actually applied so the change log carries them.
        let mut applied: Vec<String> = Vec::new();

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
                applied.push(sparql);
            }
        }

        if !applied.is_empty() {
            // Housekeeping: `lastActive` is a per-machine usage signal, and this
            // path runs on EVERY tool call — the one place a diff must never be
            // taken. The statements are already applied above, so the seam is
            // handed a no-op closure and records the gap honestly.
            store::mutate_and_write(
                &graph,
                tp,
                "",
                store::Scope::Target,
                store::Intent::Housekeeping,
                |_| Ok(Some(applied.join(";\n"))),
            )?;
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
/// Handle `mv` of a project directory:
///   · PAUL project (moved dir has `.paul/paul.toml`) → auto-rewrite the paul.toml
///     `path` field to the new location (base ingest then self-corrects the graph).
///   · non-PAUL registered project (basename match) → emit a `repath` nudge.
/// Silent on ordinary `mv`s.
fn emit_move_nudge(config: &BaseConfig, cwd: &Path, command: &str) {
    let moves = detect_dir_moves(command);
    if moves.is_empty() {
        return;
    }
    let mut projects: Option<Vec<(String, String, String)>> = None;

    for (src, dest) in &moves {
        let Some(new_dir) = resolve_moved_dir(cwd, src, dest) else { continue };

        // PAUL project moved → keep its paul.toml path field honest, automatically.
        let paul_toml = new_dir.join(".paul").join("paul.toml");
        if paul_toml.is_file() {
            let abs = new_dir.to_string_lossy().to_string();
            if update_paul_path(&paul_toml, &abs).unwrap_or(false) {
                println!(
                    "<base:paul-path-synced>\n\
                     Moved PAUL project → {abs}\n\
                     Rewrote {} path field to the new location; base re-ingests the real \
                     directory at next session-start (ops:path self-corrects).\n\
                     </base:paul-path-synced>",
                    paul_toml.display()
                );
            }
            continue;
        }

        // Non-PAUL registered project → repath nudge (graph path is the source there).
        let Some(src_base) = basename(&strip_paul(src)) else { continue };
        let list = projects.get_or_insert_with(|| {
            crud::project::list_paths(cwd, &config.namespace).unwrap_or_default()
        });
        for (slug, name, ppath) in list.iter() {
            if basename(&strip_paul(ppath)).as_deref() == Some(src_base.as_str()) {
                println!(
                    "<base:path-drift>\n\
                     Folder moved: {src} → {dest}\n\
                     Registered project '{name}' (slug {slug}) still points at the old path: {ppath}\n\
                     Sync graph + domain trigger:  base project repath {slug} {dest}\n\
                     </base:path-drift>"
                );
            }
        }
    }
}

/// Resolve the new on-disk directory of a moved folder. `mv src dest` lands either
/// at `dest` (rename) or `dest/<basename(src)>` (move into an existing dir).
fn resolve_moved_dir(cwd: &Path, src: &str, dest: &str) -> Option<PathBuf> {
    let abs = |p: &str| {
        let pb = PathBuf::from(p);
        if pb.is_absolute() { pb } else { cwd.join(pb) }
    };
    let dest_path = abs(dest);
    let into = basename(src).map(|b| dest_path.join(b));
    match into {
        Some(d) if d.is_dir() => Some(d),
        _ if dest_path.is_dir() => Some(dest_path),
        _ => None,
    }
}

/// Rewrite the top-level `path = "..."` line in a paul.toml. Returns Ok(true) when
/// the file changed. Line-based (preserves paul's formatting + comments). Operates
/// only on the top-level keys (above the first `[table]`); inserts after `name =`
/// when no path field exists.
fn update_paul_path(paul_toml: &Path, new_dir: &str) -> std::io::Result<bool> {
    let content = std::fs::read_to_string(paul_toml)?;
    let target = format!("path = \"{new_dir}\"");

    // Phase 1: replace an existing top-level path line (stop at the first table).
    let mut out: Vec<String> = Vec::with_capacity(content.lines().count() + 1);
    let mut replaced = false;
    let mut in_tables = false;
    for line in content.lines() {
        let t = line.trim_start();
        if t.starts_with('[') {
            in_tables = true;
        }
        if !replaced && !in_tables && (t.starts_with("path =") || t.starts_with("path=")) {
            if line == target {
                return Ok(false); // already correct
            }
            out.push(target.clone());
            replaced = true;
        } else {
            out.push(line.to_string());
        }
    }

    // Phase 2: no path field existed → insert after `name =`, else before the first
    // table, else at the top — always keeping it a top-level key.
    if !replaced {
        let pos = out
            .iter()
            .position(|l| {
                let t = l.trim_start();
                t.starts_with("name =") || t.starts_with("name=")
            })
            .map(|i| i + 1)
            .or_else(|| out.iter().position(|l| l.trim_start().starts_with('[')))
            .unwrap_or(0);
        out.insert(pos, target);
    }

    let mut body = out.join("\n");
    if content.ends_with('\n') {
        body.push('\n');
    }
    let tmp = paul_toml.with_extension("toml.tmp");
    std::fs::write(&tmp, &body)?;
    std::fs::rename(&tmp, paul_toml)?;
    Ok(true)
}

/// Parse `mv` invocations from a shell command, returning (src, dest) pairs. Naive
/// by design (whitespace tokens, strips simple quotes + flags) — it only feeds a
/// nudge, so false negatives are fine and false positives are filtered by the
/// project-path match.
fn detect_dir_moves(command: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for seg in command
        .split(['\n', ';'])
        .flat_map(|s| s.split("&&"))
        .flat_map(|s| s.split("||"))
    {
        let toks: Vec<String> = seg.split_whitespace().map(unquote).collect();
        if toks.first().map(String::as_str) != Some("mv") {
            continue;
        }
        let args: Vec<&String> = toks[1..].iter().filter(|t| !t.starts_with('-')).collect();
        if args.len() < 2 {
            continue;
        }
        let dest = args[args.len() - 1].clone();
        for src in &args[..args.len() - 1] {
            out.push(((*src).clone(), dest.clone()));
        }
    }
    out
}

fn unquote(s: &str) -> String {
    s.trim_matches('"').trim_matches('\'').to_string()
}

fn strip_paul(p: &str) -> String {
    match p.find("/.paul") {
        Some(i) => p[..i].to_string(),
        None => p.to_string(),
    }
}

fn basename(p: &str) -> Option<String> {
    Path::new(p.trim_end_matches('/'))
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
}

fn find_workspace_trig(cwd: &Path) -> Option<PathBuf> {
    crate::config::walk_up(cwd, |dir| {
        let candidate = dir.join(".base").join("graph.nq");
        candidate.exists().then_some(candidate)
    })
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
