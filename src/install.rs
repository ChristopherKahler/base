use std::path::Path;

use anyhow::{Context, Result};

use crate::config::BaseConfig;
use crate::manifest::{self, Manifest};

/// The generic star-command pack offered at install. A fresh install otherwise
/// ships zero commands, which leaves a new user with the machinery and no idea
/// what to type first.
const STARTER_COMMANDS: &str = include_str!("starter-commands.toml");

/// What to do about the starter star commands: ask (interactive default),
/// or a decision already made by flag for unattended installs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StarterCommands {
    Ask,
    Yes,
    No,
}

/// Run the full install process: build, symlink, create global tier, wire hooks, write manifest.
pub fn run(
    carl_json_path: Option<&Path>,
    skip_hooks: bool,
    full: bool,
    starter: StarterCommands,
) -> Result<()> {
    let home = crate::home::home_root().context("Cannot determine home directory")?;
    let binary_path = std::env::current_exe().context("Cannot determine binary path")?;

    println!("═══════════════════════════════════════");
    println!("BASE v2 — Global Install");
    println!("═══════════════════════════════════════\n");

    // Step 1: Copy binary to ~/.local/bin/base
    let local_bin = home.join(".local").join("bin");
    let dest_path = local_bin.join("base");
    install_binary(&binary_path, &dest_path, &local_bin)?;

    // Step 2: Create ~/.base-gbl/ with defaults
    let global_dir = home.join(".base-gbl");
    create_global_tier(&global_dir)?;

    // Step 3: Wire hooks in ~/.claude/settings.json
    if !skip_hooks {
        let settings_path = home.join(".claude").join("settings.json");
        wire_hooks(&settings_path)?;
    } else {
        println!("⊘ Hook wiring skipped (--skip-hooks)\n");
    }

    // Step 4: Migrate carl.json decisions if provided
    if let Some(carl_path) = carl_json_path {
        migrate_carl(&global_dir, carl_path)?;
    }

    // Step 5: Install AST extraction scripts
    install_scripts(&binary_path, &global_dir)?;

    // Step 6: Seed system rules
    seed_system_rules(&global_dir)?;

    // Step 6: Install bundled Claude skills (local checkout, else fetch)
    install_skills(
        &binary_path,
        &home,
        env!("CARGO_PKG_VERSION"),
        SkillSource::LocalThenTag,
        SkillReport::InstallStep,
    )?;

    // Step 6b: Offer the starter star commands
    install_starter_commands(&global_dir, starter)?;

    // Step 7: Append BASE CLI section to ~/.claude/CLAUDE.md
    let claude_md = home.join(".claude").join("CLAUDE.md");
    append_claude_md(&claude_md)?;

    // Step 8: Write manifest.toml
    write_manifest(&global_dir, full)?;

    println!("═══════════════════════════════════════");
    println!("✓ Install complete");
    println!("═══════════════════════════════════════\n");
    println!("Next steps:");
    println!("  1. Open a new Claude Code session");
    println!("  2. Type a prompt that matches a domain keyword");
    println!("  3. Verify rules inject from the graph\n");
    if carl_json_path.is_none() {
        println!("Optional: migrate CARL decisions:");
        println!("  base install --carl ~/.carl/carl.json\n");
    }
    println!("───────────────────────────────────────");
    println!("ChrisAI — Built by Chris Kahler");
    println!("Chris AI Systems");
    println!();
    println!("Community & support:");
    println!("  https://www.skool.com/claude-code-titans-9203");
    println!();
    println!("Tutorials:");
    println!("  https://www.youtube.com/@chris-ai-systems");
    println!("───────────────────────────────────────");

    Ok(())
}

// ─── Starter star commands ──────────────────────────────────

/// Write the generic star-command pack to the global tier, if wanted.
///
/// An existing `commands.toml` is NEVER touched: the user's own commands are
/// the one thing here that cannot be regenerated. In that case this reports
/// and returns.
fn install_starter_commands(global_dir: &Path, choice: StarterCommands) -> Result<()> {
    let path = global_dir.join("commands.toml");

    if path.exists() {
        println!("· Star commands: {} already exists — left untouched\n", path.display());
        return Ok(());
    }

    let wanted = match choice {
        StarterCommands::Yes => true,
        StarterCommands::No => false,
        // A non-interactive install (CI, piped installer) can't answer, and
        // silently writing config nobody asked for is worse than shipping none.
        StarterCommands::Ask if !std::io::IsTerminal::is_terminal(&std::io::stdin()) => false,
        StarterCommands::Ask => prompt_starter_commands(),
    };

    if !wanted {
        println!("⊘ Starter star commands skipped.");
        println!("  Add them later:  base install --starter-commands\n");
        return Ok(());
    }

    std::fs::write(&path, STARTER_COMMANDS)
        .with_context(|| format!("writing {}", path.display()))?;
    println!("✓ Starter star commands → {}", path.display());
    println!("  *handoff  *fork  *base  *end   (see them: base commands list)\n");
    Ok(())
}

/// Ask once. Anything that isn't an explicit yes is a no.
fn prompt_starter_commands() -> bool {
    use std::io::Write as _;
    println!("Star commands are typed straight into a Claude Code chat (`*handoff`) to");
    println!("switch its behavior for that turn. A fresh install ships none.");
    println!();
    println!("  *handoff  end a session so the next one resumes where you left off");
    println!("  *fork     park side-work without derailing what you're doing");
    println!("  *base     sweep this session's decisions and learnings into the graph");
    println!("  *end      all three at once, to close out cleanly");
    println!();
    println!("They are plain TOML in ~/.base-gbl/commands.toml — edit or delete freely.");
    print!("Install the starter pack? [Y/n] ");
    let _ = std::io::stdout().flush();

    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    let a = answer.trim().to_lowercase();
    a.is_empty() || a == "y" || a == "yes"
}

// ─── Uninstall ──────────────────────────────────────────────

/// Remove base hooks from settings.json, remove binary, strip CLAUDE.md section.
/// With --purge, also removes ~/.base-gbl/ global tier.
pub fn uninstall(purge: bool) -> Result<()> {
    let home = crate::home::home_root().context("Cannot determine home directory")?;

    println!("═══════════════════════════════════════");
    println!("BASE v2 — Uninstall");
    println!("═══════════════════════════════════════\n");

    // 1. Remove hooks from settings.json
    let settings_path = home.join(".claude").join("settings.json");
    remove_hooks(&settings_path)?;

    // 2. Remove BASE CLI section from CLAUDE.md
    let claude_md = home.join(".claude").join("CLAUDE.md");
    remove_claude_md_section(&claude_md)?;

    // 3. Remove binary (try both base and base.exe for Windows compatibility)
    let binary = home.join(".local").join("bin").join("base");
    let binary_exe = home.join(".local").join("bin").join("base.exe");
    let mut removed_any = false;
    for bin in [&binary, &binary_exe] {
        if bin.exists() {
            if !removed_any {
                print!("3. Remove binary ... ");
            }
            std::fs::remove_file(bin)?;
            println!("✓ removed {}", bin.display());
            removed_any = true;
        }
    }
    if !removed_any {
        println!("3. Binary not found at {} — skipped", binary.display());
    }

    // 4. Remove bundled skills. Backups (<skill>.bak-*) are the operator's own
    //    customised copies, so they survive uninstall deliberately.
    let skills_root = home.join(".claude").join("skills");
    let mut removed_skills = Vec::new();
    for skill in BUNDLED_SKILLS {
        let dir = skills_root.join(skill);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
            removed_skills.push(*skill);
        }
    }
    if removed_skills.is_empty() {
        println!("4. No bundled skills installed — skipped");
    } else {
        println!("4. Remove bundled skills ... ✓ {}", removed_skills.join(", "));
    }

    // 5. Purge global tier if requested
    if purge {
        let global_dir = home.join(".base-gbl");
        if global_dir.exists() {
            print!("5. Purge global tier ... ");
            std::fs::remove_dir_all(&global_dir)?;
            println!("✓ removed {}", global_dir.display());
        }
    } else {
        println!("5. Global tier preserved (~/.base-gbl/) — use --purge to remove");
    }

    println!("\n═══════════════════════════════════════");
    println!("✓ Uninstall complete");
    println!("═══════════════════════════════════════\n");
    println!("Workspace .base/ directories are untouched.");
    println!("Remove them manually if needed: rm -rf <workspace>/.base/");

    Ok(())
}

fn remove_hooks(settings_path: &Path) -> Result<()> {
    print!("1. Remove hooks from settings.json ... ");

    if !settings_path.exists() {
        println!("not found — skipped");
        return Ok(());
    }

    let content = std::fs::read_to_string(settings_path)?;
    if !content.contains("base hook") {
        println!("no base hooks found — skipped");
        return Ok(());
    }

    let mut settings: serde_json::Value = serde_json::from_str(&content)?;

    if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        for (_event, entries) in hooks.iter_mut() {
            if let Some(arr) = entries.as_array_mut() {
                arr.retain(|entry| {
                    // Remove any entry whose hooks array contains a "base hook" command
                    if let Some(hook_list) = entry.get("hooks").and_then(|h| h.as_array()) {
                        !hook_list.iter().any(|h| {
                            h.get("command")
                                .and_then(|c| c.as_str())
                                .map(|c| c.contains("base hook"))
                                .unwrap_or(false)
                        })
                    } else {
                        true
                    }
                });
            }
        }
    }

    let tmp = settings_path.with_extension("json.tmp");
    let formatted = serde_json::to_string_pretty(&settings)?;
    std::fs::write(&tmp, &formatted)?;
    std::fs::rename(&tmp, settings_path)?;

    println!("✓ removed all base hook entries");
    Ok(())
}

