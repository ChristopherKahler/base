use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The value `section.field` has when `base.toml` says nothing about it.
///
/// Serialises `BaseConfig::default()` and reads the answer back out of it, so
/// this cannot drift from the defaults themselves: a new field with a
/// `#[serde(default = ...)]` is covered the moment it exists, and a field that
/// is renamed stops answering rather than answering wrongly.
///
/// `None` means no such key, which is the only case that deserves "not found".
pub fn default_value(section: &str, field: &str) -> Option<toml::Value> {
    let effective = toml::Value::try_from(BaseConfig::default()).ok()?;
    effective.get(section)?.get(field).cloned()
}

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

/// Pure: may a read that started at `start` take the map sitting at `dir`?
///
/// Home is both a map holder and the usual workspace, so from anywhere beneath
/// it an unbounded walk lands there and answers with the operator's whole
/// profile. [`resolve_ast_ttl`] refuses exactly this on the WRITE side; the read
/// side had no equivalent, which is what let `base ast query` answer a question
/// about the code in front of you with a different project's code — silently,
/// at exit 0. Measured 2026-09-08 from a clone with no map of its own: 13 rows
/// of an unrelated tool, and `--contains` on a symbol defined in that very tree
/// reported no match.
///
/// Home stays valid when it IS the start directory, so an intentional
/// workspace-wide map still answers for someone standing in it.
///
/// Deliberately takes `home` as a parameter rather than calling
/// [`crate::home::home_root`]: that function resolves differently in a test
/// binary than in the shipped one (`home.rs:41-44` is `cfg`-gated), so a rule
/// that consulted it could not be tested for what it actually does in
/// production. A pure function has no feature-gated input.
pub fn ast_map_admissible(start: &Path, dir: &Path, home: Option<&Path>) -> bool {
    home != Some(dir) || dir == start
}

/// Find the AST map to READ from `cwd`, walking up: prefers `<root>/.base-ast/ast.ttl`,
/// falling back to a legacy `<root>/.base/ast.ttl` (the pre-sidecar workspace map).
///
/// The walk is BOUNDED, by the same two rules the write path already applies:
/// it climbs no further than the app root ([`ast_app_root`]), and it never
/// adopts home's map from below ([`ast_map_admissible`]). Walking up *within* an
/// app is kept — a query from `src/crud/` must still answer from the repo's map.
///
/// The app-root stop is evaluated OUTSIDE the `isolation-guard` block on
/// purpose. That guard exists only in test builds, so a boundary that depended
/// on it would be a boundary the shipped binary does not have, and a green test
/// would say nothing about production.
pub fn find_ast_ttl(cwd: &Path) -> Option<PathBuf> {
    let app_root = ast_app_root(cwd);
    let home = crate::home::home_root();
    let mut dir = cwd.to_path_buf();
    loop {
        #[cfg(feature = "isolation-guard")]
        if !crate::home::within_sandbox(&dir) {
            return None;
        }
        if ast_map_admissible(cwd, &dir, home.as_deref()) {
            let sidecar = dir.join(".base-ast").join("ast.ttl");
            if sidecar.is_file() {
                return Some(sidecar);
            }
            let legacy = dir.join(".base").join("ast.ttl");
            if legacy.is_file() {
                return Some(legacy);
            }
        }
        // Stop AT the app root. A read never crosses into another app's map:
        // that is the difference between this and #37, which is about letting a
        // read span several maps DELIBERATELY.
        if app_root.as_deref() == Some(dir.as_path()) {
            return None;
        }
        if !dir.pop() {
            return None;
        }
    }
}

// ─── Namespace Config ────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    pub injection: InjectionConfig,
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
    pub relay: RelayConfig,
    #[serde(default)]
    pub workspace: Vec<WorkspaceEntry>,
}

// ─── Standards Config (MIDAS standards-injection layer) ─────

