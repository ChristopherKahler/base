//! The match log (K1, BO-13; Chris's D2 and D14): one row for every prompt and every tool call that touches a file,
//! saying which domains matched and by what, which rules and decisions were served, which were cut and why, and the
//! scores `select` used. It is the history every later tuning step reads (rule tests, correction sorting, replay,
//! usage counts, shadow mode).
//!
//! WHY. Until this file `hook-output.jsonl` recorded bytes per hook run and nothing about matching. On 2026-10-01 a
//! prompt about base's own hooks matched only the always-on GLOBAL domain (the `base` domain had no prompt keywords),
//! and nothing anywhere recorded that it had been a miss.
//!
//! WHERE. `<tier>/.base/match-log.jsonl`, beside `hook-output.jsonl`: the workspace's `.base` when the cwd has one,
//! else the global tier's (`crud::handoff_show::session_start_dir`, the resolution every hook record uses).
//!
//! HOW IT IS WRITTEN. Each row is serialised first and appended in ONE `write_all` under `O_APPEND`, as
//! `record::keep` writes, so two hooks writing at once never interleave inside a line. The hook writes its row only
//! after its output is printed, and a failed write is one line on stderr: the log never changes or holds up what the
//! model receives (K1g).
//!
//! WHAT IT READS. Nothing is recomputed for the log. The domain matches carry the keyword or path that fired them
//! (`domain::matcher`), `rules::Selection` carries what `select` cut and the scores it used, every prompt block
//! carries the rules and decisions in it ([`super::prompt::PromptBlock::logged`]), and the fit decides which blocks
//! were printed. A block the budget dropped puts its rules and decisions under `cut`, reason `budget`.
//!
//! PROMPT TEXT (K1c, the K1 recommendation). Kept raw by default, on this machine only, after [`crate::scrub`]:
//! `[log] prompt_text = "full" | "matched" | "off"`. `matched` keeps only the words that matched a keyword or a topic
//! rule; `off` keeps no text. A value base does not know reads as `off`, the setting that keeps the least.
//!
//! RETENTION (K1e, D14). Session start removes rows older than `[log] prompt_days` (90), counted in calendar days:
//! a row from day D goes on day D + prompt_days + 1. The file being appended to is never rewritten: once a day
//! session start moves it to `match-log/<when>.jsonl` and deletes the archive files whose rows have all aged out
//! ([`retain`] says why). [`files`] lists every file of the log, oldest first.

use std::io::{BufRead, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, NaiveDate};
use serde::{Deserialize, Serialize};

/// The log, in a tier's `.base`.
pub const FILE: &str = "match-log.jsonl";

/// `[log] prompt_text`: how much of a prompt a row keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptText {
    /// The whole prompt, after the secret scrub. The default.
    Full,
    /// Only the words that matched a domain keyword or a topic rule.
    Matched,
    /// No text.
    Off,
}

impl PromptText {
    /// `full`, `matched` or `off`, case and spaces ignored. Anything else is `off`: a misspelt setting keeps less,
    /// never more.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "full" => Self::Full,
            "matched" => Self::Matched,
            _ => Self::Off,
        }
    }
}

/// One domain a row matched, and by what.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Matched {
    pub domain: String,
    /// `always`, `keyword`, `path`, `parent`, `file_keyword`, or `command` for a star command.
    pub by: String,
    /// The keyword, the folder or trigger that held the path, or `<child> nested` for a parent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// For a path match on a prompt: the path this session touched that brought it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl Matched {
    pub fn new(domain: &str, by: &str, value: Option<String>) -> Self {
        Self { domain: domain.to_string(), by: by.to_string(), value, path: None }
    }
}

/// A rule, decision or command mode a row names.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Item {
    /// A rule's stable id (`rules::rule_id`), a decision's slug, `bracket:<hash>` for a bracket rule, a command's name.
    pub id: String,
    /// `rule`, `decision`, `bracket-rule` or `command`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// The prompt block that carried it, as `base hooks show` names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<String>,
    /// For a rule with matchers of its own: `always`, `topic`, `place: <place>` or `action: <action>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// A topic rule's score.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

