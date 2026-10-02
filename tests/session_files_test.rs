//! BO-06 (F11): each session keeps its own hook-output files, in `.base/hook-output/<session id>/`, and the hook points
//! at them.
//!
//! On 2026-10-01 six sessions wrote one workspace's `last-session-start.md`; when lynx opened it, it held robin's start,
//! not lynx's, and the instruction "never guess; run it" sent the reader there. The letters file did the same to
//! `base handoff show A`. Reproduced on a copy of the operator's store at 50295b9: inside session A, `show A` opened the
//! handoff session B's start had lettered A (FINAL STATE of BO-06).

mod seed;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use seed::{run_base, run_base_in_session, run_prompt_submit, run_session_start, Seed};

const S1: &str = "b0600000-0000-4000-8000-0000000000a1";
const S2: &str = "b0600000-0000-4000-8000-0000000000b2";

fn root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("base-bo06-files-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    root
}

fn store(tag: &str, global_toml: &str) -> Seed {
    let sizes = seed::Sizes { open_handoffs: 3, archived_handoffs: 0, open_forks: 2, archived_forks: 0, ..seed::TINY };
    seed::write(&root(tag), &sizes, global_toml)
}

fn session_dir(s: &Seed, session: &str) -> PathBuf {
    s.ws.join(".base").join("hook-output").join(session)
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The Letters line a session start printed.
fn letters_line(out: &str) -> String {
    out.lines().find(|l| l.starts_with("Letters: ")).unwrap_or_else(|| panic!("no Letters line:\n{out}")).to_string()
}

/// A handoff newer than every seeded one, so the next session start letters it A.
fn register_newer_handoff(s: &Seed) -> String {
    let doc = s.ws.join("bo06-newer-handoff.md");
    std::fs::write(&doc, "# newer\n").expect("handoff doc");
    let slug = "2099-01-01-0000-otter-bo06-newer";
    let doc_arg = doc.display().to_string();
    let (code, out, err) =
        run_base(s, &["handoff", "create", "--project", "bo06-newer", "--doc", &doc_arg, "--slug", slug]);
    assert_eq!(code, 0, "handoff create: {out}{err}");
    slug.to_string()
}

/// F11a, F11c. Two sessions start in one workspace, a newer handoff registered between them, and each runs a prompt.
/// Each keeps its own untrimmed session start and its own last prompt's output; the workspace files hold the latest of
/// either. Before BO-06 there was one file of each, and after the second start the first session's was gone.
#[test]
fn full_output_written_per_session() {
    let s = store("per-session", "[bracket]\nenabled = true\n");
    let (code, out1, err) = run_session_start(&s, Some(S1));
    assert_eq!(code, 0, "{err}");
    register_newer_handoff(&s);
    let (code, out2, err) = run_session_start(&s, Some(S2));
    assert_eq!(code, 0, "{err}");
    let (l1, l2) = (letters_line(&out1), letters_line(&out2));
    assert_ne!(l1, l2, "control: the two starts lettered different handoffs");

    let own1 = read(&session_dir(&s, S1).join("session-start.md"));
    let own2 = read(&session_dir(&s, S2).join("session-start.md"));
    assert!(own1.contains(&l1) && !own1.contains(&l2), "session 1's file holds session 1's start:\n{own1}");
    assert!(own2.contains(&l2) && !own2.contains(&l1), "session 2's file holds session 2's start:\n{own2}");
    assert_eq!(read(&s.ws.join(".base").join("last-session-start.md")), own2, "the workspace copy is the latest start");

    for (session, prompt) in [(S1, "first prompt of session one"), (S2, "first prompt of session two")] {
        let (code, _, err) = run_prompt_submit(&s, prompt, Some(session));
        assert_eq!(code, 0, "{err}");
    }
    let (code, _, err) = run_prompt_submit(&s, "second prompt of session one", Some(S1));
    assert_eq!(code, 0, "{err}");
    let p1 = read(&session_dir(&s, S1).join("prompt-submit.md"));
    let p2 = read(&session_dir(&s, S2).join("prompt-submit.md"));
    assert!(p1.contains("(prompt 2)"), "session 1's file is its own latest prompt:\n{p1}");
    assert!(p2.contains("(prompt 1)"), "session 2's file is its own, untouched by session 1's later prompt:\n{p2}");
    assert_eq!(read(&s.ws.join(".base").join("last-prompt-submit.md")), p1, "the workspace copy is the latest prompt");
}

/// F11b, Example 3. Line 1 names the session's own file, and that file holds what was trimmed away. With no session id
/// from the host there is no session folder, and line 1 names the workspace file, as before.
#[test]
fn hook_output_points_at_own_session_file() {
    let s = store("pointer", "");
    let (code, out, err) = run_session_start(&s, Some(S1));
    assert_eq!(code, 0, "{err}");
    let own = session_dir(&s, S1).join("session-start.md");
    let line1 = out.lines().next().unwrap_or_default();
    assert!(line1.ends_with(&format!("· full: {}]", own.display())), "line 1 names this session's file: {line1}");
    assert!(read(&own).contains(&letters_line(&out)), "and the file is this start's text");

    let (code, out, err) = run_session_start(&s, None);
    assert_eq!(code, 0, "{err}");
    let shared = s.ws.join(".base").join("last-session-start.md");
    let line1 = out.lines().next().unwrap_or_default();
    assert!(line1.ends_with(&format!("· full: {}]", shared.display())), "control, no session: {line1}");
}

/// F11d, Example 4. Session 1 starts; a newer handoff is registered; session 2 starts and letters it A. Inside session
/// 1, `base handoff show A` still opens the handoff session 1 was shown as A; inside session 2 it opens the new one.
/// Before BO-06 both read the one workspace file, so session 1's A was session 2's. A session that never started here
/// has no letters of its own, and `show` says so rather than reading another session's.
#[test]
fn handoff_letters_per_session() {
    let s = store("letters", "");
    let (code, out1, err) = run_session_start(&s, Some(S1));
    assert_eq!(code, 0, "{err}");
    let a1 = letters_line(&out1)
        .split_whitespace()
        .find_map(|w| w.strip_prefix("A="))
        .expect("session 1 lettered A")
        .to_string();
    let newer = register_newer_handoff(&s);
    let (code, out2, err) = run_session_start(&s, Some(S2));
    assert_eq!(code, 0, "{err}");
    assert!(letters_line(&out2).contains(&format!("A={newer}")), "control: session 2 letters the newer handoff A");
    assert_ne!(a1, newer, "control: the two sessions' A differ");

    let (code, shown1, err) = run_base_in_session(&s, &["handoff", "show", "A"], S1);
    assert_eq!(code, 0, "{err}");
    assert!(shown1.contains(&format!("handoff: {a1} ")), "session 1's A is still {a1}:\n{shown1}");
    let (code, shown2, err) = run_base_in_session(&s, &["handoff", "show", "A"], S2);
    assert_eq!(code, 0, "{err}");
    assert!(shown2.contains(&format!("handoff: {newer} ")), "session 2's A is {newer}:\n{shown2}");

    let (_, shown3, _) = run_base_in_session(&s, &["handoff", "show", "A"], "b0600000-never-started");
    assert!(
        shown3.contains("this session (b0600000-never-started) has no letters file here"),
        "a session with no start here is told so, not handed another session's letters:\n{shown3}"
    );
    let numbers = read(&session_dir(&s, S1).join("letters.json"));
    assert!(numbers.contains("\"reminders\""), "the session's DUE NOW numbers sit beside its letters:\n{numbers}");
}

/// BO-06 review findings 1 and 2. When a session's own letters file is missing (its write failed), the session reads
/// the workspace copy only while that copy says this session wrote it; once another session's start has replaced it,
/// the session gets no letters and is told so, never the other session's A.
#[test]
fn a_session_missing_its_own_letters_reads_only_its_own_workspace_copy() {
    let s = store("own-copy", "");
    let (code, out1, err) = run_session_start(&s, Some(S1));
    assert_eq!(code, 0, "{err}");
    let a1 = letters_line(&out1)
        .split_whitespace()
        .find_map(|w| w.strip_prefix("A="))
        .expect("session 1 lettered A")
        .to_string();
    let own = session_dir(&s, S1).join("letters.json");
    std::fs::remove_file(&own).expect("remove session 1's own letters");
    let (code, shown, err) = run_base_in_session(&s, &["handoff", "show", "A"], S1);
    assert_eq!(code, 0, "{err}");
    assert!(shown.contains(&format!("handoff: {a1} ")), "the workspace copy is session 1's, so it is read:\n{shown}");

    register_newer_handoff(&s);
    let (code, _, err) = run_session_start(&s, Some(S2));
    assert_eq!(code, 0, "{err}");
    assert!(!own.exists(), "control: session 1's own letters are still missing");
    let (_, shown, _) = run_base_in_session(&s, &["handoff", "show", "A"], S1);
    assert!(
        shown.contains(&format!("this session ({S1}) has no letters file here")),
        "the workspace copy is session 2's now, so session 1 is told it has none:\n{shown}"
    );
}

/// Set a file's modification time `days` ago.
fn age(path: &Path, days: u64) {
    let file = std::fs::OpenOptions::new().write(true).open(path).expect("open to set its time");
    file.set_modified(SystemTime::now() - Duration::from_secs(days * 24 * 60 * 60)).expect("set the time");
}

/// A session folder holding one file written `days` ago.
fn old_session(s: &Seed, session: &str, days: u64) -> PathBuf {
    let dir = session_dir(s, session);
    std::fs::create_dir_all(&dir).expect("session folder");
    std::fs::write(dir.join("session-start.md"), "old\n").expect("old file");
    age(&dir.join("session-start.md"), days);
    dir
}

/// F11e. Session start removes the folders of sessions not written for `[log] prompt_days` (90 by default, D14), and
/// keeps younger ones. A folder not named like a session id is never touched. The legacy per-session DUE NOW files
/// (`.base/due-now/<session>.json`, written until BO-06) go after the seven days their writer kept them.
#[test]
fn old_session_folders_pruned() {
    let s = store("prune", "");
    let ninety_one = old_session(&s, "b0600000-aged-91-days", 91);
    let fifty = old_session(&s, "b0600000-aged-50-days", 50);
    let odd = s.ws.join(".base").join("hook-output").join("not a session");
    std::fs::create_dir_all(&odd).expect("odd folder");
    std::fs::write(odd.join("keep.md"), "x").expect("odd file");
    age(&odd.join("keep.md"), 400);
    let legacy = s.ws.join(".base").join("due-now");
    std::fs::create_dir_all(&legacy).expect("legacy folder");
    for (name, days) in [("old-session.json", 8), ("recent-session.json", 1)] {
        std::fs::write(legacy.join(name), "{}").expect("legacy file");
        age(&legacy.join(name), days);
    }

    let (code, _, err) = run_session_start(&s, Some(S1));
    assert_eq!(code, 0, "{err}");
    assert!(!ninety_one.exists(), "a session folder 91 days old is removed at the default 90");
    assert!(fifty.exists(), "a session folder 50 days old is kept at the default 90");
    assert!(session_dir(&s, S1).join("session-start.md").is_file(), "control: this session's own folder is written");
    assert!(odd.join("keep.md").exists(), "a folder not named like a session id is never touched");
    assert!(!legacy.join("old-session.json").exists() && legacy.join("recent-session.json").exists(), "legacy files");

    // `[log] prompt_days` decides: at 30 the 50-day folder goes too.
    let toml = s.home.join(".base-gbl").join("base.toml");
    let text = read(&toml);
    std::fs::write(&toml, format!("{text}\n[log]\nprompt_days = 30\n")).expect("base.toml");
    let (code, _, err) = run_session_start(&s, Some(S2));
    assert_eq!(code, 0, "{err}");
    assert!(!fifty.exists(), "at prompt_days = 30 a 50-day folder is removed");
    assert!(session_dir(&s, S1).exists(), "the session that started a moment ago is kept");
}

/// Carried in from BO-00's review: the prompt hook wrote `last-prompt-submit.md` even with `[budget] write_full_output =
/// false`. Now neither hook writes any untrimmed text with it off, the session's or the workspace's, and line 1 says the
/// file was not written. The letters still are: `base handoff show <letter>` needs them.
#[test]
fn full_output_off_writes_no_full_text_files() {
    let s = store("off", "[bracket]\nenabled = true\n\n[budget]\nwrite_full_output = false\n");
    let (code, out, err) = run_session_start(&s, Some(S1));
    assert_eq!(code, 0, "{err}");
    let (code, prompt, err) = run_prompt_submit(&s, "a prompt with the full output off", Some(S1));
    assert_eq!(code, 0, "{err}");
    assert!(!prompt.is_empty(), "control: the prompt hook printed something it could have written");
    let base = s.ws.join(".base");
    for path in [
        base.join("last-session-start.md"),
        base.join("last-prompt-submit.md"),
        session_dir(&s, S1).join("session-start.md"),
        session_dir(&s, S1).join("prompt-submit.md"),
    ] {
        assert!(!path.exists(), "{} was written with write_full_output = false", path.display());
    }
    assert!(out.lines().next().unwrap_or_default().contains("full: not written ([budget] write_full_output = false)"));
    assert!(session_dir(&s, S1).join("letters.json").is_file(), "the letters are written either way");
}
