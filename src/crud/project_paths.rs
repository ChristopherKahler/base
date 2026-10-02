//! `base project paths` (P5): every project gets its real folder.
//!
//! `--suggest` writes nothing. It lists each project whose stored folder is missing, not a folder, too broad (it
//! holds two or more other projects, the same bar F29 holds a path trigger to) or contradicted by the project's own
//! docs, with a folder proposed from where those docs point and the count behind it. Two folders with evidence are
//! listed together, never chosen between. `--apply` writes a list the operator reviewed.
//!
//! Evidence, per project:
//! - its handoff and fork docs (`ops:Handoff` filed under the project's slug, or `<slug>-<topic>`): every absolute
//!   path the doc text names;
//! - its project docs: the `ops:Document` records of the project's domain, at their own paths;
//! - markers on a candidate folder: `.git` (a code repo), `.firm` (a Cadre firm), `.paul` (a PAUL project).
//!
//! A path inside a dot folder counts for the folder above it (`grazer/.worktrees/x/...` is `grazer`; `~/.base/forks`
//! is the home folder), so base's own store, a tool's cache and a worktree never become a project's folder; the same
//! for `AppData` and `node_modules`. The
//! home folder, its Documents, Desktop and Downloads, every registered workspace root, and any folder holding two or
//! more registered projects are containers: evidence stops below them and they are never proposed.
//!
//! WSL paths are never touched from Windows (`\\wsl.localhost\...`, `/home/...`): opening one starts the WSL
//! machine. A project stored at one is reported as not checked from this machine, not as missing.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use oxigraph::sparql::QueryResults;

use crate::config::BaseConfig;
use crate::crud;
use crate::crud::project::{PathRoots, ProjectRecord, Refused, cell_text};
use crate::domain::matcher::path_under;

/// One proposed folder and what points at it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Candidate {
    pub folder: String,
    /// Handoff and fork docs filed under the project that name a path inside the folder.
    pub handoff_docs: usize,
    /// The project's own docs in the graph that lie inside the folder.
    pub project_docs: usize,
    /// The folder's name is the project's slug or name (case, spaces and dashes aside, one letter of slack).
    pub named_like: bool,
    /// The project's stored path (a file, or a path into a dot folder) lies inside this folder.
    pub holds_stored: bool,
    /// `looks like a code repo` (`.git`), `a Cadre firm` (`.firm`), `a PAUL project` (`.paul`).
    pub marks: Vec<String>,
}

impl Candidate {
    fn docs(&self) -> usize {
        self.handoff_docs + self.project_docs
    }

    /// The evidence column: counts, then what the folder looks like.
    pub fn evidence(&self) -> String {
        let mut parts = Vec::new();
        if self.handoff_docs > 0 {
            parts.push(count(self.handoff_docs, "handoff or fork doc", "handoff and fork docs"));
        }
        if self.project_docs > 0 {
            parts.push(count(self.project_docs, "project doc", "project docs"));
        }
        let mut s = if !parts.is_empty() {
            format!("{} point inside it", parts.join(", "))
        } else if self.named_like {
            "named like the project; no doc points inside it".to_string()
        } else {
            "no doc points inside it".to_string()
        };
        if self.holds_stored {
            s.push_str("; the stored path is inside it");
        }
        if !self.named_like {
            s.push_str("; not named like the project");
        }
        for m in &self.marks {
            s.push_str(&format!(" ({m})"));
        }
        s
    }
}

fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

/// One project whose folder needs a decision.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Suggestion {
    pub project: String,
    /// The stored folder, resolved (F25b). `None`: the project has none.
    pub now: Option<String>,
    /// Why it is listed.
    pub reason: String,
    /// Best first. Empty: none found. Two or more: the operator picks.
    pub candidates: Vec<Candidate>,
}

/// Everything `--suggest` found.
#[derive(Debug, Default, serde::Serialize)]
pub struct Report {
    pub suggestions: Vec<Suggestion>,
    /// Projects whose folder stands.
    pub kept: Vec<String>,
    /// Projects whose folder this machine cannot look at (a WSL path read from Windows, and the reverse).
    pub not_checked: Vec<String>,
}

// ─── The filesystem, read once per path ──────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Dir,
    File,
    Missing,
}

/// A stored path as this machine can open it, or `None` when opening it is not this machine's to do: a WSL path
/// on Windows (opening `\\wsl.localhost\...` starts WSL), a Windows path on Linux with no `/mnt/<drive>`.
fn local(p: &str) -> Option<PathBuf> {
    let low = p.to_ascii_lowercase();
    if low.starts_with("//wsl.localhost/") || low.starts_with("//wsl$/") {
        return None;
    }
    if cfg!(windows) {
        if let Some(rest) = p.strip_prefix("/mnt/") {
            let mut it = rest.splitn(2, '/');
            let drive = it.next().filter(|d| d.len() == 1)?;
            return Some(PathBuf::from(format!("{}:/{}", drive.to_ascii_uppercase(), it.next().unwrap_or(""))));
        }
        if p.starts_with('/') && !p.starts_with("//") {
            return None;
        }
        Some(PathBuf::from(p))
    } else {
        let b = p.as_bytes();
        if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
            let mount = format!("/mnt/{}", (b[0] as char).to_ascii_lowercase());
            if !Path::new(&mount).is_dir() {
                return None;
            }
            return Some(PathBuf::from(format!("{mount}{}", &p[2..])));
        }
        if p.starts_with("//") {
            return None;
        }
        Some(PathBuf::from(p))
    }
}

