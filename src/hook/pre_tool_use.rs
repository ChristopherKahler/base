use std::path::{Path, PathBuf};

use anyhow::Result;
use oxigraph::model::TermRef;

use crate::config::BaseConfig;
use crate::crud;
use crate::domain;
use crate::domain::session::SessionState;

/// PreToolUse: see file path in tool call → match file_keywords + path triggers → inject rules BEFORE tool executes.
/// Also: inject AST file map for source files, and redirect grep/find to ast query.
///
/// Returns the injection text instead of printing it: Claude Code only feeds
/// PreToolUse context to the model via the JSON `hookSpecificOutput.additionalContext`
/// envelope (plain stdout is transcript-only), so the dispatcher assembles ALL
/// pre-tool output — this handler's plus the relay blocks — into one envelope.
pub fn handle(
    config: &BaseConfig,
    cwd: &Path,
    event: &serde_json::Value,
) -> Result<(super::HookEventData, String)> {
    let mut output = String::new();
    let mut data = super::HookEventData::default();

    // ─── Memory intercept (Write/Edit/Read on memory files) ──
    // Must be FIRST — if we intercept, we may block the tool call (exit 2).
    if let Some((message, blocked)) = crate::hook::memory::handle_memory(config, cwd, event) {
        if blocked {
            // Exit 2 = block the tool call. Stdout = reason shown to Claude.
            print!("{message}");
            eprint!("{message}");
            std::process::exit(2);
        }
        // Not blocked: print enrichment and continue (dual-write mode)
        output.push_str(&message);
        output.push('\n');
    }

    // ─── Grep/find intercept (Bash tool) ─────────────────────
    if let Some(hint) = grep_intercept(event, cwd) {
        output.push_str(&hint);
        output.push('\n');
        data.grep_intercepted = true;
    }

    // ─── Context-mode source file intercept ──────────────────
    // When context-mode (ctx_batch_execute, ctx_execute) is used to scan
    // source files, nudge toward base ast query first.
    if let Some(hint) = context_mode_intercept(event, cwd) {
        output.push_str(&hint);
        output.push('\n');
    }

    // ─── Domain rule injection (file path match) ─────────────
    let file_paths = extract_file_paths(event);
    if !file_paths.is_empty() {
        // Single SessionState lifecycle for the whole branch — domain dedup
        // marks and AST-injected marks share one instance, saved once (Q3).
        let base_dir = crate::config::find_workspace_base(cwd);
        let mut session = base_dir
            .as_deref()
            .map(SessionState::load)
            .unwrap_or_default();
        let mut session_dirty = false;

        // Track apps whose files are being edited this turn so the Stop hook can
        // refresh exactly those code maps — not just the session-cwd app. This is
        // what keeps blast-radius injection current: edits this turn → map refresh
        // on Stop → next touch sees the updated call graph.
        if is_mutating_tool(event) {
            for fp in &file_paths {
                if let Some(root) = crate::config::ast_app_root(fp).and_then(|r| r.to_str().map(String::from))
                    && session.mark_dirty_app(&root)
                {
                    session_dirty = true;
                }
            }
        }

        let domains = domain::load_domains(cwd);
        let file_path_strings: Vec<String> = file_paths
            .iter()
            .filter_map(|p| p.to_str().map(String::from))
            .collect();
        let matched = match_by_file(&domains, &file_path_strings);

        // Sync BEFORE the single graph load so the store sees fresh rules.
        if !matched.is_empty() {
            crate::hook::user_prompt_submit::ensure_domain_sync_pub(config, cwd);
        }

        // Single graph load per invocation — domain injection and PAUL
        // context both read from this store (Q2).
        let graph_store = crate::store::load_merged(cwd);

        for domain_def in &matched {
            // Session dedup: skip if this domain's rules were already injected.
            // Hash the rendered rules (text + rationale) so a rationale edit re-injects.
            let rules_hash = domain::session::rules_hash(&domain_def.rendered_rules());
            if session.is_injected(&domain_def.name, rules_hash) {
                data.suppressed += 1;
                continue;
            }

            let rules_text = match &graph_store {
                Some(store) => query_rules_from_graph(store, config, domain_def),
                None => format_toml_rules(domain_def),
            };

            // Query-triggered injection for filepath-matched domains
            let query_text = match (&graph_store, &domain_def.query) {
                (Some(store), Some(query_name)) => {
                    let fmt = domain_def.query_format.as_deref().unwrap_or("list");
                    crate::domain::query::resolve_and_run_query(
                        store, config, cwd, query_name, fmt, &domain_def.name,
                    )
                }
                _ => String::new(),
            };

            if !rules_text.is_empty() || !query_text.is_empty() {
                if !rules_text.is_empty() {
                    output.push_str(&rules_text);
                    output.push('\n');
                }
                if !query_text.is_empty() {
                    output.push_str(&query_text);
                    output.push('\n');
                }
                data.domains_matched.push(domain_def.name.clone());
                data.rules_injected += rules_text.lines().filter(|l| l.starts_with("  ")).count();
                session.mark_injected(&domain_def.name, rules_hash);
                session_dirty = true;
            }
        }

        // ─── Markdown authoring guidance (Write/Edit on .md) ─────
        let tool_name = event
            .get("tool_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if tool_name == "Write" || tool_name == "Edit" {
            for fp in &file_paths {
                if fp.to_str().is_some_and(|s| s.ends_with(".md")) {
                    output.push_str(MARKDOWN_GUIDANCE);
                    output.push('\n');
                    break;
                }
            }
        }

        // ─── File-touch context injection ───────────────────────
        // Touch a file under an app root → inject targeted context about it:
        //   source files → AST map + call neighborhood (what it calls / calls it)
        //   docs (.md)    → the semantic concepts the graph derived from it
        // Content-keyed dedup re-injects only when the file changed — a stale map
        // is worse than none while you're actively editing.
        for fp in &file_paths {
            if let Some(fp_str) = fp.to_str() {
                let is_src = is_source_file(fp_str);
                let is_doc = is_doc_file(fp_str);
                if !(is_src || is_doc) {
                    continue;
                }
                let ver = file_version(fp);
                if session.has_ast_injected(fp_str, ver) {
                    continue;
                }
                let mut block = String::new();
                if is_src
                    && let Some(map) = crud::ast_query::file_map_compact(cwd, &config.namespace, fp_str) {
                        block.push_str(&map);
                        block.push('\n');
                    }
                if is_doc {
                    let fname = std::path::Path::new(fp_str)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(fp_str);
                    let concepts = crud::semantic::concepts_for_file(cwd, &config.namespace, fname);
                    if !concepts.is_empty() {
                        block.push_str(&format!("[Concepts] {fname} — {} linked\n", concepts.len()));
                        for (name, summary) in concepts.iter().take(8) {
                            if summary.is_empty() {
                                block.push_str(&format!("  {name}\n"));
                            } else {
                                block.push_str(&format!("  {name} — {summary}\n"));
                            }
                        }
                    }
                }
                if !block.is_empty() {
                    output.push_str(&block);
                    session.mark_ast_injected(fp_str, ver);
                    session_dirty = true;
                    data.ast_injected = true;
                }
            }
        }

        // ─── PAUL context injection (file change history) ───────
        // When editing a file that has FileChange/Decision history in the
        // graph, surface the decisions and changes that shaped it.
        if let Some(store) = &graph_store {
            for fp in &file_paths {
                if let Some(fp_str) = fp.to_str() {
                    let paul_ctx = query_paul_context(store, config, fp_str);
                    if !paul_ctx.is_empty() {
                        output.push_str(&paul_ctx);
                        output.push('\n');
                    }
                }
            }
        }

        // ─── Extension pre-tool triggers ────────────────────────
        let extensions = crate::extension::load_extensions();
        for ext in &extensions {
            if let Some(hooks) = &ext.hooks
                && let Some(pre) = &hooks.pre_tool
            {
                for trigger in &pre.triggers {
                    for fp in &file_paths {
                        let fp_str = fp.to_string_lossy();
                        if trigger.paths.iter().any(|p| fp_str.contains(p.as_str())) {
                            output.push_str(&format!("\n{}", trigger.inject));
                        }
                    }
                }
            }
        }

        // ─── Standards injection (MIDAS best practices) ─────────
        // Mutating tools only, injected LAST so the standards are the most
        // recent thing read before the edit lands — top-of-awareness. The
        // matcher scores each standard against the file's language, path,
        // semantic classes, and the edit payload itself; budget-capped so
        // injection stays scarce enough to be applied rather than skimmed.
        if is_mutating_tool(event) && config.standards.enabled {
            let payload = crate::standards::edit_payload(event);
            for fp in &file_paths {
                if let Some(block) =
                    crate::standards::inject_for_file(config, cwd, fp, &payload, &mut session)
                {
                    data.standards_injected += block.lines().filter(|l| l.starts_with("  ")).count();
                    output.push_str(&block);
                    output.push('\n');
                    session_dirty = true;
                }
            }
        }

        // Single save for the whole branch — only when something changed.
        if session_dirty
            && let Some(bd) = base_dir.as_deref() {
                let _ = session.save(bd);
            }
    }

    let context = output.trim_end().to_string();
    Ok((data, context))
}

