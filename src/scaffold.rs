use std::path::Path;

use anyhow::{Context, Result};

use crate::config::BaseConfig;

/// Scaffold a new workspace: create .base/, write starter configs, register globally.
pub fn run(target: &Path) -> Result<()> {
    println!("═══════════════════════════════════════");
    println!("BASE — Scaffold Workspace");
    println!("═══════════════════════════════════════\n");

    let base_dir = target.join(".base");

    // Step 1: Create .base/
    print!("1. Create .base/ ... ");
    if base_dir.exists() {
        println!("exists (preserved)");
    } else {
        std::fs::create_dir_all(&base_dir)
            .with_context(|| format!("Creating {}", base_dir.display()))?;
        println!("✓");
    }

    // Step 2: Write starter domains.toml
    let domains_path = base_dir.join("domains.toml");
    print!("2. Workspace domains.toml ... ");
    if domains_path.exists() {
        println!("exists (preserved)");
    } else {
        let ws_name = target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workspace");
        std::fs::write(
            &domains_path,
            format!(
                r#"# BASE — Workspace domain triggers for {ws_name}
# Built by Chris Kahler · Chris AI Systems
# Community: https://chrisai.cv/skool
#
# Workspace-specific triggers. Rules live in the graph (base rule add/list/remove).
# Global domains in ~/.base-gbl/domains.toml apply everywhere.

# Example:
# [[domain]]
# name = "MY-PROJECT"
# mode = "triggered"
# prompt_keywords = ["my project", "working on my-project"]
# paths = ["src/my-project/"]
"#
            ),
        )?;
        println!("✓");
    }

    // Step 3: Write starter base.toml (workspace overrides)
    let config_path = base_dir.join("base.toml");
    print!("3. Workspace base.toml ... ");
    if config_path.exists() {
        println!("exists (preserved)");
    } else {
        std::fs::write(
            &config_path,
            r#"# BASE — Workspace config overrides
# Inherits from ~/.base-gbl/base.toml. Only set what you want to override.

# [namespace]
# prefix = "ops"
# uri = "http://ops-sys.local/ontology#"

# [devmode]
# enabled = true
"#,
        )?;
        println!("✓");
    }

    // Step 4: Register in ~/.base-gbl/base.toml
    print!("4. Register workspace ... ");
    let target_str = target
        .canonicalize()
        .unwrap_or_else(|_| target.to_path_buf())
        .to_string_lossy()
        .to_string();
    register_workspace(&target_str)?;

    // Step 4b: Mirror the registry into ~/.claude/CLAUDE.md so Claude is auto-aware
    // of every registered workspace without anyone having to remember to edit it.
    print!("   ↳ sync CLAUDE.md registry ... ");
    match sync_claude_md_registry() {
        Ok(n) => println!("✓ ({n} workspaces)"),
        Err(e) => println!("⊘ ({e})"),
    }

    // Step 5: Initialize graph
    print!("5. Sync domains to graph ... ");
    let config = BaseConfig::load(target);
    match crate::domain::sync::sync_domains_to_graph(&config, target, None) {
        Ok(stats) => println!("✓ ({} domains, {} rules)", stats.domains, stats.rules),
        Err(_) => println!("✓ (empty graph initialized)"),
    }

    println!("\n═══════════════════════════════════════");
    println!("✓ Workspace scaffolded");
    println!("═══════════════════════════════════════\n");
    println!("  Path: {}", target.display());
    println!("  Config: .base/base.toml");
    println!("  Domains: .base/domains.toml");
    println!("  Graph: .base/graph.nq\n");
    println!("Next:");
    println!("  Add workspace-specific domain triggers to .base/domains.toml");
    println!("  Add rules: base rule add --domain MY-DOMAIN --text \"...\"");

    println!("\n───────────────────────────────────────");
    println!("BASE — Built by Chris Kahler");
    println!("Chris AI Systems");
    println!();
    println!("Community & support:");
    println!("  https://chrisai.cv/skool");
    println!("───────────────────────────────────────");

    Ok(())
}

const WS_START: &str =
    "<!-- BASE:WORKSPACES:START — auto-generated from ~/.base-gbl/base.toml; do not edit between markers -->";
const WS_END: &str = "<!-- BASE:WORKSPACES:END -->";

