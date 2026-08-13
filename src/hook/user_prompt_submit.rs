use std::path::Path;

use anyhow::Result;
use oxigraph::model::TermRef;

use crate::config::BaseConfig;
use crate::domain;
use crate::domain::matcher::match_domains;
use crate::domain::query::{query_domain_from_graph, resolve_and_run_query, format_toml_rules};
use crate::domain::session::{rules_hash, Bracket, SessionState};

pub fn handle(config: &BaseConfig, cwd: &Path, event: &serde_json::Value) -> Result<super::HookEventData> {
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
            dirs::home_dir().map(|h| h.join(".base-gbl").join(".base")).filter(|p| p.is_dir())
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

    // Force-refresh dedup in DEPLETED/CRITICAL on interval
    if session.should_force_refresh_for(&config.bracket, session_id, context_pct) {
        session.clear_dedup();
    }

    // Bracket rules — tier-gated, never deduped. Re-injecting every prompt IS the
    // feature: these are the rules that must not erode as context fills, which a
    // once-per-session domain injection cannot guarantee. Built before the *command
    // branch so a star command cannot bypass them.
    let bracket_rules = crate::domain::session::format_bracket_rules(bracket, &config.bracket.rules);

    // Deferred from above: nothing else to do without domains, but the bracket
    // block still goes out.
    if domains.is_empty() {
        if let Some(ref base_dir) = base_dir {
            let _ = session.save(base_dir);
        }
        print!("{bracket_rules}");
        return Ok(super::HookEventData {
            prompt_num: Some(session.prompt_count_for(session_id)),
            ..Default::default()
        });
    }

    // Check for *COMMAND(s) before domain matching — supports stacking, so
    // "*audit *steelman" activates BOTH modes (every matched *word injects).
    let commands = crate::command::load_commands(cwd);
    let matched = crate::command::match_commands(&prompt, &commands);
    if !matched.is_empty() {
        let cmd_output: String = matched
            .iter()
            .map(|cmd| crate::command::format_command_output(cmd))
            .collect::<Vec<_>>()
            .join("\n");
        if !cmd_output.is_empty() {
            // Star commands bypass domain matching — they're explicit invocations
            if let Some(ref base_dir) = base_dir {
                let _ = session.save(base_dir);
            }
            // Bracket rules ride along with star commands too — a mode changes
            // stance, it does not suspend the always-on layer.
            print!("{bracket_rules}{cmd_output}");
            return Ok(super::HookEventData {
                prompt_num: Some(session.prompt_count_for(session_id)),
                ..Default::default()
            });
        }
    }

    // Ensure domain sync has run BEFORE loading the graph, so the single
    // load below sees freshly synced rules. Marker-gated — no-op when fresh.
    ensure_domain_sync(config, cwd);

    // Single graph load per invocation (merged: global + workspace).
    // gather_active_paths and the injection loop all share this store.
    let graph_store = crate::store::load_merged(cwd);

    // Gather active file paths from graph (if available)
    let active_paths = gather_active_paths(config, &graph_store);

    let matched = match_domains(&prompt, &domains, &active_paths);
    if matched.is_empty() {
        // Still save session state (prompt_count) even if nothing matched
        if let Some(ref base_dir) = base_dir {
            let _ = session.save(base_dir);
        }
        return Ok(super::HookEventData {
            prompt_num: Some(session.prompt_count),
            ..Default::default()
        });
    }

    // This session's depth — not the workspace-wide total, which concurrent
    // sessions inflate.
    let prompt_num = session.prompt_count_for(session_id);

    // Emit context bracket tag, then the tier's rules
    let mut output = format!(
        "<context-bracket>[{bracket}] (prompt {prompt_num})</context-bracket>\n\n"
    );
    output.push_str(&bracket_rules);

    // Determine if we're in lean mode (FRESH, first 2 prompts — rules only, skip neighborhood)
    let lean_mode = bracket == Bracket::Fresh && prompt_num <= 2;

    // Track injection metadata for DEVMODE
    let mut loaded_domains: Vec<(String, String, usize)> = Vec::new(); // (name, match_reason, rule_count)
    let mut deduped_count = 0usize;
    // Steering layer (v0.4): dedup domain-linked command injection across domains,
    // and remember whether any fresh content was injected (gates the grounding block).
    let mut injected_commands: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut injected_any = false;

    // Format and emit matched rules
    for dm in &matched {
        let domain_def = dm.domain;

        // Try graph-backed injection first, fall back to TOML rules
        let (rules_text, neighborhood_text) = match &graph_store {
            Some(store) => {
                let (r, n) = query_domain_from_graph(store, config, domain_def);
                if lean_mode {
                    (r, String::new()) // skip neighborhood in lean mode
                } else {
                    (r, n)
                }
            }
            None => (format_toml_rules(domain_def), String::new()),
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

        // Assemble in steering order:
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

        // Dedup: hash combined output (rules + neighborhood), skip if unchanged.
        // Hash over SORTED lines — SPARQL result order shifts when the graph
        // file is rewritten (post-tool-use fires on every edit), and an
        // order-sensitive hash would re-inject unchanged content every prompt.
        let combined_hash = {
            let mut lines: Vec<String> = domain_output.lines().map(String::from).collect();
            lines.sort();
            rules_hash(&lines)
        };
        // Count actual injected rules (from graph, not TOML)
        let injected_rule_count = rules_text.lines().filter(|l| l.starts_with("  ")).count();

        if session.is_injected(&domain_def.name, combined_hash) {
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
            format!("{}", dm.reason)
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

        output.push_str(&domain_output);
        output.push('\n');
        injected_any = true;

        // Mark as injected in session state
        session.mark_injected(&domain_def.name, combined_hash);
    }

    // Grounding (Phase 30): when enabled, ride a source-verification block on any
    // fresh injection this prompt. Skipped on dedup-only prompts (already grounded).
    if config.grounding.enabled && injected_any {
        output.push_str(&grounding_block());
    }

    // DEVMODE block (Task 2 will populate this fully)
    if config.devmode.enabled {
        output.push_str(&format_devmode_block(
            &loaded_domains,
            &domains,
            bracket,
            session.prompt_count,
            deduped_count,
        ));
    }

    // Save updated session state
    if let Some(ref base_dir) = base_dir {
        let _ = session.save(base_dir);
    }

    if !output.is_empty() {
        print!("{}", output.trim_end());
    }

    // Build event data for JSONL logging
    let domains_matched: Vec<String> = loaded_domains
        .iter()
        .filter(|(_, reason, _)| !reason.starts_with("dedup"))
        .map(|(name, _, _)| name.clone())
        .collect();
    let total_rules: usize = loaded_domains
        .iter()
        .filter(|(_, reason, _)| !reason.starts_with("dedup"))
        .map(|(_, _, count)| count)
        .sum();

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

    Ok(super::HookEventData {
        domains_matched,
        rules_injected: total_rules,
        suppressed: deduped_count,
        prompt_num: Some(session.prompt_count),
        prompt_text: prompt_preview,
        tool_name: None,
        file_path: None,
        session_id: None, // populated by run() after handle returns
        ..Default::default()
    })
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
    if let Some(home) = dirs::home_dir() {
        let global_dir = home.join(".base-gbl");
        let global_base = global_dir.join(".base");
        if global_base.is_dir() {
            let marker = global_base.join(".domain-sync-ts");
            let domains_toml = global_dir.join("domains.toml");
            if domains_toml.exists() {
                let needs_sync = needs_sync_check(&domains_toml, &marker);
                if needs_sync
                    && domain::sync::sync_domains_to_graph(config, &global_dir, None).is_ok() {
                        let _ = std::fs::write(&marker, "");
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
            let _ = std::fs::write(&marker, "");
        }
}

/// Check if a domains.toml is newer than its sync marker.
fn needs_sync_check(domains_toml: &Path, marker: &Path) -> bool {
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

/// Gather recently-active file paths from the merged graph (for path-based domain matching).
/// Returns empty vec if no graph available — graceful degradation.
fn gather_active_paths(config: &BaseConfig, graph: &Option<oxigraph::store::Store>) -> Vec<String> {
    let graph = match graph {
        Some(g) => g,
        None => return Vec::new(),
    };

    let sparql = format!(
        "PREFIX {p}: <{u}>\n\
         SELECT ?path WHERE {{\n\
           GRAPH ?g {{\n\
             ?entity {p}:path ?path .\n\
             ?entity {p}:lastActive ?ts .\n\
           }}\n\
         }}",
        p = config.namespace.prefix,
        u = config.namespace.uri,
    );

    match crate::store::query(graph, &sparql) {
        Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) => solutions
            .filter_map(|r| r.ok())
            .filter_map(|row| {
                row.get("path")
                    .map(|t| match t.into() {
                        TermRef::Literal(l) => l.value().to_string(),
                        _ => String::new(),
                    })
                    .filter(|s| !s.is_empty())
            })
            .collect(),
        _ => Vec::new(),
    }
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
