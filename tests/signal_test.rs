use base::config::{BaseConfig, NamespaceConfig};
use base::crud;
use base::emit::{Level, Reason};
use base::hook::session_start::SessionOutput;
use base::signal;

fn test_config() -> BaseConfig {
    BaseConfig::default()
}

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

/// Helper: populate a workspace with test entities at various timestamps.
fn seed_workspace(dir: &std::path::Path) {
    let ns = ns();

    // Active project (recent)
    crud::project::add(dir, &ns, "Active Project", "active", None).unwrap();

    // Blocked project
    crud::project::add(dir, &ns, "Blocked Project", "blocked", None).unwrap();
    crud::project::update(dir, &ns, "blocked-project", Some("blocked"), Some("waiting on API"), None).unwrap();

    // Active task
    crud::task::add(dir, &ns, "active-project", "Fix Auth", Some("high"), None).unwrap();

    // Deferred project — excluded from active-awareness (protocol's call, not a time window).
    crud::project::add(dir, &ns, "Deferred Project", "deferred", None).unwrap();

    // Decision
    crud::decision::log(dir, &ns, "dev", "Use JWT", "Stateless", None).unwrap();
}

#[test]
fn active_awareness_surfaces_recent_entities() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    seed_workspace(tmp.path());

    let config = test_config();
    let output = signal::active_awareness::run(tmp.path(), &config).unwrap();

    assert!(output.contains("Active Project"), "Should include active project");
    assert!(output.contains("Blocked Project"), "Should include blocked project");
    assert!(output.contains("Fix Auth"), "Should include active task");
    // Deferred project must NOT appear — protocol's deferral is the gate, not a time window.
    assert!(!output.contains("Deferred Project"), "Deferred project should not appear in active-awareness");
}

#[test]
fn pulse_shows_counts() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    seed_workspace(tmp.path());

    let config = test_config();
    let output = signal::pulse::run(tmp.path(), &config.namespace, &config.signal).unwrap();

    assert!(output.contains("Pulse"), "Should have Pulse header");
    assert!(output.contains("active"), "Should mention active count");
    assert!(output.contains("blocked"), "Should mention blocked count");
}

#[test]
fn suppression_skips_unchanged_signals() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    seed_workspace(tmp.path());

    let config = test_config();

    // First run — should produce output
    let output1 = signal::run_signals(tmp.path(), &config, "test").unwrap();
    assert!(!output1.is_empty(), "First run should produce output");
    // Rank 00 commit B: run_signals no longer records what it returns. Session start records a
    // signal only after it rendered in full; this stands in for a start where everything fit.
    output1.record_shown(|_| true);

    // Second run — nothing changed, should be suppressed
    let output2 = signal::run_signals(tmp.path(), &config, "test").unwrap();
    assert!(output2.is_empty(), "Second run should be suppressed (no changes)");
    assert!(
        !output2.unchanged().is_empty(),
        "a suppressed signal is reported as unchanged, never dropped without a trace"
    );
}

#[test]
fn suppression_re_emits_on_change() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    seed_workspace(tmp.path());

    let config = test_config();

    // First run, recorded as shown in full the way session start records it, so the re-emit
    // below comes from the change and not from a record that was never written (commit B).
    signal::run_signals(tmp.path(), &config, "test")
        .unwrap()
        .record_shown(|_| true);

    // Change data — add a new project
    crud::project::add(tmp.path(), &ns(), "New Project", "active", None).unwrap();

    // Third run — data changed, should re-emit
    let output3 = signal::run_signals(tmp.path(), &config, "test").unwrap();
    assert!(!output3.is_empty(), "Should re-emit after data change");
}

#[test]
fn a_signal_collapsed_by_the_budget_is_shown_again_next_session() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    seed_workspace(tmp.path());
    let config = test_config();

    // Session one: every block rendered in full except the task list, which the budget collapsed.
    signal::run_signals(tmp.path(), &config, "test")
        .unwrap()
        .record_shown(|kind| kind != "tasks");

    let output = signal::run_signals(tmp.path(), &config, "test").unwrap();
    let shown: Vec<&str> = output.signals().iter().map(|s| s.name).collect();
    let skipped: Vec<&str> = output.unchanged().iter().map(|s| s.name).collect();
    assert!(
        shown.contains(&"active-awareness"),
        "a signal that did not render in full is shown again: shown {shown:?}"
    );
    assert!(
        skipped.contains(&"pulse"),
        "control: a signal that rendered in full is skipped: skipped {skipped:?}"
    );
}

