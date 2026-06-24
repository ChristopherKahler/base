use std::path::{Path, PathBuf};

use serde::Deserialize;

// ─── Workspace discovery ─────────────────────────────────────

/// Find the workspace `.base/` directory by walking up from cwd.
pub fn find_workspace_base(cwd: &Path) -> Option<PathBuf> {
    let mut dir = cwd.to_path_buf();
    loop {
        let base = dir.join(".base");
        if base.is_dir() {
            return Some(base);
        }
        if !dir.pop() {
            return None;
        }
    }
}

// ─── Namespace Config ────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct NamespaceConfig {
    #[serde(default = "default_prefix")]
    pub prefix: String,
    #[serde(default = "default_uri")]
    pub uri: String,
}

fn default_prefix() -> String {
    "ops".into()
}
fn default_uri() -> String {
    "http://ops-sys.local/ontology#".into()
}

impl Default for NamespaceConfig {
    fn default() -> Self {
        Self {
            prefix: default_prefix(),
            uri: default_uri(),
        }
    }
}

// ─── Base Config (base.toml) ─────────────────────────────────

#[derive(Debug, Clone, Deserialize, Default)]
pub struct BaseConfig {
    #[serde(default)]
    pub namespace: NamespaceConfig,
    #[serde(default)]
    pub sync: SyncConfig,
    #[serde(default)]
    pub signal: SignalConfig,
    #[serde(default)]
    pub bracket: BracketConfig,
    #[serde(default)]
    pub devmode: DevmodeConfig,
    #[serde(default)]
    pub grounding: GroundingConfig,
    #[serde(default)]
    pub graph: GraphConfig,
    #[serde(default)]
    pub flow: FlowConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub protocol: ProtocolConfig,
    #[serde(default)]
    pub workspace: Vec<WorkspaceEntry>,
}

// ─── Workspace Registry ─────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceEntry {
    pub path: String,
}

// ─── Context Bracket Config ─────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct BracketConfig {
    #[serde(default = "default_fresh_until")]
    pub fresh_until: u32,
    #[serde(default = "default_moderate_until")]
    pub moderate_until: u32,
    #[serde(default = "default_depleted_until")]
    pub depleted_until: u32,
    #[serde(default = "default_refresh_interval")]
    pub refresh_interval: u32,
    #[serde(default = "default_bracket_enabled")]
    pub enabled: bool,
}

fn default_fresh_until() -> u32 { 3 }
fn default_moderate_until() -> u32 { 10 }
fn default_depleted_until() -> u32 { 20 }
fn default_refresh_interval() -> u32 { 5 }
fn default_bracket_enabled() -> bool { true }

impl Default for BracketConfig {
    fn default() -> Self {
        Self {
            fresh_until: default_fresh_until(),
            moderate_until: default_moderate_until(),
            depleted_until: default_depleted_until(),
            refresh_interval: default_refresh_interval(),
            enabled: default_bracket_enabled(),
        }
    }
}

// ─── Devmode Config ─────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DevmodeConfig {
    #[serde(default)]
    pub enabled: bool,
}

// ─── Grounding Config (Phase 30) ────────────────────────────

/// System-level toggle (like devmode). When enabled, every prompt-time hook
/// injection carries a `<grounding>` block instructing source-verification of
/// factual claims. Settable via `base config set grounding.enabled true`.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct GroundingConfig {
    #[serde(default)]
    pub enabled: bool,
}

// ─── Graph Config (Phase 52 — proactive compaction) ─────────

/// Graph-hygiene policy. Auto-compaction runs from the session-start guard (a
/// low-frequency path — NOT the hook hot path) when a tier graph exceeds the size
/// threshold, so graphs never balloon on a user's machine.
#[derive(Debug, Clone, Deserialize)]
pub struct GraphConfig {
    /// Master switch for proactive auto-compaction (opt-out).
    #[serde(default = "default_true")]
    pub auto_compact: bool,
    /// Compact a tier graph at session-start once it exceeds this many MB.
    #[serde(default = "default_compact_threshold_mb")]
    pub compact_threshold_mb: u64,
    /// Minimum hours between auto-compactions of the same tier (anti-churn).
    #[serde(default = "default_compact_cooldown_hours")]
    pub compact_cooldown_hours: i64,
}

