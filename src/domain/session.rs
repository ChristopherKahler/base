use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::BracketConfig;

// ─── Context Bracket ────────────────────────────────────────

/// A session's depth. FRESH, a new session's, is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Bracket {
    #[default]
    Fresh,
    Moderate,
    Depleted,
    Critical,
}

impl fmt::Display for Bracket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fresh => write!(f, "FRESH"),
            Self::Moderate => write!(f, "MODERATE"),
            Self::Depleted => write!(f, "DEPLETED"),
            Self::Critical => write!(f, "CRITICAL"),
        }
    }
}

impl Bracket {
    /// The tier a label names, exactly as `Display` writes it. `None` for anything else.
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "FRESH" => Some(Self::Fresh),
            "MODERATE" => Some(Self::Moderate),
            "DEPLETED" => Some(Self::Depleted),
            "CRITICAL" => Some(Self::Critical),
            _ => None,
        }
    }

    /// The rules in force at this tier: `always` first, then the tier's own bucket.
    ///
    /// Additive rather than exclusive — a DEPLETED prompt gets `always` + `depleted`.
    /// `always` leads so the permanent rules keep a stable position in the block
    /// regardless of tier.
    pub fn entries<'a>(&self, rules: &'a crate::config::BracketRules) -> Vec<&'a crate::config::BracketRule> {
        let tier = match self {
            Self::Fresh => &rules.fresh,
            Self::Moderate => &rules.moderate,
            Self::Depleted => &rules.depleted,
            Self::Critical => &rules.critical,
        };
        rules.always.iter().chain(tier.iter()).collect()
    }

}

/// Render the given rule texts as the bracket block for `bracket`, numbered from 0. Empty string for no rules.
/// The prompt hook passes only the rules it is sending (F3): the ones not covered by a loaded CLAUDE.md and not
/// yet sent this session.
pub fn render_bracket_rules(bracket: Bracket, texts: &[&str]) -> String {
    if texts.is_empty() {
        return String::new();
    }
    let mut out = format!("[BRACKET RULES — {bracket}]\n");
    for (i, rule) in texts.iter().enumerate() {
        out.push_str(&format!("  {i}. {rule}\n"));
    }
    out.push('\n');
    out
}

/// One bracket rule's identity in the per-rule record (F3): an id and a content hash, both from its text.
/// The same text in two buckets is one rule, sent once; an edited text is a new rule, sent once more.
pub fn bracket_rule_key(text: &str) -> (String, u64) {
    let hash = rules_hash(&[text.to_string()]);
    (format!("bracket:{hash:016x}"), hash)
}

// ─── Session State ──────────────────────────────────────────

/// Tracks which domains have been injected in the current session.
/// Stored at `.base/.session` (JSON). Session-start clears it.
/// Separator for session-scoped map keys. Control char — cannot occur in a domain
/// name, standard id, or file path, so it can never collide with real key content.
const SCOPE_SEP: char = '\u{1}';

/// Scope used when no session id is available (direct CLI calls, older hook payloads).
/// All such callers share one namespace, which is the pre-existing behavior.
const SHARED_SCOPE: &str = "_shared";

/// Dead sessions never come back, so their dedup entries are pure growth.
/// Keeps `.session` bounded without needing a reaper.
const MAX_TRACKED_SESSIONS: usize = 20;

/// Session id for this process, set once at hook entry.
///
/// A hook invocation is a fresh process serving exactly one Claude session, so a
/// process-wide binding is precise rather than a shortcut — and it makes every
/// existing `SessionState::load` call site session-scoped without touching its
/// signature. Tests bypass it with `load_for`/`set_active`, since a test process
/// impersonates several sessions.
static PROCESS_SESSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Bind this process to a session id. Call once, at hook entry, before any load.
pub fn set_process_session(session_id: Option<&str>) {
    if let Some(id) = session_id.filter(|s| !s.is_empty()) {
        let _ = PROCESS_SESSION.set(id.to_string());
    }
}

fn process_session() -> Option<&'static str> {
    PROCESS_SESSION.get().map(String::as_str)
}

/// What one session was told about one rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShownRule {
    /// Unix seconds. The action throttle (F5, at most once per 10 minutes per rule) compares against it.
    pub at: u64,
    /// The bracket tier in force when it was shown. A tier change re-serves the rules
    /// now in force, once (F8).
    pub tier: String,
    /// A hash of what was actually rendered. A text or rationale edit changes it, so
    /// an edited rule is shown again (F8's last line).
    pub content: u64,
}

