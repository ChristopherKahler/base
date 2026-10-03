use std::path::Path;

use anyhow::Result;
use oxigraph::sparql::QueryResults;

use crate::config::NamespaceConfig;
use crate::crud;

/// Thin delegate to [`log_with`] — see the note on `note::learn`.
pub fn log(
    cwd: &Path,
    ns: &NamespaceConfig,
    domain: &str,
    decision_text: &str,
    rationale: &str,
    recall: Option<&str>,
) -> Result<String> {
    log_with(cwd, ns, domain, decision_text, rationale, recall, None)
}

/// [`log`], plus the decision this one supersedes — both in one update.
pub fn log_with(
    cwd: &Path,
    ns: &NamespaceConfig,
    domain: &str,
    decision_text: &str,
    rationale: &str,
    recall: Option<&str>,
    supersedes: Option<&str>,
) -> Result<String> {
    let slug = format!("{}.{}", crud::slugify(domain), crud::slugify(decision_text));
    let iri = crud::build_iri(ns, "decision", &slug);
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);
    let now = crud::now_iso();
    let p = &ns.prefix;

    let decision_text = crud::escape_sparql_literal(decision_text);
    let rationale = crud::escape_sparql_literal(rationale);

    let recall_triple = recall
        .map(|r| {
            let r = crud::escape_sparql_literal(r);
            format!("      {p}:recall \"{r}\" ;\n")
        })
        .unwrap_or_default();

    let domain_slug = crud::slugify(domain);
    let domain_iri = crud::build_iri(ns, "domain", &domain_slug);

    let sparql = format!(
        "INSERT DATA {{\n\
           GRAPH <{graph}> {{\n\
             <{iri}> rdf:type {p}:Decision ;\n\
               {p}:name \"{decision_text}\" ;\n\
               {p}:rationale \"{rationale}\" ;\n\
         {recall_triple}\
               {p}:status \"active\" ;\n\
               {p}:createdAt \"{now}\"^^xsd:dateTime ;\n\
               {p}:lastActive \"{now}\"^^xsd:dateTime .\n\
             <{domain_iri}> {p}:hasDecision <{iri}> .\n\
           }}\n\
         }}"
    );

    // Named for the refusal: `--supersedes` resolves in the store THIS write
    // loads and in no other, so the message has to say which one it searched.
    let tier = crud::tier_label(cwd);
    crud::load_read_then_mutate(cwd, ns, |store| {
        let clause = crud::supersedes_clause(store, ns, &graph, &iri, supersedes, &tier)?;
        Ok(format!("{sparql}{clause}"))
    })?;
    Ok(slug)
}

/// One decision, all fields the graph holds. Stable `--json` contract for the dashboard.
/// `id` is the stable selector `{domain}.{decision}` that `update`/`delete` address.
#[derive(Debug, serde::Serialize)]
pub struct DecisionRecord {
    pub id: String,
    pub name: String,
    pub rationale: Option<String>,
    pub recall: Option<String>,
    pub status: Option<String>,
    pub domain: Option<String>,
    pub created: Option<String>,
    pub last_active: Option<String>,
    /// Its keywords, sorted (BO-03, F5). Empty when it has none.
    pub keywords: Vec<String>,
}

