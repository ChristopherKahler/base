//! U3: the starter star commands follow the upgrade (BO-26).
//!
//! A user's `commands.toml` holds the starter pack base wrote at install, often with their own commands beside it. A
//! `[[command]]` block that is still, byte for byte, one an earlier base shipped ([`crate::install::STARTER_COMMANDS_SHIPPED`])
//! was never edited, so it gets this build's text: only that block's bytes change, after a backup. A block the user
//! edited is left as it is; when it is `*base` and its `rule add` line lacks `--fires-on` (0.16's `rule add` asks for
//! `--keywords` and `--fires-on` inside a session, BO-15), they are told once.
//!
//! Byte for byte except line endings: `.gitattributes` does not pin `*.toml`, so a Windows build carries the pack with
//! CRLF and a Linux build with LF, and a user's file holds whichever wrote it.
//!
//! A file that is a link is never written, not even through the link (lynx's G0 ruling on Q3): a link is shared with
//! something else, and on this machine the global one points into WSL, where a different base may read it. It gets the
//! notice instead.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::command::{parse_commands, CommandDef};

/// What the upgrade found, and did, in one `commands.toml`.
#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    /// Blocks replaced by this build's text, by command name, in file order.
    pub updated: Vec<String>,
    /// Where the file was before, when blocks were replaced.
    pub backup: Option<PathBuf>,
    /// The file is a link: blocks that are an earlier base's text, left as they are.
    pub linked_old: Vec<String>,
    /// An edited `*base` whose `rule add` line has no `--fires-on`.
    pub old_rule_add: bool,
}

/// One `[[command]]` table of a commands.toml: from its header line to its last line that is neither blank nor a
/// comment, so a comment above the next table is not part of it.
#[derive(Debug, Clone)]
struct Block {
    start: usize,
    end: usize,
}

fn is_header(line: &str) -> bool {
    let t = line.trim();
    let t = t.split('#').next().unwrap_or("").trim_end();
    t.starts_with('[') && t.ends_with(']') && !t.starts_with("[\"") && !t.starts_with("['")
}

fn blocks(text: &str) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    let mut at = 0;
    let mut open: Option<Block> = None;
    for line in text.split_inclusive('\n') {
        let start = at;
        at += line.len();
        if is_header(line) {
            if let Some(b) = open.take() {
                out.push(b);
            }
            if line.trim().starts_with("[[command]]") {
                open = Some(Block { start, end: at });
            }
            continue;
        }
        let t = line.trim();
        if let Some(b) = open.as_mut()
            && !t.is_empty()
            && !t.starts_with('#')
        {
            b.end = at;
        }
    }
    if let Some(b) = open {
        out.push(b);
    }
    out
}

/// A block's text with line endings as LF and no line break at its end: what two copies of one block compare on.
fn normal(text: &str) -> String {
    text.replace("\r\n", "\n").trim_end_matches(['\n', '\r']).to_string()
}

/// The one command a block holds, when it parses as exactly one.
fn command_of(block: &str) -> Option<CommandDef> {
    let mut all = parse_commands(block)?;
    (all.len() == 1).then(|| all.remove(0))
}

/// Every block of a starter pack text, by name, normalised.
fn pack(text: &str) -> Vec<(String, String)> {
    blocks(text)
        .into_iter()
        .filter_map(|b| {
            let t = &text[b.start..b.end];
            command_of(t).map(|c| (c.name, normal(t)))
        })
        .collect()
}

/// The block `name` has in this build's pack.
fn current(name: &str) -> Option<String> {
    pack(crate::install::STARTER_COMMANDS).into_iter().find(|(n, _)| n == name).map(|(_, t)| t)
}

/// Whether `text` is the block `name` had in a pack an earlier base shipped.
fn shipped_before(name: &str, text: &str) -> bool {
    crate::install::STARTER_COMMANDS_SHIPPED
        .iter()
        .any(|(_, pack_text)| pack(pack_text).iter().any(|(n, t)| n == name && t == text))
}

/// An edited `*base` whose `rule add` line has no `--fires-on`: Example 3's case.
fn lacks_fires_on(c: &CommandDef) -> bool {
    c.name == "base" && c.rules.iter().any(|r| r.contains("rule add") && !r.contains("--fires-on"))
}

