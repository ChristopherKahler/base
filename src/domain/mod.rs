pub mod bm25;
pub mod global_decisions;
pub mod link;
pub mod matcher;
pub mod paths;
pub mod query;
pub mod replay;
pub mod rule_test;
pub mod rules;
pub mod score_index;
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
        /// The rule's own triggers: `[[domain.rules.match]]` tables (spec F2, F3, F12). Absent on every rule
        /// until an operator approves a conversion (K4), and not written back when empty, so a file with no
        /// matchers round-trips unchanged.
        #[serde(default, rename = "match", skip_serializing_if = "Vec::is_empty")]
        matchers: Vec<crate::domain::rules::Matcher>,
        /// Prompts that must serve this rule, and prompts that must not (K2a, `base rule test`). Not written back when
        /// empty, so a file with no tests round-trips unchanged.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        fires_on: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        quiet_on: Vec<String>,
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

    /// The rule's own matchers, as written. Empty for a bare string and for every unconverted rule.
    pub fn matchers(&self) -> &[crate::domain::rules::Matcher] {
        match self {
            RuleEntry::Bare(_) => &[],
            RuleEntry::Detailed { matchers, .. } => matchers,
        }
    }

    /// The rule's test prompts (K2a): those that must serve it, and those that must not. Empty for a bare string.
    pub fn tests(&self) -> (&[String], &[String]) {
        match self {
            RuleEntry::Bare(_) => (&[], &[]),
            RuleEntry::Detailed { fires_on, quiet_on, .. } => (fires_on, quiet_on),
        }
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
    /// Names this domain had before `base project rename` (BO-24, R4). `--domain`, `domain get` and slug lookups
    /// read one of them as this domain and say so. Not written back when empty, so a file without it round-trips
    /// unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    #[serde(default = "default_mode")]
    pub mode: String, // "always" | "triggered"
    /// `auto_inject = false` keeps this domain out of every automatic injection — the
    /// prompt hook, the tool hook and the session-start cheat-sheet — whatever its mode
    /// or triggers (F29 D3). Explicit
    /// readers (`base context`, `base recall`, star commands) still see it. Absent means
    /// true, and true is not written back, so a round-trip leaves the file as it was.
    #[serde(default = "default_auto_inject", skip_serializing_if = "is_auto_inject")]
    pub auto_inject: bool,
    /// The tier root this domain was loaded from (home for the global tier, the
    /// workspace root for the workspace tier). Relative path triggers resolve against
    /// it; never read from or written to domains.toml (F29).
    #[serde(skip)]
    pub root: Option<String>,
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

fn default_auto_inject() -> bool {
    true
}

fn is_auto_inject(auto_inject: &bool) -> bool {
    *auto_inject
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

    // Global — relative path triggers resolve against the home directory.
    let home = crate::home::home_root();
    if let Some(home) = &home
        && let Ok(content) =
            std::fs::read_to_string(home.join(".base-gbl").join("domains.toml"))
        && let Ok(file) = toml::from_str::<DomainsFile>(&content)
    {
        domains = rooted(file.domain, Some(home));
    }

    // Workspace (overlays global by name) — triggers resolve against the workspace root.
    let ws_root = crate::config::find_workspace_base(cwd).and_then(|b| b.parent().map(Path::to_path_buf));
    if let Some(base_dir) = crate::config::find_workspace_base(cwd)
        && let Ok(content) = std::fs::read_to_string(base_dir.join("domains.toml"))
        && let Ok(file) = toml::from_str::<DomainsFile>(&content)
    {
        domains = merge_domains(domains, rooted(file.domain, ws_root.as_deref()));
    }

    // Extension domains (Phase 22 — merged into normal pool, lowest priority). Their
    // triggers name workspace state dirs (`.outpost/`), so they root at the workspace
    // when there is one, else at home.
    let extensions = crate::extension::load_extensions();
    for ext in &extensions {
        let ext_domains = rooted(
            crate::extension::extension_domains_to_domain_defs(ext),
            ws_root.as_deref().or(home.as_deref()),
        );
        domains = merge_domains(domains, ext_domains);
    }

    domains
}

/// Stamp the tier root every domain in `domains` came from (F29): relative path
/// triggers resolve against it, and a domain with no root has no rooted relative trigger.
fn rooted(mut domains: Vec<DomainDef>, root: Option<&Path>) -> Vec<DomainDef> {
    let root = root.map(|r| r.display().to_string());
    for d in &mut domains {
        d.root = root.clone();
    }
    domains
}

/// `add_trigger` refused a path trigger that could never fire (F29 step 6). Typed so
/// `project add` can tell this apart from an I/O or parse failure and degrade to a warning.
#[derive(Debug)]
pub struct TriggerRefused(pub String);

impl std::fmt::Display for TriggerRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TriggerRefused {}

/// One tier's domains.toml, rooted at that tier (F29): the reader doctor uses to judge a
/// tier on its own. Absent or unparsable is empty; the loaders fail open by design and
/// doctor reports a corrupt file through `config_errors`.
pub fn load_domains_file(path: &Path, root: Option<&Path>) -> Vec<DomainDef> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    match toml::from_str::<DomainsFile>(&content) {
        Ok(file) => rooted(file.domain, root),
        Err(_) => Vec::new(),
    }
}

