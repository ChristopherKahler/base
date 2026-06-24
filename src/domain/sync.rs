use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::config::{BaseConfig, NamespaceConfig};
use crate::crud;
use crate::domain;

// ─── Sync stats ─────────────────────────────────────────────

pub struct SyncStats {
    pub domains: usize,
    pub rules: usize,
    pub decisions: usize,
}

// ─── Domain → graph sync ────────────────────────────────────

/// Sync domains.toml domains/rules into the graph as ops:Domain and ops:Rule entities.
/// Optionally migrates decisions from a carl.json file.
/// Loads all domains (global + workspace) via `load_domains`.
pub fn sync_domains_to_graph(
    config: &BaseConfig,
    cwd: &Path,
    carl_json_path: Option<&Path>,
) -> Result<SyncStats> {
    let domains = domain::load_domains(cwd);
    sync_domain_list(&config.namespace, cwd, &domains, carl_json_path)
}

/// Sync a specific list of domains into the graph.
/// Core implementation — separated for test isolation (avoids global config leakage).
fn sync_domain_list(
    ns: &NamespaceConfig,
    cwd: &Path,
    domains: &[domain::DomainDef],
    carl_json_path: Option<&Path>,
) -> Result<SyncStats> {
    let (store, trig_path) = crud::load_workspace_store(cwd)?;
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);
    let pfx = crud::prefixes(ns);
    let p = &ns.prefix;

    let mut total_rules = 0usize;

    for domain_def in domains {
        let domain_slug = crud::slugify(&domain_def.name);
        let domain_iri = crud::build_iri(ns, "domain", &domain_slug);

        // UPSERT domain metadata — never delete. Graph is additive.
        // Sync only ensures the domain entity exists with current trigger config.
        // Rules, decisions, notes are graph-native and never touched by sync.

        // Insert domain entity (upsert via INSERT DATA — duplicates are idempotent in RDF)
        let prompt_kw_triples: String = domain_def
            .prompt_keywords
            .iter()
            .map(|kw| format!("      {p}:promptKeyword \"{}\" ;\n", crud::escape_sparql_literal(kw)))
            .collect();

        let file_kw_triples: String = domain_def
            .file_keywords
            .iter()
            .map(|kw| format!("      {p}:fileKeyword \"{}\" ;\n", crud::escape_sparql_literal(kw)))
            .collect();

        let path_triples: String = domain_def
            .paths
            .iter()
            .map(|path| format!("      {p}:triggerPath \"{}\" ;\n", crud::escape_sparql_literal(path)))
            .collect();

        let now = crud::now_iso();
        let domain_insert = format!(
            "{pfx}\n\
             INSERT DATA {{\n\
               GRAPH <{graph}> {{\n\
                 <{domain_iri}> rdf:type {p}:Domain ;\n\
                   {p}:name \"{}\" ;\n\
                   {p}:status \"{}\" ;\n\
             {prompt_kw_triples}\
             {file_kw_triples}\
             {path_triples}\
                   {p}:updatedAt \"{now}\"^^xsd:dateTime .\n\
               }}\n\
             }}",
            crud::escape_sparql_literal(&domain_def.name),
            crud::escape_sparql_literal(&domain_def.mode),
        );
        store
            .update(&domain_insert)
            .with_context(|| format!("Failed to insert domain '{}'", domain_def.name))?;

        // Determine source marker: extension domains use "ext:{name}", others use "domains.toml"
        let source_marker = if domain_def.name.starts_with("ext:") {
            // "ext:outpost:content" → source = "ext:outpost"
            domain_def
                .name
                .splitn(3, ':')
                .take(2)
                .collect::<Vec<_>>()
                .join(":")
        } else {
            "domains.toml".to_string()
        };

        // Garbage-collect this domain's previously SYNCED rules before re-inserting.
        // Scoped by {p}:source marker — rules added via `base rule add`
        // carry no source marker and MUST survive sync (I9).
        let rule_gc = format!(
            "{pfx}\n\
             DELETE {{\n\
               GRAPH <{graph}> {{\n\
                 ?rule ?rp ?ro .\n\
                 <{domain_iri}> {p}:hasRule ?rule .\n\
               }}\n\
             }}\n\
             WHERE {{\n\
               GRAPH <{graph}> {{\n\
                 <{domain_iri}> {p}:hasRule ?rule .\n\
                 ?rule {p}:source \"{}\" ;\n\
                   ?rp ?ro .\n\
               }}\n\
             }}",
            crud::escape_sparql_literal(&source_marker),
        );
        store
            .update(&rule_gc)
            .with_context(|| format!("Failed to GC synced rules for domain '{}'", domain_def.name))?;

        // Insert rules (marked with source so the GC above scopes to them next sync).
        // Rationale (Phase 26) persists as an optional ops:rationale triple — the
        // graph is the live render path for rules (query_domain_from_graph).
        for (i, rule) in domain_def.rules.iter().enumerate() {
            let rule_iri = crud::build_iri(ns, "rule", &format!("{domain_slug}/{i}"));
            let rationale_triple = match rule.rationale() {
                Some(r) => format!(
                    "                       {p}:rationale \"{}\" ;\n",
                    crud::escape_sparql_literal(r)
                ),
                None => String::new(),
            };
            let rule_insert = format!(
                "{pfx}\n\
                 INSERT DATA {{\n\
                   GRAPH <{graph}> {{\n\
                     <{rule_iri}> rdf:type {p}:Rule ;\n\
                       {p}:ruleText \"{}\" ;\n\
                       {p}:priority \"{i}\" ;\n\
                 {rationale_triple}\
                       {p}:source \"{}\" .\n\
                     <{domain_iri}> {p}:hasRule <{rule_iri}> .\n\
                   }}\n\
                 }}",
                crud::escape_sparql_literal(rule.text()),
                crud::escape_sparql_literal(&source_marker),
            );
            store
                .update(&rule_insert)
                .with_context(|| format!("Failed to insert rule {i} for domain '{}'", domain_def.name))?;
            total_rules += 1;
        }
    }

    // Migrate rules + decisions from carl.json if provided
    let (carl_rules, total_decisions) = if let Some(carl_path) = carl_json_path {
        sync_carl_decisions(&store, ns, &graph, &pfx, carl_path)?
    } else {
        (0, 0)
    };
    total_rules += carl_rules;

    crate::store::write_back(&store, &trig_path)?;

    Ok(SyncStats {
        domains: domains.len(),
        rules: total_rules,
        decisions: total_decisions,
    })
}

