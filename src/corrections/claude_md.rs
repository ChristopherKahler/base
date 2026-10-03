//! C3 for every user (Round 2, D10): one line in the user's CLAUDE.md asks the AI to mark a corrected reply with
//! `CORRECTED:`, so the marker detector has something to read on a machine without Chris's T2 rules.
//!
//! - `base install` and `base scaffold` offer to add the line ([`offer`]). It goes just above the `## BASE CLI`
//!   heading, outside the span `install::refresh_claude_md_section` rewrites, so a refresh never removes it.
//! - A CLAUDE.md that already asks for a marker is left alone and never asked about: one holding the line,
//!   `CORRECTED:`, or `UPDATED:` (Chris's T2, which D10 keeps as it is).
//! - Declined, or never asked (an upgrade that never ran `base install`): session start carries the same line, once
//!   per session ([`session_start_line`]).

use std::path::{Path, PathBuf};

/// The line, word for word as BO-15's scope gives it.
pub const LINE: &str = "When the user corrects you, start that reply with \"CORRECTED: <what you got wrong>\".";

/// Where the user's answer is kept: `added` or `declined`. A file, not config: it is a record of a question asked,
/// and `base install --corrections-line` changes it.
const ANSWER_FILE: &str = ".corrections-line";

/// Does this CLAUDE.md text already ask the AI to mark a corrected reply?
pub fn covered(text: &str) -> bool {
    text.contains(LINE) || text.contains("CORRECTED:") || text.contains("UPDATED:")
}

/// The line for session start, when no CLAUDE.md Claude Code loads for `cwd` asks for a marker. `None` when one does,
/// or when `[corrections] enabled = false`.
pub fn session_start_line(config: &crate::config::BaseConfig, cwd: &Path) -> Option<&'static str> {
    (config.corrections.enabled && !covered(&crate::claude_md::loaded_text(cwd))).then_some(LINE)
}

fn answer_path() -> Option<PathBuf> {
    crate::home::home_root().map(|h| h.join(".base-gbl").join(ANSWER_FILE))
}

/// The answer given before, if any: `added` or `declined`.
pub fn answered() -> Option<String> {
    let text = std::fs::read_to_string(answer_path()?).ok()?;
    let a = text.trim();
    (!a.is_empty()).then(|| a.to_string())
}

fn record(answer: &str) {
    if let Some(p) = answer_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, format!("{answer}\n"));
    }
}

/// An answer to `[Y/n]`: empty, `y` or `yes` is yes; anything else is no.
pub fn answer_is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_lowercase().as_str(), "" | "y" | "yes")
}

/// Put [`LINE`] into `path` (created when missing): just above the `## BASE CLI` heading, or at the end when there is
/// no such heading. A file that uses CRLF keeps CRLF. Returns false when the line is already there.
pub fn insert_line(path: &Path) -> std::io::Result<bool> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    if content.contains(LINE) {
        return Ok(false);
    }
    let nl = if content.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out = String::with_capacity(content.len() + LINE.len() + 4);
    match heading_start(&content) {
        Some(at) => {
            out.push_str(&content[..at]);
            out.push_str(LINE);
            out.push_str(nl);
            out.push_str(nl);
            out.push_str(&content[at..]);
        }
        None => {
            out.push_str(&content);
            if !content.is_empty() && !content.ends_with('\n') {
                out.push_str(nl);
            }
            if !content.trim().is_empty() {
                out.push_str(nl);
            }
            out.push_str(LINE);
            out.push_str(nl);
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("md.tmp");
    std::fs::write(&tmp, out)?;
    std::fs::rename(&tmp, path)?;
    Ok(true)
}

/// The byte offset of the line that starts `## BASE CLI`, the heading `install` writes.
fn heading_start(content: &str) -> Option<usize> {
    let mut pos = 0usize;
    for line in content.split_inclusive('\n') {
        if line.trim_start_matches('\u{feff}').starts_with("## BASE CLI") {
            return Some(pos);
        }
        pos += line.len();
    }
    None
}

/// `content` without [`LINE`] and the one blank line [`insert_line`] put after it. For `base uninstall`.
pub fn remove_line(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut lines = content.split_inclusive('\n').peekable();
    while let Some(line) = lines.next() {
        if line.trim_end_matches(['\r', '\n']) == LINE {
            if lines.peek().is_some_and(|next| next.trim().is_empty()) {
                lines.next();
            }
            continue;
        }
        out.push_str(line);
    }
    out
}

/// What `base install` or `base scaffold` was told to do about the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Ask on a terminal, unless it was answered before; without a terminal, do nothing.
    Ask,
    /// `--corrections-line`.
    Yes,
    /// `--no-corrections-line`.
    No,
}

