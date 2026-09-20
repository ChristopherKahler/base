//! What a hook emitted, kept per run so `base doctor` can say it (spec A7).
//!
//! One JSON line per measured emission, in `<tier>/.base/hook-output.jsonl` beside the full-output file. The trail
//! every hook run already writes, `hook-events.jsonl`, cannot hold this: its writer keeps the last 5,000 LINES and
//! session starts are about 0.5% of them. Measured 2026-09-15 on a live global tier: 23 session starts in 8,947 lines,
//! none in the last 5,000. A JSON ring buffer would keep them, but every session start would rewrite the whole file,
//! and two starting together would erase one record without an error.
//!
//! So this file is append-only. A record is serialised first and written in ONE `write_all` under `O_APPEND`, so two
//! writers never interleave inside a line, and the bound is a rename, never a rewrite: at [`CAP_BYTES`] the file
//! becomes [`PREVIOUS`] and the next record starts a fresh one. The reader reads both, so a writer still holding the
//! old handle writes into a file that is read.

use std::io::Write as _;
use std::path::Path;

use serde::Serialize;

use super::{Reason, Rendered};

/// The record file, in a tier's `.base`.
pub const FILE: &str = "hook-output.jsonl";
/// Where a full [`FILE`] goes when the next record arrives.
pub const PREVIOUS: &str = "hook-output.1.jsonl";
/// The size at which [`FILE`] is renamed to [`PREVIOUS`].
pub const CAP_BYTES: u64 = 1 << 20;
/// How many runs per hook doctor reads (spec A7's "the last N runs").
pub const WINDOW: usize = 20;

/// The record of one emission. The only builder of a record, and it reads nothing but a [`Rendered`], so a size is
/// only ever recorded by the path that measured it.
pub fn record_of(r: &Rendered, hook: &str, session_id: Option<&str>) -> serde_json::Value {
    let withheld: Vec<serde_json::Value> = r
        .withheld
        .iter()
        .map(|w| serde_json::json!({ "block": w.block, "items": w.items, "reason": w.reason.as_str() }))
        .collect();
    serde_json::json!({
        "ts": chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        "hook": hook,
        "session_id": session_id,
        // `_bytes` names say what these now are. The old `_u16` names are still READ (see `parse`)
        // so existing history keeps working, but nothing writes them any more.
        "emitted_bytes": r.emitted_bytes,
        "budget_bytes": r.budget_bytes,
        "full_bytes": r.full_bytes,
        // Still UTF-16: the first screen is a readability limit, not a delivery one.
        "first_screen_u16": r.first_screen_u16,
        "over_budget": r.over_budget,
        "first_screen_ok": r.first_screen_ok,
        "withheld": withheld,
    })
}

/// Append `record` to `dir`'s [`FILE`], renaming a full one to [`PREVIOUS`] first. Never panics: a failure comes back
/// naming the path.
pub fn keep(dir: &Path, record: &serde_json::Value) -> Result<(), String> {
    keep_capped(dir, record, CAP_BYTES)
}

fn keep_capped(dir: &Path, record: &serde_json::Value, cap: u64) -> Result<(), String> {
    let path = dir.join(FILE);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() >= cap) {
        // A rename refused under an open handle loses nothing: the append below still happens, the file grows past
        // its cap, and a later record renames it.
        let _ = crate::store::rename_with_retry(&path, &dir.join(PREVIOUS));
    }
    let mut line = record.to_string();
    line.push('\n');
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(line.as_bytes()))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// A withheld row read back: block, items, reason.
pub type Row = (String, usize, String);

/// What a recorded size is measured in.
///
/// RECORDS WRITTEN BEFORE 2026-09-20 HOLD UTF-16 UNITS; EVERYTHING SINCE HOLDS BYTES. The host was
/// measured counting bytes that day, and the emitters were converted.
///
/// The obvious cheap move was to read old rows as though they were bytes. UTF-16 units are never
/// larger than bytes, so historical figures would only ever have been UNDERSTATED — about 5% on
/// real output — and understating errs in the safe direction.
///
/// **That reasoning was rejected, and the rejection is the point.** "Safe because the error happens
/// to point the right way" is the same sentence as "it does not bite because the output happens to
/// be ASCII", and a coerced row cannot be un-coerced by a later reader: someone opening
/// `hook-output.jsonl` in six months would see every row looking like bytes with nothing saying
/// three of them are not. **A gap the reader can see beats a number quietly 5% wrong.**
///
/// So a row keeps the unit it was written in, and anything comparing sizes must check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Unit {
    /// Bytes: what the host counts, and what every record since 2026-09-20 holds.
    Bytes,
    /// UTF-16 code units: what records written before 2026-09-20 hold.
    Utf16,
}

