use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use oxigraph::sparql::QueryResults;

use crate::config::{load_queries, BaseConfig};
use crate::emit::{
    self, Block, Emission, Facts, Fragments, FullOutput, Level, Rank, Reason, Rendered,
};
use crate::ontology;
use crate::signal::SignalOutput;
use crate::store;

/// Run session start and collect everything it has to say into `out`. Prints nothing: the
/// dispatcher adds the relay blocks and prints [`SessionOutput::finish`]'s text once. What was
/// collected before an error stays in `out`, as it used to stay on stdout.
pub fn handle(
    config: &BaseConfig,
    cwd: &Path,
    session_id: Option<&str>,
    out: &mut SessionOutput,
) -> Result<()> {
    // Surface graph corruption at boot — loud, before any other output, so a
    // broken graph announces itself immediately instead of degrading silently.
    warn_unhealthy_graphs(cwd, out);

    // Proactive graph hygiene (Phase 52): compact any tier graph that has ballooned
    // past the threshold so graphs never balloon on a user's machine. Low-frequency
    // path; backup-first + atomic + cooldown-gated; skips an unhealthy graph.
    for outcome in crate::graph::auto_compact_tiers(&config.graph, cwd) {
        out.push(
            "auto-compact",
            &format!("{}\n", crate::graph::format_auto_compact_notice(&outcome)),
            1,
        );
    }

    // Clear session dedup state for fresh session
    // Try workspace first, fall back to global tier for no-workspace users
    let session_base_dir = crate::config::find_workspace_base(cwd)
        .or_else(|| {
            crate::home::home_root().map(|h| h.join(".base-gbl").join(".base")).filter(|p| p.is_dir())
        });
    // Clear THIS session only. A blanket clear() deleted the shared file, which
    // reset every concurrently-running session's bracket to FRESH mid-conversation.
    if let Some(ref base_dir) = session_base_dir {
        crate::domain::session::SessionState::clear_for(base_dir, session_id);
    }

    // Auto-sync domains to graph
    crate::hook::user_prompt_submit::ensure_domain_sync_pub(config, cwd);

    // Scan and ingest paul.toml projects into graph (idempotent)
    ingest_paul_projects(config, cwd);

    // Every record carries a domain link (Chris, 2026-09-04). One-time and
    // idempotent: the work is recomputed from the store on every run where the
    // stamp is absent, and the stamp lands in the SAME write_back as the data, so
    // a `.bak` restore re-migrates correctly instead of skipping forever.
    //
    // Placed after domain sync and paul ingest on purpose: a backfill links only
    // to a domain record that already exists, and both of those create some.
    //
    // Affordable here, measured: one full snapshot+parse+rewrite of the real
    // 13.4 MB store costs 1.25-1.51 s against a hook that already spends 5.0-7.9 s
    // and already rewrites the store in `auto_compact_tiers` (hawk C2). One time,
    // not every session.
    if config.graph.auto_migrate {
        let notice = crate::migrate::format_outcomes(&crate::migrate::migrate_tiers(
            cwd,
            &config.namespace,
            crate::migrate::Trigger::SessionStart,
        ));
        if !notice.is_empty() {
            out.push("migrate", &notice, 1);
        }
    }

    // A release that adds a hook wires it here, once per version. The
    // auto-update swaps the binary and touches nothing else, and a hook that
    // is not in settings.json never fires — silently.
    let added = crate::install::ensure_hooks_wired();
    if !added.is_empty() {
        out.push(
            "hooks-wired",
            &format!(
                "[hooks] wired base hook {} into ~/.claude/settings.json (new in this release; live from the next session).\n",
                added.join(", ")
            ),
            1,
        );
    }

    // A home that got base without either installer -- an `npx` bootstrap, a
    // release zip, a binary copied into place -- has never seen the first-run
    // message. This is the third path, and it prints the SAME text: byte
    // identity across all three is the whole point of `first_run`.
    //
    // It is exclusive with the update notice below by construction: a home with
    // a swap in update.log is not on its first run, it is on its fourth.
    if let Some(msg) = crate::first_run::session_start_message() {
        out.push("first-run", &msg, 1);
    }

    // Same cluster, same reason: this is the new binary's first session, and it
    // is the only process that can say what it is now running.
    if let Some(notice) = crate::update::session_start_notice() {
        out.push("update-applied", &format!("{notice}\n"), 1);
    }

    // The installed CLAUDE.md contract refreshes here, once per version, for the
    // same reason: the process that runs `base update` is the outgoing binary and
    // carries the old text, so only the new binary's first session can write its own.
    match crate::install::ensure_claude_md_current() {
        Some(crate::install::ClaudeMdRefresh::Refreshed) => {
            out.push(
                "contract",
                "[contract] refreshed the BASE CLI section of ~/.claude/CLAUDE.md to this release.\n",
                1,
            );
        }
        Some(crate::install::ClaudeMdRefresh::Duplicate(n)) => {
            out.push(
                "contract",
                &format!("[contract] ~/.claude/CLAUDE.md carries {n} '## BASE CLI' sections; base refreshes none until one remains.\n"),
                1,
            );
        }
        _ => {}
    }

    // Every app gets a code map the first time a session opens in it — a
    // marked repo, or a bare folder of source files nobody has `git init`ed
    // yet — and a refresh when it has one (Chris, 2026-09-01: "anytime a dev
    // project is started, it auto creates the AST map ... no app should ever
    // go without one"). Detached and debounced; never the home directory, a
    // user folder, or a workspace that only holds other apps. The rules live
    // in `hook::automap`; only a FIRST build, or a failing one, is announced.
    // #20: one line, only when the trail holds a failure; silent otherwise.
    for (tier, base_dir) in crate::hook::hook_log_dirs(cwd) {
        // Only when hooks are failing NOW. A hook that failed once and has
        // succeeded since is history: `doctor` still reports it, but announcing it
        // at every session start would be a permanent banner for a transient miss.
        if let Some(t) = crate::hook::hook_failure_summary(&base_dir)
            && t.broken_now
        {
            out.push(
                "hooks-health",
                &format!("[hooks] {tier} tier {} Run `base doctor`.\n", t.summary),
                1,
            );
        }
    }
    if let Some(line) = crate::hook::automap::session_start_notice(cwd) {
        out.push("automap", &format!("{line}\n"), 1);
    }

    // Mechanical reconcile (task-artifact protocol): replace hook-stamped lastActive
    // with the real folder last-touch, then decay cold projects active→deferred (and
    // revive the reverse) BEFORE signals surface, so the rendered state is already
    // true. Fail-open; gated on [protocol] enabled.
    reconcile_active_state(config, cwd);

    // Emit operator profile (if configured)
    if let Some(profile) = crate::operator::load() {
        out.push(
            "operator",
            &format!("{}\n", crate::operator::format_block(&profile)),
            1,
        );
    }

    // Silent self-update, then the legacy check/banner for pinned installs.
    auto_update(config);
    check_and_banner(out);

    // Try signals first (Phase 5) — primary injection source
    let mut diagnostics: Vec<String> = Vec::new();

    if let Ok(signal_result) = crate::signal::run_signals(cwd, config, "session-start") {
        diagnostics.extend(signal_result.diagnostics.iter().cloned());
        let any_signal = !signal_result.is_empty();
        out.push_signals(signal_result);

        if any_signal {
            // Flow protocol injection (static behavioral rules) — after signals
            if config.flow.enabled && config.flow.protocol {
                out.push(
                    "flow-protocol",
                    &format!("\n{}", crate::hook::flow::protocol_block()),
                    1,
                );
            }

            // Diagnostics: always emitted, bypass suppression
            if !diagnostics.is_empty() {
                out.push(
                    "diagnostics",
                    &format!("\n{}", diagnostics.join("\n")),
                    diagnostics.len(),
                );
            }

            // Extension status injection (Phase 23)
            inject_extension_status(config, cwd, out);

            // Context triggers cheat-sheet (Phase 21)
            let triggers = crate::domain::query::context_triggers_block(cwd);
            if !triggers.is_empty() {
                out.push("triggers", &format!("\n{triggers}"), 1);
            }

            return Ok(());
        }
    }

    // Fallback: ad-hoc queries from queries.toml (Phase 1 behavior)
    let trig_files = discover_trig_files(cwd);

    if trig_files.is_empty() {
        // Emit diagnostics even when no graph files found
        if !diagnostics.is_empty() {
            out.push("diagnostics", &diagnostics.join("\n"), diagnostics.len());
        }
        return Ok(());
    }

    let paths: Vec<&Path> = trig_files.iter().map(|p| p.as_path()).collect();
    let graph = store::load_graphs(&paths)?;

    ontology::load_vocabulary(&graph, &config.namespace)?;

    let queries = load_queries(cwd, config);
    let mut output = String::new();
    let mut queries_shown = 0usize;

    for qdef in &queries {
        let sparql = format!(
            "PREFIX {p}: <{u}>\n\
             PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\n\
             PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
             PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\n\
             {body}",
            p = config.namespace.prefix,
            u = config.namespace.uri,
            body = qdef.sparql,
        );

        if let Ok(results) = store::query(&graph, &sparql) {
            let section = format_results(results, &qdef.format, &qdef.description);
            if !section.is_empty() {
                output.push_str(&section);
                output.push('\n');
                queries_shown += 1;
            }
        }
    }

    if !output.is_empty() {
        out.push("queries", output.trim_end(), queries_shown);
    }

    // Flow protocol injection — also in fallback path
    if config.flow.enabled && config.flow.protocol {
        if !output.is_empty() {
            out.newline();
        }
        out.push("flow-protocol", crate::hook::flow::protocol_block(), 1);
    }

    // Diagnostics: always emitted at end of output
    if !diagnostics.is_empty() {
        if !output.is_empty() || (config.flow.enabled && config.flow.protocol) {
            out.newline();
        }
        out.push("diagnostics", &diagnostics.join("\n"), diagnostics.len());
    }

    // Extension status injection (Phase 23)
    inject_extension_status(config, cwd, out);

    Ok(())
}