#[test]
fn an_over_budget_signal_collapses_to_a_floor_with_a_ledger_row() {
    // Replaces `budget_cap_truncates` (rank 00 commit B). That test pinned the `[signal]
    // max_chars` branch inside run_signals, which dropped whole signals past the cap, exempted
    // the four largest and reported the drop in one line at the tail. The branch is gone:
    // nothing is dropped inside run_signals, and session start trims to `[budget]
    // session_start_chars` instead. What the old test was for still holds, and is asserted
    // here: a small budget never loses a block without a trace.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    seed_workspace(tmp.path());

    let mut config = test_config();
    config.signal.max_chars = 50; // the legacy key: read by nothing now
    config.budget.session_start_bytes = 60;
    config.budget.write_full_output = false;

    let output = signal::run_signals(tmp.path(), &config, "test").unwrap();
    let names: Vec<&str> = output.signals().iter().map(|s| s.name).collect();
    assert!(
        names.contains(&"active-awareness") && names.contains(&"pulse"),
        "nothing is dropped for size before the budget: {names:?}"
    );

    let mut out = SessionOutput::new();
    out.push_signals(output);
    let rendered = out.finish(&config, tmp.path());
    assert!(!rendered.text.is_empty(), "the output is not empty");
    let collapsed: Vec<&str> = rendered
        .blocks
        .iter()
        .filter(|b| b.level() == Level::Collapsed)
        .map(|b| b.id())
        .collect();
    assert!(
        !collapsed.is_empty(),
        "a 60-unit budget collapses something: {}",
        rendered.text
    );
    for id in &collapsed {
        assert!(
            rendered
                .withheld
                .iter()
                .any(|w| w.block == *id && w.reason == Reason::Collapsed),
            "{id} collapsed without a ledger row"
        );
        assert!(
            rendered.text.lines().any(|l| l.starts_with(&format!("{id} "))),
            "{id} collapsed without its floor line: {}",
            rendered.text
        );
    }
}

#[test]
fn a_signal_skipped_as_unchanged_leaves_a_ledger_row_per_block() {
    // T7, the hash skip (rank 00 commit B). A signal skipped because its output has not changed
    // prints nothing, as before, and now leaves a ledger row for every block it would have printed.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    seed_workspace(tmp.path());
    let mut config = test_config();
    config.budget.write_full_output = false;

    let mut first = SessionOutput::new();
    first.push_signals(signal::run_signals(tmp.path(), &config, "test").unwrap());
    // Everything fits the default budget, so every signal is recorded as shown in full.
    let _ = first.finish(&config, tmp.path());

    let again = signal::run_signals(tmp.path(), &config, "test").unwrap();
    let skipped: Vec<(&'static str, usize)> = again
        .unchanged()
        .iter()
        .flat_map(|s| s.blocks.iter().map(|b| (b.kind, b.items)))
        .collect();
    assert!(
        !skipped.is_empty(),
        "nothing was skipped as unchanged, so this measured nothing"
    );

    let mut second = SessionOutput::new();
    second.push_signals(again);
    let rendered = second.finish(&config, tmp.path());
    for (kind, items) in &skipped {
        assert!(
            rendered
                .withheld
                .iter()
                .any(|w| w.block == *kind && w.items == *items && w.reason == Reason::HashUnchanged),
            "{kind}: skipped as unchanged without a ledger row. ledger: {:?}",
            rendered.withheld
        );
    }
}

#[test]
fn disabled_signals_emit_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".base")).unwrap();
    seed_workspace(tmp.path());

    let mut config = test_config();
    config.signal.enabled = false;

    let output = signal::run_signals(tmp.path(), &config, "test").unwrap();
    assert!(output.is_empty(), "Disabled signals should emit nothing");
    assert!(output.diagnostics.is_empty(), "Disabled signals should emit no diagnostics");
}


// --------------------------------------------------------------------------
// Cross-lane item 2: the working set reads BOTH tiers.
//
// Every arm below gets its OWN fake home through `with_thread_home`, because
// `load_merged` resolves the global tier through `home::home_root()`. The
// process-wide test root is shared by every thread in this binary, and these
// arms write a global tier -- so sharing it would let one arm's global task
// appear in another's. A thread-local root cannot do that.
//
// Predictions registered before the run (B42 step 6):
//   I1 RED before the fix, GREEN after -- the defect's own test
//   I2 GREEN both sides -- the merge must not lose the workspace tier
//   I4 GREEN both sides -- control: the common path is untouched
//   I5 GREEN both sides -- pins behaviour that is currently right by accident
// --------------------------------------------------------------------------

/// Build a tier rooted at `dir` and return it, creating its `.base/` first.
fn tier(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::create_dir_all(dir.join(".base")).unwrap();
    dir.to_path_buf()
}