/// One spelling for a Windows folder whatever machine reads it: `/mnt/c/x` is `C:/x`. Every folder this module
/// compares, tallies or proposes is in this form; [`local`] turns it into this machine's form only to open it.
fn drive_form(p: &str) -> String {
    let b = p.as_bytes();
    if p.starts_with("/mnt/") && b.len() >= 6 && b[5].is_ascii_alphabetic() && (b.len() == 6 || b[6] == b'/') {
        let rest = if b.len() > 7 { &p[7..] } else { "" };
        return format!("{}:/{rest}", (b[5] as char).to_ascii_uppercase());
    }
    p.to_string()
}

/// Is `p` a folder this machine can check, and not there? A WSL path read from Windows is not missing: unchecked.
pub(crate) fn is_missing(p: &str) -> bool {
    local(p).is_some_and(|lp| !lp.exists())
}

/// The `.paul/paul.toml` or `.paul/paul.json` that makes a project a PAUL project: inside its folder, or the stored
/// path itself when it names that file (`x/.paul/paul.json`, as `base sync` stored one before 0.16.0). Only a file
/// this machine can open counts.
pub(crate) fn paul_file(stored: &str) -> Option<String> {
    let t = stored.trim_end_matches('/');
    let folder = t.strip_suffix("/.paul/paul.json").or_else(|| t.strip_suffix("/.paul/paul.toml")).unwrap_or(t);
    ["paul.toml", "paul.json"]
        .iter()
        .map(|f| format!("{folder}/.paul/{f}"))
        .find(|f| local(f).is_some_and(|lp| lp.is_file()))
}

/// The project folder a PAUL file belongs to: the folder holding its `.paul`.
pub(crate) fn paul_folder(file: &str) -> Option<String> {
    let t = file.trim_end_matches('/');
    t.strip_suffix("/.paul/paul.json").or_else(|| t.strip_suffix("/.paul/paul.toml")).map(str::to_string)
}

/// A path the operator may write from Windows without this machine opening it: a WSL path. Any other `/`-rooted
/// path on Windows (`/Users/x`, a Git Bash `/c/x`) names no folder base can find, and is refused.
fn wsl_shaped(p: &str) -> bool {
    let low = p.to_ascii_lowercase();
    low.starts_with("//wsl.localhost/") || low.starts_with("//wsl$/") || p.starts_with("/home/") || p.starts_with("/root/")
}

#[derive(Default)]
struct Fs {
    kinds: HashMap<String, Option<Kind>>,
}

impl Fs {
    /// `None`: not this machine's to check.
    fn kind(&mut self, p: &str) -> Option<Kind> {
        if let Some(k) = self.kinds.get(p) {
            return *k;
        }
        let k = local(p).map(|lp| match std::fs::metadata(&lp) {
            Ok(m) if m.is_dir() => Kind::Dir,
            Ok(_) => Kind::File,
            Err(_) => Kind::Missing,
        });
        self.kinds.insert(p.to_string(), k);
        k
    }

    fn has(&mut self, folder: &str, child: &str) -> bool {
        local(folder).is_some_and(|lp| lp.join(child).exists())
    }

    /// The sub-folders of `folder`, as stored paths.
    fn children(&mut self, folder: &str) -> Vec<String> {
        let Some(lp) = local(folder) else { return Vec::new() };
        let Ok(rd) = std::fs::read_dir(lp) else { return Vec::new() };
        let mut out: Vec<String> = rd
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().to_str().map(|n| join(folder, n)))
            .collect();
        out.sort();
        out
    }
}

fn join(folder: &str, name: &str) -> String {
    if folder.ends_with('/') { format!("{folder}{name}") } else { format!("{folder}/{name}") }
}

fn parent(p: &str) -> Option<String> {
    let t = p.trim_end_matches('/');
    let i = t.rfind('/')?;
    let up = &t[..i];
    if up.is_empty() || up == "/" {
        return Some("/".to_string());
    }
    // `C:` alone is the drive root, `//server` alone is no folder at all.
    if up.len() == 2 && up.as_bytes()[1] == b':' {
        return Some(format!("{up}/"));
    }
    if up == "/" || (up.starts_with("//") && up[2..].find('/').is_none()) {
        return None;
    }
    Some(up.to_string())
}

fn last(p: &str) -> &str {
    p.trim_end_matches('/').rsplit('/').next().unwrap_or(p)
}

