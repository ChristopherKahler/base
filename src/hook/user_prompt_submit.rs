use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::BaseConfig;
use crate::domain;
use crate::domain::matcher::{match_domains_auto, MatchReason, TriggerContext};
use crate::domain::query::resolve_and_run_query;
use crate::domain::session::{rules_hash, Bracket, ReShow, SessionState};
use crate::domain::score_index::DocKind;
use crate::emit::match_log::{Cut, Item, Matched, Score, Trace};
use crate::emit::prompt::{BlockPart, Claim, Fitted, Priority, PromptBlock, PromptBlocks};

/// What the prompt hook collects before anything is printed: the context-bracket line, the named blocks, and the
/// session whose shown-records wait on what is printed.
///
/// THE CONTRACT CHANGED TWICE, AND BOTH TIMES FOR THE SAME REASON. Rank 00 took the `print!` out of this function:
/// it used to print at its four return sites while the dispatcher printed twice more, three emitters each blind to
/// the others' spend, so no budget could hold. BO-01 took the `session.save` out of it as well. The hook used to
/// record a rule as shown when it BUILT the text, before the budget cut it, so a rule the cut removed was never sent
/// again in that session (F28, ruled by Chris as D15: shown means printed whole). Now every block carries what it
/// would record as [`Claim`]s, the dispatcher fits the blocks to the budget, and [`PromptSink::commit`] records the
/// claims of the blocks that were printed, and saves.
#[derive(Default)]
pub struct PromptSink {
    /// `<context-bracket>[TIER] (prompt N)</context-bracket>`, or empty on the paths that never printed one. Always
    /// kept, and counted first.
    pub header: String,
    pub blocks: PromptBlocks,
    pending: Option<Pending>,
    /// What matched and what `select` and the walk cut, for the match log (K1, BO-13). The blocks carry what they
    /// serve; the dispatcher writes the row once the output is printed.
    pub trace: Trace,
    /// The prompt, as the hook read it, for the match log's row. Empty when there was none.
    pub prompt: String,
    /// This session's prompt number, for the match log's row.
    pub prompt_num: Option<u32>,
}

/// Where the session lives and the tier it was served at, held until the output is fitted.
struct Pending {
    base_dir: PathBuf,
    tier: Bracket,
}

/// What [`PromptSink::commit`] recorded: rules recorded as shown, and whether the bracket block was.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Committed {
    pub rules: usize,
    pub bracket_block: bool,
}

impl PromptSink {
    /// Save what `collect` changed (the prompt count, the tier, a force-refresh) NOW, as every return site did before
    /// BO-01, and keep where it lives for `commit`. The records of what was shown are not in it: they wait for the
    /// fit. Saving here and reloading in `commit` keeps the window between a load and a save of the shared `.session`
    /// file as short as it was, rather than holding the state across the relay delivery in between.
    fn hold(&mut self, session: SessionState, base_dir: Option<PathBuf>, tier: Bracket) {
        if let Some(base_dir) = base_dir {
            let _ = session.save(&base_dir);
            self.pending = Some(Pending { base_dir, tier });
        }
    }

    /// Record in the session what `fitted` printed, and save it (D15): the session is reloaded, the claims of the
    /// printed blocks are applied, and it is saved. A dropped block's rules, context, walk names and bracket block stay
    /// due. Each claim sits on exactly one block, so nothing printed is recorded on another block's account.
    pub fn commit(&mut self, fitted: &Fitted) -> Committed {
        let mut done = Committed::default();
        let pending = self.pending.take();
        let mut session = pending.as_ref().map(|p| SessionState::load(&p.base_dir));
        let now = SessionState::now_secs();
        for claim in fitted.printed_claims() {
            match claim {
                Claim::Rule { id, content, scope } => {
                    done.rules += 1;
                    if let (Some(s), Some(p)) = (session.as_mut(), pending.as_ref()) {
                        s.mark_rule_shown(id, *content, p.tier, scope.as_deref(), now);
                    }
                }
                Claim::Injected { key, hash } => {
                    if let Some(s) = session.as_mut() {
                        s.mark_injected(key, *hash);
                    }
                }
                Claim::BracketRule { id, content } => {
                    done.bracket_block = true;
                    if let (Some(s), Some(p)) = (session.as_mut(), pending.as_ref()) {
                        s.mark_rule_shown(id, *content, p.tier, None, now);
                    }
                }
            }
        }
        if let (Some(s), Some(p)) = (session, pending) {
            let _ = s.save(&p.base_dir);
        }
        done
    }
}

/// The two priorities a matched domain's blocks take (F2): its rules and its CONTEXT. An always-on domain is global
/// (4, 4); a domain matched by a prompt keyword or by a path this session touched is matched to the prompt (1, 2).
fn domain_priorities(reason: &MatchReason) -> (Priority, Priority) {
    match reason {
        MatchReason::Always => (Priority::Global, Priority::Global),
        MatchReason::Keyword | MatchReason::Filepath | MatchReason::KeywordAndFilepath => {
            (Priority::Matched, Priority::Context)
        }
    }
}

/// Lines in a CONTEXT or query block that are records rather than headers, tags or table rules.
fn count_records(text: &str) -> usize {
    text.lines()
        .map(str::trim_start)
        .filter(|l| !l.is_empty() && !l.starts_with('[') && !l.starts_with('<') && !l.starts_with("|-"))
        .count()
}

/// The prompt hook in process: [`collect`], fit to `[budget] prompt_bytes`, record in the session what was printed
/// (D15), and append the printed text to `out`. Without the relay blocks and the files the dispatcher adds; the hook
/// itself runs through `hook::dispatch`. `rules_injected` and `bracket_rules_injected` count what was printed.
pub fn handle(
    config: &BaseConfig,
    cwd: &Path,
    event: &serde_json::Value,
    out: &mut String,
) -> Result<super::HookEventData> {
    let mut sink = PromptSink::default();
    let handled = collect(config, cwd, event, &mut sink);
    let (fitted, committed) = sink.fit_and_commit(config);
    out.push_str(&fitted.text);
    let mut data = handled?;
    data.rules_injected = committed.rules;
    data.bracket_rules_injected = committed.bracket_block;
    Ok(data)
}

impl PromptSink {
    /// Fit the collected blocks to `[budget] prompt_bytes`, then [`PromptSink::commit`] what that printed. The
    /// dispatcher adds the relay blocks to `blocks` before calling it.
    pub fn fit_and_commit(&mut self, config: &BaseConfig) -> (Fitted, Committed) {
        // THE KEY IS THE ONE THAT RESOLVED, NOT A LITERAL. This used to pass "prompt_chars" - the LEGACY spelling -
        // so every over-budget prompt told the operator to raise a key base would then warn them to rename.
        let key = config.budget.key_as_written("prompt_bytes");
        let fitted = crate::emit::prompt::fit(
            &self.header,
            std::mem::take(&mut self.blocks),
            config.budget.prompt_bytes,
            key,
        );
        let committed = self.commit(&fitted);
        (fitted, committed)
    }
}

