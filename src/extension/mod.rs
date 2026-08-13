pub mod ingest;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ─── Extension contract types ────────────────────────────────

/// Top-level container matching the TOML structure:
/// ```toml
/// [extension]
/// name = "..."
/// [hooks.session_start]
/// ...
/// ```
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExtensionFile {
    pub extension: ExtensionMeta,
    #[serde(default)]
    pub hooks: Option<ExtensionHooks>,
    /// Drop-in CLI commands this extension contributes (v0.6). Each becomes a
    /// `base <name>` subcommand routed to `handler` via the external-subcommand
    /// seam. See `crate::plugin` for the registry + dispatch.
    #[serde(default)]
    pub commands: Vec<CommandSpec>,
    /// Optional cross-platform distribution descriptor (Plugin Distribution P1).
    /// Absent ⇒ today's local/`--bundle` behavior, unchanged. Present ⇒ P2's
    /// platform-aware fetch can pull the compiled binary from a GitHub release.
    #[serde(default)]
    pub dist: Option<DistSpec>,
}

/// The `[dist]` table — how to fetch this plugin's compiled binary per-OS.
///
/// ```toml
/// [dist]
/// repo    = "ChristopherKahler/meta-cli"   # GitHub owner/repo holding releases
/// version = "0.1.0"                          # pinned tag, resolved as v<version>
/// binary  = "meta"                            # bin name (→ meta.exe on Windows)
/// ```
///
/// Asset naming reuses base's own scheme: `<binary>-<os>-<arch>.<ext>` —
/// `<binary>-linux-x86_64.tar.gz` / `<binary>-darwin-aarch64.tar.gz` /
/// `<binary>-windows-x86_64.zip`. Parsed in P1; consumed by P2 (fetch).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct DistSpec {
    pub repo: String,
    pub version: String,
    pub binary: String,
}

/// A single `[[commands]]` entry — a drop-in `base <name>` subcommand.
///
/// ```toml
/// [[commands]]
/// name = "nano-banana"
/// handler = "bin/nano-banana.mjs"   # executable (shebang'd script or binary)
/// description = "Generate/edit images via the Gemini image API"
/// usage = "base nano-banana generate --prompt \"...\" [--ratio 16:9] [--json]"
/// ```
///
/// `handler` resolution: tilde-expanded; absolute used as-is; relative resolved
/// against `framework_dir` (then the extension file's own directory). The
/// handler MUST be directly executable — a shebang'd script (`#!/usr/bin/env
/// node`, `+x`) or a binary — so base can exec it without knowing the language.
///
/// `deny_unknown_fields` below is load-bearing, not tidiness. `state_dir` written
/// one line under a `[[commands]]` header is bound by TOML to the command, not to
/// `[extension]` — and was silently dropped, so ingest fell back to cwd, found no
/// files, and reported nothing. A manifest that declares something base ignores
/// must fail loudly at parse time rather than degrade into a no-op.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommandSpec {
    pub name: String,
    pub handler: String,
    pub description: String,
    #[serde(default)]
    pub usage: Option<String>,
}

/// The `[extension]` table — identity + optional paths.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExtensionMeta {
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(default)]
    pub framework_dir: Option<String>,
    #[serde(default)]
    pub state_dir: Option<String>,
}

/// Resolved extension ready for use — flattened from ExtensionFile.
#[derive(Debug, Clone, Serialize)]
pub struct ExtensionDef {
    pub name: String,
    pub version: String,
    pub description: String,
    pub framework_dir: Option<String>,
    pub state_dir: Option<String>,
    pub hooks: Option<ExtensionHooks>,
    /// Drop-in CLI commands contributed by this extension (v0.6).
    #[serde(default)]
    pub commands: Vec<CommandSpec>,
    /// Cross-platform distribution descriptor (Plugin Distribution P1), carried
    /// from the manifest so P2's fetch path can read it. `None` for local plugins.
    #[serde(default)]
    pub dist: Option<DistSpec>,
    /// The file this extension was loaded from (not serialized to TOML).
    #[serde(skip)]
    pub source_path: Option<PathBuf>,
}