/// Spec B1: every block's rank, and inside its rank its place. The trimmer takes the bottom of the
/// lowest rank first, so the `Tail` order is B1 row 8's list read upward: diagnostics shrink first,
/// pulse last, and the relay wake contract outlasts the operator profile and the notices. A kind
/// missing from this table sorts after all of it, and `every_pushed_kind_has_a_place_in_the_layout`
/// fails the build when one does.
pub const LAYOUT: [(&str, Rank); 30] = [
    ("instructions", Rank::Pinned),
    ("graph-unhealthy", Rank::DueNow),
    ("reminders", Rank::DueNow),
    ("relay-inbox", Rank::DueNow),
    ("handoffs", Rank::Primary),
    ("forks", Rank::Secondary),
    ("projects", Rank::Secondary),
    ("tasks", Rank::Secondary),
    ("milestones", Rank::Secondary),
    ("blocked", Rank::Secondary),
    ("pulse", Rank::Tail),
    ("flow-resurface", Rank::Tail),
    ("memory", Rank::Tail),
    ("triggers", Rank::Tail),
    ("relay-tasks", Rank::Tail),
    ("relay-wake", Rank::Tail),
    ("operator", Rank::Tail),
    ("extensions", Rank::Tail),
    ("queries", Rank::Tail),
    ("flow-protocol", Rank::Tail),
    ("auto-compact", Rank::Tail),
    ("migrate", Rank::Tail),
    ("hooks-wired", Rank::Tail),
    ("first-run", Rank::Tail),
    ("update-applied", Rank::Tail),
    ("update-banner", Rank::Tail),
    ("contract", Rank::Tail),
    ("hooks-health", Rank::Tail),
    ("automap", Rank::Tail),
    ("diagnostics", Rank::Tail),
];

