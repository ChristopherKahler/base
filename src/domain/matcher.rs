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
}

/// What the path rules need beyond the domain itself (F29).
#[derive(Debug, Default, Clone)]
pub struct TriggerContext {
    /// The home directory, for `~`-relative triggers.
    pub home: Option<String>,
    /// Every registered project, for the broadcast test: a trigger that is a prefix of
    /// two or more of these is inert.
    pub registered: Vec<Registered>,
}

/// A registered project as the trigger rules see it: its name and its path resolved
/// the way `resolve_trigger` resolves a trigger, so the two compare.
#[derive(Debug, Clone, PartialEq)]
pub struct Registered {
    pub name: String,
    pub path: String,
}

/// Why a path trigger is inert (F29, G0 step 6). It cannot fire; doctor names it per
/// tier and devmode names it per prompt, so the drop is never silent.
#[derive(Debug, Clone, PartialEq)]
pub enum TriggerFault {
    /// Not a rooted path: a glob, or a relative trigger with no tier root to resolve against.
    Unrooted,
    /// A prefix of the paths of two or more registered projects — a broadcast, not a trigger.
    Covers(Vec<String>),
}

impl std::fmt::Display for TriggerFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unrooted => write!(
                f,
                "is not a rooted path; write it absolute, ~-relative or relative to the tier root"
            ),
            Self::Covers(names) => write!(
                f,
                "covers {} registered projects ({}); narrow it or set auto_inject = false",
                names.len(),
                names.join(", ")
            ),
        }
    }
}

/// Match domains against prompt text and active file paths.
/// Pure matcher — returns every domain whose triggers fire, with the reason.
/// Dedup/suppression is owned by the hook layer, which hashes the fully
/// rendered output (rules + neighborhood + query results) — the only hash
/// that accurately reflects what would be injected.
pub fn match_domains<'a>(
    prompt: &str,
    domains: &'a [DomainDef],
    active_paths: &[String],
    ctx: &TriggerContext,
) -> Vec<DomainMatch<'a>> {
    let prompt_lower = prompt.to_lowercase();

    domains
        .iter()
        .filter_map(|d| {
            let (reason, path) = is_matched(d, &prompt_lower, active_paths, ctx)?;
            Some(DomainMatch { domain: d, reason, path })
        })
        .collect()
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

/// Determine if a domain matches the current context.
/// Returns Some(reason) on match, None on no match.
fn is_matched(
    domain: &DomainDef,
    prompt_lower: &str,
    active_paths: &[String],
    ctx: &TriggerContext,
) -> Option<(MatchReason, Option<String>)> {
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
        return Some((MatchReason::Always, None));
    }

    // Keyword match: a prompt keyword as whole words in the prompt text. A substring
    // test stood here until 0.14.0 and fired `base` on `database` (F29). Excludes keep
    // the substring test on purpose: a veto that fires too often errs toward silence.
    let keyword_hit = domain
        .prompt_keywords
        .iter()
        .any(|kw| contains_word(prompt_lower, &kw.to_lowercase()));

    // Path match: an active path lies under a trigger resolved against the tier the
    // domain came from. The path that satisfied it rides along so devmode can name
    // the file.
    let path_hit = domain.paths.iter().find_map(|dp| {
        // An inert trigger (unrooted, or a broadcast over registered projects) cannot
        // fire; doctor and devmode name it.
        let trigger = live_trigger(dp, domain.root.as_deref(), ctx)?;
        active_paths.iter().find(|ap| path_under(ap, &trigger)).cloned()
    });

    let reason = match (keyword_hit, path_hit.is_some()) {
        (true, true) => MatchReason::KeywordAndFilepath,
        (true, false) => MatchReason::Keyword,
        (false, true) => MatchReason::Filepath,
        (false, false) => return None,
    };
    Some((reason, path_hit))
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

/// One trigger, judged: the resolved path it names when it may fire, else its fault.
/// `root` is the tier the domain came from.
pub fn trigger_state(trigger: &str, root: Option<&str>, ctx: &TriggerContext) -> Result<String, TriggerFault> {
    let Some(resolved) = resolve_trigger(trigger, root, ctx.home.as_deref()) else {
        return Err(TriggerFault::Unrooted);
    };
    let covered: Vec<String> = ctx
        .registered
        .iter()
        .filter(|r| path_under(&r.path, &resolved))
        .map(|r| r.name.clone())
        .collect();
    if covered.len() >= 2 {
        Err(TriggerFault::Covers(covered))
    } else {
        Ok(resolved)
    }
}

