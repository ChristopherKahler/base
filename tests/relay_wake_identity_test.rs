//! Issue #132 — the `.watching` sentinel must name the session whose loop
//! touched it, so a retired session's loop never reads as the live titleholder.
//!
//! The mechanism, at `4bc5c1c`: `src/relay/wake.rs:32-33` puts the sentinel at
//! `relay-inbox/<title>/.watching`, `:89` has the armed monitor `touch` it every
//! five seconds, and `:51-58` turn its mtime alone into the watching state. A
//! retirement renames the registry title but does not move the monitor, which was
//! armed with an absolute path — so the predecessor keeps touching the
//! SUCCESSOR's sentinel. `wake.rs:14-18` already records this as a "Known edge".
//!
//! Isolation: every test runs inside `with_thread_home`, so both the registry
//! (`~/.base-gbl/.base/sessions.json`) and the inbox
//! (`~/.base-gbl/.base/relay-inbox/<title>/`) resolve inside a tempdir. `HOME`
//! is deliberately left alone, which keeps base's own write tripwire armed
//! (law 21 as amended: `HOME` equal to the fake root is what made a leak
//! indistinguishable from a legitimate write).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use base::home::with_thread_home;
use base::relay::session_registry;
use base::relay::wake;

/// The inbox directory for a title. Mirrors `task_inbox::inbox_root` and
/// `title_dir` (`src/relay/task_inbox.rs:106-112`), which are crate-private, so
/// this test builds the same path by hand rather than reaching into them.
fn inbox(root: &Path, title: &str) -> PathBuf {
    root.join(".base-gbl").join(".base").join("relay-inbox").join(title)
}

/// A faithful bare `touch`: the file exists, is EMPTY, and its mtime is now.
///
/// The mtime is set explicitly rather than left to the write, because a
/// zero-byte write over an existing zero-byte file does not move the mtime on
/// Windows — the defect `wake.rs:161-164` documents and works around.
fn bare_touch(p: &Path) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b"").unwrap();
    std::fs::File::options()
        .write(true)
        .open(p)
        .unwrap()
        .set_modified(SystemTime::now())
        .unwrap();
}

/// Backdate a path by `secs`.
fn age(p: &Path, secs: u64) {
    let when = SystemTime::now() - Duration::from_secs(secs);
    std::fs::File::options()
        .write(true)
        .open(p)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

/// Control leg: the nudge tests below measure whether an arming block is
/// produced, and `BASE_NO_WAKE_NUDGE` (`wake.rs:187`) suppresses it wholesale.
/// With that variable set, "no block" would be the right answer for the wrong
/// reason, before AND after the fix — a void result rather than a red one.
fn assert_nudge_is_observable() {
    assert!(
        std::env::var_os("BASE_NO_WAKE_NUDGE").is_none(),
        "BASE_NO_WAKE_NUDGE is set, which disables the arming block entirely; \
         this test cannot observe what it claims to measure"
    );
}

/// Requirement 4 — an old-shape monitor writes only the bare sentinel, naming
/// nobody. It must degrade to an identified state, never to Watching.
///
/// RED at `4bc5c1c`: `is_watching` is `fresh(mtime)` and nothing more, so a
/// touch by anything at all reads as this titleholder's own live monitor.
#[test]
fn a_bare_sentinel_naming_nobody_does_not_read_as_watching() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    with_thread_home(root, || {
        let holder = "11111111-aaaa-4bbb-8ccc-111111111111";
        session_registry::register("t132bare", holder, root, Some("base")).unwrap();
        bare_touch(&inbox(root, "t132bare").join(".watching"));

        let cell = wake::watch_cell("t132bare");
        // NARROW: this is the assertion this test was written for. It originally
        // named `is_watching`; that predicate had to keep its wide 0.15.0 meaning
        // (see the C8 assertion below), so the narrow question moved to its own
        // function. The claim is unchanged and still fails on the pre-fix tree.
        assert!(
            !wake::watching_by_holder("t132bare"),
            "a bare touch identifies no session, so it must not read as the \
             titleholder's own monitor; cell was {cell:?}"
        );
        assert!(
            cell.contains("unidentified"),
            "the state must be named rather than silently downgraded; cell was {cell:?}"
        );
        // WIDE, and this half is #132's C8: `is_watching` is what
        // `session_registry::pick_name` consults before handing a codename to
        // someone else. A loop IS running here, so it must stay true — narrowing
        // it frees the titles of live sessions whose heartbeat merely looks
        // stale. Measured on this machine 2026-09-10: 7 such titles at once.
        assert!(
            wake::is_watching("t132bare"),
            "a loop is touching this sentinel, so the title must NOT be reusable"
        );
    });
}

