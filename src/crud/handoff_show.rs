//! The handoff list session start shows (spec B4, B5), and `base handoff show`, which finds one
//! handoff from what a person says (board ruling R7).
//!
//! Chris never answers a session start with a letter. He says "get the handoff for the skyrim
//! thing". So `show` takes a letter, a slug, a project name or a few loose words, prints the doc
//! to read and what it matched, and when several handoffs match it lists them and picks none. It
//! writes only to revive: one match that is deferred comes back to open, and an open match, several
//! matches or none write nothing (lane 3 verdicts, AMENDMENTS C).
//!
//! One selection serves session start and the letter path of `show`. The letters a session start
//! printed are kept in a file beside its full output, so a letter names the handoff that session
//! was shown even after a newer handoff is registered, which in a respawning chain is minutes later.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Result;
use oxigraph::sparql::QueryResults;
use oxigraph::store::Store;
use serde::{Deserialize, Serialize};

use crate::config::{DeferKind, NamespaceConfig, SessionStartConfig};
use crate::crud;

/// Spec B4: letters A to J, so never more than ten handoffs are listed.
pub const MAX_SHOWN: usize = 10;

/// The letters the last session start printed, kept beside its full output.
pub const LETTERS_FILE: &str = "last-session-start-letters.json";

/// One open handoff. Forks share the record type and are never in this list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenHandoff {
    pub slug: String,
    pub project: String,
    pub doc: String,
    /// `createdAt` as stored.
    pub created: String,
    /// The tier whose file holds it, when the rows were read one tier at a time.
    pub tier: Option<String>,
}

impl OpenHandoff {
    /// Whole days since it was created. A date that does not parse counts as 0, as the old list did.
    pub fn age_days(&self, now: chrono::DateTime<chrono::Local>) -> i64 {
        chrono::DateTime::parse_from_rfc3339(&self.created)
            .map(|dt| now.signed_duration_since(dt).num_days())
            .unwrap_or(0)
    }
}

/// Open handoffs in `store`, newest created first, the slug breaking ties. `due_only` keeps the
/// ones whose `resurfaceAt` has passed, which is what session start lists; `show` also searches
/// the snoozed ones.
pub fn open_handoffs(
    store: &Store,
    ns: &NamespaceConfig,
    due_only: bool,
    tier: Option<&str>,
) -> Result<Vec<OpenHandoff>> {
    let p = &ns.prefix;
    let due = if due_only {
        let now = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        format!(
            "             ?h {p}:resurfaceAt ?resurfaceAt .\n\
             FILTER(?resurfaceAt <= \"{now}\"^^xsd:dateTime)\n"
        )
    } else {
        String::new()
    };
    let sparql = format!(
        "{pfx}\nSELECT ?h ?project ?doc ?created WHERE {{\n\
           GRAPH ?g {{\n\
             ?h a {p}:Handoff ;\n\
               {p}:status \"open\" ;\n\
               {p}:project ?project ;\n\
               {p}:handoffDoc ?doc ;\n\
               {p}:createdAt ?created .\n\
{due}\
             OPTIONAL {{ ?h {p}:kind ?kind }}\n\
             FILTER(!BOUND(?kind) || ?kind != \"fork\")\n\
           }}\n\
         }}\n\
         ORDER BY DESC(?created) ?h",
        pfx = crud::prefixes(ns)
    );

    let QueryResults::Solutions(solutions) = crate::store::query(store, &sparql)? else {
        return Ok(Vec::new());
    };
    // A subject holding two values for one field comes back as two rows; the first stands.
    let mut seen = HashSet::new();
    let mut rows = Vec::new();
    for row in solutions.filter_map(|r| r.ok()) {
        let get = |k: &str| {
            row.get(k)
                .map(|t| crud::term_display(t.into()))
                .unwrap_or_default()
        };
        let h = get("h");
        let slug = h.rsplit('/').next().unwrap_or(&h).to_string();
        if !seen.insert(slug.clone()) {
            continue;
        }
        rows.push(OpenHandoff {
            slug,
            project: get("project"),
            doc: get("doc"),
            created: get("created"),
            tier: tier.map(str::to_string),
        });
    }
    Ok(rows)
}