impl Item {
    pub fn rule(id: &str, domain: &str) -> Self {
        Self { id: id.to_string(), kind: "rule".into(), domain: Some(domain.to_string()), ..Self::default() }
    }

    /// A decision by its graph IRI (`<...decision/global.x>`): logged by its slug, the IRI's last segment.
    pub fn decision(iri: &str, domain: Option<&str>) -> Self {
        Self { id: decision_id(iri), kind: "decision".into(), domain: domain.map(String::from), ..Self::default() }
    }

    pub fn of_kind(id: &str, kind: &str) -> Self {
        Self { id: id.to_string(), kind: kind.to_string(), ..Self::default() }
    }

    fn in_block(mut self, block: &str) -> Self {
        self.block = Some(block.to_string());
        self
    }
}

/// A decision's slug from its IRI, `<...decision/global.x>` to `global.x`.
pub fn decision_id(iri: &str) -> String {
    let bare = iri.trim_start_matches('<').trim_end_matches('>');
    bare.rsplit('/').next().unwrap_or(bare).to_string()
}

/// Is this IRI a decision's?
pub fn is_decision(iri: &str) -> bool {
    crate::graph_query::iri_kind(iri) == Some("decision")
}

/// A rule or decision that was in reach and was not served, and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cut {
    #[serde(flatten)]
    pub item: Item,
    /// `budget`, `topic limit` or `not matched`.
    pub reason: String,
    /// The setting that decided it: `prompt_bytes`, `walk_budget`, `topic_max` or `topic_min_score`.
    pub limit: String,
}

impl Cut {
    pub fn new(item: Item, reason: &str, limit: &str) -> Self {
        Self { item, reason: reason.to_string(), limit: limit.to_string() }
    }
}

/// A score `select` computed for a topic rule, above zero, whether or not the rule was served.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Score {
    pub id: String,
    pub domain: String,
    pub score: f32,
}

/// What a hook gathered for its row while it decided what to serve.
#[derive(Debug, Default)]
pub struct Trace {
    pub matched: Vec<Matched>,
    /// What a tool call served (it prints no blocks). The prompt hook's served list comes from its printed blocks.
    pub served: Vec<Item>,
    /// What was cut before the fit: `topic limit`, `not matched`, and records the walk's own budget dropped.
    pub cut: Vec<Cut>,
    pub scores: Vec<Score>,
    /// The words of the prompt that matched, for `prompt_text = "matched"`.
    pub words: Vec<String>,
    /// A tool call: its name and every path it touched.
    pub tool: Option<String>,
    pub paths: Vec<String>,
}

impl Trace {
    /// Add a word that matched, once.
    pub fn word(&mut self, w: &str) {
        let w = w.trim();
        if !w.is_empty() && !self.words.iter().any(|x| x.eq_ignore_ascii_case(w)) {
            self.words.push(w.to_string());
        }
    }
}

/// One row of the log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub ts: String,
    #[serde(default)]
    pub session: Option<String>,
    /// `prompt` or `file`.
    pub event: String,
    /// This session's prompt number, on a prompt row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_num: Option<u32>,
    /// The prompt as `[log] prompt_text` keeps it, scrubbed. Absent when it is `off`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// The first path a tool call touched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Every path it touched, when there was more than one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(default)]
    pub matched: Vec<Matched>,
    #[serde(default)]
    pub served: Vec<Item>,
    #[serde(default)]
    pub cut: Vec<Cut>,
    #[serde(default)]
    pub scores: Vec<Score>,
}