/// How a rule's record decides that it is due again (spec F8's table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReShow {
    /// Always, place and topic, and every rule with no matcher: once per session per scope, again when the
    /// bracket changes tier (when `on_tier_change`), and again when its text or rationale changes.
    PerSession { on_tier_change: bool },
    /// Action: every time its action runs, at most once per `secs`, and again when its content changes. A tier
    /// change does not re-fire it, because it fires on its action and not on a tier.
    Throttle { secs: u64 },
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SessionState {
    /// Session this instance is acting for. Not persisted — it is set at load and
    /// namespaces every dedup key below, so one session's injections cannot
    /// suppress another's. Accessors apply it internally, which keeps their
    /// signatures unchanged for the ~15 call sites across the hooks.
    #[serde(skip)]
    active: String,
    /// `session_id + SCOPE_SEP + domain name` → rules hash (for change detection)
    #[serde(default)]
    pub injected: HashMap<String, u64>,
    /// session id → unix seconds last touched, for pruning dead sessions.
    #[serde(default)]
    pub last_seen: HashMap<String, u64>,
    /// Number of user prompts, workspace-wide. Retained for backward compatibility
    /// and as the fallback when no session id is available; mirrors the active
    /// session's count so existing readers stay coherent.
    #[serde(default)]
    pub prompt_count: u32,
    /// Prompt count per Claude Code `session_id`.
    ///
    /// `.session` is one file per WORKSPACE, but several Claude sessions can run in
    /// one workspace at once (cx terminals, squads). Keying the counter on the
    /// workspace made concurrent sessions increment and clear each other's count —
    /// observed in telemetry as counts repeating, jumping, and resetting mid-session.
    /// The bracket was therefore reporting some other conversation's depth.
    #[serde(default)]
    pub prompt_counts: HashMap<String, u32>,
    /// File path → content-version of the AST map last injected this session.
    /// Keyed on content (not just path) so a file that CHANGES re-injects fresh
    /// context, while an unchanged re-touch stays deduped.
    #[serde(default)]
    pub ast_injected: HashMap<String, u64>,
    /// App roots whose files were edited this turn — the Stop hook refreshes
    /// exactly these code maps (not just the session-cwd app), then clears the set.
    #[serde(default)]
    pub dirty_apps: HashSet<String>,
    /// Standard id → content hash of the injected rule text. Once per standard
    /// per session; the bracket force-refresh clears it (via clear_dedup) so
    /// long sessions get a top-of-awareness restore.
    #[serde(default)]
    pub standards_injected: HashMap<String, u64>,
    /// Scoped key → what this session was told about one rule, and when.
    ///
    /// F9: dedup per RULE, not per domain block. Before this the unit was the block,
    /// so adding one rule to a seventeen-rule domain handed the reader all seventeen
    /// again, sixteen of which it had already been told this session.
    ///
    /// The key carries the SCOPE, which is what lets one map serve all four kinds
    /// without a second one: a topic or always rule is keyed on its id alone and is
    /// therefore once per session, while a place or action rule is keyed on its id
    /// AND the place or action that matched, and is therefore once per place and once
    /// per action.
    ///
    /// The bracket rules live here too, one record per rule under [`bracket_rule_key`] (BO-03, F3). Until
    /// BO-03 a separate map, `bracket_shown`, held the one tier whose whole block had been served, and a tier
    /// change sent the whole block again. A `.session` file that still carries that key loads as before:
    /// serde ignores it. [`SessionState::clear_dedup`] does not clear this map, so the DEPLETED and CRITICAL
    /// force-refresh, which exists for domain context, never re-sends a bracket rule.
    #[serde(default)]
    pub rules_shown: HashMap<String, ShownRule>,
    /// Scoped [`bracket_rule_key`] id → whether a CLAUDE.md Claude Code loads covered that bracket rule when this
    /// session first met it (BO-03, F3).
    ///
    /// Decided ONCE per session, the way Claude Code reads CLAUDE.md: at launch, and again at `/compact`, which is
    /// also a session start, and session start clears this session's records (`clear_for`). A CLAUDE.md edited
    /// mid-session is not in the reader's context, so a verdict read on every prompt would follow an edit the
    /// reader never saw. And it costs one read per session instead of one per prompt.
    #[serde(default)]
    pub bracket_covered: HashMap<String, bool>,
    /// Scoped key → the bracket tier the PROMPT hook most recently computed for this session.
    ///
    /// `petrel`'s FINDING 1 on `5c099d1`. The prompt hook reads its tier from the transcript's real
    /// percentage. The tool hook's event carries no such reading, so it fell back to the prompt count. Both
    /// write one per-rule record that re-opens whenever its stored tier differs, so in percent mode every
    /// switch between a prompt and a tool call served the same rules again. The prompt hook owns the counter
    /// and the reading, so it records the tier here, and every tool-hook branch reads this one value.
    #[serde(default)]
    pub bracket_tier: HashMap<String, String>,
}

impl SessionState {
    /// Load session state from `.base/.session`. Returns empty state if missing or malformed.
    ///
    /// Binds to the process session id (see [`set_process_session`]), so every
    /// caller gets per-session dedup without passing an id explicitly.
    pub fn load(base_dir: &Path) -> Self {
        let path = base_dir.join(".session");
        let mut state: Self = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        state.set_active(process_session());
        state.touch();
        state.prune_dead_sessions();
        state
    }

