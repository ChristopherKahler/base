//! U2: path triggers after an upgrade (BO-26, lynx's G0 ruling on Q2).
//!
//! A broad trigger is advice in doctor, not a fault, so nothing here narrows one. Two things happen on upgrade:
//!
//! - **The mechanical part (P3).** A trigger written relative resolves against its tier's root and fires as before; base
//!   stores a trigger as its full path, so each one is written out as the full path it already resolves to. Same place,
//!   so nothing it matches changes. Only those strings change in the file, and the result is parsed back and refused
//!   unless exactly those values moved: a `domains.toml` written by hand keeps its comments and layout, which
//!   `domain::set_paths` would not. A file that is a link is left as it is and keeps doctor's advice line.
//! - **Q2b.** A trigger over two or more registered projects was inert in 0.15.2 (F29) and fires since BO-10 on every
//!   file outside those projects' folders. Nothing is written for it; the upgrade names such triggers once, with the
//!   narrowing command.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::domain::matcher;

/// `(domain, as written, as now written)`.
pub type Changed = (String, String, String);

/// What the rewrite did in one `domains.toml`.
#[derive(Debug, Default)]
pub struct Rewrite {
    /// In file order.
    pub changed: Vec<Changed>,
    pub backup: Option<PathBuf>,
}

/// A trigger written relative: not absolute, not `~`, not a pattern (the same test as doctor's advice line). One with a
/// `..` part is left alone: written out it would be compared part by part with `..` still in it.
pub fn is_relative(t: &str) -> bool {
    let t = t.trim();
    !t.is_empty()
        && !matcher::is_absolute(t)
        && !t.starts_with('~')
        && !t.contains(['*', '?'])
        && !t.replace('\\', "/").split('/').any(|p| p == "..")
}

/// Every relative trigger in `file`, by `(domain, trigger)`, with the full path it resolves to.
fn relative_triggers(file: &Path, root: &Path) -> BTreeMap<(String, String), String> {
    let home = crate::home::home_root().map(|h| h.display().to_string());
    let mut out = BTreeMap::new();
    for d in crate::domain::load_domains_file(file, Some(root)) {
        for t in d.paths.iter().filter(|t| is_relative(t)) {
            if let Some(resolved) = matcher::resolve_trigger(t, d.root.as_deref(), home.as_deref()) {
                out.insert((d.name.clone(), t.clone()), crate::domain::paths::spelled(&resolved));
            }
        }
    }
    out
}

/// Write each relative trigger in `file` (a tier's `domains.toml`, whose relative triggers resolve against `root`) as the
/// full path it resolves to, after a backup named for `version`. A link, a file with none, or one that does not parse is
/// left as it is.
pub fn rewrite_relative(file: &Path, root: &Path, version: &str) -> Result<Rewrite> {
    let Ok(meta) = std::fs::symlink_metadata(file) else {
        return Ok(Rewrite::default());
    };
    if meta.file_type().is_symlink() {
        return Ok(Rewrite::default());
    }
    let map = relative_triggers(file, root);
    if map.is_empty() {
        return Ok(Rewrite::default());
    }
    let text = std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let (next, changed) = rewrite_text(&text, &map)?;
    if changed.is_empty() {
        return Ok(Rewrite::default());
    }
    let backup = crate::upgrade::backup(file, version)?;
    let tmp = file.with_extension("toml.upgrade-tmp");
    std::fs::write(&tmp, &next).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, file).with_context(|| format!("replacing {}", file.display()))?;
    Ok(Rewrite { changed, backup: Some(backup) })
}