impl From<ExtensionFile> for ExtensionDef {
    fn from(f: ExtensionFile) -> Self {
        Self {
            name: f.extension.name,
            version: f.extension.version,
            description: f.extension.description,
            framework_dir: f.extension.framework_dir,
            state_dir: f.extension.state_dir,
            hooks: f.hooks,
            commands: f.commands,
            dist: f.dist,
            source_path: None,
        }
    }
}

// ─── Hook binding types ──────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExtensionHooks {
    #[serde(default)]
    pub session_start: Option<SessionStartHook>,
    #[serde(default)]
    pub user_prompt: Option<UserPromptHook>,
    #[serde(default)]
    pub pre_tool: Option<PreToolHook>,
    #[serde(default)]
    pub post_tool: Option<PostToolHook>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SessionStartHook {
    #[serde(default)]
    pub queries: Vec<String>,
    #[serde(default)]
    pub ingest: Vec<IngestSource>,
    #[serde(default)]
    pub inject: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IngestSource {
    pub file: String,
    pub entity: String,
    #[serde(default = "default_strategy")]
    pub strategy: String,
}

fn default_strategy() -> String {
    "upsert".into()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UserPromptHook {
    #[serde(default)]
    pub domains: Vec<ExtensionDomain>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExtensionDomain {
    pub name: String,
    #[serde(default)]
    pub prompt_keywords: Vec<String>,
    #[serde(default)]
    pub file_keywords: Vec<String>,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub rules: Vec<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub query_format: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PreToolHook {
    #[serde(default)]
    pub triggers: Vec<PreToolTrigger>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PreToolTrigger {
    pub paths: Vec<String>,
    pub inject: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PostToolHook {
    #[serde(default)]
    pub handlers: Vec<PostToolHandler>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PostToolHandler {
    /// File-match pattern. Substring match, OR the reserved token "designset"
    /// which delegates to the built-in design/frontend file heuristic.
    pub pattern: String,
    #[serde(default = "default_action")]
    pub action: String,
    /// Message printed to stdout (so Claude sees it) when action = "inject".
    #[serde(default)]
    pub message: Option<String>,
    /// For action = "inject": fire only once per session (default true).
    #[serde(default)]
    pub once_per_session: Option<bool>,
    /// For action = "inject": only fire for these tools.
    /// Default = ["Write", "Edit", "MultiEdit"] (never Read).
    #[serde(default)]
    pub on_tools: Option<Vec<String>>,
}

fn default_action() -> String {
    "log".into()
}

// ─── Loader ──────────────────────────────────────────────────

/// Load all valid extensions from `~/.base-gbl/extensions/`.
/// Fail-open: malformed files produce stderr warnings and are skipped.
/// Duplicate names (by extension.name) are skipped with a warning.
/// Returns extensions sorted by name for deterministic ordering.
pub fn load_extensions() -> Vec<ExtensionDef> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let ext_dir = home.join(".base-gbl").join("extensions");
    load_extensions_from(&ext_dir)
}

/// Load extensions from a specific directory (for testing).
pub fn load_extensions_from(ext_dir: &Path) -> Vec<ExtensionDef> {
    if !ext_dir.is_dir() {
        return Vec::new();
    }

    let Ok(entries) = std::fs::read_dir(ext_dir) else {
        return Vec::new();
    };

    let mut extensions: Vec<ExtensionDef> = Vec::new();
    let mut seen_names: Vec<String> = Vec::new();

    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|ext| ext == "toml")
                && p.file_name()
                    .is_some_and(|n| n != "_template.toml")
        })
        .collect();
    paths.sort();

    for path in paths {
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "base: extension {}: read error: {e}",
                    path.display()
                );
                continue;
            }
        };

        let file: ExtensionFile = match toml::from_str(&content) {
            Ok(f) => f,
            Err(e) => {
                eprintln!(
                    "base: extension {}: parse error: {e}",
                    path.display()
                );
                continue;
            }
        };

        let name = &file.extension.name;
        if name.is_empty() {
            eprintln!(
                "base: extension {}: empty name — skipped",
                path.display()
            );
            continue;
        }

        if seen_names.contains(name) {
            eprintln!(
                "base: extension {}: duplicate name '{}' — skipped",
                path.display(),
                name
            );
            continue;
        }

        seen_names.push(name.clone());
        let mut ext: ExtensionDef = file.into();
        ext.source_path = Some(path);
        extensions.push(ext);
    }

    extensions.sort_by(|a, b| a.name.cmp(&b.name));
    extensions
}

