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
///
/// ALIVE, NOT CURRENT. This answers "can a ping reach them", which is what a sender
/// warning and a liveness sweep want. Whether the running monitor is the script base
/// would print TODAY is a different question — see [`is_current`].
pub fn is_watching(title: &str) -> bool {
    sentinel_path(title).is_some_and(|p| fresh(&p))
}

/// The template the watch loop is rendered from, hashed as the version of the
/// contract. **Hashed BEFORE substitution, deliberately** (auk's ruling, 2026-09-20,
/// after grebe pushed back on hashing the rendered script).
///
/// A RENDERED hash would have to be reproduced identically on both sides, and it
/// depends on the inbox path rendering the same way each time — `title_dir`
/// resolution, backslash replacement, trailing separators. Three ways the two sides
/// drift apart for reasons that have nothing to do with the thing being checked, and
/// the failure mode is a check that is PERMANENTLY red for every session, which gets
/// ignored within a day and is worse than no check at all. The precedent is in this
/// repo: `doctor::skill_drift_warning` returns `None` whenever two stamps match, and a
/// stamp that could not move inside a release let a coach sit eight days behind while
/// the check said nothing. **A freshness check is not a truth check.**
///
/// A compile-time constant has neither problem. The title is stored beside the hash so
/// a retitle is caught too, since the template itself carries no title.
const WATCH_TEMPLATE: &str = r#"INBOX="{inbox}"
mkdir -p "$INBOX"
seen="|"
reported="|"
while true; do
  printf '%s %s' "{fp}" "{title}" > "$INBOX/.watching" 2>/dev/null
  for f in $(ls -1t "$INBOX"/ping-*.json 2>/dev/null); do
    b=$(basename "$f")
    case "$seen" in *"|$b|"*) continue;; esac
    raw=$(cat "$f" 2>/dev/null | tr -d '\n')
    if [ -z "$raw" ]; then
      case "$reported" in *"|$b|"*) ;; *)
        echo "RELAY EMPTY READ: $b gave nothing on one read - a reply drained it, or it is on disk and empty, and this cannot tell which. Not consumed, so a later read still announces it. This line prints once. Path: $f"
        reported="$reported$b|" ;;
      esac
      continue
    fi
    from=$(printf '%s' "$raw" | grep -o '"from": *"[^"]*"' | head -1 | cut -d'"' -f4)
    msg=$(printf '%s' "$raw" | sed -n 's/.*"summary": *"\(.*\)", *"doc".*/\1/p')
    [ -z "$msg" ] && msg="$raw"
    chars=$(printf '%s' "$msg" | wc -m)
    bytes=$(printf '%s' "$msg" | wc -c)
    out=$(printf '%s' "$msg" | cut -c1-700)
    if [ "$chars" -gt 700 ]; then
      out="$out  [TRUNCATED at 700 of $chars chars ($bytes bytes) - full file: $f]"
    fi
    echo "RELAY PING from ${from:-unknown}: $out"
    seen="$seen$b|"
  done
  sleep 5
done"#;

/// FNV-1a over the template, 16 hex characters. Hand-rolled because a hash crate is a
/// new dependency and those go past Chris first; this value is never a security claim,
/// only "is this the same text".
fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The current contract version. Changes the moment [`WATCH_TEMPLATE`] is edited,
/// which is exactly when every armed monitor becomes out of date.
pub fn template_fingerprint() -> String {
    format!("{:016x}", fnv1a(WATCH_TEMPLATE))
}

/// What a sentinel says about the monitor that wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Armed {
    /// Fresh, and the monitor runs the script base would print now.
    Current,
    /// Fresh, but armed from a DIFFERENT template or under a different title. The
    /// session is reachable and is running an out-of-date contract, so it must be
    /// re-prompted — the state that had no representation before this existed.
    Outdated,
    /// No live monitor at all.
    NotWatching,
}