/// The resolved path of a trigger that may fire; `None` for an inert one.
pub fn live_trigger(trigger: &str, root: Option<&str>, ctx: &TriggerContext) -> Option<String> {
    trigger_state(trigger, root, ctx).ok()
}

/// The fault of a trigger, if it has one.
pub fn trigger_fault(trigger: &str, root: Option<&str>, ctx: &TriggerContext) -> Option<TriggerFault> {
    trigger_state(trigger, root, ctx).err()
}

/// Every inert trigger across `domains` as (domain, trigger, fault), for doctor and devmode.
pub fn inert_triggers<'a>(domains: &'a [DomainDef], ctx: &TriggerContext) -> Vec<(&'a str, &'a str, TriggerFault)> {
    domains
        .iter()
        .flat_map(|d| {
            d.paths
                .iter()
                .filter_map(move |t| trigger_fault(t, d.root.as_deref(), ctx).map(|f| (d.name.as_str(), t.as_str(), f)))
        })
        .collect()
}

/// The one sentence doctor prints and `add-trigger` refuses with.
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
        assert_eq!(r("Documents\\Meet Caddy/").as_deref(), Some("c:/Users/x/Documents/Meet Caddy"));
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
        assert!(path_under("/home/x/Documents/Meet Caddy/notes.md", "/home/x/Documents/Meet Caddy"));
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

    /// A trigger over two or more registered projects is a broadcast: inert, named. One
    /// over a single project, or its own, is live. An unrooted one is inert too.
    #[test]
    fn a_broadcast_trigger_is_inert_and_named() {
        let reg = |n: &str, p: &str| Registered { name: n.into(), path: p.into() };
        let ctx = TriggerContext {
            home: Some("/home/u".into()),
            registered: vec![
                reg("agentic-os", "c:/Users/x/Documents/agentic-os"),
                reg("renda-group", "c:/Users/x/Documents/Meet Caddy/renda-group"),
                reg("meet-caddy", "c:/Users/x/Documents/Meet Caddy"),
                reg("stt", "c:/Users/x/Tools/stt"),
                reg("hub", "c:/Users/x/Tools/hub"),
            ],
        };
        let root = Some("C:/Users/x");
        assert_eq!(
            trigger_fault("Documents", root, &ctx),
            Some(TriggerFault::Covers(vec!["agentic-os".into(), "renda-group".into(), "meet-caddy".into()]))
        );
        assert_eq!(trigger_fault("tools", root, &ctx), Some(TriggerFault::Covers(vec!["stt".into(), "hub".into()])));
        assert_eq!(trigger_fault("Documents/Meet Caddy/renda-group", root, &ctx), None);
        assert_eq!(trigger_fault("*.md", root, &ctx), Some(TriggerFault::Unrooted));
        assert_eq!(trigger_fault("Documents", None, &ctx), Some(TriggerFault::Unrooted));

        let mut broad = make_domain("vintrix", "triggered", &[], &["Rule"]);
        broad.paths = vec!["Documents".into()];
        broad.root = Some("C:/Users/x".into());
        let touched = vec!["C:/Users/x/Documents/agentic-os/a.md".to_string()];
        assert!(match_domains("hello", std::slice::from_ref(&broad), &touched, &ctx).is_empty());
        let inert = inert_triggers(std::slice::from_ref(&broad), &ctx);
        assert_eq!(inert.len(), 1);
        assert_eq!(
            fault_sentence(inert[0].0, inert[0].1, &inert[0].2),
            "path trigger `Documents` on `vintrix` covers 3 registered projects (agentic-os, renda-group, meet-caddy); narrow it or set auto_inject = false"
        );
        // The same trigger with nothing registered under it is live.
        let alone = TriggerContext { home: Some("/home/u".into()), registered: vec![reg("agentic-os", "c:/Users/x/Documents/agentic-os")] };
        assert_eq!(match_domains("hello", std::slice::from_ref(&broad), &touched, &alone).len(), 1);
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