/// Requirement 3 — a successor must still be told to arm while a retired
/// predecessor's loop is touching the same path.
///
/// RED at `4bc5c1c`: `arm_blocks_for` skips on `is_watching` (`wake.rs:192`), so
/// the foreign loop's freshness silences the nudge and the successor never arms.
/// That is the "Known edge" at `wake.rs:14-18`, and it is the half of #132 that
/// leaves two sessions consuming one codename's pings.
#[test]
fn a_successor_is_still_told_to_arm_while_a_foreign_loop_touches() {
    assert_nudge_is_observable();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    with_thread_home(root, || {
        let successor = "22222222-aaaa-4bbb-8ccc-222222222222";
        session_registry::register("t132succ", successor, root, Some("base")).unwrap();
        // The retired predecessor's monitor, still running, still touching.
        bare_touch(&inbox(root, "t132succ").join(".watching"));

        assert!(
            wake::arm_blocks_for(successor, true).is_some(),
            "the successor holds this title and has no monitor of its own, so it \
             must be handed an arming block; a foreign loop must not silence it"
        );
    });
}

/// Positive control for the pair above: with NOTHING touching, today's reader
/// already answers correctly. Without this leg, a reader that always said "not
/// watching" would pass both tests above while being useless.
#[test]
fn control_an_untouched_sentinel_reads_never_and_nudges() {
    assert_nudge_is_observable();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    with_thread_home(root, || {
        let holder = "33333333-aaaa-4bbb-8ccc-333333333333";
        session_registry::register("t132never", holder, root, Some("base")).unwrap();
        std::fs::create_dir_all(inbox(root, "t132never")).unwrap();

        assert!(!wake::watching_by_holder("t132never"));
        assert_eq!(wake::watch_cell("t132never"), "✗ never");
        assert!(wake::arm_blocks_for(holder, true).is_some());
        // The WIDE predicate must be false too when nothing is touching at all.
        // Without this the C8 assertions elsewhere would pass on a predicate that
        // simply always returned true, which protects titles by being useless.
        assert!(
            !wake::is_watching("t132never"),
            "no loop is running, so this title IS free for reuse"
        );
    });
}

/// Requirement 2 — when ONLY a session that does not hold the title is
/// touching, the state is a NAMED foreign watcher, never Watching. This is the
/// `condor [DEAD · 15m]` row that cost four sessions a paragraph each.
#[test]
fn only_a_foreign_loop_touching_reads_as_a_named_foreign_watcher() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    with_thread_home(root, || {
        let holder = "55555555-aaaa-4bbb-8ccc-555555555555";
        let retired = "00000000-dead-4bee-8fff-000000000000";
        session_registry::register("t132foreign", holder, root, Some("base")).unwrap();
        // A retired predecessor's monitor, new shape: it touches the sentinel
        // AND writes its own identity. The holder's own monitor is gone.
        let dir = inbox(root, "t132foreign");
        bare_touch(&dir.join(".watching"));
        bare_touch(&dir.join(format!(".watching-by-{retired}")));

        assert!(
            !wake::watching_by_holder("t132foreign"),
            "a loop belonging to another session proves nothing about this holder"
        );
        assert!(
            wake::is_watching("t132foreign"),
            "but a loop IS running, so the title must not become reusable (C8)"
        );
        assert_eq!(wake::watch_cell("t132foreign"), "✗ foreign");
        let detail = wake::watch_detail("t132foreign").expect("a foreign watcher gets a footer line");
        assert!(
            detail.contains(retired),
            "the footer must name the watcher's FULL session id; got {detail:?}"
        );
        assert!(
            !detail.contains(holder),
            "it must not name the holder as the watcher; got {detail:?}"
        );
        assert!(matches!(
            wake::watch_state("t132foreign"),
            wake::WatchState::Foreign { .. }
        ));
    });
}

