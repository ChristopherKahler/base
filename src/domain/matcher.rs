use std::collections::{HashMap, HashSet};

use crate::domain::DomainDef;

/// Why a domain matched. Only tracked when DEVMODE is on.
#[derive(Debug, Clone, PartialEq)]
pub enum MatchReason {
    Always,
    Keyword,
    Filepath,
    KeywordAndFilepath,
}

impl std::fmt::Display for MatchReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Always => write!(f, "always"),
            Self::Keyword => write!(f, "keyword"),
            Self::Filepath => write!(f, "filepath"),
            Self::KeywordAndFilepath => write!(f, "keyword + filepath"),
        }
    }
}

/// A matched domain paired with why it matched.
#[derive(Debug)]
pub struct DomainMatch<'a> {
    pub domain: &'a DomainDef,
    pub reason: MatchReason,
    /// The active path that satisfied a path trigger, when one did. Devmode names it,
    /// so "why did this domain load" has a file for an answer, not a mode (F29).
    pub path: Option<String>,
    /// Set when the domain matched only as the parent of the project that owns a touched path, through
    /// `nested = true` (D13): the project it came with. Such a match is ordered after every other one, so a tight
    /// budget drops the parent's rules before the child's.
    pub parent_of: Option<String>,
    /// The prompt keywords that hit, as `domains.toml` writes them (K1: "by what").
    pub keywords: Vec<String>,
    /// For a path match, what held the path: the owning project's folder, or the trigger as resolved (K1).
    pub held_by: Option<String>,
}

/// What the path rules need beyond the domain itself (F29, D1, D13).
#[derive(Debug, Default, Clone)]
pub struct TriggerContext {
    /// The home directory, for `~`-relative triggers.
    pub home: Option<String>,
    /// Every registered project: their folders decide which project owns a touched path (P2), their parent links
    /// carry the nested walk (D13), and a trigger may not hold them (P3).
    pub registered: Vec<Registered>,
}

/// A registered project as the path rules see it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Registered {
    pub name: String,
    /// Its folder, resolved the way `resolve_trigger` resolves a trigger, so the two compare. Empty when the
    /// project has none: it can still be a parent, and it owns nothing.
    pub path: String,
    /// The project's slug. Its rules are the domain whose name has this slug (one name in three places, the
    /// 2026-09-09 decision).
    pub slug: String,
    /// The slug of the project it sits inside (`ops:parentProject`, BO-09).
    pub parent: Option<String>,
    /// `nested = true`: work in this project also carries its parent's rules (D13).
    pub nested: bool,
}

impl Registered {
    /// A project with no parent, its slug taken from its name.
    pub fn new(name: &str, path: &str) -> Self {
        Self { name: name.to_string(), path: path.to_string(), slug: crate::crud::slugify(name), ..Default::default() }
    }
}

/// A path trigger's fault. Doctor names it per tier, `add-trigger` refuses it, and devmode names the unrooted
/// ones per prompt, so nothing is dropped silently.
#[derive(Debug, Clone, PartialEq)]
pub enum TriggerFault {
    /// Not a rooted path: a glob, or a relative trigger with no tier root to resolve against. It cannot fire.
    Unrooted,
    /// It holds registered projects it must not (D1): every project under it, except, when the trigger is its
    /// domain's own project folder or lies inside it, that project and the projects below it through parent links.
    /// It still fires, but never into a registered project folder that lies between it and the touched path.
    Broad(Vec<String>),
}

impl std::fmt::Display for TriggerFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unrooted => write!(
                f,
                "is not a rooted path; write it absolute, ~-relative or relative to the tier root"
            ),
            Self::Broad(names) => write!(
                f,
                "holds {}: a trigger must be one project's own folder or a file (base domain paths --suggest proposes one)",
                count_projects(names)
            ),
        }
    }
}

/// `3 registered projects (a, b, c)`, `1 registered project (a)`.
pub fn count_projects(names: &[String]) -> String {
    let noun = if names.len() == 1 { "project" } else { "projects" };
    format!("{} registered {noun} ({})", names.len(), names.join(", "))
}

/// Match domains against prompt text and active file paths.
/// Pure matcher — returns every domain whose triggers fire, with the reason.
/// Dedup/suppression is owned by the hook layer, which hashes the fully
/// rendered output (rules + neighborhood + query results) — the only hash
/// that accurately reflects what would be injected.
///
/// A path brings in the domains [`path_hits`] names: its owner's, the ones whose own trigger holds it with no
/// project folder between, and the owner's nested parents. A domain matched only as such a parent comes after every
/// other match (D13).
pub fn match_domains<'a>(
    prompt: &str,
    domains: &'a [DomainDef],
    active_paths: &[String],
    ctx: &TriggerContext,
) -> Vec<DomainMatch<'a>> {
    let prompt_lower = prompt.to_lowercase();
    let hits = path_hits(domains, active_paths, ctx);

    let mut out: Vec<DomainMatch<'a>> = domains
        .iter()
        .enumerate()
        .filter_map(|(i, d)| {
            let hit = hits.iter().find(|h| h.domain == i);
            let (reason, path, keywords) = is_matched(d, &prompt_lower, hit.map(|h| h.path.as_str()))?;
            let parent_of = match (&reason, hit.map(|h| &h.via)) {
                (MatchReason::Filepath, Some(PathVia::Parent(child))) => Some(child.clone()),
                _ => None,
            };
            let held_by = path.as_ref().and(hit).map(|h| h.value.clone());
            Some(DomainMatch { domain: d, reason, path, parent_of, keywords, held_by })
        })
        .collect();
    // Stable: the rest keep their order, and the parents keep theirs behind them.
    out.sort_by_key(|m| m.parent_of.is_some());
    out
}