fn remove_claude_md_section(claude_md_path: &Path) -> Result<()> {
    print!("2. Remove BASE CLI section from CLAUDE.md ... ");

    if !claude_md_path.exists() {
        println!("not found — skipped");
        return Ok(());
    }

    let content = std::fs::read_to_string(claude_md_path)?;

    if !content.contains("## BASE CLI") {
        println!("not present — skipped");
        return Ok(());
    }

    // Find and remove the BASE CLI section (from "## BASE CLI" to end of file or next ## heading)
    let mut lines: Vec<&str> = content.lines().collect();
    let start = lines.iter().position(|l| l.starts_with("## BASE CLI"));

    if let Some(start_idx) = start {
        // Find the next ## heading after the BASE CLI section (or end of file)
        let end = lines[start_idx + 1..]
            .iter()
            .position(|l| l.starts_with("## ") && !l.starts_with("### "))
            .map(|pos| start_idx + 1 + pos)
            .unwrap_or(lines.len());

        lines.drain(start_idx..end);

        let new_content = lines.join("\n");
        let tmp = claude_md_path.with_extension("md.tmp");
        std::fs::write(&tmp, new_content.trim_end())?;
        std::fs::rename(&tmp, claude_md_path)?;

        println!("✓ removed");
    } else {
        println!("not found — skipped");
    }

    Ok(())
}

// ─── Step 1: Install binary ─────────────────────────────────

fn install_binary(binary: &Path, dest: &Path, bin_dir: &Path) -> Result<()> {
    print!("1. Install binary → {} ... ", dest.display());

    std::fs::create_dir_all(bin_dir)
        .with_context(|| format!("Creating {}", bin_dir.display()))?;

    // If we're already running from the destination, the binary is in place.
    // Removing it would unlink the running executable — the gated installer
    // extracts straight to ~/.local/bin/base, then invokes `base install`, so
    // current_exe() == dest. Remove-then-copy would self-delete the binary.
    let already_in_place = dest.exists()
        && matches!(
            (binary.canonicalize(), dest.canonicalize()),
            (Ok(a), Ok(b)) if a == b
        );

    if !already_in_place {
        // Remove existing binary
        if dest.exists() {
            std::fs::remove_file(dest)
                .with_context(|| format!("Removing existing {}", dest.display()))?;
        }

        // Copy binary (not symlink — this is a shippable install)
        std::fs::copy(binary, dest)
            .with_context(|| format!("Copying {} → {}", binary.display(), dest.display()))?;
    }

    // Set executable permission on unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755))?;
    }

    println!("✓");
    Ok(())
}

/// Percent-mode keys added to an existing `[bracket]` section on upgrade.
/// `mode` is written commented-OUT deliberately: turning percent on silently would
/// measure the user against the fallback 200k window, and anyone on a larger-context
/// model would compute several times their real depletion and sit in CRITICAL from
/// their first prompt. They opt in after setting `context_window` for their model.
const BRACKET_PERCENT_KEYS: &str = r#"
# ── context-percentage mode (added in v0.10.5) ──
# Reads REAL context depletion from the session transcript instead of counting
# prompts. Turn length is a wildcard — a build turn reading three large files eats
# far more window than a discussion turn — so prompt counts trip early while
# chatting and late while building. Percent measures the thing you actually care
# about, and means the same on every machine.
#
# TO ENABLE: set context_window for YOUR model, then uncomment mode.
# Left off until you do: with a wrong window every session reads CRITICAL.
# mode = "percent"
context_window = 200000   # your model's window (1000000 for 1M-context models)
fresh_until_pct = 20      # 0–N% consumed: full injection
moderate_until_pct = 45   # then: trimmed injection
depleted_until_pct = 70   # past this: minimal injection
"#;

/// Add percent-mode keys to a pre-v0.10.5 `[bracket]` section, then say so.
///
/// Idempotent: a config already carrying `context_window` is left alone. Existing
/// turn thresholds are never touched — they remain the fallback for the first
/// prompt of a session and for unreadable transcripts.
fn migrate_bracket_percent(base_toml: &Path) {
    let Ok(content) = std::fs::read_to_string(base_toml) else {
        return;
    };
    if content.contains("context_window") {
        return; // already migrated
    }

    let updated = if let Some(idx) = content.find("[bracket]") {
        // Insert immediately after the section header so the keys land inside it.
        let after_header = idx + "[bracket]".len();
        let mut s = String::with_capacity(content.len() + BRACKET_PERCENT_KEYS.len());
        s.push_str(&content[..after_header]);
        s.push_str(BRACKET_PERCENT_KEYS);
        s.push_str(&content[after_header..]);
        s
    } else {
        // No [bracket] section at all — append a complete one.
        format!("{content}\n# ─── [bracket] — context-window pressure tiers ───────────────\n[bracket]\nenabled = true\nfresh_until = 3\nmoderate_until = 10\ndepleted_until = 20\nrefresh_interval = 5\n{BRACKET_PERCENT_KEYS}")
    };

    if std::fs::write(base_toml, updated).is_err() {
        return;
    }

    println!("   ↑ base.toml: added context-percentage bracket keys (v0.10.5)");
    println!("     Brackets still count turns until you opt in. To switch:");
    println!("       1. set  context_window  to your model's window");
    println!("       2. uncomment  mode = \"percent\"");
    println!("     Then tune fresh/moderate/depleted _pct to taste.");
    println!("     File: {}", base_toml.display());
}

// ─── Step 2: Global tier ────────────────────────────────────