/// Collect everything this hook says into `sink`; never print, never save the session.
///
/// See [`PromptSink`] for why: the dispatcher owns the one measured write and the one session save, so what is
/// recorded as shown is exactly what was printed.
pub fn collect(
    config: &BaseConfig,
    cwd: &Path,
    event: &serde_json::Value,
    sink: &mut PromptSink,
) -> Result<super::HookEventData> {
    let prompt = extract_prompt(event);
    if prompt.is_empty() {
        return Ok(super::HookEventData::default());
    }
    sink.prompt = prompt.clone();

    // No early return on an empty domain set: bracket rules are tier-gated, not
    // domain-gated, and must still inject for a user with no domains configured.
    // The check moves below, once the bracket block has been built.
    let domains = domain::load_domains(cwd);

    // Resolve base dir: workspace first, fall back to global tier
    let base_dir = crate::config::find_workspace_base(cwd)
        .or_else(|| {
            crate::home::home_root().map(|h| h.join(".base-gbl").join(".base")).filter(|p| p.is_dir())
        });
    let mut session = base_dir
        .as_deref()
        .map(SessionState::load)
        .unwrap_or_default();

    // Session identity: `.session` is per-workspace but several Claude sessions can
    // share a workspace, so the counter must be keyed or they clobber each other.
    let session_id = event.get("session_id").and_then(serde_json::Value::as_str);

    // Real context depletion, read off the live transcript. None on the first
    // prompt (no usage written yet) or an unreadable path → turn-count fallback.
    let context_pct = event
        .get("transcript_path")
        .and_then(serde_json::Value::as_str)
        .and_then(|p| crate::domain::transcript::context_pct(p, config.bracket.context_window));

    // Track prompt count and derive bracket
    session.increment_prompt_for(session_id);
    let bracket = session.bracket_for(&config.bracket, session_id, context_pct);
    // The tool hook serves at this same tier (`petrel` FINDING 1). Its event carries no transcript reading,
    // and a tier it computed from the prompt count instead re-opened every rule record in percent mode.
    session.record_tier(bracket);

    // Force-refresh in DEPLETED and CRITICAL, on an interval. `clear_dedup` clears this session's domain-block
    // hashes (`injected`) and standards hashes, so a matched domain's neighbourhood, query and steering lines
    // are served again. It does NOT clear the per-rule record (`rules_shown`) or the bracket block: since F9
    // a rule is shown again on a tier change, not on an interval, which is spec F8's table (`petrel` F2).
    if session.should_force_refresh_for(&config.bracket, session_id, context_pct) {
        session.clear_dedup();
    }

    // Bracket rules — tier-gated, built before the *command branch so a star command
    // cannot bypass them, and pushed at all four return sites below, so a new return
    // site added later cannot forget them. Priority 5, last (F2).
    //
    // WHAT GOES IN THE BLOCK (BO-03, F3). Each rule in force at this tier, once per
    // session, and only when no CLAUDE.md Claude Code loads already carries it. See
    // `bracket_rules_block` for both rules.
    //
    // ASKED, NOT CLAIMED (D15). Each rule is recorded as sent by `commit`, only if
    // this block was printed, so a rule the budget dropped goes on the next prompt.
    let bracket_block = bracket_rules_block(config, cwd, &mut session, bracket);

    // Deferred from above: no domains to match, but the bracket block still goes
    // out, and so do rules that carry matchers of their own: a rule added with
    // `base rule add --kind` needs no domains.toml entry to fire (4d).
    if domains.is_empty() {
        let store = crate::store::load_merged(cwd);
        let converted = crate::domain::rules::rules_with_matchers(store.as_ref(), config, &domains);
        let prompt_num = session.prompt_count_for(session_id);
        sink.prompt_num = Some(prompt_num);
        let scoring = load_scoring(config, base_dir.as_deref(), &prompt, &mut sink.trace);
        sink.blocks.extend(matcher_blocks(
            config,
            &prompt,
            &session,
            bracket,
            &converted,
            &domains,
            scoring.as_ref(),
            &mut sink.trace,
        ));
        sink.blocks.extend(bracket_block);
        sink.hold(session, base_dir, bracket);
        return Ok(super::HookEventData {
            prompt_num: Some(prompt_num),
            ..Default::default()
        });
    }

    // Check for *COMMAND(s) before domain matching — supports stacking, so
    // "*audit *steelman" activates BOTH modes (every matched *word injects).
    let commands = crate::command::load_commands(cwd);
    let matched = crate::command::match_commands(&prompt, &commands);
    if !matched.is_empty() {
        let cmd_blocks: Vec<PromptBlock> = matched
            .iter()
            .map(|cmd| {
                PromptBlock::new(
                    format!("command-{}", crate::crud::slugify(&cmd.name)),
                    Priority::Matched,
                    &crate::command::format_command_output(cmd),
                    cmd.rules.len(),
                    "rule",
                )
                .with_logged([Item::of_kind(&cmd.name, "command")])
            })
            .filter(|b| !b.text.is_empty())
            .collect();
        if !cmd_blocks.is_empty() {
            // Star commands bypass domain matching — they're explicit invocations.
            // Bracket rules ride along with star commands too — a mode changes
            // stance, it does not suspend the always-on layer.
            let prompt_num = session.prompt_count_for(session_id);
            sink.prompt_num = Some(prompt_num);
            for b in &cmd_blocks {
                sink.trace.matched.extend(b.logged.iter().map(|i| Matched::new(&i.id, "command", None)));
            }
            sink.blocks.extend(cmd_blocks);
            sink.blocks.extend(bracket_block);
            sink.hold(session, base_dir, bracket);
            return Ok(super::HookEventData {
                prompt_num: Some(prompt_num),
                ..Default::default()
            });
        }
    }

    // Ensure domain sync has run BEFORE loading the graph, so the single
    // load below sees freshly synced rules. Marker-gated — no-op when fresh.
    ensure_domain_sync(config, cwd);

    // Single graph load per invocation (merged: global + workspace); the
    // injection loop and the walk share this store.
    let graph_store = crate::store::load_merged(cwd);
    // The decisions of the always-on domains and their keywords (BO-03, F5). One reaches this prompt only when the
    // prompt carries one of its keywords, through the `global-decisions` block below, once per session; the
    // always-on domain's CONTEXT leaves them out, and the walk skips the ones this prompt does not get.
    let global = graph_store
        .as_ref()
        .map(|s| crate::domain::global_decisions::GlobalDecisions::load(s, config, &domains))
        .unwrap_or_default();

    // Rules with matchers of their own (4d): read once, served on this prompt whether or not a domain matches, and
    // kept out of every domain block below (F1).
    let converted = crate::domain::rules::rules_with_matchers(graph_store.as_ref(), config, &domains);
    let converted_ids: HashSet<&str> = converted.iter().map(|c| c.rule.id.as_str()).collect();

    // The paths this session touched (tool-hook log), never the store.
    let active_paths = gather_active_paths(cwd, base_dir.as_deref(), session_id);

    let trigger_ctx = TriggerContext {
        home: crate::home::home_root().map(|h| h.display().to_string()),
        registered: graph_store
            .as_ref()
            .map(|s| crate::domain::registered_projects(s, &config.namespace, cwd))
            .unwrap_or_default(),
    };
    let matched = match_domains_auto(&prompt, &domains, &active_paths, &trigger_ctx);
    for dm in &matched {
        for m in matched_entries(dm) {
            if m.by == "keyword"
                && let Some(kw) = &m.value
            {
                sink.trace.word(kw);
            }
            sink.trace.matched.push(m);
        }
    }
    // BM25 (BO-18, K7): this prompt's score for every rule and global decision, and the rules no keyword or path
    // brought whose score reaches `[match] min_score`, by domain, when one is set. With no index, or `[match] bm25 = false`, there is no
    // scoring and the prompt is served keyword-only, as before.
    let scoring = load_scoring(config, base_dir.as_deref(), &prompt, &mut sink.trace);
    let admitted = scoring
        .as_ref()
        .map(|s| admitted_by_score(s, &prompt, &domains, &matched, &converted_ids))
        .unwrap_or_default();
    for a in &admitted {
        sink.trace.matched.push(Matched::new(&a.domain.name, "score", Some(format!("{:.2}", a.best))));
        for t in &a.terms {
            sink.trace.word(t);
        }
    }
    if matched.is_empty() && admitted.is_empty() {
        // N-WALK-DEAD-WITHOUT-A-MATCHED-DOMAIN. The walk resolves what the
        // PROMPT TEXT names, which has nothing to do with whether a domain
        // trigger fired. Returning here without running it left every machine
        // whose domains are all `mode = "triggered"` with no prompt-time
        // traversal at all -- the headline claim dead on any install with no
        // always-on domain. Same shape, and the same fix, as the early return
        // `base context` used to take in `domain::query`.
        //
        // AND N-BRACKET-DEAD-WITHOUT-A-MATCHED-DOMAIN, the same early return
        // costing the same users a second feature. The bracket line and its
        // rules are tier-gated, never domain-gated, so withholding them because
        // an unrelated domain trigger did not fire left the user with the fewest
        // domains getting nothing at all -- and on an install whose domains are
        // all `mode = "triggered"`, nothing ever.
        //
        // `prompt_count_for(session_id)`, NOT the raw `session.prompt_count`.
        // Of the four return sites in this function this was the only one
        // reaching for the raw counter, and the main path below prints the
        // per-session number -- so the raw one would show a user two different
        // prompt numbers depending on whether a domain happened to match, each
        // inflated by every concurrent session sharing the workspace.
        //
        // The returned `HookEventData` keeps `session.prompt_count` unchanged.
        // That field feeds the JSONL log rather than the prompt, it has always
        // carried that number here, and whether it should is a separate
        // question from what the reader sees.
        //
        // Nothing to dedup against: `domain_served` is filled by the domain
        // loop below, which an empty `matched` makes a no-op.
        //
        // No lean-mode gate either. `lean_mode` needs the prompt number
        // computed below this return -- and the walk on the main path is not
        // gated on it, so gating here would be the one place in the hook where
        // the walk still consulted it.
        let nomatch_prompt_num = session.prompt_count_for(session_id);
        sink.prompt_num = Some(nomatch_prompt_num);
        sink.header = format!("<context-bracket>[{bracket}] (prompt {nomatch_prompt_num})</context-bracket>");
        sink.blocks.extend(matcher_blocks(
            config,
            &prompt,
            &session,
            bracket,
            &converted,
            &domains,
            scoring.as_ref(),
            &mut sink.trace,
        ));
        if let Some(ref store) = graph_store {
            let nothing_served = std::collections::HashSet::new();
            let walked =
                crate::hook::walk::walk_from_text(store, cwd, config, &prompt, &nothing_served, &|id| {
                    global_withheld(&global, &session, id, &prompt)
                });
            // Dedup and render as one unit; the marks wait for the print -- see `render_walk_block`.
            let w = render_walk_block(&session, walked, config.injection.walk_budget);
            sink.blocks.extend(w.blocks);
            sink.trace.cut.extend(w.cut);
        }
        sink.blocks.extend(bracket_block);
        let prompt_count = session.prompt_count;
        sink.hold(session, base_dir, bracket);
        return Ok(super::HookEventData {
            prompt_num: Some(prompt_count),
            ..Default::default()
        });
    }

    // This session's depth — not the workspace-wide total, which concurrent
    // sessions inflate.
    let prompt_num = session.prompt_count_for(session_id);

    // The context bracket tag, always kept and counted first.
    sink.prompt_num = Some(prompt_num);
    sink.header = format!("<context-bracket>[{bracket}] (prompt {prompt_num})</context-bracket>");
    let matcher =
        matcher_blocks(config, &prompt, &session, bracket, &converted, &domains, scoring.as_ref(), &mut sink.trace);
    let matcher_served = matcher.iter().any(PromptBlock::has_claims);
    sink.blocks.extend(matcher);

    // Determine if we're in lean mode (FRESH, first 2 prompts — rules only, skip neighborhood)
    let lean_mode = bracket == Bracket::Fresh && prompt_num <= 2;

    // Track injection metadata for DEVMODE
    let mut loaded_domains: Vec<(String, String, usize)> = Vec::new(); // (name, match_reason, rule_count)
    let mut deduped_count = 0usize;
    // Remember whether any fresh content was injected (gates the grounding block).
    let mut injected_any = matcher_served;
    // Domain-linked command modes (Phase 28), one entry per command: the best priority among the domains that link
    // it, its rendered rules, and how many. Each becomes a block of its own after the loop (BO-01).
    let mut linked: Vec<(String, Priority, String, usize)> = Vec::new();
    // Every record IRI the domain blocks serve this prompt. The walk below dedups
    // against it, so a record cannot arrive twice under two headings.
    let mut domain_served: std::collections::HashSet<String> = std::collections::HashSet::new();
    let now = SessionState::now_secs();

    // Format and emit matched rules
    for dm in &matched {
        let domain_def = dm.domain;
        let (rules_priority, context_priority) = domain_priorities(&dm.reason);

        // The rules and the neighbourhood are read separately now, because they are
        // deduped differently: the rules per RULE (F9), the neighbourhood as a block.
        // A rule with matchers of its own left this block in 4d: it was served on them above (F1).
        let rules: Vec<crate::domain::rules::ServedRule> =
            crate::domain::rules::rules_for_domain(graph_store.as_ref(), config, domain_def)
                .into_iter()
                .filter(|r| !converted_ids.contains(r.id.as_str()))
                .collect();
        // Due, not claimed (D15): `commit` records each rule only if this domain's rules block is printed.
        let mut fresh: Vec<(usize, &crate::domain::rules::ServedRule)> = rules
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                session.rule_due(&r.id, r.content_hash, bracket, None, ReShow::PerSession { on_tier_change: true }, now)
            })
            .collect();
        // Best first when the prompt was scored (BO-18, K7d), today's order breaking ties; each rule keeps its number.
        if let Some(s) = &scoring {
            fresh.sort_by(|a, b| s.scores.get(&b.1.id).total_cmp(&s.scores.get(&a.1.id)));
        }
        // What this prompt serves, so the walk still resolves a rule this block held
        // back and does not list again one it serves.
        domain_served.extend(fresh.iter().filter_map(|(_, r)| r.iri.clone()));
        // D13: a domain that came only as a nested parent says whose parent it is, as the tool hook's block does.
        let label = match &dm.parent_of {
            Some(child) => format!("{} (parent of {child})", domain_def.name),
            None => domain_def.name.clone(),
        };
        let rules_text = crate::domain::rules::render_block_as("DOMAIN", &label, &fresh, rules.len(), &domain_def.name);

        // An always-on domain's CONTEXT block named the domain as served whenever it listed decisions, so the walk
        // never listed the domain itself. Its decisions now have their own block and the CONTEXT is often empty, so
        // the domain is marked here, in lean mode too: the walk would otherwise add "domain GLOBAL" to blocks that
        // reach it (measured on the BO-03 replay, 5 of 300 prompts).
        if domain_def.is_always() {
            domain_served.insert(domain_walk_key(config, domain_def));
        }
        // The decisions the CONTEXT block lists, for the match log (K1).
        let mut context_decisions: Vec<Item> = Vec::new();
        let neighborhood_text = match (&graph_store, lean_mode) {
            (Some(store), false) => {
                let (n, served) =
                    crate::domain::query::query_domain_neighborhood(store, config, domain_def, &|id| {
                        domain_def.is_always() && global.contains(id)
                    });
                if !n.is_empty() {
                    context_decisions = served
                        .iter()
                        .filter(|iri| crate::emit::match_log::is_decision(iri))
                        .map(|iri| Item::decision(iri, Some(&domain_def.name)))
                        .collect();
                }
                domain_served.extend(served);
                n
            }
            _ => String::new(),
        };

        // ─── Steering layer (v0.4): role / linked commands / output-mode / format ───
        // Role (Phase 29): first line of the domain block.
        let role_line = domain_def.role.as_deref().map(str::trim).filter(|r| !r.is_empty());

        // Domain-linked command rules (Phase 28). Explicit *commands short-circuit
        // before domain matching, so this path only fires when no explicit star was
        // typed. BO-01: a linked mode is no longer folded into the first domain's
        // block. Two domains of different priorities can link one mode, and the
        // budget can drop the one that carried it while keeping the other, so the
        // mode is one block, at the better of the two priorities.
        if command_activation_fires(&domain_def.command_activation, &dm.reason) {
            for cmd_name in &domain_def.commands {
                if let Some(cmd) = commands.iter().find(|c| c.name.eq_ignore_ascii_case(cmd_name)) {
                    let rendered = crate::command::format_command_output(cmd);
                    if rendered.is_empty() {
                        continue;
                    }
                    let key = cmd.name.to_lowercase();
                    match linked.iter_mut().find(|(k, ..)| *k == key) {
                        Some(entry) => entry.1 = entry.1.min(rules_priority),
                        None => linked.push((key, rules_priority, rendered, cmd.rules.len())),
                    }
                }
            }
        }

        // Output mode (Phase 31) + format directive (Phase 32).
        let output_mode_line = output_mode_directive(domain_def.output_mode.as_deref());
        let format_line = domain_def.format.as_deref().map(str::trim).filter(|f| !f.is_empty());

        // Notes surface ONLY through explicit queries — no bulk dumps.
        // If a domain needs notes injected, configure `query = "..."` in domains.toml
        // pointing to a SPARQL file in queries/ that filters and shapes the output.
        let query_text = match (&graph_store, &domain_def.query) {
            (Some(store), Some(query_name)) => {
                let fmt = domain_def.query_format.as_deref().unwrap_or("list");
                resolve_and_run_query(store, config, cwd, query_name, fmt, &domain_def.name)
            }
            _ => String::new(),
        };

        // TWO PARTS, DEDUPED AND RANKED APART (BO-01). The rules were deduped one at
        // a time above. The steering lines and the CONTEXT were one block hash until
        // BO-01, and they are two now, each recorded only when its own block is
        // printed: a context the budget keeps dropping stays due, and the steering
        // lines that did print are not sent again with it on every prompt.
        //
        // Hash over SORTED lines: SPARQL result order shifts when the graph file is
        // rewritten (post-tool-use fires on every edit), and an order-sensitive hash
        // would re-inject unchanged content every prompt.
        let sorted_hash = |text: &str| {
            let mut lines: Vec<String> = text.lines().map(String::from).collect();
            lines.sort();
            rules_hash(&lines)
        };
        let steering: Vec<&str> = [role_line, output_mode_line, format_line].into_iter().flatten().collect();
        let steering_hash = sorted_hash(&steering.join("\n"));
        let steering_due = !steering.is_empty() && !session.is_injected(&domain_def.name, steering_hash);
        let context_text: String = [neighborhood_text.as_str(), query_text.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let context_key = format!("context:{}", domain_def.name);
        let context_hash = sorted_hash(&context_text);
        let context_due = !context_text.is_empty() && !session.is_injected(&context_key, context_hash);
        let injected_rule_count = fresh.len();

        // Nothing due. A domain that HAS rules and had every one of them already served
        // this session is not "contributes nothing": it is a dedup, and it has to be
        // COUNTED as one, or it drops out of `HookEventData::suppressed` and the
        // devmode dedup list and reads as a domain that never matched
        // (`graph_injection_test::dedup_skips_unchanged_graph_injection`). A domain
        // with nothing at all to say (no rules, a query that returned nothing) is
        // neither injected nor a dedup, and no longer turns on the grounding block.
        if rules_text.is_empty() && !steering_due && !context_due {
            if !rules.is_empty() || !steering.is_empty() || !context_text.is_empty() {
                deduped_count += 1;
                let dedup_reason = if config.devmode.enabled {
                    format!("dedup [{}]", dm.reason)
                } else {
                    "dedup".into()
                };
                loaded_domains.push((domain_def.name.clone(), dedup_reason, injected_rule_count));
            }
            continue;
        }

        // Use the actual match reason from the matcher (only meaningful in DEVMODE)
        let match_reason = if config.devmode.enabled {
            match &dm.path {
                Some(p) => format!("{} ({p})", dm.reason),
                None => format!("{}", dm.reason),
            }
        } else if domain_def.is_always() {
            "always_on".to_string()
        } else {
            "matched".to_string()
        };
        loaded_domains.push((
            domain_def.name.clone(),
            match_reason,
            injected_rule_count,
        ));

        // The rules block: the steering lines ride with fresh rules, as they always
        // did, and alone when they changed. A domain matched to the prompt puts its
        // rules at 1 and its context at 2; an always-on domain puts both at 4 (F2).
        let slug = crate::crud::slugify(&domain_def.name);
        let with_steering = steering_due || !rules_text.is_empty();
        let rules_part: Vec<&str> = [
            role_line.filter(|_| with_steering),
            Some(rules_text.as_str()),
            output_mode_line.filter(|_| with_steering),
            format_line.filter(|_| with_steering),
        ]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect();
        // Recorded whenever the rules block prints, steering lines or not: the domain's own key says it was served.
        let steering_claim =
            with_steering.then(|| Claim::Injected { key: domain_def.name.clone(), hash: steering_hash });
        let rules_block = match (&scoring, crate::domain::rules::block_lines("DOMAIN", &label, &fresh, rules.len(), &domain_def.name)) {
            // Scored (BO-18): one part per rule, best first, so the budget withholds the weakest first (K7d). The role
            // line rides with the header, the output-mode and format lines with the tail.
            (Some(s), Some(lines)) => {
                let head = lines_of([role_line, Some(lines.header.as_str())]);
                let tail = lines_of([lines.tail.as_deref(), output_mode_line, format_line]);
                let parts = fresh.iter().zip(&lines.rules).map(|((_, r), line)| rule_part(line, r, &domain_def.name, s, None)).collect();
                PromptBlock::ranked(format!("{slug}-rules"), rules_priority, &head, parts, &tail, "rule").with_claims(steering_claim)
            }
            _ => PromptBlock::new(format!("{slug}-rules"), rules_priority, &rules_part.join("\n"), fresh.len(), "rule")
                .with_claims(fresh.iter().map(|(_, r)| Claim::Rule {
                    id: r.id.clone(),
                    content: r.content_hash,
                    scope: None,
                }))
                .with_claims(steering_claim)
                .with_logged(fresh.iter().map(|(_, r)| Item::rule(&r.id, &domain_def.name))),
        };
        injected_any |= !rules_block.text.is_empty();
        sink.blocks.push(rules_block);
        if context_due {
            sink.blocks.push(
                PromptBlock::new(format!("{slug}-context"), context_priority, &context_text, count_records(&context_text), "record")
                    .with_claims([Claim::Injected { key: context_key, hash: context_hash }])
                    .with_logged(context_decisions),
            );
            injected_any = true;
        }
    }

    // K7d (BO-18): the rules no keyword or path brought whose score reached `[match] min_score`, each domain's in its
    // own `[DOMAIN: …]` block at priority 1, best first. Only the rules: the domain's context, steering lines and linked
    // modes still need a keyword or a path.
    if let Some(s) = &scoring {
        for a in &admitted {
            let domain_def = a.domain;
            let rules: Vec<crate::domain::rules::ServedRule> =
                crate::domain::rules::rules_for_domain(graph_store.as_ref(), config, domain_def)
                    .into_iter()
                    .filter(|r| a.ids.contains(&r.id))
                    .collect();
            let mut fresh: Vec<(usize, &crate::domain::rules::ServedRule)> = rules
                .iter()
                .enumerate()
                .filter(|(_, r)| {
                    session.rule_due(&r.id, r.content_hash, bracket, None, ReShow::PerSession { on_tier_change: true }, now)
                })
                .collect();
            fresh.sort_by(|x, y| s.scores.get(&y.1.id).total_cmp(&s.scores.get(&x.1.id)));
            domain_served.extend(fresh.iter().filter_map(|(_, r)| r.iri.clone()));
            let Some(lines) = crate::domain::rules::block_lines("DOMAIN", &domain_def.name, &fresh, rules.len(), &domain_def.name)
            else {
                continue;
            };
            let parts = fresh
                .iter()
                .zip(&lines.rules)
                .map(|((_, r), line)| rule_part(line, r, &domain_def.name, s, Some("score")))
                .collect();
            let slug = crate::crud::slugify(&domain_def.name);
            let block = PromptBlock::ranked(format!("{slug}-rules"), Priority::Matched, &lines.header, parts, lines.tail.as_deref().unwrap_or(""), "rule");
            injected_any |= !block.text.is_empty();
            loaded_domains.push((domain_def.name.clone(), "score".to_string(), fresh.len()));
            sink.blocks.push(block);
        }
    }

    // F5: the global decisions this prompt names by keyword, each once per session (and again after a DEPLETED or
    // CRITICAL force-refresh, as CONTEXT is) and recorded only if printed (D15), at priority 4 with the rest of the
    // always-on layer. Not held back in lean mode: unlike the
    // neighbourhood, a keyword is a direct match to what the prompt is about. A decision another domain's
    // CONTEXT already printed on this prompt is not listed twice.
    let (decisions_text, listed) = global.prompt_block(&prompt, &|d| {
        let (key, hash) = d.claim_key();
        session.is_injected(&key, hash) || domain_served.contains(&d.id)
    });
    if !listed.is_empty() {
        domain_served.extend(listed.iter().map(|d| d.id.clone()));
        match &scoring {
            // Scored (BO-18): ranked like the rules, admitted only by their keywords as before (F5, lynx's G0 Q4). One
            // block per always-on domain, so each keeps its header over its own decisions.
            Some(s) => {
                let mut by_domain: Vec<(&str, Vec<&crate::domain::global_decisions::GlobalDecision>)> = Vec::new();
                for d in &listed {
                    match by_domain.iter_mut().find(|(name, _)| *name == d.domain) {
                        Some((_, list)) => list.push(d),
                        None => by_domain.push((&d.domain, vec![*d])),
                    }
                }
                for (domain, mut list) in by_domain {
                    list.sort_by(|a, b| s.scores.get(&b.id).total_cmp(&s.scores.get(&a.id)));
                    let head = format!("[{domain} CONTEXT · decisions matched by keyword]");
                    let parts = list
                        .iter()
                        .map(|d| {
                            let score = s.scores.get(&d.id);
                            let (key, hash) = d.claim_key();
                            BlockPart::new(&format!("  - Decision: {}", d.name), score)
                                .with_claims([Claim::Injected { key, hash }])
                                .with_logged([Item { score: (score > 0.0).then_some(score), ..Item::decision(&d.id, Some(&d.domain)) }])
                        })
                        .collect();
                    sink.blocks.push(PromptBlock::ranked("global-decisions", Priority::Global, &head, parts, "", "decision"));
                }
            }
            None => sink.blocks.push(
                PromptBlock::new("global-decisions", Priority::Global, &decisions_text, listed.len(), "decision")
                    .with_claims(listed.iter().map(|d| {
                        let (key, hash) = d.claim_key();
                        Claim::Injected { key, hash }
                    }))
                    .with_logged(listed.iter().map(|d| Item::decision(&d.id, Some(&d.domain)))),
            ),
        }
        injected_any = true;
    }

    // The linked command modes, each once per session per text, recorded only if printed (D15).
    for (key, priority, text, rules) in linked {
        let claim_key = format!("command:{key}");
        let hash = rules_hash(std::slice::from_ref(&text));
        if session.is_injected(&claim_key, hash) {
            continue;
        }
        sink.blocks.push(
            PromptBlock::new(format!("command-{}", crate::crud::slugify(&key)), priority, &text, rules, "rule")
                .with_claims([Claim::Injected { key: claim_key, hash }])
                .with_logged([Item::of_kind(&key, "command")]),
        );
        injected_any = true;
    }

    // ─── Prompt-time traversal ───────────────────────────────────────────
    // AFTER the domain loop, not before: it dedups against the IRIs those
    // blocks served, and it cannot do that before they have run.
    //
    // NOT gated on lean mode, deliberately. The flag it used to sit behind was
    // introduced by fb1cd48 (2026-06-01) for the NEIGHBOURHOOD, three months
    // before this walk existed -- `git show fb1cd48:src/hook/user_prompt_submit.rs`
    // contains no walk at all. 29ff99a (2026-09-07) then hung the walk on the
    // same flag without the question being asked, so there was never a design
    // intent here to preserve.
    //
    // And the saving was not real. Skipping the injection does not reduce what
    // the session consumes, it moves the cost and makes it larger: the session
    // then spends more than the block's bytes running recall, greps and file
    // reads to find by hand what the block would have handed it. An injection is
    // cheaper than the search it replaces. Prompts 1 and 2 are usually where
    // someone starts work, which is where that signal is worth most.
    //
    // The NEIGHBOURHOOD skip above still honours lean mode. That is what the
    // flag was built for and it is untouched.
    //
    // The maps, the closures and the reasons for both now live in
    // `walk::walk_from_text`, which `base context` calls as well. One seam, so the
    // command and the prompt path cannot answer differently about the same graph.
    let walked = graph_store.as_ref().map(|store| {
        crate::hook::walk::walk_from_text(store, cwd, config, &prompt, &domain_served, &|id| {
            global_withheld(&global, &session, id, &prompt)
        })
    });

    // The walk's blocks rank at 2 with the domains' context, after it, so a reader
    // sees the configured layer first and the named-thing layer as the specific addition.
    let mut walk_note = String::new();
    if let Some(walked) = walked {
        // Dedup and render as one unit; the marks wait for the print -- see
        // `render_walk_block`. The no-match return above is the other caller.
        let mut w = render_walk_block(&session, walked, config.injection.walk_budget);
        if config.devmode.enabled && w.deduped > 0 {
            walk_note.push_str(&format!("  walk: {} name(s) already injected this session\n", w.deduped));
        }
        if !w.blocks.is_empty() {
            injected_any = true;
        }
        sink.trace.cut.extend(std::mem::take(&mut w.cut));
        if config.devmode.enabled {
            for (r, recs) in &w.served {
                walk_note.push_str(&format!(
                    "  walk: {} → {} ({}) {} hop(s), {} record(s){}\n",
                    r.name,
                    r.id.trim_matches(['<', '>']),
                    r.kind,
                    r.hops,
                    recs.len(),
                    if r.ties > 1 { format!(", {} same-kind ties", r.ties) } else { String::new() },
                ));
            }
            if w.dropped > 0 {
                walk_note.push_str(&format!("  walk: {} record(s) dropped by walk_budget\n", w.dropped));
            }
        }
        sink.blocks.extend(w.blocks);
    }

    // Test 5's instrument, and kite F9's. Both claims -- one parse per prompt,
    // and no AST sidecar on the prompt path -- are invisible in the output: the
    // injected text is identical whether the graph was parsed once or twice.
    // So the counts are REPORTED rather than inferred, and the hook harness
    // asserts on this line instead of on a stopwatch.
    //
    // Outside the `if let Some(walked)` above on purpose: a prompt that resolves
    // no names is exactly where an accidental second parse would hide.
    if config.devmode.enabled {
        walk_note.push_str(&format!(
            "  parses: graph={} ast={}\n",
            crate::store::graph_loads(),
            crate::graph_query::ast_loads(),
        ));
    }

    // Grounding (Phase 30): when enabled, ride a source-verification block on any
    // fresh injection this prompt. Skipped on dedup-only prompts (already grounded).
    // Priority 4: an instruction for every prompt, not one matched to this one.
    if config.grounding.enabled && injected_any {
        sink.blocks.push(PromptBlock::new("grounding", Priority::Global, &grounding_block(), 1, "instruction"));
    }

    sink.blocks.extend(bracket_block);

    // DEVMODE, last: priority 5 after the bracket rules. What it lists as loaded is what the hook built; the
    // budget may still drop a block it names, and the pointer line for it says so.
    if config.devmode.enabled {
        let mut devmode = format_devmode_block(
            &loaded_domains,
            &domains,
            bracket,
            session.prompt_count,
            deduped_count,
        );
        // An inert trigger is named on every prompt it would otherwise have judged, so a
        // domain that stopped loading is never a silent drop (F29 step 6).
        for (domain, trigger, fault) in crate::domain::matcher::inert_triggers(&domains, &trigger_ctx) {
            devmode.push_str(&format!(
                "  inert: {}\n",
                crate::domain::matcher::fault_sentence(domain, trigger, &fault)
            ));
        }
        if !walk_note.is_empty() {
            devmode.push_str(&walk_note);
        }
        sink.blocks.push(PromptBlock::new("devmode", Priority::Bracket, &devmode, 1, "block"));
    }

    // Build event data for JSONL logging
    let domains_matched: Vec<String> = loaded_domains
        .iter()
        .filter(|(_, reason, _)| !reason.starts_with("dedup"))
        .map(|(name, _, _)| name.clone())
        .collect();

    // Capture first 120 chars of the prompt for dashboard display
    let prompt_preview = if prompt.len() > 120 {
        let truncated: String = prompt.char_indices()
            .take_while(|(i, _)| *i < 117)
            .map(|(_, c)| c)
            .collect();
        Some(format!("{truncated}…"))
    } else {
        Some(prompt.clone())
    };

    let prompt_count = session.prompt_count;
    // The session is saved by `PromptSink::commit`, once the output is fitted: AFTER the walk, so the prompt
    // count and tier it recorded are in what gets saved, and with only the marks of what was printed.
    sink.hold(session, base_dir, bracket);

    // `rules_injected` and `bracket_rules_injected` are filled by the dispatcher from what `commit` recorded:
    // what was printed, not what was built.
    Ok(super::HookEventData {
        domains_matched,
        suppressed: deduped_count,
        prompt_num: Some(prompt_count),
        prompt_text: prompt_preview,
        tool_name: None,
        file_path: None,
        session_id: None, // populated by run() after handle returns
        ..Default::default()
    })
}