/// A kind's rank and its place in [`LAYOUT`].
pub fn place(kind: &str) -> (Rank, usize) {
    LAYOUT
        .iter()
        .position(|(k, _)| *k == kind)
        .map(|i| (LAYOUT[i].1, i))
        .unwrap_or((Rank::Tail, LAYOUT.len()))
}

/// The B1 data blocks. The instruction block is printed only when one of them has an item: its
/// lines are about these blocks, and a session with none of them has nothing to follow.
const DATA_BLOCKS: [&str; 6] = ["reminders", "handoffs", "forks", "projects", "tasks", "milestones"];

/// Blocks whose producer marks them shown while producing them: a welcome stamped, an update
/// marked noticed, relay messages marked delivered, a task marked announced, a wake nudge
/// stamped. Collapsed, their text would never be seen, so their floor names the full-output
/// file, which is written before anything prints.
pub const SHOWN_ONCE: [&str; 5] = [
    "first-run",
    "update-applied",
    "relay-inbox",
    "relay-tasks",
    "relay-wake",
];

/// The command that prints all of a block, where one exists. A block without one collapses to
/// a line naming the full-output file instead.
pub fn command_for(kind: &str) -> Option<&'static str> {
    match kind {
        "graph-unhealthy" | "hooks-health" => Some("base doctor"),
        "operator" => Some("base operator show"),
        "handoffs" => Some("base handoff list"),
        "reminders" => Some("base reminder list"),
        "forks" => Some("base fork list"),
        "projects" => Some("base project list --all"),
        "tasks" => Some("base task list"),
        "milestones" => Some("base milestone list"),
        "extensions" => Some("base extension list"),
        _ => None,
    }
}

/// The one line a block becomes when the budget cannot afford it (spec A4): its kind, its
/// count, and where the rest is.
pub fn floor_line(kind: &str, items: usize, full: &FullOutput) -> String {
    if let Some(command) = command_for(kind) {
        return format!("{kind} {items} · all: {command}");
    }
    match (full.written_path(), full.failure()) {
        (Some(path), _) => format!("{kind} {items} · full text: {path}"),
        (None, Some(why)) => {
            format!("{kind} {items} · not shown, and the full output was not written: {why}")
        }
        (None, None) => {
            format!("{kind} {items} · not shown, and [budget] write_full_output is false")
        }
    }
}

/// Where the untrimmed session start goes (spec A6): the workspace `.base` when one resolves,
/// else the global tier's `.base` when it exists. Never created here, so a session opened
/// outside every tier gets no file and its floors say so. The letters file sits beside it.
fn full_output_path(cwd: &Path) -> Option<PathBuf> {
    crate::crud::handoff_show::session_start_dir(cwd).map(|dir| dir.join("last-session-start.md"))
}