fn default_compact_threshold_mb() -> u64 { 12 }
fn default_compact_cooldown_hours() -> i64 { 24 }

impl Default for GraphConfig {
    fn default() -> Self {
        Self {
            auto_compact: true,
            compact_threshold_mb: default_compact_threshold_mb(),
            compact_cooldown_hours: default_compact_cooldown_hours(),
        }
    }
}

// ─── Flow Config ────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct FlowConfig {
    /// Master switch — opt-in feature, default false
    #[serde(default)]
    pub enabled: bool,
    /// Blocked-by + deferred-orphan resurface scans
    #[serde(default = "default_true")]
    pub resurface: bool,
    /// Static behavioral rules injection
    #[serde(default = "default_true")]
    pub protocol: bool,
    /// Recurring idea tracking
    #[serde(default)]
    pub mentions: bool,
    /// Mentions needed before surfacing as recurring
    #[serde(default = "default_mention_threshold")]
    pub mention_threshold: u32,
}

fn default_true() -> bool { true }
fn default_mention_threshold() -> u32 { 3 }

impl Default for FlowConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            resurface: default_true(),
            protocol: default_true(),
            mentions: false,
            mention_threshold: default_mention_threshold(),
        }
    }
}

// ─── Memory Config ──────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct MemoryConfig {
    /// Master switch — opt-in feature, default false
    #[serde(default)]
    pub enabled: bool,
    /// "claude" = native memory, "both" = mirror to graph + flat files, "base" = graph only
    #[serde(default = "default_memory_mode")]
    pub mode: String,
}

fn default_memory_mode() -> String { "claude".into() }

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: default_memory_mode(),
        }
    }
}

// ─── Protocol Config (task-artifact protocol) ───────────────

/// The operating protocol: where project artifact folders live (by lifecycle stage)
/// and whether tasks must declare a produced artifact. Set by os-config in the global
/// `~/.base-gbl/base.toml`; inherited by every workspace via the config overlay
/// (set once, every scaffolded workspace conforms). base stays agnostic.
#[derive(Debug, Clone, Deserialize)]
pub struct ProtocolConfig {
    /// Opt-in master switch (default off so base works unconfigured).
    #[serde(default)]
    pub enabled: bool,
    /// Tasks must declare what they produce — their definition of done.
    #[serde(default = "default_true")]
    pub require_artifact: bool,
    /// Days a project folder may go untouched before its tasks are flagged.
    #[serde(default = "default_protocol_stale_days")]
    pub stale_days: u32,
    /// Lifecycle stages → folder templates. The FIRST stage is where new projects land.
    #[serde(default)]
    pub stage: Vec<StageDef>,
}

fn default_protocol_stale_days() -> u32 { 7 }

impl Default for ProtocolConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            require_artifact: default_true(),
            stale_days: default_protocol_stale_days(),
            stage: Vec::new(),
        }
    }
}

impl ProtocolConfig {
    /// Resolve a stage by name, or the first (default landing) stage when name is None.
    pub fn stage_for(&self, name: Option<&str>) -> Option<&StageDef> {
        match name {
            Some(n) => self.stage.iter().find(|s| s.name == n),
            None => self.stage.first(),
        }
    }
}

/// One project lifecycle stage and the folder its artifacts live in.
#[derive(Debug, Clone, Deserialize)]
pub struct StageDef {
    /// Stage name (e.g. "planning", "project").
    pub name: String,
    /// Folder template relative to the workspace root; `{slug}` is substituted.
    pub folder: String,
    /// Optional context-doc filename created in the folder on project creation.
    #[serde(default)]
    pub context_doc: Option<String>,
}

// ─── Signal Config ───────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct SignalConfig {
    #[serde(default = "default_max_chars")]
    pub max_chars: usize,
    #[serde(default = "default_signal_enabled")]
    pub enabled: bool,
    /// Working-set scope: "workspace" (default — the current-workspace view) or "global"
    /// (the flat union of every registered workspace; restores pre-v0.8 behavior, Req 5).
    #[serde(default = "default_signal_scope")]
    pub scope: String,
}

fn default_max_chars() -> usize { 2000 }
fn default_signal_enabled() -> bool { true }
fn default_signal_scope() -> String { "workspace".into() }

impl Default for SignalConfig {
    fn default() -> Self {
        Self {
            max_chars: default_max_chars(),
            enabled: default_signal_enabled(),
            scope: default_signal_scope(),
        }
    }
}