/// Context-triggered best-practice injection on PreToolUse Edit/Write.
/// The budget fields keep injection scarce — whole-catalog injection is
/// context pollution and gets tuned out.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DevmodeConfig {
    #[serde(default)]
    pub enabled: bool,
}

// ─── Relay Config ───────────────────────────────────────────

/// Multi-session relay (`base relay`): titled sessions, pings, task hand-offs,
/// and the hook-injected wake contract that keeps an idle session pingable.
/// Everything it writes stays under `~/.base-gbl/.base/relay-inbox/`.
///
/// `enabled = false` stops the hooks from drawing a codename for every session
/// and from injecting the wake contract; a session that runs `base relay
/// register` itself still takes part. `wake_nudge = false` keeps titles and
/// pings but never injects the Monitor arming block — for harnesses without a
/// Monitor tool, or operators who arm it by hand. `BASE_NO_AUTONAME` and
/// `BASE_NO_WAKE_NUDGE` remain the per-process equivalents.
///
/// `base config set relay.enabled false` / `base config set relay.wake_nudge false`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub wake_nudge: bool,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self { enabled: true, wake_nudge: true }
    }
}

// ─── Update Config ──────────────────────────────────────────

/// Self-update behavior. Session start is the trigger: when the periodic check
/// finds a newer release, base installs it in a detached background process and
/// says nothing. Everyone stays current without being asked to run anything, and
/// nobody eats a download in the middle of a session — the swap is an atomic
/// rename, so the running process keeps its inode and the next session is new.
///
/// Pin a machine with `base config set update.auto false`.
/// What prompt-time traversal is allowed to spend.
///
/// The walk runs on every non-lean prompt, so an unbounded one would grow the
/// prompt without ever saying so. The default is sized to what the neighbourhood
/// block already spends, so injection volume does not move on average when this
/// ships -- it moves in WHAT it spends the budget on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjectionConfig {
    #[serde(default = "default_walk_budget")]
    pub walk_budget: usize,
}

fn default_walk_budget() -> usize {
    2000
}