fn create_global_tier(global_dir: &Path) -> Result<()> {
    print!("2. Global tier → {} ... ", global_dir.display());

    std::fs::create_dir_all(global_dir)?;

    // base.toml — only create if missing
    let base_toml = global_dir.join("base.toml");
    if !base_toml.exists() {
        std::fs::write(
            &base_toml,
            r#"# BASE — Proactive context-injection engine for Claude Code
# Built by Chris Kahler · Chris AI Systems
# Community: https://www.skool.com/claude-code-titans-9203

# Each [section] documents what it does, what it runs at session-start, and what
# every knob controls. Set enabled = false to silence a whole section.

# ─── [namespace] — graph identity ────────────────────────────
# The prefix + URI stamped on every triple base writes. Set once; don't change.
[namespace]
prefix = "ops"
uri = "http://ops-sys.local/ontology#"

# ─── [devmode] — per-response diagnostics ────────────────────
# Appends a 🔧 DEVMODE block (loaded domains + context bracket) to each response.
[devmode]
enabled = true            # false = no diagnostic block

# ─── [bracket] — context-window pressure tiers ───────────────
# Scales how much gets injected as the context window fills.
#
# mode = "percent" (default) reads REAL depletion from the session transcript.
# mode = "turns" uses the prompt count instead — the legacy behavior.
# Percent is preferred because turn length is a wildcard: a build turn that reads
# three large files eats far more window than a discussion turn, so a fixed prompt
# count trips early while chatting and late while building — backwards from intent.
# The turn thresholds below stay live as the fallback for the first prompt of a
# session (no usage written yet) or an unreadable transcript, so this never blinds.
[bracket]
enabled = true
mode = "percent"          # "percent" (context-aware) | "turns" (legacy)

# Percent-mode thresholds — % of context_window consumed.
context_window = 200000   # set to your model's window (1000000 for 1M-context models)
fresh_until_pct = 20      # 0–N%: full injection
moderate_until_pct = 45   # then: trimmed injection
depleted_until_pct = 70   # past this: minimal injection

# Turn-mode thresholds — also the percent-mode fallback.
fresh_until = 3           # prompts 0–N: full injection
moderate_until = 10       # then: trimmed injection
depleted_until = 20       # past this: minimal injection
refresh_interval = 5      # re-survey window pressure every N prompts

# ─── [bracket.rules] — rules injected by tier ────────────────
# Domains inject on a keyword or path match, which makes them the wrong home for a
# rule that must hold regardless of subject — it silently stops applying the moment
# the conversation drifts off its triggers. These inject on the TIER alone.
#
# `always` goes out every prompt at every tier: the layer that survives a long
# session because it is re-sent, not remembered. Tier buckets are ADDITIVE with it,
# so a DEPLETED prompt receives always + depleted. Never deduped.
#
# [bracket.rules]
# always   = ["A rule that must never erode, with its BECAUSE attached."]
# fresh    = ["Room to spare — fuller guidance here."]
# moderate = ["Condensed."]
# depleted = ["Terse. Prefer precision over coverage."]
# critical = ["Minimal. Only what cannot be dropped."]

# ─── [signal] — session-start injection engine ───────────────
# The block you see when a session opens. Runs:
#   active_awareness → [Active Projects] / [Active Tasks]  (your working set)
#   pulse            → <base-pulse> workspace-grooming health
#   flow_resurface   → see [flow]
#   handoff_scan     → [Pick up where you left off]
#   reminder_scan    → [Reminders]
[signal]
enabled = true            # master switch for all session-start injection
max_chars = 2000          # injection budget per session-start (truncates past it)

# ─── [sync] — graph extraction globs ─────────────────────────
# Which files `base sync` reads to extract metadata/AST into the graph.
[sync]
include = ["**/*.md", "**/paul.json"]
exclude = ["node_modules/", "target/", ".git/", ".base/"]

# ─── [flow] — resurfacing scans + behavioral rules ───────────
# Surfaces things needing attention. Runs:
#   blocked_by_scan        → items just unblocked (their blocker completed)
#   deferred_orphan_scan   → deferred items past their resurface date
#   mention_threshold_scan → recurring ideas worth promoting to projects
#   protocol rules block   → injects static status-lifecycle behavioral rules
[flow]
enabled = true            # master switch for all flow scans
resurface = true          # blocked-by + deferred-resurface scans
protocol = true           # inject status-lifecycle behavioral rules
mentions = true           # surface recurring ideas (once >= mention_threshold)
mention_threshold = 3     # mentions before an idea surfaces

# ─── [memory] — auto-memory persistence ──────────────────────
# Where Claude's auto-memory lands.
#   mode: "claude" = flat files · "base" = graph only · "both" = mirror to both
[memory]
enabled = true
mode = "base"

# ─── [protocol] — active⇄deferred reconcile ──────────────────
# At session-start, sets each project's lastActive from its folder's newest file,
# then auto-defers working projects gone cold (and revives touched ones). This is
# what keeps [Active Projects] honest — your true working set.
[protocol]
enabled = true
stale_days = 7            # a working project untouched this many days → auto-deferred
"#,
        )?;
        println!("✓ (created base.toml)");
    } else {
        println!("✓ (base.toml exists, preserved)");
        migrate_bracket_percent(&base_toml);
    }

    // domains.toml — only create if missing
    let domains_toml = global_dir.join("domains.toml");
    if !domains_toml.exists() {
        std::fs::write(
            &domains_toml,
            r#"# BASE — Domain configuration
# Built by Chris Kahler · Chris AI Systems
# Community: https://www.skool.com/claude-code-titans-9203
#
# Global domains — loaded in every workspace.
# Workspace-specific domains go in {workspace}/.base/domains.toml

[[domain]]
name = "GLOBAL"
mode = "always"
prompt_keywords = []
file_keywords = []
rules = []
# Add your always-on rules here
"#,
        )?;
        println!("   Created domains.toml (empty — configure your domains)");
    } else {
        println!("   domains.toml exists, preserved");
    }

    // standards.toml — bootstrap from the embedded curated seed if missing.
    // Ships the standards-injection layer active out of the box; MIDAS users
    // get canonical text re-derived on their first `base standards sync`.
    // Disable via `[standards] enabled = false` in base.toml.
    let standards_toml = global_dir.join("standards.toml");
    if !standards_toml.exists() {
        let seed = crate::standards::sync::seed_file();
        std::fs::write(&standards_toml, toml::to_string_pretty(&seed)?)?;
        println!(
            "   Created standards.toml ({} standards seeded — `base standards list`)",
            seed.standards.len()
        );
    } else {
        println!("   standards.toml exists, preserved");
    }

    // docs/markdown-ontology-protocol.md — bundled MOP spec
    let docs_dir = global_dir.join("docs");
    std::fs::create_dir_all(&docs_dir)?;
    let mop_path = docs_dir.join("markdown-ontology-protocol.md");
    if !mop_path.exists() {
        std::fs::write(&mop_path, include_str!("../docs/markdown-ontology-protocol.md"))?;
        println!("   Created docs/markdown-ontology-protocol.md");
    }

    // docs/parallel-paul-protocol.md — relay choreography for parallel sessions
    let relay_doc = docs_dir.join("parallel-paul-protocol.md");
    std::fs::write(&relay_doc, include_str!("../docs/parallel-paul-protocol.md"))?;

    // extensions/ directory + _template.toml
    let ext_dir = global_dir.join("extensions");
    std::fs::create_dir_all(&ext_dir)?;
    let template_path = ext_dir.join("_template.toml");
    if !template_path.exists() {
        std::fs::write(
            &template_path,
            r#"# BASE Extension Contract v1
# ═══════════════════════════════════════════════════════════
# Copy this file, rename to your extension name (e.g., outpost.toml),
# fill in the sections you need, and place in this directory.
#
# BASE scans ~/.base-gbl/extensions/*.toml on every hook fire.
# Files that parse = active. Delete or rename to disable.
#
# Only [extension] section is required. All [hooks.*] sections are optional.
# Declare only the hooks your framework needs.
#
# Built by Chris Kahler · Chris AI Systems
# Community: https://www.skool.com/claude-code-titans-9203
# ═══════════════════════════════════════════════════════════

[extension]
name = "my-extension"           # Required. Unique slug (lowercase, hyphens, no spaces).
version = "0.1.0"              # Required. Semver.
description = "One-line description of what this extension does"  # Required.
# framework_dir = "~/.claude/my-framework/"    # Optional. Where framework files live.
# state_dir = ".my-state/"                      # Optional. Workspace-relative state path.

# ─── Session Start Hook ──────────────────────────────────
# Runs once per session. Use for: status injection, state ingestion, summary queries.
#
# [hooks.session_start]
# queries = ["queries/summary.sparql"]    # SPARQL files, relative to framework_dir
# inject = "My Extension: {count} items"  # Template string, vars from query results
#
# [[hooks.session_start.ingest]]          # State files to pull into the graph
# file = "data.json"                      # Relative to state_dir
# entity = "MyEntity"                     # RDF entity type (ops:MyEntity)
# strategy = "upsert"                     # "upsert" or "replace"

# ─── User Prompt Hook ────────────────────────────────────
# Domains merge into the normal domain pool. Get dedup, bracketing, matching for free.
#
# [[hooks.user_prompt.domains]]
# name = "my-domain"
# prompt_keywords = ["my-keyword", "another-keyword"]
# file_keywords = [".my-state/"]
# rules = ["Rule text injected when domain matches."]
# # query = "my-query"         # Optional: SPARQL query file to run on match
# # query_format = "table"     # Optional: "table" | "list" | "prose"

# ─── Pre-Tool Hook ───────────────────────────────────────
# File-path triggers for context injection before tool execution.
#
# [[hooks.pre_tool.triggers]]
# paths = [".my-state/"]
# inject = "This file is managed by My Extension."

# ─── Post-Tool Hook ──────────────────────────────────────
# React to file changes after tool execution. Each handler matches a file then
# runs an action. `pattern` is a substring of the file path, OR the reserved
# token "designset" (built-in design/frontend file heuristic: stylesheets,
# components, templates, svg, design folders, config + token files, and
# CSS-in-JS / Tailwind / inline-style content markers).
#
# action = "reingest"  — re-pull the matched file into the graph
# action = "log"       — debug line to stderr (you won't see it; for diagnostics)
# action = "inject"    — print a message to Claude (stdout). The "verify-reflex":
#                        nudge Claude to do something after it writes a file.
#
# [[hooks.post_tool.handlers]]
# pattern = "data.json"
# action = "reingest"
#
# Verify-reflex example — fire ONCE per session when design work is written,
# telling Claude to verify it (this is exactly how the design-humanizer skill
# wires itself in):
#
# [[hooks.post_tool.handlers]]
# pattern = "designset"           # built-in design-file detector (or use a substring)
# action = "inject"
# once_per_session = true         # default true; re-fires only if `message` changes
# # on_tools = ["Write", "Edit", "MultiEdit"]   # default; never fires on Read
# message = "Design work detected — verify with /design-humanizer scan before shipping."

# ─── Drop-in CLI Commands (v0.6) ─────────────────────────
# Contribute new `base <name>` subcommands without forking the binary. Any
# unrecognized `base <name> …` is routed to your handler with the args forwarded
# verbatim. Core commands always win — a plugin can never shadow a built-in.
#
# The handler MUST be directly executable: a shebang'd script (`#!/usr/bin/env
# node`, then `chmod +x`) or a compiled binary — any language. base inherits its
# stdio, so whatever the handler prints to stdout flows straight to the caller
# (print a `--json` line and Claude/the shell sees it unchanged).
#
# Handler path: tilde-expanded; absolute used as-is; relative resolved against
# `framework_dir`, else this file's own directory.
#
# base injects an env contract into every plugin process:
#   BASE_WORKSPACE   — workspace root (the dir containing .base/)
#   BASE_GRAPH_PATH  — the workspace graph file (.base/graph.nq)
#   BASE_GLOBAL_DIR  — ~/.base-gbl
#   BASE_BIN         — path to the running `base` binary
# …plus every KEY=VALUE in ~/.base-gbl/.env (the base-framework secret store —
# put your API keys there; they arrive as env vars, never overriding an
# already-exported var). Plugins mutate base state ONLY by calling back through
# $BASE_BIN (e.g. `"$BASE_BIN" learn --text … --domain …`) — base stays the sole
# graph writer.
#
# [[commands]]
# name = "my-command"                      # invoked as `base my-command …`
# handler = "bin/my-command.mjs"           # executable; relative to framework_dir
# description = "What it does (shown in `base ext list`)"
# usage = "base my-command --flag value"   # optional one-line usage hint
"#,
        )?;
        println!("   Created extensions/ + _template.toml");
    } else {
        println!("   extensions/ exists, preserved");
    }

    Ok(())
}

// ─── Step 3: Wire hooks ─────────────────────────────────────

/// Every hook base needs wired, and the command that serves it.
///
/// ONE table, two consumers: [`wire_hooks`] merges it into a host's
/// `settings.json`, and [`hooks_manifest`] publishes it so another installer —
/// the desktop app, which owns host-config merging — wires exactly the same
/// thing. A second hand-maintained copy is the whole failure this constant
/// exists to prevent: it would drift, and the drift would surface as hooks that
/// look installed and never fire.
pub const HOOK_TABLE: [(&str, &str); 5] = [
    ("SessionStart", "base hook session-start"),
    ("UserPromptSubmit", "base hook user-prompt-submit"),
    ("PreToolUse", "base hook pre-tool-use"),
    ("PostToolUse", "base hook post-tool-use"),
    ("Stop", "base hook stop"),
];

