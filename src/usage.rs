//! Usage signals (K8, BO-19; D6, F14c): per rule and per decision, read from the match log (BO-13) and the correction
//! signals in it (BO-15): how often it was served, how often a correction followed, when it was last served. `base
//! doctor` lists what needs attention (dead, noisy and ignored rules, decisions to review, the correction detector's
//! record) and `base rule stats` prints the numbers for every rule.
//!
//! SERVED MEANS PRINTED (D15). A rule or decision counts as served only where a row lists it under `served`: what the
//! hook printed whole. A `cut` entry never counts, nor a BM25 score: the reader does not even build `scores`
//! ([`LeanRow`] has no such field, so serde passes over it).
//!
//! WHY NOISY IS NOT A PRINTED SHARE. A rule is printed once per session (`session.claim_rule`), so the share of
//! prompts it is printed on is capped by sessions over prompts: on the operator's log of 2026-10-03, 61 typed prompts
//! in about 25 sessions, so no rule could reach 25%. "One word pulls in a whole domain" is read where it happens: each
//! prompt row's `matched` list names the domain and the keyword whatever the dedup did afterwards. Only a keyword the
//! domain still has counts, so narrowing a keyword clears the line at once rather than 30 days later. Path matches are
//! not keyword breadth (D1: a session in a project folder matches it on every prompt by design; doctor's trigger
//! faults judge paths). Task notifications are left out: on that same log they were 646 of 707 prompt rows. A rule
//! with matchers of its own is not judged noisy: it too is logged once per session, and its per-prompt topic score
//! sits in `scores`, which this reader does not build.
//!
//! CORRECTED AFTER: the reply the rule was served for, or the next one. Each signal is about one reply. The hooks log
//! C1 (a phrase), the C2 repeat check and a C3 marker (`UPDATED`, `CORRECTED`) on the prompt that answers a reply, so
//! they are about the reply before it (`prompt_num` - 1); they log a C2 interrupt, refusal or edited file on the turn
//! it happened in, so it is about that turn's reply (`corrections::on_prompt`, `on_stop`). A rule served on prompt N
//! counts as corrected after when a signal is about reply N or N + 1. A phrase on the very prompt that brought the
//! rule in is about the reply before the rule was seen, and does not count. `MISREAD` (a misunderstanding) and
//! `DEFERRED` (a disagreement) never count.
//!
//! WHOSE LOG SAYS HOW OLD IT IS. Rows are read from the cwd's tier and the global one, but how many days the log covers
//! is the cwd tier's own log: a workspace first used yesterday is one day old, whatever the global log holds.
//!
//! ADVICE ONLY. Nothing here is one of the conjuncts of `DoctorReport::healthy`: a user who updates must not see doctor
//! go UNHEALTHY because of usage counts (Chris, 2026-10-03).

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset, Local, NaiveDate, Offset};
use oxigraph::store::Store;
use serde::{Deserialize, Serialize};

use crate::config::BaseConfig;
use crate::crud;
use crate::domain::global_decisions::GlobalDecisions;
use crate::domain::DomainDef;
use crate::emit::match_log;

/// Typed prompts the window must hold before noisy is judged (lynx's G0 ruling, question 2): a new user's first three
/// prompts must not make a domain 67% noisy.
pub const NOISY_MIN_PROMPTS: usize = 100;

/// How many times the log's average share of servings followed by a correction a rule's own share must reach before
/// it is listed as ignored (lynx's ruling at gate 4). On the operator's replayed log 1,243 of 7,536 servings (16.5%)
/// were followed by a correction about that reply or the next, so a rule served 20 times passes 3 corrections by
/// chance: with `[doctor] ignored_after` alone, 46 of the 52 rules ever served were listed; with this, 1.
pub const IGNORED_TIMES_AVERAGE: f64 = 2.0;

/// Lines each list prints before it says how many more there are.
const SHOWN: usize = 10;

/// A rule, by its id (`rules::rule_id`), or a decision, by its slug.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Key {
    Rule(String),
    Decision(String),
}

// ─── Reading the log ─────────────────────────────────────────────────────────

/// How the log is read. Each file is read in blocks of whole lines; a block is cut into one piece per thread, the
/// pieces are parsed at once, and their rows are then applied in file order on one thread, so the counts are the same
/// at any thread count (`usage_scan_is_the_same_at_any_thread_count`). No more than one block of one file is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    pub threads: usize,
    /// Bytes read at a time; a block runs on to the end of its last line.
    pub block: usize,
    /// The fewest bytes a thread is given: a small block is read on fewer threads, a small file on the calling one.
    pub piece: usize,
}

impl Reading {
    /// 32 MB: about two days of a heavy user's log (the operator's live log wrote about 17 MB a day on 2026-10-03).
    pub const BLOCK: usize = 32 << 20;
    /// The most threads used, whatever the machine has (lynx's ruling at gate 4).
    pub const MAX_THREADS: usize = 8;
    /// 1 MB: below it a thread costs more to start than the lines it would parse.
    pub const PIECE: usize = 1 << 20;
    /// Set to a number, the threads to use instead: for timing (gate 4 ran it at 2) and tests.
    pub const THREADS_ENV: &str = "BASE_USAGE_THREADS";

    /// [`Reading::THREADS_ENV`] when it holds a number from 1 to 64, else the machine's parallelism, at most
    /// [`Reading::MAX_THREADS`].
    pub fn from_env() -> Self {
        let set = std::env::var(Self::THREADS_ENV).ok().and_then(|v| v.trim().parse::<usize>().ok()).filter(|n| (1..=64).contains(n));
        let threads =
            set.unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()).min(Self::MAX_THREADS));
        Reading { threads, block: Self::BLOCK, piece: Self::PIECE }
    }
}

/// A row as this reader needs it, its plain strings borrowed from the block where the JSON holds no escape. No
/// `scores` field: serde passes over a prompt row's BM25 scores (a median of 76 per row since BO-18) without building
/// them.
#[derive(Deserialize)]
struct LeanRow<'a> {
    #[serde(borrow)]
    ts: Cow<'a, str>,
    #[serde(default, borrow, deserialize_with = "opt_str")]
    session: Option<Cow<'a, str>>,
    #[serde(borrow)]
    event: Cow<'a, str>,
    #[serde(default)]
    prompt_num: Option<u32>,
    #[serde(default, borrow, deserialize_with = "opt_str")]
    text: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    matched: Vec<LeanMatched<'a>>,
    #[serde(default, borrow)]
    served: Vec<LeanItem<'a>>,
    #[serde(default, borrow)]
    cut: Vec<LeanItem<'a>>,
    #[serde(default, borrow)]
    signals: Vec<LeanSignal<'a>>,
}

#[derive(Deserialize)]
struct LeanMatched<'a> {
    #[serde(borrow)]
    domain: Cow<'a, str>,
    #[serde(borrow)]
    by: Cow<'a, str>,
    #[serde(default, borrow, deserialize_with = "opt_str")]
    value: Option<Cow<'a, str>>,
}

/// A served item, or a cut one (its `reason` then set).
#[derive(Deserialize)]
struct LeanItem<'a> {
    #[serde(borrow)]
    id: Cow<'a, str>,
    #[serde(borrow)]
    kind: Cow<'a, str>,
    /// The prompt block that carried it, as `base hooks show` names it.
    #[serde(default, borrow, deserialize_with = "opt_str")]
    block: Option<Cow<'a, str>>,
    #[serde(default, borrow, deserialize_with = "opt_str")]
    reason: Option<Cow<'a, str>>,
}

#[derive(Deserialize)]
struct LeanSignal<'a> {
    #[serde(borrow)]
    layer: Cow<'a, str>,
    #[serde(borrow)]
    kind: Cow<'a, str>,
}

/// An optional string, borrowed from the block where its JSON holds no escape (serde's `Option<Cow<str>>` always
/// copies). `null` is `None`, as a missing field is.
fn opt_str<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Cow<'de, str>>, D::Error> {
    #[derive(Deserialize)]
    struct Borrowed<'a>(#[serde(borrow)] Cow<'a, str>);
    Ok(Option::<Borrowed>::deserialize(d)?.map(|b| b.0))
}