/// One lettered line of the session start list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lettered {
    pub letter: char,
    pub handoff: OpenHandoff,
    /// Open handoffs for the same project, older than this one, that are not listed (spec B5).
    pub older: usize,
}

/// The session start list: how many handoffs are open, and the lettered ones shown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HandoffList {
    pub open: usize,
    pub shown: Vec<Lettered>,
}

impl HandoffList {
    pub fn letters(&self) -> Vec<(char, String)> {
        self.shown
            .iter()
            .map(|l| (l.letter, l.handoff.slug.clone()))
            .collect()
    }
}

/// Spec B4 and B5 over rows already sorted newest first: one handoff per project with the older
/// ones counted on its line, at most `handoffs_shown` and never more than ten, lettered from A.
///
/// A child handoff folding under its parent (B5's second bullet) is not here: no record carries a
/// parent link, and E3, which adds one, is lane 2's (lane doc B16, flag 1).
pub fn session_start_list(rows: Vec<OpenHandoff>, cfg: &SessionStartConfig) -> HandoffList {
    let open = rows.len();
    let mut firsts: Vec<(OpenHandoff, usize)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for h in rows {
        if cfg.one_per_project {
            if let Some(&i) = at.get(&h.project) {
                firsts[i].1 += 1;
                continue;
            }
            at.insert(h.project.clone(), firsts.len());
        }
        firsts.push((h, 0));
    }
    let shown = firsts
        .into_iter()
        .take(cfg.handoffs_shown.min(MAX_SHOWN))
        .zip(b'A'..)
        .map(|((handoff, older), letter)| Lettered {
            letter: letter as char,
            handoff,
            older,
        })
        .collect();
    HandoffList { open, shown }
}

/// The codename in a slug shaped `YYYY-MM-DD-HHMM-<codename>-<rest>`. A handoff record stores no
/// codename, so the slug is the only place one can come from. A slug of any other shape has none,
/// and its line prints the slug unchanged rather than a guessed name (`auk`'s ruling on flag 2).
pub fn codename_of(slug: &str) -> Option<&str> {
    let mut parts = slug.splitn(6, '-');
    let digits = |s: Option<&str>, n: usize| {
        s.is_some_and(|s| s.len() == n && s.bytes().all(|b| b.is_ascii_digit()))
    };
    if !(digits(parts.next(), 4)
        && digits(parts.next(), 2)
        && digits(parts.next(), 2)
        && digits(parts.next(), 4))
    {
        return None;
    }
    let code = parts.next()?;
    let rest = parts.next()?;
    (!code.is_empty() && !rest.is_empty() && code.bytes().all(|b| b.is_ascii_alphanumeric()))
        .then_some(code)
}

/// Where session start keeps its full output and its letters: the workspace `.base` when one
/// resolves, else the global tier's `.base` when it exists. Never created here.
pub fn session_start_dir(cwd: &Path) -> Option<PathBuf> {
    crate::config::find_workspace_base(cwd)
        .or_else(|| crate::config::global_base_dir().filter(|dir| dir.is_dir()))
}

#[derive(Serialize, Deserialize)]
struct LettersFile {
    written_at: String,
    letters: BTreeMap<String, String>,
}

/// Keep the letters session start printed, so `show <letter>` names the handoff that session saw.
/// Written through a temp file and a rename; a failure comes back as a value, never a panic.
pub fn write_letters(dir: &Path, letters: &[(char, String)]) -> crate::emit::FullOutput {
    let file = LettersFile {
        written_at: crud::now_iso(),
        letters: letters
            .iter()
            .map(|(l, slug)| (l.to_string(), slug.clone()))
            .collect(),
    };
    let text = serde_json::to_string_pretty(&file).unwrap_or_default();
    crate::emit::write_full_output(&dir.join(LETTERS_FILE), &text)
}

enum Letters {
    Absent,
    Unreadable(String),
    Read {
        written_at: String,
        map: BTreeMap<String, String>,
    },
}

fn read_letters(dir: Option<&Path>) -> Letters {
    let Some(dir) = dir else {
        return Letters::Absent;
    };
    let path = dir.join(LETTERS_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Letters::Absent,
        Err(e) => return Letters::Unreadable(format!("{}: {e}", path.display())),
    };
    match serde_json::from_str::<LettersFile>(&text) {
        Ok(file) => Letters::Read {
            written_at: file.written_at,
            map: file.letters,
        },
        Err(e) => Letters::Unreadable(format!("{}: {e}", path.display())),
    }
}