    /// Save session state atomically.
    pub fn save(&self, base_dir: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(base_dir)?;
        let path = base_dir.join(".session");
        let json = serde_json::to_string(self)?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    /// Clear session state (called by session-start for fresh session).
    pub fn clear(base_dir: &Path) {
        let _ = std::fs::remove_file(base_dir.join(".session"));
    }

    /// Load state and bind it to a session, so dedup is per-session rather than
    /// per-workspace. Prefer this over `load` everywhere a session id exists.
    pub fn load_for(base_dir: &Path, session_id: Option<&str>) -> Self {
        let mut state = Self::load(base_dir);
        state.set_active(session_id);
        state.touch();
        state.prune_dead_sessions();
        state
    }

    /// Bind this instance to a session id. `None` uses the shared scope.
    pub fn set_active(&mut self, session_id: Option<&str>) {
        self.active = session_id
            .filter(|s| !s.is_empty())
            .unwrap_or(SHARED_SCOPE)
            .to_string();
    }

    /// The scope in effect. `SessionState::default()` leaves `active` empty, so
    /// normalize here rather than in every caller — otherwise a default-constructed
    /// state writes keys under an empty scope that a loaded one can never find.
    fn active_scope(&self) -> &str {
        if self.active.is_empty() {
            SHARED_SCOPE
        } else {
            &self.active
        }
    }

    /// Namespace a dedup key to the active session.
    fn scoped(&self, key: &str) -> String {
        format!("{}{SCOPE_SEP}{}", self.active_scope(), key)
    }

    /// Whether a stored key belongs to the active session.
    fn is_own(&self, key: &str) -> bool {
        key.split_once(SCOPE_SEP)
            .is_some_and(|(scope, _)| scope == self.active_scope())
    }

    pub fn now_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    }

    /// Record that the active session is alive.
    fn touch(&mut self) {
        let now = Self::now_secs();
        self.last_seen.insert(self.active_scope().to_string(), now);
    }

    /// Drop the least-recently-seen sessions once too many accumulate. Without
    /// this, every abandoned session leaves its dedup keys behind forever.
    fn prune_dead_sessions(&mut self) {
        if self.last_seen.len() <= MAX_TRACKED_SESSIONS {
            return;
        }
        let mut by_age: Vec<(String, u64)> =
            self.last_seen.iter().map(|(k, v)| (k.clone(), *v)).collect();
        by_age.sort_by_key(|(_, seen)| std::cmp::Reverse(*seen));

        let doomed: Vec<String> = by_age
            .into_iter()
            .skip(MAX_TRACKED_SESSIONS)
            .map(|(k, _)| k)
            .filter(|k| k != &self.active) // never evict the live session
            .collect();

        for id in doomed {
            self.forget_session(&id);
        }
    }

    /// Remove every trace of one session from all maps.
    fn forget_session(&mut self, session_id: &str) {
        let prefix = format!("{session_id}{SCOPE_SEP}");
        self.injected.retain(|k, _| !k.starts_with(&prefix));
        self.ast_injected.retain(|k, _| !k.starts_with(&prefix));
        self.standards_injected.retain(|k, _| !k.starts_with(&prefix));
        self.bracket_tier.retain(|k, _| !k.starts_with(&prefix));
        self.rules_shown.retain(|k, _| !k.starts_with(&prefix));
        self.bracket_covered.retain(|k, _| !k.starts_with(&prefix));
        self.dirty_apps.retain(|k| !k.starts_with(&prefix));
        self.prompt_counts.remove(session_id);
        self.last_seen.remove(session_id);
    }

    /// Check if a domain was already injected with the same rules hash.
    pub fn is_injected(&self, domain: &str, hash: u64) -> bool {
        self.injected.get(&self.scoped(domain)) == Some(&hash)
    }

    /// Mark a domain as injected with its current rules hash.
    pub fn mark_injected(&mut self, domain: &str, hash: u64) {
        self.injected.insert(self.scoped(domain), hash);
    }

    /// Increment prompt count and return the new value.
    /// Workspace-wide; prefer `increment_prompt_for` with a session id.
    pub fn increment_prompt(&mut self) -> u32 {
        self.prompt_count += 1;
        self.prompt_count
    }

    /// Increment this session's prompt count and return the new value.
    /// Falls back to the workspace-wide counter when no session id is available.
    pub fn increment_prompt_for(&mut self, session_id: Option<&str>) -> u32 {
        match session_id {
            Some(id) => {
                let count = self.prompt_counts.entry(id.to_string()).or_insert(0);
                *count += 1;
                let count = *count;
                // Mirror onto the legacy field so existing readers see this
                // session's depth rather than a stale workspace total.
                self.prompt_count = count;
                count
            }
            None => self.increment_prompt(),
        }
    }

    /// This session's prompt count, or the workspace-wide count when unkeyed.
    pub fn prompt_count_for(&self, session_id: Option<&str>) -> u32 {
        session_id
            .and_then(|id| self.prompt_counts.get(id).copied())
            .unwrap_or(self.prompt_count)
    }

    /// Derive context bracket from prompt count and config thresholds.
    /// Turn-based only; prefer `bracket_for`, which uses real context depletion.
    pub fn bracket(&self, config: &BracketConfig) -> Bracket {
        self.bracket_for(config, None, None)
    }