/// Read the sentinel and say which of the three states it is in.
///
/// AN EMPTY SENTINEL IS `Outdated`, AND THAT IS THE WHOLE MIGRATION. Every monitor
/// alive on this machine right now only `touch`es the sentinel, so it writes NOTHING.
/// If empty were read as "no data, assume fine", every running session would stay
/// silent forever and this fix would never reach a single seat — the defect it exists
/// to fix, reproduced by its own fix. So absence of evidence is a mismatch here, not a
/// pass.
pub fn armed_state(title: &str) -> Armed {
    let Some(p) = sentinel_path(title) else {
        return Armed::NotWatching;
    };
    let body = std::fs::read_to_string(&p).unwrap_or_default();
    armed_from(&body, title, fresh(&p))
}

/// The pure half, so every state is reachable in a test without a home on disk and
/// without waiting for a sentinel to age. `is_fresh` is the caller's reading of the
/// file's mtime; this function never touches the filesystem.
fn armed_from(body: &str, title: &str, is_fresh: bool) -> Armed {
    if !is_fresh {
        return Armed::NotWatching;
    }
    let mut parts = body.split_whitespace();
    let (Some(fp), Some(t)) = (parts.next(), parts.next()) else {
        // Missing either field. An EMPTY sentinel lands here, and it must be Outdated:
        // see the doc above, this is the case every live monitor is in today.
        return Armed::Outdated;
    };
    if fp == template_fingerprint() && t == title {
        Armed::Current
    } else {
        Armed::Outdated
    }
}

/// Is this title's monitor both alive AND running today's script?
pub fn is_current(title: &str) -> bool {
    armed_state(title) == Armed::Current
}

