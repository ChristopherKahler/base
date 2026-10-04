use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use oxigraph::model::TermRef;

use crate::config::BaseConfig;
use crate::crud;
use crate::domain;
use crate::domain::session::SessionState;
use crate::emit::match_log::{Item, Matched, Trace};

/// PreToolUse: see file path in tool call → match file_keywords + path triggers → inject rules BEFORE tool executes.
/// Also: inject AST file map for source files, and point a code search at the code map that covers it (F20).
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
    handle_traced(config, cwd, event).map(|(data, context, _, _)| (data, context))
}

/// [`handle`], and what the call matched and served, for the match log (K1, BO-13): the touched paths, each domain
/// they brought in and by what, and every rule printed. The dispatcher writes the row after the output is printed.
/// While a shadow runs (BO-20), also what the candidate decides on, which the dispatcher hands it after the print.
pub fn handle_traced(
    config: &BaseConfig,
    cwd: &Path,
    event: &serde_json::Value,
) -> Result<(super::HookEventData, String, Trace, Option<FileKept>)> {
    let mut output = String::new();
    let mut data = super::HookEventData::default();
    let mut trace = Trace { tool: event.get("tool_name").and_then(|v| v.as_str()).map(String::from), ..Trace::default() };

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

    // ─── Bash first contact ──────────────────────────────────
    // A session booted in a workspace navigates with the shell: `cd app &&
    // cat src/x` names paths the tool_input never does. Existing paths in
    // the command (and cd targets) are contact; a `wsl …` command from
    // Windows hands the same check to the WSL base, which alone can see
    // a Linux path.
    if event.get("tool_name").and_then(|v| v.as_str()) == Some("Bash")
        && let Some(cmd) = event.get("tool_input").and_then(|t| t.get("command")).and_then(|v| v.as_str())
    {
        let home = crate::home::home_root();
        for p in crate::hook::automap::bash_paths(cmd, cwd, home.as_deref()) {
            if p.exists() {
                // Marked roots only — a `cd` through a folder is not the same
                // choice as booting a session in it. See `bash_first_contact`.
                crate::hook::automap::bash_first_contact(&p);
            }
        }
        crate::hook::automap::delegate_wsl_contact(&crate::hook::automap::linux_paths(cmd));
    }

    let file_paths = extract_file_paths(event);
    // P1: every path this call touches, absolute. The tool's own paths above feed the code maps, the standards and
    // the rest exactly as before; this list decides which project's rules come with the call.
    let named = touched_paths(event, cwd, crate::home::home_root().as_deref(), &file_paths);
    // A Bash or PowerShell command that names no path touches the session's folder (Example 7), for the project and
    // trigger match only: place rules and file keywords keep to what the call names.
    let touched: Vec<String> = if named.is_empty() && tool_command(event).is_some() {
        vec![cwd.display().to_string()]
    } else {
        named.clone()
    };
    trace.paths = touched.clone();
    // Single SessionState lifecycle for the whole hook — rule marks, domain dedup
    // marks and AST-injected marks share one instance, saved once at the end (Q3).
    let base_dir = crate::config::find_workspace_base(cwd);
    let mut session = base_dir
        .as_deref()
        .map(SessionState::load)
        .unwrap_or_default();
    let mut session_dirty = false;

    // ─── AST hint (F20) ──────────────────────────────────────
    // A code search through Bash, PowerShell or context-mode is pointed at the code map that covers the folder it
    // searches; nothing else is. The rules are in `ast_hint`. After Bash first contact above, so a no-map hint reads
    // the build that contact just started.
    let hint = crate::hook::ast_hint::hint(event, cwd, &mut session);
    if let Some(text) = hint.text {
        output.push_str(&text);
        output.push('\n');
        data.grep_intercepted = true;
    }
    session_dirty |= hint.marked;

    let domains = domain::load_domains(cwd);
    // Sync BEFORE the single graph load so the store sees fresh rules. Marker-gated,
    // a no-op when fresh; it ran only for a matched domain until F29, and the match
    // now needs the store (the registered projects decide which triggers are live).
    crate::hook::user_prompt_submit::ensure_domain_sync_pub(config, cwd);

    // Single graph load per invocation — rule serving, domain injection and PAUL
    // context all read from this store (Q2).
    let graph_store = crate::store::load_merged(cwd);

    // The bracket tier this hook is serving at, READ and never incremented: the
    // prompt hook owns the counter, and a tool call must not advance a session's
    // depth. A tier change re-serves the rules now in force, once, so the record
    // has to know which tier it was told them at.
    //
    // And it is the tier the PROMPT hook last computed (`petrel` FINDING 1), never one this hook derives
    // on its own. The prompt hook reads the transcript's percentage; this event carries none, and a tier
    // taken from the prompt count disagreed with it in percent mode, so every switch between the two hooks
    // served the same rules again.
    let tier = session.served_tier(
        &config.bracket,
        event.get("session_id").and_then(|v| v.as_str()),
    );

    // ─── Rule serving: rules with matchers of their own, and the touched file's project ──
    // One function, `serve_file` (BO-20, K9b), so a shadow candidate decides exactly as live does. What it read is kept
    // in a `FileWorld`, and the candidate runs on it after the print, on the session as it was before this call.
    let converted = domain::rules::rules_with_matchers(graph_store.as_ref(), config, &domains);
    let trigger_ctx = (!touched.is_empty()).then(|| domain::matcher::TriggerContext {
        home: crate::home::home_root().map(|h| h.display().to_string()),
        registered: graph_store
            .as_ref()
            .map(|s| domain::registered_projects(s, &config.namespace, cwd))
            .unwrap_or_default(),
    });
    let world = FileWorld {
        store: graph_store,
        domains,
        converted,
        tier,
        tool: event.get("tool_name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        command: tool_command(event).map(String::from),
        cwd: cwd.to_path_buf(),
        named: named.clone(),
        touched: touched.clone(),
        trigger_ctx,
        rules: Default::default(),
    };
    let shadow = crate::shadow::active();
    let shadow_session = shadow.as_ref().map(|_| session.clone());
    let served = serve_file(config, &world, &FileView::live(&world), &mut session, None).unwrap_or_default();
    if served.select_count > 0 {
        output.push_str(&served.select_text);
        data.rules_injected += served.select_count;
        session_dirty = true;
    }
    trace.served.extend(served.select_served.iter().cloned());
    trace.cut.extend(served.select_cut.iter().cloned());

    if !file_paths.is_empty() {
        // Track apps whose files are being edited this turn so the Stop hook can
        // refresh exactly those code maps — not just the session-cwd app. This is
        // what keeps blast-radius injection current: edits this turn → map refresh
        // on Stop → next touch sees the updated call graph.
        if is_mutating_tool(event) {
            for fp in &file_paths {
                if let Some(root) = crate::config::ast_app_root(fp).and_then(|r| r.to_str().map(String::from)) {
                    // Twice, deliberately: the cwd-scoped copy the Stop hook
                    // has always drained, and the global-tier copy it drains
                    // as of 0.13.8 — because cwd can differ between this call
                    // and the Stop that follows it (see mark_dirty_app_global).
                    if session.mark_dirty_app(&root) {
                        session_dirty = true;
                    }
                    SessionState::mark_dirty_app_global(&root);
                }
            }
        }

        // First contact builds: the first Read / Edit / Grep of a file inside
        // an app with no code map yet starts one, whatever the session's cwd.
        // A session opened at home that only READ a project used to leave it
        // unmapped until something edited it. One stat per path once mapped.
        for fp in &file_paths {
            if let Some(root) = crate::config::ast_app_root(fp) {
                crate::hook::automap::ensure_first_map(&root);
            }
        }
    }

    // ─── Domain rule injection: the touched file's project (P2, D13) ─────
    trace.matched.extend(served.matched.iter().cloned());
    for fd in &served.domains {
        let domain_def = &fd.def;
        if fd.fresh.is_empty() && fd.all > 0 {
            data.suppressed += 1;
            continue;
        }
        if !fd.fresh.is_empty() {
            session_dirty = true;
        }
        trace.served.extend(fd.fresh.iter().map(|(_, r)| Item::rule(&r.id, &domain_def.name)));
        let fresh: Vec<(usize, &domain::rules::ServedRule)> = fd.fresh.iter().map(|(i, r)| (*i, r)).collect();
        let rules_text = domain::rules::render_block_as("FILE MATCH", &fd.label, &fresh, fd.all, &domain_def.name);
        {
            // Query-triggered injection for filepath-matched domains
            let mut query_text = match (&world.store, &domain_def.query) {
                (Some(store), Some(query_name)) => {
                    let fmt = domain_def.query_format.as_deref().unwrap_or("list");
                    crate::domain::query::resolve_and_run_query(
                        store, config, cwd, query_name, fmt, &domain_def.name,
                    )
                }
                _ => String::new(),
            };
            // Once per session for the same output, as the rules are: with the session's folder standing in for a
            // command that names no path, every shell call in a project would otherwise print it again.
            if !query_text.is_empty() {
                let key = format!("pre-tool-query:{}", domain_def.name);
                let hash = domain::rules::content_hash(&query_text);
                if session.is_injected(&key, hash) {
                    query_text.clear();
                } else {
                    session.mark_injected(&key, hash);
                    session_dirty = true;
                }
            }

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
                data.rules_injected += fresh.len();
                session_dirty = true;
            }
        }
    }

    if !file_paths.is_empty() {
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
        if let Some(store) = &world.store {
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

    }

    // Single save for the whole hook — only when something changed.
    if session_dirty
        && let Some(bd) = base_dir.as_deref() {
            let _ = session.save(bd);
        }

    let context = output.trim_end().to_string();
    // What a shadow candidate decides on after the print (BO-20): only while one runs.
    let live: std::collections::BTreeSet<String> =
        trace.served.iter().filter(|i| i.kind == "rule").map(|i| i.id.clone()).collect();
    let kept = shadow.zip(shadow_session).map(|(active, session)| FileKept { active, world, session, live });
    Ok((data, context, trace, kept))
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

// ─── The rule serving, once (BO-20, K9b) ─────────────────────────

/// What a tool call's rule serving reads: the store, the domains, the rules with matchers of their own, the tier, the
/// tool and the paths. Kept while a shadow runs, so the candidate decides on the very same data after the print.
pub struct FileWorld {
    pub store: Option<oxigraph::store::Store>,
    pub domains: Vec<domain::DomainDef>,
    pub converted: Vec<domain::rules::Converted>,
    pub tier: domain::session::Bracket,
    pub tool: String,
    pub command: Option<String>,
    pub cwd: PathBuf,
    /// Every path the call names.
    pub named: Vec<String>,
    /// The paths the project match reads: `named`, or the session's folder for a command that names none.
    pub touched: Vec<String>,
    /// `None` when the call touched nothing.
    pub trigger_ctx: Option<domain::matcher::TriggerContext>,
    /// Each domain's rules, read the first time a run asks.
    pub rules: std::cell::RefCell<HashMap<String, std::rc::Rc<Vec<domain::rules::ServedRule>>>>,
}

impl FileWorld {
    /// `d`'s rules (`rules_for_domain`), read once.
    pub fn rules_of(&self, config: &BaseConfig, d: &domain::DomainDef) -> std::rc::Rc<Vec<domain::rules::ServedRule>> {
        if let Some(r) = self.rules.borrow().get(&d.name) {
            return std::rc::Rc::clone(r);
        }
        let r = std::rc::Rc::new(domain::rules::rules_for_domain(self.store.as_ref(), config, d));
        self.rules.borrow_mut().insert(d.name.clone(), std::rc::Rc::clone(&r));
        r
    }
}

/// What a run serves with: live's domains and rules, or a proposals candidate's copies with their changes made.
pub struct FileView<'a> {
    pub domains: std::borrow::Cow<'a, [domain::DomainDef]>,
    pub converted: std::borrow::Cow<'a, [domain::rules::Converted]>,
    /// By domain name: the ids of the rules the changes take away, and the rules they add.
    pub rule_edits: HashMap<String, (HashSet<String>, Vec<domain::rules::ServedRule>)>,
}

impl<'a> FileView<'a> {
    pub fn live(w: &'a FileWorld) -> Self {
        FileView {
            domains: std::borrow::Cow::Borrowed(&w.domains),
            converted: std::borrow::Cow::Borrowed(&w.converted),
            rule_edits: HashMap::new(),
        }
    }
}

/// What a tool call's rule serving decided: `select`'s rules (rendered, logged and cut) and each matched domain's.
#[derive(Default)]
pub struct FileServed {
    pub select_text: String,
    pub select_count: usize,
    pub select_served: Vec<Item>,
    pub select_cut: Vec<crate::emit::match_log::Cut>,
    pub matched: Vec<Matched>,
    pub domains: Vec<FileDomain>,
}

impl FileServed {
    /// The rule ids it would print.
    pub fn printed(&self) -> std::collections::BTreeSet<String> {
        let mut out: std::collections::BTreeSet<String> =
            self.select_served.iter().filter(|i| i.kind == "rule").map(|i| i.id.clone()).collect();
        out.extend(self.domains.iter().flat_map(|d| d.fresh.iter().map(|(_, r)| r.id.clone())));
        out
    }
}

/// One domain a tool call's paths brought in, and its rules due now.
pub struct FileDomain {
    pub def: domain::DomainDef,
    /// Its name, and `(parent of <child>)` when it came as a nested parent (D13).
    pub label: String,
    /// The rules not yet shown this session, each with its place in the domain's list.
    pub fresh: Vec<(usize, domain::rules::ServedRule)>,
    /// How many rules the domain serves through its trigger.
    pub all: usize,
}

/// What a running shadow candidate decides on after the tool hook's print (BO-20): the world, the session as it was
/// before live's serving recorded anything in it, and the rule ids live printed.
pub struct FileKept {
    pub active: crate::shadow::Active,
    pub world: FileWorld,
    pub session: SessionState,
    pub live: std::collections::BTreeSet<String>,
}

/// A tool call's rule serving (K9b's one function for the tool hook): the rules with matchers of their own `select`
/// serves for this call (F4, F5; A2, A3), then the domains the touched paths bring (P2, D13), each with its rules not
/// shown this session. Records in `session` as it decides, as `select` and `claim_rule` always have; live passes its
/// session, a shadow candidate a copy. Past `deadline` it stops before reading a domain's rules ([`Slow`]).
///
/// [`Slow`]: crate::hook::user_prompt_submit::Slow
pub fn serve_file(
    config: &BaseConfig,
    w: &FileWorld,
    view: &FileView<'_>,
    session: &mut SessionState,
    deadline: crate::hook::user_prompt_submit::Deadline,
) -> std::result::Result<FileServed, crate::hook::user_prompt_submit::Slow> {
    let mut out = FileServed::default();
    let converted: &[domain::rules::Converted] = &view.converted;
    let converted_ids: HashSet<&str> = converted.iter().map(|c| c.rule.id.as_str()).collect();
    // Every tool call reaches this, not only one that names a file: a Bash, PowerShell or MCP call carries no
    // `file_path`, and that is where action rules fire (A3). Place rules match the tool's file path and every
    // path its command names (F4). A rule with matchers of its own is served here and never through its domain's
    // trigger (F1); a rule with none stays on the domain path below, exactly as before (K4, `auk`'s HARD RULE).
    if !converted.is_empty() {
        let home = crate::home::home_root();
        let home_str = home.as_ref().map(|h| h.display().to_string());
        // The touched paths (P1), plus every path-shaped word of the command whether it exists or not, which is what
        // a place rule matched on before 0.16.0.
        let mut paths: Vec<String> = w.named.clone();
        if let Some(cmd) = w.command.as_deref() {
            for p in crate::hook::automap::bash_paths(cmd, &w.cwd, home.as_deref()) {
                if let Some(s) = p.to_str()
                    && !paths.iter().any(|x| x == s)
                {
                    paths.push(s.to_string());
                }
            }
        }
        let keywords = HashMap::new();
        let cx = domain::rules::SelectContext {
            bracket: w.tier,
            now: SessionState::now_secs(),
            home: home_str.as_deref(),
            keywords: &keywords,
            rules: &config.rules,
            bm25: None,
        };
        let rule_event = domain::rules::Event::PreTool { tool: &w.tool, paths: &paths, command: w.command.as_deref() };
        let selection = domain::rules::select(converted, &rule_event, session, &cx);
        if !selection.served.is_empty() {
            out.select_text = domain::rules::render_selection(&selection);
            out.select_count = selection.served.len();
        }
        out.select_served = domain::rules::served_items(&selection.served);
        out.select_cut = domain::rules::cut_items(&selection);
    }
    let Some(trigger_ctx) = w.trigger_ctx.as_ref() else { return Ok(out) };
    let matched = match_by_file(&view.domains, &w.touched, &w.named, trigger_ctx);
    out.matched = matched.iter().map(|m| m.logged.clone()).collect();
    for FileMatch { domain: domain_def, parent_of, .. } in &matched {
        if !w.rules.borrow().contains_key(&domain_def.name) {
            crate::hook::user_prompt_submit::in_time(deadline)?;
        }
        // Read the rules FIRST, then key the dedup on what came back.
        //
        // Until 0.16.0 this was the other way round: the key was
        // `rules_hash(&domain_def.rendered_rules())`, which renders the TOML,
        // and the payload was `query_rules_from_graph`, which reads the graph.
        // A domain whose rules live only in the graph — which is every domain
        // whose rules were added with `base rule add` — has an EMPTY
        // `rendered_rules()`, so its key was a constant. The first tool call
        // injected and marked it; every later call in that session computed the
        // same constant and was suppressed, however the rules had changed. The
        // reverse cost the other way: a domains.toml edit changed the key and
        // re-injected text the reader had already seen.
        //
        // Reading before deciding costs one query on a domain that turns out to
        // be deduped. That is the price of a key that describes the payload, and
        // the defect it removes is a rule the operator added never arriving.
        // A rule with matchers of its own left this path in 4d: it was served on them above (F1).
        let base = w.rules_of(config, domain_def);
        let rules: Vec<domain::rules::ServedRule> = match view.rule_edits.get(&domain_def.name) {
            None => base.iter().filter(|r| !converted_ids.contains(r.id.as_str())).cloned().collect(),
            Some((gone, added)) => base
                .iter()
                .filter(|r| !gone.contains(&r.id))
                .cloned()
                .chain(added.iter().cloned())
                .filter(|r| !converted_ids.contains(r.id.as_str()))
                .collect(),
        };

        // Dedup per RULE, not per domain block (F9). Before this the whole block
        // was one unit, so adding a single rule to a seventeen-rule domain handed
        // the reader all seventeen again, sixteen of which it had already been
        // told this session. Measured live 2026-09-14: editing BASE-WORK-ORDER.md
        // served all thirteen basemode rules, for an edit to a base work order.
        //
        // Scope is `None`, which means once per session and again on a tier
        // change. Every rule left on this path has no matchers of its own, so it is
        // serving through its domain's trigger, and K4 says an unconverted rule keeps
        // exactly the behaviour it has today until an operator approves a conversion.
        //
        // `claim_rule` records as it decides, so this loop cannot claim a rule it
        // then fails to render: everything that survives the filter is rendered.
        let fresh: Vec<(usize, domain::rules::ServedRule)> = rules
            .iter()
            .enumerate()
            .filter(|(_, r)| session.claim_rule(&r.id, r.content_hash, w.tier, None))
            .map(|(i, r)| (i, r.clone()))
            .collect();
        // D13: a parent's block says whose parent it is.
        let label = match parent_of {
            Some(child) => format!("{} (parent of {child})", domain_def.name),
            None => domain_def.name.clone(),
        };
        out.domains.push(FileDomain { def: (*domain_def).clone(), label, fresh, all: rules.len() });
    }
    Ok(out)
}

/// The domains a tool call's touched paths bring in, in serving order, each with the project it is the nested parent
/// of when that is why it came (D13): the touched file's project and the domains whose own trigger holds the file
/// ([`domain::matcher::path_hits`], the one seam the prompt hook uses too), then the domains whose `file_keywords`
/// appear in a touched path, then the parents, so a tight budget drops parent rules first.
///
/// `auto_inject = false` is honoured before any other test (F29 D3): this hook is the other automatic path. Always-on
/// domains fire on the prompt, not here.
fn match_by_file<'a>(
    domains: &'a [domain::DomainDef],
    paths: &[String],
    named: &[String],
    ctx: &domain::matcher::TriggerContext,
) -> Vec<FileMatch<'a>> {
    let eligible = |d: &domain::DomainDef| d.auto_inject && !d.is_always();
    let mut direct: Vec<FileMatch<'a>> = Vec::new();
    let mut parents: Vec<FileMatch<'a>> = Vec::new();
    // The touched path goes in the log's entry only when the call touched more than one: the row names a lone one.
    let which = |p: &str| (paths.len() > 1).then(|| p.to_string());
    for hit in domain::matcher::path_hits(domains, paths, ctx) {
        let d = &domains[hit.domain];
        if !eligible(d) {
            continue;
        }
        match hit.via {
            domain::matcher::PathVia::Parent(child) => {
                let logged = Matched { path: which(&hit.path), ..Matched::new(&d.name, "parent", Some(format!("{child} nested"))) };
                parents.push(FileMatch { domain: d, parent_of: Some(child), logged });
            }
            _ => {
                let logged = Matched { path: which(&hit.path), ..Matched::new(&d.name, "path", Some(hit.value.clone())) };
                direct.push(FileMatch { domain: d, parent_of: None, logged });
            }
        }
    }
    // File keyword match: a keyword in a path the call names (lightweight: a full content scan would read the file).
    for d in domains.iter().filter(|d| eligible(d)) {
        let listed = direct.iter().chain(&parents).any(|m| std::ptr::eq(m.domain, d));
        let file_kw_hit = d
            .file_keywords
            .iter()
            .find(|kw| named.iter().any(|fp| fp.to_lowercase().contains(&kw.to_lowercase())));
        if !listed && let Some(kw) = file_kw_hit {
            direct.push(FileMatch { domain: d, parent_of: None, logged: Matched::new(&d.name, "file_keyword", Some(kw.clone())) });
        }
    }
    direct.extend(parents);
    direct
}