/// Content-version of a file for content-keyed dedup: a hash of its bytes (0 if
/// unreadable). Changes whenever the file changes, so the AST map re-injects after
/// an edit instead of staying stale for the rest of the session.
fn file_version(path: &std::path::Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let Ok(bytes) = std::fs::read(path) else { return 0 };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// Whether the tool call mutates a file (so its app's code map should be
/// refreshed on Stop). Read/Grep/etc. don't change code, so they don't dirty.
fn is_mutating_tool(event: &serde_json::Value) -> bool {
    matches!(
        event.get("tool_name").and_then(|v| v.as_str()),
        Some("Edit" | "Write" | "MultiEdit" | "NotebookEdit")
    )
}

/// Doc files that may carry graph-extracted semantic concepts.
fn is_doc_file(path: &str) -> bool {
    let p = path.to_lowercase();
    p.ends_with(".md") || p.ends_with(".markdown")
}

/// Check if a file path is a source code file worth AST injection.
fn is_source_file(path: &str) -> bool {
    let exts = [
        ".rs", ".py", ".js", ".ts", ".go", ".jsx", ".tsx", ".c", ".cpp", ".h", ".hpp",
        ".java", ".rb", ".swift", ".kt", ".kts", ".scala", ".php", ".cs", ".lua", ".zig",
        ".ps1", ".ex", ".exs", ".jl", ".vue", ".svelte", ".astro", ".dart", ".sql", ".r",
        ".f90", ".pas", ".sh", ".bash", ".json", ".toml", ".yaml", ".yml",
    ];
    exts.iter().any(|ext| path.ends_with(ext))
}

/// Check if AST data has been extracted for the current workspace.
/// ast.ttl IS the AST store (never merged into graph.nq — AUDIT C10),
/// so its existence is the correct populated check.
///
/// Resolution MUST go through `find_ast_ttl`, which checks the `.base-ast/`
/// sidecar before the legacy `{ws}/.base/ast.ttl`. Hand-joining the legacy path
/// here made this check unsatisfiable for every workspace mapped after the
/// sidecar migration, so the "not yet populated" hint fired forever no matter
/// how many times `base sync --ast` was run.
fn ast_graph_populated(cwd: &Path) -> bool {
    match crate::config::find_ast_ttl(cwd) {
        Some(ast_path) => std::fs::metadata(&ast_path).map(|m| m.len() > 0).unwrap_or(false),
        None => false,
    }
}

/// Detect grep/find/rg in Bash commands and suggest ast query instead.
fn grep_intercept(event: &serde_json::Value, cwd: &Path) -> Option<String> {
    let tool_name = event.get("tool_name").and_then(|v| v.as_str())?;
    if tool_name != "Bash" {
        return None;
    }

    let command = event
        .get("tool_input")
        .and_then(|ti| ti.get("command"))
        .and_then(|v| v.as_str())?;

    // Intercept code search patterns (grep, rg, ag, ack, fd, find)
    let is_code_search = command.starts_with("grep -r")
        || command.starts_with("grep -rn")
        || command.starts_with("grep -n")
        || command.starts_with("grep -l")
        || command.starts_with("grep -rl")
        || command.contains("| grep")
        || command.starts_with("rg ")
        || command.starts_with("ag ")
        || command.starts_with("ack ")
        || command.starts_with("fd ")
        || (command.starts_with("find ") && command.contains("-name"));

    if !is_code_search {
        return None;
    }

    // Try to extract the search term
    let search_term = extract_search_term(command);

    // Check if AST graph is populated — different message if not
    if !ast_graph_populated(cwd) {
        return Some(
            "<ast-hint>\n\
             AST graph not yet populated for this workspace.\n\
             Would you like to index the codebase? Run:\n\
               base sync --ast\n\
             This takes ~10 seconds and indexes 35+ languages.\n\
             Then use `base ast query` for code navigation instead of grep/find.\n\
             </ast-hint>"
                .to_string(),
        );
    }

    let suggestion = if let Some(term) = search_term {
        format!(
            "<ast-hint>\n\
             AST graph available for this workspace. Try:\n\
               base ast query --contains \"{term}\"\n\
             The graph knows file locations, line numbers, and call relationships.\n\
             </ast-hint>"
        )
    } else {
        "<ast-hint>\n\
         AST graph available for this workspace. Try `base ast query` for code navigation.\n\
         Modes: --contains <name>, --file <path>, --calls <name>, --imports <path>\n\
         </ast-hint>"
            .to_string()
    };

    Some(suggestion)
}

/// Best-effort extraction of search term from grep/rg/find commands.
fn extract_search_term(command: &str) -> Option<String> {
    let parts: Vec<&str> = command.split_whitespace().collect();

    // grep -r "term" or grep -rn "term"
    if parts.first().map(|s| *s == "grep").unwrap_or(false) {
        for part in parts.iter() {
            // Skip flags
            if part.starts_with('-') {
                continue;
            }
            // Skip "grep" itself
            if *part == "grep" {
                continue;
            }
            // First non-flag, non-grep token is the pattern
            let term = part.trim_matches('"').trim_matches('\'');
            if !term.is_empty() && !term.starts_with('/') && !term.starts_with('.') {
                return Some(term.to_string());
            }
        }
    }

    // rg "term"
    if parts.first().map(|s| *s == "rg").unwrap_or(false)
        && let Some(term) = parts.get(1) {
            let t = term.trim_matches('"').trim_matches('\'');
            if !t.starts_with('-') {
                return Some(t.to_string());
            }
        }

    None
}

/// Detect context-mode MCP tools scanning source files and nudge toward base ast query.
/// Catches ctx_batch_execute and ctx_execute when commands reference source file patterns.
fn context_mode_intercept(event: &serde_json::Value, cwd: &Path) -> Option<String> {
    let tool_name = event.get("tool_name").and_then(|v| v.as_str())?;

    // Match context-mode MCP tool names (plugin-namespaced)
    let is_ctx_tool = tool_name.contains("ctx_batch_execute")
        || tool_name.contains("ctx_execute")
        || tool_name.contains("ctx_execute_file");

    if !is_ctx_tool {
        return None;
    }

    // Check if the tool input references source files
    let input = event.get("tool_input")?;
    let input_str = serde_json::to_string(input).unwrap_or_default();

    // Look for source file extensions in the command/query text
    let has_source_refs = [".rs", ".py", ".js", ".ts", ".go", ".tsx", ".jsx", ".vue", ".svelte"]
        .iter()
        .any(|ext| input_str.contains(ext));

    // Also catch common code navigation commands
    let has_nav_commands = ["cat ", "head ", "tail ", "find ", "grep ", "ls src", "ls ./src"]
        .iter()
        .any(|cmd| input_str.contains(cmd));

    if !has_source_refs && !has_nav_commands {
        return None;
    }

    if !ast_graph_populated(cwd) {
        return Some(
            "<ast-hint>\n\
             AST graph not yet populated for this workspace.\n\
             Would you like to index the codebase? Run:\n\
               base sync --ast\n\
             This takes ~10 seconds and indexes 35+ languages.\n\
             Then use `base ast query` for code navigation instead of scanning files.\n\
             </ast-hint>"
                .to_string(),
        );
    }

    Some(
        "<ast-hint>\n\
         BASE AST graph available. Before scanning source files, use:\n\
           base ast query --file \"<filename>\"     (entity map for a file)\n\
           base ast query --contains \"<name>\"     (find entities by name)\n\
           base ast query --calls \"<function>\"     (call chain)\n\
         The graph already knows the codebase structure — scan after, not before.\n\
         </ast-hint>"
            .to_string(),
    )
}

/// Match domains by file path triggers and file_keywords against file content.
fn match_by_file<'a>(
    domains: &'a [domain::DomainDef],
    file_paths: &[String],
) -> Vec<&'a domain::DomainDef> {
    domains
        .iter()
        .filter(|d| {
            // Skip always-on (those fire on user-prompt-submit, not here)
            if d.is_always() {
                return false;
            }

            // Path match: any file path starts with or contains a domain path trigger
            let path_hit = d.paths.iter().any(|dp| {
                file_paths
                    .iter()
                    .any(|fp| fp.starts_with(dp) || fp.contains(dp))
            });

            // File keyword match: check if any file_keywords appear in the file paths
            // (lightweight — full content scan would require reading the file)
            let file_kw_hit = d.file_keywords.iter().any(|kw| {
                file_paths
                    .iter()
                    .any(|fp| fp.to_lowercase().contains(&kw.to_lowercase()))
            });

            path_hit || file_kw_hit
        })
        .collect()
}

