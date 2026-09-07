use std::path::Path;

use anyhow::Result;
use oxigraph::sparql::QueryResults;

use crate::config::NamespaceConfig;
use crate::crud;

/// Create a note (memory entry) with optional relational edges.
///
/// Thin delegate to [`learn_with`]. The `--supersedes` flag arrived after ~40 call
/// sites already existed; one implementation plus a one-line wrapper keeps the
/// behaviour single while leaving those callers untouched.
pub fn learn(
    cwd: &Path,
    ns: &NamespaceConfig,
    text: &str,
    note_type: &str,
    domain: Option<&str>,
    project: Option<&str>,
    entity: Option<&str>,
) -> Result<String> {
    learn_with(cwd, ns, text, note_type, domain, project, entity, None)
}

/// [`learn`], plus the record this note supersedes.
///
/// The note and its supersession edges are applied in ONE update against ONE load,
/// so a correction and the edge saying what it corrects land together or not at all.
// Eight, one past clippy's default. Seven of them are `learn`'s existing shape and
// the eighth is `--supersedes`; collapsing the one-line delegates into this is the
// agreed follow-up, and reshaping the signature to satisfy a lint is not it.
#[allow(clippy::too_many_arguments)]
pub fn learn_with(
    cwd: &Path,
    ns: &NamespaceConfig,
    text: &str,
    note_type: &str,
    domain: Option<&str>,
    project: Option<&str>,
    entity: Option<&str>,
    supersedes: Option<&str>,
) -> Result<String> {
    let slug = crud::slugify(text);
    let iri = crud::build_iri(ns, "note", &slug);
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);
    let now = crud::now_iso();
    let p = &ns.prefix;

    let escaped_text = crud::escape_sparql_literal(text);

    // Build relational edges. The domain link is written under BOTH predicates
    // for the 0.14.0 overlap: `hasDomain` is what every other record kind already
    // uses (`crud/project.rs`, `crud/entity.rs`, `extract/paul_toml.rs`) and what
    // N+1 keeps; `relatedTo` is what every 0.13.x reader still looks for, and
    // dropping it now would blind a rolled-back binary to notes written meanwhile.
    // See `domain::link` for the window.
    let mut edge_triples = String::new();
    if let Some(d) = domain {
        let domain_iri = crud::build_iri(ns, "domain", &crud::slugify(d));
        let canonical = crate::domain::link::CANONICAL;
        let legacy = crate::domain::link::LEGACY;
        edge_triples.push_str(&format!("      <{iri}> {p}:{canonical} <{domain_iri}> .\n"));
        edge_triples.push_str(&format!("      <{iri}> {p}:{legacy} <{domain_iri}> .\n"));
    }
    if let Some(proj) = project {
        let proj_iri = crud::build_iri(ns, "project", &crud::slugify(proj));
        edge_triples.push_str(&format!("      <{iri}> {p}:relatedTo <{proj_iri}> .\n"));
    }
    if let Some(ent) = entity {
        let ent_iri = crud::build_iri(ns, "entity", &crud::slugify(ent));
        edge_triples.push_str(&format!("      <{iri}> {p}:relatedTo <{ent_iri}> .\n"));
    }

    let sparql = format!(
        "INSERT DATA {{\n\
           GRAPH <{graph}> {{\n\
             <{iri}> rdf:type {p}:Note ;\n\
               {p}:noteText \"{escaped_text}\" ;\n\
               {p}:noteType \"{note_type}\" ;\n\
               {p}:status \"active\" ;\n\
               {p}:createdAt \"{now}\"^^xsd:dateTime .\n\
         {edge_triples}\
           }}\n\
         }}"
    );

    crud::load_read_then_mutate(cwd, ns, |store| {
        let clause = crud::supersedes_clause(store, ns, &graph, &iri, supersedes)?;
        Ok(format!("{sparql}{clause}"))
    })?;
    Ok(slug)
}

/// Like recall() but returns a formatted String instead of printing.
/// Used by hook injection (memory intercept, session-start).
pub fn recall_to_string(
    cwd: &Path,
    ns: &NamespaceConfig,
    keyword: Option<&str>,
    domain: Option<&str>,
) -> String {
    recall_to_string_with(cwd, ns, keyword, domain, false)
}