/// The reply a signal logged on prompt `n` says was wrong, when it says one was: C1, the C2 repeat and a C3 `UPDATED`
/// or `CORRECTED` answer the reply before their prompt; a C2 interrupt, refusal or edited file is about its own turn.
fn about(s: &LeanSignal, n: u32) -> Option<u32> {
    match (&*s.layer, &*s.kind) {
        ("C1", _) | ("C2", "repeat") | ("C3", "UPDATED" | "CORRECTED") => n.checked_sub(1),
        ("C2", _) => Some(n),
        _ => None,
    }
}

/// Every rule and decision a row named, by kind and then id, so a row's item is found from its borrowed id without
/// building a [`Key`].
#[derive(Debug, Default, PartialEq)]
struct Items {
    rules: HashMap<String, ItemLog>,
    decisions: HashMap<String, ItemLog>,
}

/// What the log says of one rule or decision.
#[derive(Debug, Default, PartialEq)]
struct ItemLog {
    servings: Vec<Serving>,
    /// Cut for the budget in the window: it was due and not printed.
    withheld: usize,
    /// The block it was last withheld from.
    withheld_from: Option<String>,
    /// The oldest day a row named it, served or cut.
    first_named: Option<NaiveDate>,
}

impl Items {
    fn get(&self, key: &Key) -> Option<&ItemLog> {
        match key {
            Key::Rule(id) => self.rules.get(id),
            Key::Decision(slug) => self.decisions.get(slug),
        }
    }

    /// The log of a logged item of kind `rule` or `decision`, made the first time it is named; `None` for any other
    /// kind (a bracket rule, a check).
    fn named(&mut self, kind: &str, id: &str) -> Option<&mut ItemLog> {
        let map = match kind {
            "rule" => &mut self.rules,
            "decision" => &mut self.decisions,
            _ => return None,
        };
        if !map.contains_key(id) {
            map.insert(id.to_string(), ItemLog::default());
        }
        map.get_mut(id)
    }
}

impl ItemLog {
    fn named_on(&mut self, day: NaiveDate) {
        self.first_named = Some(self.first_named.map_or(day, |d| d.min(day)));
    }
}

/// One time an item was served.
#[derive(Debug, Clone, PartialEq)]
struct Serving {
    day: NaiveDate,
    /// Seconds since the epoch, for a decision whose servings count only after its last update.
    at: i64,
    session: Option<u32>,
    /// The prompt it was served on; for a file row, the session's latest prompt read before it.
    turn: Option<u32>,
}

/// What the match log says, over every file of it in the tiers read.
#[derive(Debug, Default, PartialEq)]
pub struct Scan {
    pub today: NaiveDate,
    pub window_days: u64,
    /// The oldest row's local date in the first tier read (the cwd's own). `None`: it holds no row.
    pub oldest: Option<NaiveDate>,
    /// Prompt rows in the window: typed by a person, a machine's (task notifications and the like), kept with no text.
    pub typed: usize,
    pub machine: usize,
    pub textless: usize,
    /// Every rule and decision a row named, served or cut.
    items: Items,
    /// (session, reply) pairs a correcting signal is about.
    signals: HashSet<(u32, u32)>,
    /// Rows in the window that matched each domain (by slug) by anything but `always`.
    pub domain_matched: HashMap<String, usize>,
    /// Typed prompts in the window that matched each domain (by slug) by a keyword it still has.
    pub keyword_prompts: HashMap<String, usize>,
    /// The same, per keyword.
    pub keyword_hits: HashMap<String, BTreeMap<String, usize>>,
    /// Typed prompts in the window, star commands aside, whose text holds one of a global decision's keywords.
    pub decision_reach: HashMap<String, usize>,
    /// The same, per keyword.
    pub decision_hits: HashMap<String, BTreeMap<String, usize>>,
}

/// What a scan compares the log against: the configuration as it is now.
#[derive(Debug, Default, Clone)]
pub struct Current {
    /// Each domain's prompt keywords, by the domain's slug.
    pub keywords: HashMap<String, Vec<String>>,
    /// Old names (a renamed domain's alias) to the slug the domain has now.
    pub aliases: HashMap<String, String>,
    /// The global decisions with keywords: slug and keywords.
    pub decisions: Vec<(String, Vec<String>)>,
}

impl Current {
    /// The slug a logged domain name has now.
    fn slug(&self, logged: &str) -> String {
        let s = crud::slugify(logged);
        self.aliases.get(&s).cloned().unwrap_or(s)
    }

    /// Read from the domains and the global decisions, as the prompt hook would.
    pub fn from(domains: &[DomainDef], global: Option<&GlobalDecisions>) -> Self {
        let mut out = Current::default();
        for d in domains {
            let slug = crud::slugify(&d.name);
            for a in &d.aliases {
                out.aliases.insert(crud::slugify(a), slug.clone());
            }
            out.keywords.insert(slug, d.prompt_keywords.clone());
        }
        if let Some(global) = global {
            out.decisions =
                global.all().filter(|d| !d.keywords.is_empty()).map(|d| (d.slug.clone(), d.keywords.clone())).collect();
        }
        out
    }
}

impl Scan {
    /// The first day of the window.
    pub fn window_start(&self) -> NaiveDate {
        window_start(self.today, self.window_days)
    }

    fn in_window(&self, day: NaiveDate) -> bool {
        day >= self.window_start()
    }

    /// Days the cwd tier's log covers: today minus its oldest row's day, plus one. 0 with no row.
    pub fn days_covered(&self) -> u64 {
        self.oldest.map(|o| (self.today - o).num_days().max(0) as u64 + 1).unwrap_or(0)
    }

    /// How much the log covers, as the section's first line and `rule stats` say it. `None`: no row yet.
    pub fn log_span(&self) -> Option<LogSpan> {
        self.oldest.map(|o| LogSpan {
            days: self.days_covered(),
            since: o.format("%Y-%m-%d").to_string(),
            typed: self.typed,
            machine: self.machine,
            textless: self.textless,
        })
    }

    /// The counts of one item, every serving counted.
    pub fn counts(&self, key: &Key) -> Counts {
        self.counts_since(key, None)
    }

    /// The counts of one item, counting servings at or after `since` (seconds since the epoch) for `served_all` and
    /// `corrected_after`: a decision reworded with `base decision update` starts again.
    pub fn counts_since(&self, key: &Key, since: Option<i64>) -> Counts {
        let log = self.items.get(key);
        let mut c = Counts {
            withheld_window: log.map_or(0, |l| l.withheld),
            withheld_from: log.and_then(|l| l.withheld_from.clone()),
            first_named: log.and_then(|l| l.first_named),
            ..Counts::default()
        };
        for s in log.into_iter().flat_map(|l| &l.servings) {
            if self.in_window(s.day) {
                c.served_window += 1;
            }
            c.last_served = c.last_served.max(Some(s.day));
            if since.is_some_and(|t| s.at < t) {
                continue;
            }
            c.served_all += 1;
            if let (Some(sess), Some(n)) = (s.session, s.turn)
                && (self.signals.contains(&(sess, n)) || self.signals.contains(&(sess, n + 1)))
            {
                c.corrected_after += 1;
            }
        }
        c
    }

    /// Over the whole log, every rule's and decision's servings: how many a correction followed, and how many there
    /// were. Their share is the average an ignored rule is measured against.
    pub fn average(&self) -> (usize, usize) {
        let (mut corrected, mut all) = (0, 0);
        for log in self.items.rules.values().chain(self.items.decisions.values()) {
            for s in &log.servings {
                all += 1;
                if let (Some(sess), Some(n)) = (s.session, s.turn)
                    && (self.signals.contains(&(sess, n)) || self.signals.contains(&(sess, n + 1)))
                {
                    corrected += 1;
                }
            }
        }
        (corrected, all)
    }

    /// Every decision the log shows served, by slug.
    pub fn decisions(&self) -> impl Iterator<Item = &str> {
        self.items.decisions.iter().filter(|(_, l)| !l.servings.is_empty()).map(|(slug, _)| slug.as_str())
    }
}

