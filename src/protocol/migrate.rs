//! Rank 09: the upgrade migration for existing users (spec Part G).
//!
//! # What it does
//!
//! K13, ruled 2026-09-18 (Chris's option, approved by `auk`): on upgrade, reset `lastActive` to the
//! upgrade timestamp for every active record that would otherwise defer. Everything starts at day 0,
//! and the window then measures what it was built to measure. Before this, `lastActive == createdAt`
//! meant *never written*, so deferring on it read an absent signal as a decision (law 7: absent is
//! not empty and not zero).
//!
//! # Why it asks first
//!
//! D1, and it SURVIVES K13 (`auk`, 2026-09-18). The lead argument was never the notice, it was
//! CONSISTENCY: Part G contains two migrations and only one of them asks. G4 step 3 has the rule
//! migration write nothing until the operator approves. K13 changed WHAT this migration does — mass
//! defer became mass write — and did not touch the asymmetry: before, one asks over 141 rules and one
//! does not over 165 records; after, one asks over 141 rules and one does not over 147 records. An
//! argument dies when its premise dies, and this premise never depended on what the migration did.
//!
//! So: [`plan`] computes and writes nothing. [`apply`] is reached only through
//! `base defer migrate --apply`.
//!
//! # The three K13 conditions, and where each one lives in this file
//!
//! 1. **Delete-then-insert is MANDATORY**, never a plain insert. A plain insert would create
//!    multi-valued `lastActive` on 147 records — new instances of the exact defect commit 5 fixed.
//!    Measured before the ruling: multi-valued `lastActive` is 1 subject of 985 in the workspace and
//!    1 of 143 global. The risk is not today's dirt, it is *manufacturing* 147 new instances.
//!    It also makes the `rows_in` display concern moot for this field: a path that picks the first
//!    row never has two rows to choose between when exactly one value exists.
//!    → [`reset_op`], which is the only place a `lastActive` write is built.
//! 2. **Only records that would otherwise defer are touched.** A record that is already fine is not
//!    written, so the plan count IS the write size and the blast radius is knowable before it runs.
//!    → [`plan`] keeps `Action::Defer` and nothing else.
//! 3. **NO JITTER.** The reset timestamps are not spread to soften the day-11 cliff. A record stamped
//!    five days ago that nobody touched five days ago is a field not meaning what it claims — the
//!    exact defect this option exists to remove. Trading one lie for a prettier lie is not a fix.
//!    → [`apply`] computes `now` ONCE, outside every loop, and every record gets that one value.
//!
//! # What this deliberately does not build
//!
//! - **No first-run notice.** K13 removes the mass defer, so the G5 notice has no event to attach to.
//!   The requirement is satisfied because there is nothing to report, NOT because it was skipped.
//!   Those two read the same and are not.
//! - **No cushion for the day-11 cliff.** The cliff is synchronised because the upgrade happened at
//!   one moment, which is TRUE. Ruled: nothing cushions it and we are not building one, because the
//!   alternative is a fake cushion.
//! - **No new session-start surface.** The preview is what the operator sees when THEY run
//!   `base defer migrate`. Session start does not grow a block, so the 1,990-unit bar is untouched.
//!
//! # ROW 16 — `[protocol] stale_days` is LIVE and must never be listed as dead legacy
//!
//! G6's legacy-row list must NOT name `[protocol] stale_days`. It is still read as the project
//! fallback at `config.rs:766` — `d.days.project.unwrap_or(self.protocol.stale_days)` — so listing
//! it as a dead legacy row would be wrong, and an operator told to delete it would change real
//! behaviour. It looks like a legacy row because its sibling `[signal] stale_days` is one; they
//! are different keys under different sections and only the `[signal]` one is inert.
//!
//! # ROW 28 — unknown keys under known sections are OUT OF SCOPE, said out loud
//!
//! `auk` gave two acceptable answers: G6 gains a pass for unknown keys under known sections, or it
//! is stated out loud that they are out of scope. This is the second, and the reason is on the
//! record rather than left as silence, because silence here ships a config file that lies to its
//! owner.
//!
//! The case that found it: `[signal] stale_days = 14` sits in a real `base.toml`. `SignalConfig` has
//! exactly three fields — `max_chars`, `enabled`, `scope` — and there is no `deny_unknown_fields` in
//! `config.rs`, so the key deserializes into nothing and is silently dropped. It has never done
//! anything. G6's legacy-row list names keys base KNOWS about, so it would never name this one, and
//! the operator keeps a line that reads as meaningful forever.
//!
//! **Why out of scope rather than built here:** detecting it needs a list of every key base knows,
//! per section. Lane 3's standing cross-lane rule is that rank 09's legacy rows join lane 1's
//! EXISTING `BaseConfig::legacy_keys`, never a second list — and that list arrives with plover's
//! commit F. Writing a parallel key list to catch this would create exactly the duplicate the rule
//! forbids, and it would go stale the first time a key is added.
//!
//! **Where it attaches when commit F lands:** the unknown-key pass reads `legacy_keys` for the
//! known set and reports anything under a known section that is not in it. That is one function
//! against one list. It is not built here because the list is not here yet, which is a sequencing
//! fact and not a judgement that the problem does not matter.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Local};

