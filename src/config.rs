use std::path::{Path, PathBuf};

use serde::Deserialize;

// ─── Workspace discovery ─────────────────────────────────────

/// The global tier's `.base/` directory — `<home>/.base-gbl/.base`.
///
/// Constructed, never searched for: the global tier has exactly one location, so
/// "not found" is not a state it can be in. `None` only when there is no home.
pub fn global_base_dir() -> Option<PathBuf> {
    crate::home::home_root().map(|h| h.join(".base-gbl").join(".base"))
}

/// The global tier root — the path `-g/--global` swaps cwd for (`cli::tier_cwd`).
fn global_tier_root() -> Option<PathBuf> {
    crate::home::home_root().map(|h| h.join(".base-gbl"))
}

/// Find the workspace `.base/` directory by walking up from cwd.
///
/// `--global` is not a search. `cli::tier_cwd` swaps cwd for `<home>/.base-gbl`,
/// whose `.base` is at a known path, so it is returned directly — existing or
/// not — and the walk is skipped.
///
/// Walking it was a silent wrong-tier write. Nothing in the crate creates
/// `<home>/.base-gbl/.base` (`install::create_global_tier` makes `.base-gbl` and
/// stops), so on a fresh install the walk climbed past the tier to `<home>` and
/// took `<home>/.base` — the WORKSPACE tier — and every `-g` verb reported
/// success against the wrong graph. Reproduced on both platforms; see the fork
/// `base-sync-client-surface`.
///
/// The workspace tier keeps the walk and keeps refusing when it finds nothing:
/// there, no known correct location exists, which is the whole reason
/// `crud::require_base_for_write` never auto-creates (issue #8).
pub fn find_workspace_base(cwd: &Path) -> Option<PathBuf> {
    if let Some(root) = global_tier_root()
        && cwd == root
    {
        return global_base_dir();
    }
    walk_up(cwd, |dir| {
        let base = dir.join(".base");
        base.is_dir().then_some(base)
    })
}