/// A domain a tool call's paths brought in ([`match_by_file`]): the project it is the nested parent of, when that is
/// why it came (D13), and its match-log entry (K1).
struct FileMatch<'a> {
    domain: &'a domain::DomainDef,
    parent_of: Option<String>,
    logged: Matched,
}

/// Every path a tool call names, absolute, each once (P1): the tool's own file path (Read, Edit, Write, a notebook,
/// a search's folder), a relative one joined to the session's folder; for Bash and PowerShell, every file or folder
/// the command names ([`command_paths`]). The caller stands the session's folder in for a command that names none.
fn touched_paths(event: &serde_json::Value, cwd: &Path, home: Option<&Path>, file_paths: &[PathBuf]) -> Vec<String> {
    let mut out: Vec<PathBuf> = Vec::new();
    for p in file_paths {
        let abs = if domain::matcher::is_absolute(&p.to_string_lossy()) { p.clone() } else { cwd.join(p) };
        push_unique(&mut out, abs);
    }
    if let Some(cmd) = tool_command(event) {
        for p in command_paths(cmd, cwd, home) {
            push_unique(&mut out, p);
        }
    }
    out.iter().map(|p| p.display().to_string()).collect()
}

fn push_unique(out: &mut Vec<PathBuf>, p: PathBuf) {
    if !out.contains(&p) {
        out.push(p);
    }
}