/// I1. A task recorded only in the global tier must appear in the working set.
/// Before the fix the query opened the workspace graph alone, so it could not match.
#[test]
fn a_global_tier_task_renders_in_the_working_set() {
    let home = tempfile::tempdir().unwrap();
    base::home::with_thread_home(home.path(), || {
        let gbl = tier(&home.path().join(".base-gbl"));
        crud::project::add(&gbl, &ns(), "Global Project", "active", None).unwrap();
        crud::task::add(&gbl, &ns(), "global-project", "Global Only Task", Some("high"), None).unwrap();

        let ws = tier(&home.path().join("ws"));
        crud::project::add(&ws, &ns(), "Workspace Project", "active", None).unwrap();

        let out = signal::active_awareness::run(&ws, &test_config()).unwrap();
        assert!(
            out.contains("Global Only Task"),
            "a global-tier task must render in the working set; got:\n{out}"
        );
    });
}

/// I2. Reading both tiers must not lose the workspace tier.
#[test]
fn a_workspace_task_still_renders_when_both_tiers_exist() {
    let home = tempfile::tempdir().unwrap();
    base::home::with_thread_home(home.path(), || {
        let gbl = tier(&home.path().join(".base-gbl"));
        crud::project::add(&gbl, &ns(), "Global Project", "active", None).unwrap();
        crud::task::add(&gbl, &ns(), "global-project", "Global Only Task", Some("high"), None).unwrap();

        let ws = tier(&home.path().join("ws"));
        crud::project::add(&ws, &ns(), "Workspace Project", "active", None).unwrap();
        crud::task::add(&ws, &ns(), "workspace-project", "Workspace Only Task", Some("high"), None).unwrap();

        let out = signal::active_awareness::run(&ws, &test_config()).unwrap();
        assert!(out.contains("Workspace Only Task"), "workspace tier lost; got:\n{out}");
        assert!(out.contains("Global Only Task"), "global tier lost; got:\n{out}");
    });
}

/// I4. Control. A workspace with no global graph beside it behaves exactly as before.
/// If this reddens, the change broke the ordinary path rather than widening it.
#[test]
fn a_workspace_only_task_is_unaffected() {
    let home = tempfile::tempdir().unwrap();
    base::home::with_thread_home(home.path(), || {
        // deliberately no .base-gbl at all
        let ws = tier(&home.path().join("ws"));
        crud::project::add(&ws, &ns(), "Workspace Project", "active", None).unwrap();
        crud::task::add(&ws, &ns(), "workspace-project", "Workspace Only Task", Some("high"), None).unwrap();

        let out = signal::active_awareness::run(&ws, &test_config()).unwrap();
        assert!(out.contains("Workspace Only Task"), "the common path broke; got:\n{out}");
    });
}

/// I5. One entity recorded in BOTH tiers renders once, not twice.
///
/// This already holds, and it holds by accident: `run_sections` keys a `HashMap` on
/// entity id for an entirely different reason -- gathering an entity's several owner
/// links into one row. Merging the tiers is the first thing that makes two rows share
/// an id, so the collapse now matters and nothing announces it if it breaks.
/// Accidental correctness is the fragile kind. Keep this arm whatever else changes.
#[test]
fn an_entity_recorded_in_both_tiers_renders_once() {
    let home = tempfile::tempdir().unwrap();
    base::home::with_thread_home(home.path(), || {
        let gbl = tier(&home.path().join(".base-gbl"));
        crud::project::add(&gbl, &ns(), "Shared Project", "active", None).unwrap();

        let ws = tier(&home.path().join("ws"));
        crud::project::add(&ws, &ns(), "Shared Project", "active", None).unwrap();

        let out = signal::active_awareness::run(&ws, &test_config()).unwrap();
        assert_eq!(
            out.matches("Shared Project").count(),
            1,
            "an entity in both tiers must render once, not once per tier; got:\n{out}"
        );
    });
}


// --------------------------------------------------------------------------
// The honesty envelope. Absent, empty and zero are three states; two of them
// used to produce the same screen.
//
// Predictions registered before the run:
//   A1 RED before, GREEN after -- absent renders nothing today
//   A2 RED before, GREEN after -- empty renders nothing today
//   A3 GREEN both for the rows, RED before for the scope clause
//   A4 RED before, GREEN after -- the DIFFERENTIAL arm, and the only one that
//      can catch this. A1-A3 can all pass while the defect sits untouched.
// --------------------------------------------------------------------------

/// A1. Neither tier has a graph. The block must say so, not render empty.
#[test]
fn no_graph_in_either_tier_says_so_and_never_renders_an_empty_working_set() {
    let home = tempfile::tempdir().unwrap();
    base::home::with_thread_home(home.path(), || {
        let ws = tier(&home.path().join("ws"));
        let out = signal::active_awareness::run(&ws, &test_config()).unwrap();
        assert!(
            out.contains("no graph was read"),
            "absent must say no graph was read; got:\n{out}"
        );
        assert!(
            out.contains("NOT an empty working set"),
            "absent must refuse the empty reading outright; got:\n{out}"
        );
    });
}