/// The first day of a window of `days` days ending `today`.
fn window_start(today: NaiveDate, days: u64) -> NaiveDate {
    today.checked_sub_days(chrono::Days::new(days.max(1) - 1)).unwrap_or(NaiveDate::MIN)
}

/// One item's numbers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counts {
    pub served_window: usize,
    pub served_all: usize,
    pub corrected_after: usize,
    pub last_served: Option<NaiveDate>,
    /// Cut for the budget in the window.
    pub withheld_window: usize,
    /// The block it was last withheld from.
    pub withheld_from: Option<String>,
    pub first_named: Option<NaiveDate>,
}

/// Rows' local days, looking the local offset up once per quarter hour of time. On Windows chrono asks the system for
/// the time zone (`GetTimeZoneInformationForYear`) on every conversion to local time. A zone's offset changes only on a
/// quarter hour (every offset is whole quarter hours, and clocks change on a whole or half hour of local time), so the
/// offset at any instant of a quarter hour is the offset of that quarter hour.
#[derive(Default)]
struct Clock {
    quarter: i64,
    offset: Option<FixedOffset>,
}

impl Clock {
    /// The local day of a row's time, and its seconds since the epoch.
    fn when(&mut self, ts: &str) -> Option<(NaiveDate, i64)> {
        let t = DateTime::parse_from_rfc3339(ts).ok()?;
        let at = t.timestamp();
        let quarter = at.div_euclid(900);
        let offset = match self.offset {
            Some(o) if self.quarter == quarter => o,
            _ => {
                let o = t.with_timezone(&Local).offset().fix();
                (self.quarter, self.offset) = (quarter, Some(o));
                o
            }
        };
        Some((t.with_timezone(&offset).date_naive(), at))
    }
}

/// Read every file of the log in `dirs` (each tier's `.base`, the cwd's own first), oldest file first, as of `today`,
/// with the threads [`Reading::from_env`] gives.
pub fn scan(dirs: &[PathBuf], today: NaiveDate, window_days: u64, current: &Current) -> Scan {
    scan_with(dirs, today, window_days, current, Reading::from_env())
}

/// [`scan`], read as `reading` says.
pub fn scan_with(dirs: &[PathBuf], today: NaiveDate, window_days: u64, current: &Current, reading: Reading) -> Scan {
    let mut out = Scan { today, window_days: window_days.max(1), ..Scan::default() };
    let mut walk = Walk::default();
    let mut buf = Vec::new();
    for (i, dir) in dirs.iter().enumerate() {
        let ctx = Ctx { current, start: out.window_start(), own: i == 0 };
        for path in match_log::files(dir) {
            each_block(&path, &mut buf, reading.block, |block| {
                for piece in parse_block(block, &ctx, reading) {
                    if let Some(day) = piece.quiet_oldest {
                        out.oldest = Some(out.oldest.map_or(day, |o| o.min(day)));
                    }
                    out.merge_cuts(piece.cuts);
                    for row in piece.rows {
                        take(&mut out, row, ctx.own, current, &mut walk);
                    }
                }
            });
        }
    }
    out
}

/// Each block of `path`, in order: about `size` bytes, on to the end of a line, so no line is split between two. A
/// line longer than `size` makes a longer block. A read error ends the file after its last whole line, as an
/// unreadable line did before. No file is no block.
fn each_block(path: &Path, buf: &mut Vec<u8>, size: usize, mut each: impl FnMut(&[u8])) {
    let Ok(mut file) = std::fs::File::open(path) else { return };
    // A small file needs no 32 MB block; one that grows while it is read is read on.
    let len = file.metadata().map_or(u64::MAX, |m| m.len());
    let size = size.min(usize::try_from(len.saturating_add(1)).unwrap_or(usize::MAX)).max(1);
    // The start of a line carried over from the last block, at the front of `buf`.
    let mut kept = 0;
    loop {
        // `buf` is zeroed only where it grows, so once per scan, not once per block.
        if buf.len() < kept + size {
            buf.resize(kept + size, 0);
        }
        let (mut filled, end) = (kept, kept + size);
        let (mut ended, mut failed) = (false, false);
        while filled < end {
            match file.read(&mut buf[filled..end]) {
                Ok(0) => {
                    ended = true;
                    break;
                }
                Ok(n) => filled += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => {
                    failed = true;
                    break;
                }
            }
        }
        if ended || failed {
            // At the end the last line counts with or without its line end; after an error only whole lines do, as
            // the bytes after the last line end are a line cut short.
            let upto = if failed { buf[..filled].iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1) } else { filled };
            if upto > 0 {
                each(&buf[..upto]);
            }
            return;
        }
        // Whole lines only; the rest starts the next block. No line end in what was just read: read on.
        let Some(nl) = buf[kept..filled].iter().rposition(|&b| b == b'\n') else {
            kept = filled;
            continue;
        };
        let cut = kept + nl + 1;
        each(&buf[..cut]);
        buf.copy_within(cut..filled, 0);
        kept = filled - cut;
    }
}

/// `block` cut into at most `n` pieces of whole lines, about equal in size, in order.
fn pieces(block: &[u8], n: usize) -> Vec<&[u8]> {
    let mut out = Vec::with_capacity(n);
    let mut from = 0;
    for k in 1..n {
        let target = (block.len() * k / n).max(from);
        let Some(nl) = block[target..].iter().position(|&b| b == b'\n') else { break };
        out.push(&block[from..target + nl + 1]);
        from = target + nl + 1;
    }
    if from < block.len() {
        out.push(&block[from..]);
    }
    out
}

/// What every worker reads a block against.
struct Ctx<'c> {
    current: &'c Current,
    /// The window's first day.
    start: NaiveDate,
    /// The cwd's own tier: its rows say how old the log is.
    own: bool,
}

/// One piece of a block as a worker read it.
#[derive(Default)]
struct Piece<'a> {
    rows: Vec<Parsed<'a>>,
    /// The oldest day of the tool calls that matched, served and cut nothing, in the cwd's own tier.
    quiet_oldest: Option<NaiveDate>,
    /// The piece's cut entries, folded per rule (`true`) or decision and id. A prompt row can log dozens of decisions
    /// withheld by the budget (68,425 cut entries against 2,645 servings in one day of the operator's log), so they
    /// are counted here, on the worker, and the rows applied in order carry none.
    cuts: HashMap<(bool, Cow<'a, str>), CutFold<'a>>,
}

/// One item's cut entries in a piece.
struct CutFold<'a> {
    /// The oldest day of them.
    first: NaiveDate,
    /// How many were for the budget in the window.
    withheld: usize,
    /// The block the last of those named.
    block: Option<Cow<'a, str>>,
}

impl Scan {
    /// A piece's cut entries, merged in piece order so the block an item was last withheld from is the last in the log.
    fn merge_cuts(&mut self, cuts: HashMap<(bool, Cow<'_, str>), CutFold<'_>>) {
        for ((rule, id), fold) in cuts {
            let Some(log) = self.items.named(if rule { "rule" } else { "decision" }, &id) else { continue };
            log.named_on(fold.first);
            log.withheld += fold.withheld;
            if let Some(block) = fold.block
                && log.withheld_from.as_deref() != Some(&*block)
            {
                log.withheld_from = Some(block.into_owned());
            }
        }
    }
}

/// A row and what its worker worked out from it.
struct Parsed<'a> {
    row: LeanRow<'a>,
    day: NaiveDate,
    at: i64,
    prompt: PromptKind,
}

/// A prompt row in the window: whose text it is.
enum PromptKind {
    /// Not a prompt row, or outside the window.
    Not,
    /// Kept with no text (`[log] prompt_text`).
    Textless,
    /// A task notification and the like (`transcript::machine_prompt`).
    Machine,
    /// Typed by a person: each global decision whose keywords the text holds (its index in [`Current::decisions`]),
    /// with those keywords, lowercased.
    Typed(Vec<(usize, Vec<String>)>),
}