/// [`recall_to_string`], with the choice of whether superseded records are served.
///
/// `include_superseded` is false everywhere except the CLI flag of the same name.
/// Recall is a SERVING surface: it answers a question, so it answers with the live
/// version. Structure surfaces (`graph analyze/neighbors/path/get-node`, the
/// dashboard, the LOGOS export) drop nothing — the superseded record IS the drift
/// evidence, and deleting it from the view would leave its successor's edge
/// pointing at something the operator cannot see.
pub fn recall_to_string_with(
    cwd: &Path,
    ns: &NamespaceConfig,
    keyword: Option<&str>,
    domain: Option<&str>,
    include_superseded: bool,
) -> String {
    let p = &ns.prefix;
    // Session traffic is excluded from every read surface by one list
    // (`ontology::transient`), never by a per-command filter.
    let no_transient = crate::ontology::transient::sparql_exclude(ns, "n");
    // Superseded records are dropped from every arm for the same reason and in the
    // same place as transients: beside the arm's other filters, INSIDE its `GRAPH ?g`
    // group. A filter outside every GRAPH group is matched against the default graph,
    // where base keeps nothing, so it reads as a working filter and excludes nothing
    // (F16, kite, 2026-09-06). Empty when the operator asked to see the chain.
    let no_superseded = if include_superseded {
        String::new()
    } else {
        crate::supersede::sparql_exclude_superseded(ns, "n")
    };
    // A note's domain link is `hasDomain` from 0.14.0 and `relatedTo` before it.
    // Reading only one of them is how `base recall --domain` goes silently empty —
    // an empty SPARQL result is indistinguishable from "no notes in that domain".
    // `links_to` is an EXISTS test, not a triple pattern: during the overlap a note
    // carries both predicates and a pattern would return it twice.

    // Every branch below ends in a TOTAL order. Without one, oxigraph returns
    // solutions in store order, so `base recall` reshuffles its rows after any
    // rewrite — compact, purge, or this fork's migration — and two runs over the
    // same knowledge disagree. Measured on a copy of Chris's store: the 80 rows of
    // `recall --domain base` were identical as a SET before and after the
    // migration, and in a different order. `graph_query::load_graph` already sorts
    // its adjacency for exactly this reason; these reads never did. DESC(?created)
    // keeps the newest-first reading store order happened to give, and `?text`
    // breaks ties — a note's IRI is slugify(text), so it is unique per note.
    let sparql = match (keyword, domain) {
        (Some(kw), Some(dom)) => {
            let kw_lower = crud::escape_sparql_literal(&kw.to_lowercase());
            let domain_iri = crud::build_iri(ns, "domain", &crud::slugify(dom));
            let dlink = crate::domain::link::links_to(ns, "n", &domain_iri);
            // DISTINCT because the two arms are not disjoint: a note that contains
            // the keyword AND belongs to the domain satisfies both and is returned
            // twice. Pre-existing in 0.13.x — the arms have never been deduped —
            // and it only got easier to hit, so it is fixed here rather than left
            // in a read this fork is already rewriting. It cannot merge two real
            // notes: a note's IRI is `slugify(text)`, so equal text is one record.
            format!(
                "SELECT DISTINCT ?text ?type ?created WHERE {{\n\
                   GRAPH ?g {{\n\
                     {{ ?n a {p}:Note ; {p}:noteText ?text ; {p}:noteType ?type .\n\
                        OPTIONAL {{ ?n {p}:createdAt ?created }}\n\
                        FILTER(CONTAINS(LCASE(STR(?text)), \"{kw_lower}\"))\n\
                     }} UNION {{\n\
                        ?n a {p}:Note ; {p}:noteText ?text ; {p}:noteType ?type .\n\
                        {dlink}\n\
                        OPTIONAL {{ ?n {p}:createdAt ?created }}\n\
                     }}\n\
                     ?n {p}:status \"active\" .\n\
                     {no_transient}{no_superseded}\
                   }}\n\
                 }}\n\
                 ORDER BY DESC(?created) ?text"
            )
        }
        (Some(kw), None) => {
            let kw_lower = crud::escape_sparql_literal(&kw.to_lowercase());
            format!(
                "SELECT ?text ?type ?created ?extra WHERE {{\n\
                   {{\n\
                     GRAPH ?g {{\n\
                       ?n a {p}:Note ; {p}:noteText ?text ; {p}:noteType ?type ; {p}:status \"active\" .\n\
                       OPTIONAL {{ ?n {p}:createdAt ?created }}\n\
                       FILTER(CONTAINS(LCASE(STR(?text)), \"{kw_lower}\"))\n\
                       {no_transient}{no_superseded}\
                     }}\n\
                   }} UNION {{\n\
                     GRAPH ?g {{\n\
                       ?n a {p}:Decision ; {p}:name ?text .\n\
                       BIND(\"decision\" AS ?type)\n\
                       OPTIONAL {{ ?n {p}:rationale ?extra }}\n\
                       OPTIONAL {{ ?n {p}:fromPlan ?created }}\n\
                       FILTER(CONTAINS(LCASE(STR(?text)), \"{kw_lower}\"))\n\
                       {no_transient}{no_superseded}\
                     }}\n\
                   }} UNION {{\n\
                     GRAPH ?g {{\n\
                       ?n a {p}:Decision ; {p}:rationale ?text .\n\
                       BIND(\"decision\" AS ?type)\n\
                       OPTIONAL {{ ?n {p}:name ?extra }}\n\
                       OPTIONAL {{ ?n {p}:fromPlan ?created }}\n\
                       FILTER(CONTAINS(LCASE(STR(?text)), \"{kw_lower}\"))\n\
                       {no_transient}{no_superseded}\
                     }}\n\
                   }} UNION {{\n\
                     GRAPH ?g {{\n\
                       ?n a {p}:FileChange ; {p}:filePath ?text .\n\
                       BIND(\"file-change\" AS ?type)\n\
                       OPTIONAL {{ ?n {p}:purpose ?extra }}\n\
                       OPTIONAL {{ ?n {p}:fromPlan ?created }}\n\
                       FILTER(CONTAINS(LCASE(STR(?text)), \"{kw_lower}\"))\n\
                       {no_transient}{no_superseded}\
                     }}\n\
                   }} UNION {{\n\
                     GRAPH ?g {{\n\
                       ?n a {p}:AcceptanceCriteriaResult ; {p}:criterion ?text .\n\
                       BIND(\"ac-result\" AS ?type)\n\
                       OPTIONAL {{ ?n {p}:status ?extra }}\n\
                       OPTIONAL {{ ?n {p}:fromPlan ?created }}\n\
                       FILTER(CONTAINS(LCASE(STR(?text)), \"{kw_lower}\"))\n\
                       {no_transient}{no_superseded}\
                     }}\n\
                   }}\n\
                 }}\n\
                 ORDER BY DESC(?created) ?text"
            )
        }
        (None, Some(dom)) => {
            let domain_iri = crud::build_iri(ns, "domain", &crud::slugify(dom));
            let dlink = crate::domain::link::links_to(ns, "n", &domain_iri);
            format!(
                "SELECT ?text ?type ?created WHERE {{\n\
                   GRAPH ?g {{\n\
                     ?n a {p}:Note ; {p}:noteText ?text ; {p}:noteType ?type ; {p}:status \"active\" .\n\
                     {dlink}\n\
                     OPTIONAL {{ ?n {p}:createdAt ?created }}\n\
                     {no_transient}{no_superseded}\
                   }}\n\
                 }}\n\
                 ORDER BY DESC(?created) ?text"
            )
        }
        (None, None) => return String::new(),
    };

    let results = match crud::load_and_query(cwd, ns, &sparql) {
        Ok(r) => r,
        Err(_) => return String::new(),
    };

    if let QueryResults::Solutions(solutions) = results {
        let rows: Vec<Vec<String>> = solutions
            .filter_map(|r| r.ok())
            .map(|row| {
                vec![
                    row.get("type")
                        .map(|t| crud::term_display(t.into()))
                        .unwrap_or_default(),
                    row.get("text")
                        .map(|t| crud::term_display(t.into()))
                        .unwrap_or_default(),
                    row.get("extra")
                        .map(|t| crud::term_display(t.into()))
                        .unwrap_or_else(|| "-".into()),
                    row.get("created")
                        .map(|t| crud::term_display(t.into()))
                        .unwrap_or_else(|| "-".into()),
                ]
            })
            .collect();

        if rows.is_empty() {
            return String::new();
        }

        let mut out = String::from("| type | text | context | plan/date |\n");
        out.push_str("|------|------|---------|----------|\n");
        for row in &rows {
            out.push_str(&format!("| {} | {} | {} | {} |\n", row[0], row[1], row[2], row[3]));
        }
        out
    } else {
        String::new()
    }
}