/// What [`offer`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offered {
    /// The user's CLAUDE.md already asks for a marker. Nothing asked, nothing written.
    Covered,
    /// The line was written into this file.
    Added(PathBuf),
    /// No.
    Declined,
    /// Answered before (`added` or `declined`), so not asked again.
    AnsweredBefore(String),
    /// No terminal to ask on and no flag: nothing written, nothing recorded; session start carries the line.
    NotAsked,
    /// The write failed.
    Failed(String),
}

/// Offer the line for the user's CLAUDE.md (`$CLAUDE_CONFIG_DIR/CLAUDE.md` or `~/.claude/CLAUDE.md`), as `choice` says.
/// `ask` asks the question and returns the answer; the caller passes one that reads the terminal.
pub fn offer(choice: Choice, ask: impl FnOnce(&Path) -> bool) -> Offered {
    let Some(path) = crate::claude_md::user_file() else {
        return Offered::Failed("no home folder to find CLAUDE.md in".to_string());
    };
    if covered(&std::fs::read_to_string(&path).unwrap_or_default()) {
        return Offered::Covered;
    }
    let yes = match choice {
        Choice::Yes => true,
        Choice::No => false,
        Choice::Ask => {
            if let Some(before) = answered() {
                return Offered::AnsweredBefore(before);
            }
            if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                return Offered::NotAsked;
            }
            ask(&path)
        }
    };
    if !yes {
        record("declined");
        return Offered::Declined;
    }
    match insert_line(&path) {
        Ok(_) => {
            record("added");
            Offered::Added(path)
        }
        Err(e) => Offered::Failed(format!("{}: {e}", path.display())),
    }
}

/// The question on a terminal, as Example 5 words it. Anything that is not a yes is a no.
pub fn ask_on_terminal(path: &Path) -> bool {
    use std::io::Write as _;
    let shown = crate::home::home_root()
        .and_then(|h| path.strip_prefix(&h).ok().map(|rel| format!("~/{}", rel.display().to_string().replace('\\', "/"))))
        .unwrap_or_else(|| path.display().to_string());
    print!("Add one line to {shown} so base can learn from your corrections? [Y/n] ");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    answer_is_yes(&answer)
}

/// One line for an installer's progress list, from what [`offer`] did.
pub fn report(offered: &Offered) -> String {
    match offered {
        Offered::Covered => "· Corrections line: CLAUDE.md already asks the AI to mark a corrected reply".to_string(),
        Offered::Added(p) => format!("✓ Corrections line → {}", p.display()),
        Offered::Declined => "⊘ Corrections line skipped; session start carries it instead".to_string(),
        Offered::AnsweredBefore(a) => format!("· Corrections line: answered before ({a})"),
        Offered::NotAsked => {
            "⊘ Corrections line not written (no terminal to ask on); session start carries it. Add it: base install --corrections-line".to_string()
        }
        Offered::Failed(why) => format!("⊘ Corrections line not written: {why}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covered_by_the_line_or_either_marker() {
        assert!(covered(LINE));
        assert!(covered("Print exactly one of UPDATED: or MISREAD: when you change position."));
        assert!(covered("start with CORRECTED: and say what changed"));
        assert!(!covered("## BASE CLI\nUse base recall."));
    }

    #[test]
    fn the_line_goes_above_the_base_cli_heading_and_comes_out_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("CLAUDE.md");
        let before = "# My rules\r\n\r\nBe brief.\r\n\r\n## BASE CLI — Proactive Context Engine\r\n\r\nbase is on PATH.\r\n";
        std::fs::write(&path, before).unwrap();
        assert!(insert_line(&path).unwrap());
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains(&format!("Be brief.\r\n\r\n{LINE}\r\n\r\n## BASE CLI")), "{after:?}");
        assert!(!insert_line(&path).unwrap(), "a second insert adds nothing");
        assert_eq!(remove_line(&after), before);
    }

    #[test]
    fn with_no_section_the_line_goes_at_the_end() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("CLAUDE.md");
        std::fs::write(&path, "Be brief.").unwrap();
        insert_line(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), format!("Be brief.\n\n{LINE}\n"));
        let fresh = dir.path().join("sub").join("CLAUDE.md");
        insert_line(&fresh).unwrap();
        assert_eq!(std::fs::read_to_string(&fresh).unwrap(), format!("{LINE}\n"));
    }

    #[test]
    fn answers() {
        for yes in ["", "y", "Y", "yes", " YES "] {
            assert!(answer_is_yes(yes), "{yes:?}");
        }
        for no in ["n", "no", "nope", "q"] {
            assert!(!answer_is_yes(no), "{no:?}");
        }
    }
}