    /// Derive the context bracket.
    ///
    /// Uses `context_pct` (real depletion read from the transcript) when percent
    /// mode is on and a reading is available. Falls back to turn thresholds
    /// otherwise, so a missing or not-yet-written transcript degrades rather than
    /// blinding the bracket.
    pub fn bracket_for(
        &self,
        config: &BracketConfig,
        session_id: Option<&str>,
        context_pct: Option<f64>,
    ) -> Bracket {
        if !config.enabled {
            return Bracket::Moderate; // default when brackets disabled
        }

        if config.is_percent_mode()
            && let Some(pct) = context_pct
        {
            return if pct <= config.fresh_until_pct {
                Bracket::Fresh
            } else if pct <= config.moderate_until_pct {
                Bracket::Moderate
            } else if pct <= config.depleted_until_pct {
                Bracket::Depleted
            } else {
                Bracket::Critical
            };
        }

        let count = self.prompt_count_for(session_id);
        if count <= config.fresh_until {
            Bracket::Fresh
        } else if count <= config.moderate_until {
            Bracket::Moderate
        } else if count <= config.depleted_until {
            Bracket::Depleted
        } else {
            Bracket::Critical
        }
    }

    /// Record the tier the prompt hook computed, for the tool hook to serve at (`petrel` FINDING 1).
    pub fn record_tier(&mut self, tier: Bracket) {
        let key = self.scoped("tier");
        self.bracket_tier.insert(key, tier.to_string());
    }

    /// The tier a tool call serves at: the one the prompt hook last computed for this session, so the two
    /// hooks agree about what was served. Before the session's first prompt there is none, and the prompt
    /// count decides, which is what the tool hook always did.
    pub fn served_tier(&self, config: &BracketConfig, session_id: Option<&str>) -> Bracket {
        self.bracket_tier
            .get(&self.scoped("tier"))
            .and_then(|t| Bracket::from_label(t))
            .unwrap_or_else(|| self.bracket_for(config, session_id, None))
    }

    /// Whether to force-refresh dedup (re-inject all domains) this prompt.
    /// True when DEPLETED or CRITICAL AND prompt lands on the refresh interval.
    pub fn should_force_refresh(&self, config: &BracketConfig) -> bool {
        self.should_force_refresh_for(config, None, None)
    }

    /// Session-aware force-refresh check. The interval still counts prompts —
    /// it is a cadence, not a depth measure — but the bracket gating it comes
    /// from real depletion when available.
    pub fn should_force_refresh_for(
        &self,
        config: &BracketConfig,
        session_id: Option<&str>,
        context_pct: Option<f64>,
    ) -> bool {
        if !config.enabled || config.refresh_interval == 0 {
            return false;
        }
        let bracket = self.bracket_for(config, session_id, context_pct);
        let count = self.prompt_count_for(session_id);
        matches!(bracket, Bracket::Depleted | Bracket::Critical)
            && count > 0
            && count.is_multiple_of(config.refresh_interval)
    }

    /// Clear state for ONE session, leaving every concurrent session untouched.
    ///
    /// SessionStart previously called `clear()`, deleting the shared file — so a
    /// new terminal reset every other live session's bracket to FRESH and wiped
    /// their dedup. Now only the starting session's own namespace is removed, so
    /// it still gets its full re-injection without disturbing anyone else.
    pub fn clear_for(base_dir: &Path, session_id: Option<&str>) {
        let Some(id) = session_id.filter(|s| !s.is_empty()) else {
            Self::clear(base_dir);
            return;
        };
        let mut state = Self::load(base_dir);
        state.forget_session(id);
        state.prompt_count = 0;
        let _ = state.save(base_dir);
    }

    /// Clear THIS session's dedup state (used for the bracket force-refresh).
    /// Scoped: a force-refresh in one session must not make every other session
    /// re-inject its whole domain set on its next prompt.
    pub fn clear_dedup(&mut self) {
        let prefix = format!("{}{SCOPE_SEP}", self.active_scope());
        self.injected.retain(|k, _| !k.starts_with(&prefix));
        self.standards_injected.retain(|k, _| !k.starts_with(&prefix));
    }

    /// Claim one rule for this session: true when it should be served now, false
    /// when this session has already been told it.
    ///
    /// F9. The unit of dedup is the rule, not the domain block. `scope` is the place
    /// or the action that matched, and `None` for a rule serving on its topic, on
    /// always, or through its domain's trigger — so a place rule is claimed once per
    /// place and an action rule once per action, out of the same map.
    ///
    /// Three things re-open a claim, and they are F8's table:
    ///   - the bracket changed tier, so the rules now in force are served once more;
    ///   - the rule's own text or rationale changed, so it is a different rule to read;
    ///   - a new Claude session, which has its own scope and has been told nothing.
    ///
    /// It DECIDES and RECORDS in one call, so two call sites cannot drift into disagreeing
    /// about what was served.
    pub fn claim_rule(
        &mut self,
        rule_id: &str,
        content: u64,
        tier: Bracket,
        scope: Option<&str>,
    ) -> bool {
        let now = Self::now_secs();
        if !self.rule_due(rule_id, content, tier, scope, ReShow::PerSession { on_tier_change: true }, now) {
            return false;
        }
        self.mark_rule_shown(rule_id, content, tier, scope, now);
        true
    }

    fn rule_key(&self, rule_id: &str, scope: Option<&str>) -> String {
        match scope {
            Some(s) => self.scoped(&format!("r{SCOPE_SEP}{rule_id}{SCOPE_SEP}{s}")),
            None => self.scoped(&format!("r{SCOPE_SEP}{rule_id}")),
        }
    }

