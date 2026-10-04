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

/// The longest deadline the host's Monitor tool accepts. Claude Code 2.1.286 and 2.1.287 describe the field as
/// "Deadlines above 1800000ms are capped to 1800000ms" (30 minutes), and the tool has no `persistent` field, so a
/// watcher always expires and is armed again (BO-04, F4c). `base relay arm` prints this value.
pub const MONITOR_TIMEOUT_MS: u64 = 1_800_000;

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
/// A compile-time constant has neither problem.
///
/// NO TITLE IS STORED BESIDE IT (auk added one, then withdrew it on 2026-09-20). The
/// worry was that a template-only hash leaves a RETITLED session reading Current while
/// its monitor watches the wrong inbox. THE PATH ALREADY CLOSES THAT: the sentinel lives
/// at `relay-inbox/<title>/.watching`, so base builds a DIFFERENT path for a retitled
/// session, finds no file, and reads `NotWatching`. A missing file is already a mismatch,
/// and the field caught nothing the path did not — at a measured cost of 11 characters in
/// a block that renders on every session start.
///
/// ⚑ RESIDUAL, NAMED RATHER THAN HIDDEN (auk): `title_dir` SANITIZES, so two distinct
/// titles can map to one directory — `auk-1` and `auk_1` both become `auk-1`. That is a
/// real collision and a title field would not have fixed it well. It is a defect in the
/// sanitizer, recorded here so the next reader does not re-derive it.
///
/// BO-05 (F12b): `SESSION` is the id of the session that held the title when the watcher was armed, and a ping file
/// whose `to_session` names another session is skipped, never printed. A watcher is a delivery path: without this, an
/// old holder's watcher still running on the folder printed in full every ping sent to the title's new holder, and a
/// ping still addressed to the old holder was printed by the new holder's watcher before the hooks archived it. An
/// empty `SESSION` (no holder when armed) or a file with no `to_session` skips nothing. Changing the template moves
/// the fingerprint, so every watcher running the previous script reads `Outdated` and its session is told once to
/// re-arm.
const WATCH_TEMPLATE: &str = r#"INBOX="{inbox}"
SESSION="{session}"
mkdir -p "$INBOX"
seen="|"
reported="|"
while true; do
  printf '%s' "{fp}" > "$INBOX/.watching" 2>/dev/null
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
    to=$(printf '%s' "$raw" | grep -o '"to_session": *"[^"]*"' | head -1 | cut -d'"' -f4)
    if [ -n "$SESSION" ] && [ -n "$to" ] && [ "$to" != "$SESSION" ]; then
      seen="$seen$b|"
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
    armed_from(&body, fresh(&p))
}

