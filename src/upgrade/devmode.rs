//! The upgrade turns the 0.15 installer's developer mode off (BO-28).
//!
//! Every installer from 0.7.0 to 0.15.2 wrote `[devmode] enabled = true` into the global base.toml, so most users' Claude
//! ended every reply with a DEVMODE block nobody chose. 0.16's installer writes it off (BO-27); this turns it off for the
//! users who still have the installer's line, once, and tells them how to get it back where they cannot miss it.
//!
//! **Whose value it is.** The installer's line is recognised byte for byte, comment included ([`INSTALLER_LINE`]).
//! `base config set` parses the whole file and writes it back without a single comment, so a value turned on by command
//! never carries that line, and a hand-edited one rarely does: both are the user's, and stay as they are, with no line.
//!
//! **When.** The background upgrade only marks a tier's `devmode_off` as due ([`mark`]). The next session start makes the
//! change and prints the paragraph in the same run ([`at_session_start`]), so the block stops at the start that says so,
//! never part way through a session that was never told. That start prints it first, above everything else (a `Pinned`
//! block, which no budget pass shortens), and `base doctor` repeats it for [`ADVICE_DAYS`] days ([`advice`]).
//!
//! **Once.** A tier's record keeps `devmode_off` whatever happened, so nothing turns developer mode off a second time, by
//! this version or a later one, even after the user restores the backup.

use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::{read_record, write_record, Record, Tier, VERSION};

/// The `[devmode]` line every installer from 0.7.0 (24c7bf6) to 0.15.2 wrote, byte for byte. Installers before it
/// wrote a plain `enabled = true`, which reads as a value someone set.
pub const INSTALLER_LINE: &str = "enabled = true            # false = no diagnostic block";

/// What the line becomes: the line this version's installer writes, so an upgraded file reads like a new one.
pub const OFF_LINE: &str = "enabled = false           # true = a diagnostic block on every response";

/// The block's first line. Session start's text reaches the model, so the line asks it to pass the paragraph on.
pub const HEADER: &str =
    "DEVELOPER MODE TURNED OFF · in your first reply, before anything else, show the user this paragraph word for word:";

/// How long `base doctor` repeats the paragraph after the change.
pub const ADVICE_DAYS: i64 = 14;

/// A tier's developer-mode step, in its `upgrade.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Off {
    /// `due`: the background upgrade found the installer's line, and the next session start turns it off. Then `done`;
    /// `failed` (the line was left on and said so); `kept` (the line changed before that start, so someone set it);
    /// `linked` (the file is a link, left as it is and said so).
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// The copy of base.toml from just before the change: `base doctor --restore` takes it back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The version that made the change, which the paragraph names wherever it is printed again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// `[devmode]` from a table header line, whitespace and any trailing comment ignored.
fn header(line: &str) -> String {
    line.split('#').next().unwrap_or("").chars().filter(|c| !c.is_whitespace()).collect()
}

/// Where `want` is in `text`, as the `[devmode]` table's only `enabled` line, without its line ending, when the file
/// parses and its `devmode.enabled` is `value`.
fn only_enabled_line(text: &str, want: &str, value: bool) -> Option<std::ops::Range<usize>> {
    let table: toml::Table = text.parse().ok()?;
    if table.get("devmode")?.get("enabled")?.as_bool() != Some(value) {
        return None;
    }
    let mut section = String::new();
    let mut at = 0;
    let mut lines = 0;
    let mut found = None;
    for line in text.split_inclusive('\n') {
        let start = at;
        at += line.len();
        let t = line.trim_start();
        if t.starts_with('[') {
            section = header(t);
            continue;
        }
        if section != "[devmode]" || t.starts_with('#') {
            continue;
        }
        if t.split_once('=').map(|(k, _)| k.trim().trim_matches(['"', '\''])) != Some("enabled") {
            continue;
        }
        lines += 1;
        let body = line.strip_suffix('\n').unwrap_or(line);
        let body = body.strip_suffix('\r').unwrap_or(body);
        if body == want {
            found = Some(start..start + body.len());
        }
    }
    if lines == 1 { found } else { None }
}