// ─── Validator ───────────────────────────────────────────────

/// Validate an extension manifest file against the contract.
/// Returns the parsed ExtensionDef on success, or a list of violations on failure.
pub fn validate_extension(path: &Path) -> Result<ExtensionDef, Vec<String>> {
    let mut violations = Vec::new();

    let content = std::fs::read_to_string(path)
        .map_err(|e| vec![format!("Cannot read file: {e}")])?;

    let file: ExtensionFile = toml::from_str(&content)
        .map_err(|e| vec![format!("TOML parse error: {e}")])?;

    let meta = &file.extension;

    if meta.name.is_empty() {
        violations.push("extension.name is required (non-empty)".into());
    } else if !meta
        .name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        violations.push(format!(
            "extension.name '{}' must contain only lowercase alphanumeric + hyphens",
            meta.name
        ));
    }

    if meta.version.is_empty() {
        violations.push("extension.version is required (non-empty)".into());
    }

    if meta.description.is_empty() {
        violations.push("extension.description is required (non-empty)".into());
    }

    // An ingest block without a resolvable state_dir can only ever no-op: ingest
    // falls back to cwd, matches nothing, and reports nothing. Catch it here so
    // `base ext validate` fails instead of the plugin appearing to work.
    if let Some(hooks) = &file.hooks
        && let Some(ss) = &hooks.session_start
        && !ss.ingest.is_empty()
    {
        match &meta.state_dir {
            None => violations.push(
                "extension.state_dir is required when [[hooks.session_start.ingest]] is declared \
                 — without it ingest resolves against the session cwd and silently matches nothing. \
                 NOTE: state_dir must appear ABOVE the first [[commands]] header, or TOML binds it \
                 to that command instead of to [extension]."
                    .into(),
            ),
            Some(sd) if sd.trim().is_empty() => {
                violations.push("extension.state_dir is declared but empty".into());
            }
            Some(_) => {}
        }

        for (i, src) in ss.ingest.iter().enumerate() {
            if src.file.trim().is_empty() {
                violations.push(format!("hooks.session_start.ingest[{i}].file is empty"));
            }
            if src.entity.trim().is_empty() {
                violations.push(format!("hooks.session_start.ingest[{i}].entity is empty"));
            }
            if !matches!(src.strategy.as_str(), "upsert" | "replace") {
                violations.push(format!(
                    "hooks.session_start.ingest[{i}].strategy '{}' is not 'upsert' or 'replace'",
                    src.strategy
                ));
            }
        }
    }

    if violations.is_empty() {
        let mut ext: ExtensionDef = file.into();
        ext.source_path = Some(path.to_path_buf());
        Ok(ext)
    } else {
        Err(violations)
    }
}

// ─── Conversion helper ───────────────────────────────────────

/// Convert an extension's declared domains into DomainDef entries.
/// Domain names are prefixed with `ext:{ext_name}:` to avoid collisions.
/// Used by Phase 23 (hook integration) to merge into the domain pool.
pub fn extension_domains_to_domain_defs(ext: &ExtensionDef) -> Vec<crate::domain::DomainDef> {
    let Some(hooks) = &ext.hooks else {
        return Vec::new();
    };
    let Some(user_prompt) = &hooks.user_prompt else {
        return Vec::new();
    };

    user_prompt
        .domains
        .iter()
        .map(|ed| crate::domain::DomainDef {
            name: format!("ext:{}:{}", ext.name, ed.name),
            mode: "triggered".into(),
            prompt_keywords: ed.prompt_keywords.clone(),
            file_keywords: ed.file_keywords.clone(),
            paths: ed.paths.clone(),
            exclude: ed.exclude.clone(),
            rules: ed
                .rules
                .iter()
                .cloned()
                .map(crate::domain::RuleEntry::Bare)
                .collect(),
            query: ed.query.clone(),
            query_format: ed.query_format.clone(),
            commands: Vec::new(),
            command_activation: "both".into(),
            role: None,
            output_mode: None,
            format: None,
        })
        .collect()
}

