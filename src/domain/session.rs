use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::BracketConfig;

// ─── Context Bracket ────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bracket {
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
    /// Rules to inject at this tier: `always` first, then the tier's own bucket.
    ///
    /// Additive rather than exclusive — a DEPLETED prompt gets `always` + `depleted`.
    /// `always` leads so the permanent rules keep a stable position in the block
    /// regardless of tier, which matters for a rule that is re-read every prompt.
    pub fn rules<'a>(&self, rules: &'a crate::config::BracketRules) -> Vec<&'a str> {
        let tier = match self {
            Self::Fresh => &rules.fresh,
            Self::Moderate => &rules.moderate,
            Self::Depleted => &rules.depleted,
            Self::Critical => &rules.critical,
        };
        rules
            .always
            .iter()
            .chain(tier.iter())
            .map(String::as_str)
            .collect()
    }
}

/// Render the bracket's rules as an injectable block. Empty string when the tier
/// contributes nothing, so the hook can push it unconditionally.
pub fn format_bracket_rules(bracket: Bracket, rules: &crate::config::BracketRules) -> String {
    let selected = bracket.rules(rules);
    if selected.is_empty() {
        return String::new();
    }
    let mut out = format!("[BRACKET RULES — {bracket}]\n");
    for (i, rule) in selected.iter().enumerate() {
        out.push_str(&format!("  {i}. {rule}\n"));
    }
    out.push('\n');
    out
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

#[derive(Debug, Default, Serialize, Deserialize)]
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

    fn now_secs() -> u64 {
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

        if config.is_percent_mode() {
            if let Some(pct) = context_pct {
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

    #[test]
    fn bracket_rules_are_always_plus_tier() {
        let r = sample_rules();
        assert_eq!(
            Bracket::Fresh.rules(&r),
            vec!["ALWAYS_A", "ALWAYS_B", "FRESH_ONLY"]
        );
        assert_eq!(
            Bracket::Critical.rules(&r),
            vec!["ALWAYS_A", "ALWAYS_B", "CRIT_ONLY"]
        );
        // A tier never leaks another tier's bucket.
        assert!(!Bracket::Moderate.rules(&r).contains(&"DEP_ONLY"));
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
            let got = b.rules(&r);
            assert!(got.contains(&"ALWAYS_A"), "{b} dropped ALWAYS_A");
            assert!(got.contains(&"ALWAYS_B"), "{b} dropped ALWAYS_B");
        }
    }

    #[test]
    fn empty_rules_render_nothing() {
        let empty = crate::config::BracketRules::default();
        assert!(empty.is_empty());
        assert_eq!(format_bracket_rules(Bracket::Depleted, &empty), "");
    }

    #[test]
    fn rendered_block_is_numbered_and_tier_labelled() {
        let out = format_bracket_rules(Bracket::Depleted, &sample_rules());
        assert!(out.starts_with("[BRACKET RULES — DEPLETED]\n"));
        assert!(out.contains("  0. ALWAYS_A\n"));
        assert!(out.contains("  2. DEP_ONLY\n"));
    }

    #[test]
    fn percent_mode_overrides_turn_count() {
        let mut config = BracketConfig::default();
        config.mode = Some("percent".into());
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

        let mut beta = SessionState::default();
        beta.injected = std::mem::take(&mut alpha.injected);
        beta.standards_injected = std::mem::take(&mut alpha.standards_injected);
        beta.ast_injected = std::mem::take(&mut alpha.ast_injected);
        beta.set_active(Some("beta"));

        assert!(!beta.is_standard_injected("std-1", 7));
        assert!(!beta.has_ast_injected("src/main.rs", 3));
    }

    #[test]
    fn force_refresh_clears_only_the_calling_session() {
        let mut alpha = as_session("alpha");
        alpha.mark_injected("shared-domain", 1);
        let stash = std::mem::take(&mut alpha.injected);

        let mut beta = SessionState::default();
        beta.injected = stash;
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

        let mut beta = SessionState::default();
        beta.dirty_apps = stash;
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