/// A2. A tier exists and holds no working rows. The empty rendering, with scope.
#[test]
fn an_empty_working_set_names_its_scope_so_it_cannot_read_as_absent() {
    let home = tempfile::tempdir().unwrap();
    base::home::with_thread_home(home.path(), || {
        let ws = tier(&home.path().join("ws"));
        // A graph that exists and holds nothing in a working state.
        crud::project::add(&ws, &ns(), "Done Project", "complete", None).unwrap();

        let out = signal::active_awareness::run(&ws, &test_config()).unwrap();
        assert!(out.contains("SCOPE:"), "empty must still name its scope; got:\n{out}");
        assert!(
            out.contains("other workspaces were not read"),
            "the scope clause must name the axis it does NOT cover; got:\n{out}"
        );
        assert!(
            !out.contains("no graph was read"),
            "a graph WAS read; this must not claim otherwise; got:\n{out}"
        );
    });
}

/// A3. The ordinary listing still lists, and now carries its scope too.
#[test]
fn a_populated_working_set_still_lists_and_names_its_scope() {
    let home = tempfile::tempdir().unwrap();
    base::home::with_thread_home(home.path(), || {
        let ws = tier(&home.path().join("ws"));
        crud::project::add(&ws, &ns(), "Live Project", "active", None).unwrap();

        let out = signal::active_awareness::run(&ws, &test_config()).unwrap();
        assert!(out.contains("Live Project"), "the rows must still render; got:\n{out}");
        assert!(out.contains("SCOPE:"), "a populated block must name its scope too; got:\n{out}");
    });
}

/// A4. THE DIFFERENTIAL ARM, and the only one of the four that can catch this.
///
/// It asserts nothing about whether either output is correct. It asserts the two
/// are NOT THE SAME STRING. When the defect IS two states rendering identically,
/// an assertion about either one on its own cannot see it -- A1, A2 and A3 can
/// every one of them pass while absent and empty still produce the same screen.
#[test]
fn absent_and_empty_do_not_render_the_same_thing() {
    let absent_home = tempfile::tempdir().unwrap();
    let absent = base::home::with_thread_home(absent_home.path(), || {
        let ws = tier(&absent_home.path().join("ws"));
        signal::active_awareness::run(&ws, &test_config()).unwrap()
    });

    let empty_home = tempfile::tempdir().unwrap();
    let empty = base::home::with_thread_home(empty_home.path(), || {
        let ws = tier(&empty_home.path().join("ws"));
        crud::project::add(&ws, &ns(), "Done Project", "complete", None).unwrap();
        signal::active_awareness::run(&ws, &test_config()).unwrap()
    });

    assert_ne!(
        absent, empty,
        "absent and empty render identically, so the envelope does not exist \
         whatever the code says.\nabsent:\n{absent}\nempty:\n{empty}"
    );
}


/// A5. A tier that read badly is reported IN THE BLOCK, not only on stderr.
///
/// This is the state the whole prerequisite commit exists to make reachable.
/// Before `load_merged_reporting` the count went to stderr and the caller could
/// not see it, so no block could say this however much it wanted to.
///
/// HONEST NOTE ON ITS RED: this arm was written AFTER the fix, unlike the other
/// four. Its red is asserted from the code path, not observed, because the
/// function it needs did not exist to be called before. Weaker evidence than
/// the arms that were watched failing, and recorded as such.
#[test]
fn a_damaged_tier_is_reported_in_the_block_not_only_on_stderr() {
    let home = tempfile::tempdir().unwrap();
    base::home::with_thread_home(home.path(), || {
        let ws = tier(&home.path().join("ws"));
        // A real project, so the block has rows to render, plus one line that
        // will not parse -- the strict load fails and the lenient one skips it.
        crud::project::add(&ws, &ns(), "Live Project", "active", None).unwrap();
        let g = ws.join(".base").join("graph.nq");
        let mut text = std::fs::read_to_string(&g).unwrap();
        text.push_str("this line is not a quad\n");
        std::fs::write(&g, text).unwrap();

        let out = signal::active_awareness::run(&ws, &test_config()).unwrap();
        assert!(
            out.contains("THIS READ WAS INCOMPLETE"),
            "a skipped line must be reported in the block; got:\n{out}"
        );
        assert!(
            out.contains("skipped 1 malformed line"),
            "the block must name what was skipped, not just that something was; got:\n{out}"
        );
        assert!(
            out.contains("Live Project"),
            "the rows it DID read must still render; a degraded read is not an empty one; got:\n{out}"
        );
    });
}