/// The match log's entries for one matched domain (K1): `always`; one per keyword that hit; and, for a path this
/// session touched, `path` with the folder or trigger that held it, or `parent` with `<child> nested` (D13).
fn matched_entries(dm: &crate::domain::matcher::DomainMatch) -> Vec<Matched> {
    let name = &dm.domain.name;
    let mut out = Vec::new();
    if dm.reason == MatchReason::Always {
        out.push(Matched::new(name, "always", None));
    }
    for kw in &dm.keywords {
        out.push(Matched::new(name, "keyword", Some(kw.clone())));
    }
    if let Some(path) = &dm.path {
        let (by, value) = match &dm.parent_of {
            Some(child) => ("parent", Some(format!("{child} nested"))),
            None => ("path", dm.held_by.clone()),
        };
        out.push(Matched { path: Some(path.clone()), ..Matched::new(name, by, value) });
    }
    out
}

/// A domain's record in the walk's key form, `<iri>`.
pub(crate) fn domain_walk_key(config: &BaseConfig, domain_def: &domain::DomainDef) -> String {
    format!("<{}>", crate::crud::build_iri(&config.namespace, "domain", &crate::crud::slugify(&domain_def.name)))
}

/// The walk's F5 filter: a global decision filed only under always-on domains that this prompt does not name by
/// keyword, or any global decision this session was already given by the `global-decisions` block.
fn global_withheld(
    global: &crate::domain::global_decisions::GlobalDecisions,
    session: &SessionState,
    id: &str,
    prompt: &str,
) -> bool {
    global.withheld_from(id, prompt)
        || global.get(id).is_some_and(|d| {
            let (key, hash) = d.claim_key();
            session.is_injected(&key, hash)
        })
}