/// The pieces of `block`, parsed at once on up to `reading.threads` threads, in order.
fn parse_block<'a>(block: &'a [u8], ctx: &Ctx, reading: Reading) -> Vec<Piece<'a>> {
    let n = reading.threads.min(block.len().div_ceil(reading.piece.max(1))).max(1);
    let parts = pieces(block, n);
    if parts.len() <= 1 {
        return vec![parse_piece(block, ctx)];
    }
    std::thread::scope(|s| {
        // A thread the system will not give is no failure: that piece is read here, in its place.
        let workers: Vec<_> = parts
            .into_iter()
            .map(|p| std::thread::Builder::new().spawn_scoped(s, move || parse_piece(p, ctx)).map_err(|_| p))
            .collect();
        workers
            .into_iter()
            .map(|w| match w {
                Ok(worker) => worker.join().unwrap_or_else(|e| std::panic::resume_unwind(e)),
                Err(p) => parse_piece(p, ctx),
            })
            .collect()
    })
}

/// A tool call that matched, served and cut nothing: most rows of a log. Passed over before any parse.
fn quiet(text: &str) -> bool {
    text.contains("\"event\":\"file\"")
        && text.contains("\"matched\":[]")
        && text.contains("\"served\":[]")
        && text.contains("\"cut\":[]")
}

fn parse_piece<'a>(piece: &'a [u8], ctx: &Ctx) -> Piece<'a> {
    let mut out = Piece::default();
    let mut clock = Clock::default();
    for line in piece.split(|&b| b == b'\n') {
        let Ok(text) = std::str::from_utf8(line) else { continue };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        if quiet(text) {
            if ctx.own && let Some(day) = ts_day(text, &mut clock) {
                out.quiet_oldest = Some(out.quiet_oldest.map_or(day, |o| o.min(day)));
            }
            continue;
        }
        let Ok(mut row) = serde_json::from_str::<LeanRow>(text) else { continue };
        let Some((day, at)) = clock.when(&row.ts) else { continue };
        let window = day >= ctx.start;
        // Taken, not drained: the rows applied in order keep no room for the cut entries.
        for item in std::mem::take(&mut row.cut) {
            let rule = match &*item.kind {
                "rule" => true,
                "decision" => false,
                _ => continue,
            };
            let fold = out.cuts.entry((rule, item.id)).or_insert(CutFold { first: day, withheld: 0, block: None });
            fold.first = fold.first.min(day);
            if window && item.reason.as_deref() == Some("budget") {
                fold.withheld += 1;
                if item.block.is_some() {
                    fold.block = item.block;
                }
            }
        }
        let prompt = if row.event == "prompt" && window {
            match row.text.take() {
                None => PromptKind::Textless,
                Some(t) if crate::domain::transcript::machine_prompt(&t) => PromptKind::Machine,
                Some(t) => PromptKind::Typed(decision_hits(&t, &row, ctx.current)),
            }
        } else {
            row.text = None;
            PromptKind::Not
        };
        out.rows.push(Parsed { row, day, at, prompt });
    }
    out
}

/// The global decisions whose keywords a typed prompt holds, as BO-16's replay judges one: a star command passes every
/// rule by. The prompt is lowercased once, as `global_decisions::keyword_hit` would for each keyword.
fn decision_hits(text: &str, row: &LeanRow, current: &Current) -> Vec<(usize, Vec<String>)> {
    if current.decisions.is_empty() || row.matched.iter().any(|m| m.by == "command") {
        return Vec::new();
    }
    let lower = text.to_lowercase();
    let mut out = Vec::new();
    for (i, (_, kws)) in current.decisions.iter().enumerate() {
        let hit: Vec<String> = kws
            .iter()
            .map(|k| k.trim().to_lowercase())
            .filter(|k| crate::domain::matcher::contains_word(&lower, k))
            .collect();
        if !hit.is_empty() {
            out.push((i, hit));
        }
    }
    out
}

/// A row's day from its `ts` field, without parsing the rest.
fn ts_day(text: &str, clock: &mut Clock) -> Option<NaiveDate> {
    let at = text.find("\"ts\":\"")? + 6;
    let end = text[at..].find('"')? + at;
    clock.when(&text[at..end]).map(|(d, _)| d)
}

/// What applying rows in order carries from one row to the next.
#[derive(Default)]
struct Walk {
    /// Session ids, numbered as first seen.
    sessions: HashMap<String, u32>,
    /// Each session's latest prompt read so far.
    last_prompt: HashMap<u32, u32>,
    /// Logged domain names to their slug now.
    slugs: HashMap<String, String>,
}

impl Walk {
    fn session(&mut self, id: &str) -> u32 {
        if let Some(&n) = self.sessions.get(id) {
            return n;
        }
        let n = self.sessions.len() as u32;
        self.sessions.insert(id.to_string(), n);
        n
    }

    fn slug(&mut self, current: &Current, logged: &str) -> &str {
        if !self.slugs.contains_key(logged) {
            self.slugs.insert(logged.to_string(), current.slug(logged));
        }
        &self.slugs[logged]
    }
}

/// One more for `key`, its String made only the first time.
fn bump(map: &mut HashMap<String, usize>, key: &str) {
    match map.get_mut(key) {
        Some(n) => *n += 1,
        None => {
            map.insert(key.to_string(), 1);
        }
    }
}

fn take(out: &mut Scan, parsed: Parsed, own: bool, current: &Current, walk: &mut Walk) {
    let Parsed { row, day, at, prompt } = parsed;
    if own {
        out.oldest = Some(out.oldest.map_or(day, |o| o.min(day)));
    }
    let session = row.session.as_deref().map(|s| walk.session(s));
    let window = out.in_window(day);

    if row.event == "signal" {
        if let (Some(s), Some(n)) = (session, row.prompt_num) {
            for reply in row.signals.iter().filter_map(|sig| about(sig, n)) {
                out.signals.insert((s, reply));
            }
        }
        return;
    }
    let turn = match &*row.event {
        "prompt" => {
            if let (Some(s), Some(n)) = (session, row.prompt_num) {
                walk.last_prompt.insert(s, n);
            }
            row.prompt_num
        }
        _ => session.and_then(|s| walk.last_prompt.get(&s).copied()),
    };

    for item in &row.served {
        let Some(log) = out.items.named(&item.kind, &item.id) else { continue };
        log.named_on(day);
        log.servings.push(Serving { day, at, session, turn });
    }
    if !window {
        return;
    }
    for m in row.matched.iter().filter(|m| m.by != "always") {
        bump(&mut out.domain_matched, walk.slug(current, &m.domain));
    }
    let hits = match prompt {
        PromptKind::Not => return,
        PromptKind::Textless => {
            out.textless += 1;
            return;
        }
        PromptKind::Machine => {
            out.machine += 1;
            return;
        }
        PromptKind::Typed(hits) => hits,
    };
    out.typed += 1;
    // Keyword breadth: each domain once per prompt, each keyword once per prompt, only keywords the domain still has.
    let mut domains: HashSet<String> = HashSet::new();
    let mut kws: HashSet<(String, String)> = HashSet::new();
    for m in row.matched.iter().filter(|m| m.by == "keyword") {
        let slug = walk.slug(current, &m.domain);
        let Some(kw) = m.value.as_deref().map(str::trim) else { continue };
        let still = current.keywords.get(slug).is_some_and(|list| list.iter().any(|k| k.trim().eq_ignore_ascii_case(kw)));
        if still {
            kws.insert((slug.to_string(), kw.to_lowercase()));
            domains.insert(slug.to_string());
        }
    }
    for d in domains {
        *out.keyword_prompts.entry(d).or_default() += 1;
    }
    for (d, kw) in kws {
        *out.keyword_hits.entry(d).or_default().entry(kw).or_default() += 1;
    }
    for (i, hit) in hits {
        let slug = &current.decisions[i].0;
        bump(&mut out.decision_reach, slug);
        let per = out.decision_hits.entry(slug.clone()).or_default();
        for k in hit {
            *per.entry(k).or_default() += 1;
        }
    }
}

// ─── What the rules and decisions are now ────────────────────────────────────

