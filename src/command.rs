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

/// Find every *COMMAND token anywhere in the prompt and return the matching
/// commands, in first-seen order, deduped. Composition is the point: stacking
/// two modes in one prompt activates both — "*audit *steelman review this" →
/// [AUDIT, STEELMAN]. Matching is case-insensitive and tolerant of trailing
/// punctuation (*blunt, → BLUNT).
pub fn match_commands<'a>(prompt: &str, commands: &'a [CommandDef]) -> Vec<&'a CommandDef> {
    let mut matched: Vec<&CommandDef> = Vec::new();
    for token in prompt.split_whitespace() {
        let Some(rest) = token.strip_prefix('*') else { continue };
        // Tolerate trailing punctuation: "*blunt," / "*audit." still match.
        let name = rest.trim_end_matches(|c: char| !c.is_alphanumeric());
        if name.is_empty() {
            continue;
        }
        if let Some(cmd) = commands.iter().find(|c| c.name.eq_ignore_ascii_case(name))
            && !matched.iter().any(|m| m.name.eq_ignore_ascii_case(&cmd.name))
        {
            matched.push(cmd);
        }
    }
    matched
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
