use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::BaseConfig;
use crate::domain;
use crate::domain::matcher::{match_domains_auto, MatchReason, TriggerContext};
use crate::domain::query::resolve_and_run_query;
use crate::domain::session::{rules_hash, Bracket, ReShow, SessionState};
use crate::emit::prompt::{Claim, Fitted, Priority, PromptBlock, PromptBlocks};

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
}

/// The session state `handle` changed, held until the output is fitted.
struct Pending {
    session: SessionState,
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
    fn hold(&mut self, session: SessionState, base_dir: Option<PathBuf>, tier: Bracket) {
        if let Some(base_dir) = base_dir {
            self.pending = Some(Pending { session, base_dir, tier });
        }
    }

    /// Record in the session what `fitted` printed, and save it (D15). A dropped block's rules stay due. A
    /// domain's block hash is recorded only when every block carrying it was printed, so a domain whose rules
    /// arrived and whose context was dropped serves its context again.
    pub fn commit(&mut self, fitted: &Fitted) -> Committed {
        let mut done = Committed::default();
        let vetoed: HashSet<(&str, u64)> = fitted
            .dropped_blocks()
            .flat_map(|b| b.claims.iter())
            .filter_map(|c| match c {
                Claim::Injected { key, hash } => Some((key.as_str(), *hash)),
                _ => None,
            })
            .collect();
        let mut pending = self.pending.take();
        let now = SessionState::now_secs();
        for claim in fitted.kept_blocks().flat_map(|b| b.claims.iter()) {
            match claim {
                Claim::Rule { id, content, scope } => {
                    done.rules += 1;
                    if let Some(p) = pending.as_mut() {
                        p.session.mark_rule_shown(id, *content, p.tier, scope.as_deref(), now);
                    }
                }
                Claim::Injected { key, hash } => {
                    if let Some(p) = pending.as_mut()
                        && !vetoed.contains(&(key.as_str(), *hash))
                    {
                        p.session.mark_injected(key, *hash);
                    }
                }
                Claim::BracketBlock => {
                    done.bracket_block = true;
                    if let Some(p) = pending.as_mut() {
                        p.session.mark_bracket_block(p.tier);
                    }
                }
            }
        }
        if let Some(p) = pending {
            let _ = p.session.save(&p.base_dir);
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

    // Bracket rules — tier-gated, and served ONCE per tier (F7, ruled by Chris as K1
    // on 2026-09-12: "no, inject one time, then no more, inject only when bracket
    // changes the rules for that bracket"). Built before the *command branch so a
    // star command cannot bypass them.
    //
    // This block is pushed at all four return sites below, so a new return site
    // added later cannot forget the rule. It is priority 5, last (F2): CLAUDE.md
    // carries much the same text, and on 2026-10-01 it took 2.4 KB of a 3.9 KB
    // prompt ahead of everything matched to the prompt.
    //
    // ASKED, NOT CLAIMED (D15). The tier is recorded as served by `commit`, only
    // if this block was printed, so a block the budget dropped goes on the next
    // prompt. An empty render records nothing either: base ships no bracket rules,
    // and claiming a tier for a block that was never printed would silence the
    // first real one after an operator configures some.
    let bracket_text = crate::domain::session::format_bracket_rules(bracket, &config.bracket.rules);
    let bracket_block = (!bracket_text.is_empty() && session.bracket_block_due(bracket)).then(|| {
        PromptBlock::new(
            "bracket-rules",
            Priority::Bracket,
            &bracket_text,
            bracket.rules(&config.bracket.rules).len(),
            "rule",
        )
        .with_claims([Claim::BracketBlock])
    });

    // Deferred from above: no domains to match, but the bracket block still goes
    // out, and so do rules that carry matchers of their own: a rule added with
    // `base rule add --kind` needs no domains.toml entry to fire (4d).
    if domains.is_empty() {
        let store = crate::store::load_merged(cwd);
        let converted = crate::domain::rules::rules_with_matchers(store.as_ref(), config, &domains);
        let prompt_num = session.prompt_count_for(session_id);
        sink.blocks.extend(matcher_blocks(config, &prompt, &session, bracket, &converted, &domains));
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
            })
            .filter(|b| !b.text.is_empty())
            .collect();
        if !cmd_blocks.is_empty() {
            // Star commands bypass domain matching — they're explicit invocations.
            // Bracket rules ride along with star commands too — a mode changes
            // stance, it does not suspend the always-on layer.
            let prompt_num = session.prompt_count_for(session_id);
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
    if matched.is_empty() {
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
        sink.header = format!("<context-bracket>[{bracket}] (prompt {nomatch_prompt_num})</context-bracket>");
        sink.blocks.extend(matcher_blocks(config, &prompt, &session, bracket, &converted, &domains));
        if let Some(ref store) = graph_store {
            let nothing_served = std::collections::HashSet::new();
            let walked =
                crate::hook::walk::walk_from_text(store, cwd, config, &prompt, &nothing_served);
            // Dedup and render as one unit; the marks wait for the print -- see `render_walk_block`.
            let w = render_walk_block(&session, walked, config.injection.walk_budget);
            sink.blocks.extend(w.blocks);
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
    sink.header = format!("<context-bracket>[{bracket}] (prompt {prompt_num})</context-bracket>");
    let matcher = matcher_blocks(config, &prompt, &session, bracket, &converted, &domains);
    let matcher_served = matcher.iter().any(|b| !b.claims.is_empty());
    sink.blocks.extend(matcher);

    // Determine if we're in lean mode (FRESH, first 2 prompts — rules only, skip neighborhood)
    let lean_mode = bracket == Bracket::Fresh && prompt_num <= 2;

    // Track injection metadata for DEVMODE
    let mut loaded_domains: Vec<(String, String, usize)> = Vec::new(); // (name, match_reason, rule_count)
    let mut deduped_count = 0usize;
    // Steering layer (v0.4): dedup domain-linked command injection across domains,
    // and remember whether any fresh content was injected (gates the grounding block).
    let mut injected_commands: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut injected_any = matcher_served;
    // Every record IRI the domain blocks serve this prompt. The walk below dedups
    // against it, so a record cannot arrive twice under two headings.
    let mut domain_served: std::collections::HashSet<String> = std::collections::HashSet::new();
    let now = SessionState::now_secs();

    // Format and emit matched rules
    for dm in &matched {
        let domain_def = dm.domain;

        // The rules and the neighbourhood are read separately now, because they are
        // deduped differently: the rules per RULE (F9), the neighbourhood as a block.
        // A rule with matchers of its own left this block in 4d: it was served on them above (F1).
        let rules: Vec<crate::domain::rules::ServedRule> =
            crate::domain::rules::rules_for_domain(graph_store.as_ref(), config, domain_def)
                .into_iter()
                .filter(|r| !converted_ids.contains(r.id.as_str()))
                .collect();
        // Due, not claimed (D15): `commit` records each rule only if this domain's rules block is printed.
        let fresh: Vec<(usize, &crate::domain::rules::ServedRule)> = rules
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                session.rule_due(&r.id, r.content_hash, bracket, None, ReShow::PerSession { on_tier_change: true }, now)
            })
            .collect();
        // What this prompt serves, so the walk still resolves a rule this block held
        // back and does not list again one it serves.
        domain_served.extend(fresh.iter().filter_map(|(_, r)| r.iri.clone()));
        let rules_text =
            crate::domain::rules::render_block("DOMAIN", &fresh, rules.len(), &domain_def.name);

        let neighborhood_text = match (&graph_store, lean_mode) {
            (Some(store), false) => {
                let (n, served) =
                    crate::domain::query::query_domain_neighborhood(store, config, domain_def);
                domain_served.extend(served);
                n
            }
            _ => String::new(),
        };

        // ─── Steering layer (v0.4): role / linked commands / output-mode / format ───
        // Role (Phase 29): first line of the domain block.
        let role_line = domain_def.role.as_deref().map(str::trim).filter(|r| !r.is_empty());

        // Domain-linked command rules (Phase 28): inject each linked mode's rules
        // once. Explicit *commands short-circuit before domain matching, so this
        // path only fires when no explicit star was typed — no cross-dedup needed.
        let mut command_block = String::new();
        if command_activation_fires(&domain_def.command_activation, &dm.reason) {
            for cmd_name in &domain_def.commands {
                let key = cmd_name.to_lowercase();
                if injected_commands.contains(&key) {
                    continue;
                }
                if let Some(cmd) = commands.iter().find(|c| c.name.eq_ignore_ascii_case(cmd_name)) {
                    let rendered = crate::command::format_command_output(cmd);
                    if !rendered.is_empty() {
                        if !command_block.is_empty() {
                            command_block.push('\n');
                        }
                        command_block.push_str(&rendered);
                        injected_commands.insert(key);
                    }
                }
            }
        }

        // Output mode (Phase 31) + format directive (Phase 32).
        let output_mode_line = output_mode_directive(domain_def.output_mode.as_deref());
        let format_line = domain_def.format.as_deref().map(str::trim).filter(|f| !f.is_empty());

        // Skip only when the domain contributes nothing — rules, neighborhood, a
        // query, or any steering directive all count as content.
        if rules_text.is_empty()
            && neighborhood_text.is_empty()
            && domain_def.query.is_none()
            && role_line.is_none()
            && command_block.is_empty()
            && output_mode_line.is_none()
            && format_line.is_none()
        {
            // A domain that HAS rules and had every one of them already served this
            // session is not "contributes nothing". It is a dedup, and it has to be
            // COUNTED as one.
            //
            // Since F9 dedups one rule at a time, a fully served domain arrives here
            // with an empty `rules_text`, and a domain carrying nothing else used to
            // `continue` before the dedup branch below ever ran. The injection was
            // right and the report was not: the domain dropped out of
            // `HookEventData::suppressed`, which feeds the JSONL log, and out of the
            // devmode dedup list. A domain that silently vanishes from the count reads
            // as a domain that never matched — a false clean bill in the telemetry.
            // `graph_injection_test::dedup_skips_unchanged_graph_injection` caught it
            // on the first full-suite run after F9 landed.
            if !rules.is_empty() {
                deduped_count += 1;
                let dedup_reason = if config.devmode.enabled {
                    format!("dedup [{}]", dm.reason)
                } else {
                    "dedup".into()
                };
                loaded_domains.push((domain_def.name.clone(), dedup_reason, 0));
            }
            continue;
        }

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

        // The steering order the hash has always been computed over:
        // role → command rules → rules → neighborhood → query → output-mode → format.
        let mut sections: Vec<&str> = Vec::new();
        if let Some(r) = role_line {
            sections.push(r);
        }
        if !command_block.is_empty() {
            sections.push(&command_block);
        }
        if !rules_text.is_empty() {
            sections.push(&rules_text);
        }
        if !neighborhood_text.is_empty() {
            sections.push(&neighborhood_text);
        }
        if !query_text.is_empty() {
            sections.push(&query_text);
        }
        if let Some(om) = output_mode_line {
            sections.push(om);
        }
        if let Some(f) = format_line {
            sections.push(f);
        }
        let domain_output = sections.join("\n");

        // The rules have already been deduped one at a time above. What is hashed
        // here is everything ELSE the block carries — the neighbourhood, the query,
        // the steering lines — which is still a block and still deduped as one.
        //
        // Hash over SORTED lines: SPARQL result order shifts when the graph file is
        // rewritten (post-tool-use fires on every edit), and an order-sensitive hash
        // would re-inject unchanged content every prompt.
        let combined_hash = {
            let mut lines: Vec<String> = domain_output
                .lines()
                .filter(|l| !rules_text.contains(*l))
                .map(String::from)
                .collect();
            lines.sort();
            rules_hash(&lines)
        };
        let injected_rule_count = fresh.len();

        // A fresh rule is served even when nothing else about the block changed. The
        // block hash can only suppress the block when it carries no new rule.
        if rules_text.is_empty() && session.is_injected(&domain_def.name, combined_hash) {
            deduped_count += 1;
            let dedup_reason = if config.devmode.enabled {
                format!("dedup [{}]", dm.reason)
            } else {
                "dedup".into()
            };
            loaded_domains.push((
                domain_def.name.clone(),
                dedup_reason,
                injected_rule_count,
            ));
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

        // TWO BLOCKS, RANKED APART (F2): the rules with their steering lines, and the CONTEXT with the query. A
        // domain matched to the prompt puts its rules at 1 and its context at 2; an always-on domain puts both at 4.
        // The block hash is recorded only if every block carrying it is printed (see `PromptSink::commit`).
        let (rules_priority, context_priority) = domain_priorities(&dm.reason);
        let slug = crate::crud::slugify(&domain_def.name);
        let rules_part: Vec<&str> = [role_line, Some(command_block.as_str()), Some(rules_text.as_str()), output_mode_line, format_line]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .collect();
        let context_part: Vec<&str> =
            [neighborhood_text.as_str(), query_text.as_str()].into_iter().filter(|s| !s.is_empty()).collect();
        let domain_claim = Claim::Injected { key: domain_def.name.clone(), hash: combined_hash };
        sink.blocks.push(
            PromptBlock::new(format!("{slug}-rules"), rules_priority, &rules_part.join("\n"), fresh.len(), "rule")
                .with_claims(fresh.iter().map(|(_, r)| Claim::Rule {
                    id: r.id.clone(),
                    content: r.content_hash,
                    scope: None,
                }))
                .with_claims([domain_claim.clone()]),
        );
        let context_text = context_part.join("\n");
        sink.blocks.push(
            PromptBlock::new(format!("{slug}-context"), context_priority, &context_text, count_records(&context_text), "record")
                .with_claims([domain_claim]),
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
        crate::hook::walk::walk_from_text(store, cwd, config, &prompt, &domain_served)
    });

    // The walk's blocks rank at 2 with the domains' context, after it, so a reader
    // sees the configured layer first and the named-thing layer as the specific addition.
    let mut walk_note = String::new();
    if let Some(walked) = walked {
        // Dedup and render as one unit; the marks wait for the print -- see
        // `render_walk_block`. The no-match return above is the other caller.
        let w = render_walk_block(&session, walked, config.injection.walk_budget);
        if config.devmode.enabled && w.deduped > 0 {
            walk_note.push_str(&format!("  walk: {} name(s) already injected this session\n", w.deduped));
        }
        if !w.blocks.is_empty() {
            injected_any = true;
        }
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

/// Rules that carry matchers of their own, for one prompt (4d): always rules on the session's first prompt and on
/// each tier change, and topic rules ranked against the prompt and capped at `topic_max`, with F6's pointer line
/// for what the cap withheld (F6, F7, F8).
///
/// One block per header (F2): topic rules are matched to the prompt (1), `always` rules are global (4). Nothing is
/// recorded here; each block carries its rules as claims, recorded only if the block is printed (D15).
///
/// Three callers in `handle`, one per return site that is not a star command: no domains at all, no domain matched,
/// and the main path. A star command is an explicit invocation and passes rules by, as it passes domains by.
fn matcher_blocks(
    config: &BaseConfig,
    prompt: &str,
    session: &SessionState,
    bracket: Bracket,
    converted: &[crate::domain::rules::Converted],
    domains: &[domain::DomainDef],
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
    };
    let event = crate::domain::rules::Event::Prompt { text: prompt };
    let selection = crate::domain::rules::select_unrecorded(converted, &event, session, &cx);
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
        blocks.push(PromptBlock::new(id, priority, &text, g.served.len(), "rule").with_claims(claims));
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
    let (rendered, dropped) = crate::hook::walk::render(&served, budget);
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
        blocks.push(
            PromptBlock::new(format!("walk-{}", crate::crud::slugify(name)), Priority::Context, piece, records, "record")
                .with_claims(claims),
        );
    }
    WalkBlock { blocks, deduped, dropped, served }
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