/// One rule as the hooks would serve it now.
#[derive(Debug, Clone)]
pub struct RuleNow {
    pub id: String,
    pub domain: String,
    pub text: String,
    /// It has matchers of its own (`select` serves it, not its domain's triggers).
    pub own_matchers: bool,
    /// Its domain is always on.
    pub always: bool,
    /// Its domain injects (`auto_inject`): a rule of one that does not is off, not dead.
    pub injects: bool,
}

impl RuleNow {
    /// `<domain>.<first 8 of id>`, as `base rule list` and `base rule test` take it.
    pub fn short(&self) -> String {
        crate::domain::rule_test::short_ref(&self.domain, &self.id)
    }
}

/// Every rule of every domain, once each, in the domains' order.
pub fn rules_now(domains: &[DomainDef], store: Option<&Store>, config: &BaseConfig) -> Vec<RuleNow> {
    let converted: HashSet<String> =
        crate::domain::rules::rules_with_matchers(store, config, domains).into_iter().map(|c| c.rule.id).collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for d in domains {
        for r in crate::domain::rules::rules_for_domain(store, config, d) {
            if !seen.insert(r.id.clone()) {
                continue;
            }
            out.push(RuleNow {
                own_matchers: converted.contains(&r.id),
                id: r.id,
                domain: d.name.clone(),
                text: r.text,
                always: d.is_always(),
                injects: d.auto_inject,
            });
        }
    }
    out
}

/// A decision's name and dates, read from the store.
#[derive(Debug, Clone, Default)]
pub struct DecisionNow {
    pub name: String,
    /// Its `updatedAt`, else its `createdAt`.
    pub changed: Option<DateTime<chrono::FixedOffset>>,
    pub updated: Option<DateTime<chrono::FixedOffset>>,
    pub superseded: bool,
}

/// The decisions among `slugs` the store holds, by slug.
pub fn decisions_now(store: &Store, config: &BaseConfig, slugs: &[&str]) -> HashMap<String, DecisionNow> {
    let mut out: HashMap<String, DecisionNow> = HashMap::new();
    if slugs.is_empty() {
        return out;
    }
    let ns = &config.namespace;
    let p = &ns.prefix;
    let values: String = slugs.iter().map(|s| format!("<{}> ", crud::build_iri(ns, "decision", s))).collect();
    let q = format!(
        "{}\nSELECT ?d ?name ?created ?updated ?status ?by WHERE {{\n\
           VALUES ?d {{ {values}}}\n\
           GRAPH ?g {{ ?d a {p}:Decision }}\n\
           OPTIONAL {{ GRAPH ?g1 {{ ?d {p}:name ?name }} }}\n\
           OPTIONAL {{ GRAPH ?g2 {{ ?d {p}:createdAt ?created }} }}\n\
           OPTIONAL {{ GRAPH ?g3 {{ ?d {p}:updatedAt ?updated }} }}\n\
           OPTIONAL {{ GRAPH ?g4 {{ ?d {p}:status ?status }} }}\n\
           OPTIONAL {{ GRAPH ?g5 {{ ?d {p}:{} ?by }} }}\n\
         }}",
        crud::prefixes(ns),
        crate::supersede::PRED_SUPERSEDED_BY,
    );
    let Ok(oxigraph::sparql::QueryResults::Solutions(rows)) = crate::store::query(store, &q) else { return out };
    let date = |s: Option<String>| s.and_then(|v| DateTime::parse_from_rfc3339(v.trim()).ok());
    for row in rows.filter_map(Result::ok) {
        let lit = |k: &str| row.get(k).map(|t| crud::term_display(t.into()));
        let Some(id) = lit("d").map(|d| crud::slug_of(&d)) else { continue };
        let e = out.entry(id).or_default();
        if e.name.is_empty() {
            e.name = lit("name").unwrap_or_default();
        }
        let created = date(lit("created"));
        let updated = date(lit("updated"));
        e.updated = e.updated.max(updated);
        e.changed = e.changed.max(updated.or(created)).max(e.updated);
        e.superseded |= lit("by").is_some() || lit("status").as_deref() == Some(crate::supersede::STATUS_SUPERSEDED);
    }
    out
}

// ─── The doctor section ──────────────────────────────────────────────────────