/// Look at `file` and, unless it is a link, bring every unedited starter block up to this build's text. `version` names
/// the backup. A missing file is nothing to do. Never writes a file it could not parse back as the same commands with
/// only those blocks changed.
pub fn upgrade(file: &Path, version: &str) -> Result<Outcome> {
    let Ok(meta) = std::fs::symlink_metadata(file) else {
        return Ok(Outcome::default());
    };
    let linked = meta.file_type().is_symlink();
    let text = std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let Some(before) = parse_commands(&text) else {
        // A file that does not parse is doctor's to report (`config_errors`); nothing here touches it.
        return Ok(Outcome::default());
    };
    let mut out = Outcome::default();
    let mut replace: Vec<(Block, String, String)> = Vec::new();
    for b in blocks(&text) {
        let raw = &text[b.start..b.end];
        let Some(cmd) = command_of(raw) else { continue };
        let Some(now) = current(&cmd.name) else { continue };
        let mine = normal(raw);
        if mine == now {
            continue;
        }
        if shipped_before(&cmd.name, &mine) {
            replace.push((b, cmd.name.clone(), now));
        } else if lacks_fires_on(&cmd) {
            out.old_rule_add = true;
        }
    }
    if linked {
        out.linked_old = replace.into_iter().map(|(_, n, _)| n).collect();
        return Ok(out);
    }
    if replace.is_empty() {
        return Ok(out);
    }
    let crlf = text.contains("\r\n");
    let mut next = text.clone();
    for (b, _, now) in replace.iter().rev() {
        let body = if crlf { now.replace('\n', "\r\n") } else { now.clone() };
        // The block's own trailing line break stays: `normal` dropped it, and the bytes after the block are the user's.
        let tail = &text[b.start..b.end];
        let ending = &tail[tail.trim_end_matches(['\n', '\r']).len()..];
        next.replace_range(b.start..b.end, &format!("{body}{ending}"));
    }
    // Parsed back: the same commands in the same order, the replaced ones now this build's, every other one unchanged.
    let Some(after) = parse_commands(&next) else {
        bail!("{} would not parse after the update; nothing written", file.display());
    };
    let names: Vec<&str> = replace.iter().map(|(_, n, _)| n.as_str()).collect();
    let shipped = parse_commands(crate::install::STARTER_COMMANDS).unwrap_or_default();
    let same = before.len() == after.len()
        && before.iter().zip(&after).all(|(b, a)| {
            if names.contains(&b.name.as_str()) {
                shipped.iter().any(|s| s == a) && a.name == b.name
            } else {
                a == b
            }
        });
    if !same {
        bail!("updating {} would change more than its starter commands; nothing written", file.display());
    }
    let backup = crate::upgrade::backup(file, version)?;
    let tmp = file.with_extension("toml.upgrade-tmp");
    std::fs::write(&tmp, &next).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, file).with_context(|| format!("replacing {}", file.display()))?;
    out.updated = names.iter().map(|n| n.to_string()).collect();
    out.backup = Some(backup);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_pack_splits_into_its_four_commands() {
        for (_, text) in crate::install::STARTER_COMMANDS_SHIPPED.iter().chain([&("now", crate::install::STARTER_COMMANDS)]) {
            let names: Vec<String> = pack(text).into_iter().map(|(n, _)| n).collect();
            assert_eq!(names, ["handoff", "fork", "base", "end"]);
        }
    }

    #[test]
    fn a_comment_before_the_next_table_is_not_part_of_the_block() {
        let text = "[[command]]\nname = \"a\"\nrules = [\"x\"]\n\n# mine\n[[command]]\nname = \"b\"\n";
        let b = blocks(text);
        assert_eq!(b.len(), 2);
        assert_eq!(&text[b[0].start..b[0].end], "[[command]]\nname = \"a\"\nrules = [\"x\"]\n");
    }

    #[test]
    fn crlf_and_lf_copies_of_a_block_are_the_same_block() {
        let lf = "[[command]]\nname = \"a\"\n";
        assert_eq!(normal(lf), normal(&lf.replace('\n', "\r\n")));
    }
}
