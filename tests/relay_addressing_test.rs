//! BO-05: a ping reaches the session it was sent to, not whoever holds the title now (F12).
//!
//! Driven through the real binary, as Claude Code drives it: `base relay ...` with `CLAUDE_CODE_SESSION_ID` set, and the
//! session-start and prompt hooks with JSON on stdin. The seed's hook runner sets `BASE_RELAY_AS=seed-kite` on every
//! hook it runs for a session, as the operator's launchers do (cx.cmd, launch-tab-direct.ps1), so a session that holds
//! no title is auto-titled `seed-kite` on its first hook: the path a new session takes a codename by.
//!
//! What happened on 2026-10-01: session 5b860473 was auto-given the title `lynx`, inherited the inbox of the session
//! that had held it, and was told on every prompt that bison was waiting on it about a doc it had never seen. bison
//! had no way to know its pings had reached the wrong session.

mod seed;

use std::path::{Path, PathBuf};

use seed::{run_base, run_base_in_session, run_prompt_submit, run_session_start};

/// The receiver's title: the one the seed's hook runner auto-titles a session with.
const KITE: &str = "seed-kite";
const BISON: &str = "seed-bison";

fn root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("base-relay-addressing-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    assert!(!root.exists(), "the seed root {} survived its clean", root.display());
    root
}

/// A seed whose budgets fit everything, so a block is never missing for a budget reason.
fn fixture(tag: &str) -> seed::Seed {
    seed::write(&root(tag), &seed::TINY, "[budget]\nprompt_bytes = 50000\nsession_start_bytes = 50000\n")
}

fn ok(out: (i32, String, String), what: &str) -> String {
    let (code, stdout, stderr) = out;
    assert_eq!(code, 0, "{what} failed: {stderr}\n{stdout}");
    stdout
}

fn register(s: &seed::Seed, title: &str, session: &str) {
    ok(run_base_in_session(s, &["relay", "register", "--as", title], session), "relay register");
}

/// A ping sent from `session`, which holds the sender's title, to `to`.
fn ping_from(s: &seed::Seed, session: &str, to: &str, msg: &str) {
    ok(run_base_in_session(s, &["relay", "ping", "--to", to, "--msg", msg], session), "relay ping");
}

fn prompt(s: &seed::Seed, session: &str) -> String {
    ok(run_prompt_submit(s, "carry on with the build", Some(session)), "prompt hook")
}

fn start(s: &seed::Seed, session: &str) -> String {
    ok(run_session_start(s, Some(session)), "session start")
}

fn inbox_root(s: &seed::Seed) -> PathBuf {
    s.home.join(".base-gbl").join(".base").join("relay-inbox")
}

/// The JSON items directly in `dir`, parsed, oldest name first.
fn items_in(dir: &Path) -> Vec<serde_json::Value> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    paths.iter().map(|p| serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()).collect()
}

fn inbox(s: &seed::Seed, title: &str) -> Vec<serde_json::Value> {
    items_in(&inbox_root(s).join(title))
}

fn archive(s: &seed::Seed, title: &str, session: &str) -> Vec<serde_json::Value> {
    items_in(&inbox_root(s).join(".archive").join(format!("{title}-{session}")))
}

/// The not-delivered notices in `title`'s inbox.
fn notices(s: &seed::Seed, title: &str) -> Vec<serde_json::Value> {
    inbox(s, title).into_iter().filter(|t| t["kind"] == "undelivered").collect()
}

fn text(v: &serde_json::Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_string()
}

/// `hours` before now, as base stamps an inbox item.
fn hours_ago(hours: i64) -> String {
    (chrono::Local::now() - chrono::Duration::hours(hours)).format("%Y-%m-%dT%H:%M:%S%z").to_string()
}

/// Write an inbox item for `title` the way a binary from before BO-05 could have: no `to_session` key at all.
fn legacy_ping(s: &seed::Seed, title: &str, slug: &str, from: &str, msg: &str, created: &str) {
    let dir = inbox_root(s).join(title);
    std::fs::create_dir_all(&dir).unwrap();
    let item = serde_json::json!({
        "slug": slug, "summary": msg, "doc": "", "from": from, "to_title": title, "priority": "high",
        "created": created, "status": "pending", "last_loud_session": "", "last_alert_ts": "", "kind": "ping",
    });
    std::fs::write(dir.join(format!("{slug}.json")), serde_json::to_string_pretty(&item).unwrap()).unwrap();
}