fn same(a: &str, b: &str) -> bool {
    path_under(a, b) && path_under(b, a)
}

/// `p` cut at its first dot folder, `AppData` (Windows' hidden app state) or `node_modules` (someone else's code): a
/// path inside `.base`, `.cache`, `.git` or a worktree folder counts for the folder that holds it. Below the home
/// folder or a workspace root (the deepest of `roots` holding `p`) only the parts after that root are looked at, so
/// a workspace that itself sits under a temp folder keeps its own paths; a path under none of them is looked at whole.
fn above_dot_folders(p: &str, roots: &[String]) -> String {
    let start = roots
        .iter()
        .filter(|r| path_under(p, r))
        .map(|r| r.split('/').filter(|c| !c.is_empty()).count())
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    let mut parts = 0;
    for (i, c) in p.split('/').enumerate() {
        if !c.is_empty() {
            parts += 1;
        }
        let hidden = (c.starts_with('.') && c != "." && c != "..") || c == "node_modules" || c.eq_ignore_ascii_case("AppData");
        if parts > start && hidden {
            break;
        }
        if i > 0 {
            out.push('/');
        }
        out.push_str(c);
    }
    if out.is_empty() || out.ends_with(':') { format!("{out}/") } else { out }
}

// ─── Names ───────────────────────────────────────────────────

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn edits(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push((prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Is a folder called `folder` named like the project `id` / `name`? Equal once case, spaces and dashes are gone
/// (`Secure Engineering Framework` is `secure-engineering-framework`), or one letter apart for names of five
/// letters or more (`Vintryx` is `vintrix`).
pub fn named_like(folder: &str, id: &str, name: &str) -> bool {
    let f = norm(folder);
    if f.is_empty() {
        return false;
    }
    [norm(id), norm(name)]
        .iter()
        .any(|n| !n.is_empty() && (*n == f || (n.chars().count() >= 5 && f.chars().count() >= 5 && edits(n, &f) <= 1)))
}

// ─── Paths in a doc's text ───────────────────────────────────

/// Every absolute path the text names: `C:/...`, `C:\...`, `~/...`, and `/a/b...` (two parts at least, after white
/// space, a quote or a bracket, so `and/or` and `http://x` are not paths). A path in backticks or quotes is read to
/// the closing mark, so a folder name with spaces survives; any other stops at white space.
pub fn paths_in(text: &str, home: Option<&Path>) -> Vec<String> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let prev = if i == 0 { b' ' } else { b[i - 1] };
        let boundary = !(prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'/' || prev == b'\\' || prev == b'.');
        let drive = i + 2 < b.len() && b[i].is_ascii_alphabetic() && b[i + 1] == b':' && (b[i + 2] == b'/' || b[i + 2] == b'\\');
        let tilde = i + 1 < b.len() && b[i] == b'~' && (b[i + 1] == b'/' || b[i + 1] == b'\\');
        let unix = b[i] == b'/'
            && (i == 0 || prev.is_ascii_whitespace() || matches!(prev, b'`' | b'"' | b'\'' | b'(' | b'[' | b'<' | b'='))
            && b.get(i + 1).is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'~'));
        if !(boundary && (drive || tilde || unix)) {
            i += 1;
            continue;
        }
        let closer = match prev {
            b'`' => Some(b'`'),
            b'"' => Some(b'"'),
            b'\'' => Some(b'\''),
            _ => None,
        };
        let mut j = i;
        while j < b.len() {
            let c = b[j];
            if c == b'\n' || c == b'\r' {
                break;
            }
            match closer {
                Some(q) if c == q => break,
                None if c.is_ascii_whitespace() || matches!(c, b'`' | b'"' | b'\'' | b'<' | b'>' | b'|' | b')' | b'*') => break,
                _ => {}
            }
            j += 1;
        }
        let raw = text[i..j].trim_end_matches(['.', ',', ';', ':', ')', ']', '}']);
        if unix && raw.trim_end_matches('/').matches('/').count() < 2 {
            i = j.max(i + 1);
            continue;
        }
        let path = if tilde {
            home.map(|h| format!("{}{}", h.display(), &raw[1..]))
        } else {
            Some(raw.to_string())
        };
        if let Some(p) = path.and_then(|p| crate::crud::project::absolute_path(&p, None, home)) {
            out.push(p);
        }
        i = j.max(i + 1);
    }
    out
}

// ─── Where a project's evidence points ───────────────────────

struct Ctx {
    fs: Fs,
    /// The home folder, its ancestors, Documents, Desktop, Downloads, every workspace root.
    fixed: Vec<String>,
    /// Every project's folder, resolved: (slug, folder).
    folders: Vec<(String, String)>,
    /// child slug → parent slug (D13).
    parents: HashMap<String, String>,
}