/// Board cell: watching state with evidence.
///
/// A fresh sentinel from an OUT-OF-DATE script reads `✓ old script`, not a bare tick.
/// Those two used to render identically — same fresh sentinel, same silence — and that
/// is the whole reason a stale monitor could sit there being counted as compliant.
pub fn watch_cell(title: &str) -> String {
    if armed_state(title) == Armed::Outdated {
        return "✓ old script".into();
    }
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
/// the sentinel each poll IS the compliance proof.
fn watch_script(title: &str) -> Option<String> {
    watch_script_for(&title_dir(title)?)
}

/// `watch_script` for an explicit inbox, so the loop can be executed in a test
/// rather than only eyeballed.
///
/// WHAT THE PREVIOUS VERSION GOT WRONG, recorded here because the fix looks
/// like a tidy-up and is not. It announced `ls -1t | head -5` and then ran
/// `seen=$cur`, where `cur` was the FULL listing. With six or more waiting
/// pings it printed five and marked every one of them consumed; `cur` then
/// stopped changing, so the remainder were never printed and never would be.
/// Measured 2026-09-20: 6 of 8 lost. Silent message loss, inside the
/// message-delivery system, in the script every session is told to arm
/// verbatim.
///
/// `seen` is now marked ONLY after a ping has actually been echoed.
///
/// AND IT DELIBERATELY DOES NOT PRIME `seen` FROM THE INBOX AT START-UP. Doing
/// so would stop a re-arm re-announcing pings already read, which is tempting,
/// but it would also silence pings that arrived while no monitor was running —
/// trading a visible duplicate for an invisible drop. A duplicate is noise the
/// reader can see. A drop is not. Replies delete their ping file, so the inbox
/// is normally near-empty and the duplicate is bounded.
///
/// Truncation is now marked and names the file, so a clipped ping cannot be
/// mistaken for a short one. The scan is narrowed to `ping-*.json`, and
/// dotfiles (`.watching`, `.status`) stay invisible to it.
///
/// TWO CORRECTIONS FROM auk's GRADING OF THE FIRST VERSION OF THIS FIX, both
/// of them the round's own defects committed inside the commit that fixes one.
///
/// UNITS. It cut with `cut -c` (characters) and reported with `wc -c` (bytes),
/// labelled "bytes". On multibyte content the cut passed up to 2,100 bytes
/// while the label claimed 700. `cut -c` is kept because it cannot split a
/// character mid-sequence; BOTH units are now named, each as what it is.
///
/// DELIMITER. `seen` matched `*"$b|"*`, a trailing pipe only, so a filename
/// ending with another entry's name would false-match and that ping would be
/// SILENTLY SKIPPED — the exact failure this function exists to remove.
/// `seen` now opens with `|` and the match is anchored on both sides.
///
/// RANK E — ONE READ, AND NEVER CONSUME ON AN EMPTY ONE. Found by auk
/// EXPERIENCING it: its waker printed `RELAY PING from grebe:` with no body.
///
/// The loop took ONE `ls` snapshot and then read each file THREE times — once
/// for `from`, once for the summary, once for the raw fallback. **A reply
/// clears inbound pings**, so a file later in the snapshot can be deleted
/// before it is read. All three reads then return empty, the header prints
/// with nothing after it, and the next line marks the file consumed. Silent
/// loss again, by a different mechanism than `head -5`, in the same script.
///
/// The body is now read ONCE into `raw`, and an empty read is `continue`
/// WITHOUT marking `seen`. If a reply really did clear the file, the next `ls`
/// simply does not list it and nothing was lost; if the read failed
/// transiently, the next poll retries it.
///
/// AND IT SAYS SO, WHICH RANK E DID NOT (2026-09-20, raised by `plover`, who read
/// the emitted script off its own re-arm hook). Rank E shipped the read-once fix
/// and nothing else: an empty read printed NOTHING and continued. That removed a
/// mislabelled ping and put silence in its place, which is the same shape one turn
/// on — THE INSTRUMENT HAD NO WAY TO SAY "I COULD NOT SEE", and a quiet inbox and a
/// ping that vanished under the read reached the reader identically.
///
/// The two cases above are also not all of them. A file that is PRESENT AND EMPTY is
/// neither cleared nor transient: it stays on the `ls`, so it is re-read every poll
/// forever. A drained file self-limits by disappearing; this one does not. Before
/// this line it did that unboundedly AND silently, so the only case with an unbounded
/// retry was also the only case with no output at all.
///
/// So the empty branch prints ONE line naming both states it cannot separate, with the
/// path, tracked in `reported` rather than `seen`. `reported` IS A SEPARATE LIST ON
/// PURPOSE: marking `seen` would bound the retry by consuming a file a later poll might
/// read successfully, which reverses the paragraph above rather than completing it.
///
/// KNOWN AND LEFT: the retry is still unbounded. Bounding it is rank E's decision to
/// revisit, not a line to slip in beside a logging fix. `plover`'s sentence is why the
/// line came first — A BOUNDED SILENCE LOOKS DELIBERATE.
///
/// There is deliberately NO separate existence test. auk proposed read-once
/// plus a guard; the guard only ever existed to bridge the gap between listing
/// and reading, and reading once removes the gap rather than narrowing it. A
/// test between `ls` and `cat` can always be overtaken by the delete it is
/// checking for.
pub fn watch_script_for(inbox: &std::path::Path) -> Option<String> {
    // The title is the inbox's own last component, so the name the monitor STAMPS is
    // always the name of the folder it WATCHES. Deriving it here rather than taking it
    // as an argument keeps those two from ever disagreeing, and keeps every existing
    // caller working.
    let title = inbox.file_name()?.to_string_lossy().into_owned();
    let inbox = inbox.to_string_lossy().replace('\\', "/");
    // Substitution happens AFTER the hash is taken, and `replace` is used rather than
    // `format!` because the template is a plain const whose braces are literal shell.
    Some(
        WATCH_TEMPLATE
            .replace("{inbox}", &inbox)
            .replace("{fp}", &template_fingerprint())
            .replace("{title}", &title),
    )
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
         PROJECT TAG: register with NO --project first — `base relay register --as {title}` — \
         which joins this workspace's relay store and puts you on `base relay board`, the \
         operator's hub view. Then read your own row back before anything else; a registration \
         you did not read back did not happen. Passing --project names a DIFFERENT store, and if \
         no store by that name exists you are registered globally only and never appear on the \
         board. Add the project keyword afterwards, once you are on it.\n"
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
        // is_current, NOT is_watching. This gate used to ask whether a monitor EXISTS
        // when the question is whether the running one MATCHES what base prints now. A
        // live monitor touches its sentinel every 5s, so a session running an OLD
        // script never went stale and was never shown the new one — every wake fix was
        // undeliverable to exactly the sessions already running. Found by grebe,
        // verified by auk in this file's own doc at lines 9-12.
        if is_current(&title) {
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

    /// The five gate states, driven through the pure half so none of them needs a home
    /// on disk or a sentinel aged in real time.
    ///
    /// LEG 2 IS THE ONE THAT DECIDES WHETHER THIS FIX REACHES ANYBODY. Every monitor
    /// alive on this machine only touches the sentinel, so it writes NOTHING. If empty
    /// read as "no data, assume fine", every running session would stay silent forever
    /// and the fix would never reach a seat — the defect reproduced by its own fix.
    #[test]
    fn the_gate_separates_current_outdated_and_not_watching() {
        let fp = template_fingerprint();

        // 1. fresh, right fingerprint, right title
        assert_eq!(armed_from(&format!("{fp} finch"), "finch", true), Armed::Current);

        // 2. THE MIGRATION: an empty sentinel is Outdated, never a pass
        assert_eq!(armed_from("", "finch", true), Armed::Outdated,
            "an EMPTY sentinel must be Outdated - every monitor running today writes nothing");

        // 3. a monitor armed from an older template
        assert_eq!(armed_from("0000000000000000 finch", "finch", true), Armed::Outdated);

        // 4. right fingerprint, WRONG title - a retitle, which the template hash alone
        //    cannot see, which is why the title is stored beside it
        assert_eq!(armed_from(&format!("{fp} plover"), "finch", true), Armed::Outdated);

        // 5. no live monitor at all beats every other reading
        assert_eq!(armed_from(&format!("{fp} finch"), "finch", false), Armed::NotWatching);
    }

    /// A half-written sentinel — fingerprint present, title missing — is Outdated, not
    /// Current. Without this, a torn write would read as compliant.
    #[test]
    fn a_sentinel_missing_its_second_field_is_outdated() {
        let fp = template_fingerprint();
        assert_eq!(armed_from(&fp, "finch", true), Armed::Outdated);
        assert_eq!(armed_from("   ", "finch", true), Armed::Outdated);
    }

    /// Whitespace and a trailing newline must not change the reading. The script writes
    /// with `printf` and no newline today, but a future shell or editor adding one must
    /// not make every session read as Outdated - that is the permanently-red failure
    /// auk ruled against.
    #[test]
    fn the_gate_tolerates_surrounding_whitespace() {
        let fp = template_fingerprint();
        assert_eq!(armed_from(&format!("  {fp}  finch 
"), "finch", true), Armed::Current);
    }

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
    fn arm_block_carries_the_sentinel_write_and_persistent_flag() {
        // title_dir needs a home dir; any real home works — content only.
        if let Some(block) = arm_block_for("wake-test-title", "Pat") {
            // CHANGED 2026-09-20 WITH THE CONTRACT IT PINS. This asserted the loop
            // ran `touch "$INBOX/.watching"`. The loop now WRITES the sentinel instead,
            // with the template fingerprint and the title, so the old string is gone by
            // design. The replacement has more teeth than the original, not less: it
            // pins that both FIELDS reach the emitted block, which is what the gate
            // reads. A test asserting only that some line mentions the path would pass
            // over a loop that wrote nothing into it.
            assert!(
                block.contains("> \"$INBOX/.watching\""),
                "the loop must WRITE the sentinel, not touch it"
            );
            assert!(
                block.contains(&template_fingerprint()),
                "the emitted block must carry the template fingerprint"
            );
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