/// Where the installer's line is in `text`. `None` unless the file parses, its `devmode.enabled` is `true`, and its
/// `[devmode]` table has exactly one `enabled` line, which is [`INSTALLER_LINE`].
pub fn installer_line(text: &str) -> Option<std::ops::Range<usize>> {
    only_enabled_line(text, INSTALLER_LINE, true)
}

/// The line is still the one the upgrade wrote: nobody has set developer mode since.
pub fn is_upgrades_line(text: &str) -> bool {
    only_enabled_line(text, OFF_LINE, false).is_some()
}

/// `text` with the installer's line replaced by [`OFF_LINE`]; every other byte, line endings included, as it was. Parsed
/// back and refused unless `devmode.enabled` is the only value that changed.
pub fn turn_off(text: &str) -> Result<String> {
    let at = installer_line(text).context("the [devmode] line is not the installer's")?;
    let out = format!("{}{OFF_LINE}{}", &text[..at.start], &text[at.end..]);
    let mut want: toml::Table = text.parse().context("the base.toml does not parse")?;
    if let Some(toml::Value::Table(d)) = want.get_mut("devmode") {
        d.insert("enabled".to_string(), toml::Value::Boolean(false));
    }
    let got: toml::Table = out.parse().context("the rewritten base.toml would not parse; nothing written")?;
    if got != want {
        bail!("the rewrite would change more than [devmode] enabled; nothing written");
    }
    Ok(out)
}

fn is_link(file: &Path) -> bool {
    std::fs::symlink_metadata(file).is_ok_and(|m| m.file_type().is_symlink())
}

/// How a user turns it on, or off, in this tier's file, as the rest of a sentence: `base config set` writes the global
/// file only, and a workspace's value wins over the global one.
fn set_to(tier: &Tier, file: &Path, value: bool) -> String {
    if tier.label == "global" {
        format!("run `base config set devmode.enabled {value}`")
    } else {
        format!("set `enabled = {value}` under `[devmode]` in `{}`", file.display())
    }
}

/// The paragraph session start prints once and `base doctor` repeats: what changed and why, what the block did, where
/// to see the same thing without it, and both ways back. Every command in backticks, so the period after one is never
/// copied into it (BO-30).
pub fn paragraph(tier: &Tier, file: &Path, backup: &Path, version: &str) -> String {
    format!(
        "base {version} turned developer mode off. The 0.15 installer turned it on for everyone, so Claude ended every \
         reply with a DEVMODE block listing which of your domains base loaded and why. To see what base adds without the \
         block, run `base log matches`. To turn developer mode back on, {} (while it is on, base does not update \
         itself). To undo this change, run `base doctor --restore \"{}\"`.",
        set_to(tier, file, true),
        backup.display()
    )
}

pub(super) fn linked_line(tier: &Tier, file: &Path) -> String {
    let or_command = if tier.label == "global" { ", or run `base config set devmode.enabled false`" } else { "" };
    format!(
        "{} Developer mode is still on. We left `{}` as it is because it is a link to another file. To turn it off, set \
         `enabled = false` under `[devmode]` in that file{or_command}.",
        super::LEAD,
        file.display()
    )
}

pub(super) fn failed_line(tier: &Tier, file: &Path, why: &str) -> String {
    format!(
        "{} We could not turn developer mode off in `{}`: {why}. It is still on. To turn it off, {}.",
        super::LEAD,
        file.display(),
        set_to(tier, file, false)
    )
}

// ─── The background upgrade ────────────────────────────────────────────────