// ─── Sync Config ─────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct SyncConfig {
    #[serde(default = "default_include")]
    pub include: Vec<String>,
    #[serde(default = "default_exclude")]
    pub exclude: Vec<String>,
}

fn default_include() -> Vec<String> {
    vec!["**/*.md".into(), "**/paul.json".into()]
}
fn default_exclude() -> Vec<String> {
    vec![
        "node_modules/".into(),
        "target/".into(),
        ".git/".into(),
        ".base/".into(),
    ]
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            include: default_include(),
            exclude: default_exclude(),
        }
    }
}

impl BaseConfig {
    /// Load config: global `~/.base-gbl/base.toml` as base, workspace `.base/base.toml` overlaid on top.
    /// Workspace sections override global at the key level; missing sections inherit from global.
    pub fn load(cwd: &Path) -> Self {
        Self::try_load(cwd).unwrap_or_default()
    }

    fn try_load(cwd: &Path) -> Option<Self> {
        let home = dirs::home_dir()?;
        let global_path = home.join(".base-gbl").join("base.toml");
        let ws_path = cwd.join(".base").join("base.toml");

        let global = Self::load_toml_table(&global_path);
        let workspace = Self::load_toml_table(&ws_path);

        let merged = match (global, workspace) {
            (Some(g), Some(w)) => merge_toml_tables(g, w),
            (Some(g), None) => g,
            (None, Some(w)) => w,
            (None, None) => return None,
        };

        toml::Value::Table(merged).try_into().ok()
    }

    fn load_toml_table(path: &Path) -> Option<toml::Table> {
        let content = std::fs::read_to_string(path).ok()?;
        content.parse::<toml::Table>().ok()
    }

}

/// Deep-merge two TOML tables. Overlay values win; nested tables merge recursively.
/// Arrays and scalars in overlay replace base entirely.
fn merge_toml_tables(base: toml::Table, overlay: toml::Table) -> toml::Table {
    let mut merged = base;
    for (key, overlay_val) in overlay {
        match (merged.remove(&key), overlay_val) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => {
                merged.insert(key, toml::Value::Table(merge_toml_tables(b, o)));
            }
            (_, val) => {
                merged.insert(key, val);
            }
        }
    }
    merged
}

// ─── Query Config (queries.toml) ─────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct QueryDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub sparql: String,
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default)]
    pub order: u32,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_format() -> String {
    "list".into()
}
fn default_enabled() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct QueriesFile {
    #[serde(default)]
    query: Vec<QueryDef>,
}

const DEFAULT_QUERIES_TOML: &str = include_str!("queries.default.toml");

/// Load queries with tiered override: embedded defaults → global → workspace.
/// Replaces `{{prefix}}` placeholder with configured namespace prefix.
pub fn load_queries(cwd: &Path, config: &BaseConfig) -> Vec<QueryDef> {
    let mut queries = parse_queries_toml(DEFAULT_QUERIES_TOML);

    // Layer global queries
    if let Some(home) = dirs::home_dir()
        && let Ok(content) =
            std::fs::read_to_string(home.join(".base-gbl").join("queries.toml"))
    {
        queries = merge_queries(queries, parse_queries_toml(&content));
    }

    // Layer workspace queries
    if let Ok(content) = std::fs::read_to_string(cwd.join(".base").join("queries.toml")) {
        queries = merge_queries(queries, parse_queries_toml(&content));
    }

    // Replace {{prefix}} placeholder in SPARQL text
    for q in &mut queries {
        q.sparql = q.sparql.replace("{{prefix}}", &config.namespace.prefix);
    }

    queries.retain(|q| q.enabled);
    queries.sort_by_key(|q| q.order);
    queries
}

fn parse_queries_toml(content: &str) -> Vec<QueryDef> {
    toml::from_str::<QueriesFile>(content)
        .map(|f| f.query)
        .unwrap_or_default()
}

/// Merge overlay queries onto base: override by name, append new.
fn merge_queries(base: Vec<QueryDef>, overlay: Vec<QueryDef>) -> Vec<QueryDef> {
    let mut merged = base;
    for oq in overlay {
        if let Some(pos) = merged.iter().position(|q| q.name == oq.name) {
            merged[pos] = oq;
        } else {
            merged.push(oq);
        }
    }
    merged
}
