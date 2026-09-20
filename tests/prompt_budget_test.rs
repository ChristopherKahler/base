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
//!
//! THE BUDGET IS IN BYTES, AND IT WAS IN UTF-16 UNITS UNTIL 2026-09-20. The host was measured
//! counting bytes, with a probe built so the candidate units predict opposite outcomes: 13,000
//! box-drawing characters — 39,000 bytes but only 13,000 UTF-16 units — PERSISTED, where a UTF-16
//! limit predicts it arrives whole. A UTF-16 budget cannot bound a byte limit: UTF-8 allows 3
//! bytes per unit, so 4,000 units permitted up to 12,000 bytes against a threshold at or below
//! 10,957. base would have trimmed correctly, reported its withheld figure truthfully, and the
//! host would have cut the survivor anyway — a loss notice that is itself lost with every number
//! in it accurate. **So this file asserts on `str::len()`, never on `units()`.**

mod seed;

use seed::run_prompt_submit;

/// `[budget] prompt_chars`'s built-in default, which is what an operator with no `[budget]` section
/// gets. Spelled here rather than imported so the test states the number it is asserting.
///
/// **BYTES.** The name predates the measurement that settled the unit; see the file header.
const PROMPT_BYTES: usize = 4000;

/// The opening of the withheld notice. Both arms key off it, so it is spelled once.
const NOTICE: &str = "[base: user-prompt-submit withheld ";

/// Bytes, because that is what the host counts and therefore what the budget must bound.
fn emitted_bytes(s: &str) -> usize {
    s.len()
}

/// The LARGEST emission measured from this hook on a real machine: `finch`, 2026-09-20, prompt 10
/// of a single session, 26,016 bytes, of which 2,048 arrived. Prompt 1 of that same session emitted
/// 13,678 — SO THE EMISSION NEARLY DOUBLED AS THE SESSION GREW, which is why the fixture is keyed
/// to the observed maximum and not to a typical figure. A trim validated at 13 KB passes and still
/// loses everything at 26 KB.
///
/// **It was measured in BYTES**, which is now the unit the budget uses too — so this floor and the
/// budget are finally the same kind of number. They were not before 2026-09-20.
const HIGH_WATER: usize = 26_016;