// ─── Hook presence helpers ───────────────────────────────────

impl ExtensionDef {
    /// Returns a compact string showing which hooks are bound: e.g., "S·U·P·T"
    pub fn hook_summary(&self) -> String {
        let Some(hooks) = &self.hooks else {
            return "none".into();
        };
        let mut parts = Vec::new();
        if hooks.session_start.is_some() {
            parts.push("S");
        }
        if hooks.user_prompt.is_some() {
            parts.push("U");
        }
        if hooks.pre_tool.is_some() {
            parts.push("P");
        }
        if hooks.post_tool.is_some() {
            parts.push("T");
        }
        if parts.is_empty() {
            "none".into()
        } else {
            parts.join("·")
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_extension_manifest() {
        let toml_str = r#"
[extension]
name = "outpost"
version = "0.1.0"
description = "Content production & distribution"
framework_dir = "~/.claude/outpost-framework/"
state_dir = ".outpost/"

[hooks.session_start]
queries = ["queries/content_summary.sparql"]
inject = "Outpost: {piece_count} pieces"

[[hooks.session_start.ingest]]
file = "registry.json"
entity = "ContentPiece"
strategy = "upsert"

[[hooks.user_prompt.domains]]
name = "content"
prompt_keywords = ["outpost", "content pipeline"]
rules = ["Outpost content framework active."]

[[hooks.pre_tool.triggers]]
paths = [".outpost/"]
inject = "Managed by Outpost."

[[hooks.post_tool.handlers]]
pattern = "registry.json"
action = "reingest"
"#;

        let file: ExtensionFile = toml::from_str(toml_str).expect("parse failed");
        let ext: ExtensionDef = file.into();

        assert_eq!(ext.name, "outpost");
        assert_eq!(ext.version, "0.1.0");
        assert_eq!(ext.framework_dir.as_deref(), Some("~/.claude/outpost-framework/"));
        assert_eq!(ext.state_dir.as_deref(), Some(".outpost/"));

        let hooks = ext.hooks.as_ref().expect("hooks missing");

        // Session start
        let ss = hooks.session_start.as_ref().expect("session_start missing");
        assert_eq!(ss.queries.len(), 1);
        assert_eq!(ss.queries[0], "queries/content_summary.sparql");
        assert_eq!(ss.ingest.len(), 1);
        assert_eq!(ss.ingest[0].entity, "ContentPiece");
        assert_eq!(ss.ingest[0].strategy, "upsert");
        assert_eq!(ss.inject.as_deref(), Some("Outpost: {piece_count} pieces"));

        // User prompt
        let up = hooks.user_prompt.as_ref().expect("user_prompt missing");
        assert_eq!(up.domains.len(), 1);
        assert_eq!(up.domains[0].name, "content");
        assert_eq!(up.domains[0].prompt_keywords, vec!["outpost", "content pipeline"]);

        // Pre tool
        let pt = hooks.pre_tool.as_ref().expect("pre_tool missing");
        assert_eq!(pt.triggers.len(), 1);
        assert_eq!(pt.triggers[0].paths, vec![".outpost/"]);

        // Post tool
        let po = hooks.post_tool.as_ref().expect("post_tool missing");
        assert_eq!(po.handlers.len(), 1);
        assert_eq!(po.handlers[0].pattern, "registry.json");
        assert_eq!(po.handlers[0].action, "reingest");
    }

    #[test]
    fn parse_minimal_extension() {
        let toml_str = r#"
[extension]
name = "minimal"
version = "1.0.0"
description = "Minimal extension with no hooks"
"#;

        let file: ExtensionFile = toml::from_str(toml_str).expect("parse failed");
        let ext: ExtensionDef = file.into();

        assert_eq!(ext.name, "minimal");
        assert!(ext.hooks.is_none());
        assert_eq!(ext.hook_summary(), "none");
    }

    #[test]
    fn validate_valid_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.toml");
        std::fs::write(
            &path,
            r#"
[extension]
name = "valid-ext"
version = "0.1.0"
description = "A valid extension"
"#,
        )
        .unwrap();

        let result = validate_extension(&path);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name, "valid-ext");
    }