fn now() -> String {
    Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

/// The prompt hook's row: what `fitted` printed is served, what it dropped is cut for the budget named `key`.
pub fn prompt_row(
    trace: Trace,
    fitted: &super::prompt::Fitted,
    key: &str,
    session: Option<&str>,
    prompt: &str,
    prompt_num: Option<u32>,
    mode: PromptText,
) -> Row {
    let served: Vec<Item> =
        fitted.kept_blocks().flat_map(|b| b.logged.iter().map(|i| i.clone().in_block(&b.id))).collect();
    let mut cut: Vec<Cut> = fitted
        .dropped_blocks()
        .flat_map(|b| b.logged.iter().map(|i| Cut::new(i.clone().in_block(&b.id), "budget", key)))
        .collect();
    cut.extend(trace.cut);
    let text = match mode {
        PromptText::Full => Some(crate::scrub::scrub(prompt)),
        // No word matched: no text, rather than an empty one a reader would take for an empty prompt.
        PromptText::Matched => (!trace.words.is_empty()).then(|| crate::scrub::scrub(&trace.words.join(" "))),
        PromptText::Off => None,
    };
    Row {
        ts: now(),
        session: session.map(String::from),
        event: "prompt".into(),
        prompt_num,
        text,
        tool: None,
        path: None,
        paths: Vec::new(),
        matched: scrubbed(trace.matched),
        served,
        cut,
        scores: trace.scores,
    }
}

/// The matched entries with their paths and values scrubbed too: a touched path or a folder is written as carefully as
/// the prompt.
fn scrubbed(mut matched: Vec<Matched>) -> Vec<Matched> {
    for m in &mut matched {
        m.value = m.value.as_deref().map(crate::scrub::scrub);
        m.path = m.path.as_deref().map(crate::scrub::scrub);
    }
    matched
}

/// A tool call's row, or `None` when it touched no path and served nothing: such a call is not a file touch.
pub fn file_row(trace: Trace, session: Option<&str>) -> Option<Row> {
    if trace.paths.is_empty() && trace.served.is_empty() {
        return None;
    }
    let paths: Vec<String> = trace.paths.iter().map(|p| crate::scrub::scrub(p)).collect();
    Some(Row {
        ts: now(),
        session: session.map(String::from),
        event: "file".into(),
        prompt_num: None,
        text: None,
        tool: trace.tool,
        path: paths.first().cloned(),
        paths: if paths.len() > 1 { paths } else { Vec::new() },
        matched: scrubbed(trace.matched),
        served: trace.served,
        cut: trace.cut,
        scores: trace.scores,
    })
}

/// Append `row` to `dir`'s [`FILE`] as one line in one write. A failure comes back naming the path; never a panic.
pub fn append(dir: &Path, row: &Row) -> Result<(), String> {
    let path = dir.join(FILE);
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    let mut line = serde_json::to_string(row).map_err(|e| format!("{}: {e}", path.display()))?;
    line.push('\n');
    let mut file = private(std::fs::OpenOptions::new().create(true).append(true)).open(&path).map_err(err)?;
    owner_only(&file);
    file.write_all(line.as_bytes()).map_err(err)
}

/// The log holds what prompts said, so on Unix a file it creates is readable by its owner only (0600), as the secret
/// store is. Windows files inherit the folder's access list, which is the user's own under their profile.
fn private(opts: &mut std::fs::OpenOptions) -> &mut std::fs::OpenOptions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts
}

/// [`private`]'s mode applies only to a file it creates. A log another build created readable by others is made the
/// owner's only here. Best effort: a file this user cannot change is not this user's to keep private.
fn owner_only(file: &std::fs::File) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = file.metadata()
            && meta.permissions().mode() & 0o077 != 0
        {
            let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
        }
    }
    #[cfg(not(unix))]
    let _ = file;
}

/// The local calendar day of a row's `ts`.
fn row_day(line: &str) -> Option<NaiveDate> {
    #[derive(Deserialize)]
    struct Ts {
        ts: String,
    }
    let ts: Ts = serde_json::from_str(line).ok()?;
    DateTime::parse_from_rfc3339(&ts.ts).ok().map(|t| t.with_timezone(&Local).date_naive())
}

