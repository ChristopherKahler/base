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
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SessionState {
    /// domain name → rules hash (for change detection)
    #[serde(default)]
    pub injected: HashMap<String, u64>,
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
    pub fn load(base_dir: &Path) -> Self {
        let path = base_dir.join(".session");
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
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

    /// Check if a domain was already injected with the same rules hash.
    pub fn is_injected(&self, domain: &str, hash: u64) -> bool {
        self.injected.get(domain) == Some(&hash)
    }

    /// Mark a domain as injected with its current rules hash.
    pub fn mark_injected(&mut self, domain: &str, hash: u64) {
        self.injected.insert(domain.to_string(), hash);
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

    /// Clear state for ONE session, leaving concurrent sessions' counters intact.
    ///
    /// SessionStart previously called `clear()`, deleting the shared file — so a
    /// new terminal reset every other live session's bracket to FRESH. Dedup maps
    /// are still cleared workspace-wide, which is the pre-existing behavior a fresh
    /// session depends on for a full re-injection.
    pub fn clear_for(base_dir: &Path, session_id: Option<&str>) {
        let Some(id) = session_id else {
            Self::clear(base_dir);
            return;
        };
        let mut state = Self::load(base_dir);
        state.prompt_counts.remove(id);
        state.prompt_count = 0;
        state.injected.clear();
        state.standards_injected.clear();
        state.ast_injected.clear();
        state.dirty_apps.clear();
        let _ = state.save(base_dir);
    }

    /// Clear all dedup state (used for force-refresh).
    pub fn clear_dedup(&mut self) {
        self.injected.clear();
        self.standards_injected.clear();
    }

    /// Whether this standard was already injected this session with the same
    /// rule content. Edited standards (new hash) re-inject.
    pub fn is_standard_injected(&self, id: &str, hash: u64) -> bool {
        self.standards_injected.get(id) == Some(&hash)
    }

    /// Record a standard as injected at its current content hash.
    pub fn mark_standard_injected(&mut self, id: &str, hash: u64) {
        self.standards_injected.insert(id.to_string(), hash);
    }

    /// Whether this file's AST map was already injected this session AT ITS
    /// CURRENT content-version. A changed file (new version) returns false → re-inject.
    pub fn has_ast_injected(&self, file_path: &str, version: u64) -> bool {
        self.ast_injected.get(file_path) == Some(&version)
    }

    /// Record that a file's AST map was injected at the given content-version.
    pub fn mark_ast_injected(&mut self, file_path: &str, version: u64) {
        self.ast_injected.insert(file_path.to_string(), version);
    }

    /// Flag an app root as edited this turn. Returns true if newly added.
    pub fn mark_dirty_app(&mut self, app_root: &str) -> bool {
        self.dirty_apps.insert(app_root.to_string())
    }

    /// Drain the edited-app set (the Stop hook refreshes these maps).
    pub fn take_dirty_apps(&mut self) -> Vec<String> {
        self.dirty_apps.drain().collect()
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