// ─── Carl.json migration (decisions + rules) ────────────────

/// carl.json structure. The REAL CARL format stores `domains` as a map of
/// name → entry; an older fixture format used an array of named entries.
/// Both are accepted (untagged).
#[derive(Deserialize)]
struct CarlJson {
    #[serde(default)]
    domains: CarlDomains,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CarlDomains {
    Map(std::collections::HashMap<String, CarlDomainEntry>),
    List(Vec<CarlDomainNamed>),
}

impl Default for CarlDomains {
    fn default() -> Self {
        CarlDomains::List(Vec::new())
    }
}

impl CarlDomains {
    fn into_pairs(self) -> Vec<(String, CarlDomainEntry)> {
        match self {
            CarlDomains::Map(m) => m.into_iter().collect(),
            CarlDomains::List(v) => v
                .into_iter()
                .map(|d| (d.name, CarlDomainEntry { rules: d.rules, decisions: d.decisions }))
                .collect(),
        }
    }
}

#[derive(Deserialize)]
struct CarlDomainEntry {
    #[serde(default)]
    rules: Vec<CarlRule>,
    #[serde(default)]
    decisions: Vec<CarlDecision>,
}

#[derive(Deserialize)]
struct CarlDomainNamed {
    name: String,
    #[serde(default)]
    rules: Vec<CarlRule>,
    #[serde(default)]
    decisions: Vec<CarlDecision>,
}

#[derive(Deserialize)]
struct CarlRule {
    text: String,
}

#[derive(Deserialize)]
struct CarlDecision {
    decision: String,
    #[serde(default)]
    rationale: String,
    #[serde(default)]
    recall: RecallField,
    #[serde(default = "default_decision_status")]
    status: String,
}

/// `recall` is a comma-joined string in old fixtures, an array of keywords
/// in the real CARL format. Accept both.
#[derive(Deserialize)]
#[serde(untagged)]
enum RecallField {
    Text(String),
    List(Vec<String>),
}

impl Default for RecallField {
    fn default() -> Self {
        RecallField::Text(String::new())
    }
}

impl RecallField {
    fn joined(&self) -> String {
        match self {
            RecallField::Text(s) => s.clone(),
            RecallField::List(v) => v.join(", "),
        }
    }
}

fn default_decision_status() -> String {
    "active".into()
}

fn sync_carl_decisions(
    store: &oxigraph::store::Store,
    ns: &NamespaceConfig,
    graph: &str,
    pfx: &str,
    carl_path: &Path,
) -> Result<(usize, usize)> {
    let content = std::fs::read_to_string(carl_path)
        .with_context(|| format!("Failed to read carl.json at {}", carl_path.display()))?;
    let carl: CarlJson = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse carl.json at {}", carl_path.display()))?;

    let p = &ns.prefix;
    let now = crud::now_iso();
    let mut rule_count = 0usize;
    let mut decision_count = 0usize;

    for (domain_name, domain) in carl.domains.into_pairs() {
        let domain_slug = crud::slugify(&domain_name);
        let domain_iri = crud::build_iri(ns, "domain", &domain_slug);

        // ── Rules: import with the same semantics as `base rule add` —
        // IRI rule/{slug}/cli-{n}, {p}:index + {p}:priority, NO source
        // marker (domains.toml GC never touches them, rule remove works).
        // Idempotent: skip rules whose exact text already exists for the domain.
        let mut next_idx = carl_next_rule_index(store, pfx, p, &domain_iri);
        for rule in &domain.rules {
            let escaped_text = crud::escape_sparql_literal(&rule.text);
            let exists = format!(
                "{pfx}\n\
                 ASK {{ GRAPH <{graph}> {{ <{domain_iri}> {p}:hasRule ?r . ?r {p}:ruleText \"{escaped_text}\" }} }}"
            );
            if let Ok(oxigraph::sparql::QueryResults::Boolean(true)) = store.query(&exists) {
                continue;
            }

            let rule_iri = crud::build_iri(ns, "rule", &format!("{domain_slug}/cli-{next_idx}"));
            let insert = format!(
                "{pfx}\n\
                 INSERT DATA {{\n\
                   GRAPH <{graph}> {{\n\
                     <{rule_iri}> rdf:type {p}:Rule ;\n\
                       {p}:ruleText \"{escaped_text}\" ;\n\
                       {p}:index \"{next_idx}\" ;\n\
                       {p}:priority \"{next_idx}\" .\n\
                     <{domain_iri}> {p}:hasRule <{rule_iri}> .\n\
                   }}\n\
                 }}"
            );
            store
                .update(&insert)
                .with_context(|| format!("Failed to import carl rule for domain '{domain_name}'"))?;
            next_idx += 1;
            rule_count += 1;
        }

        for decision in &domain.decisions {
            let dec_slug = format!(
                "{}.{}",
                domain_slug,
                crud::slugify(&decision.decision)
            );
            let dec_iri = crud::build_iri(ns, "decision", &dec_slug);

            // Delete existing (idempotent)
            let delete = format!(
                "{pfx}\n\
                 DELETE {{ GRAPH <{graph}> {{ <{dec_iri}> ?p ?o }} }}\n\
                 WHERE {{ GRAPH <{graph}> {{ <{dec_iri}> ?p ?o }} }}"
            );
            let _ = store.update(&delete);

            let recall = decision.recall.joined();
            let recall_triple = if recall.is_empty() {
                String::new()
            } else {
                format!("      {p}:recall \"{}\" ;\n", crud::escape_sparql_literal(&recall))
            };

            let insert = format!(
                "{pfx}\n\
                 INSERT DATA {{\n\
                   GRAPH <{graph}> {{\n\
                     <{dec_iri}> rdf:type {p}:Decision ;\n\
                       {p}:name \"{}\" ;\n\
                       {p}:rationale \"{}\" ;\n\
                 {recall_triple}\
                       {p}:status \"{}\" ;\n\
                       {p}:createdAt \"{now}\"^^xsd:dateTime .\n\
                     <{domain_iri}> {p}:hasDecision <{dec_iri}> .\n\
                   }}\n\
                 }}",
                crud::escape_sparql_literal(&decision.decision),
                crud::escape_sparql_literal(&decision.rationale),
                crud::escape_sparql_literal(&decision.status),
            );
            store
                .update(&insert)
                .with_context(|| format!("Failed to insert decision for domain '{domain_name}'"))?;
            decision_count += 1;
        }
    }

    Ok((rule_count, decision_count))
}

/// MAX({p}:index)+1 over a domain's rules in the given store.
/// Mirrors crud::rule::next_rule_index but works against an already-open store.
fn carl_next_rule_index(store: &oxigraph::store::Store, pfx: &str, p: &str, domain_iri: &str) -> u32 {
    let sparql = format!(
        "{pfx}\n\
         SELECT (MAX(?idx) AS ?max_idx) WHERE {{\n\
           GRAPH ?g {{\n\
             <{domain_iri}> {p}:hasRule ?rule .\n\
             ?rule {p}:index ?idx .\n\
           }}\n\
         }}"
    );
    if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&sparql)
        && let Some(Ok(row)) = solutions.into_iter().next()
        && let Some(term) = row.get("max_idx")
    {
        let max_str = crud::term_display(term.into());
        if let Ok(max_val) = max_str.parse::<u32>() {
            return max_val + 1;
        }
    }
    0
}