/// The object base pushes into `settings.hooks[event]` for one hook.
///
/// Public because the manifest publishes it verbatim: an external installer
/// merges the SAME value base would have written, not its own reading of a
/// description of it.
pub fn hook_entry(command: &str) -> serde_json::Value {
    serde_json::json!({
        "hooks": [ { "type": "command", "command": command } ]
    })
}

/// The hook command table as JSON, for an installer outside base.
///
/// base deliberately does not merge host configs itself — see the fork
/// `base-sync-client-surface` R4. Two implementations of merge-never-overwrite
/// is one too many on the platform where a bad merge costs someone their editor
/// config, so base publishes the table and the app owns the merge.
///
/// `binary` is this executable's resolved path, for a host that cannot rely on
/// `PATH`. `command` is what base itself writes and is PATH-relative; an
/// installer that needs an absolute command substitutes `binary` for the
/// leading `base` token.
/// How an external installer must merge [`hooks_manifest`], in one string so
/// the documented rule and the tested rule cannot drift apart.
pub fn manifest_merge_rule() -> &'static str {
    "append the `entry` object to the array at settings.hooks[event]; \
     skip when an entry with the same `command` is already present"
}

pub fn hooks_manifest() -> serde_json::Value {
    let hooks: Vec<serde_json::Value> = HOOK_TABLE
        .iter()
        .map(|(event, command)| {
            serde_json::json!({
                "event": event,
                "command": command,
                "entry": hook_entry(command),
            })
        })
        .collect();

    serde_json::json!({
        "version": 1,
        "binary": std::env::current_exe()
            .ok()
            .map(|p| p.display().to_string()),
        "settings_key": "hooks",
        "merge": manifest_merge_rule(),
        "hooks": hooks,
    })
}

fn wire_hooks(settings_path: &Path) -> Result<()> {
    print!("3. Wire hooks → {} ... ", settings_path.display());

    if !settings_path.exists() {
        println!("⊘ settings.json not found, skipped");
        return Ok(());
    }

    let content = std::fs::read_to_string(settings_path)?;
    let mut settings: serde_json::Value = serde_json::from_str(&content)
        .context("Failed to parse settings.json")?;

    let hook_entries = HOOK_TABLE;

    // Check if already fully wired
    let all_present = hook_entries.iter().all(|(_, cmd)| content.contains(cmd));
    if all_present {
        println!("✓ (already wired)");
        return Ok(());
    }

    let hooks = settings
        .as_object_mut()
        .context("settings.json is not an object")?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));

    let hooks_obj = hooks
        .as_object_mut()
        .context("hooks is not an object")?;

    let mut added = Vec::new();

    for (event, command) in &hook_entries {
        // Skip if this specific hook is already present
        if content.contains(command) {
            continue;
        }

        let event_hooks = hooks_obj
            .entry(*event)
            .or_insert_with(|| serde_json::json!([]));

        if !event_hooks.is_array() {
            *event_hooks = serde_json::json!([]);
        }

        let arr = event_hooks.as_array_mut().unwrap();

        // The manifest publishes this exact value; build it in one place so an
        // external installer cannot merge something base would not have written.
        arr.push(hook_entry(command));

        added.push(*event);
    }

    // Write back atomically
    let tmp_path = settings_path.with_extension("json.tmp");
    let formatted = serde_json::to_string_pretty(&settings)?;
    std::fs::write(&tmp_path, &formatted)?;
    std::fs::rename(&tmp_path, settings_path)?;

    if added.is_empty() {
        println!("✓ (already wired)");
    } else {
        println!("✓ (added base hook {})", added.join(", "));
    }
    Ok(())
}

// ─── Step 4: Migrate CARL ───────────────────────────────────

fn migrate_carl(global_dir: &Path, carl_path: &Path) -> Result<()> {
    print!("4. Migrate CARL decisions → graph ... ");

    if !carl_path.exists() {
        println!("⊘ carl.json not found at {}", carl_path.display());
        return Ok(());
    }

    let config = BaseConfig::load(global_dir);
    match crate::domain::sync::sync_domains_to_graph(&config, global_dir, Some(carl_path)) {
        Ok(stats) => {
            println!(
                "✓ ({} domains, {} rules, {} decisions)",
                stats.domains, stats.rules, stats.decisions
            );
        }
        Err(e) => {
            println!("⚠ Migration failed: {e}");
            println!("   You can retry later: base domain sync --carl {}", carl_path.display());
        }
    }

    Ok(())
}

// ─── Step 5: Install scripts ────────────────────────────────

fn install_scripts(binary_path: &Path, global_dir: &Path) -> Result<()> {
    print!("5. Install AST scripts ... ");

    let scripts_dest = global_dir.join("scripts").join("ast");
    std::fs::create_dir_all(&scripts_dest)?;

    // Find scripts relative to the binary source (dev builds) or cwd
    let source_candidates = [
        // Same directory as source repo
        binary_path
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("scripts").join("ast")),
        // Cargo target dir (target/release/../scripts/ast → ../../scripts/ast)
        binary_path
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(|p| p.join("scripts").join("ast")),
        // Current working directory
        Some(std::env::current_dir().unwrap_or_default().join("scripts").join("ast")),
    ];

    let source_dir = source_candidates
        .iter()
        .filter_map(|p| p.as_ref())
        .find(|p| p.join("onto_ast.py").exists());

    let Some(source_dir) = source_dir else {
        // No source near the binary (e.g. `cargo install` drops only the binary,
        // or a Windows release ships scripts straight to the global dir). If the
        // destination already has the extractor, that's success — report ✓.
        if scripts_dest.join("onto_ast.py").exists() {
            println!("✓ (already present in {})", scripts_dest.display());
            install_python_deps(&scripts_dest);
        } else {
            println!("⊘ scripts/ast/ not found near binary — skipped");
            println!("   Copy scripts/ast/ to {} manually", scripts_dest.display());
        }
        return Ok(());
    };

    // Copy all .py files
    let mut count = 0;
    for entry in std::fs::read_dir(source_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "py") {
            let dest = scripts_dest.join(entry.file_name());
            std::fs::copy(&path, &dest)?;
            count += 1;
        }
    }

    // Also copy requirements.txt if present
    let req = source_dir.join("requirements.txt");
    if req.exists() {
        std::fs::copy(&req, scripts_dest.join("requirements.txt"))?;
    }

    println!("✓ ({count} scripts → {})", scripts_dest.display());
    install_python_deps(&scripts_dest);
    Ok(())
}

// ─── Claude skills ──────────────────────────────────────────

/// Skills shipped with base, installed into `~/.claude/skills/<name>/`.
const BUNDLED_SKILLS: &[&str] = &["base-help"];

/// Repo-relative parent of the bundled skills (also the GitHub API path).
const SKILLS_REPO_DIR: &str = "claude/skills";

/// Generous next to the 3s update ping: `base-help` carries an 87KB Q&A bank and
/// this runs once per install, not on every session start.
const SKILL_FETCH_TIMEOUT_SECS: u64 = 15;

/// How a skill install reports itself. `base install` is a numbered checklist;
/// `base update` is a one-line footnote to a binary swap; the background update
/// has no reader at all and relies on `base doctor` to surface drift later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SkillReport {
    /// Numbered step inside the `base install` checklist.
    InstallStep,
    /// One line under `base update`.
    Line,
    /// Print nothing (background update).
    Silent,
}

/// Where a skill's files come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SkillSource {
    /// A local checkout wins if there is one, else fetch the tag. Right for
    /// `base install`: that checkout is the tree the operator is running.
    LocalThenTag,
    /// Always fetch the tag being installed. Right for `base update`, where the
    /// operator is explicitly installing a *release* — `find_local_skills_dir`
    /// falls back to `current_dir()`, so running `base update` inside a base
    /// checkout would otherwise install that working tree instead of the
    /// release's skill.
    TagOnly,
}

/// Install bundled skills into `~/.claude/skills/`.
///
/// Two sources, in order. A local checkout wins, because that is the tree the
/// operator is actually running. Failing that we fetch from GitHub: release
/// assets carry only the binary and the AST scripts, so anyone who installed
/// from a tarball has no local copy of the skill at all.
///
/// `version` is the base version whose skill we want — for `base install` that
/// is this binary, for `base update` it is the binary that was just swapped in.
/// It must be threaded rather than read from `CARGO_PKG_VERSION` at the fetch,
/// because during an update this process IS the outgoing version.
///
/// Never fatal. A skill that cannot be installed prints a manual fallback and
/// `base install` still completes — the CLI works fine without it.
pub(crate) fn install_skills(
    binary_path: &Path,
    home: &Path,
    version: &str,
    source: SkillSource,
    report: SkillReport,
) -> Result<()> {
    if report == SkillReport::InstallStep {
        print!("6. Install Claude skills ... ");
    }

    let dest_root = home.join(".claude").join("skills");
    std::fs::create_dir_all(&dest_root)?;
    let local_root = match source {
        SkillSource::LocalThenTag => find_local_skills_dir(binary_path),
        SkillSource::TagOnly => None,
    };

    let mut results: Vec<String> = Vec::new();
    let mut failures: Vec<(String, String)> = Vec::new();

    for skill in BUNDLED_SKILLS {
        let local = local_root
            .as_ref()
            .map(|r| r.join(skill))
            .filter(|p| p.join("SKILL.md").is_file());

        match install_one_skill(skill, local.as_deref(), &dest_root, version) {
            Ok(outcome) => results.push(outcome),
            Err(e) => failures.push((skill.to_string(), e.to_string())),
        }
    }

    match report {
        SkillReport::Silent => {}
        SkillReport::InstallStep => {
            if failures.is_empty() {
                println!("✓ ({})", results.join(", "));
            } else {
                println!("partial");
                for r in &results {
                    println!("   ✓ {r}");
                }
                for (skill, err) in &failures {
                    println!("   ⊘ {skill} — {err}");
                    println!(
                        "      Copy {SKILLS_REPO_DIR}/{skill}/ to {} manually",
                        dest_root.join(skill).display()
                    );
                }
            }
        }
        SkillReport::Line => {
            for r in &results {
                println!("✓ Claude skills: {r}");
            }
            // A failed refresh is worth one honest line, not a stack trace: the
            // binary swap already succeeded and that is the part that matters.
            for (skill, err) in &failures {
                println!("  (skill {skill} not refreshed — {err}; run `base install` to retry)");
            }
        }
    }
    Ok(())
}

