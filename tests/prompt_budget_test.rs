//! Rank 00, widened: the prompt hook's output is MEASURED against `[budget] prompt_chars`.
//!
//! WHY THIS FILE EXISTS. `finch` measured `base hook user-prompt-submit` emitting 13.3 KB on
//! prompt 1 with only 2 KB arriving — the same arrival figure as the 0.15.2 baseline, so the loss
//! did not shrink, it MOVED HOOKS. Lost inline: all fifteen GLOBAL domain rules including rule 3,
//! the bracket rules past 2, and the whole relay wake contract.
//!
//! THE CAUSE WAS NOT A MISSING DEFAULT. `BudgetConfig::prompt_chars` has always existed and always
//! defaulted to 4000. It had ZERO CONSUMERS — as do `pre_tool_chars` and `post_tool_chars` — while
//! `session_start_chars` had five. `Emission::new` appeared in exactly one hook. The measured
//! emission was only ever wired to one of four, and three declared budget fields were
//! documentation rather than behaviour. A field an operator can set that changes nothing is worse
//! than an absent one: they have evidence they configured it.
//!
//! WHY A TEST AND NOT A MEASUREMENT. 13.3-KB-in-2-KB-out is one machine at one moment, and a
//! reading that cannot be re-run is not a guard.

mod seed;

use seed::{run_prompt_submit, units};

/// `[budget] prompt_chars`'s built-in default, which is what an operator with no `[budget]` section
/// gets. Spelled here rather than imported so the test states the number it is asserting.
const PROMPT_CHARS: usize = 4000;

/// A fixture LOUD ENOUGH TO OVERFLOW, which the bare seed is not.
///
/// THE FIRST VERSION OF THIS TEST PASSED ON THE UNFIXED TREE, AND THAT WAS A VOID READING, NOT A
/// GREEN ONE. The bare `seed::REAL` fixture emits 55 UTF-16 units from this hook — just the context
/// bracket — so `55 <= 4000` passed trivially and this would have shipped as an inert guard
/// claiming to cover rank 00. Measured, not guessed: a throwaway probe printed `emitted_units=55`.
///
/// `finch`'s 13.3 KB came from the operator's real machine, where the output is dominated by things
/// a bare seed has none of: fifteen always-on domain rules, the bracket rules, and the relay wake
/// contract. The fixture supplies all three. A budget test whose fixture cannot exceed the budget
/// is not a budget test.
/// `tag` IS NOT DECORATION AND IT IS NOT OPTIONAL. The first version of this fixture keyed its root
/// on the process id alone, so both tests in this file built at the SAME path and cargo ran them in
/// parallel: each one's `remove_dir_all` deleted the tree the other was using. The budget arm read
/// 6146 units and the notice arm read a cleaned tree and passed vacuously — a green that meant
/// nothing, from a red run.
///
/// That is the same defect this round already measured in `migrate_test`, where a pair sharing one
/// marker root read GREEN in a full-suite run and RED in a targeted run on the same commit. A test
/// whose result depends on what another test is doing was never evidence in either direction.
fn loud(tag: &str) -> seed::Seed {
    let root = std::env::temp_dir()
        .join(format!("base-prompt-budget-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        !root.exists(),
        "the seed root {} survived its clean, so this test would measure another run's tree",
        root.display()
    );

    let global = "[bracket]\nenabled = true\n";
    let s = seed::write(&root, &seed::REAL, global);

    // An always-on domain carrying fifteen long rules: the shape that dominates the real machine's
    // output, and the population `finch` measured as lost inline.
    let ws_base = s.ws.join(".base");
    std::fs::create_dir_all(&ws_base).expect("workspace .base");
    let mut toml = String::from("[[domain]]\nname = \"global\"\nmode = \"always\"\nrules = [\n");
    for i in 0..15 {
        toml.push_str(&format!(
            "  \"Global rule {i}: this rule is deliberately long enough to matter against a four \
             thousand unit budget, because a budget test whose fixture fits inside the budget \
             proves nothing at all about what happens when it does not.\",\n"
        ));
    }
    toml.push_str("]\n");
    std::fs::write(ws_base.join("domains.toml"), toml).expect("domains.toml");
    s
}

/// The hook's stdout fits `[budget] prompt_chars`.
///
/// RED on the unfixed tree: every `print!` in `user_prompt_submit::handle` writes straight to
/// stdout with nothing measuring it, so the output is whatever the graph happens to produce.
#[test]
fn the_prompt_hook_fits_its_budget() {
    let s = loud("budget");
    let (code, stdout, stderr) =
        run_prompt_submit(&s, "write code to fix a bug in src/", Some("prompt-budget"));
    assert_eq!(code, 0, "the hook should not fail:\nstdout:\n{stdout}\nstderr:\n{stderr}");

    let emitted = units(&stdout);
    // THE CONTROL. A hook that emitted nothing, or emitted only the context bracket, makes the
    // budget assertion vacuous — which is exactly how the first version of this test passed on a
    // tree with no budget at all.
    assert!(
        emitted > 1000,
        "control: the hook emitted only {emitted} units, so it never approached the {PROMPT_CHARS} \
         unit budget and a passing assertion below would prove nothing. The fixture is not loud \
         enough. stdout:\n{stdout}"
    );

    assert!(
        emitted <= PROMPT_CHARS,
        "the prompt hook emitted {emitted} UTF-16 units against [budget] prompt_chars = \
         {PROMPT_CHARS}. Nothing measures it, so whatever the host drops is lost silently — which \
         is the defect. First 400 units:\n{}",
        stdout.chars().take(400).collect::<String>()
    );
}

/// THE ARM THAT MATTERS MOST: when the budget withholds anything, the notice saying so lands
/// INSIDE the surviving region.
///
/// This is rank 00's exact failure mode and it must not be rebuilt inside the fix for it. base
/// already detected its session-start loss correctly and wrote the report at byte 23,815 of
/// 26,310 — eleven thousand bytes inside the region the host never delivers. The report of the
/// loss was destroyed by the loss it reported. A writer that trims at the boundary and THEN
/// appends its notice reproduces that precisely, and it would look like a fix.
#[test]
fn the_withheld_notice_survives_the_trim_that_caused_it() {
    let s = loud("notice");
    let (code, stdout, _) =
        run_prompt_submit(&s, "write code to fix a bug in src/", Some("prompt-budget"));
    assert_eq!(code, 0);

    const NEEDLE: &str = "[base: user-prompt-submit withheld ";
    let Some(at) = stdout.find(NEEDLE) else {
        // No notice is only acceptable if nothing was withheld. Over budget with no notice is the
        // silent loss this change exists to end.
        assert!(
            units(&stdout) <= PROMPT_CHARS,
            "the output is {} units, over the {PROMPT_CHARS} budget, and carries NO withheld \
             notice: the loss is silent, which is the defect itself",
            units(&stdout)
        );
        return;
    };

    let notice_ends = units(&stdout[..at]) + units(NEEDLE);
    assert!(
        notice_ends <= PROMPT_CHARS,
        "the withheld notice ends at UTF-16 unit {notice_ends}, past the {PROMPT_CHARS} the host \
         delivers. The report of the loss died inside the loss — the exact defect this fixes."
    );
}
