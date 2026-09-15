//! The output budget: base measures what a hook is about to say before it says it.
//!
//! Claude Code persists any hook output over its limit and hands Claude a 2,000-character
//! preview of it. Under the limit Claude sees everything; over it, the first 2,000
//! characters, however far over the output is. base used to print first and get cut later.
//!
//! Measured 2026-09-14 at `0ace1ba` on a real session start, off the file Claude Code itself
//! persisted: 46,105 bytes, 45,232 `char`s, 45,234 UTF-16 code units, 384 lines. Six of nine
//! blocks were lost whole, and the `[+N signals suppressed — budget cap]` notice sat at
//! character 40,159: the report of the loss was destroyed by the loss it reported.
//!
//! Every hook event's output is assembled here as ranked blocks, trimmed to its budget, and
//! only then emitted. The unit is UTF-16 code units, because the host is JavaScript and its
//! limit is a JS string length. On the reference output bytes over-count by 871 and `char`s
//! under-count by 2, so neither is the number the host applies.
//!
//! Output written to stderr is outside every budget by construction: Claude Code feeds only a
//! hook's stdout to the model.

use std::path::{Path, PathBuf};

/// The length the host measures: UTF-16 code units. Nothing in this module measures any other way.
pub fn u16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Trim order, and output order. `Pinned` and `DueNow` are never degraded. DUE NOW shows in full
/// and first even when that pushes the first screen over: the overflow is reported in
/// [`Rendered::first_screen_ok`], never resolved by collapsing what is due. The rest degrade from
/// `Tail` upward.
///
/// It is a total order on purpose: a trimmer that picks the biggest block produces a different
/// layout run to run, so nobody can learn where to look and no test can assert a position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rank {
    /// The header map line's companions: the instruction block.
    Pinned,
    /// Due now.
    DueNow,
    /// Handoffs.
    Primary,
    /// Forks, projects, tasks, milestones.
    Secondary,
    /// Everything else, one line each at worst.
    Tail,
}

/// How much of a block is rendered. `Collapsed` is the floor: one line with a count and the
/// command that prints the rest. A block never disappears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Full,
    Shortened,
    Collapsed,
}

/// Why something is not shown in full. Every path that removes content lands in one ledger,
/// because a counter incremented at each removal site fails in one direction only, and that
/// direction is clean: the site nobody instrumented reports nothing withheld.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Collapsed to its count line.
    Collapsed,
    /// The shortened form lists fewer items.
    ListCut,
    /// The shortened form carries the same items in less text.
    TextShortened,
    /// Suppressed by a path outside the trimmer.
    SignalSuppressed,
    /// Skipped because its output had not changed since an earlier session.
    HashUnchanged,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Collapsed => "collapsed",
            Reason::ListCut => "list cut",
            Reason::TextShortened => "shortened",
            Reason::SignalSuppressed => "suppressed",
            Reason::HashUnchanged => "unchanged",
        }
    }
}

/// One row of the withheld ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Withheld {
    pub block: String,
    pub items: usize,
    pub reason: Reason,
    pub command: String,
}

/// One renderable block of a hook's output.
#[derive(Debug, Clone)]
pub struct Block {
    id: String,
    rank: Rank,
    full: String,
    full_shown: usize,
    shortened: Option<(String, usize)>,
    collapsed: String,
    collapsed_given: bool,
    items_total: usize,
    command: String,
    level: Level,
}

impl Block {
    /// `collapsed` is the one-line floor. Pass an empty string and the block builds its own
    /// from its id, its count and its command, so no block can collapse into nothing.
    pub fn new(
        id: impl Into<String>,
        rank: Rank,
        full: impl Into<String>,
        collapsed: impl Into<String>,
        command: impl Into<String>,
    ) -> Self {
        let collapsed = collapsed.into();
        let collapsed_given = !collapsed.trim().is_empty();
        let mut b = Block {
            id: id.into(),
            rank,
            full: full.into(),
            full_shown: 0,
            shortened: None,
            collapsed,
            collapsed_given,
            items_total: 0,
            command: command.into(),
            level: Level::Full,
        };
        b.refresh_floor();
        b
    }

    /// How many items exist, and how many the full rendering lists.
    pub fn items(mut self, total: usize, shown: usize) -> Self {
        self.items_total = total;
        self.full_shown = shown.min(total);
        self.refresh_floor();
        self
    }