    /// Whether a rule is due to be shown, recording NOTHING (F8). `domain::rules::select` asks this first and
    /// records only what it returns, so a rule the topic cap cut is never marked as shown.
    pub fn rule_due(&self, rule_id: &str, content: u64, tier: Bracket, scope: Option<&str>, reshow: ReShow, now: u64) -> bool {
        let Some(prev) = self.rules_shown.get(&self.rule_key(rule_id, scope)) else {
            return true;
        };
        if prev.content != content {
            return true;
        }
        match reshow {
            ReShow::PerSession { on_tier_change } => on_tier_change && prev.tier != tier.to_string(),
            ReShow::Throttle { secs } => now.saturating_sub(prev.at) >= secs,
        }
    }

    /// Record that a rule was shown at `now`, at this tier.
    pub fn mark_rule_shown(&mut self, rule_id: &str, content: u64, tier: Bracket, scope: Option<&str>, now: u64) {
        let key = self.rule_key(rule_id, scope);
        self.rules_shown.insert(key, ShownRule { at: now, tier: tier.to_string(), content });
    }

    /// Whether this bracket rule is still to be sent in this session, recording NOTHING (BO-03, F3).
    ///
    /// Once per session per rule: true until the rule has been printed whole in this session, then false for
    /// the rest of it, whatever the tier does. A tier change brings only the rules of the new tier that have
    /// not been sent; a rule already sent is not repeated (lynx ruled per rule over per block, 2026-10-02, so
    /// the DEPLETED and CRITICAL buckets still go out once when their tier arrives). An edited text is a new
    /// rule ([`bracket_rule_key`]). The prompt hook records a rule with [`SessionState::mark_rule_shown`] only
    /// once the block carrying it is printed (D15), so a rule the budget dropped stays due.
    ///
    /// History: K1 (Chris, 2026-09-12) took the block off every prompt and sent it once per tier; on
    /// 2026-10-01 that still sent the T1 to T6 rules, which CLAUDE.md already carries, again at each tier.
    pub fn bracket_rule_due(&self, text: &str) -> bool {
        let (id, content) = bracket_rule_key(text);
        // `on_tier_change: false` makes the tier argument irrelevant: only the text and the session decide.
        self.rule_due(&id, content, Bracket::Fresh, None, ReShow::PerSession { on_tier_change: false }, 0)
    }

    /// Whether this session found the bracket rule `text` covered by a loaded CLAUDE.md, or `None` when it has not
    /// decided yet. See [`SessionState::bracket_covered`].
    pub fn bracket_coverage(&self, text: &str) -> Option<bool> {
        self.bracket_covered.get(&self.scoped(&bracket_rule_key(text).0)).copied()
    }

    /// Record this session's coverage verdict for the bracket rule `text`.
    pub fn record_bracket_coverage(&mut self, text: &str, covered: bool) {
        let key = self.scoped(&bracket_rule_key(text).0);
        self.bracket_covered.insert(key, covered);
    }

    /// Whether this standard was already injected this session with the same
    /// rule content. Edited standards (new hash) re-inject.
    pub fn is_standard_injected(&self, id: &str, hash: u64) -> bool {
        self.standards_injected.get(&self.scoped(id)) == Some(&hash)
    }

    /// Record a standard as injected at its current content hash.
    pub fn mark_standard_injected(&mut self, id: &str, hash: u64) {
        self.standards_injected.insert(self.scoped(id), hash);
    }

    /// Whether this file's AST map was already injected this session AT ITS
    /// CURRENT content-version. A changed file (new version) returns false → re-inject.
    pub fn has_ast_injected(&self, file_path: &str, version: u64) -> bool {
        self.ast_injected.get(&self.scoped(file_path)) == Some(&version)
    }

    /// Record that a file's AST map was injected at the given content-version.
    pub fn mark_ast_injected(&mut self, file_path: &str, version: u64) {
        self.ast_injected.insert(self.scoped(file_path), version);
    }

    /// Flag an app root as edited this turn. Returns true if newly added.
    pub fn mark_dirty_app(&mut self, app_root: &str) -> bool {
        self.dirty_apps.insert(self.scoped(app_root))
    }

    /// Mark an app dirty in the state file under `base_dir`, saving only when
    /// the mark is new. See [`mark_dirty_app_global`] for why a second copy of
    /// the mark exists at all.
    pub fn mark_dirty_app_in(base_dir: &Path, app_root: &str) -> bool {
        let mut s = SessionState::load(base_dir);
        let added = s.mark_dirty_app(app_root);
        if added {
            let _ = s.save(base_dir);
        }
        added
    }

    /// Drain THIS session's dirty apps from the state file under `base_dir`.
    pub fn take_dirty_apps_in(base_dir: &Path) -> Vec<String> {
        let mut s = SessionState::load(base_dir);
        let mine = s.take_dirty_apps();
        if !mine.is_empty() {
            let _ = s.save(base_dir);
        }
        mine
    }

