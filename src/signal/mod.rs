pub mod active_awareness;
pub mod flow_resurface;
// staleness signal removed — superseded by [protocol] reconcile decay.
pub mod memory;
pub mod pulse;
pub mod suppression;

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::BaseConfig;

/// Signals that surface every session until acted on. Never skipped as unchanged.
const PERSISTENT: [&str; 3] = ["handoff", "reminder", "fork"];

/// One block of a signal's output.
pub struct SignalBlock {
    /// The block's kind in the session-start emission. Its floor and command are keyed on it.
    pub kind: &'static str,
    pub text: String,
    /// How many items the text lists.
    pub items: usize,
    /// How many items exist. More than `items` when the block lists some of them, as HANDOFFS lists
    /// ten of the open handoffs and FORKS the newest three (spec B4, B6).
    pub total: usize,
}

/// One signal's output. Its blocks' texts joined are exactly what the signal rendered, and the
/// suppression hash is taken over that joined text, as it always was.
pub struct Signal {
    pub name: &'static str,
    pub blocks: Vec<SignalBlock>,
    hash: u64,
}

impl Signal {
    fn new(name: &'static str, blocks: Vec<SignalBlock>) -> Self {
        let text: String = blocks.iter().map(|b| b.text.as_str()).collect();
        let hash = suppression::hash_output(&text);
        Signal { name, blocks, hash }
    }

    fn single(
        name: &'static str,
        kind: &'static str,
        text: String,
        total: usize,
        items: usize,
    ) -> Self {
        Self::new(
            name,
            vec![SignalBlock {
                kind,
                text,
                items,
                total,
            }],
        )
    }
}

/// Every signal with something to say, split by whether it is shown this session.
///
/// Nothing is dropped here for size. The old `[signal] max_chars` cap dropped whole signals past
/// 2,000 bytes, exempted the four largest, and reported the drop in one line at the tail, which
/// the host's cut had already removed (measured 2026-09-14: character 40,159 of 45,232). The
/// session-start emission now measures and trims everything, and says what it trimmed.
#[derive(Default)]
pub struct SignalOutput {
    signals: Vec<Signal>,
    unchanged: Vec<Signal>,
    /// No-match tags: `<hook-query:no-match>` for each query that ran but found nothing.
    /// Always emitted — bypass suppression so operator can verify queries executed.
    pub diagnostics: Vec<String>,
    /// The letter and slug of every handoff HANDOFFS lists, for the instruction block and the
    /// letters file `base handoff show` reads.
    pub letters: Vec<(char, String)>,
    state: Option<(PathBuf, suppression::SignalState)>,
}

impl SignalOutput {
    /// Signals to show this session, in emission order.
    pub fn signals(&self) -> &[Signal] {
        &self.signals
    }

    /// Signals skipped because their output has not changed since they were last shown in full.
    pub fn unchanged(&self) -> &[Signal] {
        &self.unchanged
    }

    /// Nothing to show this session.
    pub fn is_empty(&self) -> bool {
        self.signals.is_empty()
    }

    /// Record as shown each signal whose every block `rendered_in_full` accepts, then save.
    ///
    /// The record is what lets a later session skip an unchanged signal, so it has to follow
    /// what reached the output. A signal the budget collapsed was not shown: it stays
    /// unrecorded, and the next session shows it again.
    pub fn record_shown(mut self, rendered_in_full: impl Fn(&str) -> bool) {
        let Some((base_dir, mut state)) = self.state.take() else {
            return;
        };
        for s in &self.signals {
            if s.blocks.iter().all(|b| rendered_in_full(b.kind)) {
                state.update(s.name, s.hash);
            }
        }
        let _ = state.save(&base_dir);
    }
}

