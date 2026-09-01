//! Wake-monitor contract — every relay-registered session keeps a persistent
//! harness Monitor watching its ping inbox, and proves it with a sentinel.
//!
//! An idle Claude Code session cannot be woken from outside the harness:
//! hooks fire only on activity, and the Monitor tool is the one primitive
//! that produces a mid-idle wake. So the lever is not an external daemon —
//! it is guaranteeing every registered title arms a Monitor at boot, and
//! making compliance observable. The armed watch loop touches
//! `relay-inbox/<title>/.watching` every poll; sentinel freshness IS the
//! watching state. The board renders it, `relay ping` warns senders on it,
//! and the hooks re-emit the arming block whenever it goes stale (monitor
//! died, /clear, new title) — self-healing, zero human prompting.
//!
//! Known edge: two sessions bound to the same title share one sentinel, so
//! the newer session sees a fresh sentinel (the older session's monitor) and
//! skips arming. Pings still wake the older session's monitor and deliver to
//! the newer via hooks; a mid-idle wake of the newer session waits until the
//! older monitor dies and the sentinel goes stale.

use std::path::PathBuf;

use super::task_inbox::title_dir;

/// Sentinel older than this = not watching. 3× the watch loop's 5s poll:
/// one slow loop can't flap the board, a dead monitor shows within ~15s.
pub const WATCH_STALE_SECS: u64 = 15;

/// Stale-sentinel re-arm nudges are throttled per title so a session that
/// cannot arm (no Monitor tool in its harness) isn't nagged every tool call.
const NUDGE_COOLDOWN_SECS: u64 = 180;

fn sentinel_path(title: &str) -> Option<PathBuf> {
    title_dir(title).map(|d| d.join(".watching"))
}

fn nudge_path(title: &str) -> Option<PathBuf> {
    title_dir(title).map(|d| d.join(".watch-nudge"))
}

/// Seconds since the file's mtime; None when the file doesn't exist.
fn age_secs(path: &std::path::Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .elapsed()
        .ok()
        .map(|e| e.as_secs())
}

fn fresh(path: &std::path::Path) -> bool {
    age_secs(path).is_some_and(|a| a < WATCH_STALE_SECS)
}

/// Is a wake monitor for this title provably alive right now?
pub fn is_watching(title: &str) -> bool {
    sentinel_path(title).is_some_and(|p| fresh(&p))
}

/// Board cell: watching state with evidence.
pub fn watch_cell(title: &str) -> String {
    match sentinel_path(title).and_then(|p| age_secs(&p)) {
        Some(a) if a < WATCH_STALE_SECS => "✓".into(),
        Some(a) => format!("✗ stale {}", human(a)),
        None => "✗ never".into(),
    }
}

fn human(secs: u64) -> String {
    match secs {
        s if s < 120 => format!("{s}s"),
        s if s < 7200 => format!("{}m", s / 60),
        s if s < 172_800 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

/// The canonical watch loop for a title — the single source of truth every
/// session arms verbatim (bash: Git Bash on Windows, bash in WSL). Touching
/// the sentinel each poll IS the compliance proof. Dotfiles stay invisible
/// to both the loop's `ls -1` and base's *.json inbox scan.
fn watch_script(title: &str) -> Option<String> {
    let inbox = title_dir(title)?.to_string_lossy().replace('\\', "/");
    Some(format!(
        r#"INBOX="{inbox}"
mkdir -p "$INBOX"
seen=""
while true; do
  touch "$INBOX/.watching" 2>/dev/null
  cur=$(ls -1 "$INBOX" 2>/dev/null | sort | tr '\n' '|')
  if [ "$cur" != "$seen" ]; then
    for f in $(ls -1t "$INBOX" 2>/dev/null | head -5); do
      case "$seen" in *"$f|"*) continue;; esac
      from=$(grep -o '"from": *"[^"]*"' "$INBOX/$f" 2>/dev/null | head -1 | cut -d'"' -f4)
      msg=$(tr -d '\n' < "$INBOX/$f" 2>/dev/null | sed -n 's/.*"summary": *"\(.*\)", *"doc".*/\1/p' | cut -c1-700)
      [ -z "$msg" ] && msg=$(tr -d '\n' < "$INBOX/$f" 2>/dev/null | cut -c1-400)
      echo "RELAY PING from ${{from:-unknown}}: $msg"
    done
    seen=$cur
  fi
  sleep 5
done"#
    ))
}

/// The arming block injected into hook context (and printed by `relay
/// register`) when a title's sentinel is stale. It is the operator's standing
/// instruction from `[relay]` in base.toml, so it says what it is, where it
/// comes from, and how to switch it off — it never asks the model to act
/// without telling the operator.
pub fn arm_block(title: &str) -> Option<String> {
    arm_block_for(title, &operator_name())
}

/// `arm_block` with the operator label supplied — the pure half, so the text
/// can be tested without a profile on disk.
fn arm_block_for(title: &str, operator: &str) -> Option<String> {
    let inbox_disp = title_dir(title)?.to_string_lossy().replace('\\', "/");
    let script = watch_script(title)?;
    let indented: String = script.lines().map(|l| format!("    {l}\n")).collect();
    Some(format!(
        "=== RELAY WAKE CONTRACT ({title}) ===\n\
         This session holds the relay title \"{title}\" and has no live wake monitor (sentinel \
         stale or missing). The relay is base's local multi-session layer, on by way of \
         `[relay]` in ~/.base-gbl/base.toml: `base config set relay.wake_nudge false` stops \
         this block, `base config set relay.enabled false` stops the auto-codename too. \
         Everything below stays on this machine — the monitor watches one local inbox folder, \
         and the sentinel it touches is what `base relay board` reads.\n\
         To arm it, call the Monitor tool once with the script below, then continue your task. \
         If Monitor is a deferred tool in your harness, load it first (ToolSearch \
         \"select:Monitor\") — a direct call fails with InputValidationError until the schema \
         is loaded. No test ping is needed. If THIS session already runs a monitor for \
         \"{title}\" armed with THIS script (its loop touches .watching), skip — never arm a \
         duplicate. If your running monitor is an older script that does not touch the \
         sentinel, TaskStop it first, then arm this one.\n\n\
         \x20 description: relay wake: {title}\n\
         \x20 persistent: true\n\
         \x20 command:\n{indented}\n\
         While the monitor runs, its loop touches the .watching sentinel every 5s poll; \
         `base relay board` shows that as Watching, and this block repeats (at most once per \
         3 minutes) until the sentinel is fresh.\n\
         STATUS LINE: whenever what you are working on changes, write one short line to \
         {inbox_disp}/.status (e.g. `echo \"building X\" > .../.status`). It is a local file \
         the operator's ping hub shows on this session's card, so {operator} sees live work \
         state at a glance.\n\
         PROJECT TAG: the moment you know which project this session serves (and again whenever \
         it changes), run `base relay register --as {title} --project <project-name>` — every \
         project ever named stays on the session's hub card as a filter keyword; nothing is removed.\n"
    ))
}

/// Who the status line is for: the `base operator init` profile's name, or a
/// generic label when no profile is set — never a name baked into the binary.
fn operator_name() -> String {
    crate::operator::load()
        .map(|p| p.name)
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "the operator".to_string())
}