    #[test]
    fn validate_invalid_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(
            &path,
            r#"
[extension]
name = "Bad Name!"
version = "0.1.0"
description = "Invalid name"
"#,
        )
        .unwrap();

        let result = validate_extension(&path);
        assert!(result.is_err());
        let violations = result.unwrap_err();
        assert!(violations[0].contains("lowercase alphanumeric"));
    }

    #[test]
    fn validate_missing_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.toml");
        std::fs::write(
            &path,
            r#"
[extension]
name = ""
version = ""
description = ""
"#,
        )
        .unwrap();

        let result = validate_extension(&path);
        assert!(result.is_err());
        let violations = result.unwrap_err();
        assert_eq!(violations.len(), 3);
    }

    #[test]
    fn load_from_directory_skips_template_and_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let ext_dir = dir.path();

        // Valid extension
        std::fs::write(
            ext_dir.join("alpha.toml"),
            r#"
[extension]
name = "alpha"
version = "1.0.0"
description = "Alpha extension"
"#,
        )
        .unwrap();

        // _template.toml — should be skipped
        std::fs::write(
            ext_dir.join("_template.toml"),
            r#"
[extension]
name = "template"
version = "0.0.0"
description = "Template"
"#,
        )
        .unwrap();

        // Malformed — should be skipped with warning
        std::fs::write(ext_dir.join("broken.toml"), "not valid toml {{{{").unwrap();

        let extensions = load_extensions_from(ext_dir);
        assert_eq!(extensions.len(), 1);
        assert_eq!(extensions[0].name, "alpha");
    }

    #[test]
    fn load_skips_duplicate_names() {
        let dir = tempfile::tempdir().unwrap();
        let ext_dir = dir.path();

        std::fs::write(
            ext_dir.join("aaa.toml"),
            r#"
[extension]
name = "dupe"
version = "1.0.0"
description = "First"
"#,
        )
        .unwrap();

        std::fs::write(
            ext_dir.join("bbb.toml"),
            r#"
[extension]
name = "dupe"
version = "2.0.0"
description = "Second — should be skipped"
"#,
        )
        .unwrap();

        let extensions = load_extensions_from(ext_dir);
        assert_eq!(extensions.len(), 1);
        assert_eq!(extensions[0].version, "1.0.0");
    }

    #[test]
    fn extension_domains_to_domain_defs_conversion() {
        let toml_str = r#"
[extension]
name = "outpost"
version = "0.1.0"
description = "Test"

[[hooks.user_prompt.domains]]
name = "content"
prompt_keywords = ["outpost", "publish"]
rules = ["Content rule"]
"#;

        let file: ExtensionFile = toml::from_str(toml_str).unwrap();
        let ext: ExtensionDef = file.into();
        let domains = extension_domains_to_domain_defs(&ext);

        assert_eq!(domains.len(), 1);
        assert_eq!(domains[0].name, "ext:outpost:content");
        assert_eq!(domains[0].mode, "triggered");
        assert_eq!(domains[0].prompt_keywords, vec!["outpost", "publish"]);
        assert_eq!(domains[0].rule_texts(), vec!["Content rule"]);
    }

    #[test]
    fn hook_summary_display() {
        let toml_str = r#"
[extension]
name = "test"
version = "0.1.0"
description = "Test"

[hooks.session_start]
queries = []

[[hooks.user_prompt.domains]]
name = "d"
"#;

        let file: ExtensionFile = toml::from_str(toml_str).unwrap();
        let ext: ExtensionDef = file.into();
        assert_eq!(ext.hook_summary(), "S·U");
    }

    // ─── Plugin Distribution P1: optional [dist] block ─────────

    #[test]
    fn dist_block_parses() {
        let toml_str = r#"
[extension]
name = "meta"
version = "0.1.0"
description = "Meta Ads CLI"

[dist]
repo = "ChristopherKahler/meta-cli"
version = "0.1.0"
binary = "meta"
"#;
        let file: ExtensionFile = toml::from_str(toml_str).unwrap();
        assert_eq!(
            file.dist,
            Some(DistSpec {
                repo: "ChristopherKahler/meta-cli".into(),
                version: "0.1.0".into(),
                binary: "meta".into(),
            })
        );
        // AC-3: From carries it into ExtensionDef.
        let ext: ExtensionDef = file.into();
        assert_eq!(ext.dist.unwrap().binary, "meta");
    }

    #[test]
    fn absent_dist_is_none_and_rest_unchanged() {
        // Mirrors today's nano-banana shape: [extension] + [[commands]], no [dist].
        let toml_str = r#"
[extension]
name = "nano-banana"
version = "1.0.0"
description = "Image gen"

[[commands]]
name = "nano-banana"
handler = "bin/nano-banana.mjs"
description = "Generate/edit images"
"#;
        let file: ExtensionFile = toml::from_str(toml_str).unwrap();
        assert_eq!(file.dist, None, "absent [dist] → None (backward-compat)");
        assert_eq!(file.commands.len(), 1, "commands still parse unchanged");
        let ext: ExtensionDef = file.into();
        assert_eq!(ext.dist, None);
        assert_eq!(ext.commands[0].name, "nano-banana");
    }

    fn write_ext(dir: &Path, body: &str) -> PathBuf {
        let p = dir.join("ext.toml");
        std::fs::write(&p, body).unwrap();
        p
    }

    /// The exact shape that cost a day: `state_dir` one line under `[[commands]]`.
    /// TOML binds it to the command, serde used to drop it, ingest fell back to cwd,
    /// and everything downstream reported success.
    #[test]
    fn state_dir_under_commands_header_is_rejected_not_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_ext(
            tmp.path(),
            r#"
[extension]
name = "lore"
version = "0.2.0"
description = "world model"

[[commands]]
name = "lore"
handler = "bin/lore"
description = "pen"
state_dir = "~/.base-gbl/lore/"

[[hooks.session_start.ingest]]
file = "facts.json"
entity = "LoreFact"
strategy = "upsert"
"#,
        );
        let err = validate_extension(&p).expect_err("misplaced state_dir must not validate");
        assert!(
            err.iter().any(|v| v.contains("state_dir")),
            "expected a state_dir violation, got {err:?}"
        );
    }

    #[test]
    fn ingest_without_state_dir_is_a_violation() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_ext(
            tmp.path(),
            r#"
[extension]
name = "lore"
version = "0.2.0"
description = "world model"

[[hooks.session_start.ingest]]
file = "facts.json"
entity = "LoreFact"
strategy = "upsert"
"#,
        );
        let err = validate_extension(&p).expect_err("ingest without state_dir must not validate");
        assert!(err.iter().any(|v| v.contains("state_dir is required")));
    }

    #[test]
    fn ingest_with_state_dir_above_commands_validates() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_ext(
            tmp.path(),
            r#"
[extension]
name = "lore"
version = "0.2.0"
description = "world model"
state_dir = "~/.base-gbl/lore/"

[[commands]]
name = "lore"
handler = "bin/lore"
description = "pen"

[[hooks.session_start.ingest]]
file = "facts.json"
entity = "LoreFact"
strategy = "upsert"
"#,
        );
        let ext = validate_extension(&p).expect("correct placement must validate");
        assert_eq!(ext.state_dir.as_deref(), Some("~/.base-gbl/lore/"));
    }

    #[test]
    fn bad_ingest_strategy_is_a_violation() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write_ext(
            tmp.path(),
            r#"
[extension]
name = "lore"
version = "0.2.0"
description = "world model"
state_dir = "~/.base-gbl/lore/"

[[hooks.session_start.ingest]]
file = "facts.json"
entity = "LoreFact"
strategy = "merge"
"#,
        );
        let err = validate_extension(&p).expect_err("unknown strategy must not validate");
        assert!(err.iter().any(|v| v.contains("strategy")));
    }
}