/// A TOML string, quoted and escaped.
fn toml_str(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

/// The value of one TOML string token, `"..."` or `'...'`.
fn token_value(token: &str) -> Option<String> {
    let t: toml::Table = toml::from_str(&format!("v = {token}")).ok()?;
    t.get("v")?.as_str().map(String::from)
}

/// The byte ranges of the string tokens inside the array that opens at `open` (the `[`), and where it closes. `None`
/// when the array is not one this scanner reads (a multi-line string, an unclosed bracket).
fn array_strings(text: &str, open: usize) -> Option<(Vec<(usize, usize)>, usize)> {
    let b = text.as_bytes();
    let mut i = open + 1;
    let mut depth = 1usize;
    let mut out = Vec::new();
    while i < b.len() {
        match b[i] {
            b'#' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some((out, i));
                }
            }
            q @ (b'"' | b'\'') => {
                if text[i..].starts_with("\"\"\"") || text[i..].starts_with("'''") {
                    return None;
                }
                let start = i;
                i += 1;
                while i < b.len() && b[i] != q {
                    if q == b'"' && b[i] == b'\\' {
                        i += 1;
                    }
                    if b[i] == b'\n' {
                        return None;
                    }
                    i += 1;
                }
                if i >= b.len() {
                    return None;
                }
                out.push((start, i + 1));
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// `text` with every trigger `map` names replaced by its full path, in its own domain's `paths` array only, and what
/// changed. Parsed back and refused unless exactly those values moved.
fn rewrite_text(text: &str, map: &BTreeMap<(String, String), String>) -> Result<(String, Vec<Changed>)> {
    // Each `[[domain]]` table: from its header to the next header. Its `name` and `paths` come before any table of its
    // own (TOML puts a table's keys before its sub-tables).
    let mut starts: Vec<(usize, bool)> = Vec::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        let head = t.split('#').next().unwrap_or("").trim_end();
        if head.starts_with('[') && head.ends_with(']') && !head.starts_with("[\"") {
            starts.push((at, head.replace(' ', "") == "[[domain]]"));
        }
        at += line.len();
    }
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut changed: Vec<Changed> = Vec::new();
    for (k, &(start, is_domain)) in starts.iter().enumerate() {
        if !is_domain {
            continue;
        }
        let end = starts.get(k + 1).map_or(text.len(), |s| s.0);
        let block = &text[start..end];
        let Ok(table) = toml::from_str::<toml::Table>(block) else { continue };
        let Some(name) = table
            .get("domain")
            .and_then(|d| d.as_array())
            .and_then(|a| a.first())
            .and_then(|d| d.get("name"))
            .and_then(|n| n.as_str())
        else {
            continue;
        };
        // The `paths = [` line of this block.
        let mut off = 0;
        let mut open = None;
        for line in block.split_inclusive('\n') {
            let t = line.trim_start();
            if let Some(rest) = t.strip_prefix("paths")
                && let Some(rest) = rest.trim_start().strip_prefix('=')
                && rest.trim_start().starts_with('[')
            {
                let lead = line.len() - t.len();
                let eq = lead + "paths".len() + (t["paths".len()..].len() - t["paths".len()..].trim_start().len()) + 1;
                let bracket = eq + (line[eq..].len() - line[eq..].trim_start().len());
                open = Some(start + off + bracket);
                break;
            }
            off += line.len();
        }
        let Some(open) = open else { continue };
        let Some((tokens, _)) = array_strings(text, open) else { continue };
        for (s, e) in tokens {
            let Some(value) = token_value(&text[s..e]) else { continue };
            if let Some(full) = map.get(&(name.to_string(), value.clone())) {
                edits.push((s, e, toml_str(full)));
                changed.push((name.to_string(), value, full.clone()));
            }
        }
    }
    let mut next = text.to_string();
    for (s, e, with) in edits.iter().rev() {
        next.replace_range(*s..*e, with);
    }
    // Parsed back: the file as it was, with exactly those values moved.
    let mut want: toml::Table = toml::from_str(text).context("domains.toml does not parse")?;
    if let Some(toml::Value::Array(domains)) = want.get_mut("domain") {
        for d in domains.iter_mut() {
            let Some(name) = d.get("name").and_then(|n| n.as_str()).map(String::from) else { continue };
            if let Some(toml::Value::Array(paths)) = d.get_mut("paths") {
                for p in paths.iter_mut() {
                    if let Some(full) = p.as_str().and_then(|s| map.get(&(name.clone(), s.to_string()))) {
                        *p = toml::Value::String(full.clone());
                    }
                }
            }
        }
    }
    let got: toml::Table = toml::from_str(&next).context("the rewritten domains.toml would not parse; nothing written")?;
    if got != want {
        bail!("writing the relative triggers out would change more than those values; nothing written");
    }
    Ok((next, changed))
}

/// The triggers in `file` that 0.15.2 left inert and that fire now (Q2b): rooted, on a domain that injects, holding two
/// or more registered projects by name, 0.15.2's rule (F29). As `` `trigger` on `domain` (N projects)``.
pub fn newly_firing(file: &Path, root: &Path, ctx: &matcher::TriggerContext) -> Vec<String> {
    let mut out = Vec::new();
    for d in crate::domain::load_domains_file(file, Some(root)).iter().filter(|d| d.auto_inject) {
        for t in &d.paths {
            let Some(resolved) = matcher::resolve_trigger(t, d.root.as_deref(), ctx.home.as_deref()) else { continue };
            let mut names: Vec<String> = Vec::new();
            for r in ctx.registered.iter().filter(|r| !r.path.is_empty() && matcher::path_under(&r.path, &resolved)) {
                if !names.iter().any(|n| n.eq_ignore_ascii_case(&r.name)) {
                    names.push(r.name.clone());
                }
            }
            if names.len() >= 2 {
                out.push(format!("`{t}` on `{}` ({} projects)", d.name, names.len()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str, &str)]) -> BTreeMap<(String, String), String> {
        pairs.iter().map(|(d, t, f)| ((d.to_string(), t.to_string()), f.to_string())).collect()
    }

    #[test]
    fn only_the_relative_values_change_and_comments_stay() {
        let text = "# mine\n[[domain]]\nname = \"a\"\n# the folder\npaths = [\"tools\", \"C:/x\"] # two\nrules = [\"r\"]\n\n[[domain]]\nname = \"b\"\npaths = [\n  'tools', # same word, other domain\n]\n";
        let (next, changed) = rewrite_text(text, &map(&[("a", "tools", "C:/Users/u/tools")])).unwrap();
        assert_eq!(
            next,
            "# mine\n[[domain]]\nname = \"a\"\n# the folder\npaths = [\"C:/Users/u/tools\", \"C:/x\"] # two\nrules = [\"r\"]\n\n[[domain]]\nname = \"b\"\npaths = [\n  'tools', # same word, other domain\n]\n"
        );
        assert_eq!(changed, vec![("a".to_string(), "tools".to_string(), "C:/Users/u/tools".to_string())]);
    }

    #[test]
    fn a_relative_trigger_is_not_absolute_home_a_pattern_or_a_climb() {
        for t in ["tools", "Documents/x", "a\\b"] {
            assert!(is_relative(t), "{t}");
        }
        for t in ["C:/x", "/home/u", "~", "~/x", "*.md", "../x", ""] {
            assert!(!is_relative(t), "{t}");
        }
    }
}