/// The pure half, so every state is reachable in a test without a home on disk and
/// without waiting for a sentinel to age. `is_fresh` is the caller's reading of the
/// file's mtime; this function never touches the filesystem. It takes no title: the
/// sentinel's PATH carries that, and a retitle is a different path.
fn armed_from(body: &str, is_fresh: bool) -> Armed {
    if !is_fresh {
        return Armed::NotWatching;
    }
    // An EMPTY sentinel yields no token and falls through to Outdated, which is the
    // case every live monitor on this machine is in today. See the doc above: if empty
    // read as fine, this fix would never reach a single seat.
    match body.split_whitespace().next() {
        Some(fp) if fp == template_fingerprint() => Armed::Current,
        _ => Armed::Outdated,
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
///
/// The script skips pings addressed to any session but the one holding `title` now (BO-05).
fn watch_script(title: &str) -> Option<String> {
    let holder = super::session_registry::resolve(title).map(|e| e.session_id).unwrap_or_default();
    watch_script_for_session(&title_dir(title)?, &holder)
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
    watch_script_for_session(inbox, "")
}

/// [`watch_script_for`] for a watcher that prints only the pings addressed to `session` (BO-05); empty prints all.
pub fn watch_script_for_session(inbox: &std::path::Path, session: &str) -> Option<String> {
    let inbox = inbox.to_string_lossy().replace('\\', "/");
    // A session id is a uuid: only its characters go into the shell string.
    let session: String = session.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    // Substitution happens AFTER the hash is taken, and `replace` is used rather than
    // `format!` because the template is a plain const whose braces are literal shell.
    Some(
        WATCH_TEMPLATE
            .replace("{inbox}", &inbox)
            .replace("{session}", &session)
            .replace("{fp}", &template_fingerprint()),
    )
}

/// What `base relay arm` prints for a title: the host Monitor tool's three fields with their values, the re-arm
/// instruction, and the status-line and register steps (BO-04, F4a and F4c).
///
/// BEFORE BO-04 this text, then called the wake contract, was injected into prompt-submit AND pre-tool: about 3,400
/// bytes plus a 47-line script, repeated every three minutes while the sentinel was stale. On 2026-10-01 it took 3,400 of
/// a 4,000-byte prompt budget and was cut at line 37 of the script, and it told the model to pass `persistent: true` to
/// a Monitor tool that has no such field. The hooks now carry one line ([`nudge_line`]) and this text is printed only
/// when asked for, by `base relay arm`, or by `base relay register` for a title with no current watcher.
pub fn arm_text(title: &str) -> Option<String> {
    arm_text_for(title, &operator_name())
}

/// `arm_text` with the operator label supplied, the pure half, so the text can be tested without a profile on disk.
fn arm_text_for(title: &str, operator: &str) -> Option<String> {
    let inbox_disp = title_dir(title)?.to_string_lossy().replace('\\', "/");
    let script = watch_script(title)?;
    let indented: String = script.lines().map(|l| format!("    {l}\n")).collect();
    let minutes = MONITOR_TIMEOUT_MS / 60_000;
    Some(format!(
        "Start your relay watcher with the Monitor tool, using these fields:\n\
         \x20 description: relay wake: {title}\n\
         \x20 timeout_ms:  {MONITOR_TIMEOUT_MS}   (the longest this Claude Code allows; it expires after {minutes} minutes)\n\
         \x20 command:\n{indented}\
         When the Monitor reports that it expired, run `base relay arm` again.\n\
         If Monitor is a deferred tool, load it first: ToolSearch \"select:Monitor\". If this session already runs a \
         watcher for \"{title}\" with this script, do not start a second one; if it runs an older script, stop that one \
         (TaskStop) first.\n\
         Status line: echo \"<what you are working on>\" > {inbox_disp}/.status (shown to {operator} on this session's \
         card).\n\
         Not registered yet? base relay register --as {title}, then check your row in base relay sessions.\n\
         The watcher only reads this machine's inbox folder {inbox_disp}. `base config set relay.wake_nudge false` stops \
         the one-line reminder in the hooks.\n"
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

/// The one line the hooks carry in place of the old wake contract (BO-04, F4b). It names the command that prints the
/// rest; it never carries the script.
pub fn nudge_line(title: &str, state: &Armed) -> String {
    match state {
        Armed::Outdated => format!(
            "relay: {title}'s inbox watcher runs an old script · run base relay arm and start the Monitor it prints"
        ),
        _ => format!("relay: {title} has no inbox watcher · run base relay arm and start the Monitor it prints"),
    }
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

/// Record that `session_id` was given the nudge line for this title: the file holds the session id and its mtime is the
/// time of the nudge.
fn stamp_nudge(path: &std::path::Path, session_id: &str) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, session_id);
    stamp(path);
}

fn mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// Whether the nudge line is due for this title on this event. The pure half, so every case is reachable in a test
/// without waiting for a sentinel to age.
///
/// ONCE PER SESSION, AND ONCE PER STALE EVENT (BO-04, F4b). `last` is the session the line was last given to and when.
/// A session already told is told again only when a watcher ran AFTER that (the sentinel was written later than the
/// nudge) and is dead now: a 30-minute Monitor expiry is normal on this host, and each one is one event. A watcher that
/// is alive but runs an older script (`Outdated`) is told once per session. `force` is session start: a fresh context
/// is always told.
///
/// What this replaced: a 180-second cooldown, so a session that could not or did not arm was given the whole contract
/// every three minutes across prompt-submit and pre-tool for as long as it ran.
fn nudge_due(
    state: &Armed,
    last: Option<(&str, std::time::SystemTime)>,
    sentinel: Option<std::time::SystemTime>,
    session_id: &str,
    force: bool,
) -> bool {
    if *state == Armed::Current {
        return false;
    }
    if force {
        return true;
    }
    let Some((told, at)) = last else {
        return true;
    };
    if told != session_id {
        return true;
    }
    *state == Armed::NotWatching && sentinel.is_some_and(|s| s > at)
}

/// The nudge lines due now for every title in use this session holds, one line per title, with the stamps held back as
/// commits: the prompt hook runs them only if it prints the block (BO-01), so a dropped line is still due next prompt.
pub fn nudge_lines_deferred(session_id: &str, force: bool) -> Option<super::Part> {
    // Harnesses without a Monitor tool (Agent SDK runs, brain.js NPCs) can't
    // comply — let them opt out of the line altogether.
    if std::env::var_os("BASE_NO_WAKE_NUDGE").is_some() {
        return None;
    }
    nudge_lines_for(session_id, force)
}

/// [`nudge_lines_deferred`] without the environment opt-out, so a test does not depend on the shell it runs in.
fn nudge_lines_for(session_id: &str, force: bool) -> Option<super::Part> {
    let mut out = String::new();
    let mut commits: Vec<super::Commit> = Vec::new();
    // Only a title in use (BO-27, V2): set by hand, or one that sent or was sent a ping. A title session start drew
    // for a session that never used relay is not told to arm anything.
    for title in super::session_registry::titles_in_use_for(session_id) {
        // armed_state, NOT is_watching: a live watcher running an older script must still be told once, or a fix to
        // the script never reaches the sessions already running one (grebe, 2026-09-20).
        let state = armed_state(&title);
        let Some(path) = nudge_path(&title) else { continue };
        let told = std::fs::read_to_string(&path).ok().map(|s| s.trim().to_string());
        let last = told.as_deref().zip(mtime(&path));
        let sentinel = sentinel_path(&title).and_then(|p| mtime(&p));
        if !nudge_due(&state, last, sentinel, session_id, force) {
            continue;
        }
        out.push_str(&nudge_line(&title, &state));
        out.push('\n');
        let sid = session_id.to_string();
        commits.push(Box::new(move || stamp_nudge(&path, &sid)));
    }
    let items = out.lines().count();
    (!out.is_empty()).then_some(super::Part { text: out, commits, items })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE MIGRATION LEG, and the one that decides whether this fix reaches anybody.
    /// Every monitor alive on this machine only touches the sentinel, so it writes
    /// NOTHING. If empty read as "no data, assume fine", every running session would
    /// stay silent forever and the fix would never reach a seat — the defect reproduced
    /// by its own fix. auk asked for this case to be named on its own, and it is.
    #[test]
    fn an_empty_sentinel_is_outdated_never_a_pass() {
        assert_eq!(armed_from("", true), Armed::Outdated);
    }

    /// A monitor running today's template under its own title is the only Current case.
    #[test]
    fn a_matching_fingerprint_is_current() {
        let fp = template_fingerprint();
        assert_eq!(armed_from(&format!("{fp} finch"), true), Armed::Current);
    }

    /// The control that stops every leg above passing on a function that calls
    /// everything Outdated.
    #[test]
    fn an_older_template_is_outdated() {
        assert_eq!(armed_from("0000000000000000 finch", true), Armed::Outdated);
    }

    /// A retitle needs no field of its own: the sentinel for a new title is a new PATH
    /// with no file behind it, and `armed_state` returns `NotWatching` for a missing
    /// sentinel. This leg pins the piece that lives in this function — anything after
    /// the fingerprint is ignored, so a stale second field cannot make a stale monitor
    /// read Current.
    #[test]
    fn anything_after_the_fingerprint_is_ignored() {
        let fp = template_fingerprint();
        assert_eq!(armed_from(&format!("{fp} plover"), true), Armed::Current);
        assert_eq!(armed_from("0000000000000000 finch", true), Armed::Outdated);
    }

    /// No live monitor beats every other reading: a perfect sentinel that has gone stale
    /// is NotWatching, not Current.
    #[test]
    fn a_stale_sentinel_is_not_watching_whatever_it_says() {
        let fp = template_fingerprint();
        assert_eq!(armed_from(&format!("{fp} finch"), false), Armed::NotWatching);
    }

    /// Whitespace-only is not a fingerprint. A torn or blanked write must read Outdated,
    /// not Current.
    #[test]
    fn a_whitespace_only_sentinel_is_outdated() {
        assert_eq!(armed_from("   ", true), Armed::Outdated);
        assert_eq!(armed_from("
", true), Armed::Outdated);
    }

    /// Whitespace and a trailing newline must not change the reading. The script writes
    /// with `printf` and no newline today, but a future shell or editor adding one must
    /// not make every session read as Outdated - that is the permanently-red failure
    /// auk ruled against.
    #[test]
    fn the_gate_tolerates_surrounding_whitespace() {
        let fp = template_fingerprint();
        assert_eq!(armed_from(&format!("  {fp}  
"), true), Armed::Current);
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

    /// F4c: the text `base relay arm` prints carries the host Monitor tool's fields and nothing it does not have.
    #[test]
    fn arm_text_carries_the_sentinel_write_and_the_host_monitor_fields() {
        let tmp = tempfile::tempdir().unwrap();
        crate::home::with_thread_home(tmp.path(), || {
            let text = arm_text_for("wake-test-title", "Pat").expect("a home resolves an inbox");
            // CHANGED 2026-09-20 WITH THE CONTRACT IT PINS: the loop WRITES the sentinel, with the template
            // fingerprint, which is what the gate reads.
            assert!(text.contains("> \"$INBOX/.watching\""), "the loop must WRITE the sentinel, not touch it");
            assert!(text.contains(&template_fingerprint()), "the text must carry the template fingerprint");
            assert!(text.contains("description: relay wake: wake-test-title"));
            assert!(text.contains(&format!("timeout_ms:  {MONITOR_TIMEOUT_MS} ")), "{text}");
            assert_eq!(MONITOR_TIMEOUT_MS, 1_800_000, "the host maximum on Claude Code 2.1.286 and 2.1.287");
            assert!(text.contains("command:\n    INBOX="), "{text}");
            assert!(text.contains("When the Monitor reports that it expired, run `base relay arm` again."));
            assert!(!text.contains("persistent"), "the host Monitor tool has no persistent field:\n{text}");
            assert!(text.contains("relay-inbox"));
            assert!(text.contains("do not start a second one"));
            // Issue #11 / #13: the operator comes from the profile, never the binary.
            assert!(text.contains("shown to Pat"));
            assert!(!text.contains("shown to Chris"), "the home path may contain a name; the sentence must not");
            assert!(text.contains("relay.wake_nudge false"));
            assert!(text.contains("base relay register --as wake-test-title"));
        });
    }

    /// The pure rule behind F4b: once per session, once per stale event, never while a current watcher runs.
    #[test]
    fn nudge_due_is_once_per_session_and_once_per_stale_event() {
        let now = std::time::SystemTime::now();
        let ago = |s: u64| now - std::time::Duration::from_secs(s);
        let w = Armed::NotWatching;
        assert!(nudge_due(&w, None, None, "a", false), "never told: due");
        assert!(!nudge_due(&w, Some(("a", ago(100))), None, "a", false), "told this session, no watcher since");
        assert!(nudge_due(&w, Some(("b", ago(100))), None, "a", false), "told another session only");
        assert!(nudge_due(&w, Some(("a", ago(600))), Some(ago(60)), "a", false), "a watcher ran after, and died");
        assert!(!nudge_due(&w, Some(("a", ago(100))), Some(ago(200)), "a", false), "the watcher died before the line");
        assert!(!nudge_due(&Armed::Current, None, None, "a", true), "a current watcher is never nudged, forced or not");
        assert!(!nudge_due(&Armed::Outdated, Some(("a", ago(100))), Some(now), "a", false), "an old script: once");
        assert!(nudge_due(&Armed::Outdated, Some(("b", ago(100))), Some(now), "a", false));
        assert!(nudge_due(&w, Some(("a", ago(1))), None, "a", true), "session start always tells a fresh context");
    }

    fn set_mtime(p: &std::path::Path, t: std::time::SystemTime) {
        std::fs::File::options().write(true).open(p).unwrap().set_modified(t).unwrap();
    }

    /// BO-04 F4b, end to end on disk: the line, never the script; once per session; once more per stale event.
    #[test]
    fn wake_nudge_once_per_session_and_once_per_stale_event() {
        let tmp = tempfile::tempdir().unwrap();
        crate::home::with_thread_home(tmp.path(), || {
            let home = tmp.path();
            crate::relay::session_registry::register("kite", "sid-A", home, None).unwrap();
            let lines = |sid: &str| -> Option<String> {
                nudge_lines_for(sid, false).map(crate::relay::Part::commit)
            };
            let first = lines("sid-A").expect("a title with no watcher is told");
            assert_eq!(
                first,
                "relay: kite has no inbox watcher · run base relay arm and start the Monitor it prints\n"
            );
            assert!(lines("sid-A").is_none(), "once per session");
            assert!(lines("sid-A").is_none(), "and still once");

            // A watcher ran after the line (the sentinel was written later) and has died: one stale event.
            let nudge = nudge_path("kite").unwrap();
            let sentinel = sentinel_path("kite").unwrap();
            let now = std::time::SystemTime::now();
            set_mtime(&nudge, now - std::time::Duration::from_secs(1_900));
            std::fs::write(&sentinel, template_fingerprint()).unwrap();
            set_mtime(&sentinel, now - std::time::Duration::from_secs(60));
            assert!(lines("sid-A").expect("a dead watcher is one event").contains("has no inbox watcher"));
            assert!(lines("sid-A").is_none(), "once per stale event");

            // A live watcher running today's script: nothing.
            std::fs::write(&sentinel, template_fingerprint()).unwrap();
            assert!(lines("sid-A").is_none(), "a current watcher is never nudged");

            // A live watcher on an older script: told once, in its own words.
            std::fs::write(&sentinel, "0000000000000000").unwrap();
            assert!(lines("sid-A").is_none(), "this session was already told");
            crate::relay::session_registry::register("kite", "sid-B", home, None).unwrap();
            assert!(lines("sid-B").expect("a new session is told").contains("runs an old script"));
            assert!(lines("sid-B").is_none());
        });
    }

    #[test]
    fn stamp_moves_a_stale_throttle_file_to_now() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join(".watch-nudge");
        std::fs::write(&p, b"").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(1_800);
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(old).unwrap();
        assert!(age_secs(&p).unwrap() >= 1_800, "precondition: stale");
        stamp(&p);
        assert!(age_secs(&p).unwrap() < 60, "stamp must read as just touched");
        // Through a missing parent too — the first nudge for a fresh title.
        let deep = tmp.path().join("a").join("b").join(".watch-nudge");
        stamp(&deep);
        assert!(age_secs(&deep).is_some());
    }
}
