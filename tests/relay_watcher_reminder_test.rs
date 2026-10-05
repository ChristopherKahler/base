//! BO-27, V2 and V3: the hooks remind a session to start its relay inbox watcher only when its title is in use: set by
//! hand (`base relay register --as`, or pinned at launch by `BASE_RELAY_AS`), or it has sent or been sent a ping.
//!
//! Before BO-27 session start gave every session a codename and the hooks then told each one to arm a watcher, so a
//! user who never used relay was told in every session to start a background Monitor. A session titled only by session
//! start now hears nothing about relay; one that is pinged hears it from then on; one registered by hand hears it as
//! before.

mod seed;

use std::path::PathBuf;

use seed::{run_base_in_session, run_hook, run_pre_tool_use, Seed};

/// The reminder's own words, as `src/relay/wake.rs::nudge_line` prints it.
fn reminder_for(title: &str) -> String {
    format!("relay: {title} has no inbox watcher · run base relay arm and start the Monitor it prints")
}

fn fixture(tag: &str, global_toml: &str) -> Seed {
    let root: PathBuf = std::env::temp_dir().join(format!("base-bo27-relay-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    seed::write(&root, &seed::TINY, global_toml)
}

fn ok(out: (i32, String, String), what: &str) -> String {
    assert_eq!(out.0, 0, "{what} failed:\n{}\n{}", out.1, out.2);
    out.1
}

/// One session start, as Claude Code runs it, with `env` on top of the scrubbed environment (no launcher title unless
/// the test sets one).
fn start(s: &Seed, session: &str, env: &[(&str, &str)]) -> String {
    let payload = serde_json::json!({
        "cwd": s.ws.display().to_string(),
        "hook_event_name": "SessionStart",
        "source": "startup",
        "session_id": session,
    });
    ok(run_hook(s, "session-start", &payload, env), "session start")
}

fn prompt(s: &Seed, session: &str, env: &[(&str, &str)]) -> String {
    let payload = serde_json::json!({
        "cwd": s.ws.display().to_string(),
        "hook_event_name": "UserPromptSubmit",
        "prompt": "what is left on the list for today",
        "session_id": session,
    });
    ok(run_hook(s, "user-prompt-submit", &payload, env), "prompt")
}

fn tool(s: &Seed, session: &str, env: &[(&str, &str)]) -> String {
    ok(run_pre_tool_use(s, "Bash", serde_json::json!({ "command": "ls" }), session, env), "tool call")
}

/// The lines that open with `relay:`. Every relay line the hooks print opens this way (BO-04).
fn relay_lines(out: &str) -> Vec<&str> {
    out.lines().filter(|l| l.starts_with("relay:")).collect()
}

/// The registry row this session holds, read from the global tier's `sessions.json`.
fn row_of(s: &Seed, session: &str) -> serde_json::Value {
    let path = s.home.join(".base-gbl").join(".base").join("sessions.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let reg: serde_json::Value = serde_json::from_str(&text).expect("sessions.json");
    reg["sessions"]
        .as_object()
        .expect("a sessions map")
        .values()
        .find(|e| e["session_id"] == session)
        .unwrap_or_else(|| panic!("no row for session {session}:\n{text}"))
        .clone()
}

fn title_of(s: &Seed, session: &str) -> String {
    row_of(s, session)["title"].as_str().expect("a title").to_string()
}

/// V2: a session titled only by session start prints no relay line at session start, at a prompt or at a tool call,
/// across two starts (a start always repeats the reminder when it is due). Control: the same session, once registered
/// by hand, is reminded at once, so the silence was the title's use and not a hook that printed nothing.
#[test]
fn unused_title_prints_no_watcher_line() {
    let s = fixture("unused", "");
    let session = "bo27-unused";
    let first = start(&s, session, &[]);
    let title = title_of(&s, session);
    assert_eq!(row_of(&s, session)["auto"], true, "control: session start drew the title");
    let runs = [first, prompt(&s, session, &[]), tool(&s, session, &[]), start(&s, session, &[]), prompt(&s, session, &[])];
    for (i, out) in runs.iter().enumerate() {
        assert!(relay_lines(out).is_empty(), "run {i} of {} printed a relay line for {title}:\n{out}", runs.len());
        assert!(!out.contains("inbox watcher"), "run {i}: {out}");
    }

    ok(run_base_in_session(&s, &["relay", "register", "--as", &title], session), "relay register");
    let after = prompt(&s, session, &[]);
    assert!(after.contains(&reminder_for(&title)), "control: registered by hand, it is reminded:\n{after}");
}

/// V2, Example 3: session B has no watcher and a title session start drew. Session A pings that title. B's next prompt
/// shows the ping, as before BO-27, and the reminder with it; from then on B is reminded as a registered session is
/// (its next session start). The registry records the use on B's row.
#[test]
fn pinged_title_prints_the_watcher_line() {
    let s = fixture("pinged", "");
    let b = "bo27-b";
    let before = start(&s, b, &[]);
    let title = title_of(&s, b);
    assert!(relay_lines(&before).is_empty(), "control: unused until the ping:\n{before}");

    ok(run_base_in_session(&s, &["relay", "register", "--as", "sender-kite"], "bo27-a"), "register the sender");
    ok(
        run_base_in_session(&s, &["relay", "ping", "--to", &title, "--msg", "the nightly build is green"], "bo27-a"),
        "the ping",
    );
    assert_eq!(row_of(&s, b)["used_by"], b, "the ping marked the addressee's title as used by its holder");

    let next = prompt(&s, b, &[]);
    assert!(next.contains("the nightly build is green"), "the ping still reaches the unwatched session:\n{next}");
    assert!(next.contains(&reminder_for(&title)), "and the reminder comes with it:\n{next}");
    let later = start(&s, b, &[]);
    assert!(later.contains(&reminder_for(&title)), "from then on, as a registered session:\n{later}");
}

/// V2: the sending half. A session with only a drawn title that sends a ping now uses that title (replies come back to
/// it), so its next prompt carries the reminder.
#[test]
fn sending_a_ping_puts_the_title_in_use() {
    let s = fixture("sender", "");
    ok(run_base_in_session(&s, &["relay", "register", "--as", "peer-kite"], "bo27-peer"), "register the peer");
    let c = "bo27-c";
    let before = start(&s, c, &[]);
    let title = title_of(&s, c);
    assert!(relay_lines(&before).is_empty(), "control: unused before it sends:\n{before}");
    ok(run_base_in_session(&s, &["relay", "ping", "--to", "peer-kite", "--msg", "can you review the diff"], c), "send");
    assert_eq!(row_of(&s, c)["used_by"], c, "the send marked the sender's title");
    let next = prompt(&s, c, &[]);
    assert!(next.contains(&reminder_for(&title)), "{next}");
}

/// V3, Example 4: a title registered by hand is reminded as before BO-27, at session start; `wake_nudge = false` still
/// turns the reminder off for everyone, registered or not.
#[test]
fn hand_registered_title_prints_the_watcher_line() {
    let s = fixture("hand", "");
    let d = "bo27-d";
    ok(run_base_in_session(&s, &["relay", "register", "--as", "hand-kite"], d), "relay register");
    let out = start(&s, d, &[]);
    assert!(out.contains(&reminder_for("hand-kite")), "{out}");

    let quiet = fixture("hand-off", "[relay]\nwake_nudge = false\n");
    ok(run_base_in_session(&quiet, &["relay", "register", "--as", "hand-kite"], d), "relay register");
    let off = start(&quiet, d, &[]);
    assert!(!off.contains("inbox watcher"), "wake_nudge = false keeps it off:\n{off}");
}

/// V3: a launcher that pins the title with `BASE_RELAY_AS` chose it by hand, so the session is reminded from its first
/// session start. A session that took its launcher's title before this build (its row has no `used_by`) is marked at
/// its next hook. Control: the same session without the launcher variable stays quiet until then.
#[test]
fn launcher_pinned_title_prints_the_watcher_line() {
    let s = fixture("pinned", "");
    let pinned = start(&s, "bo27-e", &[("BASE_RELAY_AS", "pinned-kite")]);
    assert!(pinned.contains(&reminder_for("pinned-kite")), "{pinned}");

    let f = "bo27-f";
    let quiet = start(&s, f, &[]);
    let title = title_of(&s, f);
    assert!(relay_lines(&quiet).is_empty(), "control: no launcher, no use:\n{quiet}");
    let launched = prompt(&s, f, &[("BASE_RELAY_AS", title.as_str())]);
    assert!(launched.contains(&reminder_for(&title)), "a running launcher session keeps its reminder:\n{launched}");
}