use crate::config::BaseConfig;
use crate::protocol::reconcile::{plan_records, Action, RecordDecision};
use crate::{crud, store};

/// The marker naming this migration's state, a sibling of the other `.base/` markers
/// (`.last-auto-compact`, `.domain-sync-ts`). Plain `key=value` lines, because rollback needs the
/// snapshot paths and a bare timestamp cannot carry them.
const MARKER: &str = ".defer-migration";

/// Where the migration stands for one tier root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Never run here. The pass must not write (see [`blocks_automatic_defer`]).
    Pending,
    /// Applied at this moment. Normal behaviour from here on.
    Applied(DateTime<Local>),
}

/// One record the reset would touch, and the tier file it lives in.
#[derive(Debug, Clone)]
pub struct Reset {
    pub iri: String,
    pub slug: String,
    pub kind: crate::config::DeferKind,
    /// Days on the clock today — the number that would have deferred it.
    pub days: Option<i64>,
    /// The window its kind gives it.
    pub window: i64,
}

/// What [`apply`] would do, per tier. Built without a lock and without writing.
// NO `Default` DERIVE, deliberately (`auk` G2, 2026-09-18). A default-constructed `Plan` has empty
// tiers AND `full_total` 0, which `apply` reads as NothingToDo — the one empty case allowed to mark
// the migration Applied. That is the defect this type exists to close, in the exact shape it closes
// it. Rather than confirm no caller does it today, the type makes it impossible: every construction
// must state `full_total`, so there is no way to build one that lies about what it was staged from.
#[derive(Debug)]
pub struct Plan {
    /// `(tier name, tier file, the records that would otherwise defer)`.
    pub tiers: Vec<(&'static str, PathBuf, Vec<Reset>)>,
    /// The total of the plan THIS one was staged from. Equal to [`Plan::total`] for an unstaged
    /// plan, and larger for a staged one.
    ///
    /// This field exists because of a BLOCKING defect (`auk` G2, 2026-09-18). `apply` used to
    /// receive only the staged plan and so could not tell "nothing to do" from "you filtered
    /// everything out" — the two are the same COUNT and completely different FACTS. It marked the
    /// whole migration Applied either way, which lifted the write block and let every record the
    /// operator had deliberately NOT staged defer in one pass on the next session start. That is
    /// the mass defer K13 exists to prevent, reached through the staging feature that exists to
    /// make the operator safe.
    ///
    /// It is carried ON THE PLAN rather than passed as an argument on purpose: a parameter can be
    /// forgotten by the next caller, and the one thing this defect proves is that the caller is
    /// exactly who forgets. `stage` sets it; nothing else may.
    pub full_total: usize,
}

impl Plan {
    /// The write size. Condition 2 is what makes this number the blast radius rather than an estimate.
    pub fn total(&self) -> usize {
        self.tiers.iter().map(|(_, _, r)| r.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }

    /// D1 step 3: `--limit N` and `--older-than D` stage the reset so an operator can take it in
    /// pieces rather than all at once.
    ///
    /// `older_than` keeps only records at least that many days cold, which is a property of the
    /// record. `limit` then caps the total, which is not — it is whatever the walk reached first.
    /// So the two are applied in that order, and never the reverse: filtering after capping would
    /// silently return fewer than N records that met the filter, and the operator would have no way
    /// to tell a short list from an exhausted one.
    ///
    /// A record with no clock at all (`days` is `None`) is EXCLUDED by `older_than`. Absent is not
    /// zero and it is not old (law 7): a record carrying no time has not been shown to be cold, and
    /// a stager that treats missing as qualifying would widen a limited run rather than narrow it.
    pub fn stage(&self, limit: Option<usize>, older_than: Option<i64>) -> Plan {
        let mut tiers: Vec<(&'static str, PathBuf, Vec<Reset>)> = Vec::new();
        let mut room = limit.unwrap_or(usize::MAX);
        for (tier, file, resets) in &self.tiers {
            if room == 0 {
                break;
            }
            let mut kept: Vec<Reset> = resets
                .iter()
                .filter(|r| match older_than {
                    Some(d) => r.days.is_some_and(|days| days >= d),
                    None => true,
                })
                .cloned()
                .collect();
            kept.truncate(room);
            room -= kept.len();
            if !kept.is_empty() {
                tiers.push((*tier, file.clone(), kept));
            }
        }
        // `full_total` is set AT CONSTRUCTION, not assigned afterwards: an early return added later
        // cannot skip it, because there is no moment when the value exists without it.
        Plan { tiers, full_total: self.total() }
    }

    /// Whether this plan is the whole of what it was staged from. Only a complete run may mark the
    /// migration Applied.
    pub fn is_complete(&self) -> bool {
        self.total() == self.full_total
    }
}

/// Which of the four cases [`apply`] hit. The two empty ones are the same COUNT and different
/// FACTS, and conflating them was the blocking defect: see [`Plan::full_total`].
#[derive(Debug)]
pub enum Applied {
    /// The full plan was empty — there is genuinely nothing to reset. The ONLY empty case that may
    /// mark the migration Applied.
    NothingToDo,
    /// Every record in the full plan was written. State is now `Applied`.
    Complete(Outcome),
    /// Part of the full plan was written. Snapshots are recorded so `--rollback` still works, and
    /// **the state stays `Pending`** so the write block stays up and the next run continues.
    Partial { outcome: Outcome, done: usize, full: usize },
}

/// What [`apply`] did, and where the snapshots went so `--rollback` can find them.
#[derive(Debug, Default)]
pub struct Outcome {
    pub reset: usize,
    /// `(tier file, the snapshot taken before it was written)`.
    pub snapshots: Vec<(PathBuf, PathBuf)>,
}

/// Read the migration state for `base_dir`. An unreadable or absent marker is [`State::Pending`]:
/// the failure direction is deliberate (law 41). A migration we cannot prove ran has not run, and
/// treating an unreadable marker as Applied would let the pass write the very mass defer that K13
/// exists to prevent.
pub fn state(base_dir: &Path) -> State {
    let path = base_dir.join(MARKER);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return State::Pending;
    };
    for line in text.lines() {
        if let Some(stamp) = line.strip_prefix("applied=")
            && let Ok(t) = DateTime::parse_from_rfc3339(stamp.trim())
        {
            return State::Applied(t.with_timezone(&Local));
        }
    }
    State::Pending
}

// `blocks_automatic_defer` WAS HERE and was removed with the gate, 2026-09-19. It answered "must
// the automatic deferral pass refuse to WRITE" — a question with no caller once deferring is
// understood as recoverable rather than destructive. Its one consumer, `reconcile_records`, carries
// the reasoning at the site the guard used to occupy. The predicate is GONE, not relocated
// somewhere quieter.

/// Every record that would otherwise defer, per tier. Reads without a lock and writes nothing.
///
/// Condition 2 lives here: only `Action::Defer` is kept, so a record that is already fine is never
/// written and the count is the write size.
pub fn plan(gbl_root: Option<&Path>, cwd: &Path, config: &BaseConfig) -> Result<Plan> {
    let now = Local::now();
    let mut tiers: Vec<(&'static str, PathBuf, Vec<Reset>)> = Vec::new();
    for file in crud::all_tier_files(gbl_root, cwd) {
        let tier = tier_name(&file);
        let decisions = plan_records(&store::load_graph(&file)?, config, now)?;
        let resets: Vec<Reset> = decisions
            .iter()
            .filter(|d| matches!(d.action, Action::Defer))
            .map(Reset::from)
            .collect();
        if !resets.is_empty() {
            tiers.push((tier, file, resets));
        }
    }
    // An unstaged plan IS its own full set, and it says so at construction.
    let full_total = tiers.iter().map(|(_, _, r)| r.len()).sum();
    Ok(Plan { tiers, full_total })
}

impl From<&RecordDecision> for Reset {
    fn from(d: &RecordDecision) -> Self {
        Reset {
            iri: d.iri.clone(),
            slug: d.slug.clone(),
            kind: d.kind,
            days: d.days,
            window: d.window,
        }
    }
}

/// The ONE place a `lastActive` write is built, so condition 1 has a single home.
///
/// DELETE-then-INSERT, never a bare INSERT. The `WHERE` binds on `status`, which every record
/// carries, and takes `lastActive` as OPTIONAL, so a record holding no `lastActive` at all still
/// gets exactly one — the same shape `apply_records` uses for its Defer write.
fn reset_op(p: &str, iri: &str, stamp: &str) -> String {
    format!(
        "DELETE {{ GRAPH ?g {{ <{iri}> {p}:lastActive ?old }} }}\n\
         INSERT {{ GRAPH ?g {{ <{iri}> {p}:lastActive \"{stamp}\"^^xsd:dateTime }} }}\n\
         WHERE  {{ GRAPH ?g {{ <{iri}> {p}:status ?st }}\n\
           OPTIONAL {{ GRAPH ?g {{ <{iri}> {p}:lastActive ?old }} }} }}"
    )
}

/// Perform the reset. Snapshots every tier file it is about to write (G4 step 1) via the same
/// `store::snapshot` that repair, restore, compact and purge use — an existing shape, not a parallel
/// one — then writes under the tier lock and records the marker.
///
/// Condition 3 lives here: `stamp` is computed ONCE, before the loop, so every record in every tier
/// receives the identical timestamp. Do not move it inside a loop to spread the cliff.
pub fn apply(base_dir: &Path, plan: &Plan, config: &BaseConfig) -> Result<Applied> {
    let mut outcome = Outcome::default();
    // The two empty cases, separated. They carry the same count and mean opposite things.
    if plan.is_empty() {
        if plan.full_total > 0 {
            // Empty BECAUSE FILTERED. Refusing is the whole point: marking here would lift the
            // write block having written nothing, and with no snapshot recorded `--rollback` could
            // not undo the state flip either.
            anyhow::bail!(
                "the filter matched none of the {} records waiting. NOTHING was written and the \
                 migration is still PENDING. Widen --older-than or raise --limit, or drop both to \
                 take all {}.",
                plan.full_total,
                plan.full_total
            );
        }
        // Empty because there is genuinely nothing to reset. The only empty case that may mark.
        mark_applied(base_dir, &outcome)?;
        return Ok(Applied::NothingToDo);
    }
    // NO JITTER: one timestamp, computed once, for every record in every tier.
    let stamp = crud::now_iso();
    let p = &config.namespace.prefix;
    let pfx = crud::prefixes(&config.namespace);

    for (_tier, file, resets) in &plan.tiers {
        let backup = store::snapshot(file, "pre-defer-migrate")
            .with_context(|| format!("failed to snapshot {} before the reset", file.display()))?;
        let ops: Vec<String> = resets.iter().map(|r| reset_op(p, &r.iri, &stamp)).collect();
        store::with_graph_lock(file, || {
            let st = store::load_graph(file)?;
            store::mutate_and_write(&st, file, "", store::Scope::Wide, store::Intent::Knowledge, |s| {
                for op in &ops {
                    s.update(&format!("{pfx}\n{op}"))
                        .with_context(|| format!("lastActive reset failed: {op}"))?;
                }
                Ok(Some(ops.join(";\n")))
            })
        })?;
        outcome.reset += resets.len();
        outcome.snapshots.push((file.clone(), backup));
    }
    if plan.is_complete() {
        mark_applied(base_dir, &outcome)?;
        return Ok(Applied::Complete(outcome));
    }
    // PARTIAL. Record the snapshots so `--rollback` works, and leave the state Pending so
    // `blocks_automatic_defer` keeps the write block up and the next run continues.
    record_progress(base_dir, &outcome)?;
    let done = outcome.reset;
    Ok(Applied::Partial { outcome, done, full: plan.full_total })
}

/// G4 step 5. Restore every tier this migration wrote, from the snapshot it took, using
/// `doctor::restore_tier` — which snapshots the CURRENT file first, so a wrong rollback is itself
/// recoverable. Reuses the existing restore path rather than hand-writing the live file.
pub fn rollback(base_dir: &Path) -> Result<usize> {
    let recorded = read_snapshots(base_dir)?;
    if recorded.is_empty() {
        anyhow::bail!(
            "no recorded snapshot for this migration: nothing to roll back. \
             `{MARKER}` names the tiers written and it names none."
        );
    }
    // Restore each tier to the state before the FIRST write of this migration. Several partial runs
    // can snapshot the same tier more than once; the later ones are mid-migration states, and
    // rolling back to one of those would undo only part of what the operator asked to undo.
    let mut first_per_tier: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (file, backup) in recorded {
        if !first_per_tier.iter().any(|(f, _)| f == &file) {
            first_per_tier.push((file, backup));
        }
    }
    let mut n = 0;
    for (file, backup) in &first_per_tier {
        crate::doctor::restore_tier(file, backup)
            .with_context(|| format!("failed to roll back {}", file.display()))?;
        n += 1;
    }
    std::fs::remove_file(base_dir.join(MARKER))
        .with_context(|| format!("rolled back {n} tiers but could not clear {MARKER}"))?;
    Ok(n)
}

/// Record a PARTIAL run: its snapshots, and deliberately NO `applied=` line, so [`state`] still
/// reads `Pending` and the write block stays up.
///
/// Appends rather than overwrites, so several partial runs accumulate their snapshots. Overwriting
/// would throw away the earlier run's backup and make the first half of the migration
/// unrollbackable — the same shape as the defect this whole change exists to fix.
fn record_progress(base_dir: &Path, outcome: &Outcome) -> Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(base_dir).ok();
    let path = base_dir.join(MARKER);
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("failed to open {MARKER} to record partial progress"))?;
    writeln!(f, "partial={} at={}", outcome.reset, Local::now().to_rfc3339())
        .with_context(|| format!("failed to record partial progress in {MARKER}"))?;
    for (file, backup) in &outcome.snapshots {
        writeln!(f, "snapshot={}\t{}", file.display(), backup.display())
            .with_context(|| format!("failed to record a snapshot in {MARKER}"))?;
    }
    Ok(())
}

fn mark_applied(base_dir: &Path, outcome: &Outcome) -> Result<()> {
    // Snapshots an earlier PARTIAL run recorded are carried forward. Rewriting the marker with only
    // this run's snapshots would silently drop them, and the earlier half would stop being
    // rollbackable at the moment the migration completed.
    let earlier = read_snapshots(base_dir).unwrap_or_default();
    let mut s = format!("applied={}\n", Local::now().to_rfc3339());
    s.push_str(&format!("reset={}\n", outcome.reset));
    for (file, backup) in earlier.iter().chain(outcome.snapshots.iter()) {
        s.push_str(&format!("snapshot={}\t{}\n", file.display(), backup.display()));
    }
    std::fs::create_dir_all(base_dir).ok();
    std::fs::write(base_dir.join(MARKER), s)
        .with_context(|| format!("failed to write {MARKER}"))?;
    Ok(())
}

fn read_snapshots(base_dir: &Path) -> Result<Vec<(PathBuf, PathBuf)>> {
    let text = match std::fs::read_to_string(base_dir.join(MARKER)) {
        Ok(t) => t,
        Err(e) => anyhow::bail!("cannot read {MARKER}: {e}"),
    };
    Ok(text
        .lines()
        .filter_map(|l| l.strip_prefix("snapshot="))
        .filter_map(|rest| rest.split_once('\t'))
        .map(|(f, b)| (PathBuf::from(f), PathBuf::from(b)))
        .collect())
}

/// G4 step 7: the migration names the tier it writes to, so the operator is never told a number
/// without being told where it lands.
fn tier_name(file: &Path) -> &'static str {
    let s = file.to_string_lossy();
    if s.contains(".base-gbl") {
        "global"
    } else {
        "workspace"
    }
}