/// Stage a skill, then reconcile it against whatever is already installed.
///
/// Staging first means a failed download never leaves a half-written skill in
/// place. The reconcile step exists because `base-help` rewrites its own Q&A
/// bank as it learns — overwriting on every `base install` would silently
/// discard the operator's appended pairs, so a changed skill is backed up
/// before it is replaced, and an unchanged one is left alone entirely.
fn install_one_skill(
    skill: &str,
    local: Option<&Path>,
    dest_root: &Path,
    version: &str,
) -> Result<String> {
    let dest = dest_root.join(skill);
    // Stage inside dest_root so the final promotion is a same-filesystem rename.
    let staging = dest_root.join(format!(".{skill}.staging-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);

    let staged = match local {
        Some(src) => copy_dir_all(src, &staging).map(|n| (n, "local")),
        None => fetch_skill_from_github(skill, &staging, version).map(|n| (n, "fetched")),
    };
    let (count, source) = match staged {
        Ok(v) => v,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
    };

    if !dest.exists() {
        std::fs::rename(&staging, &dest)?;
        return Ok(format!("{skill} ({count} files, {source})"));
    }
    if dirs_identical(&staging, &dest) {
        let _ = std::fs::remove_dir_all(&staging);
        return Ok(format!("{skill} (already current)"));
    }

    // Something differs. Whether that matters depends on WHO changed it, and a
    // whole-tree compare cannot tell: the bank is designed to be appended to
    // (the skill's own close-the-loop rule writes new `### Q:` pairs into it),
    // so "the operator added answers" and "the release shipped a new bank" look
    // identical here. The recorded hash of the bank as last shipped separates
    // them, and the two cases deserve different words — never silence.
    let locally_modified = bank_locally_modified(&dest);
    let backup = dest_root.join(format!(
        "{skill}.bak-{}",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    ));
    std::fs::rename(&dest, &backup)?;
    std::fs::rename(&staging, &dest)?;
    if locally_modified {
        Ok(format!(
            "{skill} ({count} files, {source}) — YOUR EDITED Q&A BANK was replaced; your copy is kept in full at {}",
            backup.display()
        ))
    } else {
        Ok(format!(
            "{skill} ({count} files, {source}; previous kept at {})",
            backup.display()
        ))
    }
}

/// Did the operator edit the installed Q&A bank since it was last shipped?
///
/// Compares the bank on disk against the hash recorded in `manifest.toml` at its
/// last install. No recorded hash means we cannot tell — report false rather
/// than crying wolf on every machine that installed before the hash existed.
/// The backup happens either way; this only decides how loudly to say so.
fn bank_locally_modified(dest: &Path) -> bool {
    let Some(recorded) = Manifest::load()
        .and_then(|m| m.components.get("base-help").map(|c| c.content_hash.clone()))
        .filter(|h| !h.is_empty())
    else {
        return false;
    };
    let Ok(current) = std::fs::read(dest.join("references").join("qa.md")) else {
        return false;
    };
    manifest::hash_bytes(&current) != recorded
}

/// Locate `claude/skills/` relative to the running binary, mirroring the search
/// `install_scripts` does for `scripts/ast/`.
fn find_local_skills_dir(binary_path: &Path) -> Option<std::path::PathBuf> {
    let candidates = [
        binary_path.parent().and_then(|p| p.parent()).map(skills_dir),
        binary_path
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(skills_dir),
        Some(skills_dir(&std::env::current_dir().unwrap_or_default())),
    ];
    candidates.into_iter().flatten().find(|p| p.is_dir())
}

fn skills_dir(root: &Path) -> std::path::PathBuf {
    root.join("claude").join("skills")
}

/// Recursively copy `src` into `dest`, returning the number of files written.
fn copy_dir_all(src: &Path, dest: &Path) -> Result<usize> {
    std::fs::create_dir_all(dest)?;
    let mut count = 0;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let target = dest.join(entry.file_name());
        if path.is_dir() {
            count += copy_dir_all(&path, &target)?;
        } else {
            std::fs::copy(&path, &target)?;
            count += 1;
        }
    }
    Ok(count)
}

/// Byte-for-byte comparison of two directory trees.
fn dirs_identical(a: &Path, b: &Path) -> bool {
    let (Ok(mut left), Ok(mut right)) = (collect_tree(a), collect_tree(b)) else {
        return false;
    };
    left.sort();
    right.sort();
    left == right
}

/// Flatten a tree into sorted (relative path, contents) pairs.
fn collect_tree(root: &Path) -> Result<Vec<(std::path::PathBuf, Vec<u8>)>> {
    fn walk(
        dir: &Path,
        root: &Path,
        out: &mut Vec<(std::path::PathBuf, Vec<u8>)>,
    ) -> Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                walk(&path, root, out)?;
            } else {
                let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                out.push((rel, std::fs::read(&path)?));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, root, &mut out)?;
    Ok(out)
}

/// Fetch a skill from the repo. Prefers the tag matching the version being
/// installed so the skill and the CLI it documents stay in step; falls back to
/// `main` for dev builds whose version was never tagged.
///
/// `version` is passed in rather than read from `CARGO_PKG_VERSION` here: during
/// `base update` this process is the OUTGOING binary, so compiling the tag in
/// would fetch the skill for the version being replaced.
fn fetch_skill_from_github(skill: &str, dest: &Path, version: &str) -> Result<usize> {
    let tag = skill_tag(version);
    let mut last_err: Option<anyhow::Error> = None;

    for git_ref in [tag.as_str(), "main"] {
        match fetch_tree(&format!("{SKILLS_REPO_DIR}/{skill}"), git_ref, dest) {
            Ok(n) if n > 0 => return Ok(n),
            Ok(_) => last_err = Some(anyhow::anyhow!("{skill} is empty at {git_ref}")),
            Err(e) => {
                let _ = std::fs::remove_dir_all(dest);
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("could not fetch {skill}")))
}

/// The git tag for a base version. PURE — the seam that lets the version
/// threading be tested without touching the network.
pub(crate) fn skill_tag(version: &str) -> String {
    format!("v{}", version.trim().trim_start_matches('v'))
}

/// Recursively download a repo directory through the GitHub contents API.
fn fetch_tree(repo_path: &str, git_ref: &str, dest: &Path) -> Result<usize> {
    let url = format!(
        "https://api.github.com/repos/{}/contents/{repo_path}?ref={git_ref}",
        manifest::GITHUB_REPO
    );
    let entries: Vec<serde_json::Value> = ureq::get(&url)
        .set("User-Agent", "base-install")
        .timeout(std::time::Duration::from_secs(SKILL_FETCH_TIMEOUT_SECS))
        .call()
        .with_context(|| format!("listing {repo_path}@{git_ref}"))?
        .into_json()
        .with_context(|| format!("parsing listing for {repo_path}@{git_ref}"))?;

    std::fs::create_dir_all(dest)?;
    let mut count = 0;
    for entry in entries {
        let Some(name) = entry.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        match entry.get("type").and_then(|v| v.as_str()) {
            Some("dir") => {
                count += fetch_tree(&format!("{repo_path}/{name}"), git_ref, &dest.join(name))?;
            }
            Some("file") => {
                let Some(download) = entry.get("download_url").and_then(|v| v.as_str()) else {
                    continue;
                };
                let body = ureq::get(download)
                    .set("User-Agent", "base-install")
                    .timeout(std::time::Duration::from_secs(SKILL_FETCH_TIMEOUT_SECS))
                    .call()
                    .with_context(|| format!("downloading {name}"))?
                    .into_string()
                    .with_context(|| format!("reading {name}"))?;
                std::fs::write(dest.join(name), body)?;
                count += 1;
            }
            _ => {}
        }
    }
    Ok(count)
}

/// Best-effort `pip install -r requirements.txt` for the AST extractor. Uses the
/// resolved Python interpreter (`python` on Windows, `python3` on Unix) via
/// `python -m pip`, which sidesteps the Microsoft Store `python3` stub. Never
/// fatal: a failure (no pip, externally-managed env, offline) only prints a
/// manual-fallback hint so `base install` still completes.
fn install_python_deps(scripts_dest: &Path) {
    let req = scripts_dest.join("requirements.txt");
    if !req.exists() {
        return;
    }
    let py = crate::multimodal::python_bin();
    print!("   python deps (tree-sitter) ... ");
    let status = std::process::Command::new(py)
        .args(["-m", "pip", "install", "-q", "-r"])
        .arg(&req)
        .status();
    match status {
        Ok(s) if s.success() => println!("✓"),
        _ => {
            println!("⚠ skipped");
            println!("   Install manually: {py} -m pip install -r {}", req.display());
        }
    }
}

// ─── Step 6: Seed system rules ──────────────────────────────

fn seed_system_rules(global_dir: &Path) -> Result<()> {
    print!("5. Seed system rules ... ");

    let config = BaseConfig::load(global_dir);

    // Sync domains first so GLOBAL domain entity exists
    let _ = crate::domain::sync::sync_domains_to_graph(&config, global_dir, None);

    // Check if MOP rule already exists
    let ns = &config.namespace;
    let p = &ns.prefix;
    let domain_iri = crate::crud::build_iri(ns, "domain", "global");

    let check = format!(
        "SELECT ?text WHERE {{ GRAPH ?g {{ <{domain_iri}> {p}:hasRule ?r . ?r {p}:ruleText ?text . FILTER(CONTAINS(?text, \"Markdown Ontology Protocol\")) }} }}"
    );

    let already_exists = if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) =
        crate::crud::load_and_query(global_dir, ns, &check)
    {
        solutions.filter_map(|r| r.ok()).next().is_some()
    } else {
        false
    };

    if already_exists {
        println!("✓ (already seeded)");
        return Ok(());
    }

    // Seed MOP rule
    let _ = crate::crud::rule::add(
        global_dir,
        ns,
        "GLOBAL",
        "When writing or editing markdown files, follow the Markdown Ontology Protocol (MOP) — use YAML frontmatter with type, status, tags, and relatedTo fields so base sync can extract the document into the graph. Read ~/.base-gbl/docs/markdown-ontology-protocol.md before writing frontmatter.",
        None,
    );

    println!("✓ (MOP rule added to GLOBAL)");
    Ok(())
}