/// Requirement 3, the precedence that makes it work — while a retired
/// predecessor's loop is STILL touching, both siblings are fresh at once. The
/// titleholder's own must win, or a live successor would be libelled as foreign
/// for as long as its predecessor's monitor kept running.
#[test]
fn the_holders_own_loop_wins_while_a_predecessor_is_also_touching() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    with_thread_home(root, || {
        let holder = "66666666-aaaa-4bbb-8ccc-666666666666";
        let retired = "00000000-dead-4bee-8fff-000000000000";
        session_registry::register("t132both", holder, root, Some("base")).unwrap();
        let dir = inbox(root, "t132both");
        bare_touch(&dir.join(".watching"));
        bare_touch(&dir.join(format!(".watching-by-{retired}")));
        bare_touch(&dir.join(format!(".watching-by-{holder}")));

        assert!(
            wake::is_watching("t132both"),
            "the holder's own loop is running, so it IS watching"
        );
        assert_eq!(wake::watch_cell("t132both"), "✓");
        let detail = wake::watch_detail("t132both").expect("a footer line names the holder");
        assert!(detail.contains(holder), "got {detail:?}");
        assert!(
            detail.contains("registered holder"),
            "the footer must say this is the holder, not merely name an id; got {detail:?}"
        );
        // And the nudge must NOT fire: this session is genuinely covered.
        assert!(
            wake::arm_blocks_for(holder, true).is_none(),
            "a session whose own monitor is live must not be told to arm again"
        );
    });
}

/// C7 — the emitted skip condition must be an IDENTITY, not a behaviour.
///
/// Keying it on "your loop touches .watching" told every monitor armed before
/// this change to skip, because they all touch it. They would then never
/// re-arm, never write an identity, and draw the nudge every three minutes for
/// as long as they ran. The block must name the exact file instead.
#[test]
fn c7_the_skip_condition_names_the_identity_file_not_the_touch() {
    let sid = "77777777-c7c7-4bbb-8ccc-777777777777";
    let Some(block) = wake::arm_block("t132c7", Some(sid)) else {
        return;
    };
    assert!(
        block.contains(&format!("that writes .watching-by-{sid}, skip")),
        "the skip condition must name the identity file this session writes"
    );
    assert!(
        !block.contains("(its loop touches .watching), skip"),
        "the behaviour-keyed skip is what trapped every already-armed monitor"
    );
    assert!(
        block.contains("touches .watching but writes no such file"),
        "and it must tell an old-shape monitor, in those terms, to replace itself"
    );
}

/// C9 — with no registered holder there is nothing to be foreign TO.
///
/// The board iterates a per-project store; `watch_state` resolves the global
/// registry. Measured 2026-09-10: 118 rows in one store against 27 global
/// titles, only 9 in both. Calling the other 109 "foreign" would be an
/// accusation the data cannot support.
#[test]
fn c9_no_registered_holder_is_unverified_never_foreign() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    with_thread_home(root, || {
        let toucher = "99999999-c9c9-4bbb-8ccc-999999999999";
        // Deliberately NOT registered: this is the 109-row case.
        let dir = inbox(root, "t132c9");
        bare_touch(&dir.join(".watching"));
        bare_touch(&dir.join(format!(".watching-by-{toucher}")));

        let st = wake::watch_state_for("t132c9", None);
        assert!(
            matches!(st, wake::WatchState::UnknownHolder { .. }),
            "with no holder the state is unverified, not foreign; got {st:?}"
        );
        assert_eq!(wake::watch_cell_for("t132c9", None), "✗ unverified");
        let d = wake::watch_detail_for("t132c9", None).expect("it still gets a footer line");
        assert!(d.contains(toucher), "and it still names the toucher in full: {d:?}");

        // CONTROL: hand the SAME directory a holder that is not the toucher and
        // it must read foreign. Without this, "unverified" could be what this
        // code says about everything, and the test would prove nothing.
        let other = "aaaaaaaa-c9c9-4bbb-8ccc-aaaaaaaaaaaa";
        assert_eq!(wake::watch_cell_for("t132c9", Some(other)), "✗ foreign");
        // And with the toucher AS the holder it must read watching.
        assert_eq!(wake::watch_cell_for("t132c9", Some(toucher)), "✓");
    });
}