/// The bracket rules this prompt sends, as one block, or `None` when there are none (BO-03, F3).
///
/// Two rules decide, in this order:
///
/// 1. COVERED BY CLAUDE.md. A rule whose `covered_by` text is in a CLAUDE.md file Claude Code loads for this
///    session is never sent: the reader already has it. On 2026-10-01 the T1 to T6 rules took about 2,400 of
///    3,864 bytes of a prompt while `~/.claude/CLAUDE.md` carried the same rules. Decided once per session per
///    rule text (`SessionState::bracket_covered` says why): the first prompt that meets a marked rule reads the
///    files and records a verdict for every marked rule of every tier; a config without markers never reads them.
///    A text is covered when any entry carrying it is, so the same text in two buckets is one rule here too.
/// 2. ONCE PER SESSION. A rule goes out on the first prompt where its tier applies and never again in the
///    session; a tier change sends only the new tier's rules not yet sent (`SessionState::bracket_rule_due`).
///
/// The block's header still names the tier, and the `<context-bracket>` line names it on every prompt.
fn bracket_rules_block(config: &BaseConfig, cwd: &Path, session: &mut SessionState, bracket: Bracket) -> Option<PromptBlock> {
    let rules = &config.bracket.rules;
    let marked: Vec<&crate::config::BracketRule> = [&rules.always, &rules.fresh, &rules.moderate, &rules.depleted, &rules.critical]
        .into_iter()
        .flatten()
        .filter(|r| r.markers().next().is_some())
        .collect();
    if marked.iter().any(|r| session.bracket_coverage(&r.text).is_none()) {
        let loaded = crate::claude_md::loaded_text(cwd);
        let mut verdict: HashMap<&str, bool> = HashMap::new();
        for r in &marked {
            *verdict.entry(r.text.as_str()).or_insert(false) |= r.covered_in(&loaded);
        }
        for (text, covered) in verdict {
            if session.bracket_coverage(text).is_none() {
                session.record_bracket_coverage(text, covered);
            }
        }
    }
    let mut texts: Vec<&str> = Vec::new();
    for rule in bracket.entries(rules).into_iter().filter(|r| {
        !r.text.trim().is_empty() && session.bracket_rule_due(&r.text) && session.bracket_coverage(&r.text) != Some(true)
    }) {
        // The same text twice in one tier (in `always` and in the tier's bucket) is one rule, sent once.
        if !texts.contains(&rule.text.as_str()) {
            texts.push(&rule.text);
        }
    }
    let text = crate::domain::session::render_bracket_rules(bracket, &texts);
    if text.is_empty() {
        return None;
    }
    let claims = texts.iter().map(|t| {
        let (id, content) = crate::domain::session::bracket_rule_key(t);
        Claim::BracketRule { id, content }
    });
    let logged = texts.iter().map(|t| Item::of_kind(&crate::domain::session::bracket_rule_key(t).0, "bracket-rule"));
    Some(
        PromptBlock::new("bracket-rules", Priority::Bracket, &text, texts.len(), "rule")
            .with_claims(claims)
            .with_logged(logged),
    )
}