/// The files and folders a Bash or PowerShell command names (P1): each word of each command it runs, wrappers taken
/// off and quotes honoured ([`domain::rules::command_parts`]), that resolves to a file or folder that exists, absolute
/// or relative to the session's folder; a `cd` re-bases the relative words after it, and `--flag=value` is read as its
/// value. At most 64 words are looked at, so a long heredoc costs a bounded number of lookups.
fn command_paths(cmd: &str, cwd: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    const MAX_WORDS: usize = 64;
    let mut out: Vec<PathBuf> = Vec::new();
    let mut base = cwd.to_path_buf();
    let mut looked = 0usize;
    for part in domain::rules::command_parts(cmd) {
        let Some(first) = part.first() else { continue };
        if matches!(domain::rules::program_name(first).as_str(), "cd" | "pushd" | "chdir" | "set-location" | "sl") {
            if let Some(dir) = part.get(1).and_then(|w| resolve_word(w, &base, home)).filter(|p| p.is_dir()) {
                push_unique(&mut out, dir.clone());
                base = dir;
            }
            continue;
        }
        for word in &part {
            looked += 1;
            if looked > MAX_WORDS {
                return out;
            }
            let w = word.split_once('=').filter(|(k, _)| k.starts_with('-')).map_or(word.as_str(), |(_, v)| v);
            if let Some(p) = resolve_word(w, &base, home).filter(|p| p.exists()) {
                push_unique(&mut out, p);
            }
        }
    }
    out
}