/// Query decision records (typed) matching a keyword across name/rationale/recall.
/// Shared core behind human `search` and `--json` `search_json`.
pub fn search_data(cwd: &Path, ns: &NamespaceConfig, keyword: &str) -> Result<Vec<DecisionRecord>> {
    let p = &ns.prefix;
    let kw_lower = crud::escape_sparql_literal(&keyword.to_lowercase());
    let kw_pred = crate::domain::global_decisions::PRED_KEYWORD;
    let sparql = format!(
        "SELECT ?d ?name ?rationale ?recall ?status ?created ?lastActive ?domain ?kw WHERE {{\n\
           GRAPH ?g {{\n\
             ?d a {p}:Decision ;\n\
               {p}:name ?name ;\n\
               {p}:rationale ?rationale .\n\
             OPTIONAL {{ ?d {p}:recall ?recall }}\n\
             OPTIONAL {{ ?d {p}:status ?status }}\n\
             OPTIONAL {{ ?d {p}:createdAt ?created }}\n\
             OPTIONAL {{ ?d {p}:lastActive ?lastActive }}\n\
             OPTIONAL {{ ?domain {p}:hasDecision ?d }}\n\
             FILTER(\n\
               CONTAINS(LCASE(STR(?name)), \"{kw_lower}\") ||\n\
               CONTAINS(LCASE(STR(?rationale)), \"{kw_lower}\") ||\n\
               CONTAINS(LCASE(STR(?recall)), \"{kw_lower}\")\n\
             )\n\
           }}\n\
           OPTIONAL {{ GRAPH ?kg {{ ?d {p}:{kw_pred} ?kw }} }}\n\
         }}"
    );

    let results = crud::load_and_query(cwd, ns, &sparql)?;
    let mut out: Vec<DecisionRecord> = Vec::new();
    // One row per keyword, so a decision's later rows add their keyword to the record its first row made.
    let mut at: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    if let QueryResults::Solutions(solutions) = results {
        for row in solutions.filter_map(|r| r.ok()) {
            let lit = |k: &str| row.get(k).map(|t| crud::term_display(t.into()));
            let iri = |k: &str| row.get(k).map(|t| crud::slug_of(&crud::term_display(t.into())));
            let Some(id) = iri("d") else { continue };
            let i = *at.entry(id.clone()).or_insert_with(|| {
                out.push(DecisionRecord {
                    id,
                    name: lit("name").unwrap_or_default(),
                    rationale: lit("rationale"),
                    recall: lit("recall"),
                    status: lit("status"),
                    domain: iri("domain"),
                    created: lit("created"),
                    last_active: lit("lastActive"),
                    keywords: Vec::new(),
                });
                out.len() - 1
            });
            if let Some(kw) = lit("kw")
                && !out[i].keywords.contains(&kw)
            {
                out[i].keywords.push(kw);
            }
        }
    }
    for d in &mut out {
        d.keywords.sort();
    }
    Ok(out)
}

pub fn search(cwd: &Path, ns: &NamespaceConfig, keyword: &str) -> Result<()> {
    let rows = search_data(cwd, ns, keyword)?;
    if rows.is_empty() {
        println!("No decisions matching '{keyword}'.");
        return Ok(());
    }
    println!("| decision | rationale | recall |");
    println!("|----------|-----------|--------|");
    for d in &rows {
        println!(
            "| {} | {} | {} |",
            d.name,
            d.rationale.as_deref().unwrap_or("-"),
            d.recall.as_deref().unwrap_or("-"),
        );
    }
    Ok(())
}