/// Search notes by keyword text match and/or domain linkage.
pub fn recall(
    cwd: &Path,
    ns: &NamespaceConfig,
    keyword: Option<&str>,
    domain: Option<&str>,
) -> Result<()> {
    recall_with(cwd, ns, keyword, domain, false)
}

/// [`recall`], with `--include-superseded`.
pub fn recall_with(
    cwd: &Path,
    ns: &NamespaceConfig,
    keyword: Option<&str>,
    domain: Option<&str>,
    include_superseded: bool,
) -> Result<()> {
    if keyword.is_none() && domain.is_none() {
        eprintln!("Provide --keyword and/or --domain");
        return Ok(());
    }

    let output = recall_to_string_with(cwd, ns, keyword, domain, include_superseded);
    if output.is_empty() {
        println!("No results found.");
    } else {
        print!("{output}");
        println!("\nTip: use `base learn --list` to see slugs for --remove/--update");
    }
    Ok(())
}

/// Resolve the NOTE IRIs an explicit recall surfaces (notes only — not decisions /
/// file-changes / AC-results). Best-effort: returns empty on any query error. Used
/// by the CLI recall handler to stamp `lastRead`; hooks (`recall_to_string`)
/// deliberately do NOT call this, so hot-path injection never triggers a write.
pub fn recalled_note_iris(
    cwd: &Path,
    ns: &NamespaceConfig,
    keyword: Option<&str>,
    domain: Option<&str>,
) -> Vec<String> {
    let p = &ns.prefix;
    let where_clause = match (keyword, domain) {
        (Some(kw), Some(dom)) => {
            let kw_lower = crud::escape_sparql_literal(&kw.to_lowercase());
            let domain_iri = crud::build_iri(ns, "domain", &crud::slugify(dom));
            let dlink = crate::domain::link::links_to(ns, "n", &domain_iri);
            format!(
                "{{ ?n a {p}:Note ; {p}:noteText ?text ; {p}:status \"active\" .\n\
                    FILTER(CONTAINS(LCASE(STR(?text)), \"{kw_lower}\")) }}\n\
                 UNION\n\
                 {{ ?n a {p}:Note ; {p}:status \"active\" . {dlink} }}"
            )
        }
        (Some(kw), None) => {
            let kw_lower = crud::escape_sparql_literal(&kw.to_lowercase());
            format!(
                "?n a {p}:Note ; {p}:noteText ?text ; {p}:status \"active\" .\n\
                 FILTER(CONTAINS(LCASE(STR(?text)), \"{kw_lower}\"))"
            )
        }
        (None, Some(dom)) => {
            let domain_iri = crud::build_iri(ns, "domain", &crud::slugify(dom));
            let dlink = crate::domain::link::links_to(ns, "n", &domain_iri);
            format!("?n a {p}:Note ; {p}:status \"active\" . {dlink}")
        }
        (None, None) => return Vec::new(),
    };

    let no_transient = crate::ontology::transient::sparql_exclude(ns, "n");
    // This decides which notes get their `lastRead` stamped, so it must exclude
    // exactly what `recall_to_string` excluded. A superseded note that recall did
    // not print must not be marked as read: `graph purge --stale` reads that stamp,
    // and a note kept alive by a recall that never showed it is a lie the purge
    // then acts on.
    let no_superseded = crate::supersede::sparql_exclude_superseded(ns, "n");
    let sparql =
        format!("SELECT DISTINCT ?n WHERE {{ GRAPH ?g {{ {where_clause}\n{no_transient}{no_superseded} }} }}");
    let results = match crud::load_and_query(cwd, ns, &sparql) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let mut iris = Vec::new();
    if let QueryResults::Solutions(sols) = results {
        for row in sols.filter_map(|r| r.ok()) {
            if let Some(oxigraph::model::Term::NamedNode(n)) = row.get("n") {
                iris.push(n.as_str().to_string());
            }
        }
    }
    iris
}