/// Bring a throttle file's mtime to now. A zero-byte `fs::write` over an
/// existing zero-byte file leaves the mtime alone on Windows — observed as a
/// six-day-old `.watch-nudge` while the nudge fired on every tool call (the
/// "every single tool call" of issue #13) — so the time is set explicitly.
fn stamp(path: &std::path::Path) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(f) = std::fs::File::options()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)
    else {
        return;
    };
    let _ = f.set_modified(std::time::SystemTime::now());
}

/// Stale-sentinel scan across every title this session holds. Returns the
/// arming blocks due now, stamping the per-title nudge throttle. `force`
/// (session-start) bypasses the cooldown — a fresh context must always be
/// told to arm.
pub fn arm_blocks_for(session_id: &str, force: bool) -> Option<String> {
    // Harnesses without a Monitor tool (Agent SDK runs, brain.js NPCs) can't
    // comply — let them opt out instead of eating a nudge every cooldown.
    if std::env::var_os("BASE_NO_WAKE_NUDGE").is_some() {
        return None;
    }
    let mut out = String::new();
    for title in super::session_registry::titles_for(session_id) {
        if is_watching(&title) {
            continue;
        }
        let due = force
            || nudge_path(&title)
                .and_then(|p| age_secs(&p))
                .is_none_or(|a| a >= NUDGE_COOLDOWN_SECS);
        if !due {
            continue;
        }
        if let Some(p) = nudge_path(&title) {
            stamp(&p);
        }
        if let Some(block) = arm_block(&title) {
            out.push_str(&block);
            out.push('\n');
        }
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_sentinel_within_threshold_stale_after_missing_never() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join(".watching");
        assert!(!fresh(&p), "missing sentinel must read as not watching");
        std::fs::write(&p, b"").unwrap();
        assert!(fresh(&p), "just-touched sentinel must read as watching");
        let old = std::time::SystemTime::now()
            - std::time::Duration::from_secs(WATCH_STALE_SECS + 5);
        let f = std::fs::File::options().write(true).open(&p).unwrap();
        f.set_modified(old).unwrap();
        assert!(!fresh(&p), "sentinel older than threshold must read stale");
    }

    #[test]
    fn arm_block_carries_sentinel_touch_and_persistent_flag() {
        // title_dir needs a home dir; any real home works — content only.
        if let Some(block) = arm_block_for("wake-test-title", "Pat") {
            assert!(block.contains("touch \"$INBOX/.watching\""));
            assert!(block.contains("persistent: true"));
            assert!(block.contains("relay-inbox"));
            assert!(block.contains("never arm a duplicate"));
            // Issue #11 / #13: the operator comes from the profile, never the
            // binary, and the block explains itself instead of demanding.
            assert!(block.contains("so Pat sees live work state"));
            assert!(!block.contains("Chris sees"), "the home path may contain a name; the sentence must not");
            assert!(!block.contains("Do not ask permission"));
            assert!(!block.contains("arm NOW"));
            assert!(block.contains("relay.wake_nudge false"));
            assert!(block.contains("relay.enabled false"));
        }
    }

    #[test]
    fn stamp_moves_a_stale_throttle_file_to_now() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join(".watch-nudge");
        std::fs::write(&p, b"").unwrap();
        let old = std::time::SystemTime::now()
            - std::time::Duration::from_secs(NUDGE_COOLDOWN_SECS * 10);
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(old).unwrap();
        assert!(age_secs(&p).unwrap() >= NUDGE_COOLDOWN_SECS, "precondition: stale");
        stamp(&p);
        assert!(age_secs(&p).unwrap() < NUDGE_COOLDOWN_SECS, "stamp must read as just touched");
        // Through a missing parent too — the first nudge for a fresh title.
        let deep = tmp.path().join("a").join("b").join(".watch-nudge");
        stamp(&deep);
        assert!(age_secs(&deep).is_some());
    }
}
