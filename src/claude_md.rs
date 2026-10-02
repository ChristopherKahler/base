//! The CLAUDE.md files Claude Code loads into a session, read so base does not send what they already carry
//! (BO-03, F3).
//!
//! WHY. base's bracket block restated, in its own words, rules the operator's `~/.claude/CLAUDE.md` already
//! held: on 2026-10-01 the T1 to T6 bracket rules took about 2,400 of 3,864 bytes of a prompt in a session
//! where Claude Code had loaded the same rules from CLAUDE.md at launch. A bracket rule now names the
//! CLAUDE.md text that covers it (`covered_by`, [`crate::config::BracketRule`]), and this module answers
//! whether that text is loaded.
//!
//! WHICH FILES, checked against Claude Code's memory documentation (code.claude.com/docs/en/memory, read
//! 2026-10-02): at launch Claude Code loads the managed-policy CLAUDE.md, the user's `~/.claude/CLAUDE.md`,
//! and `CLAUDE.md`, `.claude/CLAUDE.md` and `CLAUDE.local.md` in the working folder and every folder above
//! it. `CLAUDE_CONFIG_DIR`, when set, replaces `~/.claude` (docs/en/env-vars). These are the files read here.
//!
//! NOT READ, and each one errs toward SENDING a rule, never toward dropping one: `@path` imports, rules
//! files under `.claude/rules/`, and CLAUDE.md files in subfolders (loaded on demand when Claude reads a file
//! there). A marker found only in one of those is treated as absent, so the rule is still sent. One case errs
//! the other way and is named rather than handled: a file a `claudeMdExcludes` setting keeps out of the
//! session is still read here.

use std::path::{Path, PathBuf};

/// The managed-policy CLAUDE.md for this platform, per Claude Code's documentation.
fn managed_policy() -> Option<PathBuf> {
    // A test build never reads a machine-wide file: what it holds is outside the test's control.
    if cfg!(feature = "isolation-guard") {
        return None;
    }
    if cfg!(windows) {
        Some(PathBuf::from(r"C:\Program Files\ClaudeCode\CLAUDE.md"))
    } else if cfg!(target_os = "macos") {
        Some(PathBuf::from("/Library/Application Support/ClaudeCode/CLAUDE.md"))
    } else {
        Some(PathBuf::from("/etc/claude-code/CLAUDE.md"))
    }
}

/// The user's CLAUDE.md: `$CLAUDE_CONFIG_DIR/CLAUDE.md` when that is set, else `~/.claude/CLAUDE.md`.
///
/// A test build ignores `CLAUDE_CONFIG_DIR`, as it ignores the real home: the variable belongs to the machine
/// running the tests, and a developer who sets it would otherwise feed their own CLAUDE.md into every test.
fn user_file() -> Option<PathBuf> {
    if !cfg!(feature = "isolation-guard")
        && let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty())
    {
        return Some(PathBuf::from(dir).join("CLAUDE.md"));
    }
    crate::home::home_root().map(|h| h.join(".claude").join("CLAUDE.md"))
}

/// The folders from the drive root down to `cwd`, in the order Claude Code reads them.
fn folders_down_to(cwd: &Path) -> Vec<PathBuf> {
    let mut up: Vec<PathBuf> = Vec::new();
    let mut dir = cwd.to_path_buf();
    loop {
        // A test's walk stops where its sandbox ends, as workspace resolution does: on Windows the walk up
        // from a temp folder passes through the user's own home, which may hold a real CLAUDE.md.
        #[cfg(feature = "isolation-guard")]
        if !crate::home::within_sandbox(&dir) {
            break;
        }
        up.push(dir.clone());
        if !dir.pop() {
            break;
        }
    }
    up.reverse();
    up
}

/// The CLAUDE.md files Claude Code loads at launch for a session working in `cwd`, in its order, existing
/// files only.
pub fn loaded_files(cwd: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut add = |p: PathBuf| {
        if p.is_file() && !out.contains(&p) {
            out.push(p);
        }
    };
    if let Some(p) = managed_policy() {
        add(p);
    }
    if let Some(p) = user_file() {
        add(p);
    }
    for dir in folders_down_to(cwd) {
        add(dir.join("CLAUDE.md"));
        add(dir.join(".claude").join("CLAUDE.md"));
        add(dir.join("CLAUDE.local.md"));
    }
    out
}

/// The text of every file [`loaded_files`] finds, joined. A file that cannot be read adds nothing, which
/// errs toward sending a rule.
pub fn loaded_text(cwd: &Path) -> String {
    loaded_files(cwd)
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_run_from_the_top_down_to_the_working_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let deep = tmp.path().join("a").join("b");
        std::fs::create_dir_all(&deep).unwrap();
        let folders = folders_down_to(&deep);
        assert_eq!(folders.last().map(PathBuf::as_path), Some(deep.as_path()), "ends at the working folder");
        let at = |p: &Path| folders.iter().position(|f| f == p).unwrap();
        assert!(at(tmp.path()) < at(&tmp.path().join("a")), "a parent comes before its child: {folders:?}");
    }
}