/// Stamp `{p}:lastRead = now` on the given note IRIs (workspace graph, STRICT load +
/// `write_back`). This is the usage signal `base graph purge --stale` reads. Called
/// ONLY from the explicit `base recall` CLI path (never hooks — latency budget +
/// Phase 35 writes-stay-strict). Returns the count stamped. Errs if the strict load
/// fails (corrupt graph) so the caller warns + skips — never a silent lenient write.
pub fn stamp_last_read(cwd: &Path, ns: &NamespaceConfig, note_iris: &[String]) -> Result<usize> {
    if note_iris.is_empty() {
        return Ok(0);
    }
    let p = &ns.prefix;
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);
    let now = crud::now_iso();

    let (store, trig_path) = crud::load_workspace_store(cwd)?;

    let mut stmts: Vec<String> = Vec::new();
    for iri in note_iris {
        stmts.push(format!(
            "DELETE {{ GRAPH <{graph}> {{ <{iri}> {p}:lastRead ?o }} }} \
             WHERE {{ GRAPH <{graph}> {{ <{iri}> {p}:lastRead ?o }} }}"
        ));
        stmts.push(format!(
            "INSERT DATA {{ GRAPH <{graph}> {{ <{iri}> {p}:lastRead \"{now}\"^^xsd:dateTime }} }}"
        ));
    }
    let update = format!("{}\n{}", crud::prefixes(ns), stmts.join(";\n"));
    // A per-machine usage signal, not knowledge: this fires on every `base
    // recall`, and shipping it would push a fact op to the team each time.
    crate::store::update_and_write(
        &store,
        &trig_path,
        &update,
        crate::store::Scope::Target,
        crate::store::Intent::Housekeeping,
    )?;
    Ok(note_iris.len())
}