impl Default for InjectionConfig {
    fn default() -> Self {
        Self { walk_budget: default_walk_budget() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GroundingConfig {
    #[serde(default)]
    pub enabled: bool,
}

// ─── Graph Config (Phase 52 — proactive compaction) ─────────

/// Graph-hygiene policy. Auto-compaction runs from the session-start guard (a
/// low-frequency path — NOT the hook hot path) when a tier graph exceeds the size
/// threshold, so graphs never balloon on a user's machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Master switch for the one-time domain migration (opt-out). On by default
    /// because Chris's ruling is that it lands automatically with the update and
    /// nobody straggles; off is for an operator who wants to run it by hand.
    #[serde(default = "default_true")]
    pub auto_migrate: bool,
}

fn default_compact_threshold_mb() -> u64 { 12 }
fn default_compact_cooldown_hours() -> i64 { 24 }

impl Default for GraphConfig {
    fn default() -> Self {
        Self {
            auto_compact: true,
            compact_threshold_mb: default_compact_threshold_mb(),
            compact_cooldown_hours: default_compact_cooldown_hours(),
            auto_migrate: true,
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
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MultimodalConfig {
    #[serde(default)]
    pub enabled: bool,
}

// ─── Flow Config ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// A `base.toml` that EXISTS but could not become settings.
///
/// There is deliberately NO `Absent` variant. A missing `base.toml` is a normal
/// first run, not a fault, and collapsing those two states is the defect this
/// type exists to remove (#158): absent, empty and unreadable are three
/// different things and only the last two are worth speaking about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigFault {
    /// The file is there and could not be read: permissions, a directory in its
    /// place, an I/O error. Anything except "not found".
    Unreadable { path: PathBuf, err: String },
    /// The file was read and is not valid TOML.
    Unparseable { path: PathBuf, err: String },
    /// Valid TOML, wrong shape for `BaseConfig`. One wrongly typed field --
    /// `auto = "yes"` where a bool belongs -- used to discard every OTHER
    /// setting in the file as well, silently.
    Mismatched { paths: Vec<PathBuf>, err: String },
    /// No home directory could be resolved, so neither tier could be located.
    HomeUnresolvable,
}

impl std::fmt::Display for ConfigFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Every arm ends with the consequence, because a path and a parse error
        // alone do not tell the operator the thing that actually matters: the
        // settings they chose are NOT the settings base is running on.
        const TAIL: &str = "base is running on DEFAULT settings, not yours";
        match self {
            Self::Unreadable { path, err } => {
                write!(f, "cannot read {} ({err}) -- {TAIL}", path.display())
            }
            Self::Unparseable { path, err } => {
                write!(f, "cannot parse {} ({err}) -- {TAIL}", path.display())
            }
            Self::Mismatched { paths, err } => {
                let names: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
                write!(f, "{} is valid TOML but not valid settings ({err}) -- {TAIL}", names.join(" + "))
            }
            Self::HomeUnresolvable => {
                write!(f, "cannot determine a home directory, so no base.toml could be read -- {TAIL}")
            }
        }
    }
}

/// Report config faults on stderr, at most once per process.
///
/// ONCE PER PROCESS, not once per call, because one `base scaffold` calls
/// `BaseConfig::load` three times (`scaffold.rs:114`, `install.rs:977` and
/// `install.rs:1520`) and the operator does not need telling three times. The
/// empty case returns before touching the latch, so a healthy first load does
/// not spend it and a fault arriving later still speaks.
///
/// STDERR, deliberately, and the reason is specific rather than stylistic.
/// `hook/mod.rs:136` is one of the nine production call sites, so this runs
/// inside a hook. A hook STDOUT is a parsed contract: on pre-tool-use,
/// post-tool-use and stop it must be exactly one JSON envelope, and on
/// session-start and user-prompt-submit it is the injected text itself. Stderr
/// is the operator-diagnostic channel on every one of those paths -- `dispatch`
/// at `hook/mod.rs:67` fails open to "stderr only, exit 0, empty stdout", and
/// `post_tool_use.rs` records that "stderr stays stderr -- those lines are
/// operator diagnostics, not model context". A config warning must never be
/// able to change one byte of hook stdout, and a test asserts exactly that.
fn report_config_faults_once(faults: &[ConfigFault]) {
    report_config_faults(faults, &FAULTS_REPORTED);
}

/// The one report this process gets.
///
/// A `static` rather than a field because there is no object to hang it on:
/// `load` is an associated function reached from nine unrelated call sites, none
/// of which share any state.
static FAULTS_REPORTED: std::sync::Once = std::sync::Once::new();

/// The body of [`report_config_faults_once`], with the latch passed in. Returns
/// whether THIS call was the one that spoke.
///
/// The latch is a parameter for exactly one reason: a `std::sync::Once` cannot be
/// re-armed, and `cargo test` runs a file's tests inside one process, so a
/// process-global latch would let whichever test reported first decide the result
/// of every other one. That is a flaky instrument, not a test.
///
/// Production never passes anything but `FAULTS_REPORTED`, so shipped behaviour is
/// unchanged. And because an injected latch can only ever prove the latch -- never
/// that production wires the real one -- the once-per-process claim is ALSO proved
/// end to end against the shipped binary, in
/// `tests/config_fault_once_per_process_test.rs`.
fn report_config_faults(faults: &[ConfigFault], latch: &std::sync::Once) -> bool {
    // Before the latch, deliberately. A healthy load must not spend the one report
    // the process gets, or a file that broke later in the same process would say
    // nothing -- which is this defect again, one level up.
    if faults.is_empty() {
        return false;
    }
    let mut spoke = false;
    latch.call_once(|| {
        spoke = true;
        for fault in faults {
            eprintln!("base: {fault}");
        }
    });
    spoke
}

impl BaseConfig {
    /// Load config: global `~/.base-gbl/base.toml` as base, workspace `.base/base.toml` overlaid on top.
    /// Workspace sections override global at the key level; missing sections inherit from global.
    ///
    /// A config that cannot be read or parsed is REPORTED (once per process)
    /// rather than silently replaced by defaults. The signature is unchanged on
    /// purpose: six of the nine production call sites have no useful error path
    /// and would each end in `unwrap_or_default()`, which would copy the silent
    /// default to six places instead of removing it from one. A caller that
    /// needs to ACT on a fault uses [`BaseConfig::load_reporting`].
    pub fn load(cwd: &Path) -> Self {
        let (config, faults) = Self::load_reporting(cwd);
        report_config_faults_once(&faults);
        config
    }

    /// Load, and say what could not be read.
    ///
    /// The returned list is empty on a healthy load AND on a first run with no
    /// `base.toml` anywhere -- absent is not a fault, which is why
    /// [`ConfigFault`] has no variant for it. Every other outcome that used to
    /// collapse into `default()` now names itself here.
    pub fn load_reporting(cwd: &Path) -> (Self, Vec<ConfigFault>) {
        let mut faults = Vec::new();

        let Some(home) = crate::home::home_root() else {
            faults.push(ConfigFault::HomeUnresolvable);
            return (Self::default(), faults);
        };
        let global_path = home.join(".base-gbl").join("base.toml");
        let ws_path = cwd.join(".base").join("base.toml");

        let global = Self::read_table(&global_path, &mut faults);
        let workspace = Self::read_table(&ws_path, &mut faults);

        // Only the files that actually produced a table are named as sources of
        // a shape error. Asking the filesystem again with `exists()` would cost
        // a syscall and could disagree with what was just read.
        let mut sources = Vec::new();
        if global.is_some() {
            sources.push(global_path);
        }
        if workspace.is_some() {
            sources.push(ws_path);
        }

        let merged = match (global, workspace) {
            (Some(g), Some(w)) => merge_toml_tables(g, w),
            (Some(g), None) => g,
            (None, Some(w)) => w,
            (None, None) => return (Self::default(), faults),
        };

        match toml::Value::Table(merged).try_into() {
            Ok(config) => (config, faults),
            Err(e) => {
                faults.push(ConfigFault::Mismatched {
                    paths: sources,
                    err: e.to_string(),
                });
                (Self::default(), faults)
            }
        }
    }

    /// One tier table. Returns `None` for "nothing to merge from here", and
    /// pushes a fault for every reason EXCEPT the file being absent.
    ///
    /// This single match arm is the honest-envelope law in code: the read used
    /// to end `.ok()?`, which made a missing file and an unreadable one
    /// indistinguishable to everything downstream.
    fn read_table(path: &Path, faults: &mut Vec<ConfigFault>) -> Option<toml::Table> {
        let content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            // The one quiet case: no file is a normal first run.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => {
                faults.push(ConfigFault::Unreadable {
                    path: path.to_path_buf(),
                    err: e.to_string(),
                });
                return None;
            }
        };
        match content.parse::<toml::Table>() {
            Ok(table) => Some(table),
            Err(e) => {
                faults.push(ConfigFault::Unparseable {
                    path: path.to_path_buf(),
                    err: e.to_string(),
                });
                None
            }
        }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    }