impl Ctx {
    /// The projects whose folder lies strictly inside `folder`, leaving out `except` and its descendants.
    fn covered(&self, folder: &str, except: Option<&str>) -> BTreeSet<String> {
        self.folders
            .iter()
            .filter(|(id, f)| path_under(f, folder) && !same(f, folder) && except.is_none_or(|e| !self.descends(id, e)))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Is `id` the project `of`, or below it through parent links?
    fn descends(&self, id: &str, of: &str) -> bool {
        let mut cur = id.to_string();
        for _ in 0..64 {
            if cur == of {
                return true;
            }
            match self.parents.get(&cur) {
                Some(p) => cur = p.clone(),
                None => return false,
            }
        }
        false
    }

    fn container(&self, folder: &str) -> bool {
        self.fixed.iter().any(|c| same(c, folder) || path_under(c, folder)) || self.covered(folder, None).len() >= 2
    }

    /// The folders a doc naming `point` counts for: the deepest folder that exists, and each folder above it,
    /// up to (not including) the first container.
    fn folders_for(&mut self, point: &str) -> Vec<String> {
        // One spelling, so `/mnt/c/x` and `C:/x` tally as one folder and compare with the stored folders.
        let mut cur = drive_form(&above_dot_folders(point, &self.fixed));
        if local(&cur).is_none() {
            return Vec::new();
        }
        // The deepest folder that exists.
        loop {
            match self.fs.kind(&cur) {
                None => return Vec::new(),
                Some(Kind::Dir) => break,
                Some(Kind::File) | Some(Kind::Missing) => match parent(&cur) {
                    Some(up) if up != cur => cur = up,
                    _ => return Vec::new(),
                },
            }
        }
        let mut out = Vec::new();
        while !self.container(&cur) {
            out.push(cur.clone());
            match parent(&cur) {
                Some(up) if up != cur => cur = up,
                _ => break,
            }
        }
        out
    }
}

/// The handoff and fork docs on record, both tiers: (filed-under project, doc path).
fn handoff_docs(store: &oxigraph::store::Store, config: &BaseConfig) -> Vec<(String, String)> {
    let p = &config.namespace.prefix;
    let sparql = format!(
        "{}\nSELECT DISTINCT ?project ?doc WHERE {{ GRAPH ?g {{ ?h a {p}:Handoff ; {p}:project ?project ; {p}:handoffDoc ?doc }} }}",
        crud::prefixes(&config.namespace)
    );
    let mut out = Vec::new();
    if let Ok(QueryResults::Solutions(rows)) = crate::store::query(store, &sparql) {
        for r in rows.filter_map(|r| r.ok()) {
            let get = |k: &str| r.get(k).map(|t| crud::term_display(t.into()));
            if let (Some(project), Some(doc)) = (get("project"), get("doc")) {
                out.push((project, doc));
            }
        }
    }
    out
}

/// The project docs on record, both tiers: (project slug, doc id, doc path resolved).
fn project_docs(store: &oxigraph::store::Store, config: &BaseConfig, roots: &PathRoots) -> Vec<(String, String, String)> {
    let ns = &config.namespace;
    let p = &ns.prefix;
    let sparql = format!(
        "{}\nSELECT DISTINCT ?proj ?doc ?g ?path WHERE {{\n\
           GRAPH ?pg {{ ?proj a {p}:Project ; {p}:hasDomain ?dom }}\n\
           GRAPH ?g {{ ?doc a {p}:Document ; {p}:hasDomain ?dom ; {p}:path ?path }}\n\
         }}",
        crud::prefixes(ns)
    );
    let mut out = Vec::new();
    if let Ok(QueryResults::Solutions(rows)) = crate::store::query(store, &sparql) {
        for r in rows.filter_map(|r| r.ok()) {
            let get = |k: &str| r.get(k).map(|t| crud::term_display(t.into()));
            let g = crate::crud::project::graph_cell(&r, "g");
            if let (Some(proj), Some(doc), Some(path)) = (get("proj"), get("doc"), get("path")) {
                out.push((crud::slug_of(&proj), doc, roots.stored(&path, g.as_deref())));
            }
        }
    }
    out
}

/// The registered project a handoff's `project` field files it under: the slug itself, or the longest slug it
/// starts with followed by a dash (`base-0160` → `base`, unless `base-0160` is registered).
fn owner<'a>(filed: &str, ids: &'a [String]) -> Option<&'a String> {
    ids.iter()
        .filter(|id| filed == id.as_str() || filed.strip_prefix(id.as_str()).is_some_and(|r| r.starts_with('-')))
        .max_by_key(|id| id.len())
}