/// The automatic entry — what the prompt hook injects without being asked. A domain
/// with `auto_inject = false` never reaches it, whatever its mode or triggers (F29 D3).
/// `match_domains` above stays the pure trigger test for explicit readers such as
/// `base context`, which the operator invoked on purpose.
pub fn match_domains_auto<'a>(
    prompt: &str,
    domains: &'a [DomainDef],
    active_paths: &[String],
    ctx: &TriggerContext,
) -> Vec<DomainMatch<'a>> {
    match_domains(prompt, domains, active_paths, ctx)
        .into_iter()
        .filter(|m| m.domain.auto_inject)
        .collect()
}

/// Determine if a domain matches the current context. `path_hit` is the touched path that brought the domain in,
/// when one did ([`path_hits`]). Returns Some(reason, the touched path, the keywords that hit) on match, None on no
/// match.
fn is_matched(
    domain: &DomainDef,
    prompt_lower: &str,
    path_hit: Option<&str>,
) -> Option<(MatchReason, Option<String>, Vec<String>)> {
    // Exclude patterns are checked first — any match vetoes the domain, an always-on
    // one included. Until 0.14.0 `always` returned before this loop, so an exclude on
    // an always-on domain was dead configuration (F29).
    for pattern in &domain.exclude {
        if prompt_lower.contains(&pattern.to_lowercase()) {
            return None;
        }
    }

    // Always-on domains match everything else
    if domain.is_always() {
        return Some((MatchReason::Always, None, Vec::new()));
    }

    // Keyword match: a prompt keyword as whole words in the prompt text. A substring
    // test stood here until 0.14.0 and fired `base` on `database` (F29). Excludes keep
    // the substring test on purpose: a veto that fires too often errs toward silence.
    // Every keyword that hits, not only the first: the match log says which (K1).
    let keywords: Vec<String> = domain
        .prompt_keywords
        .iter()
        .filter(|kw| contains_word(prompt_lower, &kw.to_lowercase()))
        .cloned()
        .collect();

    let reason = match (!keywords.is_empty(), path_hit.is_some()) {
        (true, true) => MatchReason::KeywordAndFilepath,
        (true, false) => MatchReason::Keyword,
        (false, true) => MatchReason::Filepath,
        (false, false) => return None,
    };
    Some((reason, path_hit.map(String::from), keywords))
}