/// Increment mention count on an existing note. Returns the new count.
pub fn mention(
    cwd: &Path,
    ns: &NamespaceConfig,
    slug: &str,
    context: Option<&str>,
) -> Result<u32> {
    let p = &ns.prefix;

    // First, query current mention count
    let iri = crud::build_iri(ns, "note", slug);
    let count_sparql = format!(
        "SELECT ?count WHERE {{\n\
           GRAPH ?g {{\n\
             <{iri}> a {p}:Note .\n\
             OPTIONAL {{ <{iri}> {p}:mentionCount ?count }}\n\
           }}\n\
         }}"
    );

    let results = crud::load_and_query(cwd, ns, &count_sparql)?;
    let current_count = if let QueryResults::Solutions(solutions) = results {
        solutions
            .filter_map(|r| r.ok())
            .next()
            .and_then(|row| {
                row.get("count")
                    .map(|t| crud::term_display(t.into()))
            })
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0)
    } else {
        // Note not found
        anyhow::bail!("Note not found: {slug}");
    };

    let new_count = current_count + 1;
    let now = crud::now_iso();

    // Build update: delete old count/lastMentioned, insert new values
    let mut sparql = format!(
        "DELETE {{\n\
           GRAPH ?g {{\n\
             <{iri}> {p}:mentionCount ?oldCount .\n\
             <{iri}> {p}:lastMentioned ?oldMentioned .\n\
           }}\n\
         }} WHERE {{\n\
           GRAPH ?g {{\n\
             <{iri}> a {p}:Note .\n\
             OPTIONAL {{ <{iri}> {p}:mentionCount ?oldCount }}\n\
             OPTIONAL {{ <{iri}> {p}:lastMentioned ?oldMentioned }}\n\
           }}\n\
         }};\n\
         INSERT DATA {{\n\
           GRAPH <{graph}> {{\n\
             <{iri}> {p}:mentionCount {new_count} .\n\
             <{iri}> {p}:lastMentioned \"{now}\"^^xsd:dateTime .\n\
           }}\n\
         }}",
        graph = {
            let ws_slug = crud::workspace_slug(cwd);
            crud::workspace_graph_iri(ns, &ws_slug)
        },
    );

    // If context provided, append to note text
    if let Some(ctx) = context {
        let escaped = crud::escape_sparql_literal(ctx);
        let append_text = format!("\\n\\n[Mention {new_count}: {escaped}]");
        let ws_slug = crud::workspace_slug(cwd);
        let graph = crud::workspace_graph_iri(ns, &ws_slug);

        sparql.push_str(&format!(
            ";\n\
             DELETE {{\n\
               GRAPH <{graph}> {{\n\
                 <{iri}> {p}:noteText ?oldText .\n\
               }}\n\
             }}\n\
             INSERT {{\n\
               GRAPH <{graph}> {{\n\
                 <{iri}> {p}:noteText ?newText .\n\
               }}\n\
             }}\n\
             WHERE {{\n\
               GRAPH <{graph}> {{\n\
                 <{iri}> {p}:noteText ?oldText .\n\
                 BIND(CONCAT(STR(?oldText), \"{append_text}\") AS ?newText)\n\
               }}\n\
             }}"
        ));
    }

    crud::load_and_mutate(cwd, ns, &sparql)?;
    Ok(new_count)
}

