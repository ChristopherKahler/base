pub mod matcher;
pub mod sync;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::BaseConfig;
use crate::domain::session::SessionState;

// ─── Standards data model ────────────────────────────────────
//
// A standard is a context-triggered best practice: an imperative rule, the
// real failure it prevents, and machine triggers deciding WHEN it surfaces.
// Canonical rule text lives in MIDAS protocols.md (the living library);
// `base standards sync` re-derives title/rule/failure/controls from it.
// Triggers, stacks, and severity are annotations owned by standards.toml —
// protocols.md doesn't carry them, sync never overwrites them.

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TriggerDef {
    /// Language gate + weak signal. Empty = applies to all languages.
    /// Non-empty + file's language not in the list = standard excluded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    /// Substring evidence in file bytes + the edit payload (case-sensitive).
    /// Import statements are literal text, so this subsumes AST-import evidence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<String>,
    /// Semantic classes from the fixed taxonomy in matcher::CLASSES.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub semantic: Vec<String>,
    /// Path substrings (matched case-insensitively).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StandardDef {
    /// Stable ID — protocols.md section id ("A11") or catalog id ("SC-IDOR").
    pub id: String,
    pub title: String,
    /// Imperative rule text (canonical — synced from protocols.md).
    pub rule: String,
    /// The real failure this prevents (canonical — synced from protocols.md).
    #[serde(default)]
    pub failure: String,
    /// "critical" | "high" | "medium" — annotation, tie-breaks equal scores.
    #[serde(default = "default_severity")]
    pub severity: String,
    /// Compliance controls satisfied (canonical — synced from protocols.md).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controls: Vec<String>,
    /// Provenance, e.g. "midas:protocols.md#A11".
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub triggers: TriggerDef,
    /// Per-stack idiom — the injected text arrives in the touched file's
    /// stack idiom when a variant exists (stack-adapters principle).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub stacks: BTreeMap<String, String>,
}

fn default_severity() -> String {
    "medium".into()
}

impl StandardDef {
    /// Hash of the injectable content — dedup key component so an edited
    /// rule/failure re-injects within a session.
    pub fn content_hash(&self) -> u64 {
        crate::domain::session::rules_hash(&[self.rule.clone(), self.failure.clone()])
    }
}

pub fn severity_rank(severity: &str) -> u8 {
    match severity {
        "critical" => 3,
        "high" => 2,
        _ => 1,
    }
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct SyncSource {
    /// Path to the canonical protocols.md (supports leading ~).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocols: Option<String>,
}

impl SyncSource {
    pub fn is_empty(&self) -> bool {
        self.protocols.is_none()
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct StandardsFile {
    #[serde(default, skip_serializing_if = "SyncSource::is_empty")]
    pub sync: SyncSource,
    #[serde(default, rename = "standard")]
    pub standards: Vec<StandardDef>,
}

// ─── Loading (tiered: global → workspace) ────────────────────

pub fn global_standards_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".base-gbl").join("standards.toml"))
}

fn read_standards_file(path: &Path) -> Option<StandardsFile> {
    let content = std::fs::read_to_string(path).ok()?;
    match toml::from_str::<StandardsFile>(&content) {
        Ok(f) => Some(f),
        Err(e) => {
            eprintln!("standards: failed to parse {}: {e}", path.display());
            None
        }
    }
}

/// Load standards: global `~/.base-gbl/standards.toml` → workspace
/// `.base/standards.toml` overlay, merged by id. Empty Vec if neither exists.
pub fn load_standards(cwd: &Path) -> Vec<StandardDef> {
    let mut standards: Vec<StandardDef> = Vec::new();

    if let Some(global) = global_standards_path()
        && let Some(file) = read_standards_file(&global)
    {
        standards = file.standards;
    }

    if let Some(base_dir) = crate::config::find_workspace_base(cwd)
        && let Some(file) = read_standards_file(&base_dir.join("standards.toml"))
    {
        for od in file.standards {
            if let Some(pos) = standards.iter().position(|s| s.id == od.id) {
                standards[pos] = od;
            } else {
                standards.push(od);
            }
        }
    }

    standards
}

// ─── Hook entry point ────────────────────────────────────────

/// Extract the edit payload from a mutating tool event: the content being
/// written. This is the highest-value evidence — it matches brand-new files
/// before they exist on disk, and it IS the code the standards govern.
pub fn edit_payload(event: &serde_json::Value) -> String {
    let Some(ti) = event.get("tool_input") else {
        return String::new();
    };
    let mut payload = String::new();
    for key in ["content", "new_string"] {
        if let Some(s) = ti.get(key).and_then(|v| v.as_str()) {
            payload.push_str(s);
            payload.push('\n');
        }
    }
    // MultiEdit: edits[].new_string
    if let Some(edits) = ti.get("edits").and_then(|v| v.as_array()) {
        for e in edits {
            if let Some(s) = e.get("new_string").and_then(|v| v.as_str()) {
                payload.push_str(s);
                payload.push('\n');
            }
        }
    }
    payload
}

/// Match + rank + render standards for one touched file. Returns the
/// injection block, or None when nothing clears the threshold or everything
/// matched is session-deduped. Marks injected standards in the session.
pub fn inject_for_file(
    config: &BaseConfig,
    cwd: &Path,
    file_path: &Path,
    payload: &str,
    session: &mut SessionState,
) -> Option<String> {
    let standards = load_standards(cwd);
    if standards.is_empty() {
        return None;
    }

    let ctx = matcher::build_context(file_path, payload);
    let matches = matcher::select(
        &standards,
        &ctx,
        config.standards.min_score,
        config.standards.max_inject.min(5),
    );

    let fresh: Vec<&matcher::Match> = matches
        .iter()
        .filter(|m| !session.is_standard_injected(&m.standard.id, m.standard.content_hash()))
        .collect();
    if fresh.is_empty() {
        return None;
    }

    let block = matcher::render(&fresh, &ctx);
    for m in &fresh {
        session.mark_standard_injected(&m.standard.id, m.standard.content_hash());
    }
    Some(block)
}
