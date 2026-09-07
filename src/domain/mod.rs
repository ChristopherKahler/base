pub mod link;
pub mod matcher;
pub mod query;
pub mod session;
pub mod sync;
pub mod transcript;
pub mod tier;

use std::path::Path;

use serde::{Deserialize, Serialize};

// ─── Domain data model ───────────────────────────────────────

/// A single rule on a domain. Backward-compatible deserialization:
/// `rules = ["do X"]` (bare string) and `rules = [{ text = "do X", rationale = "because Y" }]`
/// both parse. The rationale form lets CARL inject "Do X — because Y", which
/// aligns the model more reliably than the bare instruction (Phase 26).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum RuleEntry {
    Bare(String),
    Detailed {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rationale: Option<String>,
    },
}

impl RuleEntry {
    /// The instruction text, without rationale.
    pub fn text(&self) -> &str {
        match self {
            RuleEntry::Bare(s) => s,
            RuleEntry::Detailed { text, .. } => text,
        }
    }

    /// The rationale, if any (empty strings treated as absent).
    pub fn rationale(&self) -> Option<&str> {
        match self {
            RuleEntry::Bare(_) => None,
            RuleEntry::Detailed { rationale, .. } => {
                rationale.as_deref().filter(|r| !r.is_empty())
            }
        }
    }

    /// Render for injection: `text — because rationale` when rationale present,
    /// otherwise just `text`.
    pub fn render(&self) -> String {
        render_rule(self.text(), self.rationale())
    }
}

impl From<&str> for RuleEntry {
    fn from(s: &str) -> Self {
        RuleEntry::Bare(s.to_string())
    }
}

impl From<String> for RuleEntry {
    fn from(s: String) -> Self {
        RuleEntry::Bare(s)
    }
}