/// C8's population, as a test rather than a paragraph: a live session whose
/// heartbeat looks dead and whose monitor is old-shape must keep its title.
#[test]
fn c8_a_live_old_shape_session_does_not_lose_its_title() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    with_thread_home(root, || {
        let holder = "bbbbbbbb-c8c8-4bbb-8ccc-bbbbbbbbbbbb";
        session_registry::register("t132c8", holder, root, Some("base")).unwrap();
        // Its monitor is looping. It writes no sibling, because on 2026-09-10
        // there were 151 bare sentinels on this machine and zero siblings.
        bare_touch(&inbox(root, "t132c8").join(".watching"));

        assert!(
            wake::is_watching("t132c8"),
            "pick_name consults this before reusing a codename; a running loop \
             must keep the title held even when the heartbeat looks stale"
        );
        assert!(
            !wake::watching_by_holder("t132c8"),
            "and the board must still report honestly that nobody is identified"
        );
        assert_eq!(wake::watch_cell("t132c8"), "✗ unidentified");
    });
}

// ─── The cleanup (auk's ruling 2) ────────────────────────────
//
// Siblings accumulate one per session that ever watched a title, so `relay
// register` prunes abandoned ones. Every leg below pairs with the one above it:
// P1 proves the cleanup can delete at all, and without it P2 to P5 would pass
// identically on a cleanup that did nothing whatsoever.

/// The cutoff is 24h inside `wake.rs`; this is comfortably past it. The constant
/// is private, so the test names the age it builds rather than importing it.
const PAST_CUTOFF_SECS: u64 = 25 * 3600;

/// P1 POSITIVE CONTROL — an abandoned sibling really is removed.
#[test]
fn p1_control_an_abandoned_sibling_is_pruned() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    with_thread_home(root, || {
        let holder = "77777777-aaaa-4bbb-8ccc-777777777777";
        let gone = "99999999-gone-4fff-8fff-999999999999";
        session_registry::register("t132p1", holder, root, Some("base")).unwrap();
        let p = inbox(root, "t132p1").join(format!(".watching-by-{gone}"));
        bare_touch(&p);
        age(&p, PAST_CUTOFF_SECS);

        assert_eq!(wake::prune_watch_siblings("t132p1"), 1, "one file was eligible");
        assert!(!p.exists(), "an abandoned sibling must be removed");
    });
}

/// P2 — a LIVE session's sibling survives a prune running concurrently with its
/// loop. This is the leg auk asked for: the thread below is doing exactly what
/// an armed monitor does while `register` runs beside it.
#[test]
fn p2_a_live_sessions_sibling_survives_a_concurrent_prune() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let holder = "88888888-aaaa-4bbb-8ccc-888888888888";
    let live = "abababab-live-4aaa-8aaa-abababababab";
    let p = inbox(root, "t132p2").join(format!(".watching-by-{live}"));
    bare_touch(&p);

    // The live monitor's loop. It needs only the path, so it does not need the
    // thread-local home override that the prune call below runs under.
    let stop = Arc::new(AtomicBool::new(false));
    let (tp, ts) = (p.clone(), stop.clone());
    let loop_thread = std::thread::spawn(move || {
        let mut writes = 0u32;
        while !ts.load(Ordering::Relaxed) {
            let _ = std::fs::write(&tp, b"live\n");
            writes += 1;
            std::thread::sleep(Duration::from_millis(20));
        }
        writes
    });
    std::thread::sleep(Duration::from_millis(60));

    let removed = with_thread_home(root, || {
        session_registry::register("t132p2", holder, root, Some("base")).unwrap();
        wake::prune_watch_siblings("t132p2")
    });

    stop.store(true, Ordering::Relaxed);
    let writes = loop_thread.join().unwrap();
    assert!(writes > 0, "the loop must actually have written, or this proves nothing");
    assert_eq!(removed, 0, "nothing was abandoned, so nothing may be removed");
    assert!(p.exists(), "a live session's sibling must survive");
    let age_now = std::fs::metadata(&p).unwrap().modified().unwrap().elapsed().unwrap();
    assert!(
        age_now < Duration::from_secs(wake::WATCH_STALE_SECS),
        "and it must still read fresh afterwards; age was {age_now:?}"
    );
}