/// This prompt's BM25 scores (BO-18, K7c) and the `[match] min_score` they are judged against, when one is set.
pub(crate) struct Scoring {
    pub(crate) scores: crate::domain::score_index::Scores,
    pub(crate) min_score: Option<f32>,
}

/// Score the prompt against the index the last sync counted (K7e: loaded here, never counted here). `None` when
/// `[match] bm25 = false`, or when no index has been built yet; the match log then says `index: missing`. Either way the
/// prompt is served keyword-only, byte for byte as before BO-18. Every score above zero goes in the match log (K7f).
fn load_scoring(config: &BaseConfig, base_dir: Option<&Path>, prompt: &str, trace: &mut Trace) -> Option<Scoring> {
    if !config.matching.bm25 {
        return None;
    }
    let index = base_dir.and_then(crate::domain::score_index::ScoreIndex::load);
    trace.index = Some(if index.is_some() { "ok" } else { "missing" }.to_string());
    let index = index?;
    trace.min_score = config.matching.min_score;
    let scores = index.scores(prompt);
    for s in &scores.ranked {
        let id = match s.doc.kind {
            DocKind::Rule => s.doc.id.clone(),
            DocKind::Decision => crate::emit::match_log::decision_id(&s.doc.id),
        };
        trace.scores.push(Score { id, domain: s.doc.domain.clone(), score: s.score, by: "bm25".into() });
    }
    Some(Scoring { scores, min_score: config.matching.min_score })
}