/// After a tier's upgrade, whatever it came to: mark its developer mode due when its base.toml has the installer's line,
/// or say once that a linked one was left on. A tier that has a step already is left alone, and so is a tier with no
/// record for this version (its upgrade will run again).
pub(super) fn mark(tier: &Tier) {
    let Some(mut record) = read_record(&tier.store) else { return };
    if record.version != VERSION || record.devmode_off.is_some() {
        return;
    }
    let file = tier.config.join("base.toml");
    let Ok(text) = std::fs::read_to_string(&file) else { return };
    if installer_line(&text).is_none() {
        return;
    }
    record.devmode_off = Some(if is_link(&file) {
        super::announce(&mut record, linked_line(tier, &file));
        Off { status: "linked".into(), at: Some(super::now()), ..Default::default() }
    } else {
        Off { status: "due".into(), ..Default::default() }
    });
    let _ = write_record(&tier.store, &record);
}

// ─── Session start ─────────────────────────────────────────────────────────

/// Turn developer mode off in every tier marked due, and return the paragraph for each: session start prints them in
/// its `devmode-off` block under [`HEADER`]. Nothing while an upgrade process holds the lock: a later start does it. A
/// tier whose line changed since it was marked was set by someone, and is left with no line; one that cannot be written
/// is left on, and says so among the upgrade's lines this same start.
pub fn at_session_start(cwd: &Path) -> Vec<String> {
    let due = |r: &Record| r.devmode_off.as_ref().is_some_and(|d| d.status == "due");
    let tiers = super::tiers(cwd);
    if !tiers.iter().any(|t| read_record(&t.store).is_some_and(|r| due(&r))) {
        return Vec::new();
    }
    let Some(global) = tiers.iter().find(|t| t.label == "global") else { return Vec::new() };
    let Some(_lock) = super::lock(&global.store, std::time::Duration::ZERO) else { return Vec::new() };
    let mut out = Vec::new();
    for tier in &tiers {
        let Some(mut record) = read_record(&tier.store) else { continue };
        if due(&record)
            && let Some(p) = turn_off_tier(tier, &mut record)
        {
            out.push(p);
        }
    }
    out
}

fn turn_off_tier(tier: &Tier, record: &mut Record) -> Option<String> {
    let file = tier.config.join("base.toml");
    let text = std::fs::read_to_string(&file).ok().filter(|t| installer_line(t).is_some());
    let Some(text) = text else {
        record.devmode_off = Some(Off { status: "kept".into(), at: Some(super::now()), ..Default::default() });
        let _ = write_record(&tier.store, record);
        return None;
    };
    if is_link(&file) {
        super::announce(record, linked_line(tier, &file));
        record.devmode_off = Some(Off { status: "linked".into(), at: Some(super::now()), ..Default::default() });
        let _ = write_record(&tier.store, record);
        return None;
    }
    let failed = |record: &mut Record, why: String| -> Option<String> {
        super::announce(record, failed_line(tier, &file, &why));
        record.devmode_off = Some(Off { status: "failed".into(), at: Some(super::now()), error: Some(why), ..Default::default() });
        let _ = write_record(&tier.store, record);
        None
    };
    let after = match turn_off(&text) {
        Ok(after) => after,
        Err(e) => return failed(record, format!("{e:#}")),
    };
    let backup = match super::backup(&file, VERSION) {
        Ok(backup) => backup,
        Err(e) => return failed(record, format!("{e:#}")),
    };
    // The record before the file: a process stopped between the two leaves developer mode as it was and says nothing,
    // never a change nobody was told about. A record that cannot be written changes nothing, and a later start tries again.
    record.devmode_off = Some(Off {
        status: "done".into(),
        at: Some(super::now()),
        backup: Some(backup.display().to_string()),
        error: None,
        version: Some(VERSION.to_string()),
    });
    write_record(&tier.store, record).ok()?;
    let tmp = file.with_extension("toml.devmode-tmp");
    if let Err(e) = std::fs::write(&tmp, &after).and_then(|()| std::fs::rename(&tmp, &file)) {
        let _ = std::fs::remove_file(&tmp);
        return failed(record, format!("writing {}: {e}", file.display()));
    }
    Some(paragraph(tier, &file, &backup, VERSION))
}

// ─── base doctor ───────────────────────────────────────────────────────────