/// P3 — the title's own holder is never pruned, even at an impossible age.
#[test]
fn p3_the_titles_own_holder_is_never_pruned() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    with_thread_home(root, || {
        let holder = "cdcdcdcd-aaaa-4bbb-8ccc-cdcdcdcdcdcd";
        session_registry::register("t132p3", holder, root, Some("base")).unwrap();
        let p = inbox(root, "t132p3").join(format!(".watching-by-{holder}"));
        bare_touch(&p);
        age(&p, PAST_CUTOFF_SECS);

        assert_eq!(wake::prune_watch_siblings("t132p3"), 0);
        assert!(p.exists(), "the registered holder keeps its identity");
    });
}

/// P4 — a session that moved to a different title keeps its identity here.
#[test]
fn p4_a_session_holding_another_title_is_never_pruned() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    with_thread_home(root, || {
        let holder = "efefefef-aaaa-4bbb-8ccc-efefefefefef";
        let moved = "12121212-move-4bbb-8ccc-121212121212";
        session_registry::register("t132p4", holder, root, Some("base")).unwrap();
        session_registry::register("t132p4-elsewhere", moved, root, Some("base")).unwrap();
        let p = inbox(root, "t132p4").join(format!(".watching-by-{moved}"));
        bare_touch(&p);
        age(&p, PAST_CUTOFF_SECS);

        assert_eq!(wake::prune_watch_siblings("t132p4"), 0);
        assert!(p.exists(), "a session that holds some title keeps its siblings");
    });
}

/// P5 — the cleanup touches NOTHING else in the inbox. Pending pings live in
/// this same directory, so a match one character wider than
/// `.watching-by-<id>` would delete undelivered messages. This is the leg that
/// would have bitten someone.
#[test]
fn p5_the_cleanup_never_touches_anything_but_its_own_siblings() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    with_thread_home(root, || {
        let holder = "34343434-aaaa-4bbb-8ccc-343434343434";
        session_registry::register("t132p5", holder, root, Some("base")).unwrap();
        let dir = inbox(root, "t132p5");

        let bystanders = [
            ".watching",      // the sentinel itself — an outside reader depends on it
            ".status",        // the operator's live work line
            ".watch-nudge",   // the re-arm throttle
            "ping-abc.json",  // an UNDELIVERED message
            "notify-1.json",  // ditto
        ];
        for name in bystanders {
            let p = dir.join(name);
            bare_touch(&p);
            age(&p, PAST_CUTOFF_SECS);
        }
        // One genuinely abandoned sibling, so the prune is not a no-op run: if
        // it removed nothing at all, the survivals below would prove nothing.
        let doomed = dir.join(".watching-by-56565656-gone-4bbb-8ccc-565656565656");
        bare_touch(&doomed);
        age(&doomed, PAST_CUTOFF_SECS);

        assert_eq!(wake::prune_watch_siblings("t132p5"), 1, "exactly the one sibling");
        assert!(!doomed.exists(), "the abandoned sibling is gone");

        let mut checked = 0;
        for name in bystanders {
            assert!(dir.join(name).exists(), "{name} must survive the cleanup");
            checked += 1;
        }
        assert_eq!(checked, bystanders.len(), "every bystander was checked");
        assert!(checked > 0, "a loop that checked nothing proves nothing");
    });
}

/// Must-fail canary for the pair above: a sentinel older than the threshold
/// must read stale. If this ever read as watching, the freshness comparison
/// itself would be broken and every other leg here would be measuring nothing.
#[test]
fn canary_a_sentinel_past_the_threshold_reads_stale() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    with_thread_home(root, || {
        let holder = "44444444-aaaa-4bbb-8ccc-444444444444";
        session_registry::register("t132stale", holder, root, Some("base")).unwrap();
        let p = inbox(root, "t132stale").join(".watching");
        bare_touch(&p);
        age(&p, wake::WATCH_STALE_SECS + 45);

        assert!(!wake::watching_by_holder("t132stale"));
        assert!(
            !wake::is_watching("t132stale"),
            "past the threshold nothing is running, so the title is free"
        );
        let cell = wake::watch_cell("t132stale");
        assert!(cell.starts_with("✗ stale"), "cell was {cell:?}");
    });
}
