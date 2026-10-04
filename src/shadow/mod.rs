//! Shadow mode (K9, BO-20): a candidate matcher runs beside the live one on every prompt and file touch, the match log
//! records what it would have served, and it is promoted or rolled back on evidence: the user's corrections.
//!
//! WHY. Replay (BO-16) checks a change against past prompts, but cannot know whether a rule the live matcher never
//! served would have helped. A shadow runs the candidate on new prompts, where the corrections that follow say which
//! side was right (Chris's K9, locked under D12: running and reporting are automatic; promotion is automatic only when
//! the evidence is clear, with automatic rollback, and `[shadow] auto_promote = false` asks for approval instead).
//!
//! NOTHING RUNS UNTIL SOMEONE STARTS ONE. With no state file every hook prints the same bytes as before, writes no
//! `shadow` field and session start prints nothing new (Chris, 2026-10-03, on the 0.16.0 upgrade: "It should be a
//! seamless update"). The hooks pay one failed file open.
//!
//! ONE SHADOW PER MACHINE. `[match]` lives in the global `base.toml`, and every tier's hooks read the same state:
//! `~/.base-gbl/.base/shadow/state.json` and one file per version in `versions/`. The rows go to each tier's match log
//! as every row does; the report reads the cwd's tier and the global one (`usage::log_dirs`).
//!
//! THE HOOKS ONLY READ. The candidate runs after live's output is printed, on a copy of the session state live decided
//! with, on the data live already read (`hook::user_prompt_submit::World`), and the only thing it writes is the
//! `shadow` field of the row the hook writes anyway. State changes (start, stop, promote, rollback, the announcements)
//! happen in the commands and at session start, under [`LOCK`].

pub mod promote;
pub mod report;
pub mod run;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{BaseConfig, MatchConfig};
use crate::domain::replay::{Change, Target};

/// The folder, in the global tier's `.base`.
pub const DIR: &str = "shadow";
/// What runs, what was promoted, what session start has to say.
pub const STATE: &str = "state.json";
/// One JSON file per version.
pub const VERSIONS: &str = "versions";
/// Held while the state is changed (a command, or session start's pass), so two never interleave.
pub const LOCK: &str = "shadow.lock";

/// `~/.base-gbl/.base/shadow`.
pub fn dir() -> Option<PathBuf> {
    crate::config::global_base_dir().map(|d| d.join(DIR))
}

/// What the shadow is doing, on this machine.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// The version live serves, as last snapshotted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live: Option<String>,
    /// The candidate running beside it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate: Option<Running>,
    /// The last promotion, its watch (K9g) and how to undo it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promotion: Option<promote::Promotion>,
    /// Lines the next session start prints, once each (K9h).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub announce: Vec<String>,
    /// The candidate whose "ready to promote" line was printed (`auto_promote = false`, lynx's Q9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_told: Option<String>,
    /// The number the next version takes: one counter for every version on the machine.
    #[serde(default = "first_number")]
    pub next: u32,
}

fn first_number() -> u32 {
    1
}

/// The candidate that runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Running {
    pub name: String,
    /// The live version it runs against.
    pub against: String,
    /// When it started (RFC 3339): the report counts events from here.
    pub since: String,
    /// Live's content hash when it started: the report says when it has moved.
    pub live_hash: String,
}

impl State {
    /// The state in `dir`, or a fresh one when there is none or it does not parse.
    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(dir.join(STATE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    /// Write it through a temp file and a rename.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        write_json(dir, STATE, self)
    }

    /// A new version name: `<kind>-<NNNN>`, the counter moved on.
    pub fn name(&mut self, kind: &str) -> String {
        let n = self.next.max(1);
        self.next = n + 1;
        format!("{kind}-{n:04}")
    }
}

/// One proposal a version applies, as `replay::Change` holds it (that type is not serialisable).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeSnap {
    /// The proposal: `p-0001`.
    pub id: String,
    /// `domain`, `rule`, `decision` or `new-rule`.
    pub target: String,
    /// The domain's name, the rule as `<domain>.<id>`, the decision's slug, or the new rule's domain.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drop: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub second: Option<(String, Vec<String>)>,
    #[serde(default)]
    pub retire: bool,
    /// The one-line description, for the report.
    pub describe: String,
}