// ─── Step 7: CLAUDE.md integration ──────────────────────────

const BASE_CLI_SECTION: &str = r#"
## BASE CLI — Proactive Context Engine

The `base` binary is on PATH. Use these commands proactively during sessions — they write to a knowledge graph that persists across sessions and surfaces context automatically.

### When to call (proactive, not on-demand)

| Trigger | Command |
|---------|---------|
| Navigate code: find functions, callers, imports | `base ast query --contains "name"` or `--file`, `--calls`, `--imports` (add `--target apps/X` to query another app's map) |
| Discover which apps have a code map | `base ast list` |
| A decision is made (architectural, process, tooling) | `base decision log --domain X --decision "..." --rationale "..."` |
| An insight, correction, or lesson emerges | `base learn --text "..." --domain X --type insight\|correction\|decision` |
| User defines or refines a behavioral rule | `base rule add --domain X --text "..."` |
| Before making assumptions about prior context | `base recall --keyword "..."` or `base recall --domain X` |
| User asks to scaffold a new workspace | `base scaffold [path]` |

### Code navigation — MANDATORY FIRST TOOL

**ALWAYS use `base ast query` BEFORE grep, find, Read, or any MCP tool for code exploration.**
The AST graph already knows every function, struct, class, import, and call relationship. Scanning files without checking the graph first is wasteful — the graph gives you the map, then you Read only what matters.

- `base ast query --contains "auth"` (or `base a q -c "auth"`) — find entities by name
- `base ast query --file "main.rs"` (or `base a q -f "main.rs"`) — list entities in a file
- `base ast query --calls "validate"` — find all callers of a function
- `base ast query --imports "config.rs"` (or `base a q -i "config.rs"`) — find importers
- `base ast query --target apps/X -c "auth"` — query a specific app's map from anywhere (e.g. the parent workspace), no `cd` needed
- `base ast list` — see which apps have a code map, with entity counts + paths

Each app keeps its own self-contained map at `<app>/.base-ast/ast.ttl`, registered in the workspace graph and kept current automatically (a Stop hook refreshes the cwd app's map after each turn). Map a new app once with `base sync --ast --target apps/X`; thereafter it stays live.

**Order of operations:** `base ast query` first → understand structure → `Read` specific lines only. Never scan-then-understand when you can understand-then-read.

### GraphRAG — ask questions across a doc corpus

- `base graph extract --target docs/` — LLM pass over markdown → concepts + relationship edges in the graph (no API key, content-cached, ~25s/doc)
- `base graph query "<question>"` — retrieve the relevant subgraph and synthesize a **cited** answer in one command
- `base graph query "<q>" --raw` — return just the retrieved subgraph for the current session to reason over (highest-quality path)
- `base graph analyze` — emergent structure: god nodes (core abstractions), communities, surprising cross-community bridges

**Agentic retrieval** (read-only primitives — drive your own multi-call traversal instead of one-shot query):
- `base graph get-node "<label|slug>"` — one node's type, source, summary, and edges
- `base graph neighbors "<node>" -d N` — the N-hop neighborhood as edge lines
- `base graph path "<from>" "<to>"` — shortest path between two concepts

**Multimodal ingest is OFF by default** (markdown-only, zero extra deps). PDF/image/audio/video need it on:
- `base config set multimodal.enabled true` (or one-shot `base graph extract --multimodal`)
- **No sudo, ever.** PDF → in-process (`pdf-extract` crate, zero dep); image → Claude vision (uses the present `claude`, zero dep); audio/video → Whisper, whose `whisper`+`ffmpeg` install **once** via `pip install --user` (marker-gated), never again. An operator who never enables it — or only feeds docs/images — installs nothing.

### Project management

- `base project add --name "..." --path "src/x"` (or `base p a -n "..." -p "src/x"`) — register a project
- `base project list` (or `base p l`) — list projects
- `base milestone add --project X --name "..."` (or `base m a -p X -n "..."`) — add a milestone
- `base task add --project X --name "..."` (or `base t a -p X -n "..."`) — add a task
- `base task done <slug>` — mark complete

### Knowledge & memory

- `base learn --text "..." --domain X --type insight` — structured memory with relational edges
- `base recall --keyword "..." [--domain X]` — graph-backed relational search
- `base decision log --domain X --decision "..." --rationale "..."` — log a decision
- `base decision search --keyword "..."` — find prior decisions
- `base rule add --domain X --text "..."` — add a rule to a domain

### Sync & dashboard

- `base sync` — extract markdown metadata + body into graph
- `base sync --ast` — extract code structure (tree-sitter, 35+ languages)
- `base dashboard` (or `base dash`) — launch Command Center web dashboard

### What happens automatically (via hooks)

- **Session start:** Graph syncs domains, ingests paul.toml projects, runs signals
- **User prompt:** Matches keywords → injects domain rules + decisions + notes from graph
- **Pre-tool-use:** Matches file paths → injects AST file map + domain rules. Intercepts grep with graph hint. Injects markdown extraction contract on Write/Edit of .md files.
- **Post-tool-use:** Updates timestamps. Injects section-specific AST context for partial reads.

### Architecture

- Rules, decisions, notes, and projects are graph entities with relational edges
- `domains.toml` defines triggers only (keywords, paths) — rule content lives in the graph
- `~/.base-gbl/` = global tier, `{workspace}/.base/` = workspace tier
- Built by Chris Kahler · Chris AI Systems · https://www.skool.com/claude-code-titans-9203
"#;

/// Register the installed base-help coach in the manifest.
///
/// Records the base version its Q&A bank is stamped to (NOT this binary's — see
/// `manifest::detect_base_help`) plus a hash of the bank as shipped. Together
/// those give `base doctor` something to compare, so a coach lagging the binary
/// is reportable instead of invisible, and give the installer a way to tell an
/// operator-edited bank from a bank the release changed.
pub(crate) fn record_base_help(manifest: &mut Manifest, now: &str) {
    let Some(home) = crate::home::home_root() else {
        return;
    };
    if let Some(entry) = manifest::detect_base_help(&home, now) {
        manifest.components.insert("base-help".to_string(), entry);
    }
}

/// Refresh just the `base-help` manifest entry and save. Used after the update
/// path replaces the skill, where the full `base install` manifest write does
/// not run. Best-effort: a manifest that cannot be written must not fail an
/// update whose binary swap already committed.
pub(crate) fn record_base_help_after_update() {
    let mut manifest = Manifest::load().unwrap_or_default();
    let now = chrono::Local::now().to_rfc3339();
    record_base_help(&mut manifest, &now);
    let _ = manifest.save();
}

// ─── Step 8: Write manifest ─────────────────────────────────

fn write_manifest(global_dir: &Path, full: bool) -> Result<()> {
    print!("7. Write manifest.toml ... ");

    let now = chrono::Local::now().to_rfc3339();
    let mut manifest = Manifest::load().unwrap_or_default();

    // Preserve existing chrisai.installed_at if already set
    if manifest.chrisai.installed_at.is_empty() {
        manifest.chrisai.installed_at = now.clone();
    }

    // Always update/create BASE component
    let base_entry = manifest::ComponentEntry {
        version: env!("CARGO_PKG_VERSION").to_string(),
        path: "~/.local/bin/base".to_string(),
        installed_at: manifest
            .components
            .get("base")
            .map(|c| c.installed_at.clone())
            .unwrap_or_else(|| now.clone()),
        content_hash: String::new(),
    };
    manifest.components.insert("base".to_string(), base_entry);
    record_base_help(&mut manifest, &now);

    if full {
        // Detect and register all framework components
        let component_names = ["paul", "seed", "skillsmith"];
        for name in &component_names {
            if let Some(entry) = manifest::detect_component(name) {
                // Preserve existing installed_at if component was already registered
                let installed_at = manifest
                    .components
                    .get(*name)
                    .map(|c| c.installed_at.clone())
                    .unwrap_or(entry.installed_at);

                manifest.components.insert(
                    name.to_string(),
                    manifest::ComponentEntry {
                        version: entry.version,
                        path: entry.path,
                        installed_at,
                        content_hash: entry.content_hash,
                    },
                );
                println!("\n   ✓ {name} v{}", manifest.components[*name].version);
            } else {
                println!("\n   ⊘ {name} not found");
            }
        }
    }

    manifest.save()?;

    // Summary
    let component_list: Vec<String> = manifest
        .components
        .iter()
        .map(|(k, v)| format!("{k} v{}", v.version))
        .collect();

    if full {
        println!("\n   Manifest: {}/manifest.toml", global_dir.display());
        println!("   Components: {}", component_list.join(", "));
    } else {
        println!("✓ ({})", component_list.join(", "));
    }

    Ok(())
}

fn append_claude_md(claude_md_path: &Path) -> Result<()> {
    print!("5. CLAUDE.md integration ... ");

    if !claude_md_path.exists() {
        std::fs::write(claude_md_path, BASE_CLI_SECTION.trim_start())?;
        println!("✓ (created with BASE CLI section)");
        return Ok(());
    }

    let content = std::fs::read_to_string(claude_md_path)?;

    if content.contains("## BASE CLI") {
        println!("already present");
        return Ok(());
    }

    let mut new_content = content;
    if !new_content.ends_with('\n') {
        new_content.push('\n');
    }
    new_content.push_str(BASE_CLI_SECTION);

    let tmp = claude_md_path.with_extension("md.tmp");
    std::fs::write(&tmp, &new_content)?;
    std::fs::rename(&tmp, claude_md_path)?;

    println!("✓ (appended BASE CLI section)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BracketConfig;

    fn write(dir: &Path, body: &str) -> std::path::PathBuf {
        let p = dir.join("base.toml");
        std::fs::write(&p, body).unwrap();
        p
    }

    fn bracket_of(path: &Path) -> BracketConfig {
        #[derive(serde::Deserialize)]
        struct Probe {
            bracket: BracketConfig,
        }
        let text = std::fs::read_to_string(path).unwrap();
        toml::from_str::<Probe>(&text).expect("migrated base.toml must parse").bracket
    }

    #[test]
    fn migration_adds_percent_keys_to_existing_bracket() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write(
            tmp.path(),
            "[bracket]\nenabled = true\nfresh_until = 7\nmoderate_until = 20\ndepleted_until = 30\nrefresh_interval = 7\n\n[devmode]\nenabled = true\n",
        );
        migrate_bracket_percent(&p);

        let b = bracket_of(&p);
        assert_eq!(b.context_window, 200_000);
        assert_eq!(b.fresh_until_pct, 20.0);
        // Existing turn thresholds must survive untouched.
        assert_eq!(b.fresh_until, 7);
        assert_eq!(b.depleted_until, 30);
        // Percent stays OFF until the user sets their window and uncomments it —
        // silently enabling it against a wrong window pins the session to CRITICAL.
        assert!(!b.is_percent_mode());
        // Sections after [bracket] must not be captured by the insert.
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("[devmode]"));
    }

    #[test]
    fn migration_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write(tmp.path(), "[bracket]\nenabled = true\nfresh_until = 3\n");
        migrate_bracket_percent(&p);
        let once = std::fs::read_to_string(&p).unwrap();
        migrate_bracket_percent(&p);
        let twice = std::fs::read_to_string(&p).unwrap();
        assert_eq!(once, twice, "second run must be a no-op");
        // Count the assignment, not the word — it also appears in the guidance comment.
        assert_eq!(once.matches("context_window = ").count(), 1);
    }

    #[test]
    fn migration_appends_section_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write(tmp.path(), "[devmode]\nenabled = true\n");
        migrate_bracket_percent(&p);
        let b = bracket_of(&p);
        assert_eq!(b.context_window, 200_000);
        assert_eq!(b.refresh_interval, 5);
    }

    #[test]
    fn migration_leaves_already_configured_file_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let body = "[bracket]\nenabled = true\nmode = \"percent\"\ncontext_window = 1000000\n";
        let p = write(tmp.path(), body);
        migrate_bracket_percent(&p);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), body);
        assert!(bracket_of(&p).is_percent_mode());
    }

    // ─── Skill install ──────────────────────────────────────

    /// Build a skill source tree shaped like the real one: SKILL.md plus a
    /// references/ subdirectory, so the recursive paths are actually exercised.
    fn make_skill_src(root: &Path, bank: &str) -> std::path::PathBuf {
        let src = root.join("base-help");
        std::fs::create_dir_all(src.join("references")).unwrap();
        std::fs::write(src.join("SKILL.md"), "---\nname: base-help\n---\n").unwrap();
        std::fs::write(src.join("README.md"), "readme\n").unwrap();
        std::fs::write(src.join("references").join("qa.md"), bank).unwrap();
        src
    }

    #[test]
    fn copy_dir_all_recurses_and_counts_files() {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_skill_src(tmp.path(), "pairs\n");
        let dest = tmp.path().join("out");
        assert_eq!(copy_dir_all(&src, &dest).unwrap(), 3);
        assert!(dest.join("references").join("qa.md").is_file());
    }

    #[test]
    fn dirs_identical_detects_nested_difference() {
        let tmp = tempfile::tempdir().unwrap();
        let a = make_skill_src(&tmp.path().join("a"), "pairs\n");
        let b = make_skill_src(&tmp.path().join("b"), "pairs\n");
        assert!(dirs_identical(&a, &b));
        std::fs::write(b.join("references").join("qa.md"), "different\n").unwrap();
        assert!(!dirs_identical(&a, &b), "a nested change must not read as identical");
    }

    #[test]
    fn install_one_skill_installs_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_skill_src(&tmp.path().join("repo"), "pairs\n");
        let dest_root = tmp.path().join("skills");
        std::fs::create_dir_all(&dest_root).unwrap();

        let msg = install_one_skill("base-help", Some(&src), &dest_root, "0.13.2").unwrap();
        assert!(msg.contains("3 files"), "got: {msg}");
        assert!(dest_root.join("base-help").join("SKILL.md").is_file());
    }

    #[test]
    fn install_one_skill_is_a_noop_when_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_skill_src(&tmp.path().join("repo"), "pairs\n");
        let dest_root = tmp.path().join("skills");
        std::fs::create_dir_all(&dest_root).unwrap();

        install_one_skill("base-help", Some(&src), &dest_root, "0.13.2").unwrap();
        let msg = install_one_skill("base-help", Some(&src), &dest_root, "0.13.2").unwrap();
        assert!(msg.contains("already current"), "got: {msg}");
        let backups: Vec<_> = std::fs::read_dir(&dest_root)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".bak-"))
            .collect();
        assert!(backups.is_empty(), "an unchanged reinstall must not leave backups");
    }

    /// base-help appends to its own Q&A bank as it learns. A reinstall must not
    /// discard those pairs — they get preserved beside the fresh copy.
    #[test]
    fn install_one_skill_backs_up_operator_edits() {
        let tmp = tempfile::tempdir().unwrap();
        let src = make_skill_src(&tmp.path().join("repo"), "shipped pairs\n");
        let dest_root = tmp.path().join("skills");
        std::fs::create_dir_all(&dest_root).unwrap();

        install_one_skill("base-help", Some(&src), &dest_root, "0.13.2").unwrap();
        // Operator's own appended pair.
        let installed_bank = dest_root.join("base-help").join("references").join("qa.md");
        std::fs::write(&installed_bank, "shipped pairs\n### Q: mine\n").unwrap();

        let msg = install_one_skill("base-help", Some(&src), &dest_root, "0.13.2").unwrap();
        assert!(msg.contains("previous kept at"), "got: {msg}");

        let backup = std::fs::read_dir(&dest_root)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_name().to_string_lossy().contains(".bak-"))
            .expect("edited skill must be backed up");
        let saved =
            std::fs::read_to_string(backup.path().join("references").join("qa.md")).unwrap();
        assert!(saved.contains("### Q: mine"), "operator's pair must survive");
        // And the fresh copy is in place.
        assert_eq!(std::fs::read_to_string(&installed_bank).unwrap(), "shipped pairs\n");
    }

    #[test]
    fn install_one_skill_leaves_nothing_behind_when_source_is_bad() {
        let tmp = tempfile::tempdir().unwrap();
        let dest_root = tmp.path().join("skills");
        std::fs::create_dir_all(&dest_root).unwrap();
        let missing = tmp.path().join("nope");

        assert!(install_one_skill("base-help", Some(&missing), &dest_root, "0.13.2").is_err());
        let leftovers: Vec<_> = std::fs::read_dir(&dest_root).unwrap().filter_map(|e| e.ok()).collect();
        assert!(leftovers.is_empty(), "a failed install must not leave staging dirs");
    }

    // The starter pack ships embedded, so a TOML typo would only surface on a
    // stranger's first install. Parse it here instead.
    #[test]
    fn starter_commands_pack_is_valid_and_complete() {
        let cmds: Vec<crate::command::CommandDef> =
            toml::from_str::<toml::Value>(super::STARTER_COMMANDS)
                .expect("starter-commands.toml must parse")
                .get("command")
                .cloned()
                .expect("must define [[command]] entries")
                .try_into()
                .expect("entries must match the CommandDef schema");

        let names: Vec<&str> = cmds.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["handoff", "fork", "base", "end"]);

        for c in &cmds {
            assert!(!c.description.is_empty(), "*{} needs a description", c.name);
            assert!(c.rules.len() >= 4, "*{} is too thin to be useful", c.name);
            assert!(
                c.rules.iter().all(|r| !r.trim().is_empty()),
                "*{} has an empty rule",
                c.name
            );
        }

        // Issue #8: the shipped commands must never teach the global fallback.
        let handoff = cmds.iter().find(|c| c.name == "handoff").unwrap();
        let body = handoff.rules.join(" ");
        assert!(body.contains("base scaffold"), "handoff must be scaffold-first");
        assert!(
            body.contains("never write a handoff to the global tier"),
            "handoff must forbid the global fallback"
        );
    }
}

