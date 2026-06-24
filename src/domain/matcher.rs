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
) -> Vec<DomainMatch<'a>> {
    let prompt_lower = prompt.to_lowercase();

    domains
        .iter()
        .filter_map(|d| {
            let reason = is_matched(d, &prompt_lower, active_paths)?;
            Some(DomainMatch { domain: d, reason })
        })
        .collect()
}

/// Determine if a domain matches the current context.
/// Returns Some(reason) on match, None on no match.
fn is_matched(domain: &DomainDef, prompt_lower: &str, active_paths: &[String]) -> Option<MatchReason> {
    // Always-on domains always match
    if domain.is_always() {
        return Some(MatchReason::Always);
    }

    // Check exclude patterns first — any match vetoes the domain
    for pattern in &domain.exclude {
        if prompt_lower.contains(&pattern.to_lowercase()) {
            return None;
        }
    }

    // Keyword match: any prompt_keyword substring in prompt text
    let keyword_hit = domain
        .prompt_keywords
        .iter()
        .any(|kw| prompt_lower.contains(&kw.to_lowercase()));

    // Path match: any active file path starts with any domain path trigger
    let path_hit = domain.paths.iter().any(|dp| {
        active_paths
            .iter()
            .any(|ap| ap.starts_with(dp) || ap.contains(dp))
    });

    match (keyword_hit, path_hit) {
        (true, true) => Some(MatchReason::KeywordAndFilepath),
        (true, false) => Some(MatchReason::Keyword),
        (false, true) => Some(MatchReason::Filepath),
        (false, false) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_domain(name: &str, mode: &str, keywords: &[&str], rules: &[&str]) -> DomainDef {
        DomainDef {
            name: name.into(),
            mode: mode.into(),
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
        let matched = match_domains("anything", &domains, &[]);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].domain.name, "global");
        assert_eq!(matched[0].reason, MatchReason::Always);
    }

    #[test]
    fn keyword_match() {
        let domains = vec![make_domain("dev", "triggered", &["fix bug"], &["Dev rule"])];

        let matched = match_domains("please fix bug in auth", &domains, &[]);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].reason, MatchReason::Keyword);

        let matched = match_domains("check my calendar", &domains, &[]);
        assert!(matched.is_empty());
    }

    #[test]
    fn keyword_case_insensitive() {
        let domains = vec![make_domain("dev", "triggered", &["Fix Bug"], &["Rule"])];
        let matched = match_domains("FIX BUG please", &domains, &[]);
        assert_eq!(matched.len(), 1);
    }

    #[test]
    fn path_match() {
        let mut domain = make_domain("dev", "triggered", &[], &["Rule"]);
        domain.paths = vec!["src/".into()];
        let domains = vec![domain];

        let matched = match_domains(
            "hello",
            &domains,
            &["src/main.rs".into()],
        );
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].reason, MatchReason::Filepath);
    }

    #[test]
    fn both_keyword_and_path() {
        let mut domain = make_domain("dev", "triggered", &["code"], &["Rule"]);
        domain.paths = vec!["src/".into()];
        let domains = vec![domain];

        let matched = match_domains(
            "write code",
            &domains,
            &["src/main.rs".into()],
        );
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].reason, MatchReason::KeywordAndFilepath);
    }

    #[test]
    fn exclude_vetoes_match() {
        let mut domain = make_domain("dev", "triggered", &["code"], &["Rule"]);
        domain.exclude = vec!["review only".into()];
        let domains = vec![domain];

        let matched = match_domains("write code for this", &domains, &[]);
        assert_eq!(matched.len(), 1);

        let matched = match_domains("review only the code", &domains, &[]);
        assert!(matched.is_empty());
    }

    #[test]
    fn no_domains_no_match() {
        let matched = match_domains("anything", &[], &[]);
        assert!(matched.is_empty());
    }

    #[test]
    fn no_rules_domain_still_matched_but_empty() {
        let domains = vec![make_domain("empty", "always", &[], &[])];
        let matched = match_domains("anything", &domains, &[]);
        assert_eq!(matched.len(), 1);
        assert!(matched[0].domain.rules.is_empty());
    }
}
