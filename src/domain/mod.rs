pub mod matcher;
pub mod query;
pub mod session;
pub mod sync;
pub mod transcript;

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
    domain_name: &str,
    keyword: Option<&str>,
    path: Option<&str>,
) -> anyhow::Result<()> {
    let base_dir = crate::config::find_workspace_base(cwd)
        .unwrap_or_else(|| cwd.join(".base"));
    std::fs::create_dir_all(&base_dir)?;

    let toml_path = base_dir.join("domains.toml");
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

    Ok(())
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
    _cwd: &Path,
    domain_name: &str,
    keyword: Option<&str>,
    path: Option<&str>,
) -> anyhow::Result<()> {
    let home = crate::home::home_root().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    let toml_path = home.join(".base-gbl").join("domains.toml");
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
    Ok(())
}

pub fn remove_domain(_cwd: &Path, domain_name: &str) -> anyhow::Result<bool> {
    let home = crate::home::home_root().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    let toml_path = home.join(".base-gbl").join("domains.toml");
    if !toml_path.exists() { return Ok(false); }

    let mut file: DomainsFile = toml::from_str(&std::fs::read_to_string(&toml_path)?)?;
    let before = file.domain.len();
    file.domain.retain(|d| !d.name.eq_ignore_ascii_case(domain_name));
    if file.domain.len() == before { return Ok(false); }

    let tmp = toml_path.with_extension("toml.tmp");
    std::fs::write(&tmp, toml::to_string_pretty(&file)?)?;
    std::fs::rename(&tmp, &toml_path)?;
    Ok(true)
}

pub fn remove_trigger(
    _cwd: &Path,
    domain_name: &str,
    keyword: Option<&str>,
    path: Option<&str>,
) -> anyhow::Result<()> {
    let home = crate::home::home_root().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    let toml_path = home.join(".base-gbl").join("domains.toml");
    if !toml_path.exists() { anyhow::bail!("domains.toml not found"); }

    let mut file: DomainsFile = toml::from_str(&std::fs::read_to_string(&toml_path)?)?;
    let domain = file.domain.iter_mut()
        .find(|d| d.name.eq_ignore_ascii_case(domain_name))
        .ok_or_else(|| anyhow::anyhow!("Domain '{domain_name}' not found"))?;

    if let Some(kw) = keyword {
        domain.prompt_keywords.retain(|k| k != kw);
    }
    if let Some(p) = path {
        domain.paths.retain(|pp| pp != p);
    }

    let tmp = toml_path.with_extension("toml.tmp");
    std::fs::write(&tmp, toml::to_string_pretty(&file)?)?;
    std::fs::rename(&tmp, &toml_path)?;
    Ok(())
}

/// List all domains (for CLI output).
pub fn list_domains(cwd: &Path) {
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
            d.rules.len(),
        );
    }
}

/// Show a specific domain's full config (for CLI output).
pub fn get_domain(cwd: &Path, name: &str) {
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
            println!("Rules ({}):", d.rules.len());
            for (i, rule) in d.rules.iter().enumerate() {
                println!("  {i}. {}", rule.render());
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