/// Query rules for a domain from the graph. Returns formatted text.
fn query_rules_from_graph(
    store: &oxigraph::store::Store,
    config: &BaseConfig,
    domain_def: &domain::DomainDef,
) -> String {
    let ns = &config.namespace;
    let p = &ns.prefix;
    let domain_slug = crud::slugify(&domain_def.name);
    let domain_iri = crud::build_iri(ns, "domain", &domain_slug);
    let pfx = crud::prefixes(ns);

    let sparql = format!(
        "{pfx}\n\
         SELECT ?text ?rationale WHERE {{\n\
           GRAPH ?g {{\n\
             <{domain_iri}> {p}:hasRule ?rule .\n\
             ?rule {p}:ruleText ?text .\n\
             OPTIONAL {{ ?rule {p}:priority ?pri }}\n\
             OPTIONAL {{ ?rule {p}:rationale ?rationale }}\n\
           }}\n\
         }}\n\
         ORDER BY ?pri"
    );

    match crate::store::query(store, &sparql) {
        Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) => {
            let rules: Vec<String> = solutions
                .filter_map(|r| r.ok())
                .filter_map(|row| {
                    let text = match row.get("text")?.into() {
                        TermRef::Literal(l) => l.value().to_string(),
                        _ => return None,
                    };
                    if text.is_empty() {
                        return None;
                    }
                    let rationale = row.get("rationale").and_then(|t| match t.into() {
                        TermRef::Literal(l) => {
                            let v = l.value().to_string();
                            (!v.is_empty()).then_some(v)
                        }
                        _ => None,
                    });
                    Some(domain::render_rule(&text, rationale.as_deref()))
                })
                .collect();

            if rules.is_empty() {
                format_toml_rules(domain_def)
            } else {
                let mut out = format!("[FILE MATCH: {}]\n", domain_def.name);
                for (i, rule) in rules.iter().enumerate() {
                    out.push_str(&format!("  {i}. {rule}\n"));
                }
                out
            }
        }
        _ => format_toml_rules(domain_def),
    }
}