/// Walk up from `start` (inclusive), returning the first ancestor for which
/// `hit` yields `Some`.
///
/// The crate had six copies of this loop, and every one of them could climb out
/// of a test's tempdir into a real workspace on the machine. In test builds this
/// stops at the sandbox ceiling: on Windows `%TEMP%` is
/// `C:\Users\<user>\AppData\Local\Temp`, so walking up from a tempdir passes
/// straight through `C:\Users\<user>` — itself a real base workspace — and
/// resolves the operator's own tier as if it were the test's. On Linux the same
/// walk ends at `/` and finds nothing, which is why it took a Windows run to see
/// it. Production is unaffected: the check compiles out entirely, so a machine
/// whose home IS a workspace still resolves it.
pub fn walk_up<T>(start: &Path, hit: impl Fn(&Path) -> Option<T>) -> Option<T> {
    let mut dir = start.to_path_buf();
    loop {
        #[cfg(feature = "isolation-guard")]
        if !crate::home::within_sandbox(&dir) {
            return None;
        }
        if let Some(found) = hit(&dir) {
            return Some(found);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Find the app root for AST scoping: nearest ancestor (including `target` itself
/// when it is a directory) that carries an app marker — `.git`, `.paul`, `.base-ast`,
/// or an existing `.base`. This is what makes each codebase's AST map self-contained
/// instead of every parse clobbering one shared workspace `ast.ttl`.
pub fn ast_app_root(target: &Path) -> Option<PathBuf> {
    let start = if target.is_file() {
        target.parent()?.to_path_buf()
    } else {
        target.to_path_buf()
    };
    walk_up(&start, |dir| {
        let marked = dir.join(".git").exists()
            || dir.join(".paul").is_dir()
            || dir.join(".base-ast").is_dir()
            || dir.join(".base").is_dir();
        marked.then(|| dir.to_path_buf())
    })
}

/// Resolve where a target's AST map should be WRITTEN: `<app_root>/.base-ast/ast.ttl`.
/// A dedicated `.base-ast/` sidecar (sibling to `.base-ast-cache/`) is used instead
/// of `.base/` so an app's map never shadows workspace `.base/` resolution for
/// knowledge commands. `target` must be an absolute path.
pub fn resolve_ast_ttl(target: &Path) -> PathBuf {
    let root = ast_app_root(target)
        .or_else(|| find_workspace_base(target).and_then(|b| b.parent().map(Path::to_path_buf)))
        // Never adopt the HOME directory as an app root for a target that lives
        // beneath it. Home almost always carries `.base` (it is the usual
        // workspace), so both resolution tiers above walk up and land on it —
        // and then every project under home shares one `~/.base-ast/ast.ttl`.
        // Each sync overwrites the last and all of them register under the same
        // app name (the home folder's), which is the exact clobbering this
        // module exists to prevent.
        //
        // Verified 2026-08-14 on Windows AND Linux: mapping app B erased app A,
        // `base ast list` showed a single app, and querying app A returned
        // "No AST entities matching". Falling through to `None` here gives the
        // target its own self-contained sidecar instead.
        //
        // Home itself remains valid when it IS the target, so an intentional
        // workspace-wide map still works.
        .filter(|r| crate::home::home_root().as_deref() != Some(r.as_path()) || r.as_path() == target);
    match root {
        Some(r) => r.join(".base-ast").join("ast.ttl"),
        None => target.join(".base-ast").join("ast.ttl"),
    }
}

/// Find the AST map to READ from `cwd`, walking up: prefers `<root>/.base-ast/ast.ttl`,
/// falling back to a legacy `<root>/.base/ast.ttl` (the pre-sidecar workspace map).
pub fn find_ast_ttl(cwd: &Path) -> Option<PathBuf> {
    walk_up(cwd, |dir| {
        let sidecar = dir.join(".base-ast").join("ast.ttl");
        if sidecar.is_file() {
            return Some(sidecar);
        }
        let legacy = dir.join(".base").join("ast.ttl");
        legacy.is_file().then_some(legacy)
    })
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
    pub update: UpdateConfig,
    #[serde(default)]
    pub grounding: GroundingConfig,
    #[serde(default)]
    pub graph: GraphConfig,
    #[serde(default)]
    pub multimodal: MultimodalConfig,
    #[serde(default)]
    pub flow: FlowConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub protocol: ProtocolConfig,
    #[serde(default)]
    pub standards: StandardsConfig,
    #[serde(default)]
    pub workspace: Vec<WorkspaceEntry>,
}

// ─── Standards Config (MIDAS standards-injection layer) ─────

/// Context-triggered best-practice injection on PreToolUse Edit/Write.
/// The budget fields keep injection scarce — whole-catalog injection is
/// context pollution and gets tuned out.
#[derive(Debug, Clone, Deserialize)]
pub struct StandardsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Max standards injected per touched file (hard-capped at 5 in code).
    #[serde(default = "default_standards_max_inject")]
    pub max_inject: usize,
    /// Minimum match score — 3 means a bare language or path match never injects.
    #[serde(default = "default_standards_min_score")]
    pub min_score: u32,
}

fn default_standards_max_inject() -> usize { 3 }
fn default_standards_min_score() -> u32 { 3 }

impl Default for StandardsConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            max_inject: default_standards_max_inject(),
            min_score: default_standards_min_score(),
        }
    }
}

// ─── Workspace Registry ─────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceEntry {
    pub path: String,
}

// ─── Context Bracket Config ─────────────────────────────────