/// The paragraph again, as doctor advice, for each tier changed in the last [`ADVICE_DAYS`] days whose `[devmode]` line is
/// still the one the upgrade wrote. Any other line means the user has set it since.
pub fn advice(cwd: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for tier in super::tiers(cwd) {
        let Some(off) = read_record(&tier.store).and_then(|r| r.devmode_off) else { continue };
        let (Some(at), Some(backup)) = (off.at.as_deref(), off.backup.as_deref()) else { continue };
        if off.status != "done" || !within_advice_days(at) {
            continue;
        }
        let file = tier.config.join("base.toml");
        if std::fs::read_to_string(&file).is_ok_and(|t| is_upgrades_line(&t)) {
            out.push(paragraph(&tier, &file, Path::new(backup), off.version.as_deref().unwrap_or(VERSION)));
        }
    }
    out
}

fn within_advice_days(at: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(at)
        .is_ok_and(|at| chrono::Local::now().signed_duration_since(at) < chrono::Duration::days(ADVICE_DAYS))
}

#[cfg(test)]
mod tests {
    use super::*;

    const INSTALLED: &str = "[namespace]\nprefix = \"ops\"\n\n# Appends a DEVMODE block to each response.\n[devmode]\n\
                             enabled = true            # false = no diagnostic block\n\n[bracket]\nenabled = true\n";

    #[test]
    fn the_installers_line_is_found_and_turned_off_alone() {
        let at = installer_line(INSTALLED).expect("the installer's file");
        assert_eq!(&INSTALLED[at], INSTALLER_LINE);
        let out = turn_off(INSTALLED).unwrap();
        assert_eq!(out, INSTALLED.replace(INSTALLER_LINE, OFF_LINE), "only that line changes");
        assert!(installer_line(&out).is_none(), "once off, it is not the installer's any more");
        assert!(is_upgrades_line(&out));
    }

    #[test]
    fn crlf_line_endings_stay_crlf() {
        let crlf = INSTALLED.replace('\n', "\r\n");
        let out = turn_off(&crlf).unwrap();
        assert_eq!(out, crlf.replace(INSTALLER_LINE, OFF_LINE));
    }

    #[test]
    fn a_value_set_by_command_or_by_hand_is_not_the_installers() {
        // What `base config set devmode.enabled true` writes: the whole file again, without comments.
        let by_command = "[namespace]\nprefix = \"ops\"\n\n[devmode]\nenabled = true\n";
        assert!(installer_line(by_command).is_none());
        for line in [
            "enabled = true",
            "enabled = true # mine",
            "  enabled = true            # false = no diagnostic block",
            "enabled = true            # false = no diagnostic block ",
        ] {
            let text = format!("[devmode]\n{line}\n");
            assert!(installer_line(&text).is_none(), "{line:?} was written by someone");
        }
        // The installer's comment on a value someone turned off, and the line under another table.
        assert!(installer_line("[devmode]\nenabled = false            # false = no diagnostic block\n").is_none());
        assert!(installer_line("[bracket]\nenabled = true            # false = no diagnostic block\n").is_none());
        assert!(turn_off(by_command).is_err(), "nothing is written for a value someone set");
    }

    #[test]
    fn the_line_it_writes_is_the_one_a_new_install_writes() {
        let install = include_str!("../install.rs").replace("\r\n", "\n");
        assert!(install.contains(&format!("[devmode]\n{OFF_LINE}\n")));
    }

    #[test]
    fn doctor_repeats_it_for_fourteen_days() {
        let ago = |days: i64| (chrono::Local::now() - chrono::Duration::days(days)).to_rfc3339();
        assert!(within_advice_days(&ago(0)));
        assert!(within_advice_days(&(chrono::Local::now() - chrono::Duration::hours(14 * 24 - 1)).to_rfc3339()));
        assert!(!within_advice_days(&ago(ADVICE_DAYS)));
        assert!(!within_advice_days("not a time"));
    }
}