    /// The GLOBAL-TIER copy of the dirty mark.
    ///
    /// The pre-tool-use hook writes the mark into `find_workspace_base(cwd)`'s
    /// `.session`, and the Stop hook drains `find_workspace_base(cwd)` too — but
    /// `cwd` is whatever the session has at THAT moment, and it drifts: a Bash
    /// `cd` into the app moves it, the next tool call from the home dir moves
    /// it back. Measured 2026-09-01: a session running from `C:\Users\Chris`
    /// edited `dev/logos-wall` with cwd inside the app, the mark landed in
    /// `logos-wall/.base/.session`, the turn ended with cwd at home, the Stop
    /// hook drained `C:\Users\Chris\.base\.session`, found nothing, and the
    /// map stayed stale. One global set, still keyed per session, cannot be
    /// stranded by a cwd change.
    pub fn mark_dirty_app_global(app_root: &str) {
        if let Some(g) = crate::config::global_base_dir() {
            Self::mark_dirty_app_in(&g, app_root);
        }
    }

    /// Drain this session's global-tier dirty apps (see [`mark_dirty_app_global`]).
    pub fn take_dirty_apps_global() -> Vec<String> {
        crate::config::global_base_dir()
            .map(|g| Self::take_dirty_apps_in(&g))
            .unwrap_or_default()
    }

    /// Drain THIS session's edited-app set (the Stop hook refreshes these maps).
    /// Scoped so one session's Stop hook cannot steal another's pending refreshes.
    pub fn take_dirty_apps(&mut self) -> Vec<String> {
        let mine: Vec<String> = self
            .dirty_apps
            .iter()
            .filter(|k| self.is_own(k))
            .cloned()
            .collect();
        for key in &mine {
            self.dirty_apps.remove(key);
        }
        // Callers want the app root, not the internal scoped key.
        mine.iter()
            .filter_map(|k| k.split_once(SCOPE_SEP).map(|(_, app)| app.to_string()))
            .collect()
    }
}