/// How much the log covers, as the section's first line says it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LogSpan {
    pub days: u64,
    pub since: String,
    pub typed: usize,
    pub machine: usize,
    pub textless: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Dead {
    /// An injecting domain that matched nothing in the window: every rule of it went unserved.
    Domain { domain: String, rules: usize },
    Rule {
        rule: String,
        domain: String,
        text: String,
        last_served: Option<String>,
        /// Times cut for the budget in the window, and the block it was last cut from.
        withheld: usize,
        block: Option<String>,
        own_matchers: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Noisy {
    Domain { domain: String, prompts: usize, of: usize, keywords: Vec<(String, usize)>, rules: usize },
    Decision { decision: String, name: String, prompts: usize, of: usize, keywords: Vec<(String, usize)> },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Ignored {
    /// `rule` or `decision`.
    pub kind: String,
    /// The rule's `<domain>.<id>` or the decision's slug.
    pub id: String,
    pub text: String,
    pub served: usize,
    pub corrected: usize,
}

/// The log's average: servings a correction followed, of all servings, over the whole log.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Average {
    pub corrected: usize,
    pub servings: usize,
}

impl Average {
    pub fn share(&self) -> f64 {
        if self.servings == 0 { 0.0 } else { self.corrected as f64 / self.servings as f64 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Review {
    pub decision: String,
    pub name: String,
    pub served: usize,
    /// Days since its last update or its creation; `None` when it carries neither date.
    pub unchanged_days: Option<i64>,
}

/// The correction detector's record over the window (BO-17's backstop, K8c).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DetectorLine {
    pub judged: usize,
    pub corrections: usize,
    pub misses: usize,
}

/// The thresholds the section was judged at, so the text can name them.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Limits {
    pub dead_days: u64,
    pub broad_share: f32,
    pub ignored_after: usize,
    pub review_served: usize,
    pub review_days: i64,
    pub noisy_min_prompts: usize,
    /// `[log] prompt_text` when it is not `full`: then a row keeps too little text to tell a person's prompt from a
    /// task notification, and noisy is not judged.
    pub prompt_text: Option<String>,
}

impl Limits {
    pub fn from_config(config: &BaseConfig) -> Self {
        let dc = &config.doctor;
        let mode = crate::emit::match_log::PromptText::parse(&config.log.prompt_text);
        Limits {
            dead_days: dc.dead_days.max(1),
            broad_share: config.tune.broad_share,
            ignored_after: dc.ignored_after.max(1),
            review_served: dc.review_served.max(1),
            review_days: dc.review_days,
            noisy_min_prompts: NOISY_MIN_PROMPTS,
            prompt_text: (mode != crate::emit::match_log::PromptText::Full).then(|| config.log.prompt_text.trim().to_string()),
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Limits::from_config(&BaseConfig::default())
    }
}

/// `base doctor`'s rules-and-decisions section. Advice only: never read by `DoctorReport::healthy`. The default is the
/// section of a store with no match log yet.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Section {
    /// `None`: no match log yet.
    pub log: Option<LogSpan>,
    /// `None`: not judged, the log covers fewer days than `dead_days`.
    pub dead: Option<Vec<Dead>>,
    /// Rules not served and never named in the log: they did not exist when their domain last matched.
    pub too_new: usize,
    /// `None`: not judged (too few typed prompts in the window, or no prompt text kept).
    pub noisy: Option<Vec<Noisy>>,
    pub ignored: Vec<Ignored>,
    /// What an ignored rule's share is measured against.
    pub average: Average,
    pub review: Vec<Review>,
    pub detector: DetectorLine,
    pub limits: Limits,
}

/// What a section is built from: the log scanned against the configuration as it is now.
pub struct Inputs<'a> {
    pub config: &'a BaseConfig,
    pub domains: &'a [DomainDef],
    pub store: Option<&'a Store>,
    pub global: Option<&'a GlobalDecisions>,
    pub scan: &'a Scan,
    pub detector: crate::corrections::tune_pass::Detector,
    pub now: DateTime<Local>,
}

/// The tiers whose logs a cwd's sessions write to: its own first, then the global one.
pub fn log_dirs(cwd: &Path) -> Vec<PathBuf> {
    crate::corrections::propose::row_dirs(cwd)
}

/// Scan `cwd`'s logs and build the section. Read only: the store is the caller's, loaded with `store::load_merged`.
pub fn section_for(cwd: &Path, config: &BaseConfig, domains: &[DomainDef], store: Option<&Store>) -> Section {
    let now = Local::now();
    let global = store.map(|s| GlobalDecisions::load(s, config, domains));
    let current = Current::from(domains, global.as_ref());
    let scan = scan(&log_dirs(cwd), now.date_naive(), config.doctor.dead_days, &current);
    let detector = crate::corrections::tune_pass::detector_totals(Some(scan.window_start()));
    build(&Inputs { config, domains, store, global: global.as_ref(), scan: &scan, detector, now })
}

pub fn build(i: &Inputs) -> Section {
    let limits = Limits::from_config(i.config);
    let scan = i.scan;
    let detector = DetectorLine { judged: i.detector.judged, corrections: i.detector.corrections, misses: i.detector.misses };
    let Some(log) = scan.log_span() else {
        return Section { detector, limits, ..Section::default() };
    };
    let rules = rules_now(i.domains, i.store, i.config);
    let start = scan.window_start();

    // Dead (rules only): judged once the log covers `dead_days`.
    let (dead, too_new) = if log.days < limits.dead_days {
        (None, 0)
    } else {
        let mut by_domain: Vec<(String, usize)> = Vec::new();
        let mut lines: Vec<Dead> = Vec::new();
        let mut too_new = 0;
        for r in rules.iter().filter(|r| r.injects) {
            let c = scan.counts(&Key::Rule(r.id.clone()));
            if c.served_window > 0 {
                continue;
            }
            let reached = r.always || r.own_matchers || scan.domain_matched.get(&crud::slugify(&r.domain)).is_some_and(|n| *n > 0);
            if !reached {
                match by_domain.iter_mut().find(|(d, _)| *d == r.domain) {
                    Some((_, n)) => *n += 1,
                    None => by_domain.push((r.domain.clone(), 1)),
                }
                continue;
            }
            let named_before = c.first_named.is_some_and(|d| d < start);
            if r.own_matchers || c.withheld_window > 0 || named_before {
                lines.push(Dead::Rule {
                    rule: r.short(),
                    domain: r.domain.clone(),
                    text: r.text.clone(),
                    last_served: c.last_served.map(|d| d.format("%Y-%m-%d").to_string()),
                    withheld: c.withheld_window,
                    block: c.withheld_from.clone(),
                    own_matchers: r.own_matchers,
                });
            } else {
                too_new += 1;
            }
        }
        let mut dead: Vec<Dead> = by_domain.into_iter().map(|(domain, rules)| Dead::Domain { domain, rules }).collect();
        dead.extend(lines);
        (Some(dead), too_new)
    };

    // Noisy: judged once the window holds enough typed prompts, and only when rows keep the prompt's text.
    let of = scan.typed;
    let over = |n: usize| of > 0 && n as f32 / of as f32 > limits.broad_share;
    let sorted = |m: Option<&BTreeMap<String, usize>>| -> Vec<(String, usize)> {
        let mut v: Vec<(String, usize)> = m.map(|m| m.iter().map(|(k, v)| (k.clone(), *v)).collect()).unwrap_or_default();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    };
    let noisy = (of >= NOISY_MIN_PROMPTS && limits.prompt_text.is_none()).then(|| {
        let mut out: Vec<Noisy> = Vec::new();
        for d in i.domains.iter().filter(|d| d.auto_inject && !d.is_always()) {
            let slug = crud::slugify(&d.name);
            let n = scan.keyword_prompts.get(&slug).copied().unwrap_or(0);
            if !over(n) {
                continue;
            }
            let count = rules.iter().filter(|r| crud::slugify(&r.domain) == slug && !r.own_matchers).count();
            out.push(Noisy::Domain { domain: d.name.clone(), prompts: n, of, keywords: sorted(scan.keyword_hits.get(&slug)), rules: count });
        }
        for (slug, n) in &scan.decision_reach {
            if over(*n) {
                let name = i.global.and_then(|g| g.by_slug(slug)).map(|d| d.name.clone()).unwrap_or_default();
                out.push(Noisy::Decision { decision: slug.clone(), name, prompts: *n, of, keywords: sorted(scan.decision_hits.get(slug)) });
            }
        }
        out.sort_by(|a, b| noisy_n(b).cmp(&noisy_n(a)).then_with(|| noisy_name(a).cmp(noisy_name(b))));
        out
    });

    // Decisions: their names and dates, for the ignored and review lists.
    let decision_slugs: Vec<&str> = {
        let mut v: Vec<&str> = scan.decisions().collect();
        v.sort();
        v
    };
    let now_decisions = i.store.map(|s| decisions_now(s, i.config, &decision_slugs)).unwrap_or_default();

    // Ignored: rules and decisions, over the whole log: corrected after at least `ignored_after` times, and after at
    // least IGNORED_TIMES_AVERAGE times the log's average share of servings (lynx's ruling at gate 4).
    let (corrected, servings) = scan.average();
    let average = Average { corrected, servings };
    let floor = IGNORED_TIMES_AVERAGE * average.share();
    let ignored_by = |c: &Counts| {
        c.corrected_after >= limits.ignored_after && c.served_all > 0 && c.corrected_after as f64 >= floor * c.served_all as f64
    };
    let mut ignored: Vec<Ignored> = Vec::new();
    for r in &rules {
        let c = scan.counts(&Key::Rule(r.id.clone()));
        if ignored_by(&c) {
            ignored.push(Ignored { kind: "rule".into(), id: r.short(), text: r.text.clone(), served: c.served_all, corrected: c.corrected_after });
        }
    }
    for slug in &decision_slugs {
        let Some(d) = now_decisions.get(*slug).filter(|d| !d.superseded) else { continue };
        let c = scan.counts_since(&Key::Decision(slug.to_string()), d.updated.map(|t| t.timestamp()));
        if ignored_by(&c) {
            ignored.push(Ignored { kind: "decision".into(), id: slug.to_string(), text: d.name.clone(), served: c.served_all, corrected: c.corrected_after });
        }
    }
    ignored.sort_by(|a, b| b.corrected.cmp(&a.corrected).then_with(|| a.id.cmp(&b.id)));

    // Review (F14c): decisions served often and not changed in a long time.
    let mut review: Vec<Review> = Vec::new();
    for slug in &decision_slugs {
        let Some(d) = now_decisions.get(*slug).filter(|d| !d.superseded) else { continue };
        let c = scan.counts(&Key::Decision(slug.to_string()));
        if c.served_all < limits.review_served {
            continue;
        }
        let unchanged = d.changed.map(|t| (i.now.date_naive() - t.with_timezone(&Local).date_naive()).num_days());
        if unchanged.is_none_or(|days| days >= limits.review_days) {
            review.push(Review { decision: slug.to_string(), name: d.name.clone(), served: c.served_all, unchanged_days: unchanged });
        }
    }
    review.sort_by(|a, b| b.served.cmp(&a.served).then_with(|| a.decision.cmp(&b.decision)));

    Section { log: Some(log), dead, too_new, noisy, ignored, average, review, detector, limits }
}

fn noisy_n(n: &Noisy) -> usize {
    match n {
        Noisy::Domain { prompts, .. } | Noisy::Decision { prompts, .. } => *prompts,
    }
}

fn noisy_name(n: &Noisy) -> &str {
    match n {
        Noisy::Domain { domain, .. } => domain,
        Noisy::Decision { decision, .. } => decision,
    }
}

// ─── The next steps the section prints ───────────────────────────────────────

/// A command-line argument as bash, PowerShell and cmd read it back. Left bare when it holds only characters no shell
/// gives a meaning to; double-quoted when it holds a space or another character a shell would act on bare; single-quoted
/// when it holds one a shell still acts on inside double quotes (`$`, a backtick, `"`, `\`, `!`), a quote inside
/// written `'\''` as bash reads it.
fn arg(s: &str) -> String {
    let bare = |c: char| c.is_alphanumeric() || "._-/:@%+=,".contains(c);
    if !s.is_empty() && s.chars().all(bare) {
        return s.to_string();
    }
    if s.chars().any(|c| "$`\"\\!".contains(c)) {
        return format!("'{}'", s.replace('\'', "'\\''"));
    }
    format!("\"{s}\"")
}

/// What a list past its first lines points to.
pub const CMD_DOCTOR_JSON: &str = "base doctor --json";
/// What runs a rule pass, which the detector line counts.
pub const CMD_TUNE: &str = "base tune";

pub fn cmd_test_domain(domain: &str) -> String {
    format!("base rule test --domain {}", arg(domain))
}

pub fn cmd_test_rule(rule: &str) -> String {
    format!("base rule test --rule {}", arg(rule))
}

pub fn cmd_hooks_show(block: &str) -> String {
    format!("base hooks show {}", arg(block))
}

pub fn cmd_replay_drop(domain: &str, keyword: &str) -> String {
    format!("base rule replay --domain {} --drop-keyword {}", arg(domain), arg(keyword))
}

pub fn cmd_replay_decision_drop(slug: &str, keyword: &str) -> String {
    format!("base rule replay --decision {} --drop-keyword {}", arg(slug), arg(keyword))
}

pub fn cmd_propose_rewrite(rule: &str) -> String {
    format!("base rule propose --rule {} --text \"...\"", arg(rule))
}

pub fn cmd_decision_update(slug: &str) -> String {
    format!("base decision update {} --name \"...\"", arg(slug))
}

pub fn cmd_decision_supersede(slug: &str) -> String {
    let domain = slug.split_once('.').map(|(d, _)| d).unwrap_or("GLOBAL");
    format!("base decision log --domain {} --decision \"...\" --rationale \"...\" --supersedes {}", arg(domain), arg(slug))
}

/// One of each next step the section and `base rule stats` print, built by the functions above on invented names, a
/// keyword with a space and ones a shell would act on among them: the CLI's own parser reads each in the binary's tests
/// (lynx's G0 condition).
pub fn next_step_examples() -> Vec<String> {
    vec![
        cmd_test_domain("tools"),
        cmd_test_domain("two words"),
        cmd_test_rule("tools.1a2b3c4d"),
        cmd_hooks_show("tools-rules"),
        cmd_replay_drop("tools", "hook"),
        cmd_replay_drop("tools", "user prompt submit"),
        cmd_replay_drop("tools", "a&b|*.rs"),
        cmd_replay_drop("tools", "$HOME it's"),
        cmd_replay_decision_drop("global.keep-notes-short", "notes"),
        cmd_propose_rewrite("tools.1a2b3c4d"),
        cmd_decision_update("global.keep-notes-short"),
        cmd_decision_supersede("global.keep-notes-short"),
        CMD_DOCTOR_JSON.to_string(),
        CMD_TUNE.to_string(),
    ]
}

// ─── Rendering ───────────────────────────────────────────────────────────────

/// `1,257`.
fn thousands(n: usize) -> String {
    crate::emit::prompt::thousands(n)
}

fn plural<'s>(n: usize, one: &'s str, many: &'s str) -> &'s str {
    if n == 1 { one } else { many }
}

/// `text` on one line, cut to `max` characters with `...`.
fn clip(text: &str, max: usize) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        return one;
    }
    let head: String = one.chars().take(max).collect();
    format!("{}...", head.trim_end())
}