/// The preview. Names the tier (G4 step 7), the honest denominator, and the command that applies it.
pub fn format_plan(plan: &Plan) -> String {
    if plan.is_empty() {
        return "Deferral migration: nothing to reset. Every record's clock already reads a real \
                touch, so the window measures what it claims.\n"
            .to_string();
    }
    let mut s = format!(
        "Deferral migration — DRY RUN, nothing has been written.\n\
         {} records would have their activity clock reset to now, so the window starts measuring \
         from today instead of from a field that was never written.\n",
        plan.total()
    );
    for (tier, _file, resets) in &plan.tiers {
        s.push_str(&format!("  {tier}: {} records\n", resets.len()));
    }
    s.push_str(GATING_TRAP);
    s.push_str("\nApply:      base defer migrate --apply\nRoll back:  base defer migrate --rollback\n");
    s
}

/// ROW 29 — the gating trap, printed to the operator on the preview rather than footnoted in a
/// design doc, because it is the moment it can still change what they do.
///
/// Read from source, not assumed (`auk`, 2026-09-18): `reconcile()` returns early on
/// `!config.protocol.enabled` and `reconcile_records()` returns early on `!config.defer.enabled`
/// and NEVER consults `protocol.enabled`. Two entry points, two independent gates, not layered.
/// Mirrored in the CLI at `cli.rs:3375` and `:3398`.
///
/// So an operator who set `[protocol] enabled = false` intending to switch automatic deferral off
/// still gets record deferral on session start. On the upgrade run that is every one of these
/// records moving for someone who believes they turned the feature off. That belongs in front of
/// them here, not in a footnote they will never read.
const GATING_TRAP: &str = "\n    Note: record deferral is gated by [defer] enabled, NOT by [protocol] enabled.\n    They are separate gates and they are not layered. If you set [protocol] enabled = false\n    expecting this to stop, it does not — these records still move. Set [defer] enabled = false.\n";


// `mark_fresh_install` WAS HERE and was DELETED on 2026-09-19, function and doc comment together.
//
// It recorded the deferral migration as already applied at install time, so that an absent marker
// would mean exactly one thing: upgraded from a version predating the feature. That distinction had
// one consumer, the write gate in `reconcile::reconcile_records`, and the gate is gone. With nothing
// to gate, a machine-wide claim about migration state has nothing left to decide, so it is not
// preserved in a quieter form — it is removed.
//
// `base install` no longer touches migration state at all (see `install::run`). A 0.16.0 install
// carries no marker until an operator runs `base defer migrate --apply` themselves.

/// Where the marker lives: the GLOBAL `.base/` when there is one, because this migration spans every
/// tier and one run covers the install. Falls back to the workspace `.base/` when base was never
/// installed globally.
pub fn marker_root(cwd: &Path) -> Option<PathBuf> {
    crate::config::global_base_dir().or_else(|| crate::config::find_workspace_base(cwd))
}

// `blocks_automatic_defer_for` WAS HERE, the cwd-taking form of the predicate above, and went with
// it on 2026-09-19. Both of its call sites — `reconcile::reconcile_records` and
// `hook::session_start` — were removed in the same change.
