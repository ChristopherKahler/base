//! `base update` never refreshed the installed CLAUDE.md contract: the binary,
//! `scripts/` and the coach skill moved with each release while the `## BASE CLI`
//! section stayed as the install date left it. These tests hold the refresh that
//! closes that: only the section is rewritten, everything a user wrote around it is
//! byte-identical, repeat runs write nothing, and the once-per-version session-start
//! path runs exactly once.

use std::fs;
use std::path::{Path, PathBuf};

use base::install::{
    claude_md_stamp_name, ensure_claude_md_current, refresh_claude_md_section, ClaudeMdRefresh, BASE_CLI_SECTION,
};

const ABOVE: &str = "# Mine\n\nKeep this exactly as it is.\n\n";
const OLD_SECTION: &str = "## BASE CLI — Proactive Context Engine\n\nSome earlier release's text.\n\n### An old sub-heading\n- stale bullet\n";
const BELOW: &str = "\n\n## My rules\n\nAnd keep this too.\n";

fn claude_md(home: &Path, body: &str) -> PathBuf {
    let dir = home.join(".claude");
    fs::create_dir_all(&dir).unwrap();
    let p = dir.join("CLAUDE.md");
    fs::write(&p, body).unwrap();
    p
}

fn current() -> &'static str {
    BASE_CLI_SECTION.trim()
}

#[test]
fn an_old_section_is_replaced_and_the_users_text_around_it_is_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let p = claude_md(tmp.path(), &format!("{ABOVE}{OLD_SECTION}{BELOW}"));

    assert_eq!(refresh_claude_md_section(&p).unwrap(), ClaudeMdRefresh::Refreshed);

    let got = fs::read_to_string(&p).unwrap();
    // The newline that ended the old section's last line is outside the span and stays.
    assert_eq!(got, format!("{ABOVE}{}\n{BELOW}", current()), "only the section moves");
    assert!(!got.contains("stale bullet"));
    assert!(!p.with_extension("md.tmp").exists(), "the temp file is gone");
}

#[test]
fn a_current_section_is_not_rewritten() {
    let tmp = tempfile::tempdir().unwrap();
    let p = claude_md(tmp.path(), &format!("{ABOVE}{OLD_SECTION}{BELOW}"));
    refresh_claude_md_section(&p).unwrap();
    let after_first = fs::read(&p).unwrap();
    let mtime = fs::metadata(&p).unwrap().modified().unwrap();

    assert_eq!(refresh_claude_md_section(&p).unwrap(), ClaudeMdRefresh::Current);

    assert_eq!(fs::read(&p).unwrap(), after_first);
    assert_eq!(fs::metadata(&p).unwrap().modified().unwrap(), mtime, "no write at all");
}

#[test]
fn a_section_at_the_end_of_the_file_is_replaced_in_place() {
    // The shape `base install` leaves: the section is the last thing in the file.
    let tmp = tempfile::tempdir().unwrap();
    let p = claude_md(tmp.path(), &format!("{ABOVE}{OLD_SECTION}"));

    assert_eq!(refresh_claude_md_section(&p).unwrap(), ClaudeMdRefresh::Refreshed);
    assert_eq!(fs::read_to_string(&p).unwrap(), format!("{ABOVE}{}\n", current()));
}

#[test]
fn a_crlf_file_keeps_crlf_and_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let body = format!("{ABOVE}{OLD_SECTION}{BELOW}").replace('\n', "\r\n");
    let p = claude_md(tmp.path(), &body);

    assert_eq!(refresh_claude_md_section(&p).unwrap(), ClaudeMdRefresh::Refreshed);
    let got = fs::read_to_string(&p).unwrap();
    assert!(got.starts_with(&ABOVE.replace('\n', "\r\n")), "user text keeps its line endings");
    assert!(got.ends_with(&BELOW.replace('\n', "\r\n")));
    assert!(!got.contains("stale bullet"));
    assert!(!got.replace("\r\n", "").contains('\n'), "no bare LF was introduced: the section is CRLF too");

    assert_eq!(refresh_claude_md_section(&p).unwrap(), ClaudeMdRefresh::Current);
}

#[test]
fn a_missing_file_is_reported_and_not_created() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join(".claude").join("CLAUDE.md");
    assert_eq!(refresh_claude_md_section(&p).unwrap(), ClaudeMdRefresh::Missing);
    assert!(!p.exists());
}