/// Example 2: `base relay ping --to <title>` stores the id of the session holding the title at send time, and the
/// sending session's id when it holds the sender's title. The fields sit where the inbox watcher's script reads them:
/// it takes the message from between `"summary"` and the `"doc"` key right after it.
#[test]
fn ping_carries_target_session_id() {
    let s = fixture("carries-id");
    register(&s, KITE, "5b860473-5522-5a45-85ed-de1df8fca274");
    register(&s, BISON, "sess-bison");
    ping_from(&s, "sess-bison", KITE, "stand down on the Everything In Progress doc");

    let items = inbox(&s, KITE);
    assert_eq!(items.len(), 1, "{items:?}");
    let ping = &items[0];
    assert_eq!(ping["from"], BISON);
    assert_eq!(ping["to_title"], KITE);
    assert_eq!(ping["to_session"], "5b860473-5522-5a45-85ed-de1df8fca274", "{ping}");
    assert_eq!(ping["from_session"], "sess-bison", "{ping}");
    assert_eq!(ping["summary"], "stand down on the Everything In Progress doc");

    // The watcher script (wake.rs) reads `"summary": "<message>",` followed by `"doc"` once newlines are removed.
    let raw = std::fs::read_dir(inbox_root(&s).join(KITE))
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy().starts_with("ping-"))
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .expect("the file is named ping-*.json, the name the watcher lists");
    let flat = raw.replace(['\n', '\r'], "");
    let after = flat.split("\"summary\": \"stand down on the Everything In Progress doc\",").nth(1).expect("summary");
    assert!(after.trim_start().starts_with("\"doc\""), "the key after the message is not doc: {flat}");

    // A process sending as a title it does not hold records no sending session.
    ok(run_base(&s, &["relay", "ping", "--from", "seed-heron", "--to", KITE, "--msg", "from a script"]), "relay ping");
    let script = inbox(&s, KITE).into_iter().find(|t| t["from"] == "seed-heron").expect("the script's ping");
    assert_eq!(script["to_session"], "5b860473-5522-5a45-85ed-de1df8fca274");
    assert!(script.get("from_session").is_none(), "{script}");
}

/// F12b: delivery checks the id. A ping in the title's folder that carries another session's id (its sender resolved
/// the title a moment before it changed hands) is never shown to the holder: it is archived in its own session's
/// folder and its sender is told. The holder's own ping is shown as before.
#[test]
fn ping_delivered_only_to_matching_session() {
    let s = fixture("matching");
    register(&s, BISON, "sess-bison");
    register(&s, KITE, "sess-A");
    ping_from(&s, "sess-bison", KITE, "for session A");
    // The same sender's ping, written for a session that held the title before A.
    let mut stray = inbox(&s, KITE).remove(0);
    stray["slug"] = "ping-1000000000000-0000-0".into();
    stray["summary"] = "for the session before A".into();
    stray["to_session"] = "sess-gone".into();
    std::fs::write(inbox_root(&s).join(KITE).join("ping-1000000000000-0000-0.json"), stray.to_string()).unwrap();

    let shown = prompt(&s, "sess-A");
    assert!(shown.contains("for session A"), "{shown}");
    assert!(!shown.contains("for the session before A"), "another session's ping was shown:\n{shown}");
    assert!(!shown.contains("hidden"), "another session's ping is not part of A's thread with bison:\n{shown}");
    let archived = archive(&s, KITE, "sess-gone");
    assert_eq!(archived.len(), 1, "{archived:?}");
    assert_eq!(archived[0]["summary"], "for the session before A");
    assert_eq!(notices(&s, BISON).len(), 1, "bison is told its ping to the earlier session was not delivered");
    assert!(!start(&s, "sess-A").contains("for the session before A"));
}