    /// The middle rendering, listing `shown` items. Call it before or after [`Block::items`]:
    /// the count is clamped to the full rendering's when it is read, never when it is stored,
    /// so the order of the two calls cannot change what the ledger records.
    pub fn with_shortened(mut self, text: impl Into<String>, shown: usize) -> Self {
        self.shortened = Some((text.into(), shown));
        self
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn rank(&self) -> Rank {
        self.rank
    }

    pub fn level(&self) -> Level {
        self.level
    }

    pub fn items_total(&self) -> usize {
        self.items_total
    }

    pub fn command(&self) -> &str {
        &self.command
    }

    /// Items listed at the current level.
    pub fn items_shown(&self) -> usize {
        match (self.level, &self.shortened) {
            (Level::Full, _) | (Level::Shortened, None) => self.full_shown,
            (Level::Shortened, Some((_, shown))) => (*shown).min(self.full_shown),
            (Level::Collapsed, _) => 0,
        }
    }

    /// The text at the current level.
    pub fn text(&self) -> &str {
        match (self.level, &self.shortened) {
            (Level::Full, _) | (Level::Shortened, None) => &self.full,
            (Level::Shortened, Some((text, _))) => text,
            (Level::Collapsed, _) => &self.collapsed,
        }
    }

    fn refresh_floor(&mut self) {
        if !self.collapsed_given {
            self.collapsed = format!("{} {} · all: {}", self.id, self.items_total, self.command);
        }
    }

    /// `None` for `Pinned` and at the floor. Returning `None` for `Pinned` is what makes
    /// "never trimmed" structural rather than a rule a caller has to remember.
    fn next_level(&self) -> Option<Level> {
        if self.rank == Rank::Pinned {
            return None;
        }
        match self.level {
            Level::Full if self.shortened.is_some() => Some(Level::Shortened),
            Level::Full | Level::Shortened => Some(Level::Collapsed),
            Level::Collapsed => None,
        }
    }
}

/// Where the full, untrimmed output went. Only [`write_full_output`] can produce a written
/// path, so a header can name the file only after the file exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullOutput(FullState);

#[derive(Debug, Clone, PartialEq, Eq)]
enum FullState {
    Written(String),
    Failed(String),
    Off,
}

impl FullOutput {
    /// No full-output file for this event.
    pub fn off() -> Self {
        FullOutput(FullState::Off)
    }

    /// No file, for a reason that is not an I/O error, such as no directory to hold it. The
    /// reason reaches every floor that would have named the file.
    pub fn not_written(reason: impl Into<String>) -> Self {
        FullOutput(FullState::Failed(reason.into()))
    }

    pub fn written_path(&self) -> Option<&str> {
        match &self.0 {
            FullState::Written(p) => Some(p),
            _ => None,
        }
    }