pub fn remove(cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<bool> {
    let p = &ns.prefix;
    let iri = crud::build_iri(ns, "note", slug);
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);

    let check = format!(
        "SELECT ?text WHERE {{ GRAPH <{graph}> {{ <{iri}> {p}:noteText ?text }} }}"
    );

    let (store, trig_path) = crud::load_workspace_store(cwd)?;
    let full_check = format!("{}\n{}", crud::prefixes(ns), check);
    let results = crate::store::query(&store, &full_check)?;
    let exists = if let QueryResults::Solutions(mut sols) = results {
        sols.next().is_some()
    } else {
        false
    };

    if !exists {
        return Ok(false);
    }

    let delete = format!(
        "{}\nDELETE WHERE {{ GRAPH <{graph}> {{ <{iri}> ?p ?o }} }}",
        crud::prefixes(ns)
    );
    // Wide: a retraction that never ships leaves the deleted note alive in the
    // team's graph forever.
    crate::store::update_and_write(
        &store,
        &trig_path,
        &delete,
        crate::store::Scope::Wide,
        crate::store::Intent::Knowledge,
    )?;
    Ok(true)
}

pub fn update_text(cwd: &Path, ns: &NamespaceConfig, slug: &str, new_text: &str) -> Result<bool> {
    let p = &ns.prefix;
    let iri = crud::build_iri(ns, "note", slug);
    let ws_slug = crud::workspace_slug(cwd);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);
    let escaped = crud::escape_sparql_literal(new_text);

    let (store, trig_path) = crud::load_workspace_store(cwd)?;

    let check = format!(
        "{}\nSELECT ?text WHERE {{ GRAPH <{graph}> {{ <{iri}> {p}:noteText ?text }} }}",
        crud::prefixes(ns)
    );
    let results = crate::store::query(&store, &check)?;
    let exists = if let QueryResults::Solutions(mut sols) = results {
        sols.next().is_some()
    } else {
        false
    };

    if !exists {
        return Ok(false);
    }

    let update = format!(
        "{}\nDELETE {{ GRAPH <{graph}> {{ <{iri}> {p}:noteText ?old }} }}\n\
         INSERT {{ GRAPH <{graph}> {{ <{iri}> {p}:noteText \"{escaped}\" }} }}\n\
         WHERE {{ GRAPH <{graph}> {{ <{iri}> {p}:noteText ?old }} }}",
        crud::prefixes(ns)
    );
    crate::store::update_and_write(
        &store,
        &trig_path,
        &update,
        crate::store::Scope::Wide,
        crate::store::Intent::Knowledge,
    )?;
    Ok(true)
}

