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
//! Identity (#132): a sentinel's mtime proves a loop is running, not WHOSE.
//! Retiring a session renames its registry title but does not move the monitor,
//! which was armed with an absolute inbox path — so a predecessor keeps touching
//! its SUCCESSOR's sentinel, and the board read `Watching` for a titleholder
//! whose own monitor was dead. Measured on the operator's live board 2026-09-10:
//! of 118 rows, the only two showing `Watching` were both sessions base itself
//! called DEAD. It also left two sessions consuming one codename's pings, with
//! whichever answered first clearing the alert for both.
//!
//! So each loop additionally writes a per-session sibling beside the sentinel,
//! `.watching-by-<session-id>`, and [`watch_state`] compares it against the
//! title's registered holder. The sentinel itself does not move, is not renamed,
//! and is still touched on every poll, so anything outside base that reads its
//! mtime keeps working byte for byte. A monitor armed before this existed writes
//! no sibling and reads as [`WatchState::Unidentified`] — never as `Watching`.
//!
//! Identity lives in a per-session FILENAME rather than inside `.watching`
//! because a bare `touch` bumps an mtime without changing content: an id in the
//! body could be married to a different writer's freshness and read as a clean,
//! plausible, wrong answer. One file per writer makes identity and freshness a
//! single observation by a single process, which no second toucher can splice.

use std::path::PathBuf;

use super::task_inbox::title_dir;

/// Sentinel older than this = not watching. 3× the watch loop's 5s poll:
/// one slow loop can't flap the board, a dead monitor shows within ~15s.
pub const WATCH_STALE_SECS: u64 = 15;

/// Stale-sentinel re-arm nudges are throttled per title so a session that
/// cannot arm (no Monitor tool in its harness) isn't nagged every tool call.
const NUDGE_COOLDOWN_SECS: u64 = 180;

/// Filename prefix of the per-session identity sibling.
///
/// Matched EXACTLY, never as a `.watching*` glob: pending pings (`*.json`),
/// `.status`, `.watch-nudge` and the sentinel itself share this directory, and a
/// match one character wider would let [`prune_watch_siblings`] delete
/// undelivered messages.
const SIBLING_PREFIX: &str = ".watching-by-";

/// How long an identity sibling survives without being rewritten.
///
/// The same cutoff [`super::session_registry`] uses to prune dead entries. A
/// live monitor rewrites its own sibling every 5s, so reaching this age takes
/// 17,280 consecutive missed polls — a loop stopped for a full day.
const SIBLING_KEEP_SECS: u64 = 24 * 3600;

fn sentinel_path(title: &str) -> Option<PathBuf> {
    title_dir(title).map(|d| d.join(".watching"))
}

fn nudge_path(title: &str) -> Option<PathBuf> {
    title_dir(title).map(|d| d.join(".watch-nudge"))
}

/// Session ids are UUIDs on every harness base runs in, so this is the identity
/// function for them in practice. It exists so a hand-passed `--session` value
/// can never put a path separator or a shell metacharacter into a filename.
/// Same shape as `task_inbox::sanitize`, which is private to that module.
fn sanitize_id(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect()
}

/// The identity sibling's filename for one session.
fn sibling_name(session_id: &str) -> String {
    format!("{SIBLING_PREFIX}{}", sanitize_id(session_id))
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

/// Is an mtime age inside the watching threshold?
///
/// The one place the comparison lives. [`watch_state`] collects ages once with
/// [`age_secs`] and asks this about them, rather than re-stat-ing a path it has
/// already read.
fn fresh_age(age: u64) -> bool {
    age < WATCH_STALE_SECS
}

/// Who is touching a title's sentinel, and whether that is the titleholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchState {
    /// The title's registered holder is touching its own sibling. The only
    /// state that means a ping can wake this session while it sits idle.
    Watching { session: String, age: u64 },
    /// A fresh sibling written by a session that does NOT hold this title: a
    /// retired predecessor whose monitor is still looping, a session that
    /// re-registered under another title, or the pre-`/clear` session whose
    /// title was rebound to a new id by `session_registry::auto_register`.
    /// Deliberately not called "retired" — three different situations produce
    /// it and the id is reported rather than the cause guessed.
    Foreign { session: String, age: u64 },
    /// A named session is touching, but no session is registered as holding
    /// this title, so there is nothing to compare it against.
    ///
    /// This exists because there are TWO registries. The board iterates the
    /// per-project store; [`watch_state`] resolves the global one. Measured
    /// 2026-09-10: 27 global titles against 118 rows in one project store, with
    /// only 9 present in both. Falling through to `Foreign` would accuse 109
    /// sessions of being foreign to a title that has no holder to be foreign to.
    UnknownHolder { session: String, age: u64 },
    /// The sentinel is fresh but nothing claims it: a monitor armed before
    /// identity existed, or a toucher that is not a relay session at all.
    Unidentified { age: u64 },
    /// Something touched it once, but not recently enough to count.
    Stale { age: u64 },
    /// Nothing has ever touched it.
    Never,
}