/// Example 1: the title's holder ended with two of bison's pings unshown, and a new session is auto-titled with it.
/// The old holder's items move to `.archive/<title>-<old session>/`, the new holder starts with an empty inbox and is
/// shown nothing of the old holder's, at its start or on its prompts.
#[test]
fn title_reassignment_archives_old_inbox() {
    let s = fixture("reassign");
    register(&s, BISON, "sess-bison");
    register(&s, KITE, "sess-old");
    ping_from(&s, "sess-bison", KITE, "go: edit the Everything In Progress doc");
    ping_from(&s, "sess-bison", KITE, "stand down on the doc, the go is withdrawn");
    let slugs: Vec<String> = inbox(&s, KITE).iter().map(|t| text(t, "slug")).collect();
    assert_eq!(slugs.len(), 2, "control: two pings wait for sess-old");

    let first = start(&s, "sess-new");
    let sessions = ok(run_base(&s, &["relay", "sessions"]), "relay sessions");
    assert!(sessions.contains(&format!("{KITE}  [")) && sessions.contains("session:sess-new"), "control: auto-titled:\n{sessions}");
    let later = [prompt(&s, "sess-new"), prompt(&s, "sess-new"), start(&s, "sess-new")].join("\n");
    for out in [&first, &later] {
        assert!(!out.contains("Everything In Progress") && !out.contains("stand down"), "{out}");
        assert!(!out.contains("unanswered"), "{out}");
    }
    assert!(inbox(&s, KITE).iter().all(|t| t["kind"] == "undelivered"), "the new holder's inbox holds nothing of sess-old's");
    let archived: Vec<String> = archive(&s, KITE, "sess-old").iter().map(|t| text(t, "slug")).collect();
    assert_eq!(archived, slugs, "the two pings moved, unchanged, to the old holder's archive folder");
}

/// Example 1, the reply: each sender of an undelivered ping is told, once, in its own inbox, addressed to the session
/// that sent it: who the title belonged to, that the pings were not delivered, who holds it now, and where they are.
/// A sender whose ping was delivered is not told. The notice is shown once, as information.
#[test]
fn senders_told_when_ping_not_delivered() {
    let s = fixture("told");
    register(&s, BISON, "sess-bison");
    register(&s, "seed-heron", "sess-heron");
    register(&s, KITE, "sess-old");
    ping_from(&s, "sess-heron", KITE, "heron's question");
    assert!(prompt(&s, "sess-old").contains("heron's question"), "control: heron's ping is delivered to sess-old");
    ping_from(&s, "sess-bison", KITE, "go: edit the doc");
    ping_from(&s, "sess-bison", KITE, "stand down on the doc");

    start(&s, "sess-new");
    assert!(notices(&s, "seed-heron").is_empty(), "a delivered ping's sender is not told");
    let told = notices(&s, BISON);
    assert_eq!(told.len(), 1, "one notice for both pings: {told:?}");
    let notice = &told[0];
    assert_eq!(notice["to_session"], "sess-bison");
    assert_eq!(notice["from"], "relay");
    assert!(text(notice, "slug").starts_with("ping-"), "named so the sender's inbox watcher wakes it");
    let summary = text(notice, "summary");
    for part in [
        "your 2 pings to seed-kite (sent ",
        ") were not delivered. seed-kite was session sess-old, which no longer holds it; ",
        "the title now belongs to session sess-new (since ",
        "), which has not seen them. Archived at ",
        "/relay-inbox/.archive/seed-kite-sess-old/. Re-send to the new holder if they still apply.",
    ] {
        assert!(summary.contains(part), "{part:?} not in: {summary}");
    }

    let shown = prompt(&s, "sess-bison");
    let header = shown.lines().find(|l| l.starts_with("relay: not delivered (")).unwrap_or_else(|| panic!("{shown}"));
    assert!(header.ends_with(") · no answer needed"), "{header}");
    assert!(shown.lines().any(|l| l == summary), "the notice is its own line:\n{shown}");
    let again = [prompt(&s, "sess-bison"), start(&s, "sess-bison")].join("\n");
    assert!(!again.contains("not delivered"), "shown once:\n{again}");
    assert!(notices(&s, BISON).is_empty(), "consumed once shown");
}

/// Example 3: bison pings while session 1 holds the title, so the ping carries session 1. Session 1 ends unshown and
/// session 2 takes the title. The ping is not delivered to session 2: it is archived with session 1's inbox and bison
/// is told. The same holds when the ping lands only after the title changed hands, still carrying session 1.
#[test]
fn title_changes_between_send_and_delivery() {
    let s = fixture("between");
    register(&s, BISON, "sess-bison");
    register(&s, KITE, "sess-1");
    ping_from(&s, "sess-bison", KITE, "sent at 14:00 to session 1");
    assert_eq!(inbox(&s, KITE)[0]["to_session"], "sess-1", "control: the ping carries session 1");
    let copy = inbox(&s, KITE).remove(0);

    register(&s, KITE, "sess-2");
    assert!(inbox(&s, KITE).is_empty(), "session 2 starts with an empty inbox");
    assert_eq!(archive(&s, KITE, "sess-1").len(), 1);
    assert_eq!(notices(&s, BISON).len(), 1, "bison is told");
    assert!(!prompt(&s, "sess-2").contains("sent at 14:00"));

    // The late arrival: written after the title passed, still carrying session 1.
    let mut late = copy;
    late["slug"] = "ping-2000000000000-0000-0".into();
    late["summary"] = "landed after the title passed".into();
    std::fs::write(inbox_root(&s).join(KITE).join("ping-2000000000000-0000-0.json"), late.to_string()).unwrap();
    let shown = prompt(&s, "sess-2");
    assert!(!shown.contains("landed after the title passed"), "{shown}");
    assert_eq!(archive(&s, KITE, "sess-1").len(), 2, "archived with session 1's inbox");
    assert_eq!(notices(&s, BISON).len(), 2, "and bison told again, for that one");
}