pub fn list_notes(cwd: &Path, ns: &NamespaceConfig, type_filter: Option<&str>, domain_filter: Option<&str>) -> Result<()> {
    let p = &ns.prefix;

    let mut filters = Vec::new();
    if let Some(t) = type_filter {
        let escaped = crud::escape_sparql_literal(t);
        filters.push(format!("FILTER(?type = \"{escaped}\")"));
    }
    if let Some(d) = domain_filter {
        let domain_iri = crud::build_iri(ns, "domain", &crud::slugify(d));
        filters.push(crate::domain::link::links_to(ns, "n", &domain_iri));
    }
    filters.push(crate::ontology::transient::sparql_exclude(ns, "n"));

    let filter_block = filters.join("\n             ");

    let uri = &ns.uri;
    let sparql = format!(
        "SELECT ?n ?type ?text WHERE {{\n\
           GRAPH ?g {{\n\
             ?n a {p}:Note ; {p}:noteText ?text ; {p}:noteType ?type ; {p}:status \"active\" .\n\
             {filter_block}\n\
           }}\n\
         }}\n\
         ORDER BY ?type"
    );

    let results = crud::load_and_query(cwd, ns, &sparql)?;
    let QueryResults::Solutions(solutions) = results else {
        println!("No notes found.");
        return Ok(());
    };

    let note_prefix = format!("{uri}note/");
    let rows: Vec<(String, String, String)> = solutions
        .filter_map(|r| r.ok())
        .map(|row| {
            let slug = row.get("n").map(|t| match t {
                oxigraph::model::Term::NamedNode(n) => {
                    n.as_str().strip_prefix(&note_prefix).unwrap_or(n.as_str()).to_string()
                }
                other => other.to_string(),
            }).unwrap_or_default();
            let text = row.get("text").map(|t| match t {
                oxigraph::model::Term::Literal(l) => l.value().to_string(),
                other => other.to_string(),
            }).unwrap_or_default();
            let note_type = row.get("type").map(|t| match t {
                oxigraph::model::Term::Literal(l) => l.value().to_string(),
                other => other.to_string(),
            }).unwrap_or_default();
            (slug, note_type, text)
        })
        .collect();

    if rows.is_empty() {
        println!("No notes found.");
        return Ok(());
    }

    for (slug, note_type, text) in &rows {
        let short_text: String = text.chars().take(70).collect();
        let text_display = if text.chars().count() > 70 { format!("{short_text}…") } else { short_text };
        println!("[{note_type}] {text_display}");
        println!("  slug: {slug}");
        println!();
    }
    println!("{} note(s). Use --remove <slug> or --update <slug> --text \"...\" to manage.", rows.len());
    Ok(())
}

pub fn recall_by_slug(cwd: &Path, ns: &NamespaceConfig, slug: &str) -> Result<()> {
    let p = &ns.prefix;
    let iri = crud::build_iri(ns, "note", slug);

    let sparql = format!(
        "SELECT ?text ?type ?created WHERE {{\n\
           GRAPH ?g {{\n\
             <{iri}> a {p}:Note ; {p}:noteText ?text ; {p}:noteType ?type .\n\
             OPTIONAL {{ <{iri}> {p}:createdAt ?created }}\n\
           }}\n\
         }}"
    );

    let results = crud::load_and_query(cwd, ns, &sparql)?;
    let QueryResults::Solutions(solutions) = results else {
        println!("Not found: note/{slug}");
        return Ok(());
    };

    let mut found = false;
    for row in solutions.filter_map(|r| r.ok()) {
        found = true;
        let text = row.get("text").map(|t| match t {
            oxigraph::model::Term::Literal(l) => l.value().to_string(),
            other => other.to_string(),
        }).unwrap_or_default();
        let note_type = row.get("type").map(|t| match t {
            oxigraph::model::Term::Literal(l) => l.value().to_string(),
            other => other.to_string(),
        }).unwrap_or_default();
        let created = row.get("created").map(|t| match t {
            oxigraph::model::Term::Literal(l) => l.value().to_string(),
            other => other.to_string(),
        }).unwrap_or_else(|| "-".into());

        println!("note/{slug}");
        println!("  Type: {note_type}");
        println!("  Created: {created}");
        println!("  Text: {text}");
    }

    if !found {
        println!("Not found: note/{slug}");
    }
    Ok(())
}