fn kind_of(id: &str) -> &str {
    id.split('#').next().unwrap_or(id)
}

/// Everything session start will print, collected before any of it reaches stdout.
///
/// Session start printed from 32 sites as it went, so nothing could measure the output before
/// the host cut it: 45,232 characters on 2026-09-14, of which Claude saw the first 2,000. Each
/// site now hands its exact text to this collector. The dispatcher adds the relay blocks and
/// prints the text [`SessionOutput::finish`] returns, once.
#[derive(Default)]
pub struct SessionOutput {
    fragments: Fragments,
    signals: Option<SignalOutput>,
}

impl SessionOutput {
    pub fn new() -> Self {
        Self::default()
    }

    /// One print site's exact output, newlines included. `items` counts what it lists.
    pub fn push(&mut self, kind: &str, text: &str, items: usize) {
        self.fragments.push(kind, text, items);
    }

    /// A bare line break between two sites, which belongs to neither.
    pub fn newline(&mut self) {
        self.fragments.push("", "\n", 0);
    }

    pub fn fragments(&self) -> &Fragments {
        &self.fragments
    }

    /// Keep the signals for [`SessionOutput::finish`], which places their blocks by spec B1.
    /// Signals skipped as unchanged print nothing, as before, and leave a ledger row per block.
    pub fn push_signals(&mut self, signals: SignalOutput) {
        for signal in signals.unchanged() {
            for block in &signal.blocks {
                self.fragments.note_withheld(
                    block.kind,
                    block.items,
                    Reason::HashUnchanged,
                    command_for(block.kind).unwrap_or(""),
                );
            }
        }
        self.signals = Some(signals);
    }

    /// Place every block by spec B1, write the untrimmed output and the letters, trim to
    /// `[budget] session_start_chars` under the B2 header, and record which signals were shown in
    /// full. Prints nothing: the caller prints the returned text.
    pub fn finish(self, config: &BaseConfig, cwd: &Path) -> Rendered {
        let budget = &config.budget;
        let (parts, withheld) = self.fragments.into_parts();

        let mut placed: Vec<Placed> = parts
            .into_iter()
            .map(|part| Placed {
                id: part.id,
                kind: part.kind,
                text: part.text,
                total: part.items,
                shown: part.items,
            })
            .collect();
        let letters = self
            .signals
            .as_ref()
            .map(|s| s.letters.clone())
            .unwrap_or_default();
        if let Some(signals) = &self.signals {
            for signal in signals.signals() {
                for block in &signal.blocks {
                    placed.push(Placed {
                        id: block.kind.to_string(),
                        kind: block.kind.to_string(),
                        text: block.text.clone(),
                        total: block.total,
                        shown: block.items,
                    });
                }
            }
        }
        if placed
            .iter()
            .any(|p| p.total > 0 && DATA_BLOCKS.contains(&p.kind.as_str()))
        {
            placed.push(Placed {
                id: "instructions".to_string(),
                kind: "instructions".to_string(),
                text: instruction_block(&letters),
                total: 0,
                shown: 0,
            });
        }
        // Stable: blocks of one place keep the order they arrived in.
        placed.sort_by_key(|p| place(&p.kind));
        for (i, p) in placed.iter_mut().enumerate() {
            // The sites' own leading and trailing newlines belonged to the old print order. In
            // B1's order every block is one paragraph, a blank line before each but the first.
            let body = p.text.trim_matches('\n');
            p.text = if i == 0 {
                body.to_string()
            } else {
                format!("\n{body}")
            };
        }

        let mut untrimmed = Emission::new(budget.session_start_chars, budget.first_screen_chars);
        for p in &placed {
            let block = Block::new(p.id.clone(), place(&p.kind).0, p.text.clone(), "", "")
                .items(p.total, p.shown);
            let pushed = untrimmed.push(block);
            debug_assert!(pushed, "block ids are unique by construction");
        }
        let full = if !budget.write_full_output {
            FullOutput::off()
        } else if let Some(path) = full_output_path(cwd) {
            let written = emit::write_full_output(&path, &untrimmed.full_text());
            if let Some(why) = written.failure() {
                eprintln!("base: session start could not write its full output: {why}");
            }
            written
        } else {
            FullOutput::not_written("no workspace .base and no global .base directory to hold it")
        };
        if self.signals.is_some()
            && let Some(dir) = crate::crud::handoff_show::session_start_dir(cwd)
        {
            let kept = crate::crud::handoff_show::write_letters(&dir, &letters);
            if let Some(why) = kept.failure() {
                eprintln!("base: session start could not keep its handoff letters: {why}");
            }
        }

        let mut emission = Emission::new(budget.session_start_chars, budget.first_screen_chars);
        for p in placed {
            let floor = floor_line(&p.kind, p.total, &full);
            let command = command_for(&p.kind)
                .or(full.written_path())
                .unwrap_or("")
                .to_string();
            let block = Block::new(p.id, place(&p.kind).0, p.text, floor, command)
                .items(p.total, p.shown);
            let pushed = emission.push(block);
            debug_assert!(pushed, "block ids are unique by construction");
        }
        for row in withheld {
            emission.note_withheld(row.block, row.items, row.reason, row.command);
        }
        let rendered = emission.render(&full, Some(&header_line));
        if !rendered.first_screen_ok {
            eprintln!(
                "base: session start's header, instructions and DUE NOW take more than the first {} units",
                rendered.first_screen_u16
            );
        }
        if rendered.over_budget {
            eprintln!(
                "base: session start printed {} units against [budget] session_start_chars = {}, with every block that can shrink already at its floor",
                rendered.emitted_u16, rendered.budget_u16
            );
        }

        if let Some(signals) = self.signals {
            let in_full: HashSet<&str> = rendered
                .blocks
                .iter()
                .filter(|b| b.level() == Level::Full)
                .map(|b| kind_of(b.id()))
                .collect();
            signals.record_shown(|kind| in_full.contains(kind));
        }
        rendered
    }
}