/// The withheld figure the notice states, which is the only number in `stdout` that reports on text
/// that is NOT in `stdout`.
fn withheld_bytes(stdout: &str) -> Option<usize> {
    let at = stdout.find(NOTICE)?;
    stdout[at + NOTICE.len()..]
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

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
fn loud(tag: &str, rules: usize) -> seed::Seed {
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
    for i in 0..rules {
        toml.push_str(&format!(
            "  \"Global rule {i}: this rule is deliberately long enough to matter against a four \
             thousand unit budget, because a budget test whose fixture fits inside the budget \
             proves nothing at all about what happens when it does not. It is repeated many times \
             over so the fixture reproduces the LARGEST emission anyone has measured on a real \
             machine rather than the smallest one that happens to overflow, because a trim \
             validated at the small figure is not evidence about the large one.\",\n"
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
    let s = loud("budget", 70);
    let (code, stdout, stderr) =
        run_prompt_submit(&s, "write code to fix a bug in src/", Some("prompt-budget"));
    assert_eq!(code, 0, "the hook should not fail:\nstdout:\n{stdout}\nstderr:\n{stderr}");

    let emitted = emitted_bytes(&stdout);

    // THE CONTROL, AND IT IS KEYED ON WHAT THE HOOK WANTED TO SAY, NOT ON WHAT IT SAID.
    //
    // The first version of this control read `emitted > 1000`, which was right for the tree it was
    // written against and is WRONG FOR THE FIXED ONE. Once the writer caps the output, `emitted` is
    // bounded by `PROMPT_BYTES` by construction and can never again be evidence that anything was
    // driven through the trim: a fixture emitting 1,500 units with no overflow at all would satisfy
    // both `> 1000` and `<= 4000` and the file would go green having tested nothing. That is the
    // same inert guard this test already caught once, rebuilt by the fix that made it pass.
    //
    // `withheld` is the only figure in `stdout` that reports on text NOT in `stdout`, so it is the
    // only one that can prove a trim happened. `wanted == withheld + kept` and `kept >= 0`, so
    // `withheld >= HIGH_WATER` implies `wanted >= HIGH_WATER`: the fixture really did reproduce the
    // largest emission measured on a real machine.
    let withheld = withheld_bytes(&stdout).unwrap_or_else(|| {
        panic!(
            "control: no withheld notice, so NOTHING WAS TRIMMED and the budget assertion below \
             would be vacuous. The hook emitted {emitted} bytes against a {PROMPT_BYTES} byte \
             budget. TWO CAUSES LOOK IDENTICAL FROM HERE and the figure tells them apart: at or \
             under {PROMPT_BYTES} the fixture is too quiet to overflow and needs more rules; well \
             over it, nothing is measuring the output at all, which is the defect itself. \
             stdout:\n{stdout}"
        )
    });
    assert!(
        withheld >= HIGH_WATER,
        "control: the trim withheld {withheld} bytes, short of the {HIGH_WATER} measured on a real \
         machine, so this fixture is quieter than the failure it stands for. A budget validated \
         against a small overflow is not evidence about a large one — finch watched this hook go \
         from 13,678 to 26,016 bytes inside ONE session. Raise the fixture's rule count."
    );

    assert!(
        emitted <= PROMPT_BYTES,
        "the prompt hook emitted {emitted} BYTES against [budget] prompt_chars = \
         {PROMPT_BYTES}. Nothing measures it, so whatever the host drops is lost silently — which \
         is the defect. First 400 chars:\n{}",
        stdout.chars().take(400).collect::<String>()
    );

    // THE NOTICE MUST NAME THE KEY THAT ACTUALLY GOVERNED THE TRIM. `withheld_notice` used to
    // hard-code `prompt_chars` while taking the hook name as a parameter, so it was correct only
    // while exactly one hook called it. An overflow notice that names the wrong setting is worse
    // than no notice: it sends the operator to edit a key that governs a different hook, with every
    // appearance of having been told what to do.
    //
    // AND THIS ASSERTION USED TO HARD-CODE `prompt_chars` ITSELF. The paragraph above states the
    // rule correctly and the line below it pinned the old spelling, so the test went green over the
    // defect for its whole life and could only ever have gone RED if somebody fixed the code. A
    // test written to stop a hard-coded key, holding a hard-coded key.
    //
    // `prompt_chars` was renamed to `prompt_bytes` on 2026-09-20 when its unit changed. The notice
    // told every operator to raise the LEGACY key - which `BaseConfig::load` then warns them to
    // rename. Base directed the operator into the exact state it scolds them for, as advice, on
    // every over-budget prompt.
    let key = format!("[budget] prompt_bytes = {PROMPT_BYTES}");
    assert!(
        stdout.contains(&key),
        "the withheld notice does not name `{key}`, so it cannot tell the operator which setting \
         caused the trim. Notice as emitted:\n{}",
        stdout.rsplit_once(NOTICE).map(|(_, tail)| tail).unwrap_or("<none>")
    );

    // AND IT NAMES NO RETIRED SPELLING, read off the table the production code reads rather than a
    // list typed here. A list typed here is correct today and blind at the next rename, which is
    // the same failure as the line above.
    for k in base::config::RENAMED_BUDGET_KEYS {
        assert!(
            !stdout.contains(&format!("[budget] {} =", k.old)),
            "the notice names the retired spelling `{}`, sending the operator to set a key base \
             will then warn them to rename",
            k.old
        );
    }
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
    let s = loud("notice", 70);
    let (code, stdout, _) =
        run_prompt_submit(&s, "write code to fix a bug in src/", Some("prompt-budget"));
    assert_eq!(code, 0);

    let Some(at) = stdout.find(NOTICE) else {
        // No notice is only acceptable if nothing was withheld. Over budget with no notice is the
        // silent loss this change exists to end.
        assert!(
            emitted_bytes(&stdout) <= PROMPT_BYTES,
            "the output is {} bytes, over the {PROMPT_BYTES} budget, and carries NO withheld \
             notice: the loss is silent, which is the defect itself",
            emitted_bytes(&stdout)
        );
        return;
    };

    // `str::find` returns a BYTE offset, which is now the unit the budget is in — so the position
    // of the notice needs no conversion at all. Under the old UTF-16 budget this line had to
    // re-measure the prefix, and a byte offset compared against a UTF-16 budget would have read as
    // correct while comparing two different quantities.
    let notice_ends = at + NOTICE.len();
    assert!(
        notice_ends <= PROMPT_BYTES,
        "the withheld notice ends at byte {notice_ends}, past the {PROMPT_BYTES} the host \
         delivers. The report of the loss died inside the loss — the exact defect this fixes."
    );
}