/// A command word as the path it would name, before anyone checks it exists. Flags, URLs, variables, globs and
/// redirections are not paths. A network or WSL share (`\\wsl.localhost\...`) is never looked at: from Windows,
/// opening one starts the WSL machine. `~` is `home`; Git Bash's `/c/...` is `C:/...` on Windows.
fn resolve_word(word: &str, base: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let t = word.trim().trim_end_matches([',', ';']);
    if t.is_empty() || t.starts_with('-') || t.contains("://") || t.contains(['*', '?', '$', '{', '}', '`', '<', '>', '|']) {
        return None;
    }
    let slashed = t.replace('\\', "/");
    if slashed.starts_with("//") {
        return None;
    }
    if t == "~" || slashed.starts_with("~/") {
        return home.map(|h| h.join(slashed.trim_start_matches('~').trim_start_matches('/')));
    }
    if cfg!(windows)
        && let Some(rest) = slashed.strip_prefix('/')
        && let Some((drive, tail)) = rest.split_once('/')
        && drive.len() == 1
        && drive.chars().all(|c| c.is_ascii_alphabetic())
    {
        return Some(PathBuf::from(format!("{}:/{}", drive.to_ascii_uppercase(), tail)));
    }
    Some(if domain::matcher::is_absolute(t) { PathBuf::from(t) } else { base.join(t) })
}