/// The registered projects as the path rules see them: every `ops:Project`, its folder
/// resolved against the tier its record lives in (the workspace root for the workspace
/// graph, home for every other graph) in the shape `resolve_trigger` produces, so a
/// trigger and a project folder compare (F29 step 6), with its slug, its parent link and
/// `nested` (D13). A project with no folder is listed with an empty one: it owns nothing,
/// and it can still be a parent.
pub fn registered_projects(
    store: &oxigraph::store::Store,
    ns: &crate::config::NamespaceConfig,
    cwd: &Path,
) -> Vec<matcher::Registered> {
    let p = &ns.prefix;
    let sparql = format!(
        "{}\nSELECT ?g ?proj ?name ?path ?parent ?nested WHERE {{ GRAPH ?g {{ ?proj a {p}:Project ; {p}:name ?name . \
         OPTIONAL {{ ?proj {p}:path ?path }} OPTIONAL {{ ?proj {p}:parentProject ?parent }} \
         OPTIONAL {{ ?proj {p}:nested ?nested }} }} }}",
        crate::crud::prefixes(ns)
    );
    let ws_graph = crate::crud::workspace_graph_iri(ns, &crate::crud::workspace_slug(cwd));
    let ws_root = crate::config::find_workspace_base(cwd).and_then(|b| b.parent().map(|r| r.display().to_string()));
    let home = crate::home::home_root().map(|h| h.display().to_string());
    let project_iri = crate::crud::build_iri(ns, "project", "");
    let mut out = Vec::new();
    if let Ok(oxigraph::sparql::QueryResults::Solutions(rows)) = crate::store::query(store, &sparql) {
        for row in rows.filter_map(|r| r.ok()) {
            let lit = |k: &str| {
                row.get(k).and_then(|t| match t.into() {
                    oxigraph::model::TermRef::Literal(l) => Some(l.value().to_string()),
                    _ => None,
                })
            };
            let named = |k: &str| {
                row.get(k).and_then(|t| match t.into() {
                    oxigraph::model::TermRef::NamedNode(n) => Some(n.as_str().to_string()),
                    _ => None,
                })
            };
            let Some(name) = lit("name") else {
                continue;
            };
            let slug = named("proj")
                .and_then(|iri| iri.strip_prefix(&project_iri).map(String::from))
                .unwrap_or_else(|| crate::crud::slugify(&name));
            let parent = named("parent").and_then(|iri| iri.strip_prefix(&project_iri).map(String::from));
            let nested = lit("nested").is_some_and(|v| v == "true");
            let graph = named("g");
            let root = if graph.as_deref() == Some(ws_graph.as_str()) { ws_root.as_deref() } else { home.as_deref() };
            let path = lit("path")
                .and_then(|path| matcher::resolve_trigger(&path, root, home.as_deref()))
                .unwrap_or_default();
            out.push(matcher::Registered { name, path, slug, parent, nested });
        }
    }
    out
}