impl Unit {
    pub fn label(self) -> &'static str {
        match self {
            Self::Bytes => "bytes",
            Self::Utf16 => "UTF-16 units",
        }
    }
}

/// A recorded size and the unit it was measured in, kept together so the two cannot drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Size {
    pub value: usize,
    pub unit: Unit,
}

impl Size {
    pub fn bytes(value: usize) -> Self {
        Self { value, unit: Unit::Bytes }
    }

    /// True when `self` and `other` are in the same unit and can be ordered meaningfully.
    ///
    /// A caller that wants the larger of two sizes MUST ask this first. Comparing a UTF-16 row
    /// against a byte row silently compares two different quantities, which is the defect this type
    /// exists to prevent.
    pub fn comparable_with(self, other: Self) -> bool {
        self.unit == other.unit
    }
}

impl std::fmt::Display for Size {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.value, self.unit.label())
    }
}

/// One run, as doctor prints it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Run {
    pub ts: String,
    /// Sizes carry their unit: a row written before 2026-09-20 is in UTF-16 units and says so.
    pub emitted: Size,
    pub budget: Size,
    pub full: Size,
    /// Always UTF-16: a readability limit, unchanged by the byte conversion.
    pub first_screen_u16: usize,
    pub over_budget: bool,
    pub first_screen_ok: bool,
    /// Rows the trimmer degraded, in ledger order.
    pub trimmed: Vec<Row>,
    /// Rows withheld for a reason that is not a trim.
    pub other_withheld: Vec<Row>,
    /// Rows whose reason this build does not know. Named by doctor, never counted as "not trimmed".
    pub unrecognised: Vec<Row>,
}

/// One hook's runs on record in a tier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EventSizes {
    pub hook: String,
    /// Runs read, at most the window.
    pub runs: usize,
    pub last: Run,
    /// The most units emitted; a tie goes to the later run.
    pub largest: Run,
    pub over_budget_runs: usize,
    pub latest_over_budget: Option<String>,
    pub first_screen_overflow_runs: usize,
    pub latest_first_screen_overflow: Option<String>,
}

/// Whether a tier has a record file at all. Absent, a file holding no run, and a run of zero are three readings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FileState {
    /// Neither [`FILE`] nor [`PREVIOUS`] exists.
    Absent,
    /// At least one exists. Lines and files that could not be read are counted, never skipped silently.
    Present {
        unreadable_lines: usize,
        unreadable_files: usize,
    },
}

/// What doctor reads from one tier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TierSizes {
    pub tier: String,
    pub dir: String,
    pub file: FileState,
    /// Every hook with a run on record, session start first.
    pub events: Vec<EventSizes>,
}

impl TierSizes {
    pub fn event(&self, hook: &str) -> Option<&EventSizes> {
        self.events.iter().find(|e| e.hook == hook)
    }
}

/// Read `dir`'s [`PREVIOUS`], then its [`FILE`], and keep each hook's last `window` runs.
pub fn read(tier: &str, dir: &Path, window: usize) -> TierSizes {
    let mut present = false;
    let (mut unreadable_lines, mut unreadable_files) = (0usize, 0usize);
    let mut runs: Vec<(String, Run)> = Vec::new();
    for name in [PREVIOUS, FILE] {
        let bytes = match std::fs::read(dir.join(name)) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                present = true;
                unreadable_files += 1;
                continue;
            }
        };
        present = true;
        for line in String::from_utf8_lossy(&bytes).lines() {
            if line.trim().is_empty() {
                continue;
            }
            match parse(line) {
                Some(pair) => runs.push(pair),
                None => unreadable_lines += 1,
            }
        }
    }
    let mut hooks: Vec<String> = Vec::new();
    for (hook, _) in &runs {
        if !hooks.contains(hook) {
            hooks.push(hook.clone());
        }
    }
    hooks.sort_by_key(|h| (h != "session-start", h.clone()));
    let events = hooks
        .into_iter()
        .filter_map(|hook| {
            let mine: Vec<&Run> = runs
                .iter()
                .filter(|(h, _)| *h == hook)
                .map(|(_, r)| r)
                .collect();
            let recent = &mine[mine.len().saturating_sub(window)..];
            summarise(hook, recent)
        })
        .collect();
    TierSizes {
        tier: tier.to_string(),
        dir: dir.display().to_string(),
        file: if present {
            FileState::Present {
                unreadable_lines,
                unreadable_files,
            }
        } else {
            FileState::Absent
        },
        events,
    }
}

