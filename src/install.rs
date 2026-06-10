use std::path::Path;

use anyhow::{Context, Result};

use crate::config::BaseConfig;
use crate::manifest::{self, Manifest};

/// Run the full install process: build, symlink, create global tier, wire hooks, write manifest.
pub fn run(carl_json_path: Option<&Path>, skip_hooks: bool, full: bool) -> Result<()> {
    let home = dirs::home_dir().context("Cannot determine home directory")?;
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
    println!("  https://chrisai.cv/skool");
    println!();
    println!("Tutorials:");
    println!("  https://www.youtube.com/@chris-ai-systems");
    println!("───────────────────────────────────────");

    Ok(())
}

// ─── Uninstall ──────────────────────────────────────────────

/// Remove base hooks from settings.json, remove binary, strip CLAUDE.md section.
/// With --purge, also removes ~/.base-gbl/ global tier.
pub fn uninstall(purge: bool) -> Result<()> {
    let home = dirs::home_dir().context("Cannot determine home directory")?;

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

    // 4. Purge global tier if requested
    if purge {
        let global_dir = home.join(".base-gbl");
        if global_dir.exists() {
            print!("4. Purge global tier ... ");
            std::fs::remove_dir_all(&global_dir)?;
            println!("✓ removed {}", global_dir.display());
        }
    } else {
        println!("4. Global tier preserved (~/.base-gbl/) — use --purge to remove");
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

    // Remove existing binary
    if dest.exists() {
        std::fs::remove_file(dest)
            .with_context(|| format!("Removing existing {}", dest.display()))?;
    }

    // Copy binary (not symlink — this is a shippable install)
    std::fs::copy(binary, dest)
        .with_context(|| format!("Copying {} → {}", binary.display(), dest.display()))?;

    // Set executable permission on unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755))?;
    }

    println!("✓");
    Ok(())
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
# Community: https://chrisai.cv/skool

[namespace]
prefix = "ops"
uri = "http://ops-sys.local/ontology#"

[devmode]
enabled = true

[bracket]
enabled = true
fresh_until = 3
moderate_until = 10
depleted_until = 20
refresh_interval = 5

[signal]
enabled = true
max_chars = 2000
stale_days = 14

[sync]
include = ["**/*.md", "**/paul.json"]
exclude = ["node_modules/", "target/", ".git/", ".base/"]
"#,
        )?;
        println!("✓ (created base.toml)");
    } else {
        println!("✓ (base.toml exists, preserved)");
    }

    // domains.toml — only create if missing
    let domains_toml = global_dir.join("domains.toml");
    if !domains_toml.exists() {
        std::fs::write(
            &domains_toml,
            r#"# BASE — Domain configuration
# Built by Chris Kahler · Chris AI Systems
# Community: https://chrisai.cv/skool
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

    // docs/markdown-ontology-protocol.md — bundled MOP spec
    let docs_dir = global_dir.join("docs");
    std::fs::create_dir_all(&docs_dir)?;
    let mop_path = docs_dir.join("markdown-ontology-protocol.md");
    if !mop_path.exists() {
        std::fs::write(&mop_path, include_str!("../docs/markdown-ontology-protocol.md"))?;
        println!("   Created docs/markdown-ontology-protocol.md");
    }

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
# Community: https://chrisai.cv/skool
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
# React to file changes after tool execution.
#
# [[hooks.post_tool.handlers]]
# pattern = "data.json"
# action = "reingest"          # "reingest" | "log" | "query"
"#,
        )?;
        println!("   Created extensions/ + _template.toml");
    } else {
        println!("   extensions/ exists, preserved");
    }

    Ok(())
}

// ─── Step 3: Wire hooks ─────────────────────────────────────

fn wire_hooks(settings_path: &Path) -> Result<()> {
    print!("3. Wire hooks → {} ... ", settings_path.display());

    if !settings_path.exists() {
        println!("⊘ settings.json not found, skipped");
        return Ok(());
    }

    let content = std::fs::read_to_string(settings_path)?;
    let mut settings: serde_json::Value = serde_json::from_str(&content)
        .context("Failed to parse settings.json")?;

    // All hooks BASE needs wired
    let hook_entries = [
        ("SessionStart", "base hook session-start"),
        ("UserPromptSubmit", "base hook user-prompt-submit"),
        ("PreToolUse", "base hook pre-tool-use"),
        ("PostToolUse", "base hook post-tool-use"),
    ];

    // Check if already fully wired (all 4 present)
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

        arr.push(serde_json::json!({
            "hooks": [
                {
                    "type": "command",
                    "command": command
                }
            ]
        }));

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
        println!("⊘ scripts/ast/ not found near binary — skipped");
        println!("   Copy scripts/ast/ to {} manually", scripts_dest.display());
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
    Ok(())
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
| Navigate code: find functions, callers, imports | `base ast query --contains "name"` or `--file`, `--calls`, `--imports` |
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

**Order of operations:** `base ast query` first → understand structure → `Read` specific lines only. Never scan-then-understand when you can understand-then-read.

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
- Built by Chris Kahler · Chris AI Systems · https://chrisai.cv/skool
"#;

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
    };
    manifest.components.insert("base".to_string(), base_entry);

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