/// Compute a hash of rule texts for change detection.
/// If rules change (domains.toml edited), hash differs → re-inject.
pub fn rules_hash(rules: &[String]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for rule in rules {
        rule.hash(&mut hasher);
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_bracket_config() -> BracketConfig {
        BracketConfig::default()
    }

    fn sample_rules() -> crate::config::BracketRules {
        crate::config::BracketRules {
            always: vec!["ALWAYS_A".into(), "ALWAYS_B".into()],
            fresh: vec!["FRESH_ONLY".into()],
            moderate: vec!["MOD_ONLY".into()],
            depleted: vec!["DEP_ONLY".into()],
            critical: vec!["CRIT_ONLY".into()],
        }
    }

    /// The texts of [`Bracket::entries`].
    fn texts(b: Bracket, r: &crate::config::BracketRules) -> Vec<&str> {
        b.entries(r).into_iter().map(|e| e.text.as_str()).collect()
    }

    #[test]
    fn bracket_rules_are_always_plus_tier() {
        let r = sample_rules();
        assert_eq!(texts(Bracket::Fresh, &r), vec!["ALWAYS_A", "ALWAYS_B", "FRESH_ONLY"]);
        assert_eq!(texts(Bracket::Critical, &r), vec!["ALWAYS_A", "ALWAYS_B", "CRIT_ONLY"]);
        // A tier never leaks another tier's bucket.
        assert!(!texts(Bracket::Moderate, &r).contains(&"DEP_ONLY"));
    }

    #[test]
    fn always_rules_survive_every_tier() {
        let r = sample_rules();
        // The whole point of `always`: no tier can drop it. If this breaks, the
        // rules meant to hold under context pressure silently stop being sent.
        for b in [
            Bracket::Fresh,
            Bracket::Moderate,
            Bracket::Depleted,
            Bracket::Critical,
        ] {
            let got = texts(b, &r);
            assert!(got.contains(&"ALWAYS_A"), "{b} dropped ALWAYS_A");
            assert!(got.contains(&"ALWAYS_B"), "{b} dropped ALWAYS_B");
        }
    }

    #[test]
    fn empty_rules_render_nothing() {
        let empty = crate::config::BracketRules::default();
        assert!(empty.is_empty());
        assert_eq!(render_bracket_rules(Bracket::Depleted, &texts(Bracket::Depleted, &empty)), "");
    }

    #[test]
    fn rendered_block_is_numbered_and_tier_labelled() {
        let out = render_bracket_rules(Bracket::Depleted, &texts(Bracket::Depleted, &sample_rules()));
        assert!(out.starts_with("[BRACKET RULES — DEPLETED]\n"));
        assert!(out.contains("  0. ALWAYS_A\n"));
        assert!(out.contains("  2. DEP_ONLY\n"));
    }

    #[test]
    fn percent_mode_overrides_turn_count() {
        let config = BracketConfig { mode: Some("percent".into()), ..Default::default() };
        let mut state = SessionState::default();
        // 40 prompts would be CRITICAL on turns; 10% context says FRESH.
        state.prompt_counts.insert("s1".into(), 40);
        assert_eq!(
            state.bracket_for(&config, Some("s1"), Some(10.0)),
            Bracket::Fresh
        );
        // With no reading available it falls back to the turn thresholds.
        assert_eq!(
            state.bracket_for(&config, Some("s1"), None),
            Bracket::Critical
        );
    }

    /// A state instance acting as a named session.
    fn as_session(id: &str) -> SessionState {
        let mut s = SessionState::default();
        s.set_active(Some(id));
        s
    }

    #[test]
    fn domain_dedup_is_per_session() {
        // THE BUG: session alpha injects a domain, and session beta — sharing the
        // workspace .session file — then sees it as already injected and suppresses it.
        let tmp = tempfile::tempdir().unwrap();
        let mut alpha = as_session("alpha");
        alpha.mark_injected("skyrim", 42);
        alpha.save(tmp.path()).unwrap();

        let mut beta = SessionState::load(tmp.path());
        beta.set_active(Some("beta"));
        assert!(
            !beta.is_injected("skyrim", 42),
            "beta was suppressed by alpha's injection"
        );

        // Alpha still sees its own mark.
        let mut reloaded = SessionState::load(tmp.path());
        reloaded.set_active(Some("alpha"));
        assert!(reloaded.is_injected("skyrim", 42));
    }

    #[test]
    fn standards_and_ast_dedup_are_per_session() {
        let mut alpha = as_session("alpha");
        alpha.mark_standard_injected("std-1", 7);
        alpha.mark_ast_injected("src/main.rs", 3);

        let mut beta = SessionState {
            injected: std::mem::take(&mut alpha.injected),
            standards_injected: std::mem::take(&mut alpha.standards_injected),
            ast_injected: std::mem::take(&mut alpha.ast_injected),
            ..Default::default()
        };
        beta.set_active(Some("beta"));

        assert!(!beta.is_standard_injected("std-1", 7));
        assert!(!beta.has_ast_injected("src/main.rs", 3));
    }

    #[test]
    fn force_refresh_clears_only_the_calling_session() {
        let mut alpha = as_session("alpha");
        alpha.mark_injected("shared-domain", 1);
        let stash = std::mem::take(&mut alpha.injected);

        let mut beta = SessionState { injected: stash, ..Default::default() };
        beta.set_active(Some("beta"));
        beta.mark_injected("shared-domain", 1);
        beta.clear_dedup();

        // Beta's own mark is gone; alpha's survives its neighbour's refresh.
        assert!(!beta.is_injected("shared-domain", 1));
        beta.set_active(Some("alpha"));
        assert!(
            beta.is_injected("shared-domain", 1),
            "alpha's dedup was wiped by beta's force-refresh"
        );
    }

    #[test]
    fn dirty_apps_are_per_session_and_return_unscoped_paths() {
        let mut alpha = as_session("alpha");
        alpha.mark_dirty_app("/repo/app-a");
        let stash = std::mem::take(&mut alpha.dirty_apps);

        let mut beta = SessionState { dirty_apps: stash, ..Default::default() };
        beta.set_active(Some("beta"));
        beta.mark_dirty_app("/repo/app-b");

        // Beta drains only its own, and gets a usable path back, not a scoped key.
        let drained = beta.take_dirty_apps();
        assert_eq!(drained, vec!["/repo/app-b".to_string()]);
        assert_eq!(beta.dirty_apps.len(), 1, "alpha's pending refresh was stolen");
    }

    #[test]
    fn session_start_clear_does_not_disturb_neighbours() {
        let tmp = tempfile::tempdir().unwrap();
        let mut alpha = as_session("alpha");
        alpha.mark_injected("dom", 9);
        alpha.increment_prompt_for(Some("alpha"));
        alpha.save(tmp.path()).unwrap();

        SessionState::clear_for(tmp.path(), Some("beta"));

        let mut reloaded = SessionState::load(tmp.path());
        reloaded.set_active(Some("alpha"));
        assert!(reloaded.is_injected("dom", 9), "beta's start wiped alpha");
        assert_eq!(reloaded.prompt_count_for(Some("alpha")), 1);
    }

    #[test]
    fn dead_sessions_are_pruned() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = SessionState::default();
        // More sessions than the cap, each with dedup entries.
        for i in 0..(MAX_TRACKED_SESSIONS + 5) {
            let id = format!("sess-{i}");
            state.set_active(Some(&id));
            state.mark_injected("dom", i as u64);
            state.last_seen.insert(id, i as u64);
        }
        state.save(tmp.path()).unwrap();

        let loaded = SessionState::load(tmp.path());
        assert!(
            loaded.last_seen.len() <= MAX_TRACKED_SESSIONS + 1,
            "unbounded growth: {} tracked",
            loaded.last_seen.len()
        );
        // The oldest session's dedup entry went with it.
        assert!(!loaded.injected.keys().any(|k| k.starts_with("sess-0\u{1}")));
    }

    #[test]
    fn concurrent_sessions_do_not_share_a_counter() {
        let mut state = SessionState::default();
        for _ in 0..5 {
            state.increment_prompt_for(Some("alpha"));
        }
        state.increment_prompt_for(Some("beta"));
        // The bug this fixes: beta's SessionStart or prompts moving alpha's count.
        assert_eq!(state.prompt_count_for(Some("alpha")), 5);
        assert_eq!(state.prompt_count_for(Some("beta")), 1);
    }

    #[test]
    fn clearing_one_session_leaves_the_other_intact() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = SessionState::default();
        state.increment_prompt_for(Some("alpha"));
        state.increment_prompt_for(Some("alpha"));
        state.increment_prompt_for(Some("beta"));
        state.save(tmp.path()).unwrap();

        SessionState::clear_for(tmp.path(), Some("beta"));

        let loaded = SessionState::load(tmp.path());
        assert_eq!(loaded.prompt_count_for(Some("alpha")), 2, "alpha was reset");
        assert_eq!(loaded.prompt_count_for(Some("beta")), 0);
    }

    #[test]
    fn session_state_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = SessionState::default();
        state.mark_injected("global", 12345);
        state.save(tmp.path()).unwrap();

        let loaded = SessionState::load(tmp.path());
        assert!(loaded.is_injected("global", 12345));
        assert!(!loaded.is_injected("global", 99999));
        assert!(!loaded.is_injected("other", 12345));
    }

    #[test]
    fn session_state_clear() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = SessionState::default();
        state.mark_injected("test", 111);
        state.save(tmp.path()).unwrap();

        SessionState::clear(tmp.path());
        let loaded = SessionState::load(tmp.path());
        assert!(loaded.injected.is_empty());
    }

    #[test]
    fn rules_hash_changes_on_content() {
        let h1 = rules_hash(&["Rule A".into(), "Rule B".into()]);
        let h2 = rules_hash(&["Rule A".into(), "Rule C".into()]);
        let h3 = rules_hash(&["Rule A".into(), "Rule B".into()]);
        assert_ne!(h1, h2);
        assert_eq!(h1, h3);
    }

    #[test]
    fn prompt_count_increments() {
        let mut state = SessionState::default();
        assert_eq!(state.prompt_count, 0);
        assert_eq!(state.increment_prompt(), 1);
        assert_eq!(state.increment_prompt(), 2);
        assert_eq!(state.prompt_count, 2);
    }

    #[test]
    fn prompt_count_persists_across_save_load() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = SessionState::default();
        state.increment_prompt();
        state.increment_prompt();
        state.increment_prompt();
        state.save(tmp.path()).unwrap();

        let loaded = SessionState::load(tmp.path());
        assert_eq!(loaded.prompt_count, 3);
    }

    #[test]
    fn bracket_transitions_at_thresholds() {
        let cfg = default_bracket_config(); // fresh_until=3, moderate=10, depleted=20
        let mut state = SessionState::default();

        // prompt 0 → FRESH
        assert_eq!(state.bracket(&cfg), Bracket::Fresh);

        // prompts 1-3 → FRESH
        state.prompt_count = 1;
        assert_eq!(state.bracket(&cfg), Bracket::Fresh);
        state.prompt_count = 3;
        assert_eq!(state.bracket(&cfg), Bracket::Fresh);

        // prompt 4 → MODERATE
        state.prompt_count = 4;
        assert_eq!(state.bracket(&cfg), Bracket::Moderate);
        state.prompt_count = 10;
        assert_eq!(state.bracket(&cfg), Bracket::Moderate);

        // prompt 11 → DEPLETED
        state.prompt_count = 11;
        assert_eq!(state.bracket(&cfg), Bracket::Depleted);
        state.prompt_count = 20;
        assert_eq!(state.bracket(&cfg), Bracket::Depleted);

        // prompt 21 → CRITICAL
        state.prompt_count = 21;
        assert_eq!(state.bracket(&cfg), Bracket::Critical);
        state.prompt_count = 100;
        assert_eq!(state.bracket(&cfg), Bracket::Critical);
    }

    #[test]
    fn bracket_disabled_returns_moderate() {
        let mut cfg = default_bracket_config();
        cfg.enabled = false;
        let mut state = SessionState { prompt_count: 1, ..Default::default() };
        assert_eq!(state.bracket(&cfg), Bracket::Moderate);
        state.prompt_count = 50;
        assert_eq!(state.bracket(&cfg), Bracket::Moderate);
    }

    #[test]
    fn force_refresh_on_depleted_interval() {
        let cfg = default_bracket_config(); // refresh_interval=5, depleted_until=20
        let mut state = SessionState { prompt_count: 3, ..Default::default() };

        // FRESH — no refresh
        assert!(!state.should_force_refresh(&cfg));

        // MODERATE — no refresh
        state.prompt_count = 10;
        assert!(!state.should_force_refresh(&cfg));

        // DEPLETED, not on interval
        state.prompt_count = 11;
        assert!(!state.should_force_refresh(&cfg));

        // DEPLETED, on interval (15 % 5 == 0)
        state.prompt_count = 15;
        assert!(state.should_force_refresh(&cfg));

        // CRITICAL, on interval (25 % 5 == 0)
        state.prompt_count = 25;
        assert!(state.should_force_refresh(&cfg));

        // CRITICAL, not on interval
        state.prompt_count = 23;
        assert!(!state.should_force_refresh(&cfg));
    }

    #[test]
    fn clear_dedup_empties_injected() {
        let mut state = SessionState::default();
        state.mark_injected("a", 1);
        state.mark_injected("b", 2);
        assert!(!state.injected.is_empty());
        state.clear_dedup();
        assert!(state.injected.is_empty());
    }
}
