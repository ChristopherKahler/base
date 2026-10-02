//! Every number session start shows, counted once (BO-06, F10).
//!
//! WHAT WAS WRONG. Session start for session 5b860473 (2026-10-01 12:57) printed `projects 0 · tasks 0` in its header,
//! `Projects: 28 active` and `Tasks: 145 open` in its pulse, and `Reminders: 4 overdue` beside a DUE NOW of 5, all in
//! one output. Read from the code at 50295b9, three causes:
//!
//! 1. The header read each count off the block printed with it, and a block skipped as unchanged since an earlier
//!    session is not printed, so its count read 0. It never meant "0 shown".
//! 2. The pulse ran four queries of its own over the workspace tier only: projects and tasks whose status is exactly
//!    "active", and reminders with a `dueDate` before today, archived ones included. The blocks count working records
//!    in both tiers, and DUE NOW counts live reminders whose `resurfaceAt` has passed.
//! 3. "Overdue" (pulse) and "due" (DUE NOW) were two rules.
//!
//! Now each number is counted once, by the scan that lists its items, and kept in one [`Counts`]. The header, the pulse
//! and each block's first line print it from there, so the same label carries the same number everywhere.

/// Projects, tasks or milestones in the working set: how many are active, and how many the block lists.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Work {
    /// Working records: any status but blocked, deferred, completed or archived.
    pub active: usize,
    /// How many of them the block lists (the ones on a project touched recently).
    pub listed: usize,
    pub blocked: usize,
    /// Status `complete`, `completed` or `done`. Counted for projects only, the pulse's one use: the working-set query
    /// leaves completed tasks and milestones out, so a long history costs session start nothing (BO-06 review).
    pub completed: usize,
    /// Status `deferred`: open but paused, counted on the block's notice line, never listed.
    pub deferred: usize,
}

/// Every count session start prints. Filled once per session start, in [`super::run_signals`], whether or not the
/// signal that lists the items is shown this session: a block skipped as unchanged still has a count.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counts {
    /// Reminders due now ([`crate::crud::reminder::is_due`]), as DUE NOW numbers them.
    pub reminders_due: usize,
    pub handoffs_open: usize,
    pub handoffs_listed: usize,
    pub handoffs_deferred: usize,
    pub forks_open: usize,
    pub forks_listed: usize,
    pub forks_deferred: usize,
    /// Projects in this session's scope: the current workspace and the operator's own un-homed ones.
    pub projects: Work,
    pub tasks: Work,
    pub milestones: Work,
    /// Decisions logged in the last seven days, in both tiers.
    pub decisions_week: usize,
    /// The labels whose scan failed this session start (`due`, `handoffs`, `forks`, `projects`, `tasks`,
    /// `milestones`). Their numbers are unknown: line 1 prints `?` for them and the pulse leaves their line out, instead
    /// of a 0 nobody counted (BO-06 review). The failure itself goes to stderr where the scan failed.
    pub failed: Vec<&'static str>,
}

impl Counts {
    /// `n` as line 1 prints it under `label`: `?` when that label's scan failed.
    pub fn shown(&self, label: &str, n: usize) -> String {
        if self.failed.contains(&label) {
            "?".to_string()
        } else {
            n.to_string()
        }
    }

    /// Whether `label`'s number was counted this session start.
    pub fn known(&self, label: &str) -> bool {
        !self.failed.contains(&label)
    }

    /// Records marked deferred across handoffs, forks, projects, tasks and milestones (spec B2, line 1).
    pub fn deferred(&self) -> usize {
        self.handoffs_deferred
            + self.forks_deferred
            + self.projects.deferred
            + self.tasks.deferred
            + self.milestones.deferred
    }
}