/// A domain no keyword or path brought, with its rules whose BM25 score reached `[match] min_score` (K7d).
struct Admitted<'a> {
    domain: &'a domain::DomainDef,
    ids: HashSet<String>,
    /// Its best rule's score.
    best: f32,
    /// The prompt's terms its admitted rules hold, for the match log's `matched` text.
    terms: Vec<String>,
}

/// The rules a keyword or a path did not bring and their score did, grouped by domain, best domain first. None while
/// `[match] min_score` is unset (lynx's Q7 ruling). A domain that is always-on, has `auto_inject = false`, matched
/// already, or is vetoed by one of its `exclude` patterns admits nothing; a rule with matchers of its own is judged by
/// `select` instead.
fn admitted_by_score<'a>(
    scoring: &Scoring,
    prompt: &str,
    domains: &'a [domain::DomainDef],
    matched: &[crate::domain::matcher::DomainMatch<'_>],
    converted: &HashSet<&str>,
) -> Vec<Admitted<'a>> {
    let Some(min) = scoring.min_score else { return Vec::new() };
    let lower = prompt.to_lowercase();
    let mut out: Vec<Admitted<'a>> = Vec::new();
    for s in scoring.scores.ranked.iter().filter(|s| s.doc.kind == DocKind::Rule && s.score > 0.0 && s.score >= min) {
        if converted.contains(s.doc.id.as_str()) {
            continue;
        }
        let Some(d) = domains.iter().find(|d| d.name == s.doc.domain) else { continue };
        if !d.auto_inject
            || d.is_always()
            || matched.iter().any(|m| m.domain.name == d.name)
            || d.exclude.iter().any(|p| lower.contains(&p.to_lowercase()))
        {
            continue;
        }
        match out.iter_mut().find(|a| a.domain.name == d.name) {
            Some(a) => {
                a.ids.insert(s.doc.id.clone());
                for t in &s.terms {
                    if !a.terms.contains(t) {
                        a.terms.push(t.clone());
                    }
                }
            }
            None => out.push(Admitted {
                domain: d,
                ids: HashSet::from([s.doc.id.clone()]),
                best: s.score,
                terms: s.terms.clone(),
            }),
        }
    }
    out
}

/// One rule as a part of a ranked block (BO-18): its line, its score, its claim (D15) and its match-log item, with
/// `by` set for a rule admitted by score.
fn rule_part(
    line: &str,
    rule: &crate::domain::rules::ServedRule,
    domain: &str,
    scoring: &Scoring,
    by: Option<&str>,
) -> BlockPart {
    let score = scoring.scores.get(&rule.id);
    BlockPart::new(line, score)
        .with_claims([Claim::Rule { id: rule.id.clone(), content: rule.content_hash, scope: None }])
        .with_logged([Item {
            score: (score > 0.0).then_some(score),
            by: by.map(String::from),
            ..Item::rule(&rule.id, domain)
        }])
}

