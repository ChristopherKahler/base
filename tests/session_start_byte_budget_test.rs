//! Rank 00, the session-start half of the unit defect: **the budget is enforced in BYTES.**
//!
//! WHY. `emit::Emission` and `emit::Rendered` measured in UTF-16 code units against a host
//! threshold that was measured, on 2026-09-20, to count BYTES. The probe that settled it was built
//! so the candidate units predict OPPOSITE outcomes: 13,000 box-drawing characters — 39,000 bytes
//! but only 13,000 UTF-16 units — PERSISTED, where a UTF-16 limit predicts it arrives whole.
//!
//! An all-ASCII fixture cannot test this, because for ASCII bytes, `char`s and UTF-16 units are the
//! same number. **The first control below exists to refuse to pass on such a fixture.**
//!
//! Measured on the operator's real session start, 2026-09-20: 6,879 bytes against 6,558 UTF-16
//! units — a 321-byte excess, about 5%. So a UTF-16 budget of N lets roughly 1.05 × N bytes out,
//! and `session_start_chars = 9000` permitted up to 27,000 bytes in the worst case.

mod seed;

use seed::{run_session_start, units};

/// Small enough that the trim certainly engages on the real-size seed, large enough that the
/// multi-byte excess is tens of bytes rather than one or two.
const BUDGET: usize = 4000;

/// The key as it is spelled AFTER this change.
const KEY: &str = "session_start_bytes";

/// The pre-2026-09-20 spelling. It must keep working: an operator who tuned it must not be
/// silently dropped back to the default, which is the defect this change exists to remove rather
/// than to commit in a new place.
const LEGACY_KEY: &str = "session_start_chars";

fn seeded(tag: &str, global: &str) -> seed::Seed {
    let root = std::env::temp_dir().join(format!("base-ss-bytes-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        !root.exists(),
        "the seed root {} survived its clean, so this test would measure another run's tree",
        root.display()
    );
    seed::write(&root, &seed::REAL, global)
}

#[test]
fn session_start_fits_its_budget_in_bytes() {
    let s = seeded("budget", &format!("[budget]\n{KEY} = {BUDGET}\n"));
    let (code, stdout, stderr) = run_session_start(&s, Some("seed-session"));
    assert_eq!(code, 0, "hooks fail open. stderr: {stderr}");

    let bytes = stdout.len();
    let u16s = units(&stdout);

    // CONTROL 1 — THE FIXTURE MUST BE ABLE TO TELL THE UNITS APART.
    // For pure ASCII, bytes == UTF-16 units, and every assertion below would hold for the wrong
    // reason. This is the same inert-guard shape this round has caught twice: a test that passes
    // because nothing was driven through it. If session start ever stops emitting box-drawing and
    // dashes, this fails loudly instead of going quietly green.
    assert!(
        bytes > u16s,
        "control: the output is {bytes} bytes and {u16s} UTF-16 units — IDENTICAL, so this fixture \
         contains no multi-byte characters and CANNOT distinguish a byte budget from a UTF-16 one. \
         A pass here would prove nothing."
    );

    // CONTROL 2 — THE TRIM MUST HAVE ENGAGED.
    // An emission that fits the budget whole never exercises the cap, so the assertion below would
    // be vacuous.
    assert!(
        stdout.contains("full: ") || stdout.contains("·"),
        "control: no session-start layout markers in the output, so the hook did not render its \
         normal block set and this measured nothing. stdout:\n{stdout}"
    );

    // THE ASSERTION. Red before this change: the cap is applied to `u16_len`, so a ~5% multi-byte
    // output sits under the budget in UTF-16 and OVER it in bytes — which is the unit the host
    // actually counts.
    assert!(
        bytes <= BUDGET,
        "session start emitted {bytes} BYTES against [budget] {KEY} = {BUDGET}. It is under budget \
         in UTF-16 ({u16s}) and over it in bytes, which is the unit the host counts — so base \
         believes it fitted and the host truncates it anyway."
    );
}

#[test]
fn the_legacy_key_still_sets_the_budget_and_says_so_exactly_once() {
    let s = seeded("legacy", &format!("[budget]\n{LEGACY_KEY} = {BUDGET}\n"));
    let (code, stdout, stderr) = run_session_start(&s, Some("seed-session"));
    assert_eq!(code, 0, "hooks fail open. stderr: {stderr}");

    // THE VALUE MUST STILL BE HONOURED. If the rename merely retires the old key, an operator who
    // set it is silently returned to the default — a setting that stops working without saying so,
    // which is the exact defect class this whole round is about.
    assert!(
        stdout.len() <= BUDGET,
        "the legacy key [budget] {LEGACY_KEY} = {BUDGET} did not set the budget: session start \
         emitted {} bytes. An operator who tuned it has been silently dropped to the default.",
        stdout.len()
    );

    // AND IT MUST SAY SO, ONCE. Silence would leave the operator on a deprecated key forever;
    // repeating it every run would be noise they learn to skip.
    let mentions = stderr.matches(LEGACY_KEY).count();
    assert_eq!(
        mentions, 1,
        "the legacy key notice appeared {mentions} times on stderr, expected exactly 1. It must \
         name the old key and its replacement once per run. stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(KEY),
        "the legacy notice does not name the replacement `{KEY}`, so it tells the operator their \
         key is old without telling them what to write instead. stderr:\n{stderr}"
    );
}
