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
//! a row from day D goes on day D + prompt_days + 1. Counting whole days means the file is rewritten at most once a
//! day, at the first session start that finds an old row first in the file, and never on the other session starts.

use std::io::{BufRead, Read, Seek, SeekFrom, Write};
use std::path::Path;

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
        PromptText::Matched => Some(crate::scrub::scrub(&trace.words.join(" "))),
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
        matched: trace.matched,
        served,
        cut,
        scores: trace.scores,
    }
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
        matched: trace.matched,
        served: trace.served,
        cut: trace.cut,
        scores: trace.scores,
    })
}

/// Append `row` to `dir`'s [`FILE`] as one line in one write. A failure comes back naming the path; never a panic.
pub fn append(dir: &Path, row: &Row) -> Result<(), String> {
    let path = dir.join(FILE);
    let mut line = serde_json::to_string(row).map_err(|e| format!("{}: {e}", path.display()))?;
    line.push('\n');
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(line.as_bytes()))
        .map_err(|e| format!("{}: {e}", path.display()))
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

/// Remove the rows of `dir`'s [`FILE`] dated more than `days` calendar days before `today` (K1e; `days` read as at
/// least 1), and any line that is not a row. Returns how many lines went.
///
/// The first line decides whether anything is done: rows are appended in time order, so when the first is young
/// every row is, and the file is not read further. Otherwise the young rows go to a temp file and replace the log. A
/// row appended while that happens is carried over: the log's length is read again just before the rename, and any
/// bytes past what was read go on the end of the temp file. Fail-open: an error comes back as a value, and the log
/// is left as it was.
pub fn prune(dir: &Path, days: u64, today: NaiveDate) -> Result<usize, String> {
    let path = dir.join(FILE);
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(err(e)),
    };
    let cutoff = today - chrono::Days::new(days.max(1));
    let mut first = String::new();
    std::io::BufReader::new(&file).read_line(&mut first).map_err(err)?;
    if first.is_empty() || row_day(&first).is_some_and(|d| d >= cutoff) {
        return Ok(0);
    }
    let mut bytes = Vec::new();
    (&file).seek(SeekFrom::Start(0)).map_err(err)?;
    (&file).read_to_end(&mut bytes).map_err(err)?;
    drop(file);
    // Whole lines only. A torn last line (a writer mid-append) is not judged here: it is carried over below.
    let read_to = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |p| p + 1);
    let text = String::from_utf8_lossy(&bytes[..read_to]);
    let mut kept = String::with_capacity(text.len());
    let mut removed = 0usize;
    for line in text.split_inclusive('\n') {
        if row_day(line).is_some_and(|d| d >= cutoff) {
            kept.push_str(line);
        } else {
            removed += 1;
        }
    }
    let tmp = path.with_extension(format!("jsonl.{}.tmp", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        std::fs::write(&tmp, kept.as_bytes())?;
        // Whatever arrived after the read, the torn line included, goes on the end.
        let mut now = std::fs::File::open(&path)?;
        let len = now.metadata()?.len() as usize;
        if len > read_to {
            now.seek(SeekFrom::Start(read_to as u64))?;
            let mut tail = Vec::new();
            now.read_to_end(&mut tail)?;
            std::fs::OpenOptions::new().append(true).open(&tmp)?.write_all(&tail)?;
        }
        drop(now);
        crate::store::rename_with_retry(&tmp, &path)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(err(e));
    }
    Ok(removed)
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

/// The last `n` rows of `dir`'s [`FILE`] that `filter` keeps, oldest first. Read from the end in blocks, so a long
/// log costs only the rows it returns. A line that is not a row is passed over. No file is no rows.
pub fn last_rows(dir: &Path, n: usize, filter: &Filter) -> Result<Vec<Row>, String> {
    const BLOCK: u64 = 64 * 1024;
    let path = dir.join(FILE);
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    let mut file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(err(e)),
    };
    let mut pos = file.metadata().map_err(err)?.len();
    let mut out: Vec<Row> = Vec::new();
    // Bytes read but not yet split into whole lines: the start of the earliest line seen so far.
    let mut carry: Vec<u8> = Vec::new();
    while out.len() < n {
        if pos == 0 {
            take_line(&carry, filter, &mut out);
            break;
        }
        let step = pos.min(BLOCK);
        pos -= step;
        file.seek(SeekFrom::Start(pos)).map_err(err)?;
        let mut block = vec![0u8; step as usize];
        file.read_exact(&mut block).map_err(err)?;
        block.extend_from_slice(&carry);
        // Every line after the first newline in the block is whole; the part before it may continue further back.
        let Some(first_nl) = block.iter().position(|b| *b == b'\n') else {
            carry = block;
            continue;
        };
        for line in block[first_nl + 1..].split(|b| *b == b'\n').rev() {
            if out.len() >= n {
                break;
            }
            take_line(line, filter, &mut out);
        }
        carry = block[..first_nl].to_vec();
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
                None => "(prompt text off)".to_string(),
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