/// Thresholds for the context bracket.
///
/// Two modes. `percent` (default) derives the bracket from real context-window
/// depletion read off the transcript; `turns` uses the legacy prompt count.
/// Percent is preferred because turn length is a wildcard — a build turn reading
/// three large files consumes far more context than a discussion turn, so a fixed
/// prompt count fires early in conversation and late in heavy work. The turn
/// thresholds are retained and still used whenever the transcript is unreadable
/// (first prompt of a session, missing path), so the bracket never goes blind.
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

    /// "percent" or "turns". ABSENT means turns — a base.toml written before this
    /// feature existed must keep behaving exactly as it did. Defaulting an absent
    /// key to percent would silently measure every legacy install against the
    /// fallback 200k window, so anyone on a larger-context model would compute
    /// several times their real depletion and pin to CRITICAL permanently.
    /// New installs and the migration both write this key explicitly.
    #[serde(default)]
    pub mode: Option<String>,
    /// Context window to measure depletion against. Configured rather than
    /// inferred: the transcript records the model but not its window size.
    #[serde(default = "default_context_window")]
    pub context_window: u32,
    #[serde(default = "default_fresh_until_pct")]
    pub fresh_until_pct: f64,
    #[serde(default = "default_moderate_until_pct")]
    pub moderate_until_pct: f64,
    #[serde(default = "default_depleted_until_pct")]
    pub depleted_until_pct: f64,

    /// Rules injected by tier. See [`BracketRules`].
    #[serde(default)]
    pub rules: BracketRules,
}

/// Rules the bracket injects directly, independent of domain matching.
///
/// Domains inject on a keyword or path match, which makes them the wrong home for
/// a rule that must hold regardless of subject — the rule silently stops applying
/// the moment the conversation drifts off its triggers. These inject on the tier
/// alone, so `always` is genuinely every prompt and the tiered buckets track
/// context pressure rather than topic.
///
/// The tiered buckets are additive with `always`, not exclusive: at DEPLETED a
/// prompt receives `always` + `depleted`.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct BracketRules {
    /// Injected every prompt at every tier. For rules that must not erode —
    /// the layer that survives a long session because it is re-sent, not remembered.
    #[serde(default)]
    pub always: Vec<String>,
    #[serde(default)]
    pub fresh: Vec<String>,
    #[serde(default)]
    pub moderate: Vec<String>,
    #[serde(default)]
    pub depleted: Vec<String>,
    #[serde(default)]
    pub critical: Vec<String>,
}

impl BracketRules {
    /// True when no bucket holds anything — lets the hook skip the block entirely.
    pub fn is_empty(&self) -> bool {
        self.always.is_empty()
            && self.fresh.is_empty()
            && self.moderate.is_empty()
            && self.depleted.is_empty()
            && self.critical.is_empty()
    }
}

fn default_fresh_until() -> u32 { 3 }
fn default_moderate_until() -> u32 { 10 }
fn default_depleted_until() -> u32 { 20 }
fn default_refresh_interval() -> u32 { 5 }
fn default_bracket_enabled() -> bool { true }
fn default_context_window() -> u32 { 200_000 }
fn default_fresh_until_pct() -> f64 { 20.0 }
fn default_moderate_until_pct() -> f64 { 45.0 }
fn default_depleted_until_pct() -> f64 { 70.0 }

impl BracketConfig {
    /// Whether to derive the bracket from context percentage.
    /// Absent `mode` = legacy turn counting; percent is opt-in per the field docs.
    pub fn is_percent_mode(&self) -> bool {
        self.mode
            .as_deref()
            .is_some_and(|m| m.eq_ignore_ascii_case("percent"))
    }
}

impl Default for BracketConfig {
    fn default() -> Self {
        Self {
            fresh_until: default_fresh_until(),
            moderate_until: default_moderate_until(),
            depleted_until: default_depleted_until(),
            refresh_interval: default_refresh_interval(),
            enabled: default_bracket_enabled(),
            // None = turn mode. Percent is opt-in via config; the installer and
            // the migration write `mode = "percent"` explicitly.
            mode: None,
            context_window: default_context_window(),
            fresh_until_pct: default_fresh_until_pct(),
            moderate_until_pct: default_moderate_until_pct(),
            depleted_until_pct: default_depleted_until_pct(),
            rules: BracketRules::default(),
        }
    }
}

// ─── Devmode Config ─────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DevmodeConfig {
    #[serde(default)]
    pub enabled: bool,
}

// ─── Update Config ──────────────────────────────────────────

/// Self-update behavior. Session start is the trigger: when the periodic check
/// finds a newer release, base installs it in a detached background process and
/// says nothing. Everyone stays current without being asked to run anything, and
/// nobody eats a download in the middle of a session — the swap is an atomic
/// rename, so the running process keeps its inode and the next session is new.
///
/// Pin a machine with `base config set update.auto false`.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateConfig {
    #[serde(default = "default_auto_update")]
    pub auto: bool,
}