impl ChangeSnap {
    pub fn of(id: &str, c: &Change) -> Self {
        let (target, name, new_text) = match &c.target {
            Target::Domain(d) => ("domain", d.clone(), None),
            Target::Rule(r) => ("rule", r.clone(), None),
            Target::Decision(s) => ("decision", s.clone(), None),
            Target::NewRule { domain, text } => ("new-rule", domain.clone(), Some(text.clone())),
        };
        ChangeSnap {
            id: id.to_string(),
            target: target.to_string(),
            name,
            new_text,
            add: c.add.clone(),
            drop: c.drop.clone(),
            text: c.text.clone(),
            merge: c.merge.clone(),
            second: c.second.clone(),
            retire: c.retire,
            describe: c.describe(),
        }
    }

    pub fn change(&self) -> Change {
        let target = match self.target.as_str() {
            "domain" => Target::Domain(self.name.clone()),
            "rule" => Target::Rule(self.name.clone()),
            "decision" => Target::Decision(self.name.clone()),
            _ => Target::NewRule { domain: self.name.clone(), text: self.new_text.clone().unwrap_or_default() },
        };
        Change {
            target,
            add: self.add.clone(),
            drop: self.drop.clone(),
            text: self.text.clone(),
            merge: self.merge.clone(),
            second: self.second.clone(),
            retire: self.retire,
        }
    }
}

/// One version of what a prompt is matched with (K9a): the matcher settings, the proposals it applies, and a content
/// hash over those and the configuration they apply to, so any change can be rolled back and the report can say when
/// live moved under a running candidate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Version {
    pub name: String,
    /// `keyword` (`[match] bm25 = false`), `bm25`, or `proposals`.
    pub kind: String,
    /// Every `[match]` key.
    pub settings: MatchConfig,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposals: Vec<ChangeSnap>,
    /// SHA-256 over the settings, the proposals, every domain's prompt keywords, file keywords and paths, every rule in
    /// force with its domain and matchers, and every global decision's keywords.
    pub hash: String,
    /// When it was made (RFC 3339).
    pub created: String,
}

impl Version {
    pub fn load(dir: &Path, name: &str) -> Option<Self> {
        if !safe_name(name) {
            return None;
        }
        let text = std::fs::read_to_string(dir.join(VERSIONS).join(format!("{name}.json"))).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir.join(VERSIONS)).map_err(|e| format!("{}: {e}", dir.join(VERSIONS).display()))?;
        write_json(&dir.join(VERSIONS), &format!("{}.json", self.name), self)
    }

    /// The proposals as changes.
    pub fn changes(&self) -> Vec<Change> {
        self.proposals.iter().map(ChangeSnap::change).collect()
    }

    pub fn is_proposals(&self) -> bool {
        !self.proposals.is_empty()
    }
}

/// A version name base made: `<kind>-<digits>`, nothing that could leave the folder.
pub fn safe_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// `keyword` or `bm25`: what a matcher setting is called in a version's name.
pub fn matcher_kind(m: &MatchConfig) -> &'static str {
    if m.bm25 { "bm25" } else { "keyword" }
}

fn write_json<T: Serialize>(dir: &Path, file: &str, value: &T) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(file);
    let tmp = dir.join(format!("{file}.tmp-{}", std::process::id()));
    let json = serde_json::to_string_pretty(value).map_err(|e| format!("{}: {e}", path.display()))?;
    std::fs::write(&tmp, json).map_err(|e| format!("{}: {e}", tmp.display()))?;
    crate::store::rename_with_retry(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", path.display())
    })
}