/// Read the sentinel and its identity siblings, resolving the holder from the
/// GLOBAL session registry.
///
/// Correct for the re-arm nudge and the ping warning, which both mean the global
/// binding. The board must NOT use this: it iterates a per-project store whose
/// titles largely do not appear in the global registry, and it knows its own
/// binding — so it calls [`watch_state_for`] with it.
pub fn watch_state(title: &str) -> WatchState {
    let holder = super::session_registry::resolve(title).map(|e| e.session_id);
    watch_state_for(title, holder.as_deref())
}

/// [`watch_state`] with the holder supplied by the caller.
///
/// The caller says who holds the title, because the caller is the one iterating
/// a registry and this crate has two of them. `None` means "no holder is
/// registered" and produces [`WatchState::UnknownHolder`] rather than an
/// accusation of foreignness.
pub fn watch_state_for(title: &str, holder: Option<&str>) -> WatchState {
    let Some(dir) = title_dir(title) else {
        return WatchState::Never;
    };
    let holder = holder.map(sanitize_id);

    let mut siblings: Vec<(String, u64)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Some(id) = name.strip_prefix(SIBLING_PREFIX) else {
                continue;
            };
            if id.is_empty() {
                continue;
            }
            if let Some(age) = age_secs(&e.path()) {
                siblings.push((id.to_string(), age));
            }
        }
    }

    // The holder's own sibling wins whenever it is fresh, even while a retired
    // predecessor is also touching: BOTH siblings are fresh in that window, and
    // a live successor must not be reported as a foreign watcher.
    if let Some(h) = holder.as_deref()
        && let Some((_, age)) = siblings.iter().find(|(id, a)| id == h && fresh_age(*a))
    {
        return WatchState::Watching { session: h.to_string(), age: *age };
    }

    // Otherwise the freshest foreign sibling, with the id as a deterministic
    // tie-break so two reads a second apart cannot name different watchers.
    let mut fresh_ids: Vec<&(String, u64)> =
        siblings.iter().filter(|(_, a)| fresh_age(*a)).collect();
    fresh_ids.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    if let Some((id, age)) = fresh_ids.first() {
        // With no registered holder there is nothing to be foreign TO, so the
        // toucher is named and the absence of a holder is stated (#132 C9).
        return match holder {
            Some(_) => WatchState::Foreign { session: id.clone(), age: *age },
            None => WatchState::UnknownHolder { session: id.clone(), age: *age },
        };
    }

    // No fresh identity anywhere. The bare sentinel still separates "a loop is
    // running but names nobody" from "nothing is running".
    let bare = sentinel_path(title).and_then(|p| age_secs(&p));
    match bare {
        Some(a) if fresh_age(a) => WatchState::Unidentified { age: a },
        _ => match siblings.iter().map(|(_, a)| *a).chain(bare).min() {
            Some(a) => WatchState::Stale { age: a },
            None => WatchState::Never,
        },
    }
}

/// Is SOME loop touching this title's sentinel right now?
///
/// **This keeps the meaning it had in 0.15.0 and must not be narrowed.** Its
/// caller that matters is `session_registry::pick_name`, which asks a different
/// question from the board: not "can this titleholder be woken" but "is it safe
/// to hand this codename to somebody else". A live session whose heartbeat only
/// refreshes at boundary hooks looks dead for minutes at a time while its
/// monitor loops, and this is the signal that stops its name being taken — a
/// spawned tab stole "heron" from a mid-build session exactly that way on
/// 2026-08-17.
///
/// Narrowing this to holder identity re-opened that hole. Measured on this
/// machine 2026-09-10, before the split: 7 registered titles had a dead-looking
/// heartbeat, a sentinel touched within 5 seconds by a loop still running, and
/// no identity file — with 8 more one idle interval away, the orchestrator, the
/// reviewer and the author of this change among them. Use
/// [`watching_by_holder`] for the narrow question.
pub fn is_watching(title: &str) -> bool {
    matches!(
        watch_state(title),
        WatchState::Watching { .. }
            | WatchState::Foreign { .. }
            | WatchState::UnknownHolder { .. }
            | WatchState::Unidentified { .. }
    )
}