/// F12e: an item with no session id (written before this order) is treated as addressed to whoever held the title
/// when it was written, by the title history. Placed on the holder, it is delivered and records the id; placed on an
/// earlier holder, it is archived in that holder's folder; with no history that far back it cannot be known, so it
/// is archived, never delivered: under `unknown` when no title changed hands, with the previous holder's inbox when
/// one did. A sender is told when the history also says which session sent the item, and only then.
#[test]
fn legacy_ping_without_session_id() {
    let s = fixture("legacy");
    register(&s, BISON, "sess-bison");
    register(&s, "seed-heron", "sess-heron");
    register(&s, KITE, "sess-2");
    // The history as it reads after the title passed from session 1 to session 2 an hour ago, with bison held by one
    // session for six hours. heron's history starts now.
    let history = inbox_root(&s).parent().unwrap().join("title-history.jsonl");
    let mut lines = std::fs::read_to_string(&history).unwrap();
    for (title, session, since) in
        [(KITE, "sess-1", hours_ago(3)), (KITE, "sess-2", hours_ago(1)), (BISON, "sess-bison", hours_ago(7))]
    {
        lines.push_str(&serde_json::json!({ "title": title, "session_id": session, "since": since }).to_string());
        lines.push('\n');
    }
    std::fs::write(&history, lines).unwrap();

    legacy_ping(&s, KITE, "ping-legacy-now", BISON, "written while session 2 held it", &hours_ago(0));
    legacy_ping(&s, KITE, "ping-legacy-earlier", BISON, "written while session 1 held it", &hours_ago(2));
    legacy_ping(&s, KITE, "ping-legacy-unknown", BISON, "written before any history", &hours_ago(5));
    legacy_ping(&s, KITE, "ping-legacy-heron", "seed-heron", "heron's, from before heron's history", &hours_ago(2));

    let shown = prompt(&s, "sess-2");
    assert!(shown.contains("written while session 2 held it"), "{shown}");
    assert!(!shown.contains("written while session 1 held it") && !shown.contains("before any history"), "{shown}");
    let kept = inbox(&s, KITE).into_iter().find(|t| t["slug"] == "ping-legacy-now").expect("delivered, kept");
    assert_eq!(kept["to_session"], "sess-2", "the item records the session the history placed it on");
    assert!(!shown.contains("heron's, from before"), "{shown}");
    assert_eq!(archive(&s, KITE, "sess-1").len(), 2, "placed on session 1, archived in its folder");
    assert_eq!(archive(&s, KITE, "unknown").len(), 1, "cannot be known: archived, not delivered");
    // The history places bison's items on bison's one session, so bison is told. It cannot say which session sent
    // heron's (heron's history starts now), so heron is not: telling heron's holder might tell a session that never
    // sent it.
    assert!(notices(&s, "seed-heron").is_empty(), "{:?}", notices(&s, "seed-heron"));
    let told: Vec<String> = notices(&s, BISON).iter().map(|n| text(n, "summary")).collect();
    assert_eq!(told.len(), 2, "{told:?}");
    assert!(told.iter().any(|t| t.contains("seed-kite was session sess-1, which no longer holds it")), "{told:?}");
    assert!(
        told.iter().any(|t| t.contains("it carries no session id, and base has no record of which session held seed-kite")),
        "{told:?}"
    );

    // At a change of holder, an item the history cannot place goes with the previous holder's inbox.
    legacy_ping(&s, KITE, "ping-legacy-older", BISON, "also before any history", &hours_ago(6));
    register(&s, KITE, "sess-3");
    let with_previous: Vec<String> = archive(&s, KITE, "sess-2").iter().map(|t| text(t, "slug")).collect();
    assert!(with_previous.contains(&"ping-legacy-older".to_string()), "{with_previous:?}");
    assert!(!prompt(&s, "sess-3").contains("before any history"));
}