/// Earlier days' rows, in `<tier>/.base/match-log/`: one file per day the log was moved aside, named by when.
pub const ARCHIVE: &str = "match-log";
/// Held while session start moves the log aside and expires old files, so two session starts never both do.
const LOCK: &str = "match-log.lock";

/// What [`retain`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Retained {
    /// Where [`FILE`] went, when its first row was from an earlier day.
    pub moved_to: Option<PathBuf>,
    /// Archive files removed whole: none of their rows was young enough to keep.
    pub files_removed: usize,
    /// Lines removed from an archive file whose rows straddled the cut-off.
    pub rows_removed: usize,
}

/// Session start's retention pass (K1e, D14): rows dated more than `days` calendar days before `now` go (`days` read
/// as at least 1).
///
/// THE LOG BEING WRITTEN IS NEVER REWRITTEN. Measured on the operator's machine, a day is about 650 prompts and 3,500
/// tool calls, and a prompt row averages 2.5 KB (BO-13 replay), so 90 days of one file is a few hundred MB. Rewriting
/// that at every session start would hold the hook for seconds, and a row another hook appended during the rewrite
/// would be lost. So:
///
/// 1. When the first row of [`FILE`] is from an earlier day than `now`, or is not a row, the file is renamed to
///    `match-log/<now>.jsonl`, and the next row starts a new log: one rename, at most once a day. A hook that opened
///    the log in the instant before the rename writes into the moved file, so its row is kept.
/// 2. An archive file whose rows are all past the cut-off is deleted. One whose first row is past it and whose last
///    is not keeps its younger rows, through a temp file; nothing appends to an archive file, except such a hook in
///    that instant, and bytes it adds before the rename are carried over.
///
/// Under a lock no second session start can take at the same time (`try_lock`): a second one does nothing. Rows are
/// still appended while it runs; appends take no lock. Fail-open: an error comes back as a value.
pub fn retain(dir: &Path, days: u64, now: DateTime<Local>) -> Result<Retained, String> {
    let at = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    let lock_path = dir.join(LOCK);
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|e| at(&lock_path, e))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return Ok(Retained::default()),
        Err(std::fs::TryLockError::Error(e)) => return Err(at(&lock_path, e)),
    }
    let mut done = Retained::default();
    let live = dir.join(FILE);
    if let Some(first) = first_line(&live).map_err(|e| at(&live, e))?
        && row_day(&first).is_none_or(|d| d < now.date_naive())
    {
        let archive = dir.join(ARCHIVE);
        std::fs::create_dir_all(&archive).map_err(|e| at(&archive, e))?;
        let to = archive.join(format!("{}.jsonl", now.format("%Y-%m-%dT%H-%M-%S")));
        crate::store::rename_with_retry(&live, &to).map_err(|e| at(&live, e))?;
        done.moved_to = Some(to);
    }
    let cutoff = now.date_naive() - chrono::Days::new(days.max(1));
    for file in archive_files(dir) {
        // Rows are in time order: a young first row means a young file, and nothing past the first line is read.
        let first = first_line(&file).map_err(|e| at(&file, e))?;
        if first.as_deref().and_then(row_day).is_some_and(|d| d >= cutoff) {
            continue;
        }
        let mut last = None;
        rev_lines(&file, |line| {
            last = std::str::from_utf8(line).ok().and_then(row_day);
            last.is_none()
        })
        .map_err(|e| at(&file, e))?;
        if last.is_none_or(|d| d < cutoff) {
            std::fs::remove_file(&file).map_err(|e| at(&file, e))?;
            done.files_removed += 1;
        } else {
            done.rows_removed += keep_young(&file, cutoff).map_err(|e| at(&file, e))?;
        }
    }
    drop(lock);
    Ok(done)
}