#[cfg(test)]
mod hooks_manifest_tests {
    use super::*;

    /// Wire a fresh settings.json and hand back what landed under `hooks`.
    fn wired() -> serde_json::Map<String, serde_json::Value> {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join("settings.json");
        std::fs::write(&settings, "{}").unwrap();
        wire_hooks(&settings).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        v["hooks"].as_object().expect("hooks object").clone()
    }

    /// The property the whole verb exists for.
    ///
    /// The app merges `entry` verbatim. If it were assembled separately from
    /// what `wire_hooks` writes, the two would drift and the drift would surface
    /// as hooks that look installed and never fire — silently, because a hook
    /// that is not called reports nothing.
    #[test]
    fn the_manifest_publishes_exactly_what_the_installer_writes() {
        let hooks = wired();
        let manifest = hooks_manifest();

        for h in manifest["hooks"].as_array().unwrap() {
            let event = h["event"].as_str().unwrap();
            let installed = hooks[event].as_array().unwrap();
            assert_eq!(installed.len(), 1, "{event}: one entry per fresh wire");
            assert_eq!(
                installed[0], h["entry"],
                "{event}: the manifest must publish the value the installer writes"
            );
        }
    }

    #[test]
    fn the_manifest_covers_every_hook_and_invents_none() {
        let manifest = hooks_manifest();
        let listed: Vec<&str> =
            manifest["hooks"].as_array().unwrap().iter().map(|h| h["event"].as_str().unwrap()).collect();
        let expected: Vec<&str> = HOOK_TABLE.iter().map(|(e, _)| *e).collect();
        assert_eq!(listed, expected, "manifest and table must not diverge");

        // A consumer keys off these; renaming one silently unwires that hook.
        assert_eq!(manifest["version"], serde_json::json!(1));
        assert_eq!(manifest["settings_key"], serde_json::json!("hooks"));
    }