/// The lines given, one per line, the empty and missing ones left out.
fn lines_of<'a>(lines: impl IntoIterator<Item = Option<&'a str>>) -> String {
    lines.into_iter().flatten().filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

/// Rules that carry matchers of their own, for one prompt (4d): always rules on the session's first prompt and on
/// each tier change, and topic rules ranked against the prompt and capped at `topic_max`, with F6's pointer line
/// for what the cap withheld (F6, F7, F8).
///
/// One block per header (F2): topic rules are matched to the prompt (1), `always` rules are global (4). Nothing is
/// recorded here; each block carries its rules as claims, recorded only if the block is printed (D15).
///
/// Three callers in `handle`, one per return site that is not a star command: no domains at all, no domain matched,
/// and the main path. A star command is an explicit invocation and passes rules by, as it passes domains by.
#[allow(clippy::too_many_arguments)] // the hook's own state, passed through from its three callers
fn matcher_blocks(
    config: &BaseConfig,
    prompt: &str,
    session: &SessionState,
    bracket: Bracket,
    converted: &[crate::domain::rules::Converted],
    domains: &[domain::DomainDef],
    scoring: Option<&Scoring>,
    trace: &mut Trace,
) -> Vec<PromptBlock> {
    if converted.is_empty() {
        return Vec::new();
    }
    let keywords: HashMap<String, Vec<String>> =
        domains.iter().map(|d| (d.name.clone(), d.prompt_keywords.clone())).collect();
    let home = crate::home::home_root().map(|h| h.display().to_string());
    let cx = crate::domain::rules::SelectContext {
        bracket,
        now: SessionState::now_secs(),
        home: home.as_deref(),
        keywords: &keywords,
        rules: &config.rules,
        bm25: scoring.map(|s| (&s.scores, s.min_score)),
    };
    let event = crate::domain::rules::Event::Prompt { text: prompt };
    let selection = crate::domain::rules::select_unrecorded(converted, &event, session, &cx);
    // K1: what `select` cut and the scores it used, read off the selection, never scored again. The words of a rule
    // that reached the minimum are words that matched.
    trace.cut.extend(crate::domain::rules::cut_items(&selection));
    for s in &selection.scores {
        trace.scores.push(crate::emit::match_log::Score { id: s.id.clone(), domain: s.domain.clone(), score: s.score, by: "topic".into() });
        if s.score >= config.rules.topic_min_score {
            for w in &s.words {
                trace.word(w);
            }
        }
    }
    let mut blocks: Vec<PromptBlock> = Vec::new();
    for g in crate::domain::rules::selection_groups(&selection) {
        let (id, priority) = match &g.topic_domain {
            Some(domain) => (format!("{}-topic-rules", crate::crud::slugify(domain)), Priority::Matched),
            None if g.why == crate::domain::rules::Why::Always => ("always-rules".to_string(), Priority::Global),
            None => ("matched-rules".to_string(), Priority::Matched),
        };
        let mut text = g.text.clone();
        if let Some(domain) = &g.topic_domain
            && let Some((_, n)) = selection.topic_withheld.iter().find(|(d, _)| d == domain)
        {
            text.push_str(&crate::domain::rules::topic_withheld_line(domain, *n));
        }
        let claims = g.served.iter().map(|s| Claim::Rule {
            id: s.rule.id.clone(),
            content: s.rule.content_hash,
            scope: s.why.scope().map(String::from),
        });
        // A topic group of a scored prompt is ranked (BO-18): `select` already put its rules best first.
        if let (Some(s), Some(domain)) = (scoring, &g.topic_domain) {
            let head = g.text.lines().next().unwrap_or_default();
            let tail = selection
                .topic_withheld
                .iter()
                .find(|(d, _)| d == domain)
                .map(|(d, n)| crate::domain::rules::topic_withheld_line(d, *n))
                .unwrap_or_default();
            let items = crate::domain::rules::served_items(&g.served);
            let parts = g
                .served
                .iter()
                .zip(claims)
                .zip(items)
                .map(|((r, claim), item)| {
                    BlockPart::new(&format!("  - {}", r.rule.rendered), s.scores.get(&r.rule.id))
                        .with_claims([claim])
                        .with_logged([item])
                })
                .collect();
            blocks.push(PromptBlock::ranked(id, priority, head, parts, &tail, "rule"));
            continue;
        }
        blocks.push(
            PromptBlock::new(id, priority, &text, g.served.len(), "rule")
                .with_claims(claims)
                .with_logged(crate::domain::rules::served_items(&g.served)),
        );
    }
    // A domain whose every topic rule the cap cut has no group to carry its pointer line: it goes alone.
    for (domain, n) in &selection.topic_withheld {
        if !selection_has_topic_group(&selection, domain) {
            blocks.push(PromptBlock::new(
                format!("{}-topic-rules", crate::crud::slugify(domain)),
                Priority::Matched,
                &crate::domain::rules::topic_withheld_line(domain, *n),
                0,
                "rule",
            ));
        }
    }
    blocks
}

fn selection_has_topic_group(selection: &crate::domain::rules::Selection, domain: &str) -> bool {
    selection
        .served
        .iter()
        .any(|s| matches!(s.why, crate::domain::rules::Why::Topic(_)) && s.rule.domain == domain)
}

/// The walk's two steps as one unit: dedup against what this session already
/// served, then render under the byte budget, one block per name.
///
/// They are a unit because splitting them produces a defect in one of two
/// directions, and a single-prompt test sees neither. Drop the dedup and a name
/// resolved once is re-served on every following prompt. Mark before the render
/// rather than after, and a name whose records were all cut by the budget is
/// recorded as injected and never shown again this session -- the budget
/// quietly becoming a permanent suppression, which looks exactly like dedup
/// working.
///
/// The third step, marking, now waits for the print (D15, BO-01): each name's
/// block carries its mark as a claim, recorded only if the prompt budget kept that
/// block. The same reasoning as above, one budget further on.
///
/// Two callers, both in `handle`: the no-match early return and the main path
/// below it. `domain::query::context_pull` is deliberately not a third -- it
/// has no session, so it has nothing to dedup against and nothing to mark.
struct WalkBlock {
    /// One block per name that rendered, in render order.
    blocks: Vec<PromptBlock>,
    /// How many names dedup dropped, for the devmode note.
    deduped: usize,
    /// How many records the budget dropped, for the devmode note.
    dropped: usize,
    /// What survived dedup, in render order, for the devmode per-name lines.
    served: Vec<(crate::hook::walk::Resolved, Vec<crate::hook::walk::Record>)>,
    /// The decisions the walk budget dropped, for the match log (K1).
    cut: Vec<Cut>,
}

fn render_walk_block(
    session: &SessionState,
    walked: Vec<(crate::hook::walk::Resolved, Vec<crate::hook::walk::Record>)>,
    budget: usize,
) -> WalkBlock {
    // Once per session per node, not once per prompt. Naming the same project
    // in five consecutive prompts is one context, not five: the domain layer
    // has always worked this way and the record layer has no reason to be
    // noisier. Keyed on the resolved IRI rather than the spelling, so
    // "First Client Kit" and "first-client-kit" are one entry.
    let mut served: Vec<(crate::hook::walk::Resolved, Vec<crate::hook::walk::Record>)> = Vec::new();
    let mut deduped = 0usize;
    for (r, recs) in walked {
        let key = format!("walk:{}", r.id);
        let h = crate::domain::session::rules_hash(std::slice::from_ref(&r.id));
        if session.is_injected(&key, h) {
            deduped += 1;
            continue;
        }
        served.push((r, recs));
    }
    let (rendered, dropped, written) = crate::hook::walk::render_counted(&served, budget);
    // The pieces below come out in `served` order, one per name that wrote a record: the n-th piece is the n-th name
    // with records written, and its records are that name's first `written` ones.
    let mut shown = served.iter().zip(&written).filter(|(_, n)| **n > 0);
    // One block per `<base-context>` element. A name the walk budget squeezed out
    // entirely rendered no element, so it has no block and no claim, and is free to
    // come back next prompt.
    let mut blocks = Vec::new();
    for piece in rendered.split_inclusive("</base-context>\n").filter(|p| !p.trim().is_empty()) {
        let head = piece.lines().next().unwrap_or_default();
        let mine: Vec<&crate::hook::walk::Resolved> = served
            .iter()
            .map(|(r, _)| r)
            .filter(|r| head.contains(&format!("name=\"{}\"", r.name)))
            .collect();
        let name = mine.first().map(|r| r.name.as_str()).unwrap_or("context");
        let records = piece.lines().filter(|l| l.starts_with("  ")).count();
        let claims = mine.iter().map(|r| Claim::Injected {
            key: format!("walk:{}", r.id),
            hash: crate::domain::session::rules_hash(std::slice::from_ref(&r.id)),
        });
        let logged: Vec<Item> = shown
            .next()
            .map(|((_, recs), n)| {
                recs.iter().take(*n).filter(|r| r.kind == "decision").map(|r| Item::decision(&r.id, None)).collect()
            })
            .unwrap_or_default();
        blocks.push(
            PromptBlock::new(format!("walk-{}", crate::crud::slugify(name)), Priority::Context, piece, records, "record")
                .with_claims(claims)
                .with_logged(logged),
        );
    }
    let cut = served
        .iter()
        .zip(&written)
        .flat_map(|((_, recs), n)| recs.iter().skip(*n))
        .filter(|r| r.kind == "decision")
        .map(|r| Cut::new(Item::decision(&r.id, None), "budget", "walk_budget"))
        .collect();
    WalkBlock { blocks, deduped, dropped, served, cut }
}

// ─── DEVMODE output ─────────────────────────────────────────

/// Format the DEVMODE instruction block for Claude.
pub fn format_devmode_block(
    loaded: &[(String, String, usize)],
    all_domains: &[domain::DomainDef],
    bracket: Bracket,
    prompt_count: u32,
    deduped: usize,
) -> String {
    let mut out = String::new();
    out.push_str("\n⚠️ DEVMODE=true ⚠️\n");
    out.push_str("============================================================\n");
    out.push_str("MANDATORY: Append a DEVMODE block at the end of EVERY response.\n");
    out.push_str("NEVER skip it. NEVER forget it. NEVER omit it for any reason.\n");
    out.push_str("NEVER fabricate data in the block — only report what you actually received.\n\n");
    out.push_str("Format EXACTLY (keep under 8 lines, no rationale, no prose):\n");
    out.push_str("---\n```\n");
    out.push_str("🔧 DEVMODE\n");
    out.push_str("Bracket: [X] (prompt N)\n");
    out.push_str("Loaded: domain1 [reason] (N rules), domain2 [reason] (dedup)\n");
    out.push_str("Available: domain3, domain4, ...\n");
    out.push_str("Dedup: N skipped\n");
    out.push_str("Tools: tools used this response, or 'none'\n");
    out.push_str("```\n---\n");
    out.push_str("============================================================\n\n");

    // Bracket info
    out.push_str(&format!(
        "CONTEXT BRACKET: [{bracket}] (prompt {prompt_count})\n\n"
    ));

    // Loaded domains
    out.push_str("LOADED DOMAINS:\n");
    for (name, reason, rule_count) in loaded {
        if reason.starts_with("dedup") {
            out.push_str(&format!(
                "  [{name}] {reason} (prompt {prompt_count})\n"
            ));
        } else {
            out.push_str(&format!(
                "  [{name}] {reason} ({rule_count} rules)\n"
            ));
        }
    }

    // Available (not loaded) domains
    let loaded_names: Vec<&str> = loaded.iter().map(|(n, _, _)| n.as_str()).collect();
    let available: Vec<&domain::DomainDef> = all_domains
        .iter()
        .filter(|d| !loaded_names.contains(&d.name.as_str()) && !d.is_always())
        .collect();

    if !available.is_empty() {
        out.push_str("\nAVAILABLE (not loaded):\n");
        for d in &available {
            let kws = d.prompt_keywords.join(", ");
            out.push_str(&format!("  {} ({})\n", d.name, kws));
        }
    }

    if deduped > 0 {
        out.push_str(&format!("\nDEDUP: {deduped} domain(s) skipped (unchanged)\n"));
    }

    out
}

// ─── Graph-backed injection ─────────────────────────────────

// ─── Auto-sync ──────────────────────────────────────────────

/// Public wrapper for pre_tool_use to call.
pub fn ensure_domain_sync_pub(config: &BaseConfig, cwd: &Path) {
    ensure_domain_sync(config, cwd);
}

/// Ensure domains.toml has been synced to the graph this session.
/// Uses a timestamp marker file to avoid re-syncing on every prompt.
/// Syncs both global (~/.base-gbl/) and workspace tiers.
fn ensure_domain_sync(config: &BaseConfig, cwd: &Path) {
    // Global tier: sync ~/.base-gbl/domains.toml → ~/.base-gbl/.base/graph.nq
    if let Some(home) = crate::home::home_root() {
        let global_dir = home.join(".base-gbl");
        let global_base = global_dir.join(".base");
        if global_base.is_dir() {
            let marker = global_base.join(".domain-sync-ts");
            let domains_toml = global_dir.join("domains.toml");
            if domains_toml.exists() {
                let needs_sync = needs_sync_check(&domains_toml, &marker);
                if needs_sync
                    && domain::sync::sync_domains_to_graph(config, &global_dir, None).is_ok() {
                        let _ = touch_sync_marker(&marker);
                    }
            }
        }
    }

    // Workspace tier: sync {workspace}/.base/domains.toml → {workspace}/.base/graph.nq
    let base_dir = match crate::config::find_workspace_base(cwd) {
        Some(d) => d,
        None => return,
    };

    let marker = base_dir.join(".domain-sync-ts");
    let domains_toml = base_dir.join("domains.toml");

    if !domains_toml.exists() {
        return;
    }

    let needs_sync = needs_sync_check(&domains_toml, &marker);
    if needs_sync
        && domain::sync::sync_domains_to_graph(config, cwd, None).is_ok() {
            let _ = touch_sync_marker(&marker);
        }
}

/// Close the guard: the marker must land NEWER than `domains.toml` on every
/// filesystem, and an empty write does not do that. On NTFS, `fs::write(path, "")`
/// onto an existing empty file leaves LastWriteTime exactly where it was (measured
/// 2026-09-03 with a compiled probe on a copy of a real marker; the guard had been
/// open on that machine since June, costing two full graph rewrites per prompt).
/// A non-empty write moves it everywhere. The content is the sync time, so the
/// file also says when the last good sync ran.
fn touch_sync_marker(marker: &Path) -> std::io::Result<()> {
    let stamp = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
    std::fs::write(marker, format!("{stamp}\n"))
}

/// Check if a domains.toml is newer than its sync marker. Pure and path-scoped
/// (the test seam): `true` when the marker is missing or unreadable.
pub fn needs_sync_check(domains_toml: &Path, marker: &Path) -> bool {
    if marker.exists() {
        match (
            std::fs::metadata(domains_toml).and_then(|m| m.modified()),
            std::fs::metadata(marker).and_then(|m| m.modified()),
        ) {
            (Ok(toml_time), Ok(marker_time)) => toml_time > marker_time,
            _ => true,
        }
    } else {
        true
    }
}

// ─── Steering layer helpers (v0.4) ──────────────────────────

/// Whether a domain's linked `commands` should auto-activate, given how it
/// matched. "disabled" suppresses; "keyword"/"filepath" gate on the match
/// reason; "both" (default) and any unknown value activate on any match (Phase 28).
fn command_activation_fires(activation: &str, reason: &crate::domain::matcher::MatchReason) -> bool {
    use crate::domain::matcher::MatchReason::*;
    match activation {
        "disabled" => false,
        "keyword" => matches!(reason, Keyword | KeywordAndFilepath | Always),
        "filepath" => matches!(reason, Filepath | KeywordAndFilepath),
        _ => true,
    }
}

/// Render the output-mode directive for a domain (Phase 31). "ask"/None/unknown
/// inject nothing — the model decides per prompt.
fn output_mode_directive(mode: Option<&str>) -> Option<&'static str> {
    match mode.map(str::trim) {
        Some("file") => Some("Default output mode: write to file artifacts."),
        Some("inline") => Some("Default output mode: respond inline in chat."),
        _ => None,
    }
}