/// Rewrite one archive file with only its rows dated on or after `cutoff`; any line that is not a row goes too. A line
/// still being written (no newline yet) and anything added after the read are carried over whole. Returns how many
/// lines went.
fn keep_young(file: &Path, cutoff: NaiveDate) -> std::io::Result<usize> {
    let tmp = file.with_extension(format!("jsonl.{}.tmp", std::process::id()));
    let result = (|| -> std::io::Result<usize> {
        let mut out = std::io::BufWriter::new(
            private(std::fs::OpenOptions::new().write(true).create(true).truncate(true)).open(&tmp)?,
        );
        let mut reader = std::io::BufReader::new(std::fs::File::open(file)?);
        let (mut read_to, mut removed) = (0u64, 0usize);
        let mut line = Vec::new();
        loop {
            line.clear();
            let n = reader.read_until(b'\n', &mut line)?;
            if n == 0 || line.last() != Some(&b'\n') {
                break;
            }
            read_to += n as u64;
            if std::str::from_utf8(&line).ok().and_then(row_day).is_some_and(|d| d >= cutoff) {
                out.write_all(&line)?;
            } else {
                removed += 1;
            }
        }
        drop(reader);
        let mut now = std::fs::File::open(file)?;
        if now.metadata()?.len() > read_to {
            now.seek(SeekFrom::Start(read_to))?;
            std::io::copy(&mut now, &mut out)?;
        }
        drop(now);
        out.flush()?;
        drop(out);
        crate::store::rename_with_retry(&tmp, file)?;
        Ok(removed)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// The first line of `path`, without its newline. `None` when there is no file or it is empty.
fn first_line(path: &Path) -> std::io::Result<Option<String>> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut line = Vec::new();
    std::io::BufReader::new(file).read_until(b'\n', &mut line)?;
    Ok((!line.is_empty()).then(|| String::from_utf8_lossy(&line).trim_end().to_string()))
}

/// The archive files, oldest first: their names are the times they were moved aside.
fn archive_files(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir.join(ARCHIVE))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    out.sort();
    out
}

/// Every file of the log, oldest first: the archive's, then [`FILE`]. For the readers of the log (BO-14 on).
pub fn files(dir: &Path) -> Vec<PathBuf> {
    let mut out = archive_files(dir);
    out.push(dir.join(FILE));
    out
}

/// Each non-empty line of `path`, last first, until `each` returns false. Read from the end in blocks, so stopping
/// early costs only what was read. No file is no lines.
fn rev_lines(path: &Path, mut each: impl FnMut(&[u8]) -> bool) -> std::io::Result<()> {
    const BLOCK: u64 = 64 * 1024;
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let mut pos = file.metadata()?.len();
    // Bytes read but not yet split into whole lines: the start of the earliest line seen so far.
    let mut carry: Vec<u8> = Vec::new();
    loop {
        if pos == 0 {
            if !carry.is_empty() {
                each(&carry);
            }
            return Ok(());
        }
        let step = pos.min(BLOCK);
        pos -= step;
        file.seek(SeekFrom::Start(pos))?;
        let mut block = vec![0u8; step as usize];
        file.read_exact(&mut block)?;
        block.extend_from_slice(&carry);
        // Every line after the first newline in the block is whole; the part before it may continue further back.
        let Some(first_nl) = block.iter().position(|b| *b == b'\n') else {
            carry = block;
            continue;
        };
        for line in block[first_nl + 1..].split(|b| *b == b'\n').rev() {
            if !line.is_empty() && !each(line) {
                return Ok(());
            }
        }
        carry = block[..first_nl].to_vec();
    }
}

/// What `base log matches` keeps.
#[derive(Debug, Default, Clone)]
pub struct Filter {
    /// A session id, or the start of one.
    pub session: Option<String>,
    /// A rule's or decision's id, or the start of one, served or cut.
    pub rule: Option<String>,
}

impl Filter {
    fn keeps(&self, row: &Row) -> bool {
        let session = self
            .session
            .as_deref()
            .is_none_or(|s| row.session.as_deref().is_some_and(|r| r.starts_with(s)));
        let rule = self.rule.as_deref().is_none_or(|id| {
            row.served.iter().any(|i| i.id.starts_with(id)) || row.cut.iter().any(|c| c.item.id.starts_with(id))
        });
        session && rule
    }
}

/// The last `n` rows of the log that `filter` keeps, oldest first: [`FILE`], then the archive, newest first, each read
/// from its end, so a long log costs only the rows it returns. A line that is not a row is passed over.
pub fn last_rows(dir: &Path, n: usize, filter: &Filter) -> Result<Vec<Row>, String> {
    let mut out: Vec<Row> = Vec::new();
    for path in files(dir).iter().rev() {
        if out.len() >= n {
            break;
        }
        rev_lines(path, |line| {
            take_line(line, filter, &mut out);
            out.len() < n
        })
        .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    out.reverse();
    Ok(out)
}

fn take_line(line: &[u8], filter: &Filter, out: &mut Vec<Row>) {
    let Ok(text) = std::str::from_utf8(line) else { return };
    if let Ok(row) = serde_json::from_str::<Row>(text.trim())
        && filter.keeps(&row)
    {
        out.push(row);
    }
}

/// `base log matches` without `--json`: one line per row (BO-13 Example 4). The time alone for a row from `today`,
/// the date before it otherwise.
pub fn format_rows(rows: &[Row], today: NaiveDate) -> String {
    let mut out = String::new();
    for row in rows {
        let when = DateTime::parse_from_rfc3339(&row.ts).map(|t| t.with_timezone(&Local));
        let time = match when {
            Ok(t) if t.date_naive() == today => t.format("%H:%M:%S").to_string(),
            Ok(t) => t.format("%Y-%m-%d %H:%M:%S").to_string(),
            Err(_) => row.ts.clone(),
        };
        let tail = match row.event.as_str() {
            "prompt" => match &row.text {
                Some(t) => format!("\"{}\"", clip(t, 40)),
                None => "(no prompt text)".to_string(),
            },
            _ => row.path.as_deref().map(short_path).unwrap_or_default(),
        };
        out.push_str(&format!(
            "{time}  {:<6}  {:<36}  served {}  cut {}  {tail}\n",
            row.event,
            matched_summary(&row.matched),
            row.served.len(),
            row.cut.len()
        ));
    }
    out
}

/// `vintryx(keyword: anthony, morning) GLOBAL(always)`: each domain once, in the order it matched, with what matched
/// it; `no match` when nothing did.
fn matched_summary(matched: &[Matched]) -> String {
    let mut domains: Vec<(&str, Vec<String>)> = Vec::new();
    for m in matched {
        let what = match (m.by.as_str(), &m.value) {
            ("keyword", Some(v)) => format!("keyword: {v}"),
            ("file_keyword", Some(v)) => format!("file keyword: {v}"),
            (by, _) => by.to_string(),
        };
        match domains.iter_mut().find(|(d, _)| *d == m.domain) {
            Some((_, list)) => {
                if !list.contains(&what) {
                    list.push(what);
                }
            }
            None => domains.push((&m.domain, vec![what])),
        }
    }
    if domains.is_empty() {
        return "no match".to_string();
    }
    let parts: Vec<String> = domains.iter().map(|(d, list)| format!("{d}({})", fold_keywords(list))).collect();
    parts.join(" ")
}

/// `keyword: a, keyword: b` as `keyword: a, b`.
fn fold_keywords(list: &[String]) -> String {
    let mut out: Vec<String> = Vec::new();
    for item in list {
        match (item.strip_prefix("keyword: "), out.last_mut()) {
            (Some(kw), Some(last)) if last.starts_with("keyword: ") => {
                last.push_str(", ");
                last.push_str(kw);
            }
            _ => out.push(item.clone()),
        }
    }
    out.join(", ")
}

/// The first `max` characters, and `...` when there were more.
fn clip(text: &str, max: usize) -> String {
    let one_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max {
        return one_line;
    }
    let head: String = one_line.chars().take(max).collect();
    format!("{}...", head.trim_end())
}

/// `.../dealer-registry/README.md`: the last two parts of a path.
fn short_path(path: &str) -> String {
    let parts: Vec<&str> = path.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    if parts.len() <= 2 {
        return path.to_string();
    }
    format!(".../{}/{}", parts[parts.len() - 2], parts[parts.len() - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_text_parses_and_unknown_keeps_nothing() {
        assert_eq!(PromptText::parse("full"), PromptText::Full);
        assert_eq!(PromptText::parse(" Matched "), PromptText::Matched);
        assert_eq!(PromptText::parse("off"), PromptText::Off);
        assert_eq!(PromptText::parse("ful"), PromptText::Off, "a misspelt setting keeps less, never more");
    }

    #[test]
    fn decision_ids_are_slugs() {
        assert_eq!(decision_id("<http://ops-sys.local/ontology#decision/global.mirror-profile-a>"), "global.mirror-profile-a");
        assert!(is_decision("<http://ops-sys.local/ontology#decision/global.x>"));
        assert!(!is_decision("<http://ops-sys.local/ontology#project/x>"));
    }

    /// The reader walks back across 64 KB blocks and across files: lines split by a block edge come out whole, and a
    /// filter that matches only the oldest row reads back to it.
    #[test]
    fn last_rows_reads_back_across_blocks_and_files() {
        let dir = tempfile::tempdir().unwrap();
        let row = |i: usize| {
            let r = Row {
                ts: "2026-10-01T10:00:00-05:00".into(),
                session: Some(format!("s{i:05}")),
                event: "prompt".into(),
                prompt_num: None,
                text: Some("x".repeat(i % 300)),
                tool: None,
                path: None,
                paths: Vec::new(),
                matched: Vec::new(),
                served: vec![Item::rule(&format!("r{i:05}"), "d")],
                cut: Vec::new(),
                scores: Vec::new(),
            };
            format!("{}\n", serde_json::to_string(&r).unwrap())
        };
        let archive = dir.path().join(ARCHIVE);
        std::fs::create_dir_all(&archive).unwrap();
        std::fs::write(archive.join("2026-10-01T00-00-00.jsonl"), (0..1500).map(row).collect::<String>()).unwrap();
        std::fs::write(dir.path().join(FILE), (1500..3000).map(row).collect::<String>()).unwrap();
        assert!(std::fs::metadata(dir.path().join(FILE)).unwrap().len() > 3 * 64 * 1024, "control: several blocks");

        let last = last_rows(dir.path(), 5, &Filter::default()).unwrap();
        let ids: Vec<&str> = last.iter().map(|r| r.served[0].id.as_str()).collect();
        assert_eq!(ids, ["r02995", "r02996", "r02997", "r02998", "r02999"]);
        let all = last_rows(dir.path(), 10_000, &Filter::default()).unwrap();
        assert_eq!(all.len(), 3000, "every row, none split");
        assert!(all.windows(2).all(|w| w[0].served[0].id < w[1].served[0].id), "oldest first");
        let first = last_rows(dir.path(), 5, &Filter { rule: Some("r00000".into()), ..Filter::default() }).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].session.as_deref(), Some("s00000"));
    }

    #[test]
    fn summary_folds_keywords_per_domain() {
        let m = vec![
            Matched::new("vintryx", "keyword", Some("anthony".into())),
            Matched::new("vintryx", "keyword", Some("morning".into())),
            Matched::new("GLOBAL", "always", None),
        ];
        assert_eq!(matched_summary(&m), "vintryx(keyword: anthony, morning) GLOBAL(always)");
        assert_eq!(matched_summary(&[]), "no match");
        assert_eq!(clip("I want to make sure that we are working with a version", 40), "I want to make sure that we are working...");
        assert_eq!(short_path("C:/Users/Chris/Documents/Vintryx/dealer-registry/README.md"), ".../dealer-registry/README.md");
    }
}