/// Is the title's OWN registered holder touching its sentinel right now?
///
/// The narrow question: only this answers "a ping will wake this session while
/// it sits idle". A loop belonging to some other session proves nothing about
/// the titleholder. Used by the board, the ping warning and the re-arm nudge.
pub fn watching_by_holder(title: &str) -> bool {
    matches!(watch_state(title), WatchState::Watching { .. })
}

/// Board cell: the watching state, short enough for a table column.
///
/// The session id is deliberately NOT here. Law 32 bans showing part of a value,
/// and a 36-character id in every row wrecks a board the operator scans
/// constantly — so the id goes in [`watch_detail`]'s footer line, in full.
pub fn watch_cell(title: &str) -> String {
    cell_of(watch_state(title))
}

/// [`watch_cell`] with the holder supplied by the caller — the board's entry
/// point, since the board knows its own store's binding (#132 C9).
pub fn watch_cell_for(title: &str, holder: Option<&str>) -> String {
    cell_of(watch_state_for(title, holder))
}

fn cell_of(state: WatchState) -> String {
    match state {
        WatchState::Watching { .. } => "✓".into(),
        WatchState::Foreign { .. } => "✗ foreign".into(),
        WatchState::UnknownHolder { .. } => "✗ unverified".into(),
        WatchState::Unidentified { .. } => "✗ unidentified".into(),
        WatchState::Stale { age } => format!("✗ stale {}", human(age)),
        WatchState::Never => "✗ never".into(),
    }
}

/// One footer line naming who is touching a title's sentinel, session id in
/// FULL and never abbreviated.
///
/// `None` for the two states whose cell already says everything, which keeps the
/// footer bounded by the number of loops actually running rather than by the
/// size of the registry — two lines under a 118-row board, measured.
pub fn watch_detail(title: &str) -> Option<String> {
    detail_of(watch_state(title))
}

/// [`watch_detail`] with the holder supplied by the caller (#132 C9).
pub fn watch_detail_for(title: &str, holder: Option<&str>) -> Option<String> {
    detail_of(watch_state_for(title, holder))
}