fn percent(n: usize, of: usize) -> String {
    if of == 0 { "0%".into() } else { format!("{:.0}%", n as f64 * 100.0 / of as f64) }
}

/// `   +31 more · base doctor --json lists them all`.
fn more_line(out: &mut String, total: usize) {
    if total > SHOWN {
        out.push_str(&format!("     +{} more · {CMD_DOCTOR_JSON} lists them all\n", total - SHOWN));
    }
}

/// The section as `base doctor` prints it.
pub fn render(s: &Section) -> String {
    let l = &s.limits;
    let mut out = String::from("\n─── rules and decisions, from the match log ───\n");
    let Some(log) = &s.log else {
        out.push_str("   no match log yet: base writes it from the next prompt on, and this section fills in as it grows\n");
        return out;
    };
    let days = format!("{} {} (since {})", log.days, plural(log.days as usize, "day", "days"), log.since);
    match &l.prompt_text {
        None => out.push_str(&format!(
            "   match log: {days} · {} typed {}, {} task {} · {} with no text\n",
            thousands(log.typed),
            plural(log.typed, "prompt", "prompts"),
            thousands(log.machine),
            plural(log.machine, "notification", "notifications"),
            thousands(log.textless),
        )),
        Some(mode) => {
            let n = log.typed + log.machine + log.textless;
            out.push_str(&format!("   match log: {days} · {} {} · [log] prompt_text = \"{mode}\"\n", thousands(n), plural(n, "prompt", "prompts")));
        }
    }

    match &s.dead {
        None => out.push_str(&format!(
            "   dead (not served in {d} days): not judged yet · the match log covers {n} {day}, fewer than [doctor] dead_days = {d}\n",
            d = l.dead_days,
            n = log.days,
            day = plural(log.days as usize, "day", "days"),
        )),
        Some(dead) => {
            let rules: usize = dead.iter().map(|d| match d { Dead::Domain { rules, .. } => *rules, Dead::Rule { .. } => 1 }).sum();
            let new = if s.too_new > 0 { format!(" · {} more first seen under {} days ago", s.too_new, l.dead_days) } else { String::new() };
            out.push_str(&format!("   dead (not served in {} days): {rules}{new}\n", l.dead_days));
            for d in dead.iter().take(SHOWN) {
                out.push_str(&dead_line(d, l.dead_days));
            }
            more_line(&mut out, dead.len());
        }
    }

    match (&s.noisy, &l.prompt_text) {
        (None, Some(mode)) => out.push_str(&format!(
            "   noisy: not judged · [log] prompt_text = \"{mode}\" keeps too little of a prompt to tell yours from a task notification\n"
        )),
        (None, None) => out.push_str(&format!(
            "   noisy: not judged yet · {} typed {} in the last {} days, fewer than {}\n",
            thousands(log.typed),
            plural(log.typed, "prompt", "prompts"),
            l.dead_days,
            l.noisy_min_prompts
        )),
        (Some(noisy), _) => {
            out.push_str(&format!(
                "   noisy (matched by keyword on more than {:.0}% of typed prompts, last {} days): {}\n",
                l.broad_share * 100.0,
                l.dead_days,
                noisy.len()
            ));
            for n in noisy.iter().take(SHOWN) {
                out.push_str(&noisy_line(n));
            }
            more_line(&mut out, noisy.len());
        }
    }

    let avg = percent(s.average.corrected, s.average.servings);
    out.push_str(&format!(
        "   ignored (corrected after {}+ times, and after at least twice the log average of {avg} of servings): {}\n",
        l.ignored_after,
        s.ignored.len()
    ));
    for i in s.ignored.iter().take(SHOWN) {
        let next = match i.kind.as_str() {
            "decision" => format!("reword it: {}", cmd_decision_update(&i.id)),
            _ => format!("reword it: {}", cmd_propose_rewrite(&i.id)),
        };
        out.push_str(&format!(
            "     {} \"{}\"   served {} · corrected after {} ({}, log average {avg}) · {next}\n",
            i.id,
            clip(&i.text, 40),
            i.served,
            i.corrected,
            percent(i.corrected, i.served)
        ));
    }
    more_line(&mut out, s.ignored.len());

    out.push_str(&format!("   review (served {}+ times, unchanged {}+ days): {}\n", l.review_served, l.review_days, s.review.len()));
    for r in s.review.iter().take(SHOWN) {
        let age = match r.unchanged_days {
            Some(d) => format!("unchanged {d} {}", plural(d as usize, "day", "days")),
            None => "no date on record".to_string(),
        };
        out.push_str(&format!(
            "     {} \"{}\"   served {} · {age} · still true? {} · or replace it: {}\n",
            r.decision,
            clip(&r.name, 40),
            r.served,
            cmd_decision_update(&r.decision),
            cmd_decision_supersede(&r.decision)
        ));
    }
    more_line(&mut out, s.review.len());

    out.push_str(&detector_line(&s.detector, l.dead_days));
    out
}