/// One block on its way into the emission: where it goes, what it says, what it counts.
struct Placed {
    id: String,
    kind: String,
    text: String,
    total: usize,
    shown: usize,
}

/// Spec B3, with B7's BEHAVIOR lines merged in: what Claude does first, written before any data
/// so no trim can remove it. It names only commands that exist; the deferred line arrives with
/// lane 3's `base handoff deferred` (lane doc B16, flag 3).
pub fn instruction_block(letters: &[(char, String)]) -> String {
    let mut s = String::from(
        "DO THIS FIRST, BEFORE ANYTHING ELSE IN YOUR FIRST REPLY:\n\
         1. Show DUE NOW, then HANDOFFS, exactly as lettered. Nothing prepended. No \"is this stale?\" questions.\n\
         2. The user names a handoff by letter, project or a few words: run `base handoff show <what they said>` and read the doc it prints. Several matches: list them and ask.\n\
         3. \"snooze <letter> <N>d\" → `base handoff snooze <slug> <N>` · \"archive <letter>\" → `base handoff archive <slug>` · a handled reminder → `base reminder remove <slug>`.\n\
         4. FORKS are open side-work, not a lettered choice; several stay open. `base fork snooze <title> <N>` · `base fork archive <title>`.\n\
         5. Every block below is a summary. Its full list is the command on its line, and the whole untrimmed output is the file on line 1. Never guess; run it.",
    );
    if !letters.is_empty() {
        let map: Vec<String> = letters
            .iter()
            .map(|(letter, slug)| format!("{letter}={slug}"))
            .collect();
        s.push_str("\nLetters: ");
        s.push_str(&map.join(" "));
    }
    s
}

/// Spec B2, line 1: every count, the withheld total, and where the untrimmed output is. Rendered
/// from the blocks at their final level on every trim pass, so it is measured as it is printed.
pub fn header_line(facts: &Facts<'_>) -> String {
    let total = |id: &str| facts.block(id).map(Block::items_total).unwrap_or(0);
    let handoffs_shown = facts.block("handoffs").map(Block::items_shown).unwrap_or(0);
    let full = match (facts.full.written_path(), facts.full.failure()) {
        (Some(path), _) => path.to_string(),
        (None, Some(why)) => format!("not written ({why})"),
        (None, None) => "not written ([budget] write_full_output = false)".to_string(),
    };
    format!(
        "[BASE START · {} due · handoffs {} open ({handoffs_shown} shown) · forks {} · projects {} · tasks {} · milestones {} · withheld {} · full: {full}]",
        total("reminders"),
        total("handoffs"),
        total("forks"),
        total("projects"),
        total("tasks"),
        total("milestones"),
        facts.withheld_total(),
    )
}