fn summarise(hook: String, recent: &[&Run]) -> Option<EventSizes> {
    let last = (*recent.last()?).clone();
    // LARGEST IS ONLY MEANINGFUL WITHIN ONE UNIT. Rows written before 2026-09-20 hold UTF-16 units
    // and rows since hold bytes; picking the biggest number across both would compare two different
    // quantities and report the winner as a fact. Skipping a row that cannot be compared leaves a
    // gap the reader can see, which is the trade ruled for this record.
    let mut largest = recent[0];
    for &run in recent {
        if run.emitted.comparable_with(largest.emitted) && run.emitted.value >= largest.emitted.value
        {
            largest = run;
        }
    }
    let over: Vec<&&Run> = recent.iter().filter(|r| r.over_budget).collect();
    let overflow: Vec<&&Run> = recent.iter().filter(|r| !r.first_screen_ok).collect();
    Some(EventSizes {
        hook,
        runs: recent.len(),
        last,
        largest: largest.clone(),
        over_budget_runs: over.len(),
        latest_over_budget: over.last().map(|r| r.ts.clone()),
        first_screen_overflow_runs: overflow.len(),
        latest_first_screen_overflow: overflow.last().map(|r| r.ts.clone()),
    })
}

/// A line read back, or `None` when any field a run needs is missing or of the wrong type.
fn parse(line: &str) -> Option<(String, Run)> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let number = |k: &str| v.get(k)?.as_u64().and_then(|n| usize::try_from(n).ok());
    let flag = |k: &str| v.get(k)?.as_bool();
    // A row is in ONE unit throughout, so the unit is decided once from whichever spelling the
    // emitted size uses, and the other fields follow it. Mixing spellings within a row is not a
    // shape base has ever written.
    let sized = |bytes_key: &str, u16_key: &str| -> Option<Size> {
        if let Some(value) = number(bytes_key) {
            Some(Size { value, unit: Unit::Bytes })
        } else {
            number(u16_key).map(|value| Size { value, unit: Unit::Utf16 })
        }
    };
    let mut run = Run {
        ts: v.get("ts")?.as_str()?.to_string(),
        emitted: sized("emitted_bytes", "emitted_u16")?,
        budget: sized("budget_bytes", "budget_u16")?,
        full: sized("full_bytes", "full_u16")?,
        first_screen_u16: number("first_screen_u16")?,
        over_budget: flag("over_budget")?,
        first_screen_ok: flag("first_screen_ok")?,
        trimmed: Vec::new(),
        other_withheld: Vec::new(),
        unrecognised: Vec::new(),
    };
    for w in v.get("withheld")?.as_array()? {
        let reason = w.get("reason")?.as_str()?;
        let row: Row = (
            w.get("block")?.as_str()?.to_string(),
            usize::try_from(w.get("items")?.as_u64()?).ok()?,
            reason.to_string(),
        );
        match Reason::parse(reason) {
            Some(r) if r.is_trim() => run.trimmed.push(row),
            Some(_) => run.other_withheld.push(row),
            None => run.unrecognised.push(row),
        }
    }
    Some((v.get("hook")?.as_str()?.to_string(), run))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record as a JSON line, the shape `record_of` writes. Built by hand so no test here constructs a `Rendered`:
    /// the census of `Rendered` construction sites stays at the one in `Emission::render`.
    fn line(hook: &str, emitted: usize, withheld: &[(&str, usize, &str)]) -> serde_json::Value {
        let rows: Vec<serde_json::Value> = withheld
            .iter()
            .map(|(b, n, r)| serde_json::json!({ "block": b, "items": n, "reason": r }))
            .collect();
        serde_json::json!({
            "ts": format!("t{emitted}"), "hook": hook, "session_id": null,
            "emitted_u16": emitted, "budget_u16": 9000, "full_u16": emitted * 2, "first_screen_u16": 2000,
            "over_budget": emitted > 9000, "first_screen_ok": true, "withheld": rows,
        })
    }

    #[test]
    fn absent_no_run_and_a_zero_are_three_readings() {
        let absent = tempfile::tempdir().unwrap();
        let no_run = tempfile::tempdir().unwrap();
        keep(no_run.path(), &line("pre-tool-use", 40, &[])).unwrap();
        let zero = tempfile::tempdir().unwrap();
        keep(zero.path(), &line("session-start", 0, &[])).unwrap();

        let a = read("workspace", absent.path(), WINDOW);
        let n = read("workspace", no_run.path(), WINDOW);
        let z = read("workspace", zero.path(), WINDOW);
        assert_eq!(a.file, FileState::Absent, "no file is absent");
        assert!(a.event("session-start").is_none());
        assert_eq!(
            n.file,
            FileState::Present {
                unreadable_lines: 0,
                unreadable_files: 0
            },
            "a file is present"
        );
        assert!(
            n.event("session-start").is_none(),
            "a file holding another hook's run has no session-start run"
        );
        let run = z.event("session-start").expect("a run of zero is a run");
        assert_eq!((run.runs, run.last.emitted.value), (1, 0));
    }

    #[test]
    fn largest_is_taken_over_the_last_twenty_only() {
        let dir = tempfile::tempdir().unwrap();
        let sizes = [
            9999, 1, 2, 3, 4, // outside the window of 20
            100, 700, 300, 310, 320, 330, 340, 350, 360, 370, 380, 390, 400, 410, 420, 430, 440,
            450, 460, 200,
        ];
        for s in sizes {
            keep(dir.path(), &line("session-start", s, &[])).unwrap();
        }
        let e = read("workspace", dir.path(), WINDOW)
            .event("session-start")
            .cloned()
            .expect("runs on record");
        assert_eq!(e.runs, 20, "the window");
        assert_eq!(e.last.emitted.value, 200, "the last run");
        assert_eq!(
            e.largest.emitted.value, 700,
            "the largest inside the window, not 9999 before it and not the first in it"
        );
    }

    #[test]
    fn a_rotation_loses_no_record() {
        let dir = tempfile::tempdir().unwrap();
        keep(dir.path(), &line("session-start", 1, &[])).unwrap();
        let one = std::fs::metadata(dir.path().join(FILE)).unwrap().len();
        std::fs::remove_file(dir.path().join(FILE)).unwrap();
        // Records of equal length: three fill the file to the cap, so the fourth renames it first. Six records, one rename.
        let cap = one * 3;
        for s in 1..=6 {
            keep_capped(dir.path(), &line("session-start", s, &[]), cap).unwrap();
        }
        let previous =
            std::fs::read_to_string(dir.path().join(PREVIOUS)).expect("the renamed file");
        let current = std::fs::read_to_string(dir.path().join(FILE)).expect("the fresh file");
        assert_eq!(
            (previous.lines().count(), current.lines().count()),
            (3, 3),
            "one rename at the cap"
        );
        let e = read("workspace", dir.path(), WINDOW)
            .event("session-start")
            .cloned()
            .expect("runs on record");
        assert_eq!(
            e.runs, 6,
            "every record written across the rename is read back"
        );
        assert_eq!(
            (e.last.emitted.value, e.largest.emitted.value),
            (6, 6),
            "in order"
        );
    }

    #[test]
    fn an_unreadable_line_is_counted_and_never_read_as_a_run() {
        let dir = tempfile::tempdir().unwrap();
        let body = format!(
            "not json\n{}\n{{\"hook\":\"session-start\"}}\n",
            line("session-start", 5, &[])
        );
        std::fs::write(dir.path().join(FILE), body).unwrap();
        let t = read("workspace", dir.path(), WINDOW);
        assert_eq!(
            t.file,
            FileState::Present {
                unreadable_lines: 2,
                unreadable_files: 0
            }
        );
        assert_eq!(t.event("session-start").map(|e| e.runs), Some(1));
    }

    #[test]
    fn trim_reasons_are_exactly_collapsed_list_cut_and_shortened() {
        for r in Reason::ALL {
            assert_eq!(
                Reason::parse(r.as_str()),
                Some(r),
                "{} reads back",
                r.as_str()
            );
            let trim = matches!(
                r,
                Reason::Collapsed | Reason::ListCut | Reason::TextShortened
            );
            assert_eq!(r.is_trim(), trim, "{} is_trim", r.as_str());
        }
        assert_eq!(Reason::parse("renamed in a later build"), None);
        let t = {
            let dir = tempfile::tempdir().unwrap();
            let rows = [
                ("forks", 157, "collapsed"),
                ("memory", 3, "unchanged"),
                ("x", 1, "renamed in a later build"),
            ];
            keep(dir.path(), &line("session-start", 10, &rows)).unwrap();
            read("workspace", dir.path(), WINDOW)
        };
        let run = &t.event("session-start").expect("a run").last;
        assert_eq!(
            run.trimmed,
            vec![("forks".to_string(), 157, "collapsed".to_string())]
        );
        assert_eq!(
            run.other_withheld,
            vec![("memory".to_string(), 3, "unchanged".to_string())]
        );
        assert_eq!(
            run.unrecognised,
            vec![("x".to_string(), 1, "renamed in a later build".to_string())]
        );
    }
}