    /// The skip rule the manifest documents is the one the installer applies.
    #[test]
    fn wiring_twice_adds_nothing_which_is_the_rule_the_manifest_states() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join("settings.json");
        std::fs::write(&settings, "{}").unwrap();

        wire_hooks(&settings).unwrap();
        let once = std::fs::read_to_string(&settings).unwrap();
        wire_hooks(&settings).unwrap();
        let twice = std::fs::read_to_string(&settings).unwrap();

        assert_eq!(once, twice, "an installer that re-runs must not duplicate hooks");
        assert!(
            manifest_merge_rule().contains("skip when an entry with the same `command`"),
            "the manifest must state the rule the installer actually follows"
        );
    }

    /// A host config that already carries unrelated hooks keeps them.
    #[test]
    fn wiring_preserves_hooks_base_did_not_write() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join("settings.json");
        std::fs::write(
            &settings,
            r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"someone-elses-tool"}]}]}}"#,
        )
        .unwrap();

        wire_hooks(&settings).unwrap();

        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        let arr = v["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(arr.len(), 2, "base appends, never replaces");
        assert!(
            arr.iter().any(|e| e["hooks"][0]["command"] == serde_json::json!("someone-elses-tool")),
            "the pre-existing hook must survive"
        );
    }
}

#[cfg(test)]
mod skill_refresh_tests {
    use super::*;

    /// The bug this whole change exists to prevent: during `base update` the
    /// running process is the OUTGOING binary, so a tag compiled in from
    /// `CARGO_PKG_VERSION` fetches the skill for the version being replaced.
    /// The tag must follow the version passed in.
    #[test]
    fn fetch_tag_follows_the_installed_version_not_the_running_one() {
        assert_eq!(skill_tag("0.13.3"), "v0.13.3");
        assert_ne!(
            skill_tag("0.13.3"),
            format!("v{}", env!("CARGO_PKG_VERSION")),
            "a tag equal to the running version would reinstall the outgoing skill"
        );
    }

    /// Release tags arrive both ways depending on the source (`binary_version`
    /// yields a bare version, the GitHub release yields a `v`-prefixed tag).
    #[test]
    fn skill_tag_is_idempotent_over_the_v_prefix() {
        assert_eq!(skill_tag("v0.13.3"), "v0.13.3");
        assert_eq!(skill_tag("  0.13.3 "), "v0.13.3");
    }

    /// `base install` resolves a local checkout; `base update` must not, because
    /// `find_local_skills_dir` falls back to `current_dir()` and the operator is
    /// explicitly installing a release.
    #[test]
    fn tag_only_never_resolves_a_local_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let skills = dir.path().join("claude").join("skills").join("base-help");
        std::fs::create_dir_all(&skills).unwrap();
        std::fs::write(skills.join("SKILL.md"), "---\nname: base-help\n---\n").unwrap();

        let binary = dir.path().join("target").join("release").join("base");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();

        // LocalThenTag finds it; TagOnly is what the update path passes.
        assert!(
            find_local_skills_dir(&binary).is_some(),
            "fixture must be discoverable, or this test proves nothing"
        );
        assert_eq!(
            match SkillSource::TagOnly {
                SkillSource::LocalThenTag => find_local_skills_dir(&binary),
                SkillSource::TagOnly => None,
            },
            None,
            "TagOnly must ignore a local checkout"
        );
    }
}

#[cfg(test)]
mod skill_dod_tests {
    use super::*;

    /// Build a skill fixture: SKILL.md plus a bank with the given stamp.
    fn fixture(root: &Path, stamp: &str, extra_pair: &str) -> std::path::PathBuf {
        let skill = root.join("base-help");
        let refs = skill.join("references");
        std::fs::create_dir_all(&refs).unwrap();
        std::fs::write(skill.join("SKILL.md"), "---\nname: base-help\n---\n").unwrap();
        std::fs::write(
            refs.join("qa.md"),
            format!("**Verified against base v{stamp} on 2026-08-19.**\n{extra_pair}"),
        )
        .unwrap();
        skill
    }

    fn bank(dest_root: &Path) -> String {
        std::fs::read_to_string(dest_root.join("base-help").join("references").join("qa.md"))
            .unwrap()
    }

    /// DoD line 1, refresh half: installing over an older skill replaces it with
    /// the incoming tag's content.
    #[test]
    fn update_refreshes_the_skill_to_the_incoming_content() {
        let tmp = tempfile::tempdir().unwrap();
        let dest_root = tmp.path().join("skills");
        std::fs::create_dir_all(&dest_root).unwrap();

        let old = fixture(&tmp.path().join("old"), "0.12.3", "");
        install_one_skill("base-help", Some(&old), &dest_root, "0.12.3").unwrap();
        assert!(bank(&dest_root).contains("v0.12.3"));

        let new = fixture(&tmp.path().join("new"), "0.13.2", "### Q: a new pair\n");
        let msg = install_one_skill("base-help", Some(&new), &dest_root, "0.13.2").unwrap();

        let after = bank(&dest_root);
        assert!(after.contains("v0.13.2"), "refreshed to the incoming bank");
        assert!(after.contains("a new pair"), "new content is present");
        assert!(msg.contains("previous kept at"), "outcome names the backup: {msg}");
    }

    /// An unchanged skill is left alone entirely — no churn, no backup dir.
    #[test]
    fn an_unchanged_skill_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let dest_root = tmp.path().join("skills");
        std::fs::create_dir_all(&dest_root).unwrap();
        let src = fixture(&tmp.path().join("src"), "0.13.2", "");

        install_one_skill("base-help", Some(&src), &dest_root, "0.13.2").unwrap();
        let msg = install_one_skill("base-help", Some(&src), &dest_root, "0.13.2").unwrap();

        assert!(msg.contains("already current"), "got: {msg}");
        let backups: Vec<_> = std::fs::read_dir(&dest_root)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".bak-"))
            .collect();
        assert!(backups.is_empty(), "an identical skill must not spawn a backup");
    }

    /// DoD line 2: a modified bank is never destroyed. The operator's bytes
    /// survive in full, and the outcome line says so rather than staying quiet
    /// about it — `base update`'s background path has no other reader.
    #[test]
    fn a_modified_bank_is_preserved_and_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let dest_root = tmp.path().join("skills");
        std::fs::create_dir_all(&dest_root).unwrap();

        let src = fixture(&tmp.path().join("src"), "0.12.3", "");
        install_one_skill("base-help", Some(&src), &dest_root, "0.12.3").unwrap();

        // The operator appends a pair, exactly as the skill's close-the-loop
        // rule instructs.
        let live = dest_root.join("base-help").join("references").join("qa.md");
        let mine = format!("{}\n### Q: my own hard-won pair\n", std::fs::read_to_string(&live).unwrap());
        std::fs::write(&live, &mine).unwrap();

        let new = fixture(&tmp.path().join("new"), "0.13.2", "");
        let msg = install_one_skill("base-help", Some(&new), &dest_root, "0.13.2").unwrap();

        // The incoming bank is live …
        assert!(bank(&dest_root).contains("v0.13.2"));
        // … and the operator's copy survives byte-for-byte in the backup.
        let backup = std::fs::read_dir(&dest_root)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.file_name().unwrap().to_string_lossy().contains(".bak-"))
            .expect("a backup must exist");
        assert_eq!(
            std::fs::read_to_string(backup.join("references").join("qa.md")).unwrap(),
            mine,
            "the operator's edited bank must survive byte-for-byte"
        );
        assert!(msg.contains("previous kept at"), "never silent: {msg}");
    }
}