/// The lock every change to the state takes, waiting up to `wait` for it. `None` when it could not be had: a command
/// says so, session start passes by.
pub fn lock(dir: &Path, wait: std::time::Duration) -> Option<std::fs::File> {
    std::fs::create_dir_all(dir).ok()?;
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join(LOCK)).ok()?;
    let until = std::time::Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Some(file),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < until => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(_) => return None,
        }
    }
}

// ─── What the hooks read ───────────────────────────────────────────────────

/// A candidate that runs, as a hook needs it.
#[derive(Debug, Clone)]
pub struct Active {
    pub version: Version,
}

/// The candidate running now, read by every prompt and tool hook: one failed file open when no shadow was ever started
/// (the seamless upgrade), two small reads while one runs.
pub fn active() -> Option<Active> {
    let dir = dir()?;
    let text = std::fs::read_to_string(dir.join(STATE)).ok()?;
    let state: State = serde_json::from_str(&text).ok()?;
    let name = state.candidate?.name;
    Version::load(&dir, &name).map(|version| Active { version })
}

/// The candidate's `[match]`, when one runs: the index refreshes count what it needs as well as what live needs, so a
/// BM25 candidate beside a keyword-only live is scored on a current index.
pub fn candidate_settings() -> Option<MatchConfig> {
    active().map(|a| a.version.settings)
}

/// Does live or the running candidate score with BM25, and does either admit with the prompt IDF.
pub fn index_wanted(config: &BaseConfig) -> (bool, bool) {
    let live = &config.matching;
    let cand = candidate_settings();
    let bm25 = live.bm25 || cand.as_ref().is_some_and(|c| c.bm25);
    let idf = (live.bm25 && live.prompt_idf) || cand.as_ref().is_some_and(|c| c.bm25 && c.prompt_idf);
    (bm25, idf)
}

// ─── The content hash (K9a) ────────────────────────────────────────────────

/// The hash a version carries: its settings and proposals, and the configuration as `cwd` reads it now.
pub fn content_hash(config: &BaseConfig, cwd: &Path, settings: &MatchConfig, proposals: &[ChangeSnap]) -> String {
    let domains = crate::domain::load_domains(cwd);
    let store = crate::store::load_merged(cwd);
    hash_of(config, &domains, store.as_ref(), settings, proposals)
}