/// The suggestions for every project in the workspace file. Reads the store and the disk; writes nothing.
pub fn suggest(cwd: &Path, config: &BaseConfig) -> Result<Report> {
    let ns = &config.namespace;
    let roots = PathRoots::new(cwd, ns);
    let (records, _) = crud::project::list_data(cwd, config, &crate::scope::ProjectScope::All)?;
    let mut seen = BTreeSet::new();
    let records: Vec<ProjectRecord> = records.into_iter().filter(|r| seen.insert(r.id.clone())).collect();
    let ids: Vec<String> = records.iter().map(|r| r.id.clone()).collect();

    let home = roots.home().and_then(|h| crate::crud::project::absolute_path(&h.display().to_string(), None, None));
    let mut fixed: Vec<String> = Vec::new();
    if let Some(h) = &home {
        fixed.push(h.clone());
        for sub in ["Documents", "Desktop", "Downloads"] {
            fixed.push(join(h, sub));
        }
    }
    let ws_roots = config
        .workspace
        .iter()
        .map(|w| w.path.clone())
        .chain(roots.workspace_root().map(|r| r.display().to_string()));
    for w in ws_roots {
        if let Some(a) = crate::crud::project::absolute_path(&w, None, roots.home()) {
            fixed.push(a);
        }
    }
    let mut ctx = Ctx {
        fs: Fs::default(),
        fixed,
        folders: records.iter().filter_map(|r| r.path.clone().map(|p| (r.id.clone(), p))).collect(),
        parents: records.iter().filter_map(|r| r.parent.clone().map(|p| (r.id.clone(), p))).collect(),
    };

    // Evidence: (project → folder → (handoff doc ids, project doc ids)).
    type Tally = BTreeMap<String, (BTreeSet<String>, BTreeSet<String>)>;
    let mut tallies: HashMap<String, Tally> = HashMap::new();
    let home_path = roots.home().map(Path::to_path_buf);
    // One load of both tiers for every record read below: the store is tens of megabytes.
    let store = crate::store::load_merged(cwd);
    let (handoffs, docs) = match &store {
        Some(s) => (handoff_docs(s, config), project_docs(s, config, &roots)),
        None => (Vec::new(), Vec::new()),
    };
    drop(store);
    for (filed, doc) in handoffs {
        let Some(id) = owner(&filed, &ids) else { continue };
        let Ok(text) = std::fs::read_to_string(&doc) else { continue };
        if text.len() > 4_000_000 {
            continue;
        }
        for point in paths_in(&text, home_path.as_deref()) {
            for f in ctx.folders_for(&point) {
                tallies.entry(id.clone()).or_default().entry(f).or_default().0.insert(doc.clone());
            }
        }
    }
    for (id, doc, path) in docs {
        if !ids.contains(&id) {
            continue;
        }
        for f in ctx.folders_for(&path) {
            tallies.entry(id.clone()).or_default().entry(f).or_default().1.insert(doc.clone());
        }
    }

    let mut report = Report::default();
    for r in &records {
        let tally = tallies.remove(&r.id).unwrap_or_default();
        let now = r.path.clone();

        // Folders named like the project: from the evidence, and beside or inside the stored folder.
        let mut named: BTreeSet<String> = tally.keys().filter(|f| named_like(last(f), &r.id, &r.name)).cloned().collect();
        if let Some(s) = &now {
            let mut look = vec![s.clone()];
            if let Some(up) = parent(s) {
                look.push(up);
            }
            for dir in look {
                if ctx.fs.kind(&dir) == Some(Kind::Dir) {
                    for c in ctx.fs.children(&dir) {
                        if named_like(last(&c), &r.id, &r.name) && !ctx.container(&c) {
                            named.insert(c);
                        }
                    }
                }
            }
        }
        // The outermost of a nested pair: `grazer`, not `grazer/skills/grazer`.
        let outer: Vec<String> = named
            .iter()
            .filter(|f| !named.iter().any(|o| o != *f && path_under(f, o) && !same(f, o)))
            .cloned()
            .collect();
        let tallied = |f: &str| tally.get(f).map(|(h, d)| (h.len(), d.len())).unwrap_or((0, 0));
        let mut cands: Vec<Candidate> = outer
            .iter()
            .map(|f| {
                let (h, d) = tallied(f);
                Candidate {
                    folder: f.clone(),
                    handoff_docs: h,
                    project_docs: d,
                    named_like: true,
                    holds_stored: false,
                    marks: Vec::new(),
                }
            })
            .collect();
        if cands.iter().any(|c| c.docs() > 0) {
            cands.retain(|c| c.docs() > 0);
        }
        if cands.is_empty() {
            // No folder named like the project: the deepest folder holding 60% of the docs, two at least.
            let mut docs: BTreeSet<&String> = BTreeSet::new();
            for (h, d) in tally.values() {
                docs.extend(h.iter().chain(d.iter()));
            }
            let need = (docs.len() * 3).div_ceil(5).max(2);
            if let Some((f, (h, d))) = tally
                .iter()
                .filter(|(_, (h, d))| h.len() + d.len() >= need)
                .max_by_key(|(f, (h, d))| (f.split('/').count(), h.len() + d.len()))
            {
                cands.push(Candidate {
                    folder: f.clone(),
                    handoff_docs: h.len(),
                    project_docs: d.len(),
                    named_like: false,
                    holds_stored: false,
                    marks: Vec::new(),
                });
            }
        }
        // A stored path that names a file, or reaches into a dot folder (`x/.paul/paul.json`), points at the
        // folder holding it: that folder is a candidate too, beside whatever the docs found.
        if let Some(s) = &now {
            let holder = above_dot_folders(s, &ctx.fixed);
            if (holder != *s || ctx.fs.kind(s) == Some(Kind::File))
                && let Some(h) = ctx.folders_for(&holder).into_iter().next()
            {
                match cands.iter_mut().find(|c| same(&c.folder, &h)) {
                    Some(c) => c.holds_stored = true,
                    None => {
                        let (hd, pd) = tallied(&h);
                        cands.push(Candidate {
                            named_like: named_like(last(&h), &r.id, &r.name),
                            folder: h,
                            handoff_docs: hd,
                            project_docs: pd,
                            holds_stored: true,
                            marks: Vec::new(),
                        });
                    }
                }
            }
        }
        cands.sort_by(|a, b| b.docs().cmp(&a.docs()).then(a.folder.cmp(&b.folder)));
        for c in &mut cands {
            for (marker, mark) in [(".git", "looks like a code repo"), (".firm", "a Cadre firm"), (".paul", "a PAUL project")] {
                if ctx.fs.has(&c.folder, marker) {
                    c.marks.push(mark.to_string());
                }
            }
        }

        let reason = match &now {
            None => Some("no folder".to_string()),
            Some(s) => match ctx.fs.kind(s) {
                None => {
                    report.not_checked.push(r.id.clone());
                    continue;
                }
                Some(Kind::Missing) => Some("missing".to_string()),
                Some(Kind::File) => Some("not a folder".to_string()),
                Some(Kind::Dir) => {
                    let inside = ctx.covered(s, Some(&r.id));
                    if ctx.fixed.iter().any(|c| same(c, s) || path_under(c, s)) {
                        Some("broad: a home, Documents or workspace folder".to_string())
                    } else if inside.len() >= 2 {
                        Some(format!("broad: holds {} other projects", inside.len()))
                    } else if !cands.is_empty()
                        && cands[0].docs() >= 2
                        && !cands.iter().any(|c| path_under(&c.folder, s) || path_under(s, &c.folder))
                    {
                        Some("its docs point elsewhere".to_string())
                    } else {
                        None
                    }
                }
            },
        };
        match reason {
            Some(reason) => {
                // A candidate that is the stored folder itself is no suggestion.
                cands.retain(|c| now.as_deref().is_none_or(|s| !same(&c.folder, s)));
                report.suggestions.push(Suggestion { project: r.id.clone(), now, reason, candidates: cands });
            }
            None => report.kept.push(r.id.clone()),
        }
    }
    Ok(report)
}