// ─── Helpers ────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BaseConfig;

    /// Parse domains directly from a TOML string — bypasses global config for test isolation.
    fn parse_domains(toml_content: &str) -> Vec<domain::DomainDef> {
        #[derive(Deserialize)]
        struct DomainsFile {
            #[serde(default)]
            domain: Vec<domain::DomainDef>,
        }
        toml::from_str::<DomainsFile>(toml_content)
            .unwrap()
            .domain
    }

    fn standard_domains_toml() -> &'static str {
        r#"
[[domain]]
name = "GLOBAL"
mode = "always"
prompt_keywords = []
rules = ["Rule 0: always be helpful", "Rule 1: never lie"]

[[domain]]
name = "DEVELOPMENT"
mode = "triggered"
prompt_keywords = ["write code", "fix bug"]
file_keywords = ["use crate", "impl"]
paths = ["src/"]
rules = ["Dev rule: test everything"]
"#
    }

    fn setup_workspace(dir: &Path) {
        let base_dir = dir.join(".base");
        std::fs::create_dir_all(&base_dir).unwrap();
        std::fs::write(base_dir.join("domains.toml"), standard_domains_toml()).unwrap();
    }

    #[test]
    fn sync_creates_domain_and_rule_entities() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let config = BaseConfig::load(tmp.path());

        let domains = parse_domains(standard_domains_toml());
        let stats = sync_domain_list(&config.namespace, tmp.path(), &domains, None).unwrap();
        assert_eq!(stats.domains, 2);
        assert_eq!(stats.rules, 3); // 2 GLOBAL + 1 DEVELOPMENT

        // Verify domains exist in graph
        let ns = &config.namespace;
        let p = &ns.prefix;
        let results = crud::load_and_query(
            tmp.path(),
            ns,
            &format!("SELECT ?name WHERE {{ GRAPH ?g {{ ?d a {p}:Domain ; {p}:name ?name }} }}"),
        )
        .unwrap();

        if let oxigraph::sparql::QueryResults::Solutions(solutions) = results {
            let names: Vec<String> = solutions
                .filter_map(|r| r.ok())
                .filter_map(|row| row.get("name").map(|t| crud::term_display(t.into())))
                .collect();
            assert!(names.contains(&"GLOBAL".to_string()));
            assert!(names.contains(&"DEVELOPMENT".to_string()));
        } else {
            panic!("Expected solutions");
        }
    }

    #[test]
    fn sync_creates_rules_with_edges() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let config = BaseConfig::load(tmp.path());

        let domains = parse_domains(standard_domains_toml());
        sync_domain_list(&config.namespace, tmp.path(), &domains, None).unwrap();

        let ns = &config.namespace;
        let p = &ns.prefix;
        let domain_iri = crud::build_iri(ns, "domain", "global");

        let results = crud::load_and_query(
            tmp.path(),
            ns,
            &format!(
                "SELECT ?text WHERE {{ GRAPH ?g {{ <{domain_iri}> {p}:hasRule ?r . ?r {p}:ruleText ?text }} }}"
            ),
        )
        .unwrap();

        if let oxigraph::sparql::QueryResults::Solutions(solutions) = results {
            let texts: Vec<String> = solutions
                .filter_map(|r| r.ok())
                .filter_map(|row| row.get("text").map(|t| crud::term_display(t.into())))
                .collect();
            assert_eq!(texts.len(), 2);
            assert!(texts.iter().any(|t| t.contains("always be helpful")));
            assert!(texts.iter().any(|t| t.contains("never lie")));
        } else {
            panic!("Expected solutions");
        }
    }

    #[test]
    fn sync_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());
        let config = BaseConfig::load(tmp.path());

        let domains = parse_domains(standard_domains_toml());

        // Sync twice
        sync_domain_list(&config.namespace, tmp.path(), &domains, None).unwrap();
        let stats = sync_domain_list(&config.namespace, tmp.path(), &domains, None).unwrap();
        assert_eq!(stats.domains, 2);
        assert_eq!(stats.rules, 3);

        // Count total rules — should be 3, not 6
        let ns = &config.namespace;
        let p = &ns.prefix;
        let results = crud::load_and_query(
            tmp.path(),
            ns,
            &format!("SELECT (COUNT(?r) AS ?cnt) WHERE {{ GRAPH ?g {{ ?r a {p}:Rule }} }}"),
        )
        .unwrap();

        if let oxigraph::sparql::QueryResults::Solutions(solutions) = results {
            let row = solutions.filter_map(|r| r.ok()).next().unwrap();
            let cnt = crud::term_display(row.get("cnt").unwrap().into());
            assert_eq!(cnt, "3", "Should have exactly 3 rules, not duplicates");
        }
    }

    #[test]
    fn sync_carl_decisions_creates_entities() {
        let tmp = tempfile::tempdir().unwrap();
        setup_workspace(tmp.path());

        // Write a minimal carl.json
        let carl_path = tmp.path().join("carl.json");
        std::fs::write(
            &carl_path,
            r#"{
  "domains": [
    {
      "name": "GLOBAL",
      "decisions": [
        {
          "decision": "Use JWT for auth",
          "rationale": "Stateless, scalable",
          "recall": "When discussing auth"
        }
      ]
    }
  ]
}"#,
        )
        .unwrap();

        let config = BaseConfig::load(tmp.path());
        let domains = parse_domains(standard_domains_toml());
        let stats = sync_domain_list(&config.namespace, tmp.path(), &domains, Some(&carl_path)).unwrap();
        assert_eq!(stats.decisions, 1);

        // Verify decision linked to domain
        let ns = &config.namespace;
        let p = &ns.prefix;
        let domain_iri = crud::build_iri(ns, "domain", "global");

        let results = crud::load_and_query(
            tmp.path(),
            ns,
            &format!(
                "SELECT ?name WHERE {{ GRAPH ?g {{ <{domain_iri}> {p}:hasDecision ?d . ?d {p}:name ?name }} }}"
            ),
        )
        .unwrap();

        if let oxigraph::sparql::QueryResults::Solutions(solutions) = results {
            let names: Vec<String> = solutions
                .filter_map(|r| r.ok())
                .filter_map(|row| row.get("name").map(|t| crud::term_display(t.into())))
                .collect();
            assert_eq!(names.len(), 1);
            assert!(names[0].contains("JWT"));
        } else {
            panic!("Expected solutions");
        }
    }

    #[test]
    fn backward_compat_legacy_keywords_field() {
        let tmp = tempfile::tempdir().unwrap();
        let base_dir = tmp.path().join(".base");
        std::fs::create_dir_all(&base_dir).unwrap();
        // Legacy format: `keywords` instead of `prompt_keywords`
        let legacy_toml = r#"
[[domain]]
name = "LEGACY"
mode = "triggered"
keywords = ["old keyword"]
rules = ["Legacy rule"]
"#;
        std::fs::write(base_dir.join("domains.toml"), legacy_toml).unwrap();

        let config = BaseConfig::load(tmp.path());
        let domains = parse_domains(legacy_toml);
        let stats = sync_domain_list(&config.namespace, tmp.path(), &domains, None).unwrap();
        assert_eq!(stats.domains, 1);
        assert_eq!(stats.rules, 1);

        // Verify prompt keyword was synced from legacy field
        let ns = &config.namespace;
        let p = &ns.prefix;
        let domain_iri = crud::build_iri(ns, "domain", "legacy");
        let results = crud::load_and_query(
            tmp.path(),
            ns,
            &format!(
                "SELECT ?kw WHERE {{ GRAPH ?g {{ <{domain_iri}> {p}:promptKeyword ?kw }} }}"
            ),
        )
        .unwrap();

        if let oxigraph::sparql::QueryResults::Solutions(solutions) = results {
            let kws: Vec<String> = solutions
                .filter_map(|r| r.ok())
                .filter_map(|row| row.get("kw").map(|t| crud::term_display(t.into())))
                .collect();
            assert_eq!(kws.len(), 1);
            assert_eq!(kws[0], "old keyword");
        }
    }

    // ─── Phase 26: rationale persists to graph and renders on injection ──
    #[test]
    fn sync_persists_rationale_and_graph_render_includes_because() {
        let tmp = tempfile::tempdir().unwrap();
        let base_dir = tmp.path().join(".base");
        std::fs::create_dir_all(&base_dir).unwrap();
        let toml = r#"
[[domain]]
name = "GUARD"
mode = "triggered"
rules = [{ text = "Never lie", rationale = "trust is load-bearing" }, "Be terse"]
"#;
        std::fs::write(base_dir.join("domains.toml"), toml).unwrap();

        let config = BaseConfig::load(tmp.path());
        let domains = parse_domains(toml);
        sync_domain_list(&config.namespace, tmp.path(), &domains, None).unwrap();

        // ops:rationale lands in the graph for the detailed rule.
        let ns = &config.namespace;
        let p = &ns.prefix;
        let results = crud::load_and_query(
            tmp.path(),
            ns,
            &format!("SELECT ?r WHERE {{ GRAPH ?g {{ ?rule {p}:rationale ?r }} }}"),
        )
        .unwrap();
        let rationales: Vec<String> = match results {
            oxigraph::sparql::QueryResults::Solutions(sols) => sols
                .filter_map(|r| r.ok())
                .filter_map(|row| row.get("r").map(|t| crud::term_display(t.into())))
                .collect(),
            _ => panic!("expected solutions"),
        };
        assert!(
            rationales.iter().any(|r| r.contains("trust is load-bearing")),
            "rationale persisted: {rationales:?}"
        );

        // Isolated workspace graph render shows "Never lie — because ..." and the
        // bare rule unchanged.
        let store = crate::store::load_graph(&base_dir.join("graph.nq")).unwrap();
        let (rules_text, _) =
            crate::domain::query::query_domain_from_graph(&store, &config, &domains[0]);
        assert!(
            rules_text.contains("Never lie — because trust is load-bearing"),
            "rationale rendered: {rules_text}"
        );
        assert!(rules_text.contains("Be terse"), "bare rule intact: {rules_text}");
    }
}