/// [`content_hash`] on what a caller has loaded.
pub fn hash_of(
    config: &BaseConfig,
    domains: &[crate::domain::DomainDef],
    store: Option<&oxigraph::store::Store>,
    settings: &MatchConfig,
    proposals: &[ChangeSnap],
) -> String {
    let mut h = Sha256::new();
    let mut put = |label: &str, value: &str| {
        h.update(label.as_bytes());
        h.update(b"\x1f");
        h.update(value.as_bytes());
        h.update(b"\x1e");
    };
    for (k, v) in settings.keys() {
        put(k, &v.map(|v| v.to_string()).unwrap_or_default());
    }
    for p in proposals {
        put("proposal", &serde_json::to_string(p).unwrap_or_default());
    }
    let mut sorted: Vec<&crate::domain::DomainDef> = domains.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    for d in sorted {
        put("domain", &d.name);
        put("keywords", &d.prompt_keywords.join("\u{1d}"));
        put("file_keywords", &d.file_keywords.join("\u{1d}"));
        put("paths", &d.paths.join("\u{1d}"));
        let mut ids: Vec<String> =
            crate::domain::rules::rules_for_domain(store, config, d).into_iter().map(|r| r.id).collect();
        ids.sort();
        put("rules", &ids.join("\u{1d}"));
    }
    let mut converted: Vec<String> = crate::domain::rules::rules_with_matchers(store, config, domains)
        .into_iter()
        .map(|c| format!("{}:{}:{}", c.rule.domain, c.rule.id, serde_json::to_string(&c.matchers).unwrap_or_default()))
        .collect();
    converted.sort();
    put("matchers", &converted.join("\u{1d}"));
    if let Some(store) = store {
        let global = crate::domain::global_decisions::GlobalDecisions::load(store, config, domains);
        let mut decisions: Vec<String> = global.all().map(|d| format!("{}:{}", d.slug, d.keywords.join(","))).collect();
        decisions.sort();
        put("decisions", &decisions.join("\u{1d}"));
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

// ─── Starting and stopping ─────────────────────────────────────────────────

/// What `base shadow start` runs as the candidate.
#[derive(Debug, Clone)]
pub enum Start {
    /// A matcher: live's `[match]` with these changed.
    Matcher { bm25: bool, min_score: Option<f32>, prompt_idf: bool, min_terms: Option<usize>, relative: Option<f32> },
    /// Pending proposals, applied in memory; live's `[match]` kept.
    Proposals(Vec<String>),
}

/// The live version for `config` as `cwd` reads it: the one the state names when its hash still holds, else a new one.
fn live_version(config: &BaseConfig, cwd: &Path, state: &mut State, dir: &Path) -> Result<Version, String> {
    let hash = content_hash(config, cwd, &config.matching, &[]);
    if let Some(v) = state.live.as_deref().and_then(|n| Version::load(dir, n))
        && v.hash == hash
        && v.settings == config.matching
        && !v.is_proposals()
    {
        return Ok(v);
    }
    let v = Version {
        name: state.name(matcher_kind(&config.matching)),
        kind: matcher_kind(&config.matching).to_string(),
        settings: config.matching.clone(),
        proposals: Vec::new(),
        hash,
        created: now(),
    };
    v.save(dir)?;
    state.live = Some(v.name.clone());
    Ok(v)
}

/// `base shadow start`: snapshot live, make the candidate's version, and start it. Refused while one runs.
pub fn start(config: &BaseConfig, cwd: &Path, what: &Start) -> Result<String, String> {
    let dir = dir().ok_or("no home directory, so no global tier to keep the shadow in")?;
    let _lock = lock(&dir, std::time::Duration::from_secs(10)).ok_or("the shadow state is locked by another base; try again")?;
    let mut state = State::load(&dir);
    if let Some(c) = &state.candidate {
        return Err(format!(
            "candidate {} is already running (since {}): one shadow at a time. Stop it first with base shadow stop",
            c.name,
            day_of(&c.since)
        ));
    }
    if let Some(over) = workspace_match_override(cwd) {
        return Err(format!(
            "{over} sets its own [match], so this workspace's prompts are not matched as base.toml in ~/.base-gbl says: a \
             shadow compares against the global [match] alone. Remove [match] from that file first"
        ));
    }
    let live = live_version(config, cwd, &mut state, &dir)?;
    let (kind, settings, proposals) = match what {
        Start::Matcher { bm25, min_score, prompt_idf, min_terms, relative } => {
            let mut s = config.matching.clone();
            s.bm25 = *bm25;
            if min_score.is_some() {
                s.min_score = *min_score;
            }
            s.prompt_idf |= *prompt_idf;
            if let Some(n) = min_terms {
                s.min_terms = (*n).max(1);
            }
            if relative.is_some() {
                s.relative = *relative;
            }
            if s == config.matching {
                return Err(format!(
                    "that candidate is live's own [match] ({}): nothing would differ. Name a matcher or a setting live does \
                     not have",
                    settings_line(&s)
                ));
            }
            (matcher_kind(&s).to_string(), s, Vec::new())
        }
        Start::Proposals(ids) => {
            let snaps = pending_changes(config, cwd, ids)?;
            ("proposals".to_string(), config.matching.clone(), snaps)
        }
    };
    let candidate = Version {
        name: state.name(&kind),
        kind,
        hash: content_hash(config, cwd, &settings, &proposals),
        settings,
        proposals,
        created: now(),
    };
    candidate.save(&dir)?;
    state.candidate = Some(Running {
        name: candidate.name.clone(),
        against: live.name.clone(),
        since: now(),
        live_hash: live.hash.clone(),
    });
    state.ready_told = None;
    state.save(&dir)?;
    // A candidate that scores needs a current index, and a proposals candidate its own: counted now, not at the next
    // session start.
    run::refresh_indexes(config, cwd);
    let mut out = format!("started candidate {} beside live {}\n", candidate.name, live.name);
    out.push_str(&format!("  candidate: {}\n", describe(&candidate)));
    out.push_str(&format!("  live:      {}\n", describe(&live)));
    out.push_str(&format!(
        "  every prompt and file touch now runs both; only live's pick is served. Progress: base shadow report · end it: \
         base shadow stop\n"
    ));
    Ok(out)
}

/// `base shadow stop`: end the candidate without promoting it. Deletes nothing but the state's candidate (lynx's Q11
/// condition): its version file and its rows stay, so a report or a later `promote <version>` still reads them.
pub fn stop() -> Result<String, String> {
    let dir = dir().ok_or("no home directory, so no shadow")?;
    let _lock = lock(&dir, std::time::Duration::from_secs(10)).ok_or("the shadow state is locked by another base; try again")?;
    let mut state = State::load(&dir);
    let Some(c) = state.candidate.take() else {
        return Ok("no shadow is running; nothing stopped\n".to_string());
    };
    state.ready_told = None;
    state.save(&dir)?;
    Ok(format!(
        "stopped candidate {} (running against {} since {}); live is unchanged. Its rows stay in the match log and its \
         version stays: base shadow promote {} still promotes it\n",
        c.name,
        c.against,
        day_of(&c.since),
        c.name
    ))
}

/// The workspace `base.toml` that sets `[match]` for `cwd`, when one does: the hooks there read it over the global one.
pub fn workspace_match_override(cwd: &Path) -> Option<String> {
    let path = cwd.join(".base").join("base.toml");
    let table: toml::Table = std::fs::read_to_string(&path).ok()?.parse().ok()?;
    table.contains_key("match").then(|| path.display().to_string())
}

/// The pending proposals `ids` names, as changes, or why not.
fn pending_changes(config: &BaseConfig, cwd: &Path, ids: &[String]) -> Result<Vec<ChangeSnap>, String> {
    if ids.is_empty() {
        return Err("--from-proposals needs proposal ids: p-0001,p-0002 (base rule review lists them)".to_string());
    }
    let all = crate::corrections::review::load(config, cwd);
    let mut out = Vec::new();
    for raw in ids {
        let want = crate::corrections::review::normalize_id(raw);
        let p = all.iter().find(|p| p.id == want).ok_or_else(|| format!("no proposal {want} (base rule review lists them)"))?;
        if p.status != "pending" {
            return Err(format!("{} is {}, not pending: only a pending proposal can run as a candidate", p.id, p.status));
        }
        let change = p.change()?;
        out.push(ChangeSnap::of(&p.id, &change));
    }
    Ok(out)
}

/// `bm25 · min_score 6 · prompt_idf`: the settings a version runs with, as one line.
pub fn settings_line(s: &MatchConfig) -> String {
    let mut parts: Vec<String> = vec![if s.bm25 { "bm25".to_string() } else { "keyword-only".to_string() }];
    if let Some(m) = s.min_score {
        parts.push(format!("min_score {m}"));
    }
    if s.prompt_idf {
        parts.push("prompt_idf".to_string());
    }
    if s.min_terms > 1 {
        parts.push(format!("min_terms {}", s.min_terms));
    }
    if let Some(r) = s.relative {
        parts.push(format!("relative {r}"));
    }
    parts.join(" · ")
}

/// A version in one line: its settings, or the proposals it applies.
pub fn describe(v: &Version) -> String {
    if v.is_proposals() {
        let list: Vec<String> = v.proposals.iter().map(|p| format!("{} ({})", p.id, p.describe)).collect();
        return format!("proposals {}", list.join("; "));
    }
    settings_line(&v.settings)
}

pub fn now() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

/// `2026-10-05` from an RFC 3339 time.
pub fn day_of(ts: &str) -> String {
    ts.chars().take(10).collect()
}