/// Run all signals and apply suppression. Returns what to show, what was skipped as unchanged,
/// and diagnostics.
pub fn run_signals(cwd: &Path, config: &BaseConfig, hook: &str) -> Result<SignalOutput> {
    if !config.signal.enabled {
        return Ok(SignalOutput::default());
    }

    let ns = &config.namespace;
    let sig = &config.signal;
    let base_dir = crate::config::find_workspace_base(cwd);

    // (priority, signal): lower priority first, push order kept within a priority.
    let mut results: Vec<(u32, Signal)> = Vec::new();
    let mut diagnostics: Vec<String> = Vec::new();
    let mut letters: Vec<(char, String)> = Vec::new();
    let layout = &config.session_start;
    if layout.handoffs_shown > crate::crud::handoff_show::MAX_SHOWN {
        eprintln!(
            "base: [session_start] handoffs_shown = {} lists {} at most: spec B4 letters them A to J",
            layout.handoffs_shown,
            crate::crud::handoff_show::MAX_SHOWN
        );
    }
    if layout.handoffs_sort != "created_desc" {
        eprintln!(
            "base: [session_start] handoffs_sort = {:?} is not built; handoffs are listed newest created first",
            layout.handoffs_sort
        );
    }

    match memory::run(cwd, config) {
        Ok(block) if !block.text.is_empty() => {
            results.push((0, Signal::single("memory", "memory", block.text, block.total, block.shown)));
        }
        Ok(_) => {}
        Err(e) => eprintln!("base: signal 'memory' failed: {e}"),
    }

    match active_awareness::run_sections(cwd, config) {
        Ok(sections) if !sections.is_empty() => {
            let blocks = sections
                .into_iter()
                .map(|s| SignalBlock {
                    kind: s.kind,
                    text: s.text,
                    items: s.shown,
                    total: s.total,
                })
                .collect();
            results.push((1, Signal::new("active-awareness", blocks)));
        }
        Ok(_) => diagnostics.push(format!("<{hook}-active-awareness:no-match>")),
        Err(e) => eprintln!("base: signal 'active-awareness' failed: {e}"),
    }
    match pulse::run(cwd, ns, sig) {
        Ok(output) if !output.is_empty() => {
            results.push((2, Signal::single("pulse", "pulse", output, 1, 1)));
        }
        Ok(_) => diagnostics.push(format!("<{hook}-pulse:no-match>")),
        Err(e) => eprintln!("base: signal 'pulse' failed: {e}"),
    }
    // Staleness is now owned by [protocol]: reconcile decays cold projects to
    // "deferred" at session-start, so a separate stale-flag scan is redundant.

    // Flow resurface signal (gated by [flow] config)
    if config.flow.enabled && config.flow.resurface {
        match flow_resurface::run(cwd, ns, &config.flow, hook) {
            Ok((output, flow_diags)) => {
                if !output.is_empty() {
                    results.push((
                        2,
                        Signal::single("flow-resurface", "flow-resurface", output, 1, 1),
                    ));
                }
                diagnostics.extend(flow_diags);
            }
            Err(e) => eprintln!("base: signal 'flow-resurface' failed: {e}"),
        }
    }

    // Handoff + reminder resurface — persistent until dismissed. Their own signals so they
    // are never skipped as unchanged: they must surface EVERY session until acted on.
    match flow_resurface::handoff_scan(cwd, ns, layout) {
        Ok((output, list)) if !output.is_empty() => {
            letters = list.letters();
            results.push((
                0,
                Signal::single("handoff", "handoffs", output, list.open, list.shown.len()),
            ));
        }
        Ok(_) => diagnostics.push(format!("<{hook}-handoff-scan:no-match>")),
        Err(e) => eprintln!("base: signal 'handoff' failed: {e}"),
    }
    match flow_resurface::reminder_scan(cwd, ns) {
        Ok((output, n)) if !output.is_empty() => {
            results.push((0, Signal::single("reminder", "reminders", output, n, n)));
        }
        Ok(_) => diagnostics.push(format!("<{hook}-reminder-scan:no-match>")),
        Err(e) => eprintln!("base: signal 'reminder' failed: {e}"),
    }
    // Forks — parallel side-work build-specs. Persistent like handoffs: their own
    // signal so they are never skipped as unchanged and surface every session until
    // picked up, snoozed, or archived. Additive (multiple open).
    match flow_resurface::fork_scan(cwd, ns, layout) {
        Ok((output, open, shown)) if !output.is_empty() => {
            results.push((0, Signal::single("fork", "forks", output, open, shown)));
        }
        Ok(_) => diagnostics.push(format!("<{hook}-fork-scan:no-match>")),
        Err(e) => eprintln!("base: signal 'fork' failed: {e}"),
    }

    // Sort by priority
    results.sort_by_key(|(priority, _)| *priority);

    // Suppression: skip signals whose output has not changed since they were last shown
    let state = base_dir
        .as_deref()
        .map(suppression::SignalState::load)
        .unwrap_or_default();

    let mut signals = Vec::new();
    let mut unchanged = Vec::new();
    for (_, s) in results {
        if PERSISTENT.contains(&s.name) || state.is_novel(s.name, s.hash) {
            signals.push(s);
        } else {
            unchanged.push(s);
        }
    }

    // The state is saved only when something was novel, as it always was: a session with
    // nothing new to show leaves the file alone.
    let state = if signals.is_empty() {
        None
    } else {
        base_dir.map(|dir| (dir, state))
    };

    Ok(SignalOutput {
        signals,
        unchanged,
        diagnostics,
        letters,
        state,
    })
}