/// Does `needle` occur in `text` as whole words? The characters on either side of an
/// occurrence, when there are any, must not be word characters (alphanumeric or `_`).
/// Case is the caller's business; both sides arrive lowercased from the matcher.
pub fn contains_word(text: &str, needle: &str) -> bool {
    let needle = needle.trim();
    if needle.is_empty() {
        return false;
    }
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut from = 0;
    while let Some(pos) = text[from..].find(needle) {
        let at = from + pos;
        let end = at + needle.len();
        let before_ok = text[..at].chars().next_back().is_none_or(|c| !is_word(c));
        let after_ok = text[end..].chars().next().is_none_or(|c| !is_word(c));
        if before_ok && after_ok {
            return true;
        }
        // Step one character, not one byte, so a multi-byte prompt never splits.
        from = at + text[at..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

// ─── The file being touched decides (D1, P2, D13) ────────────────────────────

/// How a touched path brought a domain in.
#[derive(Debug, Clone, PartialEq)]
pub enum PathVia {
    /// The domain of the project that owns the path: the deepest registered project folder holding it (P2).
    Owner,
    /// One of the domain's own path triggers holds the path, with no registered project folder between the
    /// trigger and the path: a file, a folder inside a project, or a place no project holds.
    Trigger,
    /// A parent of the owner, reached through `nested = true` at every level below it (D13), with the project it
    /// is the parent of, named by that project's domain.
    Parent(String),
}

/// One domain a touched path brings in.
#[derive(Debug, Clone, PartialEq)]
pub struct PathHit {
    /// The domain's index in the slice given to [`path_hits`].
    pub domain: usize,
    /// The touched path that brought it.
    pub path: String,
    pub via: PathVia,
    /// What held the path (K1): the owning project's folder for an owner, the trigger as resolved for a trigger, the
    /// parent project's folder (empty when it has none) for a parent.
    pub value: String,
}

/// Every domain the touched `paths` bring in, each once, at its first reason (D1, P2, D13).
///
/// For each path, in order: the domain of each project that owns it ([`owners`]); then every domain one of whose
/// own triggers holds the path, unless a registered project folder lies between the trigger and the path, which is
/// how `Documents` stops reaching into the projects under it while a trigger on a file, or on a folder inside the
/// project, still fires; then the domains of the owner's nested parents ([`nested_parents`]). Every parent comes
/// after every other hit, across all the paths, so a tight budget drops parent rules first.
///
/// Always-on domains are left out: they match every prompt anyway, and the tool hook never serves them.
/// `auto_inject` is the caller's to honour, as with every other match.
pub fn path_hits(domains: &[DomainDef], paths: &[String], ctx: &TriggerContext) -> Vec<PathHit> {
    // A project's rules: the first triggered domain whose name has the project's slug.
    let mut by_slug: HashMap<String, usize> = HashMap::new();
    for (i, d) in domains.iter().enumerate().filter(|(_, d)| !d.is_always()) {
        by_slug.entry(crate::crud::slugify(&d.name)).or_insert(i);
    }
    // Direct hits and parent hits are recorded apart: a domain reached as a parent through one path and owned (or
    // held by its trigger) through another is a direct hit, labelled and ordered as one.
    let mut seen: HashSet<usize> = HashSet::new();
    let mut parent_seen: HashSet<usize> = HashSet::new();
    let mut direct: Vec<PathHit> = Vec::new();
    let mut parents: Vec<PathHit> = Vec::new();
    for path in paths {
        let owned = owners(path, &ctx.registered);
        // The owner's folder: a trigger above it is above a project, and stops there.
        let floor = owned.first().map(|o| o.path.as_str());
        for o in &owned {
            if let Some(&i) = by_slug.get(&o.slug)
                && seen.insert(i)
            {
                direct.push(PathHit { domain: i, path: path.clone(), via: PathVia::Owner, value: o.path.clone() });
            }
        }
        for (i, d) in domains.iter().enumerate() {
            if d.is_always() || seen.contains(&i) {
                continue;
            }
            let held = d.paths.iter().find_map(|t| {
                live_trigger(t, d.root.as_deref(), ctx)
                    .filter(|t| path_under(path, t) && floor.is_none_or(|f| path_under(t, f)))
            });
            if let Some(trigger) = held {
                seen.insert(i);
                direct.push(PathHit { domain: i, path: path.clone(), via: PathVia::Trigger, value: trigger });
            }
        }
        for o in &owned {
            for (child, parent) in nested_parents(o, &ctx.registered) {
                if let Some(&i) = by_slug.get(&parent.slug)
                    && !seen.contains(&i)
                    && parent_seen.insert(i)
                {
                    // Named as the child's own block is headed: its domain's name, else the project's.
                    let child_name = by_slug.get(&child.slug).map_or_else(|| child.name.clone(), |&c| domains[c].name.clone());
                    parents.push(PathHit {
                        domain: i,
                        path: path.clone(),
                        via: PathVia::Parent(child_name),
                        value: parent.path.clone(),
                    });
                }
            }
        }
    }
    parents.retain(|h| !seen.contains(&h.domain));
    direct.extend(parents);
    direct
}

/// The projects that own `path`: those whose folder is the deepest registered folder holding it (P2), each once,
/// by slug. Usually one; two only when two projects share that folder. Empty when no project folder holds it.
pub fn owners<'r>(path: &str, registered: &'r [Registered]) -> Vec<&'r Registered> {
    let mut best: Vec<&Registered> = Vec::new();
    let mut depth = 0;
    for r in registered.iter().filter(|r| !r.path.is_empty() && path_under(path, &r.path)) {
        let d = components(&r.path).len();
        if d > depth {
            best.clear();
            depth = d;
        }
        if d == depth && !best.iter().any(|b| b.slug == r.slug) {
            best.push(r);
        }
    }
    best.sort_by(|a, b| a.slug.cmp(&b.slug));
    best
}

/// The parents whose rules a path owned by `owner` also carries (D13), as (child, parent) pairs, nearest first:
/// its parent while it says `nested = true`, then that parent's parent while the parent says so, and so on. A
/// project registered twice (both tiers) is nested when either record says so. A parent that is not registered,
/// or a loop (refused when set, BO-09), ends the walk.
pub fn nested_parents<'r>(owner: &'r Registered, registered: &'r [Registered]) -> Vec<(&'r Registered, &'r Registered)> {
    let nested = |slug: &str| registered.iter().any(|r| r.slug == slug && r.nested);
    let parent_of = |slug: &str| registered.iter().find_map(|r| (r.slug == slug).then(|| r.parent.clone()).flatten());
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::from([owner.slug.clone()]);
    let mut cur = owner;
    while nested(&cur.slug) {
        let Some(p) = parent_of(&cur.slug) else { break };
        if !seen.insert(p.clone()) {
            break;
        }
        let Some(rec) = registered.iter().find(|r| r.slug == p) else { break };
        out.push((cur, rec));
        cur = rec;
    }
    out
}

