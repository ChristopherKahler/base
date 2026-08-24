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
    if let Some(home) = crate::home::home_root()
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

// ─── Diagnostics (the `base doctor` seam) ───────────────────
//
// `load_commands` above deliberately discards parse errors: hooks fail open, so
// a malformed commands.toml must never block a session. The cost is that a
// corrupt file is indistinguishable from an absent one — "No commands
// configured" with no explanation. These functions are the one place that tells
// the truth, and only `base doctor` calls them.

/// Check a single commands.toml. PURE: only touches `path`, so it is
/// deterministic under unit test. A missing or empty file is not a fault —
/// only a file that exists with content the loader cannot parse, because that
/// is the case that silently deactivates every star command in the tier.
pub fn check_command_file(tier: &str, path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    if content.trim().is_empty() {
        return None;
    }
    match toml::from_str::<CommandsFile>(&content) {
        Ok(_) => None,
        Err(e) => Some(format!(
            "{tier} commands.toml is not valid TOML — every star command in this tier is \
             silently inactive: {e} [{}]",
            path.display()
        )),
    }
}

/// Every commands.toml `load_commands` would read, in the same tier order.
pub fn command_file_paths(cwd: &Path) -> Vec<(String, std::path::PathBuf)> {
    let mut paths = Vec::new();
    if let Some(home) = crate::home::home_root() {
        paths.push((
            "global".to_string(),
            home.join(".base-gbl").join("commands.toml"),
        ));
    }
    if let Some(base_dir) = crate::config::find_workspace_base(cwd) {
        paths.push(("workspace".to_string(), base_dir.join("commands.toml")));
    }
    paths
}

/// Faults across every tier the loader reads. Empty when nothing is wrong.
pub fn check_command_files(cwd: &Path) -> Vec<String> {
    command_file_paths(cwd)
        .iter()
        .filter_map(|(tier, path)| check_command_file(tier, path))
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[derive(serde::Deserialize, serde::Serialize)]
    struct CmdFileRt {
        #[serde(default)]
        command: Vec<CommandDef>,
    }

    fn write(dir: &Path, contents: &str) -> std::path::PathBuf {
        let p = dir.join("commands.toml");
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        p
    }

    #[test]
    fn check_command_file_passes_valid_toml() {
        let d = tempfile::tempdir().unwrap();
        let p = write(
            d.path(),
            "[[command]]\nname = \"AUDIT\"\ndescription = \"d\"\nrules = [\"one\"]\n",
        );
        assert_eq!(check_command_file("global", &p), None);
    }

    #[test]
    fn check_command_file_ignores_missing_and_empty() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(check_command_file("global", &d.path().join("nope.toml")), None);
        let p = write(d.path(), "   \n\n");
        assert_eq!(check_command_file("global", &p), None);
    }

    #[test]
    fn check_command_file_reports_corrupt_toml() {
        let d = tempfile::tempdir().unwrap();
        // Exactly what the old hand-rolled emitter produced: a raw newline inside a
        // basic string, which terminates the line and invalidates the document.
        let p = write(
            d.path(),
            "[[command]]\nname = \"AUDIT\"\nrules = [\n  \"line one\nline two\",\n]\n",
        );
        let fault = check_command_file("global", &p).expect("corrupt file must be reported");
        assert!(fault.contains("global"));
        assert!(fault.contains("silently inactive"));
    }

    /// The regression behind issue #5: rules carrying newlines, tabs, and
    /// backslashes must survive serialize → write → read. The old emitter escaped
    /// only double quotes and produced a file that could never be parsed back.
    #[test]
    fn control_characters_in_rules_round_trip() {
        let original = CmdFileRt {
            command: vec![CommandDef {
                name: "AUDIT".into(),
                description: "has \"quotes\" and \\ backslash".into(),
                rules: vec![
                    "line one\nline two\nline three".into(),
                    "tabbed\tvalue".into(),
                    "windows\\path\\here".into(),
                    "trailing quote \"".into(),
                ],
            }],
        };
        let serialized = toml::to_string_pretty(&original).unwrap();
        let parsed: CmdFileRt = toml::from_str(&serialized).expect("must round-trip");
        assert_eq!(parsed.command.len(), 1);
        assert_eq!(parsed.command[0].rules, original.command[0].rules);
        assert_eq!(parsed.command[0].description, original.command[0].description);
    }

    /// A file the loader silently skips must be the same file doctor reports.
    /// This is the whole contract: fail open in the hot path, tell the truth here.
    #[test]
    fn loader_skips_exactly_what_doctor_reports() {
        let d = tempfile::tempdir().unwrap();
        let p = write(
            d.path(),
            "[[command]]\nname = \"AUDIT\"\nrules = [\n  \"line one\nline two\",\n]\n",
        );
        let content = std::fs::read_to_string(&p).unwrap();
        assert!(
            toml::from_str::<CommandsFile>(&content).is_err(),
            "loader must be unable to parse this file"
        );
        assert!(
            check_command_file("workspace", &p).is_some(),
            "doctor must report the file the loader could not parse"
        );
    }
}
