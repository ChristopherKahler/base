//! Which tier a command acts on, and what it actually changed.
//!
//! Four issues (#52, #53, #55, #18) are one seam. Reads merged both tiers,
//! writes picked one and never said which, and the message was printed from the
//! command's INTENT rather than from what changed on disk. So `domain remove`
//! said "not found" about a domain that exists (it read the other tier's file
//! and exited 0), `domain create` said "created" and wrote to a tier the user
//! was not standing in, `rule remove` said "removed" with nothing removed, and
//! `domain remove-trigger` reported success while editing a *different* domain
//! that merely shared a name.
//!
//! One resolver decides the tier. One outcome type carries what changed. The
//! CLI prints from the outcome, never from the intent.

use std::path::{Path, PathBuf};

/// Which store a command is acting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// `{workspace}/.base/`
    Workspace,
    /// `~/.base-gbl/`
    Global,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Tier::Workspace => "workspace",
            Tier::Global => "global",
        }
    }

    /// The other one, for "not found here, also checked there" messages.
    pub fn other(self) -> Tier {
        match self {
            Tier::Workspace => Tier::Global,
            Tier::Global => Tier::Workspace,
        }
    }
}

/// What a write actually did.
///
/// `count == 0` means nothing changed, and the caller must say so and exit
/// non-zero. That is the whole of #52, #55 and #18's false success: every one of
/// them printed a success sentence that no outcome had been consulted for.
#[derive(Debug, Clone, Copy)]
pub struct Changed {
    pub tier: Tier,
    pub count: usize,
}

impl Changed {
    pub fn none(tier: Tier) -> Self {
        Changed { tier, count: 0 }
    }
    pub fn is_noop(&self) -> bool {
        self.count == 0
    }
}

/// The single place that decides which `domains.toml` a WRITE touches.
///
/// `--global` forces the global tier. Otherwise the workspace is resolved the
/// way `add_trigger` always did, and the global tier is used only when there is
/// no workspace to stand in. Before this, one write command of four resolved the
/// workspace and the other three hardcoded global, so a trigger added in a
/// workspace could not be removed from the command line (#18).
pub fn domains_toml_for_write(cwd: &Path, global: bool) -> (PathBuf, Tier) {
    if global {
        return (global_domains_toml(), Tier::Global);
    }
    match crate::config::find_workspace_base(cwd) {
        Some(base) => (base.join("domains.toml"), Tier::Workspace),
        None => (global_domains_toml(), Tier::Global),
    }
}

/// Where a tier's `domains.toml` lives, for reads that want a specific one.
pub fn domains_toml_for(cwd: &Path, tier: Tier) -> Option<PathBuf> {
    match tier {
        Tier::Global => Some(global_domains_toml()),
        Tier::Workspace => {
            crate::config::find_workspace_base(cwd).map(|b| b.join("domains.toml"))
        }
    }
}

/// Where a tier's `graph.nq` lives.
pub fn graph_for(cwd: &Path, tier: Tier) -> Option<PathBuf> {
    match tier {
        Tier::Global => crate::home::home_root()
            .map(|h| h.join(".base-gbl").join(".base").join("graph.nq")),
        Tier::Workspace => {
            crate::config::find_workspace_base(cwd).map(|b| b.join("graph.nq"))
        }
    }
}

fn global_domains_toml() -> PathBuf {
    crate::home::home_root()
        .unwrap_or_default()
        .join(".base-gbl")
        .join("domains.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_is_forced_by_the_flag_wherever_you_stand() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".base")).unwrap();
        let (_, tier) = domains_toml_for_write(d.path(), true);
        assert_eq!(tier, Tier::Global, "--global must win over a workspace");
    }

    #[test]
    fn a_workspace_wins_when_you_are_standing_in_one() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".base")).unwrap();
        let (path, tier) = domains_toml_for_write(d.path(), false);
        assert_eq!(tier, Tier::Workspace, "a workspace was ignored (#18)");
        assert!(path.ends_with(".base/domains.toml"), "{path:?}");
    }

    #[test]
    fn the_other_tier_is_named_for_the_not_found_message() {
        assert_eq!(Tier::Workspace.other(), Tier::Global);
        assert_eq!(Tier::Global.other(), Tier::Workspace);
    }

    #[test]
    fn a_change_of_zero_is_a_noop_and_says_so() {
        assert!(Changed::none(Tier::Workspace).is_noop());
        assert!(!Changed { tier: Tier::Global, count: 1 }.is_noop());
    }
}