fn format_toml_rules(domain_def: &domain::DomainDef) -> String {
    if domain_def.rules.is_empty() {
        return String::new();
    }
    let mut out = format!("[FILE MATCH: {}]\n", domain_def.name);
    for (i, rule) in domain_def.rules.iter().enumerate() {
        out.push_str(&format!("  {i}. {}\n", rule.render()));
    }
    out
}

/// Query PAUL FileChange and Decision entities linked to a file path.
/// Returns formatted context string for hook injection.
fn query_paul_context(store: &oxigraph::store::Store, config: &BaseConfig, file_path: &str) -> String {
    let ns = &config.namespace;
    let p = &ns.prefix;
    let pfx = crud::prefixes(ns);

    // Normalize: strip leading ./ and match against filePath values
    let clean = file_path.trim_start_matches("./");

    // Query file changes that reference this path
    let fc_sparql = format!(
        "{pfx}\n\
         SELECT ?plan ?change ?purpose WHERE {{\n\
           GRAPH ?g {{\n\
             ?fc a {p}:FileChange ;\n\
                 {p}:filePath ?path ;\n\
                 {p}:fromPlan ?plan ;\n\
                 {p}:changeType ?change .\n\
             OPTIONAL {{ ?fc {p}:purpose ?purpose }}\n\
             FILTER(CONTAINS(STR(?path), \"{clean}\"))\n\
           }}\n\
         }} LIMIT 5"
    );

    let mut changes = Vec::new();
    if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&fc_sparql) {
        for row in solutions.flatten() {
            let plan = row.get("plan").map(|t| term_str(t.into())).unwrap_or_default();
            let change = row.get("change").map(|t| term_str(t.into())).unwrap_or_default();
            let purpose = row.get("purpose").map(|t| term_str(t.into())).unwrap_or_default();
            changes.push((plan, change, purpose));
        }
    }

    if changes.is_empty() {
        return String::new();
    }

    // For each plan that touched this file, get its decisions
    let mut plans: Vec<String> = changes.iter().map(|(p, _, _)| p.clone()).collect();
    plans.sort();
    plans.dedup();

    let mut decisions = Vec::new();
    for plan_id in &plans {
        let dec_sparql = format!(
            "{pfx}\n\
             SELECT ?desc ?rationale WHERE {{\n\
               GRAPH ?g {{\n\
                 ?d a {p}:Decision ;\n\
                    {p}:fromPlan \"{plan_id}\" ;\n\
                    {p}:description ?desc ;\n\
                    {p}:rationale ?rationale .\n\
               }}\n\
             }} LIMIT 5"
        );
        if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&dec_sparql) {
            for row in solutions.flatten() {
                let desc = row.get("desc").map(|t| term_str(t.into())).unwrap_or_default();
                let rationale = row.get("rationale").map(|t| term_str(t.into())).unwrap_or_default();
                decisions.push((plan_id.clone(), desc, rationale));
            }
        }
    }

    // Format output
    let mut out = String::from("<paul-context>\n");
    out.push_str(&format!("File history for: {clean}\n"));
    for (plan, change, purpose) in &changes {
        out.push_str(&format!("  Plan {plan}: {change}"));
        if !purpose.is_empty() {
            out.push_str(&format!(" — {purpose}"));
        }
        out.push('\n');
    }
    if !decisions.is_empty() {
        out.push_str("Decisions:\n");
        for (plan, desc, rationale) in &decisions {
            out.push_str(&format!("  [{plan}] {desc} — {rationale}\n"));
        }
    }
    out.push_str("</paul-context>");
    out
}