/// One decision by its `{domain}.{decision}` slug, all fields; `None` when there is no such decision (BO-24: the
/// reader `decision show` prints, so an old slug can be shown resolving to the renamed record).
pub fn get_data(cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<Option<DecisionRecord>> {
    let p = &ns.prefix;
    let iri = crud::build_iri(ns, "decision", slug);
    let kw_pred = crate::domain::global_decisions::PRED_KEYWORD;
    let sparql = format!(
        "SELECT ?name ?rationale ?recall ?status ?created ?lastActive ?domain ?kw WHERE {{\n\
           GRAPH ?g {{\n\
             <{iri}> a {p}:Decision ;\n\
               {p}:name ?name .\n\
             OPTIONAL {{ <{iri}> {p}:rationale ?rationale }}\n\
             OPTIONAL {{ <{iri}> {p}:recall ?recall }}\n\
             OPTIONAL {{ <{iri}> {p}:status ?status }}\n\
             OPTIONAL {{ <{iri}> {p}:createdAt ?created }}\n\
             OPTIONAL {{ <{iri}> {p}:lastActive ?lastActive }}\n\
             OPTIONAL {{ ?domain {p}:hasDecision <{iri}> }}\n\
           }}\n\
           OPTIONAL {{ GRAPH ?kg {{ <{iri}> {p}:{kw_pred} ?kw }} }}\n\
         }}"
    );
    let QueryResults::Solutions(rows) = crud::load_and_query(cwd, ns, &sparql)? else {
        return Ok(None);
    };
    let mut rec: Option<DecisionRecord> = None;
    for row in rows.filter_map(|r| r.ok()) {
        let lit = |k: &str| row.get(k).map(|t| crud::term_display(t.into()));
        let r = rec.get_or_insert_with(|| DecisionRecord {
            id: slug.to_string(),
            name: lit("name").unwrap_or_default(),
            rationale: lit("rationale"),
            recall: lit("recall"),
            status: lit("status"),
            domain: row.get("domain").map(|t| crud::slug_of(&crud::term_display(t.into()))),
            created: lit("created"),
            last_active: lit("lastActive"),
            keywords: Vec::new(),
        });
        if let Some(kw) = lit("kw")
            && !r.keywords.contains(&kw)
        {
            r.keywords.push(kw);
        }
    }
    if let Some(r) = &mut rec {
        r.keywords.sort();
    }
    Ok(rec)
}

/// `base decision show <slug>`: one decision's fields, or its record as JSON.
pub fn show(cwd: &Path, ns: &NamespaceConfig, slug: &str, json: bool) -> Result<()> {
    let rec = get_data(cwd, ns, slug)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&rec)?);
        return Ok(());
    }
    let Some(d) = rec else {
        anyhow::bail!("no decision '{slug}'");
    };
    println!("Decision: {}", d.id);
    println!("  name: {}", d.name);
    if let Some(v) = &d.domain { println!("  domain: {v}"); }
    if let Some(v) = &d.rationale { println!("  rationale: {v}"); }
    if let Some(v) = &d.recall { println!("  recall: {v}"); }
    if let Some(v) = &d.status { println!("  status: {v}"); }
    if !d.keywords.is_empty() { println!("  keywords: {}", d.keywords.join(", ")); }
    if let Some(v) = &d.created { println!("  created: {v}"); }
    if let Some(v) = &d.last_active { println!("  lastActive: {v}"); }
    Ok(())
}

/// `--json` search: valid JSON array on stdout, nothing else.
pub fn search_json(cwd: &Path, ns: &NamespaceConfig, keyword: &str) -> Result<()> {
    let rows = search_data(cwd, ns, keyword)?;
    println!("{}", serde_json::to_string_pretty(&rows)?);
    Ok(())
}

/// Update a decision in place. Decisions carry a STABLE selector — the slug
/// `{domain}.{decision}` minted at `log` time and never mutated — so they are NOT
/// append-only and support update the same way milestones/projects do. Mutates only
/// the provided fields, through the shared atomic `field_update`. Changing `name`
/// updates the display text without moving the node (the slug/IRI is the identity).
pub fn update(
    cwd: &Path,
    ns: &NamespaceConfig,
    slug: &str,
    name: Option<&str>,
    rationale: Option<&str>,
    recall: Option<&str>,
    status: Option<&str>,
) -> Result<()> {
    update_with(cwd, ns, slug, name, rationale, recall, status, None)
}