fn default_auto_update() -> bool {
    true
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self { auto: default_auto_update() }
    }
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

// ─── Multimodal Config (graph extract — P4) ─────────────────

/// Multimodal ingest for `base graph extract` (PDF / image-via-vision /
/// audio+video-via-Whisper). OFF by default: with it off, extract is markdown-only
/// and pulls ZERO extra dependencies. No sudo is ever required — PDF is in-process
/// (`pdf-extract` crate), image uses the already-present `claude`, and only
/// audio/video pull `whisper`+`ffmpeg`, installed once via `pip install --user`
/// (marker-gated, never again) the first time such a corpus is ingested with this
/// enabled. Flip on with `base config set multimodal.enabled true`.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct MultimodalConfig {
    #[serde(default)]
    pub enabled: bool,
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
        let home = crate::home::home_root()?;
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
    if let Some(home) = crate::home::home_root()
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

#[cfg(test)]
mod tests {
    use super::*;

    // Auto-update is ON unless a machine opts out: everyone should land on the
    // current release without being told to run anything. A regression here is
    // silent — users simply stop getting updates — so pin the default.
    #[test]
    fn auto_update_defaults_on_and_survives_absent_config() {
        assert!(UpdateConfig::default().auto);

        // Absent [update] section entirely.
        let c: BaseConfig = toml::from_str("").expect("empty config must parse");
        assert!(c.update.auto, "a config with no [update] section must still auto-update");

        // Section present but empty.
        let c: BaseConfig = toml::from_str("[update]\n").unwrap();
        assert!(c.update.auto);

        // Explicit opt-out is honored.
        let c: BaseConfig = toml::from_str("[update]\nauto = false\n").unwrap();
        assert!(!c.update.auto);
    }

    // ─── Global tier resolution ──────────────────────────────

    /// The decoy `<home>/.base` is the thing the old walk-up climbed into, and
    /// it is what makes this fire on Linux as well as Windows.
    #[test]
    fn the_global_tier_resolves_directly_instead_of_walking_into_the_workspace_tier() {
        let tmp = tempfile::tempdir().unwrap();
        crate::home::with_thread_home(tmp.path(), || {
            std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
            let root = tmp.path().join(".base-gbl");
            std::fs::create_dir_all(&root).unwrap();
            assert!(!root.join(".base").exists(), "precondition: the tier is not created yet");

            assert_eq!(
                find_workspace_base(&root),
                Some(root.join(".base")),
                "--global must resolve its own tier, never the workspace tier above it"
            );
        });
    }

    /// A first pull arrives before anything has created the tier, so resolution
    /// cannot depend on it already being there.
    #[test]
    fn the_global_tier_resolves_before_it_exists_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        crate::home::with_thread_home(tmp.path(), || {
            let root = tmp.path().join(".base-gbl");
            assert!(!root.exists(), "precondition: nothing on disk at all");
            assert_eq!(find_workspace_base(&root), Some(root.join(".base")));
        });
    }

    /// The workspace tier is a genuine search and stays one.
    #[test]
    fn an_ordinary_workspace_cwd_still_walks_up() {
        let tmp = tempfile::tempdir().unwrap();
        crate::home::with_thread_home(tmp.path(), || {
            let ws = tmp.path().join("proj");
            std::fs::create_dir_all(ws.join(".base")).unwrap();
            let deep = ws.join("src").join("nested");
            std::fs::create_dir_all(&deep).unwrap();

            assert_eq!(find_workspace_base(&deep), Some(ws.join(".base")));
        });
    }

    /// Outside a workspace there is no known correct location, so the answer
    /// stays `None` — `require_base_for_write` depends on it (issue #8).
    #[test]
    fn a_cwd_outside_any_workspace_still_resolves_to_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        crate::home::with_thread_home(tmp.path(), || {
            let orphan = tmp.path().join("no").join("workspace").join("here");
            std::fs::create_dir_all(&orphan).unwrap();
            assert_eq!(find_workspace_base(&orphan), None);
        });
    }
}