/// Is `id` the project `of`, or below it through parent links?
fn descends(id: &str, of: &str, registered: &[Registered]) -> bool {
    let mut cur = id.to_string();
    for _ in 0..64 {
        if cur == of {
            return true;
        }
        match registered.iter().find_map(|r| (r.slug == cur).then(|| r.parent.clone()).flatten()) {
            Some(p) => cur = p,
            None => return false,
        }
    }
    false
}

/// The registered projects a resolved trigger of `domain` holds that it must not (D1, P3), by name, sorted: every
/// project whose folder lies under it, except the project whose folder it is and that project's children (through
/// parent links), and, when it lies inside the domain's own project folder, the domain's own project and its
/// children. A project registered in both tiers counts once. Empty for a trigger that is one project's own folder,
/// a file, or a folder that holds no other project: Example 4's "one project's own folder or a file".
pub fn trigger_breadth(resolved: &str, domain: &str, ctx: &TriggerContext) -> Vec<String> {
    let own = crate::crud::slugify(domain);
    let inside_own = ctx
        .registered
        .iter()
        .any(|r| r.slug == own && !r.path.is_empty() && path_under(resolved, &r.path));
    let at: Vec<&str> = ctx
        .registered
        .iter()
        .filter(|r| !r.path.is_empty() && path_under(&r.path, resolved) && path_under(resolved, &r.path))
        .map(|r| r.slug.as_str())
        .collect();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut names: Vec<String> = Vec::new();
    for r in ctx.registered.iter().filter(|r| !r.path.is_empty() && path_under(&r.path, resolved)) {
        let its = at.iter().any(|a| descends(&r.slug, a, &ctx.registered));
        if its || (inside_own && descends(&r.slug, &own, &ctx.registered)) {
            continue;
        }
        if seen.insert(r.slug.as_str()) {
            names.push(r.name.clone());
        }
    }
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// A path as components on `/`, with the shapes this crate meets on one machine folded
/// together: `\\` is `/`, `/mnt/c/...` is `c:/...` (the WSL install and the Windows
/// install read one store), empty and `.` components vanish.
fn components(p: &str) -> Vec<String> {
    let p = p.replace('\\', "/");
    let mut out: Vec<String> = p
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .map(String::from)
        .collect();
    if p.starts_with("/mnt/")
        && out.len() >= 2
        && out[1].len() == 1
        && out[1].as_bytes()[0].is_ascii_alphabetic()
    {
        out.drain(..2);
        // keep it simple: `/mnt/c/x` -> ["c:", "x"]
        out.insert(0, format!("{}:", p[5..6].to_ascii_lowercase()));
    }
    // One spelling for a volume: `C:` and `c:` are the same drive, and a resolved
    // trigger is compared and printed as a string (doctor, add-trigger, devmode).
    if windows_shaped(&out) {
        out[0] = out[0].to_ascii_lowercase();
    }
    out
}

/// A Windows path: a drive letter first, where case does not distinguish files.
fn windows_shaped(components: &[String]) -> bool {
    components
        .first()
        .is_some_and(|c| c.len() == 2 && c.as_bytes()[1] == b':' && c.as_bytes()[0].is_ascii_alphabetic())
}

/// Is `p` already rooted on its own: `/x`, `C:/x`, `C:\\x`, `\\\\server\\x`, `/mnt/c/x`?
pub fn is_absolute(p: &str) -> bool {
    let s = p.replace('\\', "/");
    s.starts_with('/') || (s.len() >= 2 && s.as_bytes()[1] == b':' && s.as_bytes()[0].is_ascii_alphabetic())
}

/// Resolve a path trigger to the absolute path it names: absolute as written, `~` and
/// `~/x` against `home`, anything else against `root` (the tier the domain came from).
/// `None` when it cannot be rooted — a glob, or a relative trigger with no root, or a
/// `~` trigger with no home. Returned as `/`-separated components joined, so two
/// spellings of one place compare equal (F29).
pub fn resolve_trigger(trigger: &str, root: Option<&str>, home: Option<&str>) -> Option<String> {
    let t = trigger.trim();
    if t.is_empty() || t.contains(['*', '?']) {
        return None;
    }
    let joined = if is_absolute(t) {
        t.to_string()
    } else if t == "~" || t.starts_with("~/") || t.starts_with("~\\") {
        format!("{}/{}", home?, &t[1..])
    } else {
        format!("{}/{}", root?, t)
    };
    let parts = components(&joined);
    if parts.is_empty() {
        return None;
    }
    let lead = if joined.replace('\\', "/").starts_with('/') && !windows_shaped(&parts) { "/" } else { "" };
    Some(format!("{lead}{}", parts.join("/")))
}

/// Does `active` lie under the resolved trigger `trigger` (itself included)? A prefix
/// test on path components, never on characters, so `Documents` does not cover
/// `MyDocuments/a` or `Documents-old/x`; case-blind when either side is a Windows path,
/// because `Tools` and `tools` are one directory there. `contains` stood here until
/// 0.14.0 and let a trigger fire on any string holding it (F29).
pub fn path_under(active: &str, trigger: &str) -> bool {
    let t = components(trigger);
    let a = components(active);
    if t.is_empty() || t.len() > a.len() {
        return false;
    }
    if windows_shaped(&t) || windows_shaped(&a) {
        a[..t.len()].iter().zip(&t).all(|(x, y)| x.eq_ignore_ascii_case(y))
    } else {
        a[..t.len()] == t[..]
    }
}

/// One trigger of `domain`, judged: the resolved path it names when it has no fault, else its fault. `root` is the
/// tier the domain came from.
pub fn trigger_state(trigger: &str, root: Option<&str>, domain: &str, ctx: &TriggerContext) -> Result<String, TriggerFault> {
    let Some(resolved) = resolve_trigger(trigger, root, ctx.home.as_deref()) else {
        return Err(TriggerFault::Unrooted);
    };
    let broad = trigger_breadth(&resolved, domain, ctx);
    if broad.is_empty() { Ok(resolved) } else { Err(TriggerFault::Broad(broad)) }
}

/// The resolved path of a trigger that can fire: every rooted one. Until 0.16.0 a trigger over two or more
/// registered projects went inert here (F29), the opposite of D1; such a trigger now fires, and [`path_hits`] keeps
/// it out of every project folder below it.
pub fn live_trigger(trigger: &str, root: Option<&str>, ctx: &TriggerContext) -> Option<String> {
    resolve_trigger(trigger, root, ctx.home.as_deref())
}

/// The fault of a trigger of `domain`, if it has one.
pub fn trigger_fault(trigger: &str, root: Option<&str>, domain: &str, ctx: &TriggerContext) -> Option<TriggerFault> {
    trigger_state(trigger, root, domain, ctx).err()
}

/// Every trigger across `domains` that cannot fire at all (unrooted), as (domain, trigger, fault): devmode names
/// these on every prompt, since the domain silently lost a trigger.
pub fn inert_triggers<'a>(domains: &'a [DomainDef], ctx: &TriggerContext) -> Vec<(&'a str, &'a str, TriggerFault)> {
    faulty_triggers(domains, ctx).into_iter().filter(|(_, _, f)| *f == TriggerFault::Unrooted).collect()
}