/// Inject extension status lines and run extension session-start SPARQL queries.
/// Fail-open: malformed extensions, missing query files, and query errors all skip silently.
fn inject_extension_status(config: &BaseConfig, cwd: &Path, out: &mut SessionOutput) {
    let extensions = crate::extension::load_extensions();
    if extensions.is_empty() {
        return;
    }

    for ext in &extensions {
        // Print inject template + run queries if session_start hook declared
        if let Some(hooks) = &ext.hooks
            && let Some(ss) = &hooks.session_start
        {
            if let Some(inject) = &ss.inject {
                out.push("extensions", &format!("{inject}\n"), 1);
            }

            // Run extension SPARQL queries
            for query_rel_path in &ss.queries {
                let query_path = if let Some(fw_dir) = &ext.framework_dir {
                    let expanded = if fw_dir.starts_with("~/") {
                        crate::home::home_root()
                            .map(|h| h.join(&fw_dir[2..]))
                            .unwrap_or_else(|| PathBuf::from(fw_dir))
                    } else {
                        PathBuf::from(fw_dir)
                    };
                    expanded.join(query_rel_path)
                } else {
                    PathBuf::from(query_rel_path)
                };

                let sparql = match std::fs::read_to_string(&query_path) {
                    Ok(s) => s.replace("{{prefix}}", &config.namespace.prefix),
                    Err(_) => {
                        eprintln!(
                            "base: ext:{} query file not found: {}",
                            ext.name,
                            query_path.display()
                        );
                        continue;
                    }
                };

                // Load graph and run query. Union default graph for the same reason
                // as domain queries: an extension author writing plain patterns
                // would otherwise match nothing, since base stores only into
                // named graphs.
                if let Some(store) = store::load_merged(cwd) {
                    match store::query_union(&store, &sparql) {
                        Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) => {
                            let rows: Vec<_> = solutions.filter_map(|r| r.ok()).collect();
                            if !rows.is_empty() {
                                out.push(
                                    "extensions",
                                    &format!(
                                        "<ext:{}-query>\n{} result(s) from {}\n</ext:{}-query>\n",
                                        ext.name,
                                        rows.len(),
                                        query_rel_path,
                                        ext.name
                                    ),
                                    1,
                                );
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "base: ext:{} query error in {}: {e}",
                                ext.name, query_rel_path
                            );
                        }
                        _ => {}
                    }
                }
            }

            // Run ingest for extensions with declared sources
            if !ss.ingest.is_empty() {
                match crate::extension::ingest::ingest_extension(ext, cwd, config) {
                    Ok(stats) if stats.entities > 0 => {
                        eprintln!(
                            "base: ext:{} ingested {} entities from {} file(s)",
                            ext.name, stats.entities, stats.files
                        );
                    }
                    // Zero entities from a declared ingest is reported, not swallowed.
                    // The old `_ => {}` meant a misconfigured extension produced the
                    // exact same output as a working one — validate passing, HOOKS:S
                    // showing, exit 0 — which reads as success.
                    Ok(_) => {
                        eprintln!(
                            "base: ext:{} declared {} ingest source(s) but ingested 0 entities",
                            ext.name,
                            ss.ingest.len()
                        );
                    }
                    Err(e) => {
                        eprintln!("base: ext:{} ingest error: {e}", ext.name);
                    }
                }
            }
        }
    }
}

/// Check for updates and inject persistent banner if needed. Fail-open — never blocks session.
/// Silent self-update, triggered by session start.
///
/// Everyone should be on the current release without ever being told to run
/// anything, so this is on by default (`base config set update.auto false` to
/// pin a machine). The work happens in a detached child: the download never
/// delays session start, and the atomic rename means THIS session keeps the
/// binary it started with while the next one comes up new.
fn auto_update(config: &BaseConfig) {
    if !config.update.auto {
        return;
    }
    // Never fight a developer's working copy: a base built from source and run
    // out of its own target/ would be clobbered by a release binary.
    if std::env::var_os("BASE_NO_AUTO_UPDATE").is_some() || config.devmode.enabled {
        return;
    }
    crate::update::spawn_background_update();
}

fn check_and_banner(out: &mut SessionOutput) {
    let Some(mut manifest) = crate::manifest::Manifest::load() else {
        return; // No manifest = nothing to check
    };

    // A hand-swapped binary leaves the recorded version stale, and every decision
    // below is made against it. Correct it before reading anything else.
    if crate::manifest::reconcile_running_version(&mut manifest) {
        let _ = manifest.save();
    }

    let pending = &manifest.update_check.pending_update;

    // Activation used to suppress this banner: it was the paid removal of an
    // attribution that no longer exists. With the gate gone the snooze is the
    // only thing that quiets the banner, which is what the snooze is for.
    if !pending.is_empty() {
        if !crate::manifest::is_snoozed(&manifest) {
            out.push("update-banner", &crate::manifest::format_update_banner(pending), 1);
        }
        // The update is already known; no HTTP check this session.
        return;
    }

    // Version check (weekly, HTTP call)
    if !crate::manifest::should_check(&manifest) {
        return;
    }

    // Run the check — 3s timeout per endpoint, fail silently on any error
    let result = crate::manifest::check_for_updates(&mut manifest);

    // Save manifest regardless (updates last_checked)
    let _ = manifest.save();

    // The activation gate that used to sit here is gone with the feature.
    if let Ok(Some(ref pending)) = result {
        out.push("update-banner", &crate::manifest::format_update_banner(pending), 1);
    }
}