fn detail_of(state: WatchState) -> Option<String> {
    match state {
        WatchState::Watching { session, .. } => {
            Some(format!("✓ session {session} — the registered holder"))
        }
        WatchState::Foreign { session, .. } => Some(format!(
            "✗ foreign — session {session} is touching this sentinel and does not hold the title"
        )),
        WatchState::UnknownHolder { session, .. } => Some(format!(
            "✗ unverified — session {session} is touching, but no session is registered as holding this title"
        )),
        WatchState::Unidentified { .. } => {
            Some("✗ unidentified — a loop is touching, no session id on disk".into())
        }
        WatchState::Stale { .. } | WatchState::Never => None,
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

/// Remove identity siblings left behind by sessions that are long gone.
///
/// Called from `relay register` only — a human-invoked action, never a timer. A
/// narrow window firing unattended is worse than a wide one somebody triggered.
///
/// All four conditions must hold before a file is removed:
///
/// (a) the name is exactly `.watching-by-<id>`. See [`SIBLING_PREFIX`] for why
///     this is never widened to a glob.
/// (b) it has not been rewritten for [`SIBLING_KEEP_SECS`] — 17,280 consecutive
///     missed five-second polls, so a live monitor's sibling can never qualify.
/// (c) the id is not this title's registered holder.
/// (d) the id is not the registered holder of ANY title, so a session that moved
///     titles keeps its identity.
///
/// (c) is implied by (d) whenever the registry is readable. It is kept as its own
/// check because it is the case that matters most and costs one comparison, but
/// no mutation can prove it independently — the build record marks that mutation
/// INERT rather than claiming it proven.
///
/// Were this ever wrong, the owning loop rewrites its sibling on the next poll:
/// the worst case is one poll interval reading `Unidentified`, never a
/// permanent loss.
pub fn prune_watch_siblings(title: &str) -> usize {
    let Some(dir) = title_dir(title) else {
        return 0;
    };
    let holder = super::session_registry::resolve(title).map(|e| sanitize_id(&e.session_id));
    let held: std::collections::HashSet<String> = super::session_registry::list()
        .into_iter()
        .map(|e| sanitize_id(&e.session_id))
        .collect();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return 0;
    };
    let mut removed = 0usize;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(id) = name.strip_prefix(SIBLING_PREFIX) else {
            continue; // (a) not an identity sibling
        };
        if id.is_empty() {
            continue; // (a) prefix with nothing after it
        }
        if age_secs(&e.path()).is_none_or(|a| a < SIBLING_KEEP_SECS) {
            continue; // (b) too recent to be abandoned
        }
        if holder.as_deref() == Some(id) {
            continue; // (c) this title's holder
        }
        if held.contains(id) {
            continue; // (d) holds some other title
        }
        if std::fs::remove_file(e.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// The canonical watch loop for a title — the single source of truth every
/// session arms verbatim (bash: Git Bash on Windows, bash in WSL). Touching
/// the sentinel each poll IS the compliance proof. Dotfiles stay invisible
/// to both the loop's `ls -1` and base's *.json inbox scan.
fn watch_script(title: &str, session_id: Option<&str>) -> Option<String> {
    let inbox = title_dir(title)?.to_string_lossy().replace('\\', "/");
    // The identity line, with the session id baked in as a LITERAL.
    //
    // Not a shell variable and not a `$(…)`: this text is pasted into a harness
    // Monitor call and evaluated by whatever shell that monitor runs in, so an
    // expansion would make the identity depend on that shell's environment —
    // true on the machine you tested, not guaranteed anywhere else. A literal
    // cannot vary. Measured 2026-09-10 through a Windows Git Bash: the literal
    // arrives unchanged, while `$(echo X)` in the same position is substituted.
    //
    // With no session id the emitted script is EXACTLY the old one, and the
    // reader reports `Unidentified` rather than inventing an identity: absent is
    // reported as absent.
    let identity = match session_id {
        Some(s) if !s.trim().is_empty() => format!(
            "\n  printf '%s\\n' \"{id}\" > \"$INBOX/{file}\" 2>/dev/null",
            id = sanitize_id(s),
            file = sibling_name(s),
        ),
        _ => String::new(),
    };
    Some(format!(
        r#"INBOX="{inbox}"
mkdir -p "$INBOX"
seen=""
while true; do
  touch "$INBOX/.watching" 2>/dev/null{identity}
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
pub fn arm_block(title: &str, session_id: Option<&str>) -> Option<String> {
    arm_block_for(title, &operator_name(), session_id)
}

/// `arm_block` with the operator label supplied — the pure half, so the text
/// can be tested without a profile on disk.
fn arm_block_for(title: &str, operator: &str, session_id: Option<&str>) -> Option<String> {
    let inbox_disp = title_dir(title)?.to_string_lossy().replace('\\', "/");
    let script = watch_script(title, session_id)?;
    let indented: String = script.lines().map(|l| format!("    {l}\n")).collect();
    // #132 C7: the skip condition must be an IDENTITY, not a behaviour. Keying it
    // on "your loop touches .watching" told every monitor armed before this
    // change to skip — they all touch it — so they never re-armed, never wrote an
    // identity, and drew the stale-sentinel nudge every 3 minutes forever. Naming
    // the exact file a session can look for makes the check decidable.
    let skip_marker = match session_id {
        Some(s) if !s.trim().is_empty() => sibling_name(s),
        _ => ".watching".to_string(),
    };
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
         \"{title}\" that writes {skip_marker}, skip — never arm a duplicate. \
         Otherwise TaskStop whatever monitor you are running and arm this one, INCLUDING a \
         monitor that touches .watching but writes no such file: that is the older script, and \
         until it is replaced the board cannot tell your loop from a retired session's.\n\n\
         \x20 description: relay wake: {title}\n\
         \x20 persistent: true\n\
         \x20 command:\n{indented}\n\
         While the monitor runs, its loop touches the .watching sentinel every 5s poll and \
         records which session is watching beside it; `base relay board` shows that as \
         Watching, and this block repeats (at most once per 3 minutes) until the sentinel is \
         fresh.\n\
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
        // The NARROW predicate: a retired predecessor still touching this path
        // must not silence the successor's nudge (#132). Using the wide
        // `is_watching` here is the bug this lane exists to fix.
        if watching_by_holder(&title) {
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
        if let Some(block) = arm_block(&title, Some(session_id)) {
            out.push_str(&block);
            out.push('\n');
        }
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The freshness comparison itself, through the same pair of functions
    /// [`watch_state`] uses: read the age once, then judge it.
    fn fresh_path(p: &std::path::Path) -> bool {
        age_secs(p).is_some_and(fresh_age)
    }

    #[test]
    fn fresh_sentinel_within_threshold_stale_after_missing_never() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join(".watching");
        assert!(!fresh_path(&p), "missing sentinel must read as not watching");
        std::fs::write(&p, b"").unwrap();
        assert!(fresh_path(&p), "just-touched sentinel must read as watching");
        let old = std::time::SystemTime::now()
            - std::time::Duration::from_secs(WATCH_STALE_SECS + 5);
        let f = std::fs::File::options().write(true).open(&p).unwrap();
        f.set_modified(old).unwrap();
        assert!(!fresh_path(&p), "sentinel older than threshold must read stale");
    }

    #[test]
    fn arm_block_carries_sentinel_touch_and_persistent_flag() {
        // title_dir needs a home dir; any real home works — content only.
        if let Some(block) = arm_block_for("wake-test-title", "Pat", None) {
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

    /// #132: with a session id the emitted loop writes the identity sibling, and
    /// the id is a LITERAL — no `$(…)` and no shell variable, either of which
    /// would make the identity depend on the monitor shell's environment.
    #[test]
    fn arm_block_bakes_the_session_id_as_a_literal() {
        let sid = "abcd1234-0000-4000-8000-abcdefabcdef";
        let Some(block) = arm_block_for("wake-test-title", "Pat", Some(sid)) else {
            return;
        };
        assert!(
            block.contains(&format!("> \"$INBOX/.watching-by-{sid}\"")),
            "the sibling write must name the session id literally"
        );
        assert!(
            block.contains("touch \"$INBOX/.watching\""),
            "the sentinel itself must still be touched, unmoved and unrenamed"
        );
        // The identity must not be produced by anything the monitor's shell
        // evaluates at run time.
        let identity_line = block
            .lines()
            .find(|l| l.contains(".watching-by-"))
            .expect("the identity line must be present");
        assert!(
            !identity_line.contains("$("),
            "no command substitution in the identity line: {identity_line}"
        );
        assert!(
            !identity_line.contains("CLAUDE_CODE_SESSION_ID"),
            "no environment variable in the identity line: {identity_line}"
        );
    }

    /// Control for the test above: with NO session id the script is the old
    /// shape and carries no sibling write at all. Without this leg, an
    /// implementation that always emitted a sibling would pass unnoticed.
    #[test]
    fn control_no_session_id_emits_no_sibling_write() {
        let Some(block) = arm_block_for("wake-test-title", "Pat", None) else {
            return;
        };
        assert!(
            !block.contains(".watching-by-"),
            "with no session id there is no identity to write"
        );
        assert!(block.contains("touch \"$INBOX/.watching\""));
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

    /// The sibling filename is exactly the prefix plus a sanitized id, so a
    /// hand-passed `--session` value can never escape into a path.
    #[test]
    fn sibling_names_are_sanitized() {
        assert_eq!(
            sibling_name("4bde320a-0c4f-44b4-a4b3-2b4bb5383f53"),
            ".watching-by-4bde320a-0c4f-44b4-a4b3-2b4bb5383f53",
            "a UUID passes through untouched"
        );
        // Counted rather than eyeballed: `a/../b` is six characters, four of
        // which are not alphanumeric, so four dashes. Writing the literal from
        // memory gave three and the test failed on its own arithmetic.
        assert_eq!(sibling_name("a/../b"), ".watching-by-a----b");
        assert_eq!(sibling_name("x y$z"), ".watching-by-x-y-z");
    }
}
