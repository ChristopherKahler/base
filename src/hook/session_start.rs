use std::path::{Path, PathBuf};

use anyhow::Result;
use oxigraph::sparql::QueryResults;

use crate::config::{load_queries, BaseConfig};
use crate::ontology;
use crate::store;

pub fn handle(config: &BaseConfig, cwd: &Path, session_id: Option<&str>) -> Result<()> {
    // Surface graph corruption at boot — loud, before any other output, so a
    // broken graph announces itself immediately instead of degrading silently.
    warn_unhealthy_graphs(cwd);

    // Proactive graph hygiene (Phase 52): compact any tier graph that has ballooned
    // past the threshold so graphs never balloon on a user's machine. Low-frequency
    // path; backup-first + atomic + cooldown-gated; skips an unhealthy graph.
    for outcome in crate::graph::auto_compact_tiers(&config.graph, cwd) {
        println!("{}", crate::graph::format_auto_compact_notice(&outcome));
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

    // A release that adds a hook wires it here, once per version. The
    // auto-update swaps the binary and touches nothing else, and a hook that
    // is not in settings.json never fires — silently.
    let added = crate::install::ensure_hooks_wired();
    if !added.is_empty() {
        println!(
            "[hooks] wired base hook {} into ~/.claude/settings.json (new in this release; live from the next session).",
            added.join(", ")
        );
    }

    // The installed CLAUDE.md contract refreshes here, once per version, for the
    // same reason: the process that runs `base update` is the outgoing binary and
    // carries the old text, so only the new binary's first session can write its own.
    match crate::install::ensure_claude_md_current() {
        Some(crate::install::ClaudeMdRefresh::Refreshed) => {
            println!("[contract] refreshed the BASE CLI section of ~/.claude/CLAUDE.md to this release.");
        }
        Some(crate::install::ClaudeMdRefresh::Duplicate(n)) => {
            println!("[contract] ~/.claude/CLAUDE.md carries {n} '## BASE CLI' sections; base refreshes none until one remains.");
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
    if let Some(line) = crate::hook::automap::session_start_notice(cwd) {
        println!("{line}");
    }

    // Mechanical reconcile (task-artifact protocol): replace hook-stamped lastActive
    // with the real folder last-touch, then decay cold projects active→deferred (and
    // revive the reverse) BEFORE signals surface, so the rendered state is already
    // true. Fail-open; gated on [protocol] enabled.
    reconcile_active_state(config, cwd);

    // Emit operator profile (if configured)
    if let Some(profile) = crate::operator::load() {
        println!("{}", crate::operator::format_block(&profile));
    }

    // Silent self-update, then the legacy check/banner for pinned installs.
    auto_update(config);
    check_and_banner();

    // Try signals first (Phase 5) — primary injection source
    let mut diagnostics: Vec<String> = Vec::new();

    if let Ok(signal_result) = crate::signal::run_signals(cwd, config, "session-start") {
        diagnostics.extend(signal_result.diagnostics);

        if !signal_result.content.is_empty() {
            print!("{}", signal_result.content);

            // Flow protocol injection (static behavioral rules) — after signals
            if config.flow.enabled && config.flow.protocol {
                print!("\n{}", crate::hook::flow::protocol_block());
            }

            // Diagnostics: always emitted, bypass suppression
            if !diagnostics.is_empty() {
                print!("\n{}", diagnostics.join("\n"));
            }

            // Extension status injection (Phase 23)
            inject_extension_status(config, cwd);

            // Context triggers cheat-sheet (Phase 21)
            let triggers = crate::domain::query::context_triggers_block(cwd);
            if !triggers.is_empty() {
                print!("\n{triggers}");
            }

            return Ok(());
        }
    }

    // Fallback: ad-hoc queries from queries.toml (Phase 1 behavior)
    let trig_files = discover_trig_files(cwd);

    if trig_files.is_empty() {
        // Emit diagnostics even when no graph files found
        if !diagnostics.is_empty() {
            print!("{}", diagnostics.join("\n"));
        }
        return Ok(());
    }

    let paths: Vec<&Path> = trig_files.iter().map(|p| p.as_path()).collect();
    let graph = store::load_graphs(&paths)?;

    ontology::load_vocabulary(&graph, &config.namespace)?;

    let queries = load_queries(cwd, config);
    let mut output = String::new();

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
            }
        }
    }

    if !output.is_empty() {
        print!("{}", output.trim_end());
    }

    // Flow protocol injection — also in fallback path
    if config.flow.enabled && config.flow.protocol {
        if !output.is_empty() {
            println!();
        }
        print!("{}", crate::hook::flow::protocol_block());
    }

    // Diagnostics: always emitted at end of output
    if !diagnostics.is_empty() {
        if !output.is_empty() || (config.flow.enabled && config.flow.protocol) {
            println!();
        }
        print!("{}", diagnostics.join("\n"));
    }

    // Extension status injection (Phase 23)
    inject_extension_status(config, cwd);

    Ok(())
}

/// Inject extension status lines and run extension session-start SPARQL queries.
/// Fail-open: malformed extensions, missing query files, and query errors all skip silently.
fn inject_extension_status(config: &BaseConfig, cwd: &Path) {
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
                println!("{inject}");
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
                                println!(
                                    "<ext:{}-query>\n{} result(s) from {}\n</ext:{}-query>",
                                    ext.name,
                                    rows.len(),
                                    query_rel_path,
                                    ext.name
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

fn check_and_banner() {
    let Some(mut manifest) = crate::manifest::Manifest::load() else {
        return; // No manifest = nothing to check
    };

    // A hand-swapped binary leaves the recorded version stale, and every decision
    // below is made against it. Correct it before reading anything else.
    if crate::manifest::reconcile_running_version(&mut manifest) {
        let _ = manifest.save();
    }

    let activated = manifest.is_activated();
    let pending = &manifest.update_check.pending_update;

    // If updates already known...
    if !pending.is_empty() {
        if !activated && !crate::manifest::is_snoozed(&manifest) {
            // Inject banner for non-activated, non-snoozed installs
            print!("{}", crate::manifest::format_update_banner(pending));
        }
        // Don't also run HTTP check — we already know about updates.
        // Still fall through for activated installs to keep manifest current.
        if !activated {
            return;
        }
    }

    // Version check (weekly, HTTP call)
    if !crate::manifest::should_check(&manifest) {
        return;
    }

    // Run the check — 3s timeout per endpoint, fail silently on any error
    let result = crate::manifest::check_for_updates(&mut manifest);

    // Save manifest regardless (updates last_checked)
    let _ = manifest.save();

    // If updates found and not activated, show banner
    if let Ok(Some(ref pending)) = result
        && !activated {
            print!("{}", crate::manifest::format_update_banner(pending));
        }
}

/// Mechanical active⇄deferred reconcile (task-artifact protocol). Fail-open: any
/// error leaves graph state as-is and never blocks session start. Silent unless a
/// status actually flipped (suppression principle — lastActive refreshes are noiseless).
fn reconcile_active_state(config: &BaseConfig, cwd: &Path) {
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
fn warn_unhealthy_graphs(cwd: &Path) {
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
            println!("═══════════════════════════════════════");
            println!("⚠️  BASE GRAPH UNHEALTHY — {tier} tier");
            println!("   {}", path.display());
            println!("   {reason}{line}");
            println!("   recall / learn / sync are DEGRADED until repaired.");
            println!("   Repair: run `base doctor` once available (v0.5),");
            println!("           or repair manually per GRAPH-DURABILITY.md");
            println!("═══════════════════════════════════════");
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
}