/// Query PAUL FileChange and Decision entities linked to a file path.
/// Returns formatted context string for hook injection.
fn query_paul_context(store: &oxigraph::store::Store, config: &BaseConfig, file_path: &str) -> String {
    let ns = &config.namespace;
    let p = &ns.prefix;
    let pfx = crud::prefixes(ns);

    // Normalize: strip leading ./ and match against filePath values
    let clean = file_path.trim_start_matches("./");
    // The stored `:filePath` values come from PAUL summary docs — workspace
    // relative and forward-slash. A live tool path on Windows arrives
    // backslashed, so normalize before escaping: escaping alone yields a probe
    // that parses and then matches nothing, which is this same bug one layer up.
    let probe = crud::path_literal(clean);

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
             FILTER(CONTAINS(STR(?path), \"{probe}\"))\n\
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

/// The command a shell tool is about to run: Bash's, and PowerShell's too (A2). On the operator's machine the
/// PowerShell tool is the primary shell, and every relay ping sent on 2026-09-14 went through it.
fn tool_command(event: &serde_json::Value) -> Option<&str> {
    match event.get("tool_name").and_then(|v| v.as_str()) {
        Some("Bash" | "PowerShell") => event.get("tool_input")?.get("command")?.as_str(),
        _ => None,
    }
}

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
    if let Some(fp) = event
        .get("tool_input")
        .and_then(|ti| ti.get("notebook_path"))
        .and_then(|v| v.as_str())
    {
        paths.push(PathBuf::from(fp));
    }
    paths
}