/// Read `[[workspace]]` paths from the global registry (`~/.base-gbl/base.toml`).
fn registered_workspace_paths() -> Vec<String> {
    #[derive(serde::Deserialize)]
    struct WsOnly { #[serde(default)] workspace: Vec<WsPath> }
    #[derive(serde::Deserialize)]
    struct WsPath { path: String }

    let Some(home) = dirs::home_dir() else { return Vec::new() };
    let toml_path = home.join(".base-gbl").join("base.toml");
    let Ok(content) = std::fs::read_to_string(&toml_path) else { return Vec::new() };
    toml::from_str::<WsOnly>(&content)
        .map(|w| w.workspace.into_iter().map(|e| e.path).collect())
        .unwrap_or_default()
}

fn tildify(path: &str, home: &Path) -> String {
    let h = home.to_string_lossy();
    path.strip_prefix(h.as_ref())
        .map(|rest| format!("~{rest}"))
        .unwrap_or_else(|| path.to_string())
}

fn build_workspace_block(paths: &[String], home: &Path) -> String {
    let mut s = String::from(WS_START);
    s.push_str("\n## Registered Workspaces (auto — synced from base.toml)\n\n");
    s.push_str(
        "base tracks these workspaces; folder resolution is registry-aware across all of \
         them. Before treating any project or folder as missing/\"gone\", check this list — \
         it may have moved to another registered workspace (e.g. shipping products → \
         `ops-sys/toolbox`). Regenerated by `base scaffold` and `base workspace sync`; \
         curated detail lives in the §11 table above.\n\n",
    );
    if paths.is_empty() {
        s.push_str("- (none registered)\n");
    } else {
        for p in paths {
            s.push_str(&format!("- `{}`\n", tildify(p, home)));
        }
    }
    s.push_str(WS_END);
    s
}

/// Splice the managed block into `content`: replace between markers if present
/// (preserving everything outside verbatim), else append a new section.
fn splice_workspace_block(content: &str, block: &str) -> String {
    if let (Some(si), Some(ei)) = (content.find("BASE:WORKSPACES:START"), content.find("BASE:WORKSPACES:END")) {
        let block_start = content[..si].rfind('\n').map(|n| n + 1).unwrap_or(0);
        let after = ei + "BASE:WORKSPACES:END".len();
        let line_end = content[after..].find('\n').map(|n| after + n).unwrap_or(content.len());
        let mut out = String::with_capacity(content.len() + block.len());
        out.push_str(&content[..block_start]);
        out.push_str(block);
        out.push_str(&content[line_end..]);
        return out;
    }
    let mut out = content.to_string();
    if !out.ends_with('\n') { out.push('\n'); }
    out.push_str("\n");
    out.push_str(block);
    out.push('\n');
    out
}

/// Regenerate the managed registered-workspaces block in `~/.claude/CLAUDE.md` from
/// the global registry. Idempotent; only ever rewrites between the markers. No-op if
/// CLAUDE.md is absent. Returns the number of workspaces written.
pub fn sync_claude_md_registry() -> Result<usize> {
    let home = dirs::home_dir().context("Cannot determine home directory")?;
    let claude_md = home.join(".claude").join("CLAUDE.md");
    if !claude_md.exists() {
        return Ok(0);
    }
    let paths = registered_workspace_paths();
    let content = std::fs::read_to_string(&claude_md)?;
    let block = build_workspace_block(&paths, &home);
    let updated = splice_workspace_block(&content, &block);
    if updated != content {
        let tmp = claude_md.with_extension("md.tmp");
        std::fs::write(&tmp, &updated)?;
        std::fs::rename(&tmp, &claude_md)?;
    }
    Ok(paths.len())
}

/// Add a workspace path to ~/.base-gbl/base.toml [[workspace]] if not already present.
fn register_workspace(path: &str) -> Result<()> {
    let home = dirs::home_dir().context("Cannot determine home directory")?;
    let global_toml = home.join(".base-gbl").join("base.toml");

    if !global_toml.exists() {
        println!("⊘ no ~/.base-gbl/base.toml (run base install first)");
        return Ok(());
    }

    let content = std::fs::read_to_string(&global_toml)?;

    // Check if already registered — exact path match on the toml value,
    // not substring (contains() would falsely match "/a/project" against
    // "/a/project-v2"; AUDIT Q11).
    let path_line = format!("path = \"{path}\"");
    if content.lines().any(|l| l.trim() == path_line) {
        println!("already registered");
        return Ok(());
    }

    // Append [[workspace]] entry
    let entry = format!("\n[[workspace]]\npath = \"{path}\"\n");
    let mut new_content = content;
    new_content.push_str(&entry);

    let tmp = global_toml.with_extension("toml.tmp");
    std::fs::write(&tmp, &new_content)?;
    std::fs::rename(&tmp, &global_toml)?;

    println!("✓ (added to ~/.base-gbl/base.toml)");
    Ok(())
}