/// The trigger context a CLI reader builds for itself: home plus the registered
/// projects of the merged store. The hooks build theirs from the store they already hold.
pub fn trigger_context(cwd: &Path) -> matcher::TriggerContext {
    let ns = crate::config::BaseConfig::load(cwd).namespace;
    matcher::TriggerContext {
        home: crate::home::home_root().map(|h| h.display().to_string()),
        registered: crate::store::load_merged(cwd)
            .as_ref()
            .map(|s| registered_projects(s, &ns, cwd))
            .unwrap_or_default(),
    }
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

    // P3: stored as its full path, and refused before anything is written when it cannot
    // be rooted or holds other registered projects (D1).
    let path = match path {
        Some(p) => Some(checked_trigger(cwd, &toml_path, domain_name, p)?),
        None => None,
    };
    let place = place_in(&toml_path);

    // Find or create domain
    let domain = if let Some(pos) = file.domain.iter().position(|d| d.name == domain_name) {
        &mut file.domain[pos]
    } else {
        file.domain.push(DomainDef {
            name: domain_name.to_string(),
            aliases: Vec::new(),
            mode: "triggered".to_string(),
            auto_inject: true,
            root: None,
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
    // One trigger per place: `Documents/x` already written relative is the same trigger.
    if let Some(p) = path
        && !domain.paths.iter().any(|x| place(x).is_some_and(|px| Some(px) == place(&p)))
    {
        domain.paths.push(p);
    }

    // Atomic write via temp + rename
    let tmp_path = toml_path.with_extension("toml.tmp");
    let content = toml::to_string_pretty(&file)?;
    std::fs::write(&tmp_path, &content)?;
    std::fs::rename(&tmp_path, &toml_path)?;

    Ok(tier::Changed { tier, count: 1 })
}

/// The full path `add-trigger` stores for `raw` in the tier `global` picks (P3), for the CLI to say what it wrote.
pub fn trigger_spelling(cwd: &Path, global: bool, raw: &str) -> Option<String> {
    let (toml_path, _) = tier::domains_toml_for_write(cwd, global);
    let home = crate::home::home_root();
    crate::crud::project::absolute_path(raw, toml_path.parent().and_then(Path::parent), home.as_deref())
}

/// The place a trigger in the domains.toml at `toml_path` names, resolved against that file's tier root, for
/// comparing two spellings of one trigger.
fn place_in(toml_path: &Path) -> impl Fn(&str) -> Option<String> {
    let root = toml_path.parent().and_then(Path::parent).map(|r| r.display().to_string());
    let home = crate::home::home_root().map(|h| h.display().to_string());
    move |t: &str| matcher::resolve_trigger(t, root.as_deref(), home.as_deref())
}

/// `raw` as the full path a path trigger of `domain` is stored as (P3), or the refusal. Relative input is the tier
/// root's, which is where a relative trigger in that file has always resolved: the workspace root, or home for the
/// global tier; `~` is home. Refused, before anything is written, when it cannot be rooted (a glob, an empty path),
/// or when it holds registered projects other than the domain's own project and its children (D1): the refusal
/// names them and the domain's own folder.
fn checked_trigger(cwd: &Path, toml_path: &Path, domain: &str, raw: &str) -> anyhow::Result<String> {
    let root = toml_path.parent().and_then(Path::parent);
    let home = crate::home::home_root();
    let t = raw.trim();
    let unrooted = || TriggerRefused(matcher::fault_sentence(domain, raw, &matcher::TriggerFault::Unrooted));
    if t.contains(['*', '?']) {
        return Err(unrooted().into());
    }
    let Some(full) = crate::crud::project::absolute_path(t, root, home.as_deref()) else {
        return Err(unrooted().into());
    };
    let ctx = trigger_context(cwd);
    let Some(resolved) = matcher::resolve_trigger(&full, None, ctx.home.as_deref()) else {
        return Err(unrooted().into());
    };
    let broad = matcher::trigger_breadth(&resolved, domain, &ctx);
    if broad.is_empty() {
        return Ok(full);
    }
    let own = crate::crud::slugify(domain);
    let folder = ctx.registered.iter().find(|r| r.slug == own && !r.path.is_empty());
    // The domain's own project folder, holding a project not linked to it as a child: the fix is the link.
    if folder.is_some_and(|f| matcher::path_under(&resolved, &f.path) && matcher::path_under(&f.path, &resolved)) {
        return Err(TriggerRefused(format!(
            "{full} is {domain}'s folder and also holds {} that {domain} is not the parent of: link each one first \
             (base project update <slug> --parent {own}), then add the trigger.",
            matcher::count_projects(&broad)
        ))
        .into());
    }
    let mut msg = format!(
        "{full} contains {}. A trigger must be one project's own folder or a file.",
        matcher::count_projects(&broad)
    );
    if let Some(folder) = folder {
        let spelled = crate::crud::project::absolute_path(&folder.path, None, None).unwrap_or_else(|| folder.path.clone());
        msg.push_str(&format!(" {domain}'s folder is {spelled}."));
    }
    Err(TriggerRefused(msg).into())
}

/// Set each listed domain's whole path list, and its `auto_inject`, in the domains.toml at `toml_path`: one read, one
/// atomic write (`base domain paths --apply`, P6). A domain the file does not hold is an error, and nothing is written.
pub fn set_paths(toml_path: &Path, entries: &[(String, Vec<String>, bool)]) -> anyhow::Result<()> {
    let mut file: DomainsFile = toml::from_str(&std::fs::read_to_string(toml_path)?)?;
    for (name, paths, auto_inject) in entries {
        let Some(d) = file.domain.iter_mut().find(|d| d.name == *name) else {
            anyhow::bail!("no domain '{name}' in {}", toml_path.display());
        };
        d.paths = paths.clone();
        d.auto_inject = *auto_inject;
    }
    let tmp = toml_path.with_extension("toml.tmp");
    std::fs::write(&tmp, toml::to_string_pretty(&file)?)?;
    std::fs::rename(&tmp, toml_path)?;
    Ok(())
}

/// Set one rule's test prompts in the domains.toml at `toml_path` (K2a, `base rule update`): the rule of `domain` whose
/// [`rules::rule_id`] is `id`. One read, one atomic write. A plain-string rule becomes an inline table to hold them; a
/// table left with nothing but its text goes back to a plain string.
///
/// The file is written back the way `add_trigger`, `remove_trigger` and `set_paths` write it, through
/// `toml::to_string_pretty`: in a file base wrote, only that entry's line changes, and clearing the tests restores it
/// byte for byte (both pinned by `rule_tests_stored_with_rule`). A file written by hand loses its comments and its own
/// layout on the first write, as it does under every other domains.toml writer. `Ok(false)` when the file has no such
/// rule; nothing is written.
pub fn set_rule_tests(toml_path: &Path, domain: &str, id: &str, tests: &rules::RuleTests) -> anyhow::Result<bool> {
    let mut file: DomainsFile = toml::from_str(&std::fs::read_to_string(toml_path)?)?;
    let want = crate::crud::slugify(domain);
    let Some(d) = file.domain.iter_mut().find(|d| crate::crud::slugify(&d.name) == want) else {
        return Ok(false);
    };
    let name = d.name.clone();
    let Some(r) = d.rules.iter_mut().find(|r| rules::rule_id(&name, r.text()) == id) else {
        return Ok(false);
    };
    let (fires_on, quiet_on) = (tests.fires_on.clone(), tests.quiet_on.clone());
    let next = match std::mem::replace(r, RuleEntry::Bare(String::new())) {
        RuleEntry::Bare(text) => RuleEntry::Detailed { text, rationale: None, matchers: Vec::new(), fires_on, quiet_on },
        RuleEntry::Detailed { text, rationale, matchers, .. } => RuleEntry::Detailed { text, rationale, matchers, fires_on, quiet_on },
    };
    // Nothing but its text left: a plain string again.
    *r = match next {
        RuleEntry::Detailed { text, rationale: None, matchers, fires_on, quiet_on }
            if matchers.is_empty() && fires_on.is_empty() && quiet_on.is_empty() =>
        {
            RuleEntry::Bare(text)
        }
        other => other,
    };
    let tmp = toml_path.with_extension("toml.tmp");
    std::fs::write(&tmp, toml::to_string_pretty(&file)?)?;
    std::fs::rename(&tmp, toml_path)?;
    Ok(true)
}

/// Set one rule's matchers in the domains.toml at `toml_path` (BO-16, an approved keyword gap on a rule with words of
/// its own): the rule of `domain` whose [`rules::rule_id`] is `id`. Written as [`set_rule_tests`] writes; a table left
/// with nothing but its text goes back to a plain string. `Ok(false)` when the file has no such rule.
pub fn set_rule_matchers(toml_path: &Path, domain: &str, id: &str, matchers: &[rules::Matcher]) -> anyhow::Result<bool> {
    let mut file: DomainsFile = toml::from_str(&std::fs::read_to_string(toml_path)?)?;
    let want = crate::crud::slugify(domain);
    let Some(d) = file.domain.iter_mut().find(|d| crate::crud::slugify(&d.name) == want) else {
        return Ok(false);
    };
    let name = d.name.clone();
    let Some(r) = d.rules.iter_mut().find(|r| rules::rule_id(&name, r.text()) == id) else {
        return Ok(false);
    };
    let matchers = matchers.to_vec();
    let next = match std::mem::replace(r, RuleEntry::Bare(String::new())) {
        RuleEntry::Bare(text) => RuleEntry::Detailed { text, rationale: None, matchers, fires_on: Vec::new(), quiet_on: Vec::new() },
        RuleEntry::Detailed { text, rationale, fires_on, quiet_on, .. } => RuleEntry::Detailed { text, rationale, matchers, fires_on, quiet_on },
    };
    *r = match next {
        RuleEntry::Detailed { text, rationale: None, matchers, fires_on, quiet_on }
            if matchers.is_empty() && fires_on.is_empty() && quiet_on.is_empty() =>
        {
            RuleEntry::Bare(text)
        }
        other => other,
    };
    let tmp = toml_path.with_extension("toml.tmp");
    std::fs::write(&tmp, toml::to_string_pretty(&file)?)?;
    std::fs::rename(&tmp, toml_path)?;
    Ok(true)
}

/// Remove one rule of `domain` from the domains.toml at `toml_path`, by its [`rules::rule_id`] (BO-16: a rewritten file
/// rule moves to the graph, where the old wording is kept superseded). `Ok(false)` when the file has no such rule.
pub fn remove_rule(toml_path: &Path, domain: &str, id: &str) -> anyhow::Result<bool> {
    let mut file: DomainsFile = toml::from_str(&std::fs::read_to_string(toml_path)?)?;
    let want = crate::crud::slugify(domain);
    let Some(d) = file.domain.iter_mut().find(|d| crate::crud::slugify(&d.name) == want) else {
        return Ok(false);
    };
    let name = d.name.clone();
    let before = d.rules.len();
    d.rules.retain(|r| rules::rule_id(&name, r.text()) != id);
    if d.rules.len() == before {
        return Ok(false);
    }
    let tmp = toml_path.with_extension("toml.tmp");
    std::fs::write(&tmp, toml::to_string_pretty(&file)?)?;
    std::fs::rename(&tmp, toml_path)?;
    Ok(true)
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

    // By the place a trigger names, not its spelling: a project stored as `Documents/x` and moved to an absolute
    // folder (F25b, 0.16.0) drops its `Documents/x` trigger whether it was written `Documents/x` or `C:/.../x`.
    let root = base_dir.parent().map(|r| r.display().to_string());
    let home = crate::home::home_root().map(|h| h.display().to_string());
    let place = |t: &str| matcher::resolve_trigger(t, root.as_deref(), home.as_deref());
    let same = |a: &str, b: &str| a == b || place(a).is_some_and(|pa| Some(pa) == place(b));
    let before = domain.paths.clone();
    if let Some(o) = old {
        domain.paths.retain(|x| !same(x, o));
    }
    if !domain.paths.iter().any(|x| same(x, new)) {
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
    // R4: an old name still reads as the renamed domain everywhere, so a new domain under it would never be reached.
    if let Some(d) = renamed_from(&load_domains(cwd), domain_name) {
        anyhow::bail!("'{domain_name}' is an old name of domain '{}' (renamed); pick another name", d.name);
    }

    let mut kws = Vec::new();
    if let Some(kw) = keyword { kws.push(kw.to_string()); }
    // The same rule as `add-trigger` (P3): a full path, never one that holds other projects.
    let mut ps = Vec::new();
    if let Some(p) = path { ps.push(checked_trigger(cwd, &toml_path, domain_name, p)?); }

    file.domain.push(DomainDef {
        name: domain_name.to_string(),
        aliases: Vec::new(),
        mode: "triggered".to_string(),
        auto_inject: true,
        root: None,
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
        // By the place it names, so `Documents/x` removes the `C:/.../Documents/x` that add-trigger stored (P3).
        let place = place_in(&toml_path);
        let target = place(p);
        let before = domain.paths.len();
        domain.paths.retain(|pp| pp != p && (target.is_none() || place(pp) != target));
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

// ─── Rename (BO-24) ──────────────────────────────────────────

/// The domain `name` is an old name of, among `domains`: one whose `aliases` hold it. `None` while a domain is still
/// called `name`, so a real name always wins over an alias.
pub fn renamed_from<'a>(domains: &'a [DomainDef], name: &str) -> Option<&'a DomainDef> {
    let want = crate::crud::slugify(name);
    if domains.iter().any(|d| d.name == name || crate::crud::slugify(&d.name) == want) {
        return None;
    }
    domains.iter().find(|d| d.aliases.iter().any(|a| crate::crud::slugify(a) == want))
}

/// The domain to act on for a name the user typed (R4): the name itself, or, when it is an old name kept as an
/// alias, the domain's name now, with one line on stderr saying so (`vintrix is now vintryx`). Reads the
/// domains.toml files `load_domains` reads, never the graph: a file read, not a store load, on every `--domain`.
pub fn canonical_name(cwd: &Path, name: &str) -> String {
    match renamed_from(&load_domains(cwd), name) {
        Some(d) => {
            crate::crud::alias::notice(name, &d.name);
            d.name.clone()
        }
        None => name.to_string(),
    }
}

/// One domains.toml with `old` renamed to `new` (R2).
#[derive(Debug)]
pub struct TomlRename {
    /// The whole file after the rename.
    pub text: String,
    /// The rules the renamed domain declares in this file.
    pub declared_rules: usize,
}

/// `text` (a domains.toml) with the domain `old` renamed to `new` and `old` added to its `aliases` (R2, R4). Edited
/// as text, so every other line, the comments and the order stay byte for byte; a file written by base's own
/// serializer has the `[[domain]]` / `name = "..."` shape this reads. The result is parsed back and compared with
/// the original domain by domain: anything changed besides that name and that alias list is refused, and nothing
/// is returned to write. `Ok(None)` when the file holds no domain called `old`.
pub fn rename_in_text(text: &str, old: &str, new: &str) -> anyhow::Result<Option<TomlRename>> {
    let before: DomainsFile = toml::from_str(text)?;
    // By slug, the key its records carry: `project add -n Vintrix` writes `name = "Vintrix"` for `domain/vintrix`.
    let slug = |d: &DomainDef| crate::crud::slugify(&d.name);
    let hits: Vec<usize> = (0..before.domain.len()).filter(|i| slug(&before.domain[*i]) == old).collect();
    let target = match hits.as_slice() {
        [] => return Ok(None),
        [one] => *one,
        many => anyhow::bail!(
            "{} domains here are '{old}' once slugified ({}); rename them by hand",
            many.len(),
            many.iter().map(|i| before.domain[*i].name.as_str()).collect::<Vec<_>>().join(", ")
        ),
    };
    if before.domain.iter().any(|d| slug(d) == new) {
        anyhow::bail!("a domain is already called '{new}'");
    }
    let mut aliases = before.domain[target].aliases.clone();
    if !aliases.iter().any(|a| crate::crud::slugify(a) == old) {
        aliases.push(old.to_string());
    }
    aliases.retain(|a| crate::crud::slugify(a) != new);

    // The `name` and `aliases` lines of the target's own table: after its `[[domain]]` header, before the next
    // header of any kind (a `[[domain.rules.match]]` sub-table holds keys that are not the domain's).
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let (mut seen, mut in_target) = (0usize, false);
    let (mut name_at, mut aliases_at) = (None, None);
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.starts_with('[') {
            in_target = false;
            let header: String = t.split('#').next().unwrap_or("").chars().filter(|c| !c.is_whitespace()).collect();
            if header == "[[domain]]" {
                in_target = seen == target;
                seen += 1;
            }
            continue;
        }
        if !in_target {
            continue;
        }
        match toml_key(t) {
            Some("name") => name_at = Some(i),
            Some("aliases") => aliases_at = Some(i),
            _ => {}
        }
    }
    let unread = || anyhow::anyhow!("could not find domain '{old}' as a `[[domain]]` table with a `name = \"{old}\"` line; rename it by hand");
    let name_at = name_at.ok_or_else(unread)?;
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let list = toml::Value::Array(aliases.iter().map(|a| toml::Value::String(a.clone())).collect()).to_string();

    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    out[name_at] = with_value(lines[name_at], &toml::Value::String(new.to_string()).to_string()).ok_or_else(unread)?;
    match aliases_at {
        Some(i) => {
            out[i] = with_value(lines[i], &list)
                .ok_or_else(|| anyhow::anyhow!("domain '{old}' has an `aliases` list over more than one line; rename it by hand"))?;
        }
        None => {
            let line = lines[name_at];
            let indent = &line[..line.len() - line.trim_start().len()];
            let ending = if line.ends_with('\n') { "" } else { nl };
            out.insert(name_at + 1, format!("{ending}{indent}aliases = {list}{}", if ending.is_empty() { nl } else { "" }));
        }
    }
    let text_after = out.concat();

    // The check that makes "byte for byte elsewhere" a refusal rather than a hope.
    let after: DomainsFile = toml::from_str(&text_after)?;
    let mut want = before.domain.clone();
    want[target].name = new.to_string();
    want[target].aliases = aliases;
    let as_json = |d: &[DomainDef]| serde_json::to_value(d).unwrap_or_default();
    if as_json(&want) != as_json(&after.domain) {
        anyhow::bail!("renaming '{old}' in the text would change more than its name and aliases; rename it by hand");
    }
    Ok(Some(TomlRename { text: text_after, declared_rules: before.domain[target].rules.len() }))
}

/// The key of a `key = value` line, unquoted; `None` for a comment, a blank line or an array element.
fn toml_key(line: &str) -> Option<&str> {
    if line.starts_with('#') {
        return None;
    }
    let (key, _) = line.split_once('=')?;
    let key = key.trim().trim_matches(|c| c == '"' || c == '\'');
    (!key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')).then_some(key)
}

/// `line` (`key = value  # comment`) with its value replaced by `value`, keeping the key, the spacing, the comment
/// and the line ending. `None` when the value is not a one-line string or array this can bound.
fn with_value(line: &str, value: &str) -> Option<String> {
    let eq = line.find('=')?;
    let rest = &line[eq + 1..];
    let lead = rest.len() - rest.trim_start().len();
    let v = &rest[lead..];
    let end = match v.chars().next()? {
        q @ ('"' | '\'') => {
            let close = v[1..].find(q)? + 1;
            if v[1..close].contains('\\') {
                return None;
            }
            close + 1
        }
        '[' => v.find(']')? + 1,
        _ => return None,
    };
    Some(format!("{}{}{}{}", &line[..eq + 1], &rest[..lead], value, &v[end..]))
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
    // R4: an old name shows the domain it is now, and says so.
    let found = domains.iter().find(|d| d.name == name).or_else(|| {
        renamed_from(&domains, name).inspect(|d| crate::crud::alias::notice(name, &d.name))
    });
    match found {
        Some(d) => {
            println!("Domain: {}", d.name);
            if !d.aliases.is_empty() {
                println!("Aliases: {}", d.aliases.join(", "));
            }
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
            // Both tiers, for the same reason `domain list` counts both: the
            // hook injects both, so a detail view built from one tier described
            // a different domain than the one the agent actually receives.
            // `declared` comes off the DomainDef, which load_domains has ALREADY
            // merged across tiers, so it is counted once; only the graph-backed
            // rules are per tier.
            let (declared, added_ws) = rules_of(cwd, ns, d);
            let gbl = crate::home::home_root().map(|h| h.join(".base-gbl"));
            let added_gbl = match &gbl {
                Some(g) if g.as_path() != cwd => rules_of(g, ns, d).1,
                _ => Vec::new(),
            };
            println!(
                "Rules ({}):",
                declared.len() + added_ws.len() + added_gbl.len()
            );
            for (i, rule) in declared.iter().enumerate() {
                println!("  {i}. {rule}");
            }
            // Numbered by their own index, which is what `base rule remove
            // --index` takes; the two stores number independently, and so do the
            // two tiers -- which is why every cli row names the tier it is in.
            for (n, text) in &added_ws {
                println!("  [workspace cli-{n}] {text}");
            }
            for (n, text) in &added_gbl {
                println!("  [global cli-{n}] {text}");
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

    // ─── BO-24: rename_in_text ───────────────────────────────

    #[test]
    fn rename_in_text_is_none_for_a_file_without_the_domain() {
        assert!(rename_in_text("[[domain]]\nname = \"a\"\n", "b", "c").unwrap().is_none());
    }

    #[test]
    fn rename_in_text_back_to_an_old_name_swaps_the_alias() {
        let text = "[[domain]]\nname = \"b\"\naliases = [\"a\"]  # kept\nmode = \"triggered\"\n";
        let r = rename_in_text(text, "b", "a").unwrap().unwrap();
        assert_eq!(r.text, "[[domain]]\nname = \"a\"\naliases = [\"b\"]  # kept\nmode = \"triggered\"\n");
    }

    #[test]
    fn rename_in_text_handles_a_last_line_with_no_newline() {
        let r = rename_in_text("[[domain]]\nname = \"a\"", "a", "b").unwrap().unwrap();
        assert_eq!(r.text, "[[domain]]\nname = \"b\"\naliases = [\"a\"]");
    }

    #[test]
    fn rename_in_text_refuses_what_it_cannot_bound() {
        let multi = "[[domain]]\nname = \"a\"\naliases = [\n  \"z\",\n]\n";
        let err = rename_in_text(multi, "a", "b").unwrap_err().to_string();
        assert!(err.contains("more than one line"), "{err}");
        let taken = "[[domain]]\nname = \"a\"\n\n[[domain]]\nname = \"b\"\n";
        assert!(rename_in_text(taken, "a", "b").unwrap_err().to_string().contains("already called 'b'"));
    }

    #[test]
    fn rename_in_text_reads_only_the_domain_table_not_its_sub_tables() {
        let text = "[[domain]]\nname = \"a\"\n\n[[domain.rules]]\ntext = \"r\"\n\n[[domain]]\nname = \"c\"\n";
        let r = rename_in_text(text, "a", "b").unwrap().unwrap();
        assert_eq!(r.text, "[[domain]]\nname = \"b\"\naliases = [\"a\"]\n\n[[domain.rules]]\ntext = \"r\"\n\n[[domain]]\nname = \"c\"\n");
        assert_eq!(r.declared_rules, 1);
    }

    #[test]
    fn rule_entry_from_str_yields_bare() {
        let r: RuleEntry = "hello".into();
        assert_eq!(r, RuleEntry::Bare("hello".into()));
        assert_eq!(r.render(), "hello");
    }
}