#[test]
fn a_file_without_the_section_is_left_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let p = claude_md(tmp.path(), "# Mine\n\nI removed base's section on purpose.\n");
    let before = fs::read(&p).unwrap();
    assert_eq!(refresh_claude_md_section(&p).unwrap(), ClaudeMdRefresh::NotInstalled);
    assert_eq!(fs::read(&p).unwrap(), before);
}

#[test]
fn a_bom_does_not_hide_a_heading_on_the_first_line_and_survives() {
    let tmp = tempfile::tempdir().unwrap();
    let p = claude_md(tmp.path(), &format!("\u{feff}{OLD_SECTION}{BELOW}"));

    assert_eq!(refresh_claude_md_section(&p).unwrap(), ClaudeMdRefresh::Refreshed);
    let got = fs::read_to_string(&p).unwrap();
    assert_eq!(got, format!("\u{feff}{}\n{BELOW}", current()));
    assert_eq!(refresh_claude_md_section(&p).unwrap(), ClaudeMdRefresh::Current);
}

#[test]
fn two_sections_are_reported_and_nothing_is_touched() {
    let tmp = tempfile::tempdir().unwrap();
    let p = claude_md(tmp.path(), &format!("{ABOVE}{OLD_SECTION}\n{OLD_SECTION}{BELOW}"));
    let before = fs::read(&p).unwrap();

    assert_eq!(refresh_claude_md_section(&p).unwrap(), ClaudeMdRefresh::Duplicate(2));
    assert_eq!(fs::read(&p).unwrap(), before, "a duplicate is a report, never a partial rewrite");
}

#[test]
fn a_stamp_left_by_0_13_18_does_not_suppress_the_refresh() {
    // 0.13.18 stamped `.claude-md-<hash>` even when nothing was installed. That
    // name must not count: this release looks once more under a new name.
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let gbl = tmp.path().join(".base-gbl");
        fs::create_dir_all(&gbl).unwrap();
        let old_name = claude_md_stamp_name().replacen(".claude-md2-", ".claude-md-", 1);
        assert_ne!(old_name, claude_md_stamp_name());
        fs::write(gbl.join(&old_name), b"").unwrap();
        let p = claude_md(tmp.path(), &format!("{ABOVE}{OLD_SECTION}{BELOW}"));

        assert_eq!(ensure_claude_md_current(), Some(ClaudeMdRefresh::Refreshed));
        assert!(fs::read_to_string(&p).unwrap().contains(current()));
        assert!(gbl.join(claude_md_stamp_name()).exists());
    });
}

#[test]
fn session_start_does_not_stamp_until_the_text_is_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        fs::create_dir_all(tmp.path().join(".base-gbl")).unwrap();
        let stamp = tmp.path().join(".base-gbl").join(claude_md_stamp_name());

        // No file yet: reported, not stamped.
        assert_eq!(ensure_claude_md_current(), Some(ClaudeMdRefresh::Missing));
        assert!(!stamp.exists());

        // Two sections: reported, not stamped.
        let p = claude_md(tmp.path(), &format!("{OLD_SECTION}\n{OLD_SECTION}"));
        assert_eq!(ensure_claude_md_current(), Some(ClaudeMdRefresh::Duplicate(2)));
        assert!(!stamp.exists());

        // The user fixes it: the refresh still happens, and only now is it stamped.
        fs::write(&p, format!("{ABOVE}{OLD_SECTION}{BELOW}")).unwrap();
        assert_eq!(ensure_claude_md_current(), Some(ClaudeMdRefresh::Refreshed));
        assert!(stamp.exists());
        assert_eq!(ensure_claude_md_current(), None);
    });
}

#[test]
fn session_start_refreshes_once_per_version_and_stamps_it() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        fs::create_dir_all(tmp.path().join(".base-gbl")).unwrap();
        let p = claude_md(tmp.path(), &format!("{ABOVE}{OLD_SECTION}{BELOW}"));

        assert_eq!(ensure_claude_md_current(), Some(ClaudeMdRefresh::Refreshed));
        assert!(fs::read_to_string(&p).unwrap().contains(current()));
        let stamp = tmp.path().join(".base-gbl").join(claude_md_stamp_name());
        assert!(stamp.exists(), "the once-per-text stamp is written");
        assert!(!claude_md_stamp_name().contains(env!("CARGO_PKG_VERSION")), "keyed on the text, not the version");

        // Put the old text back: a stamped version must not touch the file again.
        fs::write(&p, format!("{ABOVE}{OLD_SECTION}{BELOW}")).unwrap();
        assert_eq!(ensure_claude_md_current(), None);
        assert!(fs::read_to_string(&p).unwrap().contains("stale bullet"));
    });
}