// ─── Output ──────────────────────────────────────────────────

/// The table `--suggest` prints.
pub fn format_table(report: &Report) -> String {
    let mut out = String::new();
    out.push_str("| project | now | suggested folder | evidence |\n|---|---|---|---|\n");
    for s in &report.suggestions {
        let now = match &s.now {
            Some(p) => format!("{} ({})", cell_text(p), s.reason),
            None => "(no folder)".to_string(),
        };
        match s.candidates.as_slice() {
            [] => out.push_str(&format!("| {} | {now} | none found | |\n", s.project)),
            [c] => out.push_str(&format!("| {} | {now} | {} | {} |\n", s.project, cell_text(&c.folder), c.evidence())),
            many => {
                let n = if many.len() == 2 { "TWO".to_string() } else { many.len().to_string() };
                out.push_str(&format!("| {} | {now} | {n} CANDIDATES: | |\n", s.project));
                for c in many {
                    out.push_str(&format!("| | | {} | {} |\n", cell_text(&c.folder), c.evidence()));
                }
            }
        }
    }
    out.push_str(&format!(
        "\n{} project(s) listed; {} keep their folder.",
        report.suggestions.len(),
        report.kept.len()
    ));
    if !report.not_checked.is_empty() {
        out.push_str(&format!(
            " {} not checked from this machine (a WSL path): {}.",
            report.not_checked.len(),
            report.not_checked.join(", ")
        ));
    }
    out.push('\n');
    out
}

/// The reviewed list `--out` writes and `--apply` reads: `"slug" = "folder"` lines. One candidate is written ready
/// to apply; two or more are written commented out, so applying the file untouched never picks one; none found is
/// a comment.
pub fn format_list(report: &Report) -> String {
    let q = |s: &str| toml::Value::String(s.to_string()).to_string();
    let mut out = String::from(
        "# base project paths: a list to review. `base project paths --apply <this file>` sets each project below to its\n\
         # folder, and moves the project's domain path trigger with it. A line starting with # is not applied: remove\n\
         # the # to apply it, or delete a line you do not want. Where two folders are listed, keep one.\n\n",
    );
    for s in &report.suggestions {
        let now = s.now.as_deref().map(|p| format!("{p}, {}", s.reason)).unwrap_or_else(|| "no folder".into());
        match s.candidates.as_slice() {
            [] => out.push_str(&format!("# {}: none found (now: {now})\n\n", s.project)),
            [c] => out.push_str(&format!(
                "{} = {}  # now: {now}; {}\n\n",
                q(&s.project),
                q(&c.folder),
                c.evidence()
            )),
            many => {
                out.push_str(&format!("# {}: {} CANDIDATES, keep one (now: {now})\n", s.project, many.len()));
                for c in many {
                    out.push_str(&format!("# {} = {}  # {}\n", q(&s.project), q(&c.folder), c.evidence()));
                }
                out.push('\n');
            }
        }
    }
    out
}