/// The grounding block appended to injections when the system flag is on (Phase 30).
fn grounding_block() -> String {
    "\n<grounding>\n\
     Verify factual claims against current sources before presenting as fact.\n\
     Treat unfamiliar proper nouns, version numbers, and status claims as requiring search verification.\n\
     </grounding>\n"
        .to_string()
}

// ─── Prompt extraction ──────────────────────────────────────

/// Extract prompt text from the hook event JSON.
fn extract_prompt(event: &serde_json::Value) -> String {
    // Claude Code UserPromptSubmit sends prompt in various locations
    event
        .get("prompt")
        .and_then(|v| v.as_str())
        .or_else(|| {
            event
                .get("tool_input")
                .and_then(|ti| ti.get("prompt"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("")
        .to_string()
}

/// The file paths THIS session has touched, for path-triggered domains: every `file_path`
/// the tool hooks logged for `session_id` in this tier's `hook-events.jsonl` (the file
/// `log_hook_event` writes), plus the session's cwd. Nothing from the graph.
///
/// Until 0.14.0 this was a SPARQL over `ops:path` / `ops:lastActive`, which is every path
/// ever active on the store (923 on the operator's, identical on every prompt), so a
/// `Documents` trigger fired on prompt 3 of every session whatever was typed — F29 D1, and
/// D3 with it. Empty apart from the cwd when the session is unknown or the log is absent.
fn gather_active_paths(cwd: &Path, base_dir: Option<&Path>, session_id: Option<&str>) -> Vec<String> {
    let mut paths = vec![cwd.display().to_string()];
    let (Some(base_dir), Some(sid)) = (base_dir, session_id) else {
        return paths;
    };
    let Ok(text) = std::fs::read_to_string(base_dir.join("hook-events.jsonl")) else {
        return paths;
    };
    let mut seen: std::collections::HashSet<String> = paths.iter().cloned().collect();
    for line in text.lines() {
        // The log holds every session on this tier; a substring gate keeps the JSON
        // parse to this session's rows.
        if !line.contains(sid) {
            continue;
        }
        let Ok(row) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if row.get("session_id").and_then(serde_json::Value::as_str) != Some(sid) {
            continue;
        }
        if let Some(fp) = row.get("file_path").and_then(serde_json::Value::as_str)
            && !fp.is_empty()
            && seen.insert(fp.to_string())
        {
            paths.push(fp.to_string());
        }
    }
    paths
}

#[cfg(test)]
mod steering_tests {
    use super::*;
    use crate::domain::matcher::MatchReason;

    // ─── Phase 28: command_activation gating ─────────────────

    #[test]
    fn activation_disabled_never_fires() {
        for r in [MatchReason::Always, MatchReason::Keyword, MatchReason::Filepath, MatchReason::KeywordAndFilepath] {
            assert!(!command_activation_fires("disabled", &r));
        }
    }

    #[test]
    fn activation_keyword_gates_on_keyword_match() {
        assert!(command_activation_fires("keyword", &MatchReason::Keyword));
        assert!(command_activation_fires("keyword", &MatchReason::KeywordAndFilepath));
        assert!(command_activation_fires("keyword", &MatchReason::Always));
        assert!(!command_activation_fires("keyword", &MatchReason::Filepath));
    }

    #[test]
    fn activation_filepath_gates_on_path_match() {
        assert!(command_activation_fires("filepath", &MatchReason::Filepath));
        assert!(command_activation_fires("filepath", &MatchReason::KeywordAndFilepath));
        assert!(!command_activation_fires("filepath", &MatchReason::Keyword));
        assert!(!command_activation_fires("filepath", &MatchReason::Always));
    }

    #[test]
    fn activation_both_and_unknown_fire_on_any_match() {
        for activation in ["both", "", "garbage"] {
            assert!(command_activation_fires(activation, &MatchReason::Keyword));
            assert!(command_activation_fires(activation, &MatchReason::Filepath));
        }
    }

    // ─── Phase 31: output-mode directive ─────────────────────

    #[test]
    fn output_mode_directive_maps_known_modes() {
        assert_eq!(
            output_mode_directive(Some("file")),
            Some("Default output mode: write to file artifacts.")
        );
        assert_eq!(
            output_mode_directive(Some("inline")),
            Some("Default output mode: respond inline in chat.")
        );
        assert_eq!(output_mode_directive(Some("ask")), None);
        assert_eq!(output_mode_directive(None), None);
        assert_eq!(output_mode_directive(Some("nonsense")), None);
    }

    // ─── Phase 30: grounding block ───────────────────────────

    #[test]
    fn grounding_block_has_tags() {
        let b = grounding_block();
        assert!(b.contains("<grounding>"));
        assert!(b.contains("</grounding>"));
        assert!(b.contains("Verify factual claims"));
    }
}