/// How `show` chose what it printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rule {
    /// A letter. `written_at` is when the session start that printed it ran; `None` means no
    /// letters file was usable and the list was rebuilt now.
    Letter {
        letter: char,
        written_at: Option<String>,
    },
    /// A key from the last `base handoff deferred` or `base fork deferred` (flag 5). `written_at` is
    /// empty when the keys file could not be used.
    Key {
        key: String,
        written_at: String,
    },
    Slug,
    Project,
    /// The handoffs sharing the most words with the query: `shared` of the query's `of` words.
    Words {
        shared: usize,
        of: usize,
    },
}

/// A deferred record's reason and date, read beside it so `show` can say what it brought back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parked {
    pub reason: Option<String>,
    pub deferred_at: Option<String>,
}

/// One record `show` can answer with: open, or deferred and carrying its [`Parked`] facts.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub handoff: OpenHandoff,
    pub parked: Option<Parked>,
}

/// What `show` found. One match is an answer, several are a question, none is a miss.
#[derive(Debug, Clone)]
pub struct Resolution {
    pub query: String,
    /// `handoff` or `fork`: the kind searched, and the word every line uses.
    pub noun: &'static str,
    pub matches: Vec<Candidate>,
    pub rule: Option<Rule>,
    /// Said beside the answer: a letter rebuilt now, a key refused, a letter whose record is gone.
    pub notes: Vec<String>,
    /// The tier files searched, named when nothing matched.
    pub searched: Vec<String>,
    /// Set by [`Resolution::revive_if_deferred`] when the one match was deferred and is open now.
    pub revived: Option<Parked>,
}

/// A query of exactly one letter from A to J, in either case.
fn as_letter(query: &str) -> Option<char> {
    let mut chars = query.chars();
    let c = chars.next()?.to_ascii_uppercase();
    (chars.next().is_none() && ('A'..='J').contains(&c)).then_some(c)
}