/// Render a rule's text + optional rationale for injection. Shared with the
/// graph-read paths, where rules come back as separate (text, rationale) terms.
pub fn render_rule(text: &str, rationale: Option<&str>) -> String {
    match rationale.filter(|r| !r.is_empty()) {
        Some(r) => format!("{text} — because {r}"),
        None => text.to_string(),
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DomainDef {
    pub name: String,
    #[serde(default = "default_mode")]
    pub mode: String, // "always" | "triggered"
    /// Keywords matched against user prompt text (natural language, user-configured).
    /// Backward-compatible: legacy `keywords` field deserializes here via alias.
    #[serde(default, alias = "keywords")]
    pub prompt_keywords: Vec<String>,
    /// Keywords matched against file content on tool-use (code-oriented, system-suggestable).
    #[serde(default)]
    pub file_keywords: Vec<String>,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub rules: Vec<RuleEntry>,
    /// External SPARQL query file to run on match (e.g., "icp-context" resolves to queries/icp-context.sparql).
    #[serde(default)]
    pub query: Option<String>,
    /// Format for query results: "table" | "list" | "prose". Defaults to "list".
    #[serde(default)]
    pub query_format: Option<String>,
    /// Star-commands to auto-activate when this domain loads (Phase 28), e.g.
    /// `commands = ["blunt", "analytical"]`. Referenced by name into commands.toml.
    #[serde(default)]
    pub commands: Vec<String>,
    /// How linked `commands` activate: "keyword" | "filepath" | "both" | "disabled".
    /// "disabled" preserves the config but suppresses auto-activation (Phase 28).
    #[serde(default = "default_command_activation")]
    pub command_activation: String,
    /// One-line role sentence injected as the first line of this domain's block (Phase 29).
    #[serde(default)]
    pub role: Option<String>,
    /// Default output routing for this domain: "file" | "inline" | "ask" (Phase 31).
    #[serde(default)]
    pub output_mode: Option<String>,
    /// Freeform format directive injected as the final line of this domain's block (Phase 32).
    #[serde(default)]
    pub format: Option<String>,
}

fn default_mode() -> String {
    "triggered".into()
}

fn default_command_activation() -> String {
    "both".into()
}

impl DomainDef {
    pub fn is_always(&self) -> bool {
        self.mode == "always"
    }

    /// Rule instruction texts only (no rationale). Used where stable, rationale-free
    /// strings are needed (e.g. extension round-trips).
    pub fn rule_texts(&self) -> Vec<String> {
        self.rules.iter().map(|r| r.text().to_string()).collect()
    }

    /// Rules rendered for injection (text + rationale). Used for dedup hashing so a
    /// rationale edit re-injects, and as the source for graph-free render paths.
    pub fn rendered_rules(&self) -> Vec<String> {
        self.rules.iter().map(|r| r.render()).collect()
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct DomainsFile {
    #[serde(default)]
    domain: Vec<DomainDef>,
}

// ─── Loading (tiered: global → workspace) ────────────────────

/// Load domains: global `~/.base-gbl/domains.toml` → workspace `.base/domains.toml`.
/// Returns empty Vec if neither exists (no error).
pub fn load_domains(cwd: &Path) -> Vec<DomainDef> {
    let mut domains = Vec::new();

    // Global
    if let Some(home) = crate::home::home_root()
        && let Ok(content) =
            std::fs::read_to_string(home.join(".base-gbl").join("domains.toml"))
        && let Ok(file) = toml::from_str::<DomainsFile>(&content)
    {
        domains = file.domain;
    }

    // Workspace (overlays global by name)
    if let Some(base_dir) = crate::config::find_workspace_base(cwd)
        && let Ok(content) = std::fs::read_to_string(base_dir.join("domains.toml"))
        && let Ok(file) = toml::from_str::<DomainsFile>(&content)
    {
        domains = merge_domains(domains, file.domain);
    }

    // Extension domains (Phase 22 — merged into normal pool, lowest priority)
    let extensions = crate::extension::load_extensions();
    for ext in &extensions {
        let ext_domains = crate::extension::extension_domains_to_domain_defs(ext);
        domains = merge_domains(domains, ext_domains);
    }

    domains
}

fn merge_domains(base: Vec<DomainDef>, overlay: Vec<DomainDef>) -> Vec<DomainDef> {
    let mut merged = base;
    for od in overlay {
        if let Some(pos) = merged.iter().position(|d| d.name == od.name) {
            merged[pos] = od;
        } else {
            merged.push(od);
        }
    }
    merged
}

// ─── Mutation (for CLI commands) ─────────────────────────────

/// Add a keyword or path trigger to a domain in workspace domains.toml.
/// Creates the domain (mode=triggered) if it doesn't exist.
pub fn add_trigger(
    cwd: &Path,
    global: bool,
    domain_name: &str,
    keyword: Option<&str>,
    path: Option<&str>,
) -> anyhow::Result<tier::Changed> {
    let (toml_path, tier) = tier::domains_toml_for_write(cwd, global);
    if let Some(parent) = toml_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file: DomainsFile = if toml_path.exists() {
        let content = std::fs::read_to_string(&toml_path)?;
        toml::from_str(&content)?
    } else {
        DomainsFile {
            domain: Vec::new(),
        }
    };

    // Find or create domain
    let domain = if let Some(pos) = file.domain.iter().position(|d| d.name == domain_name) {
        &mut file.domain[pos]
    } else {
        file.domain.push(DomainDef {
            name: domain_name.to_string(),
            mode: "triggered".to_string(),
            prompt_keywords: Vec::new(),
            file_keywords: Vec::new(),
            paths: Vec::new(),
            exclude: Vec::new(),
            rules: Vec::new(),
            query: None,
            query_format: None,
            commands: Vec::new(),
            command_activation: default_command_activation(),
            role: None,
            output_mode: None,
            format: None,
        });
        file.domain.last_mut().unwrap()
    };

    if let Some(kw) = keyword
        && !domain.prompt_keywords.contains(&kw.to_string())
    {
        domain.prompt_keywords.push(kw.to_string());
    }
    if let Some(p) = path
        && !domain.paths.contains(&p.to_string())
    {
        domain.paths.push(p.to_string());
    }

    // Atomic write via temp + rename
    let tmp_path = toml_path.with_extension("toml.tmp");
    let content = toml::to_string_pretty(&file)?;
    std::fs::write(&tmp_path, &content)?;
    std::fs::rename(&tmp_path, &toml_path)?;

    Ok(tier::Changed { tier, count: 1 })
}

/// Swap a path trigger on a domain: drop `old` (if present), add `new`. Used by
/// `base project repath` so a domain keeps matching its folder after the folder
/// moves. Returns Ok(true) when the domain existed and its triggers changed.
pub fn repath_trigger(
    cwd: &Path,
    domain_name: &str,
    old: Option<&str>,
    new: &str,
) -> anyhow::Result<bool> {
    let Some(base_dir) = crate::config::find_workspace_base(cwd) else {
        return Ok(false);
    };
    let toml_path = base_dir.join("domains.toml");
    if !toml_path.exists() {
        return Ok(false);
    }
    let mut file: DomainsFile = toml::from_str(&std::fs::read_to_string(&toml_path)?)?;
    let Some(domain) = file.domain.iter_mut().find(|d| d.name == domain_name) else {
        return Ok(false);
    };

    let before = domain.paths.clone();
    if let Some(o) = old {
        domain.paths.retain(|x| x != o);
    }
    if !domain.paths.iter().any(|x| x == new) {
        domain.paths.push(new.to_string());
    }
    if domain.paths == before {
        return Ok(false);
    }

    let tmp_path = toml_path.with_extension("toml.tmp");
    std::fs::write(&tmp_path, toml::to_string_pretty(&file)?)?;
    std::fs::rename(&tmp_path, &toml_path)?;
    Ok(true)
}

pub fn create_domain(
    cwd: &Path,
    global: bool,
    domain_name: &str,
    keyword: Option<&str>,
    path: Option<&str>,
) -> anyhow::Result<tier::Changed> {
    // #18. This took `_cwd` and hardcoded the global file, so `create` put the
    // domain somewhere the user was not standing -- which is what made #52 look
    // like "created without writing anything": it wrote to the other tier.
    let (toml_path, tier) = tier::domains_toml_for_write(cwd, global);
    if let Some(parent) = toml_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file: DomainsFile = if toml_path.exists() {
        toml::from_str(&std::fs::read_to_string(&toml_path)?)?
    } else {
        DomainsFile { domain: Vec::new() }
    };

    if file.domain.iter().any(|d| d.name.eq_ignore_ascii_case(domain_name)) {
        anyhow::bail!("Domain '{domain_name}' already exists");
    }

    let mut kws = Vec::new();
    if let Some(kw) = keyword { kws.push(kw.to_string()); }
    let mut ps = Vec::new();
    if let Some(p) = path { ps.push(p.to_string()); }

    file.domain.push(DomainDef {
        name: domain_name.to_string(),
        mode: "triggered".to_string(),
        prompt_keywords: kws,
        file_keywords: Vec::new(),
        paths: ps,
        exclude: Vec::new(),
        rules: Vec::new(),
        query: None,
        query_format: None,
        commands: Vec::new(),
        command_activation: default_command_activation(),
        role: None,
        output_mode: None,
        format: None,
    });

    let tmp = toml_path.with_extension("toml.tmp");
    std::fs::write(&tmp, toml::to_string_pretty(&file)?)?;
    std::fs::rename(&tmp, &toml_path)?;
    Ok(tier::Changed { tier, count: 1 })
}

pub fn remove_domain(
    cwd: &Path,
    global: bool,
    domain_name: &str,
) -> anyhow::Result<tier::Changed> {
    let (toml_path, tier) = tier::domains_toml_for_write(cwd, global);
    if !toml_path.exists() {
        return Ok(tier::Changed::none(tier));
    }

    let mut file: DomainsFile = toml::from_str(&std::fs::read_to_string(&toml_path)?)?;
    let before = file.domain.len();
    file.domain.retain(|d| !d.name.eq_ignore_ascii_case(domain_name));
    let removed = before - file.domain.len();
    if removed == 0 {
        return Ok(tier::Changed::none(tier));
    }

    let tmp = toml_path.with_extension("toml.tmp");
    std::fs::write(&tmp, toml::to_string_pretty(&file)?)?;
    std::fs::rename(&tmp, &toml_path)?;
    Ok(tier::Changed { tier, count: removed })
}

pub fn remove_trigger(
    cwd: &Path,
    global: bool,
    domain_name: &str,
    keyword: Option<&str>,
    path: Option<&str>,
) -> anyhow::Result<tier::Changed> {
    let (toml_path, tier) = tier::domains_toml_for_write(cwd, global);
    if !toml_path.exists() {
        return Ok(tier::Changed::none(tier));
    }

    let mut file: DomainsFile = toml::from_str(&std::fs::read_to_string(&toml_path)?)?;
    let Some(domain) = file
        .domain
        .iter_mut()
        .find(|d| d.name.eq_ignore_ascii_case(domain_name))
    else {
        return Ok(tier::Changed::none(tier));
    };

    // Count what actually went. This returned Ok(()) whether or not the trigger
    // was there, so "Trigger removed" printed for a keyword that never existed
    // -- and, with a same-named domain in the other tier, for a domain the user
    // never meant (#18).
    let mut removed = 0usize;
    if let Some(kw) = keyword {
        let before = domain.prompt_keywords.len();
        domain.prompt_keywords.retain(|k| k != kw);
        removed += before - domain.prompt_keywords.len();
    }
    if let Some(p) = path {
        let before = domain.paths.len();
        domain.paths.retain(|pp| pp != p);
        removed += before - domain.paths.len();
    }
    if removed == 0 {
        return Ok(tier::Changed::none(tier));
    }

    let tmp = toml_path.with_extension("toml.tmp");
    std::fs::write(&tmp, toml::to_string_pretty(&file)?)?;
    std::fs::rename(&tmp, &toml_path)?;
    Ok(tier::Changed { tier, count: removed })
}

/// List all domains (for CLI output).
pub fn list_domains(cwd: &Path, ns: &crate::config::NamespaceConfig) {
    let domains = load_domains(cwd);
    if domains.is_empty() {
        eprintln!("No domains configured.");
        return;
    }
    println!("| Domain | Mode | Prompt KW | File KW | Paths | Rules |");
    println!("|--------|------|-----------|---------|-------|-------|");
    for d in &domains {
        println!(
            "| {} | {} | {} | {} | {} | {} |",
            d.name,
            d.mode,
            d.prompt_keywords.len(),
            d.file_keywords.len(),
            d.paths.len(),
            {
                // Both tiers, because the row above them merges both and the
                // hook injects both. Counting the cwd tier alone described a
                // domain that spans tiers with a number that described one.
                // `declared` comes off the DomainDef, which load_domains has
                // ALREADY merged across tiers, so counting it per tier would
                // double it. Only the graph-backed rules are per tier.
                let (declared, aw) = rules_of(cwd, ns, d);
                let gbl = crate::home::home_root().map(|h| h.join(".base-gbl"));
                let ag = match &gbl {
                    Some(g) if g.as_path() != cwd => rules_of(g, ns, d).1,
                    _ => Vec::new(),
                };
                let w = declared.len() + aw.len();
                let g = ag.len();
                if g == 0 { format!("{w}") } else { format!("{w}+{g}") }
            },
        );
    }
    println!("\nRules column is workspace+global. `base rule list --domain X` shows them labelled.");
}

/// Every rule a domain injects: the ones declared in `domains.toml`, and the
/// ones added with `base rule add`.
///
/// The two live in different stores by design. A CLI-added rule is written to
/// the graph at `rule/<slug>/cli-N`, in its own IRI namespace, so that a
/// `base sync` rebuilding a domain from `domains.toml` cannot delete it. The
/// cost of that split was this: the readers only ever looked at the file, so
/// `base domain get` answered `Rules (0)` for a domain whose rules were being
/// injected on every matching tool call, and the natural reading of that is
/// that `base rule add` had failed (#38).
///
/// A missing or unreadable graph yields the file half alone rather than an
/// error: these are display commands, and a domain's declared rules are still
/// worth showing outside a workspace.
pub fn rules_of(
    cwd: &Path,
    ns: &crate::config::NamespaceConfig,
    d: &DomainDef,
) -> (Vec<String>, Vec<(u32, String)>) {
    let declared: Vec<String> = d.rules.iter().map(|r| r.render()).collect();
    // false: the domain block is a SERVING surface (auk, 2026-09-07). What the agent
    // receives is the rule that stands, never the one it replaced.
    let added: Vec<(u32, String)> = crate::crud::rule::fetch(cwd, ns, &d.name, false)
        .unwrap_or_default()
        .into_iter()
        .map(|(pri, text, _)| (pri, text))
        .collect();
    (declared, added)
}

/// Show a specific domain's full config (for CLI output).
pub fn get_domain(cwd: &Path, ns: &crate::config::NamespaceConfig, name: &str) {
    let domains = load_domains(cwd);
    match domains.iter().find(|d| d.name == name) {
        Some(d) => {
            println!("Domain: {}", d.name);
            println!("Mode: {}", d.mode);
            if !d.prompt_keywords.is_empty() {
                println!("Prompt Keywords: {}", d.prompt_keywords.join(", "));
            }
            if !d.file_keywords.is_empty() {
                println!("File Keywords: {}", d.file_keywords.join(", "));
            }
            if !d.paths.is_empty() {
                println!("Paths: {}", d.paths.join(", "));
            }
            if !d.exclude.is_empty() {
                println!("Exclude: {}", d.exclude.join(", "));
            }
            if let Some(role) = &d.role {
                println!("Role: {role}");
            }
            if !d.commands.is_empty() {
                println!(
                    "Commands: {} (activation: {})",
                    d.commands.join(", "),
                    d.command_activation
                );
            }
            if let Some(om) = &d.output_mode {
                println!("Output mode: {om}");
            }
            if let Some(fmt) = &d.format {
                println!("Format: {fmt}");
            }
            let (declared, added) = rules_of(cwd, ns, d);
            println!("Rules ({}):", declared.len() + added.len());
            for (i, rule) in declared.iter().enumerate() {
                println!("  {i}. {rule}");
            }
            // Numbered by their own index, which is what `base rule remove
            // --index` takes; the two stores number independently.
            for (n, text) in &added {
                println!("  [cli-{n}] {text}");
            }
        }
        None => eprintln!("Domain '{name}' not found."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml_str: &str) -> DomainDef {
        #[derive(Deserialize)]
        struct F {
            domain: Vec<DomainDef>,
        }
        toml::from_str::<F>(toml_str).unwrap().domain.pop().unwrap()
    }

    // ─── Phase 26: rule rationale ────────────────────────────

    #[test]
    fn bare_string_rule_parses_without_rationale() {
        let d = parse("[[domain]]\nname = \"X\"\nrules = [\"do the thing\"]\n");
        assert_eq!(d.rules.len(), 1);
        assert_eq!(d.rules[0].text(), "do the thing");
        assert_eq!(d.rules[0].rationale(), None);
        assert_eq!(d.rules[0].render(), "do the thing");
    }

    #[test]
    fn detailed_rule_parses_and_renders_rationale() {
        let d = parse(
            "[[domain]]\nname = \"X\"\nrules = [{ text = \"do X\", rationale = \"it aligns Y\" }]\n",
        );
        assert_eq!(d.rules[0].text(), "do X");
        assert_eq!(d.rules[0].rationale(), Some("it aligns Y"));
        assert_eq!(d.rules[0].render(), "do X — because it aligns Y");
    }

    #[test]
    fn detailed_rule_without_rationale_renders_text_only() {
        let d = parse("[[domain]]\nname = \"X\"\nrules = [{ text = \"do X\" }]\n");
        assert_eq!(d.rules[0].rationale(), None);
        assert_eq!(d.rules[0].render(), "do X");
    }

    #[test]
    fn empty_rationale_treated_as_absent() {
        let d = parse("[[domain]]\nname = \"X\"\nrules = [{ text = \"do X\", rationale = \"\" }]\n");
        assert_eq!(d.rules[0].rationale(), None);
        assert_eq!(d.rules[0].render(), "do X");
    }

    #[test]
    fn mixed_bare_and_detailed_rules_in_one_domain() {
        let d = parse(
            "[[domain]]\nname = \"X\"\nrules = [\"bare one\", { text = \"rich one\", rationale = \"reason\" }]\n",
        );
        assert_eq!(d.rule_texts(), vec!["bare one", "rich one"]);
        assert_eq!(
            d.rendered_rules(),
            vec!["bare one", "rich one — because reason"]
        );
    }

    // ─── Phases 28/29/31/32: new domain steering fields ──────

    #[test]
    fn steering_fields_default_when_absent() {
        let d = parse("[[domain]]\nname = \"X\"\nrules = []\n");
        assert!(d.commands.is_empty());
        assert_eq!(d.command_activation, "both"); // Phase 28 default
        assert_eq!(d.role, None); // Phase 29
        assert_eq!(d.output_mode, None); // Phase 31
        assert_eq!(d.format, None); // Phase 32
    }

    #[test]
    fn steering_fields_parse_when_present() {
        let d = parse(
            "[[domain]]\nname = \"X\"\nrules = []\n\
             commands = [\"blunt\", \"analytical\"]\n\
             command_activation = \"keyword\"\n\
             role = \"You are a strategist.\"\n\
             output_mode = \"file\"\n\
             format = \"Prefer tables.\"\n",
        );
        assert_eq!(d.commands, vec!["blunt", "analytical"]);
        assert_eq!(d.command_activation, "keyword");
        assert_eq!(d.role.as_deref(), Some("You are a strategist."));
        assert_eq!(d.output_mode.as_deref(), Some("file"));
        assert_eq!(d.format.as_deref(), Some("Prefer tables."));
    }

    #[test]
    fn rule_entry_from_str_yields_bare() {
        let r: RuleEntry = "hello".into();
        assert_eq!(r, RuleEntry::Bare("hello".into()));
        assert_eq!(r.render(), "hello");
    }
}
