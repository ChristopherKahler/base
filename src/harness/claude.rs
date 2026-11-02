//! Claude Code — the dialect the engine was written in.
//!
//! Nothing to translate. This module exists so `Harness::Claude` is not a
//! special case in every `match`: it is an adapter whose adapter is the identity.

use std::path::{Path, PathBuf};

use super::{Binding, Harness, WireOutcome};

/// The engine already reads and writes Claude Code's payloads.
pub fn inbound(raw: serde_json::Value) -> serde_json::Value {
    raw
}

/// Plain text and the `hookSpecificOutput` envelope are exactly what Claude
/// Code reads, so the engine's output is passed through byte for byte.
pub fn outbound(text: &str) -> Option<String> {
    Some(text.to_string())
}

pub struct ClaudeBinding;

pub static BINDING: ClaudeBinding = ClaudeBinding;

impl Binding for ClaudeBinding {
    fn harness(&self) -> Harness {
        Harness::Claude
    }

    fn detected(&self, home: &Path) -> bool {
        let path = self.config_path(home);
        path.exists() || path.parent().is_some_and(|d| d.is_dir())
    }

    fn config_path(&self, home: &Path) -> PathBuf {
        home.join(".claude").join("settings.json")
    }

    /// Wired by `base install` step 3, not by the adapter loop: Claude Code's
    /// wiring has a deferred path (steps 7 and 8 create `~/.claude` for the
    /// bundled skill, then step 3 retries) that the other hosts do not need and
    /// that would be lost in a generic loop. Calling this directly still works.
    fn wire(&self, home: &Path, _exe: &Path) -> anyhow::Result<WireOutcome> {
        let path = self.config_path(home);
        let added = crate::install::wire_hooks_quiet(&path)?;
        Ok(WireOutcome {
            path,
            changed: added.len(),
            note: if added.is_empty() {
                "already wired".to_string()
            } else {
                format!("added {}", added.join(", "))
            },
        })
    }

    /// Same reason: `base uninstall` step 1 removes Claude's hooks, with the
    /// whole-file rewrite its JSON merger has always done.
    fn unwire(&self, home: &Path) -> anyhow::Result<WireOutcome> {
        let path = self.config_path(home);
        crate::install::remove_hooks_quiet(&path)?;
        Ok(WireOutcome::unchanged(path, "removed by `base uninstall` step 1"))
    }
}