/// Mechanical active⇄deferred reconcile (task-artifact protocol). Fail-open: any
/// error leaves graph state as-is and never blocks session start. Silent unless a
/// status actually flipped (suppression principle — lastActive refreshes are noiseless).
fn reconcile_active_state(config: &BaseConfig, cwd: &Path) {
    // R3: reminders 10+ days past due archive themselves, in every tier. Not folded into
    // `protocol::reconcile` — that pass is gated on `[protocol] enabled` and is workspace-only,
    // and reminders are neither. Fail-open like the reconcile it sits beside.
    let _ = crate::crud::reminder::auto_archive_pass(
        crate::home::home_root().as_deref(),
        cwd,
        &config.namespace,
    );
    match crate::protocol::reconcile(cwd, config) {
        Ok(stats) if stats.changed() => {
            eprintln!(
                "base: reconcile — {} deferred, {} revived ({} projects scanned)",
                stats.deferred, stats.revived, stats.scanned
            );
        }
        Ok(_) => {}
        Err(e) => eprintln!("base: reconcile failed: {e}"),
    }
}

/// Scan all registered workspaces for paul.toml files and ingest into graph. Fail-silent.
fn ingest_paul_projects(config: &BaseConfig, cwd: &Path) {
    // No workspace here means there is nothing to ingest INTO. Skipping is
    // correct and silent: a session opened in a scratch directory must never
    // get a stray `.base/` scaffolded under it (issue #8).
    if crate::config::find_workspace_base(cwd).is_none() {
        return;
    }
    let projects = crate::extract::paul_toml::scan_all_workspaces(config);
    if projects.is_empty() {
        return;
    }

    // Ingest silently — errors to stderr, never block session start
    match crate::extract::paul_toml::ingest_paul_projects(cwd, config, &projects) {
        Ok(stats) => {
            if stats.registered > 0 {
                eprintln!(
                    "base: ingested {} paul project(s) into graph",
                    stats.registered
                );
            }
        }
        Err(e) => eprintln!("base: paul.toml ingest failed: {e}"),
    }
}

/// Emit a loud, clearly-delimited warning block for any graph tier whose
/// `graph.nq` fails the parser-independent health check ([`store::graph_health`]).
///
/// Fail-OPEN: never panics, never blocks session start. Missing tiers (a fresh
/// workspace with no graph yet) and healthy tiers emit nothing — zero noise,
/// per the suppression principle. The hook's "loud" channel is THIS stdout
/// block, never a nonzero exit code (a corrupt graph must never stop a session).
fn warn_unhealthy_graphs(cwd: &Path, out: &mut SessionOutput) {
    use std::fmt::Write as _;

    let mut tiers: Vec<(&str, PathBuf)> = Vec::new();

    // Global tier: ~/.base-gbl/.base/graph.nq
    if let Some(home) = crate::home::home_root() {
        let global = home.join(".base-gbl").join(".base").join("graph.nq");
        if global.exists() {
            tiers.push(("global", global));
        }
    }

    // Workspace tier: walk upward from cwd to the nearest .base/graph.nq
    if let Some(ws) = crate::config::walk_up(cwd, |dir| {
        let ws = dir.join(".base").join("graph.nq");
        ws.exists().then_some(ws)
    }) {
        tiers.push(("workspace", ws));
    }

    let mut seen = std::collections::HashSet::new();
    for (tier, path) in tiers {
        // Don't warn twice if both tiers resolve to the same underlying file.
        let key = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if !seen.insert(key) {
            continue;
        }
        if let store::GraphHealth::Unhealthy { reason, bad_line } = store::graph_health(&path) {
            let line = bad_line.map(|n| format!(" (line {n})")).unwrap_or_default();
            let mut block = String::new();
            let _ = writeln!(block, "═══════════════════════════════════════");
            let _ = writeln!(block, "⚠️  BASE GRAPH UNHEALTHY — {tier} tier");
            let _ = writeln!(block, "   {}", path.display());
            let _ = writeln!(block, "   {reason}{line}");
            let _ = writeln!(block, "   recall / learn / sync are DEGRADED until repaired.");
            let _ = writeln!(block, "   Repair: run `base doctor` once available (v0.5),");
            let _ = writeln!(block, "           or repair manually per GRAPH-DURABILITY.md");
            let _ = writeln!(block, "═══════════════════════════════════════");
            out.push("graph-unhealthy", &block, 1);
        }
    }
}

/// Discover TriG files from global and workspace tiers.
fn discover_trig_files(cwd: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    // Global tier: ~/.base-gbl/.base/graph.nq
    if let Some(home) = crate::home::home_root() {
        let global = home.join(".base-gbl").join(".base").join("graph.nq");
        if global.exists() {
            files.push(global);
        }
    }

    // Workspace tier: walk upward from cwd to find .base/graph.nq
    if let Some(ws) = crate::config::walk_up(cwd, |dir| {
        let ws = dir.join(".base").join("graph.nq");
        ws.exists().then_some(ws)
    }) {
        files.push(ws);
    }

    files
}