fn term_str(term: oxigraph::model::TermRef<'_>) -> String {
    match term {
        TermRef::Literal(l) => l.value().to_string(),
        TermRef::NamedNode(n) => n.as_str().to_string(),
        _ => term.to_string(),
    }
}

const MARKDOWN_GUIDANCE: &str = "\
<mop-markdown>
This markdown file feeds a knowledge graph. Structure it for extraction:

FRONTMATTER (between --- delimiters):
  type: doc|decision|note|spec|plan|summary
  status: draft|active|complete|archived
  tags: [specific, searchable, terms]
  relatedTo: [entity-slug-1, entity-slug-2]

BODY PATTERNS (extracted as graph edges — use intentionally):
  ## Headings        → hasSection edges (document structure + search)
  [text](path.md)    → references edges to other documents
  [[entity-name]]    → references edges to named entities
  @path/to/file      → references edges to documents
  Tags become individual graph edges — be specific, not generic
  relatedTo links to real entity slugs — check existing entities
</mop-markdown>";

fn extract_file_paths(event: &serde_json::Value) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(fp) = event
        .get("tool_input")
        .and_then(|ti| ti.get("file_path"))
        .and_then(|v| v.as_str())
    {
        paths.push(PathBuf::from(fp));
    }
    if let Some(fp) = event
        .get("tool_input")
        .and_then(|ti| ti.get("path"))
        .and_then(|v| v.as_str())
    {
        paths.push(PathBuf::from(fp));
    }
    paths
}
