use std::path::Path;

use serde::{Deserialize, Serialize};

// ─── Star command schema ────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CommandDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub rules: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CommandsFile {
    #[serde(default)]
    command: Vec<CommandDef>,
}

// ─── Loading (tiered: global → workspace) ───────────────────

/// Load commands: global ~/.base-gbl/commands.toml → workspace .base/commands.toml.
/// Returns empty Vec if neither exists.
pub fn load_commands(cwd: &Path) -> Vec<CommandDef> {
    let mut commands = Vec::new();

    // Global
    if let Some(home) = dirs::home_dir()
        && let Ok(content) =
            std::fs::read_to_string(home.join(".base-gbl").join("commands.toml"))
        && let Ok(file) = toml::from_str::<CommandsFile>(&content)
    {
        commands = file.command;
    }

    // Workspace (overlays global by name)
    if let Some(base_dir) = crate::config::find_workspace_base(cwd)
        && let Ok(content) = std::fs::read_to_string(base_dir.join("commands.toml"))
        && let Ok(file) = toml::from_str::<CommandsFile>(&content)
    {
        commands = merge_commands(commands, file.command);
    }

    commands
}

fn merge_commands(base: Vec<CommandDef>, overlay: Vec<CommandDef>) -> Vec<CommandDef> {
    let mut merged = base;
    for oc in overlay {
        if let Some(pos) = merged.iter().position(|c| c.name.eq_ignore_ascii_case(&oc.name)) {
            merged[pos] = oc;
        } else {
            merged.push(oc);
        }
    }
    merged
}

// ─── Matching ───────────────────────────────────────────────

/// Check if prompt starts with a *COMMAND. Returns the matched command if found.
/// Matching is case-insensitive: *BRIEF, *brief, *Brief all match.
pub fn match_command<'a>(prompt: &str, commands: &'a [CommandDef]) -> Option<&'a CommandDef> {
    let trimmed = prompt.trim();

    // Must start with *
    if !trimmed.starts_with('*') {
        return None;
    }

    // Extract command name (first word after *)
    let cmd_text = &trimmed[1..];
    let cmd_name = cmd_text.split_whitespace().next()?;

    commands
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(cmd_name))
}

/// Format command rules for injection.
pub fn format_command_output(cmd: &CommandDef) -> String {
    if cmd.rules.is_empty() {
        return String::new();
    }

    let mut out = format!("[*{} ACTIVATED]\n", cmd.name.to_uppercase());
    if !cmd.description.is_empty() {
        out.push_str(&format!("{}\n\n", cmd.description));
    }
    for (i, rule) in cmd.rules.iter().enumerate() {
        out.push_str(&format!("  {i}. {rule}\n"));
    }
    out
}