/// [`update`], plus the decision's keywords (BO-03, F5): `Some(list)` REPLACES every keyword the decision has
/// with `list`, and `Some(&[])` clears them; `None` leaves them as they are. One write for every field.
///
/// A decision of an always-on domain is served on a prompt only when the prompt contains one of its keywords,
/// and with none it is served at session start only (`domain::global_decisions`).
#[allow(clippy::too_many_arguments)]
pub fn update_with(
    cwd: &Path,
    ns: &NamespaceConfig,
    slug: &str,
    name: Option<&str>,
    rationale: Option<&str>,
    recall: Option<&str>,
    status: Option<&str>,
    keywords: Option<&[String]>,
) -> Result<()> {
    let iri = crud::build_iri(ns, "decision", slug);
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);
    let now = crud::now_iso();
    let p = &ns.prefix;

    let mut updates: Vec<String> = Vec::new();
    let mut field = |pred: &str, val: &str| {
        updates.push(crud::field_update(
            &graph,
            &iri,
            &format!("{p}:{pred}"),
            &format!("\"{}\"", crud::escape_sparql_literal(val)),
        ));
    };
    if let Some(v) = name { field("name", v); }
    if let Some(v) = rationale { field("rationale", v); }
    if let Some(v) = recall { field("recall", v); }
    if let Some(v) = status { field("status", v); }

    if let Some(list) = keywords {
        // The old keywords go from every graph, as `field_update` removes a single value wherever it is stamped,
        // and the new ones go into this tier's graph. Both are conditioned on the decision's type triple being in
        // that graph, as `field_update` is, and the write below refuses outright when it is not: an unconditional
        // delete with a conditional insert would leave the decision with no keywords and print that it has some.
        let pred = format!("{p}:{}", crate::domain::global_decisions::PRED_KEYWORD);
        updates.push(format!(
            "DELETE {{ GRAPH ?gg {{ <{iri}> {pred} ?old }} }}\n\
             WHERE {{ GRAPH <{graph}> {{ <{iri}> a ?type }} GRAPH ?gg {{ <{iri}> {pred} ?old }} }}"
        ));
        if !list.is_empty() {
            let values = list
                .iter()
                .map(|k| format!("\"{}\"", crud::escape_sparql_literal(k)))
                .collect::<Vec<_>>()
                .join(", ");
            updates.push(format!(
                "INSERT {{ GRAPH <{graph}> {{ <{iri}> {pred} {values} }} }}\n\
                 WHERE {{ GRAPH <{graph}> {{ <{iri}> a ?type }} }}"
            ));
        }
    }

    updates.push(crud::field_update(&graph, &iri, &format!("{p}:updatedAt"), &format!("\"{now}\"^^xsd:dateTime")));
    updates.push(crud::field_update(&graph, &iri, &format!("{p}:lastActive"), &format!("\"{now}\"^^xsd:dateTime")));

    let sparql = updates.join(" ;\n");
    if keywords.is_none() {
        return crud::load_and_mutate(cwd, ns, &sparql);
    }
    crud::load_read_then_mutate(cwd, ns, |store| {
        let ask = format!("{}\nASK {{ GRAPH <{graph}> {{ <{iri}> a ?type }} }}", crud::prefixes(ns));
        match crate::store::query(store, &ask) {
            Ok(QueryResults::Boolean(true)) => Ok(sparql),
            _ => anyhow::bail!(
                "decision '{slug}' is not in this tier's graph <{graph}>, so nothing was changed. Run the command from \
                 the workspace that holds it, or `base decision -g update ...` for the global tier."
            ),
        }
    })
}

pub fn delete(cwd: &Path, ns: &NamespaceConfig, keyword: &str) -> Result<usize> {
    let p = &ns.prefix;
    let kw_lower = crud::escape_sparql_literal(&keyword.to_lowercase());

    // Find matching decisions first
    let find_sparql = format!(
        "SELECT ?d ?name WHERE {{\n\
           GRAPH ?g {{\n\
             ?d a {p}:Decision ;\n\
               {p}:name ?name .\n\
             FILTER(CONTAINS(LCASE(STR(?name)), \"{kw_lower}\"))\n\
           }}\n\
         }}"
    );

    let results = crud::load_and_query(cwd, ns, &find_sparql)?;
    let mut iris: Vec<String> = Vec::new();
    if let QueryResults::Solutions(solutions) = results {
        for row in solutions.filter_map(|r| r.ok()) {
            if let Some(d) = row.get("d") {
                iris.push(d.to_string());
            }
        }
    }

    if iris.is_empty() {
        return Ok(0);
    }

    // Delete all triples where the decision is subject or object
    for iri in &iris {
        let iri_clean = iri.trim_matches(|c| c == '<' || c == '>');
        let delete_sparql = format!(
            "DELETE WHERE {{ GRAPH ?g {{ <{iri_clean}> ?p ?o }} }};\n\
             DELETE WHERE {{ GRAPH ?g {{ ?s ?p <{iri_clean}> }} }}"
        );
        crud::load_and_mutate(cwd, ns, &delete_sparql)?;
    }

    Ok(iris.len())
}