    /// `base config get update.auto` used to say "not found" about a setting
    /// whose default is true, because `get` reads the file and the file says
    /// nothing until you have overridden it.
    #[test]
    fn default_value_answers_for_a_key_the_file_never_mentions() {
        assert_eq!(default_value("update", "auto"), Some(toml::Value::Boolean(true)));
        assert_eq!(
            default_value("graph", "auto_compact"),
            Some(toml::Value::Boolean(true))
        );
        // Only a key that genuinely does not exist deserves "not found".
        assert_eq!(default_value("update", "nosuchfield"), None);
        assert_eq!(default_value("nosuchsection", "auto"), None);

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

    // ─── #158 leg A — a config that cannot be read is reported, never swallowed ───
    //
    // These observe `load_reporting`, which is pure and returns the faults. That is
    // NOT a channel any operator reads, so every claim below is re-proved against
    // the shipped binary's stderr and exit code in
    // `tests/config_fault_surface_test.rs`. Green here and silent there would mean
    // nothing was fixed.

    /// The global `base.toml` inside an isolated fake home, with its parent made.
    /// Returns the path so a test can corrupt exactly that file and nothing else.
    fn fake_global_toml(home: &Path) -> PathBuf {
        let dir = home.join(".base-gbl");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("base.toml")
    }

    /// The headline of #158: a file that exists and does not parse has to SPEAK,
    /// and what it says has to be actionable — which file, and what is wrong.
    #[test]
    fn an_unparseable_global_config_reports_its_path_and_the_parse_error() {
        let tmp = tempfile::tempdir().unwrap();
        crate::home::with_thread_home(tmp.path(), || {
            let path = fake_global_toml(tmp.path());
            std::fs::write(&path, "[update]\nauto = \n").unwrap();

            let (config, faults) = BaseConfig::load_reporting(tmp.path());

            assert_eq!(faults.len(), 1, "one broken file is one fault: {faults:?}");
            match &faults[0] {
                ConfigFault::Unparseable { path: p, err } => {
                    assert_eq!(p, &path, "the fault names the file the operator must fix");
                    assert!(!err.is_empty(), "and carries the parser's own words");
                }
                other => panic!("an unparseable file is Unparseable, not {other:?}"),
            }
            // The settings really are gone. That is the truth the operator needs
            // told, not hidden — what changed is that it is no longer SILENT.
            assert_eq!(config.update.auto, BaseConfig::default().update.auto);
        });
    }

    /// Exists and cannot be read is a third state, and it used to be
    /// indistinguishable from absent. A directory where the file belongs fails
    /// `read_to_string` with something that is NOT `NotFound`, on every platform,
    /// without depending on the uid the suite happens to run as — `chmod 000` is a
    /// no-op for root and would make this test silently stop measuring.
    #[test]
    fn an_unreadable_global_config_is_a_fault_and_is_not_confused_with_an_absent_one() {
        let tmp = tempfile::tempdir().unwrap();
        crate::home::with_thread_home(tmp.path(), || {
            let path = fake_global_toml(tmp.path());
            std::fs::create_dir_all(&path).unwrap();

            let (_config, faults) = BaseConfig::load_reporting(tmp.path());

            assert_eq!(faults.len(), 1, "{faults:?}");
            match &faults[0] {
                ConfigFault::Unreadable { path: p, err } => {
                    assert_eq!(p, &path);
                    assert!(!err.is_empty(), "the OS error is what tells them why");
                }
                other => panic!("a file that exists and cannot be read is Unreadable, not {other:?}"),
            }
        });
    }

    /// N2, and the widest of the four collapse points. `try_into()` is
    /// all-or-nothing: one wrongly typed field — `auto = "yes"` where a bool
    /// belongs — used to discard every OTHER setting in the file along with it,
    /// silently. A one-character typo cost the operator their whole configuration.
    #[test]
    fn a_single_wrongly_typed_field_no_longer_discards_every_other_setting_in_silence() {
        let tmp = tempfile::tempdir().unwrap();
        crate::home::with_thread_home(tmp.path(), || {
            let path = fake_global_toml(tmp.path());
            // Valid TOML that is not valid settings: it parses, then fails to convert.
            std::fs::write(
                &path,
                "[update]\nauto = \"yes\"\n\n[namespace]\nprefix = \"chosen-by-the-operator\"\n",
            )
            .unwrap();

            let (config, faults) = BaseConfig::load_reporting(tmp.path());

            assert_eq!(faults.len(), 1, "{faults:?}");
            match &faults[0] {
                ConfigFault::Mismatched { paths, err } => {
                    assert!(
                        paths.contains(&path),
                        "the file that produced the table is named: {paths:?}"
                    );
                    assert!(!err.is_empty());
                }
                other => panic!("valid TOML of the wrong shape is Mismatched, not {other:?}"),
            }
            // The unrelated setting IS still lost — this leg does not change that,
            // because the conversion is all-or-nothing. It is now announced.
            assert_eq!(
                config.namespace.prefix,
                BaseConfig::default().namespace.prefix,
                "precondition for the report mattering: the good setting really did go"
            );
        });
    }

    /// The control that makes every test above mean something. Absent is NOT a
    /// fault: a first run has no `base.toml` anywhere and must stay completely
    /// quiet, or the warning becomes noise everyone learns to scroll past.
    #[test]
    fn an_absent_config_is_not_a_fault_in_either_tier() {
        let tmp = tempfile::tempdir().unwrap();
        crate::home::with_thread_home(tmp.path(), || {
            std::fs::create_dir_all(tmp.path().join(".base-gbl")).unwrap();
            let ws = tmp.path().join("ws");
            std::fs::create_dir_all(ws.join(".base")).unwrap();
            assert!(
                !tmp.path().join(".base-gbl").join("base.toml").exists(),
                "precondition: no global file"
            );
            assert!(
                !ws.join(".base").join("base.toml").exists(),
                "precondition: no workspace file"
            );

            let (config, faults) = BaseConfig::load_reporting(&ws);

            assert!(faults.is_empty(), "a first run is not a fault: {faults:?}");
            assert_eq!(config.update.auto, BaseConfig::default().update.auto);
        });
    }

    /// The other control: a good file is silent AND its settings are the ones in
    /// force. A "fix" that reported on every load would pass the broken arms and
    /// fail here.
    #[test]
    fn a_healthy_config_reports_nothing_and_its_settings_are_honoured() {
        let tmp = tempfile::tempdir().unwrap();
        crate::home::with_thread_home(tmp.path(), || {
            std::fs::write(fake_global_toml(tmp.path()), "[update]\nauto = false\n").unwrap();

            let (config, faults) = BaseConfig::load_reporting(tmp.path());

            assert!(faults.is_empty(), "a good file has nothing to report: {faults:?}");
            assert!(
                !config.update.auto,
                "and the setting the operator chose is the one in force"
            );
        });
    }

    /// Two tiers, one broken. The fault has to name the file that is actually
    /// broken or the operator edits the wrong one — and the tier that still reads
    /// must keep applying, so a broken overlay does not cost them settings that
    /// are perfectly readable.
    #[test]
    fn a_broken_workspace_config_names_the_workspace_file_and_spares_the_global_one() {
        let tmp = tempfile::tempdir().unwrap();
        crate::home::with_thread_home(tmp.path(), || {
            std::fs::write(fake_global_toml(tmp.path()), "[update]\nauto = false\n").unwrap();
            let ws = tmp.path().join("ws");
            std::fs::create_dir_all(ws.join(".base")).unwrap();
            let ws_toml = ws.join(".base").join("base.toml");
            std::fs::write(&ws_toml, "this is not toml {{{\n").unwrap();

            let (config, faults) = BaseConfig::load_reporting(&ws);

            assert_eq!(faults.len(), 1, "only the broken tier faults: {faults:?}");
            match &faults[0] {
                ConfigFault::Unparseable { path, .. } => assert_eq!(
                    path, &ws_toml,
                    "the workspace file is the broken one; naming the global file would send them to the wrong place"
                ),
                other => panic!("{other:?}"),
            }
            assert!(
                !config.update.auto,
                "the readable global tier still applies — a broken overlay is not a reset"
            );
        });
    }

    /// DoD 15 / N3 — the shipped claim this defect makes false.
    ///
    /// `config.rs` documents the pin as `base config set update.auto false`, and
    /// `hook/session_start.rs` calls `auto_update` on EVERY session start, gated on
    /// that flag, whose default is `true`. So an unparseable file silently un-pins
    /// the machine and it resumes updating itself.
    ///
    /// This test states the residual honestly instead of hiding it: the pin IS
    /// lost, because a file that will not parse cannot say `false`. What leg A
    /// closes is the SILENCE. `load` reports before returning, and `hook/mod.rs`
    /// loads before it dispatches to `session_start::handle`, so the operator is
    /// told before `auto_update` is ever reached.
    #[test]
    fn a_pinned_machine_that_loses_its_config_is_never_silent_about_it() {
        let tmp = tempfile::tempdir().unwrap();
        crate::home::with_thread_home(tmp.path(), || {
            let path = fake_global_toml(tmp.path());

            // The pin works, and is quiet.
            std::fs::write(&path, "[update]\nauto = false\n").unwrap();
            let (pinned, quiet) = BaseConfig::load_reporting(tmp.path());
            assert!(!pinned.update.auto, "precondition: the machine is pinned");
            assert!(quiet.is_empty(), "precondition: a good file says nothing");

            // The file stops parsing, which is exactly what #158 reports.
            std::fs::write(&path, "[update]\nauto = false\n[[[ broken\n").unwrap();
            let (after, faults) = BaseConfig::load_reporting(tmp.path());

            assert!(
                after.update.auto,
                "the pin is genuinely lost — reporting that honestly is the point"
            );
            assert!(!faults.is_empty(), "and it is NO LONGER SILENT, which is the fix");
            assert!(
                faults
                    .iter()
                    .any(|f| f.to_string().contains(&path.display().to_string())),
                "the report names the file that lost the pin: {faults:?}"
            );
        });
    }

    /// A message has to carry the consequence, not only the cause. "cannot parse X"
    /// tells an operator a file is broken; it does not tell them that the settings
    /// they chose are not the settings running.
    #[test]
    fn every_fault_names_its_consequence_as_well_as_its_cause() {
        let faults = [
            ConfigFault::Unreadable {
                path: PathBuf::from("/probe/a.toml"),
                err: "permission denied".into(),
            },
            ConfigFault::Unparseable {
                path: PathBuf::from("/probe/b.toml"),
                err: "expected value".into(),
            },
            ConfigFault::Mismatched {
                paths: vec![PathBuf::from("/probe/c.toml")],
                err: "invalid type".into(),
            },
            ConfigFault::HomeUnresolvable,
        ];
        for f in &faults {
            let msg = f.to_string();
            assert!(
                msg.contains("DEFAULT settings, not yours"),
                "a fault says what it COST, not only what went wrong: {msg}"
            );
        }
        assert!(faults[0].to_string().contains("/probe/a.toml"));
        assert!(faults[1].to_string().contains("/probe/b.toml"));
        assert!(faults[2].to_string().contains("/probe/c.toml"));
        assert!(
            faults[0].to_string().contains("permission denied"),
            "the underlying error survives into the message"
        );
    }

    /// The guard is "once per process", not "once per call": one `base scaffold`
    /// calls `load` three times and the operator does not need telling three times.
    ///
    /// The empty case must return BEFORE the latch. If a healthy load spent it, a
    /// file that broke later in the same process would say nothing — which is the
    /// defect again, one level up. The latch is a parameter here for exactly this
    /// test: a process-global `Once` cannot be re-armed, so any other test that
    /// happened to report first would decide this one's result.
    #[test]
    fn the_report_latch_fires_once_and_an_empty_report_does_not_spend_it() {
        let latch = std::sync::Once::new();
        let fault = [ConfigFault::HomeUnresolvable];

        assert!(
            !report_config_faults(&[], &latch),
            "nothing to say means nothing said"
        );
        assert!(
            !latch.is_completed(),
            "and a healthy load must not spend the one report the process gets"
        );

        assert!(report_config_faults(&fault, &latch), "the first real fault speaks");
        assert!(latch.is_completed());
        assert!(
            !report_config_faults(&fault, &latch),
            "and the second, third and fourth do not"
        );
    }
}