/// `base project paths --suggest [--out <file>] [--json]`.
pub fn suggest_cmd(cwd: &Path, config: &BaseConfig, out: Option<&Path>, json: bool) -> Result<()> {
    let report = suggest(cwd, config)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", format_table(&report));
    }
    if let Some(file) = out {
        std::fs::write(file, format_list(&report)).with_context(|| format!("writing {}", file.display()))?;
        if !json {
            println!(
                "List written to {}. Review it, then: base project paths --apply {}",
                file.display(),
                file.display()
            );
        }
    }
    Ok(())
}

/// One line of a reviewed list, checked.
#[derive(Debug, serde::Serialize)]
pub struct Planned {
    pub project: String,
    pub from: Option<String>,
    pub to: String,
    /// The folder is a WSL path this machine did not open.
    pub not_checked: bool,
}

/// Read and check a reviewed list. Every line is checked, against one load of the store, before anything is
/// written; any fault refuses the whole file, naming every faulty line.
pub fn plan(cwd: &Path, config: &BaseConfig, file: &Path) -> Result<Vec<Planned>> {
    let ns = &config.namespace;
    let text = std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let table: toml::Table = toml::from_str(&text)
        .map_err(|e| Refused(format!("{} is not a list of `\"slug\" = \"folder\"` lines: {e}", file.display())))?;
    let roots = PathRoots::new(cwd, ns);
    let (records, _) = crud::project::list_data(cwd, config, &crate::scope::ProjectScope::All)?;
    let mut fs = Fs::default();
    let mut faults = Vec::new();
    let mut planned = Vec::new();
    for (slug, value) in &table {
        let Some(raw) = value.as_str() else {
            faults.push(format!("{slug}: the folder must be a quoted path"));
            continue;
        };
        let Some(record) = records.iter().find(|r| r.id == *slug) else {
            faults.push(format!("{slug}: no such project in this workspace"));
            continue;
        };
        let Some(to) = roots.from_cli(raw) else {
            faults.push(format!("{slug}: '{raw}' names no folder"));
            continue;
        };
        if cfg!(windows) && to.starts_with('/') && !to.starts_with("/mnt/") && !wsl_shaped(&to) {
            faults.push(format!(
                "{slug}: {to} is a Unix-style path Windows cannot open; write it as C:/... (or /home/... for a WSL project)"
            ));
            continue;
        }
        let kind = fs.kind(&to);
        match kind {
            Some(Kind::Dir) | None => {}
            Some(Kind::File) => faults.push(format!("{slug}: {to} is a file, not a folder")),
            Some(Kind::Missing) => faults.push(format!("{slug}: {to} does not exist")),
        }
        planned.push(Planned { project: slug.clone(), from: record.path.clone(), to, not_checked: kind.is_none() });
    }
    if !faults.is_empty() {
        return Err(Refused(format!("nothing was written; fix these lines first:\n  {}", faults.join("\n  "))).into());
    }
    Ok(planned)
}