fn words(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The open and deferred handoffs, or forks, in one store: everything `show` may answer with. Forks share
/// the record type and are told apart by their `kind` literal, as `open_handoffs` tells them apart.
fn candidates(store: &Store, ns: &NamespaceConfig, fork: bool, tier: &str) -> Result<Vec<Candidate>> {
    let p = &ns.prefix;
    let kind = if fork {
        format!("?h {p}:kind \"fork\" .")
    } else {
        format!("OPTIONAL {{ ?h {p}:kind ?kind }}\n             FILTER(!BOUND(?kind) || ?kind != \"fork\")")
    };
    let sparql = format!(
        "{pfx}\nSELECT ?h ?status ?project ?doc ?created ?why ?at WHERE {{\n\
           GRAPH ?g {{\n\
             ?h a {p}:Handoff ;\n\
               {p}:status ?status ;\n\
               {p}:project ?project ;\n\
               {p}:handoffDoc ?doc ;\n\
               {p}:createdAt ?created .\n\
             FILTER(?status IN (\"open\", \"{deferred}\"))\n\
             {kind}\n\
             OPTIONAL {{ ?h {p}:deferredReason ?why }}\n\
             OPTIONAL {{ ?h {p}:deferredAt ?at }}\n\
           }}\n\
         }}\n\
         ORDER BY DESC(?created) ?h",
        pfx = crud::prefixes(ns),
        deferred = crud::deferred::DEFERRED
    );
    let QueryResults::Solutions(solutions) = crate::store::query(store, &sparql)? else {
        return Ok(Vec::new());
    };
    let mut seen = HashSet::new();
    let mut rows = Vec::new();
    for row in solutions.filter_map(|r| r.ok()) {
        let get = |k: &str| {
            row.get(k)
                .map(|t| crud::term_display(t.into()))
                .unwrap_or_default()
        };
        let h = get("h");
        let slug = h.rsplit('/').next().unwrap_or(&h).to_string();
        // A subject holding two values for one field comes back as two rows; the first stands.
        if !seen.insert(slug.clone()) {
            continue;
        }
        let opt = |k: &str| Some(get(k)).filter(|s| !s.is_empty());
        let parked = (get("status") == crud::deferred::DEFERRED).then(|| Parked {
            reason: opt("why"),
            deferred_at: opt("at"),
        });
        rows.push(Candidate {
            handoff: OpenHandoff {
                slug,
                project: get("project"),
                doc: get("doc"),
                created: get("created"),
                tier: Some(tier.to_string()),
            },
            parked,
        });
    }
    Ok(rows)
}

/// Find the handoff, or with `fork` the fork, a person means: open or deferred, in every tier file.
/// Reads only. Bringing a deferred one-match back is [`Resolution::revive_if_deferred`], the one write.
pub fn resolve(
    gbl_root: Option<&Path>,
    cwd: &Path,
    ns: &NamespaceConfig,
    cfg: &SessionStartConfig,
    fork: bool,
    query: &str,
) -> Result<Resolution> {
    let query = query.trim().to_string();
    let noun = if fork { "fork" } else { "handoff" };
    let mut all: Vec<Candidate> = Vec::new();
    for file in crud::all_tier_files(gbl_root, cwd) {
        let store = crate::store::load_or_empty(&file)?;
        let tier = crud::tier_label_of_file(&file, gbl_root);
        all.extend(candidates(&store, ns, fork, tier)?);
    }
    let mut out = Resolution {
        query: query.clone(),
        noun,
        matches: Vec::new(),
        rule: None,
        notes: Vec::new(),
        searched: crud::handoff::searched_tiers(gbl_root, cwd),
        revived: None,
    };

    // A key from the last deferred listing is a lookup, verified again here and never trusted alone
    // (flag 5b): the slug must still exist and still be deferred, or the key is refused.
    if let Some(key) = crud::deferred::as_key(&query) {
        let kind = if fork { DeferKind::Fork } else { DeferKind::Handoff };
        match crud::deferred::key_slug(kind, &key) {
            Ok((slug, written_at)) => {
                out.matches = all
                    .iter()
                    .filter(|c| c.handoff.slug == slug && c.parked.is_some())
                    .cloned()
                    .collect();
                if out.matches.is_empty() {
                    out.notes.push(format!(
                        "key {key} was {slug}, which is no longer a deferred {noun}; re-list: base {noun} deferred"
                    ));
                }
                out.rule = Some(Rule::Key { key, written_at });
            }
            Err(why) => {
                out.notes.push(why);
                out.rule = Some(Rule::Key {
                    key,
                    written_at: String::new(),
                });
            }
        }
        return Ok(out);
    }

    if !fork && let Some(letter) = as_letter(&query) {
        let dir = session_start_dir(cwd);
        let (slug, written_at) = match read_letters(dir.as_deref()) {
            Letters::Read { written_at, map } => {
                (map.get(&letter.to_string()).cloned(), Some(written_at))
            }
            unusable => {
                let why = match unusable {
                    Letters::Unreadable(why) => format!("the letters file is unreadable ({why})"),
                    _ => "no session start here left a letters file".to_string(),
                };
                out.notes.push(format!(
                    "{why}, so the letters were rebuilt now and can differ from the ones a session start printed"
                ));
                let rebuilt = match crate::store::load_merged(cwd) {
                    Some(store) => open_handoffs(&store, ns, true, None)?,
                    None => Vec::new(),
                };
                let slug = session_start_list(rebuilt, cfg)
                    .letters()
                    .into_iter()
                    .find(|(l, _)| *l == letter)
                    .map(|(_, slug)| slug);
                (slug, None)
            }
        };
        out.rule = Some(Rule::Letter { letter, written_at });
        match slug {
            Some(slug) => {
                out.matches = all.iter().filter(|c| c.handoff.slug == slug).cloned().collect();
                if out.matches.is_empty() {
                    out.notes.push(format!(
                        "letter {letter} was {slug}, which is no longer an open or deferred handoff"
                    ));
                }
            }
            None => out.notes.push(format!("no handoff had letter {letter}")),
        }
        return Ok(out);
    }

    let by_slug: Vec<Candidate> = all.iter().filter(|c| c.handoff.slug == query).cloned().collect();
    if !by_slug.is_empty() {
        out.matches = by_slug;
        out.rule = Some(Rule::Slug);
        return Ok(out);
    }

    let wanted = query.to_lowercase();
    let by_project: Vec<Candidate> = all
        .iter()
        .filter(|c| c.handoff.project.to_lowercase() == wanted)
        .cloned()
        .collect();
    if !by_project.is_empty() {
        out.matches = by_project;
        out.rule = Some(Rule::Project);
        return Ok(out);
    }

    let asked = words(&query);
    let scored: Vec<(usize, &Candidate)> = all
        .iter()
        .map(|c| {
            let h = &c.handoff;
            let mut have = words(&h.slug);
            have.extend(words(&h.project));
            have.extend(words(
                Path::new(&h.doc)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(""),
            ));
            (asked.intersection(&have).count(), c)
        })
        .collect();
    let best = scored.iter().map(|(n, _)| *n).max().unwrap_or(0);
    if best > 0 {
        out.matches = scored
            .into_iter()
            .filter(|(n, _)| *n == best)
            .map(|(_, c)| c.clone())
            .collect();
        out.rule = Some(Rule::Words {
            shared: best,
            of: asked.len(),
        });
    }
    Ok(out)
}

impl Resolution {
    /// 0 for one match, 2 for several, 1 for none.
    pub fn exit_code(&self) -> i32 {
        match self.matches.len() {
            1 => 0,
            0 => 1,
            _ => 2,
        }
    }

    /// When the one match is deferred, bring it back to open in every tier holding it and remember what it
    /// was. An open match, several matches and none write nothing (verdicts, AMENDMENTS C): a question never
    /// revives anything, and an open record's clock moves through the Read of the doc this command prints.
    pub fn revive_if_deferred(&mut self, gbl_root: Option<&Path>, cwd: &Path, ns: &NamespaceConfig) -> Result<()> {
        let [one] = self.matches.as_slice() else {
            return Ok(());
        };
        let Some(parked) = one.parked.clone() else {
            return Ok(());
        };
        let slug = one.handoff.slug.clone();
        let written = crud::deferred::revive_handoff(gbl_root, cwd, ns, &slug)?;
        if written.is_empty() {
            anyhow::bail!(
                "{} {slug} matched as deferred, but no tier held it when the revival ran; nothing was written",
                self.noun
            );
        }
        self.revived = Some(parked);
        Ok(())
    }

    fn rule_text(&self) -> String {
        match &self.rule {
            Some(Rule::Letter {
                letter,
                written_at: Some(at),
            }) => format!("letter {letter} from the session start at {at}"),
            Some(Rule::Letter {
                letter,
                written_at: None,
            }) => format!("letter {letter}, from a list rebuilt now"),
            Some(Rule::Key { key, written_at }) => {
                format!("key {key} from base {} deferred at {written_at}", self.noun)
            }
            Some(Rule::Slug) => "the slug".to_string(),
            Some(Rule::Project) => "the project name".to_string(),
            Some(Rule::Words { shared, of }) => format!("{shared} of {of} words"),
            None => "nothing".to_string(),
        }
    }

    /// What `base handoff show` and `base fork show` print. The doc path is the first line of an answer.
    pub fn render(&self, now: chrono::DateTime<chrono::Local>) -> String {
        let noun = self.noun;
        let line = |c: &Candidate| {
            let h = &c.handoff;
            format!(
                "{} · project {} · {} · {}d",
                h.slug,
                h.project,
                h.tier.as_deref().unwrap_or("tier not read"),
                h.age_days(now)
            )
        };
        let mut s = String::new();
        match self.matches.as_slice() {
            [one] => {
                let _ = writeln!(s, "doc: {}", one.handoff.doc);
                let _ = writeln!(s, "{noun}: {}", line(one));
                let _ = writeln!(s, "matched: {}", self.rule_text());
                if let Some(p) = &self.revived {
                    let why = p.reason.as_deref().unwrap_or("no reason recorded");
                    let at = p
                        .deferred_at
                        .as_deref()
                        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok());
                    match at {
                        Some(at) => {
                            let days = now.signed_duration_since(at).num_days();
                            let _ = writeln!(s, "revived: deferred {days} days ago ({why})");
                        }
                        None => {
                            let _ = writeln!(s, "revived: was deferred ({why})");
                        }
                    }
                }
            }
            [] => {
                if let Some(Rule::Key { key, .. }) = &self.rule {
                    let _ = writeln!(s, "key {key} does not name a deferred {noun} now");
                } else {
                    let _ = writeln!(s, "no open {noun} matches \"{}\"", self.query);
                    let _ = writeln!(s, "no deferred {noun} matches either");
                }
                let _ = writeln!(s, "searched: {}", self.searched.join("; "));
                let _ = writeln!(s, "every {noun}, open or not: base {noun} list");
            }
            many => {
                let parked = many.iter().filter(|c| c.parked.is_some()).count();
                let (open_word, parked_note) = if parked == 0 {
                    ("open ", String::new())
                } else {
                    ("", format!(" ({parked} deferred)"))
                };
                let _ = writeln!(
                    s,
                    "{} {open_word}{noun}s match \"{}\"{parked_note} by {}. None was picked; run base {noun} show <slug> for one:",
                    many.len(),
                    self.query,
                    self.rule_text()
                );
                for c in many {
                    let tail = if c.parked.is_some() { " · deferred" } else { "" };
                    let _ = writeln!(s, "  {}{tail}", line(c));
                }
            }
        }
        for note in &self.notes {
            let _ = writeln!(s, "note: {note}");
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(slug: &str, project: &str) -> OpenHandoff {
        OpenHandoff {
            slug: slug.to_string(),
            project: project.to_string(),
            doc: format!("/docs/{slug}.md"),
            created: String::new(),
            tier: None,
        }
    }

    #[test]
    fn the_codename_is_read_off_the_slug_shape_and_nothing_is_guessed() {
        assert_eq!(
            codename_of("2026-09-14-1616-plover-base-0160"),
            Some("plover")
        );
        assert_eq!(codename_of("2026-08-01-1000-seed-handoff-0"), Some("seed"));
        // Not the shape: no codename, and the caller prints the slug itself.
        assert_eq!(codename_of("HANDOFF-2026-09-09-mantis"), None);
        assert_eq!(codename_of("2026-09-14-plover-base"), None, "no HHMM");
        assert_eq!(
            codename_of("2026-09-14-1616-plover"),
            None,
            "a codename needs a rest after it"
        );
        assert_eq!(
            codename_of("26-09-14-1616-plover-base"),
            None,
            "a two-digit year"
        );
    }

    #[test]
    fn one_handoff_per_project_the_older_counted_at_most_ten_lettered_from_a() {
        let mut rows = vec![h("s-new", "base"), h("s-old", "base")];
        for i in 0..12 {
            rows.push(h(&format!("other-{i}"), &format!("p{i}")));
        }
        let cfg = SessionStartConfig::default();
        let list = session_start_list(rows, &cfg);
        assert_eq!(list.open, 14);
        assert_eq!(list.shown.len(), 10);
        assert_eq!(list.shown[0].handoff.slug, "s-new");
        assert_eq!(
            list.shown[0].older, 1,
            "the older base handoff folds under the newer one"
        );
        assert_eq!(
            list.shown[1].handoff.slug, "other-0",
            "and takes no line of its own"
        );
        let letters: String = list.shown.iter().map(|l| l.letter).collect();
        assert_eq!(letters, "ABCDEFGHIJ");

        let wide = SessionStartConfig {
            handoffs_shown: 40,
            one_per_project: false,
            ..SessionStartConfig::default()
        };
        let rows = (0..14).map(|i| h(&format!("x-{i}"), "same")).collect();
        let list = session_start_list(rows, &wide);
        assert_eq!(
            list.shown.len(),
            MAX_SHOWN,
            "never more than ten, whatever the key says"
        );
        assert!(
            list.shown.iter().all(|l| l.older == 0),
            "no folding when one_per_project is off"
        );
    }

    #[test]
    fn a_letter_is_one_character_from_a_to_j() {
        assert_eq!(as_letter("a"), Some('A'));
        assert_eq!(as_letter("J"), Some('J'));
        assert_eq!(as_letter("K"), None);
        assert_eq!(as_letter("ab"), None);
        assert_eq!(as_letter(""), None);
    }

    #[test]
    fn words_split_on_everything_that_is_not_a_letter_or_digit() {
        let w = words("get the Handoff for skyrim-companion");
        for want in ["get", "the", "handoff", "for", "skyrim", "companion"] {
            assert!(w.contains(want), "{want} missing from {w:?}");
        }
        assert_eq!(w.len(), 6);
    }
}