/// Format SPARQL SELECT results according to the query's format type.
fn format_results(results: QueryResults, format: &str, description: &str) -> String {
    let QueryResults::Solutions(solutions) = results else {
        return String::new();
    };

    let vars: Vec<String> = solutions
        .variables()
        .iter()
        .map(|v| v.as_str().to_string())
        .collect();

    let rows: Vec<Vec<String>> = solutions
        .filter_map(|r| r.ok())
        .map(|row| {
            vars.iter()
                .map(|v| {
                    row.get(v.as_str())
                        .map(|term| term_display(term.into()))
                        .unwrap_or_default()
                })
                .collect()
        })
        .collect();

    if rows.is_empty() {
        return String::new();
    }

    let mut out = format!("[{description}]\n");

    match format {
        "table" => {
            out.push_str(&format!("| {} |\n", vars.join(" | ")));
            out.push_str(&format!(
                "|{}|\n",
                vars.iter().map(|_| "---").collect::<Vec<_>>().join("|")
            ));
            for row in &rows {
                out.push_str(&format!("| {} |\n", row.join(" | ")));
            }
        }
        "prose" => {
            let vals: Vec<String> = rows.iter().map(|r| r.join(" ")).collect();
            out.push_str(&vals.join(". "));
            out.push('\n');
        }
        _ => {
            // Default: list
            for row in &rows {
                out.push_str(&format!("- {}\n", row.join(" — ")));
            }
        }
    }

    out
}

/// Extract a human-readable string from an RDF term.
fn term_display(term: oxigraph::model::TermRef<'_>) -> String {
    use oxigraph::model::TermRef;
    match term {
        TermRef::Literal(l) => l.value().to_string(),
        TermRef::NamedNode(n) => {
            let iri = n.as_str();
            // Extract local name after # or last /
            iri.rfind('#')
                .or_else(|| iri.rfind('/'))
                .map(|pos| iri[pos + 1..].to_string())
                .unwrap_or_else(|| iri.to_string())
        }
        TermRef::BlankNode(b) => format!("_:{}", b.as_str()),
        #[allow(unreachable_patterns)]
        _ => term.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_finds_no_workspace_trig_in_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let files = discover_trig_files(tmp.path());
        // May find global graph if ~/.base-gbl/.base/graph.nq exists on host
        // but should NOT find a workspace graph
        assert!(!files.iter().any(|f| {
            let s = f.to_string_lossy();
            !s.contains(".base-gbl") && s.ends_with(".base/graph.nq")
        }));
    }

    #[test]
    fn discover_finds_workspace_trig() {
        let tmp = tempfile::tempdir().unwrap();
        let base_dir = tmp.path().join(".base");
        std::fs::create_dir_all(&base_dir).unwrap();
        std::fs::write(base_dir.join("graph.nq"), "# empty").unwrap();

        let files = discover_trig_files(tmp.path());
        // Must include the workspace graph we just created
        assert!(files.iter().any(|f| f.ends_with(".base/graph.nq")
            && !f.to_string_lossy().contains(".base-gbl")));
    }

    /// The string literals on one source line, in order. Push sites quote no quote.
    fn literals(line: &str) -> Vec<&str> {
        line.split('"').skip(1).step_by(2).collect()
    }

    /// Every kind a print site or a signal pushes has a place in spec B1's table. The kinds are read
    /// off the sources, not listed from what this commit knows: a kind missing from `LAYOUT` sorts
    /// after everything and is trimmed first without anyone having decided that. Proven by mutation
    /// (a new table cannot run red before it exists): drop one row and this fails naming the site.
    #[test]
    fn every_pushed_kind_has_a_place_in_the_layout() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut sites: Vec<(String, String)> = Vec::new();
        for file in [
            "src/hook/session_start.rs",
            "src/hook/mod.rs",
            "src/signal/mod.rs",
            "src/signal/active_awareness.rs",
        ] {
            let src = std::fs::read_to_string(root.join(file)).expect(file);
            let lines: Vec<&str> = src.lines().take_while(|l| !l.starts_with("#[cfg(test)]")).collect();
            for (n, line) in lines.iter().enumerate() {
                let code = line.trim_start();
                if code.starts_with("//") {
                    continue;
                }
                let kind = if let Some(at) = code.find("out.push(") {
                    let after = &code[at + "out.push(".len()..];
                    if after.is_empty() {
                        lines.get(n + 1).and_then(|next| literals(next).first().copied())
                    } else if after.starts_with('"') {
                        literals(after).first().copied()
                    } else {
                        None
                    }
                } else if let Some(at) = code.find("Signal::single(") {
                    literals(&code[at..]).get(1).copied()
                } else if code.starts_with("kind: \"") || code.starts_with("(\"") {
                    literals(code).first().copied()
                } else {
                    None
                };
                if let Some(kind) = kind {
                    sites.push((format!("{file}:{}", n + 1), kind.to_string()));
                }
            }
        }
        assert!(
            sites.len() >= 30,
            "read {} kind sites, too few to have read the push sites: {sites:?}",
            sites.len()
        );
        for (site, kind) in &sites {
            assert!(
                LAYOUT.iter().any(|(k, _)| k == kind),
                "{site}: kind {kind:?} has no place in LAYOUT"
            );
        }
    }
}