/// `base project paths --apply <file> [--dry-run] [--json]`. Each line is reported as it is set, so a failure part
/// way says which projects were already set and which were not.
pub fn apply_cmd(cwd: &Path, config: &BaseConfig, file: &Path, dry_run: bool, json: bool) -> Result<()> {
    let planned = plan(cwd, config, file)?;
    let total = planned.len();
    for (done, p) in planned.iter().enumerate() {
        let from = p.from.as_deref().unwrap_or("(none)");
        let note = if p.not_checked { " (a WSL path, not checked from this machine)" } else { "" };
        if dry_run {
            if !json {
                println!("would set {}: {from} → {}{note}", p.project, p.to);
            }
            continue;
        }
        let change = crud::project::ProjectUpdate { path: Some(&p.to), ..Default::default() };
        let outcome = crud::project::apply_update(cwd, &config.namespace, &p.project, &change).with_context(|| {
            let set: Vec<&str> = planned[..done].iter().map(|q| q.project.as_str()).collect();
            format!(
                "{}: not set. {done} of {total} were set before this ({}); the rest were not",
                p.project,
                if set.is_empty() { "none".to_string() } else { set.join(", ") }
            )
        })?;
        if !json {
            let dom = match &outcome.repath {
                Some(r) if r.domain_changed => format!(", domain '{}' trigger moved", r.name),
                _ => String::new(),
            };
            println!("{}: {from} → {}{dom}{note}", p.project, p.to);
            for w in &outcome.warnings {
                eprintln!("warning: {w}");
            }
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "dry_run": dry_run, "projects": planned }))?);
    } else {
        let verb = if dry_run { "would be set" } else { "set" };
        println!("{total} project folder(s) {verb}.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_in_reads_quoted_paths_with_spaces_and_stops_bare_ones_at_spaces() {
        let text = "See `C:/Users/x/Documents/Studio/Studio Client/notes.md` and C:\\Users\\x\\code\\repo, then ~/dev/tool.\n\
                    Not this: abc:/nope, http://host/a, and/or /status. Also \"C:/Users/x/Quoted Dir\" and (/srv/app/x.md).";
        let got = paths_in(text, Some(Path::new("C:/Users/x")));
        assert_eq!(
            got,
            vec![
                "C:/Users/x/Documents/Studio/Studio Client/notes.md".to_string(),
                "C:/Users/x/code/repo".to_string(),
                "C:/Users/x/dev/tool".to_string(),
                "C:/Users/x/Quoted Dir".to_string(),
                "/srv/app/x.md".to_string(),
            ]
        );
    }

    #[test]
    fn named_like_allows_case_spaces_and_one_letter() {
        assert!(named_like("Secure Engineering Framework", "secure-engineering-framework", "x"));
        assert!(named_like("Vintryx", "vintrix", "vintrix"));
        assert!(named_like("Studio Client", "studio-client", "Studio Client"));
        assert!(!named_like("Tools", "tool", "tool"), "short names need an exact match");
        assert!(!named_like("health", "health-protocol", "health-protocol"));
    }

    #[test]
    fn dot_folders_count_for_the_folder_above() {
        let roots = vec!["C:/Users/x".to_string(), "C:/Users/y/AppData/Local/Temp/.tmp1/ws".to_string()];
        assert_eq!(above_dot_folders("C:/a/grazer/.worktrees/v/skills/grazer/SKILL.md", &roots), "C:/a/grazer");
        assert_eq!(above_dot_folders("C:/Users/x/.base/forks/f.md", &roots), "C:/Users/x");
        assert_eq!(above_dot_folders("C:/.hidden", &roots), "C:/");
        assert_eq!(above_dot_folders("/home/u/p/x.md", &roots), "/home/u/p/x.md");
        assert_eq!(above_dot_folders("C:/Users/x/AppData/Local/Temp/a.md", &roots), "C:/Users/x");
        assert_eq!(above_dot_folders("C:/w/app/node_modules/pkg/README.md", &roots), "C:/w/app");
        // Another profile's temp folder, outside every root: cut at its AppData.
        assert_eq!(above_dot_folders("C:/Users/y/AppData/Local/Temp/w/p", &roots), "C:/Users/y");
        // A workspace that sits in a temp folder keeps its own paths; only what is below it is looked at.
        assert_eq!(
            above_dot_folders("C:/Users/y/AppData/Local/Temp/.tmp1/ws/Documents/x/.git/HEAD", &roots),
            "C:/Users/y/AppData/Local/Temp/.tmp1/ws/Documents/x"
        );
    }

    #[test]
    fn owner_takes_the_longest_registered_prefix() {
        let ids = vec!["base".to_string(), "base-0160".to_string(), "basemode".to_string()];
        assert_eq!(owner("base-0160", &ids).map(String::as_str), Some("base-0160"));
        assert_eq!(owner("base-ideation", &ids).map(String::as_str), Some("base"));
        assert_eq!(owner("basemode-gtm", &ids).map(String::as_str), Some("basemode"));
        assert_eq!(owner("bases", &ids), None);
    }

    #[test]
    fn drive_form_spells_a_windows_folder_one_way() {
        assert_eq!(drive_form("/mnt/c/Users/x"), "C:/Users/x");
        assert_eq!(drive_form("/mnt/d"), "D:/");
        assert_eq!(drive_form("/mnt/data/x"), "/mnt/data/x", "not a drive mount");
        assert_eq!(drive_form("/home/u/p"), "/home/u/p");
        assert_eq!(drive_form("C:/Users/x"), "C:/Users/x");
        assert!(wsl_shaped("/home/u/p") && wsl_shaped("//wsl.localhost/Ubuntu/home/u"));
        assert!(!wsl_shaped("/Users/x/proj") && !wsl_shaped("/c/Users/x"));
        assert_eq!(paul_folder("C:/w/app/.paul/paul.toml").as_deref(), Some("C:/w/app"));
        assert_eq!(paul_folder("C:/w/app"), None);
    }

    #[test]
    fn parent_stops_at_the_root() {
        assert_eq!(parent("C:/a/b").as_deref(), Some("C:/a"));
        assert_eq!(parent("C:/a").as_deref(), Some("C:/"));
        assert_eq!(parent("/a").as_deref(), Some("/"));
    }
}
