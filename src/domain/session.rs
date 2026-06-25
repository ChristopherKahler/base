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

// ─── Session State ──────────────────────────────────────────

/// Tracks which domains have been injected in the current session.
/// Stored at `.base/.session` (JSON). Session-start clears it.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SessionState {
    /// domain name → rules hash (for change detection)
    #[serde(default)]
    pub injected: HashMap<String, u64>,
    /// Number of user prompts in this session (for bracket calculation)
    #[serde(default)]
    pub prompt_count: u32,
    /// File path → content-version of the AST map last injected this session.
    /// Keyed on content (not just path) so a file that CHANGES re-injects fresh
    /// context, while an unchanged re-touch stays deduped.
    #[serde(default)]
    pub ast_injected: HashMap<String, u64>,
    /// App roots whose files were edited this turn — the Stop hook refreshes
    /// exactly these code maps (not just the session-cwd app), then clears the set.
    #[serde(default)]
    pub dirty_apps: HashSet<String>,
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
    pub fn increment_prompt(&mut self) -> u32 {
        self.prompt_count += 1;
        self.prompt_count
    }

    /// Derive context bracket from prompt count and config thresholds.
    pub fn bracket(&self, config: &BracketConfig) -> Bracket {
        if !config.enabled {
            return Bracket::Moderate; // default when brackets disabled
        }
        if self.prompt_count <= config.fresh_until {
            Bracket::Fresh
        } else if self.prompt_count <= config.moderate_until {
            Bracket::Moderate
        } else if self.prompt_count <= config.depleted_until {
            Bracket::Depleted
        } else {
            Bracket::Critical
        }
    }

    /// Whether to force-refresh dedup (re-inject all domains) this prompt.
    /// True when DEPLETED or CRITICAL AND prompt lands on the refresh interval.
    pub fn should_force_refresh(&self, config: &BracketConfig) -> bool {
        if !config.enabled || config.refresh_interval == 0 {
            return false;
        }
        let bracket = self.bracket(config);
        matches!(bracket, Bracket::Depleted | Bracket::Critical)
            && self.prompt_count.is_multiple_of(config.refresh_interval)
    }

    /// Clear all dedup state (used for force-refresh).
    pub fn clear_dedup(&mut self) {
        self.injected.clear();
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