fn dead_line(d: &Dead, days: u64) -> String {
    match d {
        Dead::Domain { domain, rules } => format!(
            "     domain {domain} · not matched in {days} days · its {rules} {} not served · check its triggers: {}\n",
            plural(*rules, "rule", "rules"),
            cmd_test_domain(domain)
        ),
        Dead::Rule { rule, domain, text, last_served, withheld, block, own_matchers } => {
            let last = match last_served {
                Some(d) => format!("last {d}"),
                None => "never served in the log".to_string(),
            };
            let next = if *withheld > 0 {
                let block = block.clone().unwrap_or_else(|| format!("{}-rules", crud::slugify(domain)));
                format!("withheld {withheld} {} by the budget: {}", plural(*withheld, "time", "times"), cmd_hooks_show(&block))
            } else if *own_matchers {
                format!("its own matchers did not serve it: {}", cmd_test_rule(rule))
            } else {
                format!("check its triggers: {}", cmd_test_domain(domain))
            };
            format!("     {rule} \"{}\"   0 served · {last} · {next}\n", clip(text, 40))
        }
    }
}

fn noisy_line(n: &Noisy) -> String {
    let by = |keywords: &[(String, usize)], of: usize| -> String {
        keywords.iter().take(3).map(|(k, c)| format!("{k} {}", percent(*c, of))).collect::<Vec<_>>().join(", ")
    };
    match n {
        Noisy::Domain { domain, prompts, of, keywords, rules } => {
            let next = match keywords.first() {
                Some((k, _)) => format!("narrow its keywords: {}", cmd_replay_drop(domain, k)),
                None => format!("check its triggers: {}", cmd_test_domain(domain)),
            };
            format!(
                "     domain {domain} · {} ({} of {}) · by keyword {} · its {rules} {} · {next}\n",
                percent(*prompts, *of),
                thousands(*prompts),
                thousands(*of),
                by(keywords, *of),
                plural(*rules, "rule", "rules")
            )
        }
        Noisy::Decision { decision, name, prompts, of, keywords } => {
            let next = match keywords.first() {
                Some((k, _)) => format!(" · narrow its keywords: {}", cmd_replay_decision_drop(decision, k)),
                None => String::new(),
            };
            format!(
                "     {decision} \"{}\"   {} ({} of {}) · by keyword {}{next}\n",
                clip(name, 40),
                percent(*prompts, *of),
                thousands(*prompts),
                thousands(*of),
                by(keywords, *of)
            )
        }
    }
}

/// K8c: `correction detector, last 30 days: 15 caught · 11 missed (found by the backstop) · 58% caught`.
fn detector_line(d: &DetectorLine, days: u64) -> String {
    if d.judged == 0 {
        return format!("   correction detector: no rule pass in the last {days} days ({CMD_TUNE} runs one)\n");
    }
    if d.corrections == 0 {
        return format!(
            "   correction detector, last {days} days: {} {} judged, no correction found\n",
            thousands(d.judged),
            plural(d.judged, "turn", "turns")
        );
    }
    let caught = d.corrections.saturating_sub(d.misses);
    format!(
        "   correction detector, last {days} days: {caught} caught · {} missed (found by the backstop) · {} caught\n",
        d.misses,
        percent(caught, d.corrections)
    )
}

// ─── base rule stats ─────────────────────────────────────────────────────────

/// One rule's line of `base rule stats`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StatsRow {
    pub rule: String,
    pub domain: String,
    pub id: String,
    pub served_window: usize,
    pub window_days: u64,
    pub served_all: usize,
    pub corrected_after: usize,
    pub last_served: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stats {
    /// `None`: no match log yet.
    pub log: Option<LogSpan>,
    pub window_days: u64,
    pub rules: Vec<StatsRow>,
}

/// `base rule stats`: every rule in `rules` (or only `domain`'s), with its numbers.
pub fn stats(scan: &Scan, rules: &[RuleNow], domain: Option<&str>) -> Stats {
    let want = domain.map(crud::slugify);
    let rows = rules
        .iter()
        .filter(|r| want.as_deref().is_none_or(|w| crud::slugify(&r.domain) == w))
        .map(|r| {
            let c = scan.counts(&Key::Rule(r.id.clone()));
            StatsRow {
                rule: r.short(),
                domain: r.domain.clone(),
                id: r.id.clone(),
                served_window: c.served_window,
                window_days: scan.window_days,
                served_all: c.served_all,
                corrected_after: c.corrected_after,
                last_served: c.last_served.map(|d| d.format("%Y-%m-%d").to_string()),
            }
        })
        .collect();
    Stats { log: scan.log_span(), window_days: scan.window_days, rules: rows }
}

/// Example 2's table, with one line above it saying what the log covers.
pub fn render_stats(s: &Stats) -> String {
    let mut out = match &s.log {
        Some(l) => format!("from the match log: {} {} (since {})\n", l.days, plural(l.days as usize, "day", "days"), l.since),
        None => "no match log yet: base writes it from the next prompt on\n".to_string(),
    };
    if s.rules.is_empty() {
        out.push_str("no rules\n");
        return out;
    }
    let window = format!("served {}d", s.window_days);
    let head = ["rule", window.as_str(), "served all", "corrected after", "last served"];
    let cells: Vec<[String; 5]> = s
        .rules
        .iter()
        .map(|r| {
            [
                r.rule.clone(),
                r.served_window.to_string(),
                r.served_all.to_string(),
                r.corrected_after.to_string(),
                r.last_served.clone().unwrap_or_else(|| "-".into()),
            ]
        })
        .collect();
    let mut width = head.map(|h| h.chars().count());
    for row in &cells {
        for (w, c) in width.iter_mut().zip(row) {
            *w = (*w).max(c.chars().count());
        }
    }
    let line = |row: [&str; 5]| -> String {
        let parts: Vec<String> = row.iter().zip(width).map(|(c, w)| format!(" {c:<w$} ")).collect();
        format!("|{}|\n", parts.join("|"))
    };
    out.push_str(&line(head));
    for row in &cells {
        out.push_str(&line([&row[0], &row[1], &row[2], &row[3], &row[4]]));
    }
    out
}

/// `base rule stats` from `cwd`: the rules as the hooks read them, the logs of its tiers.
pub fn stats_for(cwd: &Path, config: &BaseConfig, domain: Option<&str>) -> Stats {
    let domains = crate::domain::load_domains(cwd);
    let store = crate::store::load_merged(cwd);
    let current = Current::from(&domains, None);
    let scan = scan(&log_dirs(cwd), Local::now().date_naive(), config.doctor.dead_days, &current);
    let rules = rules_now(&domains, store.as_ref(), config);
    stats(&scan, &rules, domain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_are_quoted_only_when_a_shell_needs_it() {
        assert_eq!(arg("tools"), "tools");
        assert_eq!(arg("tools.1a2b-c_d/e:f"), "tools.1a2b-c_d/e:f");
        assert_eq!(arg("user prompt submit"), "\"user prompt submit\"");
        assert_eq!(arg("a&b|*.rs"), "\"a&b|*.rs\"");
        assert_eq!(arg("$HOME it's"), "'$HOME it'\\''s'");
        assert_eq!(cmd_replay_drop("tools", "a b"), "base rule replay --domain tools --drop-keyword \"a b\"");
    }

    #[test]
    fn a_young_window_starts_today_and_counts_days_inclusive() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 3).unwrap();
        let s = Scan { today, window_days: 30, oldest: Some(today), ..Scan::default() };
        assert_eq!(s.days_covered(), 1);
        assert_eq!(s.window_start(), NaiveDate::from_ymd_opt(2026, 9, 4).unwrap());
        let none = Scan { today, window_days: 30, ..Scan::default() };
        assert_eq!(none.days_covered(), 0);
        let huge = Scan { today, window_days: u64::MAX, ..Scan::default() };
        assert_eq!(huge.window_start(), NaiveDate::MIN, "a window past the calendar's start is the whole log, not a panic");
    }
}