/// Every trigger across `domains` with a fault, unrooted or broad, as (domain, trigger, fault), for doctor.
pub fn faulty_triggers<'a>(domains: &'a [DomainDef], ctx: &TriggerContext) -> Vec<(&'a str, &'a str, TriggerFault)> {
    domains
        .iter()
        .flat_map(|d| {
            d.paths.iter().filter_map(move |t| {
                trigger_fault(t, d.root.as_deref(), &d.name, ctx).map(|f| (d.name.as_str(), t.as_str(), f))
            })
        })
        .collect()
}

/// The one sentence doctor prints for a trigger with a fault.
pub fn fault_sentence(domain: &str, trigger: &str, fault: &TriggerFault) -> String {
    format!("path trigger `{trigger}` on `{domain}` {fault}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> TriggerContext {
        TriggerContext { home: Some("/home/u".into()), ..Default::default() }
    }

    fn make_domain(name: &str, mode: &str, keywords: &[&str], rules: &[&str]) -> DomainDef {
        DomainDef {
            name: name.into(),
            aliases: Vec::new(),
            mode: mode.into(),
            auto_inject: true,
            root: None,
            prompt_keywords: keywords.iter().map(|s| s.to_string()).collect(),
            file_keywords: Vec::new(),
            paths: Vec::new(),
            exclude: Vec::new(),
            rules: rules.iter().map(|s| crate::domain::RuleEntry::Bare(s.to_string())).collect(),
            query: None,
            query_format: None,
            commands: Vec::new(),
            command_activation: "both".into(),
            role: None,
            output_mode: None,
            format: None,
        }
    }

    #[test]
    fn always_on_always_matches() {
        let domains = vec![make_domain("global", "always", &[], &["Rule 1"])];
        let matched = match_domains("anything", &domains, &[], &ctx());
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].domain.name, "global");
        assert_eq!(matched[0].reason, MatchReason::Always);
    }

    #[test]
    fn keyword_match() {
        let domains = vec![make_domain("dev", "triggered", &["fix bug"], &["Dev rule"])];

        let matched = match_domains("please fix bug in auth", &domains, &[], &ctx());
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].reason, MatchReason::Keyword);

        let matched = match_domains("check my calendar", &domains, &[], &ctx());
        assert!(matched.is_empty());
    }

    #[test]
    fn keyword_case_insensitive() {
        let domains = vec![make_domain("dev", "triggered", &["Fix Bug"], &["Rule"])];
        let matched = match_domains("FIX BUG please", &domains, &[], &ctx());
        assert_eq!(matched.len(), 1);
    }

    /// The hooks report absolute tool paths; a relative trigger resolves against the
    /// tier root the domain came from, and the match names the file.
    #[test]
    fn path_match() {
        let mut domain = make_domain("dev", "triggered", &[], &["Rule"]);
        domain.paths = vec!["src/".into()];
        domain.root = Some("/home/u/proj".into());
        let domains = vec![domain];

        let matched = match_domains("hello", &domains, &["/home/u/proj/src/main.rs".into()], &ctx());
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].reason, MatchReason::Filepath);
        assert_eq!(matched[0].path.as_deref(), Some("/home/u/proj/src/main.rs"));

        // The same trigger with no root cannot resolve, so it cannot fire.
        let mut unrooted = make_domain("dev", "triggered", &[], &["Rule"]);
        unrooted.paths = vec!["src/".into()];
        assert!(match_domains("hello", &[unrooted], &["/home/u/proj/src/main.rs".into()], &ctx()).is_empty());
    }

    #[test]
    fn both_keyword_and_path() {
        let mut domain = make_domain("dev", "triggered", &["code"], &["Rule"]);
        domain.paths = vec!["src/".into()];
        domain.root = Some("/home/u/proj".into());
        let domains = vec![domain];

        let matched = match_domains("write code", &domains, &["/home/u/proj/src/main.rs".into()], &ctx());
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].reason, MatchReason::KeywordAndFilepath);
    }

    #[test]
    fn exclude_vetoes_match() {
        let mut domain = make_domain("dev", "triggered", &["code"], &["Rule"]);
        domain.exclude = vec!["review only".into()];
        let domains = vec![domain];

        let matched = match_domains("write code for this", &domains, &[], &ctx());
        assert_eq!(matched.len(), 1);

        let matched = match_domains("review only the code", &domains, &[], &ctx());
        assert!(matched.is_empty());
    }

    /// `auto_inject = false` is honoured before any other test, and only on the
    /// automatic entry: the explicit matcher still reports the domain.
    #[test]
    fn auto_inject_false_is_dropped_by_the_automatic_entry_only() {
        let mut always = make_domain("secret", "always", &[], &["Never say a floor out loud"]);
        always.auto_inject = false;
        let mut triggered = make_domain("terms", "triggered", &["terms"], &["Rule"]);
        triggered.paths = vec!["Documents".into()];
        triggered.auto_inject = false;
        let domains = vec![always, triggered];
        let paths = vec!["Documents/x.md".to_string()];

        assert!(match_domains_auto("what are the terms", &domains, &paths, &ctx()).is_empty());
        assert_eq!(match_domains("what are the terms", &domains, &paths, &ctx()).len(), 2);
    }

    /// Resolution: absolute as written, `~` against home, relative against the tier
    /// root, WSL mounts folded onto the drive letter, globs and rootless triggers refused.
    #[test]
    fn triggers_resolve_against_their_tier_root() {
        let r = |t: &str| resolve_trigger(t, Some("C:/Users/x"), Some("/home/u"));
        assert_eq!(r("Documents").as_deref(), Some("c:/Users/x/Documents"));
        assert_eq!(r("Documents\\Studio Work/").as_deref(), Some("c:/Users/x/Documents/Studio Work"));
        assert_eq!(r("~/notes").as_deref(), Some("/home/u/notes"));
        assert_eq!(r("/srv/data").as_deref(), Some("/srv/data"));
        assert_eq!(r("D:\\vault").as_deref(), Some("d:/vault"));
        assert_eq!(r("/mnt/c/Users/x/tools").as_deref(), Some("c:/Users/x/tools"));
        assert_eq!(r("*.md"), None);
        assert_eq!(resolve_trigger("Documents", None, Some("/home/u")), None);
        assert_eq!(resolve_trigger("~/x", Some("/proj"), None), None);
    }

    /// The substring test fired `Documents` on `MyDocuments/a` and on `Documents-old/x`;
    /// a component-boundary test does not. Windows paths compare case-blind.
    #[test]
    fn path_triggers_match_on_component_boundaries() {
        assert!(path_under("C:\\Users\\x\\Documents\\a.md", "c:/Users/x/Documents"));
        assert!(path_under("C:/Users/x/Documents", "c:/Users/x/Documents"));
        assert!(path_under("/home/x/Documents/Studio Work/notes.md", "/home/x/Documents/Studio Work"));
        assert!(path_under("C:/Users/x/Tools/stt/a.py", "c:/Users/x/tools"));
        assert!(path_under("/mnt/c/Users/x/tools/a.py", "c:/Users/x/tools"));
        assert!(!path_under("C:/Users/x/MyDocuments/a.md", "c:/Users/x/Documents"));
        assert!(!path_under("C:/Users/x/Documents-old/a.md", "c:/Users/x/Documents"));
        assert!(!path_under("C:/Users/x", "c:/Users/x/Documents"));
        assert!(!path_under("/home/x/Src/main.rs", "/home/x/src"));
        assert!(!path_under("D:/mirror/C:/Users/x/genai/vp/a.md", "c:/Users/x/genai/vp"));
        assert!(!path_under("anything", ""));
    }

    /// `base` fired on `database`; whole-word matching stops that and keeps the phrase
    /// keywords, the punctuated ones and the ones at either end of the prompt.
    #[test]
    fn keywords_match_on_word_boundaries() {
        assert!(!contains_word("show me the database schema", "base"));
        assert!(!contains_word("rebase onto main", "base"));
        assert!(contains_word("how is base doing", "base"));
        assert!(contains_word("base", "base"));
        assert!(contains_word("ask base.", "base"));
        assert!(contains_word("please fix bug in auth", "fix bug"));
        assert!(contains_word("edit .base-gbl/base.toml", ".base-gbl"));
        assert!(contains_word("über base über", "base"));
        assert!(!contains_word("anything", ""));
        let domains = vec![make_domain("cfg", "triggered", &["base"], &["Rule"])];
        assert!(match_domains("show me the database schema", &domains, &[], &ctx()).is_empty());
        assert_eq!(match_domains("how is base doing", &domains, &[], &ctx()).len(), 1);
    }

    #[test]
    fn an_exclude_vetoes_an_always_on_domain() {
        let mut domain = make_domain("quiet", "always", &[], &["Rule"]);
        domain.exclude = vec!["haiku".into()];
        let domains = vec![domain];
        assert_eq!(match_domains("what is the weather", &domains, &[], &ctx()).len(), 1);
        assert!(match_domains("write a haiku about tea", &domains, &[], &ctx()).is_empty());
    }

    fn reg(name: &str, path: &str) -> Registered {
        Registered::new(name, path)
    }

    fn child(name: &str, path: &str, parent: &str, nested: bool) -> Registered {
        Registered { parent: Some(parent.into()), nested, ..Registered::new(name, path) }
    }

    fn rooted(name: &str, paths: &[&str]) -> DomainDef {
        let mut d = make_domain(name, "triggered", &[], &["Rule"]);
        d.paths = paths.iter().map(|p| p.to_string()).collect();
        d.root = Some("C:/Users/x".into());
        d
    }

    /// D1 and P3: a trigger over registered projects is broad and named, with the projects it holds; the domain's
    /// own folder is not, even holding the domain's own child; one that is a file, or a folder no project sits in,
    /// is not. A broad trigger still fires (F29's inert rule is gone), but only where no project folder lies between
    /// it and the touched file. A project registered twice is one project. An unrooted trigger is still inert.
    #[test]
    fn a_broad_trigger_is_named_and_stops_at_every_project_folder() {
        let ctx = TriggerContext {
            home: Some("/home/u".into()),
            registered: vec![
                reg("agentic-os", "c:/Users/x/Documents/agentic-os"),
                reg("studio", "c:/Users/x/Documents/Studio"),
                child("studio-client", "c:/Users/x/Documents/Studio/client", "studio", false),
                reg("stt", "c:/Users/x/Tools/stt"),
                reg("hub", "c:/Users/x/Tools/hub"),
                reg("stt", "c:/Users/x/Tools/stt"),
            ],
        };
        let root = Some("C:/Users/x");
        assert_eq!(
            trigger_fault("Documents", root, "notes", &ctx),
            Some(TriggerFault::Broad(vec!["agentic-os".into(), "studio".into(), "studio-client".into()]))
        );
        assert_eq!(trigger_fault("tools", root, "notes", &ctx), Some(TriggerFault::Broad(vec!["hub".into(), "stt".into()])));
        assert_eq!(trigger_fault("Documents/Studio", root, "studio", &ctx), None, "its own folder, holding its own child");
        assert_eq!(trigger_fault("Documents/Studio/plan.md", root, "notes", &ctx), None, "a file");
        assert_eq!(trigger_fault("Documents/Studio/drafts", root, "notes", &ctx), None, "a folder no project sits in");
        assert_eq!(trigger_fault("Documents/Studio", root, "notes", &ctx), None, "one project's own folder, on a topic domain");
        assert_eq!(
            trigger_fault("Documents/Studio/client", root, "studio", &ctx),
            None,
            "inside its own folder, on its child's folder"
        );
        assert_eq!(trigger_fault("*.md", root, "notes", &ctx), Some(TriggerFault::Unrooted));
        assert_eq!(
            fault_sentence("notes", "tools", &trigger_fault("tools", root, "notes", &ctx).unwrap()),
            "path trigger `tools` on `notes` holds 2 registered projects (hub, stt): a trigger must be one project's own folder or a file (base domain paths --suggest proposes one)"
        );

        let notes = rooted("notes", &["Documents"]);
        let domains = std::slice::from_ref(&notes);
        let fire = |path: &str| !match_domains("hello", domains, &[path.to_string()], &ctx).is_empty();
        assert!(fire("C:/Users/x/Documents/loose.md"), "no project holds it: the trigger fires (not inert)");
        assert!(!fire("C:/Users/x/Documents/agentic-os/a.md"), "a project folder lies between");
        assert!(!fire("C:/Users/x/Documents/Studio/client/a.md"), "two do");
        assert_eq!(inert_triggers(domains, &ctx), vec![], "a broad trigger is not inert");
        assert_eq!(faulty_triggers(domains, &ctx).len(), 1, "doctor still names it");
    }

    /// P2 and D13 in the matcher: the owner is the deepest project folder; its parent comes after it, labelled,
    /// only while each level says nested; a trigger on a file or a folder inside the project fires; a sibling's
    /// domain never does.
    #[test]
    fn the_owner_and_its_nested_parents_decide_and_parents_come_last() {
        let ctx = TriggerContext {
            home: Some("/home/u".into()),
            registered: vec![
                reg("studio", "c:/Users/x/Documents/Studio"),
                child("studio-client", "c:/Users/x/Documents/Studio/client", "studio", true),
                reg("agentic-os", "c:/Users/x/Documents/agentic-os"),
            ],
        };
        let domains = vec![
            rooted("studio", &["C:/Users/x/Documents/Studio"]),
            rooted("agentic-os", &["C:/Users/x/Documents/agentic-os"]),
            rooted("studio-client", &[]),
            rooted("plans", &["C:/Users/x/Documents/Studio/client/plan.md"]),
        ];
        let names = |path: &str| -> Vec<(String, Option<String>)> {
            match_domains("hello", &domains, &[path.to_string()], &ctx)
                .into_iter()
                .map(|m| (m.domain.name.clone(), m.parent_of))
                .collect()
        };
        assert_eq!(
            names("C:/Users/x/Documents/Studio/client/plan.md"),
            vec![
                ("studio-client".to_string(), None),
                ("plans".to_string(), None),
                ("studio".to_string(), Some("studio-client".to_string())),
            ],
            "owner (no trigger needed), the file's own trigger, then the nested parent, last though declared first"
        );
        assert_eq!(names("C:/Users/x/Documents/Studio/brief.md"), vec![("studio".to_string(), None)]);
        assert_eq!(names("C:/Users/x/Documents/elsewhere.md"), vec![]);

        let hits = path_hits(&domains, &["C:/Users/x/Documents/Studio/client/a.md".into()], &ctx);
        assert_eq!(hits.iter().map(|h| h.via.clone()).collect::<Vec<_>>(), vec![PathVia::Owner, PathVia::Parent("studio-client".into())]);
        // Reached as a parent through one path and owned through the next: a direct hit, not a parent.
        let two = ["C:/Users/x/Documents/Studio/client/a.md".to_string(), "C:/Users/x/Documents/Studio/brief.md".to_string()];
        let vias: Vec<(usize, PathVia)> = path_hits(&domains, &two, &ctx).into_iter().map(|h| (h.domain, h.via)).collect();
        assert_eq!(vias, vec![(2, PathVia::Owner), (0, PathVia::Owner)], "studio owns brief.md: {vias:?}");
    }

    /// D13: the walk climbs while each level says nested, stops at the first that does not, and stops on a loop.
    #[test]
    fn the_nested_walk_stops_at_the_first_false_and_on_a_loop() {
        let registered = vec![
            child("a", "c:/x/a", "b", true),
            child("b", "c:/x/b", "c", false),
            child("c", "c:/x/c", "d", true),
            reg("d", "c:/x/d"),
        ];
        let walk = |slug: &str| -> Vec<String> {
            let owner = registered.iter().find(|r| r.slug == slug).unwrap();
            nested_parents(owner, &registered).iter().map(|(_, p)| p.slug.clone()).collect()
        };
        assert_eq!(walk("a"), vec!["b"], "b says false, so c is not reached");
        assert_eq!(walk("c"), vec!["d"]);
        assert_eq!(walk("b"), Vec::<String>::new());

        let looped = vec![child("p", "c:/x/p", "q", true), child("q", "c:/x/q", "p", true)];
        let owner = &looped[0];
        assert_eq!(nested_parents(owner, &looped).len(), 1, "p -> q, then q -> p is a loop and ends the walk");
    }

    #[test]
    fn no_domains_no_match() {
        let matched = match_domains("anything", &[], &[], &ctx());
        assert!(matched.is_empty());
    }

    #[test]
    fn no_rules_domain_still_matched_but_empty() {
        let domains = vec![make_domain("empty", "always", &[], &[])];
        let matched = match_domains("anything", &domains, &[], &ctx());
        assert_eq!(matched.len(), 1);
        assert!(matched[0].domain.rules.is_empty());
    }
}