    pub fn failure(&self) -> Option<&str> {
        match &self.0 {
            FullState::Failed(e) => Some(e),
            _ => None,
        }
    }
}

/// Write the untrimmed output to `path` through a temp file and a rename. Never panics: a
/// failure comes back as a value carrying the reason, so the header can say the file is
/// missing instead of pointing at one that is not there.
pub fn write_full_output(path: &Path, text: &str) -> FullOutput {
    match write_via_temp(path, text) {
        Ok(()) => FullOutput(FullState::Written(path.display().to_string())),
        Err(e) => FullOutput(FullState::Failed(format!("{}: {e}", path.display()))),
    }
}

fn write_via_temp(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // One temp name per process. Two sessions starting in one workspace write the same file,
    // and a shared temp name would let one of them rename the other's half-written bytes.
    let tmp = temp_path(path);
    std::fs::write(&tmp, text)?;
    // The rename retries the Windows lock family, as the graph write does. A rename that still
    // fails must not leave the temp behind: nothing else would ever remove it.
    crate::store::rename_with_retry(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

fn temp_path(path: &Path) -> PathBuf {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    PathBuf::from(tmp)
}

/// What a header renderer can see: every block at its final level, the ledger, and where the
/// full output went.
pub struct Facts<'a> {
    pub blocks: &'a [Block],
    pub withheld: &'a [Withheld],
    pub full: &'a FullOutput,
}

impl Facts<'_> {
    pub fn withheld_total(&self) -> usize {
        self.withheld.iter().map(|w| w.items).sum()
    }

    pub fn block(&self, id: &str) -> Option<&Block> {
        self.blocks.iter().find(|b| b.id == id)
    }
}

/// A header renderer. Its output is forced onto one line.
pub type Header<'h> = &'h dyn Fn(&Facts<'_>) -> String;

/// One hook event's output after trimming, with the numbers `base doctor` reports.
#[derive(Debug, Clone)]
pub struct Rendered {
    /// Exactly what to print.
    pub text: String,
    pub emitted_u16: usize,
    /// The untrimmed blocks, in UTF-16 code units, without the header line.
    pub full_u16: usize,
    pub budget_u16: usize,
    pub first_screen_u16: usize,
    /// Every block is at its floor and the output is still over budget. Reported, never
    /// resolved by truncating: silent truncation is the defect this module replaces.
    pub over_budget: bool,
    /// The header, the `Pinned` blocks and the `DueNow` blocks fit the first screen. Reported,
    /// never resolved: those blocks are the whole first screen and none of them may degrade, so
    /// trimming anything else cannot make them fit.
    pub first_screen_ok: bool,
    pub withheld: Vec<Withheld>,
    pub blocks: Vec<Block>,
}

impl Rendered {
    pub fn withheld_total(&self) -> usize {
        self.withheld.iter().map(|w| w.items).sum()
    }

    /// True when the trimmer degraded anything in this run.
    pub fn trimmed(&self) -> bool {
        self.withheld.iter().any(|w| {
            matches!(
                w.reason,
                Reason::Collapsed | Reason::ListCut | Reason::TextShortened
            )
        })
    }
}

/// One hook event's complete output, measured before it is emitted.
pub struct Emission {
    blocks: Vec<Block>,
    withheld: Vec<Withheld>,
    budget_u16: usize,
    first_screen_u16: usize,
}

impl Emission {
    pub fn new(budget_u16: usize, first_screen_u16: usize) -> Self {
        Emission {
            blocks: Vec::new(),
            withheld: Vec::new(),
            budget_u16,
            first_screen_u16,
        }
    }

    /// Add a block in rank order, after any block of the same rank already present. A second
    /// block with an id already present is refused and returns `false`: two paths print the
    /// same text today, and both landing here would count the same withheld items twice.
    #[must_use]
    pub fn push(&mut self, block: Block) -> bool {
        if self.blocks.iter().any(|b| b.id == block.id) {
            return false;
        }
        let at = self.blocks.partition_point(|b| b.rank <= block.rank);
        self.blocks.insert(at, block);
        true
    }

    /// Record content removed by a path outside the trimmer.
    pub fn note_withheld(
        &mut self,
        block: impl Into<String>,
        items: usize,
        reason: Reason,
        command: impl Into<String>,
    ) {
        self.withheld.push(Withheld {
            block: block.into(),
            items,
            reason,
            command: command.into(),
        });
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    pub fn withheld(&self) -> &[Withheld] {
        &self.withheld
    }

    /// Every block at full, in output order: the body of the full-output file. `render`
    /// consumes the emission, so this can only be taken before anything is trimmed.
    pub fn full_text(&self) -> String {
        let mut s = String::new();
        for b in &self.blocks {
            push_line(&mut s, &b.full);
        }
        s
    }

    /// Trim to the budget and compose the final text.
    ///
    /// The header is rendered from the current state on every pass, so the header that is
    /// measured is the header that is printed. Each pass either degrades one block or stops,
    /// and every block can degrade at most twice, so the loop ends.
    pub fn render(mut self, full: &FullOutput, header: Option<Header<'_>>) -> Rendered {
        let full_u16 = u16_len(&self.full_text());
        let (text, prefix) = loop {
            let head = self.header_line(full, header);
            let prefix = u16_len(&self.compose(&head, Some(Rank::DueNow)));
            let text = self.compose(&head, None);
            let total = u16_len(&text);
            // There is no first-screen pass. The header, `Pinned` and `DueNow` are the whole first
            // screen and none of them may degrade, so a first screen they overflow is reported in
            // `first_screen_ok`, never trimmed. `Pinned` is not excluded here: `next_level` already
            // refuses it, and a second guard would leave a mutation of the first one unable to fail
            // any test.
            if total > self.budget_u16 && self.degrade_bottom(|r| r != Rank::DueNow) {
                continue;
            }
            break (text, prefix);
        };
        let emitted_u16 = u16_len(&text);
        Rendered {
            over_budget: emitted_u16 > self.budget_u16,
            first_screen_ok: prefix <= self.first_screen_u16,
            text,
            emitted_u16,
            full_u16,
            budget_u16: self.budget_u16,
            first_screen_u16: self.first_screen_u16,
            withheld: self.withheld,
            blocks: self.blocks,
        }
    }

    fn header_line(&self, full: &FullOutput, header: Option<Header<'_>>) -> String {
        let Some(render) = header else {
            return String::new();
        };
        let facts = Facts {
            blocks: &self.blocks,
            withheld: &self.withheld,
            full,
        };
        render(&facts).replace(['\r', '\n'], " ")
    }

    /// The header plus every block, or only the blocks ranked at or above `upto`.
    fn compose(&self, head: &str, upto: Option<Rank>) -> String {
        let mut s = String::new();
        if self.blocks.is_empty() {
            return s;
        }
        push_line(&mut s, head);
        for b in &self.blocks {
            if upto.is_some_and(|u| b.rank > u) {
                break;
            }
            push_line(&mut s, b.text());
        }
        s
    }

    /// Degrade the bottom-most block whose rank passes `eligible` and write its ledger row.
    /// The degrade and the row are one call, so nothing can be removed without an entry.
    fn degrade_bottom(&mut self, eligible: impl Fn(Rank) -> bool) -> bool {
        let Some(idx) = self
            .blocks
            .iter()
            .rposition(|b| eligible(b.rank) && b.next_level().is_some())
        else {
            return false;
        };
        let b = &mut self.blocks[idx];
        let Some(next) = b.next_level() else {
            return false;
        };
        let before = b.items_shown();
        b.level = next;
        let after = b.items_shown();
        let reason = match next {
            Level::Collapsed => Reason::Collapsed,
            Level::Shortened if after < before => Reason::ListCut,
            Level::Shortened | Level::Full => Reason::TextShortened,
        };
        let row = Withheld {
            block: b.id.clone(),
            items: before.saturating_sub(after),
            reason,
            command: b.command.clone(),
        };
        self.withheld.push(row);
        true
    }
}

/// Append `text` as whole lines. Empty text adds nothing.
fn push_line(s: &mut String, text: &str) {
    let t = text.trim_end_matches(['\r', '\n']);
    if !t.is_empty() {
        s.push_str(t);
        s.push('\n');
    }
}

/// One block's worth of print-site output, collected by [`Fragments`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    /// Unique within one output: the kind, or `kind#2` when a kind comes back after another
    /// block, so [`Emission::push`] never refuses a part and loses its text.
    pub id: String,
    /// What printed it. A block's floor and command are looked up by kind.
    pub kind: String,
    pub text: String,
    /// How many items the text lists.
    pub items: usize,
}

/// The print sites of one hook event, collected in the order they printed, before any of
/// them reaches stdout.
///
/// Session start printed from 32 sites, and the bytes between two blocks were whatever the
/// two sites happened to write. Where one ended without a newline and the next started
/// without one, two blocks shared a line: measured 2026-09-14, the wake contract's header
/// sat on the same line as `</base-context-triggers>`.
///
/// Every site hands its exact fragment here, newlines included. Fragments of one kind in a
/// row are one block, byte for byte. [`Emission`] ends each block with exactly one newline,
/// so between two blocks the collector keeps the rest: with `T` the newlines after the last
/// visible character and `L` the newlines the next fragment starts with, the next block
/// starts with `T + L - 1` of them. A fragment made only of newlines adds to `T`. The first
/// block keeps all `T + L`. Where `T + L` is zero the old bytes had no newline between two
/// blocks and the new output has one: a glue point, a difference the byte-identity ruling
/// allows. The other is the very end, which always carries exactly one newline.
#[derive(Debug, Default)]
pub struct Fragments {
    parts: Vec<Part>,
    carry: usize,
    withheld: Vec<Withheld>,
}

impl Fragments {
    pub fn new() -> Self {
        Self::default()
    }

    /// One print site's exact output. `items` counts what it lists.
    pub fn push(&mut self, kind: &str, fragment: &str, items: usize) {
        let body = fragment.trim_start_matches('\n');
        let lead = fragment.len() - body.len();
        if body.is_empty() {
            self.carry += lead;
            return;
        }
        let content = body.trim_end_matches('\n');
        let gap = self.carry + lead;
        self.carry = body.len() - content.len();
        if let Some(last) = self.parts.last_mut()
            && last.kind == kind
        {
            last.text.push_str(&"\n".repeat(gap));
            last.text.push_str(content);
            last.items += items;
            return;
        }
        let keep = if self.parts.is_empty() {
            gap
        } else {
            gap.saturating_sub(1)
        };
        let earlier = self.parts.iter().filter(|p| p.kind == kind).count();
        let id = if earlier == 0 {
            kind.to_string()
        } else {
            format!("{kind}#{}", earlier + 1)
        };
        self.parts.push(Part {
            id,
            kind: kind.to_string(),
            text: format!("{}{content}", "\n".repeat(keep)),
            items,
        });
    }

    /// Record content removed by a path outside the trimmer, such as a signal skipped because
    /// its output had not changed since it was last shown.
    pub fn note_withheld(&mut self, block: &str, items: usize, reason: Reason, command: &str) {
        self.withheld.push(Withheld {
            block: block.to_string(),
            items,
            reason,
            command: command.to_string(),
        });
    }

    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    pub fn withheld(&self) -> &[Withheld] {
        &self.withheld
    }

    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    /// Newlines after the last visible character. The emission prints exactly one there.
    pub fn trailing_newlines(&self) -> usize {
        self.carry
    }

    pub fn into_parts(self) -> (Vec<Part>, Vec<Withheld>) {
        (self.parts, self.withheld)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blk(id: &str, rank: Rank, full: &str, total: usize) -> Block {
        let floor = format!("{id} {total} · all: base {id} list");
        Block::new(id, rank, full, floor, format!("base {id} list")).items(total, total)
    }

    fn level_of(r: &Rendered, id: &str) -> Level {
        r.blocks
            .iter()
            .find(|b| b.id() == id)
            .map(Block::level)
            .expect("block present")
    }

    fn count_header(f: &Facts<'_>) -> String {
        let full = f.full.written_path().unwrap_or("none");
        format!("[TEST · withheld {} · full: {full}]", f.withheld_total())
    }

    fn short_header(f: &Facts<'_>) -> String {
        format!("[W {}]", f.withheld_total())
    }

    #[test]
    fn utf16_is_not_bytes_and_is_not_chars() {
        // An ASCII input passes under all three units and proves nothing.
        let s = "· — → ⚑";
        assert_eq!(s.len(), 14);
        assert_eq!(u16_len(s), 7);
        let astral = "\u{1F600}";
        assert_eq!(astral.chars().count(), 1);
        assert_eq!(u16_len(astral), 2);
        assert_eq!(astral.len(), 4);
    }

    #[test]
    fn the_budget_is_counted_in_utf16_units_not_bytes() {
        let dots = "·".repeat(600);
        // Control: the input discriminates only because the two units fall either side of 700.
        assert_eq!(u16_len(&dots), 600);
        assert_eq!(dots.len(), 1200);
        let mut e = Emission::new(700, 700);
        assert!(e.push(blk("dots", Rank::Tail, &dots, 1)));
        let r = e.render(&FullOutput::off(), None);
        assert!(!r.text.is_empty());
        assert!(
            r.text.len() > 700,
            "in bytes this output is over the budget"
        );
        assert_eq!(r.emitted_u16, 601);
        assert_eq!(
            level_of(&r, "dots"),
            Level::Full,
            "601 units fit 700; bytes would trim it"
        );
        assert!(r.withheld.is_empty());
    }

    #[test]
    fn pinned_blocks_are_never_degraded() {
        let mut e = Emission::new(40, 40);
        assert!(e.push(blk("instructions", Rank::Pinned, &"I".repeat(30), 0)));
        assert!(e.push(blk("forks", Rank::Secondary, &"F".repeat(100), 157)));
        let r = e.render(&FullOutput::off(), None);
        assert_eq!(level_of(&r, "forks"), Level::Collapsed);
        assert_eq!(
            level_of(&r, "instructions"),
            Level::Full,
            "Pinned never degrades"
        );
        assert!(r.text.starts_with(&"I".repeat(30)));
        assert!(r.over_budget);
    }

    #[test]
    fn due_now_is_not_trimmed_to_meet_the_budget() {
        let mut e = Emission::new(50, 2000);
        assert!(e.push(blk("due", Rank::DueNow, &"D".repeat(60), 2)));
        assert!(e.push(blk("tasks", Rank::Tail, &"T".repeat(60), 115)));
        let r = e.render(&FullOutput::off(), None);
        assert_eq!(level_of(&r, "tasks"), Level::Collapsed);
        assert_eq!(
            level_of(&r, "due"),
            Level::Full,
            "DUE NOW is never trimmed to meet the budget"
        );
        assert!(r.over_budget, "reported, not fixed by cutting what is due");
        assert!(r.first_screen_ok);
    }

    #[test]
    fn due_now_never_collapses_and_a_first_screen_it_overflows_is_reported() {
        // DUE NOW shows in full, never collapsed, always first (auk's cross-lane ruling, lane doc
        // B24). 41 units of instructions and 201 of DUE NOW overflow a 100-unit first screen, and
        // nothing inside that prefix may shrink, so the overflow is reported. The budget pass still
        // takes what comes after: 343 units against 300 collapse `tasks`, and only `tasks`.
        let mut e = Emission::new(300, 100);
        assert!(e.push(blk("instructions", Rank::Pinned, &"I".repeat(40), 0)));
        assert!(e.push(blk("due", Rank::DueNow, &"D".repeat(200), 2)));
        assert!(e.push(blk("tasks", Rank::Tail, &"T".repeat(100), 5)));
        let r = e.render(&FullOutput::off(), None);
        assert_eq!(level_of(&r, "due"), Level::Full, "DUE NOW never collapses");
        assert!(
            !r.first_screen_ok,
            "the overflow is reported, not trimmed away"
        );
        assert_eq!(level_of(&r, "tasks"), Level::Collapsed);
        let rows: Vec<(&str, Reason, usize)> = r
            .withheld
            .iter()
            .map(|w| (w.block.as_str(), w.reason, w.items))
            .collect();
        assert_eq!(
            rows,
            [("tasks", Reason::Collapsed, 5)],
            "the ledger counts only what fell after DUE NOW"
        );
        assert!(!r.over_budget);
        assert_eq!(
            r.text,
            format!(
                "{}\n{}\ntasks 5 · all: base tasks list\n",
                "I".repeat(40),
                "D".repeat(200)
            )
        );
    }

    #[test]
    fn a_block_never_disappears() {
        let mut e = Emission::new(1, 1);
        assert!(e.push(blk("forks", Rank::Secondary, &"F".repeat(500), 157)));
        let floorless =
            Block::new("tasks", Rank::Tail, "T".repeat(500), "", "base task list").items(115, 115);
        assert!(e.push(floorless));
        let r = e.render(&FullOutput::off(), None);
        assert!(r.over_budget);
        assert!(r.text.contains("forks 157 · all: base forks list"));
        assert!(
            r.text.contains("tasks 115 · all: base task list"),
            "an empty floor is replaced, never emitted as nothing"
        );
        assert_eq!(r.text.lines().count(), 2);
    }

    #[test]
    fn every_degrade_writes_exactly_one_ledger_row() {
        let mut e = Emission::new(10, 10);
        let forks =
            blk("forks", Rank::Secondary, &"F".repeat(200), 157).with_shortened("F".repeat(50), 3);
        assert!(e.push(forks));
        assert!(e.push(blk("tasks", Rank::Tail, &"T".repeat(200), 115)));
        let r = e.render(&FullOutput::off(), None);
        let rows: Vec<(&str, Reason, usize)> = r
            .withheld
            .iter()
            .map(|w| (w.block.as_str(), w.reason, w.items))
            .collect();
        assert_eq!(
            rows,
            [
                ("tasks", Reason::Collapsed, 115),
                ("forks", Reason::ListCut, 154),
                ("forks", Reason::Collapsed, 3),
            ]
        );
        assert_eq!(r.withheld_total(), 157 + 115);
    }

    #[test]
    fn tail_is_degraded_before_primary() {
        let mut e = Emission::new(140, 140);
        assert!(e.push(blk("handoffs", Rank::Primary, &"H".repeat(100), 24)));
        assert!(e.push(blk("relay", Rank::Tail, &"R".repeat(100), 1)));
        let r = e.render(&FullOutput::off(), None);
        assert_eq!(level_of(&r, "relay"), Level::Collapsed, "Tail goes first");
        assert_eq!(
            level_of(&r, "handoffs"),
            Level::Full,
            "and the cut stops once it fits"
        );
        assert!(!r.over_budget);
        assert!(r.emitted_u16 <= 140);
    }

    #[test]
    fn within_a_rank_the_bottom_block_goes_first() {
        let mut e = Emission::new(150, 150);
        assert!(e.push(blk("pulse", Rank::Tail, &"P".repeat(100), 1)));
        assert!(e.push(blk("triggers", Rank::Tail, &"G".repeat(100), 9)));
        let r = e.render(&FullOutput::off(), None);
        assert_eq!(level_of(&r, "triggers"), Level::Collapsed);
        assert_eq!(level_of(&r, "pulse"), Level::Full);
    }

    #[test]
    fn output_follows_rank_order_not_push_order() {
        let mut e = Emission::new(10_000, 2000);
        assert!(e.push(blk("relay", Rank::Tail, "RELAY", 1)));
        assert!(e.push(blk("handoffs", Rank::Primary, "HANDOFFS", 24)));
        assert!(e.push(blk("instructions", Rank::Pinned, "DO THIS FIRST", 0)));
        assert!(e.push(blk("due", Rank::DueNow, "DUE NOW", 2)));
        let r = e.render(&FullOutput::off(), None);
        assert_eq!(r.text, "DO THIS FIRST\nDUE NOW\nHANDOFFS\nRELAY\n");
    }

    #[test]
    fn over_budget_is_reported_not_truncated() {
        let mut e = Emission::new(5, 5);
        assert!(e.push(blk("a", Rank::Tail, &"A".repeat(100), 5)));
        let r = e.render(&FullOutput::off(), None);
        assert!(r.over_budget);
        assert_eq!(
            r.text, "a 5 · all: base a list\n",
            "the floor is whole, not cut to 5 units"
        );
        assert_eq!(r.emitted_u16, u16_len(&r.text));
    }

    #[test]
    fn a_second_push_of_the_same_id_is_refused() {
        let mut e = Emission::new(1, 1);
        assert!(e.push(blk(
            "update",
            Rank::Tail,
            "banner from the manifest path",
            1
        )));
        assert!(!e.push(blk("update", Rank::Tail, "banner from the check path", 1)));
        let r = e.render(&FullOutput::off(), None);
        assert_eq!(r.blocks.len(), 1);
        assert_eq!(
            r.withheld.len(),
            1,
            "one block, one row: a duplicate cannot double-count"
        );
        assert!(!r.text.contains("check path"));
    }

    #[test]
    fn the_header_is_line_one_and_carries_the_final_withheld_total() {
        let mut e = Emission::new(120, 2000);
        assert!(e.push(blk("instructions", Rank::Pinned, "DO THIS FIRST", 0)));
        assert!(e.push(blk("forks", Rank::Secondary, &"F".repeat(300), 157)));
        let h: Header<'_> = &count_header;
        let r = e.render(&FullOutput::off(), Some(h));
        let line1 = r.text.lines().next().expect("output is not empty");
        assert_eq!(line1, "[TEST · withheld 157 · full: none]");
        assert!(r.first_screen_ok);
        assert!(!r.over_budget);
    }

    #[test]
    fn the_header_is_measured_as_emitted() {
        // The header widens from "[W 0]" to "[W 1000]" when "a" collapses. Measured before
        // trimming, the output reads 73 units, stops inside the 74-unit budget, and prints 76.
        // Measured as emitted, the trimmer collapses "s" as well and prints 37.
        let mut e = Emission::new(74, 2000);
        assert!(e.push(Block::new(
            "s",
            Rank::Secondary,
            "S".repeat(40),
            "s",
            "base s list"
        )));
        assert!(e.push(blk("a", Rank::Tail, &"A".repeat(100), 1000)));
        let h: Header<'_> = &short_header;
        let r = e.render(&FullOutput::off(), Some(h));
        assert_eq!(r.text.lines().next(), Some("[W 1000]"));
        assert_eq!(r.emitted_u16, u16_len(&r.text));
        assert!(
            r.emitted_u16 <= 74,
            "emitted {} units against a 74-unit budget",
            r.emitted_u16
        );
        assert!(!r.over_budget);
        assert_eq!(level_of(&r, "s"), Level::Collapsed);
    }

    #[test]
    fn the_header_names_the_full_output_path_only_when_it_was_written() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut e = Emission::new(200, 2000);
        assert!(e.push(blk("forks", Rank::Secondary, &"F".repeat(300), 157)));
        let body = e.full_text();
        let path = tmp.path().join(".base").join("last-session-start.md");
        let full = write_full_output(&path, &body);
        let shown = path.display().to_string();
        assert_eq!(full.written_path(), Some(shown.as_str()));
        let h: Header<'_> = &count_header;
        let r = e.render(&full, Some(h));
        let line1 = r.text.lines().next().expect("output is not empty");
        assert!(
            line1.ends_with(&format!("full: {shown}]")),
            "line 1 was: {line1}"
        );
        assert_eq!(level_of(&r, "forks"), Level::Collapsed);
        let on_disk = std::fs::read_to_string(&path).expect("full output written");
        assert_eq!(on_disk, body);
        assert!(
            on_disk.contains(&"F".repeat(300)),
            "the file holds the untrimmed block"
        );

        let blocker = tmp.path().join("not-a-dir");
        std::fs::write(&blocker, "x").expect("blocker file");
        let failed = write_full_output(&blocker.join("last-session-start.md"), &body);
        assert_eq!(failed.written_path(), None);
        assert!(failed.failure().is_some_and(|m| m.contains("not-a-dir")));
    }

    #[test]
    fn with_shortened_and_items_record_the_same_ledger_in_either_order() {
        // petrel's commit A finding A1: the shortened count was clamped when stored, so building
        // it before `items` recorded 0 shown and the ledger claimed 10 withheld for a list of 5.
        let ledger = |b: Block| {
            let mut e = Emission::new(10, 10);
            assert!(e.push(b));
            e.render(&FullOutput::off(), None).withheld
        };
        let floor = "forks 157";
        let items_first = Block::new(
            "forks",
            Rank::Secondary,
            "F".repeat(200),
            floor,
            "base fork list",
        )
        .items(10, 10)
        .with_shortened("F".repeat(50), 5);
        let shortened_first = Block::new(
            "forks",
            Rank::Secondary,
            "F".repeat(200),
            floor,
            "base fork list",
        )
        .with_shortened("F".repeat(50), 5)
        .items(10, 10);
        let first = ledger(items_first);
        let rows: Vec<(Reason, usize)> = first.iter().map(|w| (w.reason, w.items)).collect();
        assert_eq!(rows, [(Reason::ListCut, 5), (Reason::Collapsed, 5)]);
        assert_eq!(
            ledger(shortened_first),
            first,
            "the order of the two builder calls changed the ledger"
        );
    }

    #[test]
    fn a_failed_full_output_write_leaves_no_temp_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // A non-empty directory where the file should go: the temp is written, the rename fails.
        let in_the_way = tmp.path().join("last-session-start.md");
        std::fs::create_dir_all(in_the_way.join("child")).expect("directory in the way");
        let failed = write_full_output(&in_the_way, "body");
        assert!(failed.written_path().is_none());
        assert!(
            failed.failure().is_some(),
            "a rename over a directory reports its failure"
        );
        let left: Vec<String> = std::fs::read_dir(tmp.path())
            .expect("list")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(left.is_empty(), "temp files left behind: {left:?}");
    }

    #[test]
    fn the_full_output_temp_name_is_per_process() {
        let path = Path::new("/ws/.base/last-session-start.md");
        let tmp = temp_path(path).display().to_string();
        assert_eq!(
            tmp,
            format!("/ws/.base/last-session-start.md.{}.tmp", std::process::id())
        );
    }

    /// The emission's text for collected parts: each part one `Tail` block, no header, no trim.
    fn emitted(f: Fragments) -> String {
        let (parts, _) = f.into_parts();
        let mut e = Emission::new(usize::MAX, usize::MAX);
        for p in parts {
            let items = p.items;
            assert!(e.push(Block::new(p.id, Rank::Tail, p.text, "", "cmd").items(items, items)));
        }
        e.render(&FullOutput::off(), None).text
    }

    fn collected(sites: &[(&str, &str)]) -> Fragments {
        let mut f = Fragments::new();
        for (kind, fragment) in sites {
            f.push(kind, fragment, 1);
        }
        f
    }

    #[test]
    fn fragments_keep_every_newline_between_blocks_except_at_a_glue_point() {
        // The old sites' fragments, and what the emission prints. The old bytes are the
        // fragments joined; the only differences allowed are the glue newline and the end.
        let rows: [(&[(&str, &str)], &str); 6] = [
            // W1.4 row 1: the output ends in a newline and the next site starts with one. The
            // blank line survives.
            (
                &[("inject", "X\n"), ("triggers", "\n<t>\n</t>")],
                "X\n\n<t>\n</t>\n",
            ),
            // Row 2: no newline at the end, the next site starts with one. One line break.
            (&[("diag", "<d>"), ("proto", "\nP")], "<d>\nP\n"),
            // Row 3: a newline at the end, the next site starts without one. One line break.
            (&[("a", "A\n"), ("b", "B")], "A\nB\n"),
            // Row 4: neither. The glue point: the one newline the emission adds.
            (&[("triggers", "</t>"), ("wake", "=== W")], "</t>\n=== W\n"),
            // The carry: two trailing newlines and a site of newlines only all reach the next block.
            (&[("a", "A\n\n"), ("sep", "\n"), ("b", "B")], "A\n\n\nB\n"),
            // The first block keeps all of its leading newlines.
            (&[("a", "\n\nA")], "\n\nA\n"),
        ];
        for (sites, want) in rows {
            let old: String = sites.iter().map(|(_, f)| *f).collect();
            assert_eq!(emitted(collected(sites)), want, "old bytes were {old:?}");
        }
    }

    #[test]
    fn a_site_of_newlines_only_makes_no_block() {
        let mut f = Fragments::new();
        f.push("separator", "\n\n", 0);
        assert!(f.is_empty());
        assert_eq!(f.trailing_newlines(), 2);
    }

    #[test]
    fn one_kind_in_a_row_is_one_block_and_a_returning_kind_gets_its_own_id() {
        let mut f = Fragments::new();
        f.push("unhealthy", "L1\n", 1);
        f.push("unhealthy", "L2\n", 1);
        f.push("pulse", "P", 1);
        f.push("unhealthy", "\nL3", 1);
        let ids: Vec<&str> = f.parts().iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["unhealthy", "pulse", "unhealthy#2"]);
        assert_eq!(f.parts()[0].text, "L1\nL2");
        assert_eq!(f.parts()[0].items, 2);
        assert_eq!(
            emitted(f),
            "L1\nL2\nP\nL3\n",
            "nothing refused, nothing lost"
        );
    }
}
