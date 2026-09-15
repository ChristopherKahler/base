use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use oxigraph::sparql::QueryResults;
use oxigraph::store::Store;

use crate::config::NamespaceConfig;
use crate::crud;

/// Resolve the target graph file + graph IRI for a write.
///
/// Workspace tier, always. The old global catchall (issue #8) meant a handoff
/// created outside a workspace landed in `~/.base-gbl` and then resurfaced at
/// the start of every unrelated project, forever. Global is now something you
/// opt into with `-g` — which routes cwd to `~/.base-gbl`, so this resolves it
/// as that tier's workspace rather than as a silent fallback.
fn write_tier(cwd: &Path, ns: &NamespaceConfig) -> Result<(PathBuf, String)> {
    let base = crate::config::find_workspace_base(cwd).context(
        "no .base/ directory found — refusing to write outside a workspace. \
         Use --global (-g) to file this against the global tier deliberately, \
         or run `base scaffold` here first.",
    )?;
    let ws_slug = crud::workspace_slug(cwd);
    Ok((base.join("graph.nq"), crud::workspace_graph_iri(ns, &ws_slug)))
}

/// Every existing graph file across tiers — used for tier-agnostic mutations
/// (snooze/archive) so a handoff is updated wherever it lives.
///
/// Takes `gbl_root` rather than resolving the home directory itself. This
/// function is why the fork exists: reaching for the home directory here meant
/// every test that archived a fixture handoff rewrote the operator's real
/// global graph. As a parameter the compiler will not let a caller — test or
/// otherwise — forget to say which root it means.
fn all_tier_files(gbl_root: Option<&Path>, cwd: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Some(home) = gbl_root {
        let gbl = home.join(".base-gbl").join(".base").join("graph.nq");
        if gbl.exists() {
            files.push(gbl);
        }
    }
    if let Some(base) = crate::config::find_workspace_base(cwd) {
        let ws = base.join("graph.nq");
        if ws.exists() {
            files.push(ws);
        }
    }
    // Dedupe by canonical path. Under `-g` the global tier IS the workspace, so
    // both entries resolve to one file and the UPDATE ran twice on it — visible
    // in production as two identical archive lines in changes.jsonl at the same
    // second (global feed, 2026-09-07 15:07:48).
    let mut seen: Vec<PathBuf> = Vec::new();
    files.retain(|f| {
        let key = f.canonicalize().unwrap_or_else(|_| f.clone());
        if seen.contains(&key) {
            false
        } else {
            seen.push(key);
            true
        }
    });
    files
}

/// Which tier `file` belongs to, for the operator-facing line.
fn tier_label(file: &Path, gbl_root: Option<&Path>) -> &'static str {
    match gbl_root {
        Some(h) if file.starts_with(h.join(".base-gbl")) => "global tier",
        _ => "workspace tier",
    }
}

/// Does `file` hold this handoff/fork at all?
///
/// Asked BEFORE mutating, because a SPARQL UPDATE whose WHERE binds nothing is a
/// silent no-op: 0.14.1 printed `Fork '<slug>' archived` with rc 0 for a slug
/// that existed in neither tier (#72). Runs on the store the caller already
/// loaded, so it costs no extra parse of a 16 MB graph.
fn store_holds(store: &Store, ns: &NamespaceConfig, slug: &str) -> bool {
    let iri = crud::build_iri(ns, "handoff", slug);
    let p = &ns.prefix;
    let ask = format!(
        "{}\nASK {{ GRAPH ?g {{ <{iri}> a {p}:Handoff }} }}",
        crud::prefixes(ns)
    );
    matches!(
        store.query(&ask),
        Ok(QueryResults::Boolean(true))
    )
}

/// Mutate `path` only if it holds `slug`. Returns whether it did.
///
/// Load, ask and write all happen inside one graph lock: asking outside it would
/// answer about a graph that another writer may have replaced before the write.
fn mutate_file_if_holds(
    path: &Path,
    ns: &NamespaceConfig,
    slug: &str,
    sparql: &str,
) -> Result<bool> {
    crate::store::with_graph_lock(path, || {
        let store = crate::store::load_or_empty(path)?;
        if !store_holds(&store, ns, slug) {
            return Ok(false);
        }
        let full = format!("{}\n{}", crud::prefixes(ns), sparql);
        crate::store::update_and_write(
            &store,
            path,
            &full,
            crate::store::Scope::Target,
            crate::store::Intent::Knowledge,
        )
        .with_context(|| format!("handoff update failed: {full}"))?;
        Ok(true)
    })
}

/// Load one graph file, run a SPARQL UPDATE, write back atomically.
fn mutate_file(path: &Path, ns: &NamespaceConfig, sparql: &str) -> Result<()> {
    let full = format!("{}\n{}", crud::prefixes(ns), sparql);
    // Locked, load inside: four builders registering inside twelve seconds is
    // how #71-#74 were filed, and every one of those writes reported success.
    crate::store::locked_update(
        path,
        &full,
        crate::store::Scope::Target,
        crate::store::Intent::Knowledge,
    )
    .with_context(|| format!("handoff update failed: {full}"))
}

/// Derive a flow-doc slug from its doc path basename (no extension).
/// `/abs/path/FORK-COMMAND-SPEC.md` → `FORK-COMMAND-SPEC`. Used VERBATIM (no
/// slugify/lowercase) so the doc filename and the graph slug are the SAME string
/// — that single name is the title a handoff/fork is summoned by (doc==slug protocol).
fn doc_basename_slug(doc_path: &str) -> Result<String> {
    Path::new(doc_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .with_context(|| format!("could not derive a slug from doc path '{doc_path}'"))
}

/// Resolve the slug for a flow-doc: an explicit `--slug` override (verbatim) when
/// given and non-blank, else the doc basename. THE STANDARD for both handoff and
/// fork: doc filename and graph slug always align, so the operator summons the
/// next session by one consistent name.
fn resolve_doc_slug(slug: Option<&str>, doc_path: &str) -> Result<String> {
    match slug {
        Some(s) if !s.trim().is_empty() => Ok(s.trim().to_string()),
        _ => doc_basename_slug(doc_path),
    }
}

/// What a `handoff create` actually did, so the CLI can say it.
///
/// 0.14.1 archived the prior handoff silently and only in the tier it wrote to,
/// while the help promised a project-wide archive. Four builders registering
/// inside twelve seconds each archived the one before it with no output, and an
/// open handoff in the other tier sat there untouched and unmentioned (#71).
/// 0.15.2 named an open handoff in the other tier and left it open, and under
/// `-g` could not see the workspace at all (lane 2, rank 08).
#[derive(Debug)]
pub struct CreateOutcome {
    pub slug: String,
    /// The tier this create wrote to: "workspace tier" or "global tier".
    pub tier: String,
    /// Every prior handoff this create archived, as (slug, tier), across every
    /// tier (`auk`'s Q2 ruling). Empty when the project had no prior open or
    /// deferred continuity handoff anywhere.
    pub archived: Vec<(String, String)>,
}

/// The prior continuity handoffs for `project` in one graph file: status `open`
/// or `deferred` (E4), never a fork.
///
/// Forks share the Handoff type and the project but are additive side-work, so
/// they are excluded here exactly as they are in `create`'s archive step. A file
/// that cannot be read is an error, never an empty answer: `create` prints "in any
/// tier", and decides what an unreadable tier costs.
fn prior_handoffs_in(file: &Path, ns: &NamespaceConfig, project: &str) -> Result<Vec<String>> {
    let store = crate::store::load_or_empty(file)?;
    let p = &ns.prefix;
    let esc = crud::escape_sparql_literal(project);
    let q = format!(
        "{}\nSELECT ?h WHERE {{ GRAPH ?g {{ ?h a {p}:Handoff ; {p}:project \"{esc}\" ; {p}:status ?s .\n\
           FILTER(?s IN (\"open\", \"deferred\"))\n\
           OPTIONAL {{ ?h {p}:kind ?kind }}\n\
           FILTER(!BOUND(?kind) || ?kind != \"fork\") }} }}",
        crud::prefixes(ns)
    );
    let QueryResults::Solutions(solutions) = crate::store::query(&store, &q)? else {
        return Ok(Vec::new());
    };
    let mut out: Vec<String> = solutions
        .filter_map(|sol| sol.ok())
        .filter_map(|sol| sol.get("h").map(|term| crud::term_display(term.as_ref())))
        .map(|iri| iri.rsplit('/').next().unwrap_or(&iri).to_string())
        .collect();
    out.sort();
    out.dedup();
    Ok(out)
}

/// Register a handoff pointing at a resume document. Archives the project's prior
/// open or deferred continuity handoff in every tier (one open handoff per
/// project, E4), then inserts the new one with `resurfaceAt = now` so it surfaces
/// next session start. Slug defaults to the doc basename (doc==slug protocol);
/// pass `slug` to override. Re-registering the same slug re-points it
/// idempotently (no duplicate triples).
///
/// `cwd` picks the tier to write. `standing_cwd` is where the operator stands,
/// and every tier is found from there: under `-g` the CLI routes `cwd` to the
/// global tier, and discovery from it could never see the workspace (rank 08, R3).
pub fn create(
    gbl_root: Option<&Path>,
    cwd: &Path,
    standing_cwd: &Path,
    ns: &NamespaceConfig,
    project: &str,
    doc_path: &str,
    slug: Option<&str>,
) -> Result<CreateOutcome> {
    let now = crud::now_iso();
    let slug = resolve_doc_slug(slug, doc_path)?;
    let iri = crud::build_iri(ns, "handoff", &slug);
    let (path, graph) = write_tier(cwd, ns)?;
    let p = &ns.prefix;
    // The project this handoff names, as the IRI its domain hangs off (kite F7b).
    let project_iri = crud::build_iri(ns, "project", &crud::slugify(project));
    // `prior_handoffs_in` escapes the name itself. Handed the escaped copy, it
    // searched for a different literal whenever the name held a quote or a backslash.
    let project_name = project;
    let project = crud::escape_sparql_literal(project);
    let doc = crud::escape_sparql_literal(doc_path);

    // 1. Archive the project's prior open or deferred *continuity* handoff (E4), in
    //    every tier that holds one. `GRAPH ?g`: a tier file can hold another
    //    workspace's named graph, and the read that names what was archived
    //    matches any graph too, so the list and the write agree. Forks
    //    (kind = "fork") share the Handoff type + project but are additive
    //    side-work — never archived, in any tier. In the tier being written the new
    //    slug is left out, because step 2 re-points it there; in another tier it
    //    is an older copy and is archived (`auk`'s ruling D1).
    let archive_prior = |exclude: &str| {
        format!(
            "DELETE {{ GRAPH ?g {{ ?h {p}:status ?s }} }}\n\
             INSERT {{ GRAPH ?g {{ ?h {p}:status \"archived\" }} }}\n\
             WHERE  {{ GRAPH ?g {{ ?h a {p}:Handoff ; {p}:project \"{project}\" ; {p}:status ?s .\n\
               FILTER(?s IN (\"open\", \"deferred\"))\n\
               OPTIONAL {{ ?h {p}:kind ?kind }}\n\
               FILTER(!BOUND(?kind) || ?kind != \"fork\"){exclude} }} }}"
        )
    };
    let archive_prior_here = archive_prior(&format!("\n           FILTER(?h != <{iri}>)"));
    let archive_prior_elsewhere = archive_prior("");

    // 2. Clean any existing node at this exact slug so re-registration re-points
    //    it instead of layering duplicate status/timestamp triples.
    let clean_target = format!(
        "DELETE {{ GRAPH <{graph}> {{ <{iri}> ?dp ?do }} }} WHERE {{ GRAPH <{graph}> {{ <{iri}> ?dp ?do }} }}"
    );

    // 3. Insert the new handoff.
    let insert = format!(
        "INSERT DATA {{ GRAPH <{graph}> {{\n\
           <{iri}> rdf:type {p}:Handoff ;\n\
             {p}:name \"{project}\" ;\n\
             {p}:project \"{project}\" ;\n\
             {p}:handoffDoc \"{doc}\" ;\n\
             {p}:kind \"handoff\" ;\n\
             {p}:status \"open\" ;\n\
             {p}:createdAt \"{now}\"^^xsd:dateTime ;\n\
             {p}:resurfaceAt \"{now}\"^^xsd:dateTime ;\n\
             {p}:lastActive \"{now}\"^^xsd:dateTime .\n\
         }} }}"
    );

    // 4. The handoff takes the domain of the project it names, in the same write.
    let inherit = crate::domain::link::inherit_update(ns, &graph, &iri, &project_iri);

    // Ask before writing, in every tier: the archive step is a bulk UPDATE that
    // leaves no trace of WHICH handoff it closed, so the names are read while they
    // are still open or deferred. A re-register of the same slug is not a prior
    // handoff in the tier it re-points. A tier being written that cannot be read
    // is a plain error, and nothing is written.
    let tier = tier_label(&path, gbl_root);
    let mut archived: Vec<(String, String)> = prior_handoffs_in(&path, ns, project_name)?
        .into_iter()
        .filter(|prior| *prior != slug)
        .map(|prior| (prior, tier.to_string()))
        .collect();
    // What an error after the write says first: the handoff is registered, and
    // what it archived in its own tier, so no archive goes unmentioned (#71).
    let registered = if archived.is_empty() {
        format!("registered '{slug}' in the {tier}")
    } else {
        let names: Vec<String> = archived.iter().map(|(prior, t)| format!("{prior} ({t})")).collect();
        format!("registered '{slug}' in the {tier} and archived {}", names.join(", "))
    };
    // Every other tier, found from where the operator stands. Only a tier that
    // holds a prior handoff is written: the global graph is shared by every live
    // session. A tier that cannot be read must not stop the handoff being
    // registered (`auk`'s ruling D5): it is skipped, and named after the write.
    let key = |f: &Path| f.canonicalize().unwrap_or_else(|_| f.to_path_buf());
    let written = key(&path);
    let mut elsewhere: Vec<PathBuf> = Vec::new();
    let mut unreadable: Vec<String> = Vec::new();
    for file in all_tier_files(gbl_root, standing_cwd) {
        if key(&file) == written {
            continue;
        }
        let label = tier_label(&file, gbl_root);
        match prior_handoffs_in(&file, ns, project_name) {
            Ok(priors) if priors.is_empty() => {}
            Ok(priors) => {
                archived.extend(priors.into_iter().map(|prior| (prior, label.to_string())));
                elsewhere.push(file);
            }
            Err(e) => unreadable.push(format!(
                "could not read the {label} at {} to look for a prior handoff there: {e:#}",
                file.display()
            )),
        }
    }

    // The tier being written goes first, in one write with the new node, so a
    // failure in another tier leaves one extra open handoff and says so, never an
    // archived handoff with nothing registered in its place.
    mutate_file(
        &path,
        ns,
        &format!("{archive_prior_here};\n{clean_target};\n{insert};\n{inherit}"),
    )?;
    for file in &elsewhere {
        mutate_file(file, ns, &archive_prior_elsewhere).with_context(|| {
            format!(
                "{registered}, but archiving the prior handoff in the {} failed",
                tier_label(file, gbl_root)
            )
        })?;
    }
    if !unreadable.is_empty() {
        anyhow::bail!("{registered}, but {}", unreadable.join("; and "));
    }
    Ok(CreateOutcome {
        slug,
        tier: tier.to_string(),
        archived,
    })
}

/// Register a fork pointing at a build-spec document. Forks are ADDITIVE —
/// creating one does NOT archive sibling forks (contrast `create`, which archives
/// the prior open handoff for the project). Slug defaults to the doc basename
/// (doc==slug protocol), so `handoff/<doc-basename>` is the node IRI; pass `slug`
/// to override. `resurfaceAt = now` so it surfaces next session.
pub fn create_fork(
    cwd: &Path,
    ns: &NamespaceConfig,
    project: &str,
    doc_path: &str,
    slug: Option<&str>,
) -> Result<String> {
    let now = crud::now_iso();
    let slug = resolve_doc_slug(slug, doc_path)?;
    let iri = crud::build_iri(ns, "handoff", &slug);
    let (path, graph) = write_tier(cwd, ns)?;
    let p = &ns.prefix;
    // The project this handoff names, as the IRI its domain hangs off (kite F7b).
    let project_iri = crud::build_iri(ns, "project", &crud::slugify(project));
    let project = crud::escape_sparql_literal(project);
    let name = crud::escape_sparql_literal(&slug);
    let doc = crud::escape_sparql_literal(doc_path);

    // Additive: no archive-prior. A re-create of the same slug re-points it
    // (idempotent) by deleting any existing node at this IRI first.
    let insert = format!(
        "DELETE {{ GRAPH <{graph}> {{ <{iri}> ?dp ?do }} }} WHERE {{ GRAPH <{graph}> {{ <{iri}> ?dp ?do }} }};\n\
         INSERT DATA {{ GRAPH <{graph}> {{\n\
           <{iri}> rdf:type {p}:Handoff ;\n\
             {p}:name \"{name}\" ;\n\
             {p}:project \"{project}\" ;\n\
             {p}:handoffDoc \"{doc}\" ;\n\
             {p}:kind \"fork\" ;\n\
             {p}:status \"open\" ;\n\
             {p}:createdAt \"{now}\"^^xsd:dateTime ;\n\
             {p}:resurfaceAt \"{now}\"^^xsd:dateTime ;\n\
             {p}:lastActive \"{now}\"^^xsd:dateTime .\n\
         }} }}"
    );

    // The fork takes the domain of the project it names, in the same write (kite F7b).
    let inherit = crate::domain::link::inherit_update(ns, &graph, &iri, &project_iri);
    mutate_file(&path, ns, &format!("{insert};\n{inherit}"))?;
    Ok(slug)
}

/// List handoffs (continuity docs only — forks excluded) across both tiers.
pub fn list(cwd: &Path, ns: &NamespaceConfig) -> Result<()> {
    let Some(store) = crate::store::load_merged(cwd) else {
        println!("No handoffs.");
        return Ok(());
    };
    let p = &ns.prefix;
    let sparql = format!(
        "{pfx}\nSELECT ?h ?project ?status ?resurfaceAt WHERE {{\n\
           GRAPH ?g {{\n\
             ?h a {p}:Handoff ;\n\
               {p}:project ?project ;\n\
               {p}:status ?status .\n\
             OPTIONAL {{ ?h {p}:resurfaceAt ?resurfaceAt }}\n\
             OPTIONAL {{ ?h {p}:kind ?kind }}\n\
             FILTER(!BOUND(?kind) || ?kind != \"fork\")\n\
           }}\n\
         }}\n\
         ORDER BY ?status ?project",
        pfx = crud::prefixes(ns)
    );

    if let QueryResults::Solutions(solutions) = crate::store::query(&store, &sparql)? {
        let rows: Vec<Vec<String>> = solutions
            .filter_map(|r| r.ok())
            .map(|row| {
                let get = |k: &str| {
                    row.get(k).map(|t| crud::term_display(t.into())).unwrap_or_default()
                };
                let h = get("h");
                let slug = h.rsplit('/').next().unwrap_or(&h).to_string();
                vec![slug, get("project"), get("status"), get("resurfaceAt")]
            })
            .collect();

        if rows.is_empty() {
            println!("No handoffs.");
            return Ok(());
        }

        println!("| slug | project | status | resurfaceAt |");
        println!("|------|---------|--------|-------------|");
        for row in &rows {
            println!("| {} | {} | {} | {} |", row[0], row[1], row[2], row[3]);
        }
    }
    Ok(())
}

/// List forks (parallel side-work build-specs) across both tiers. Multiple may
/// be open at once. Title == slug == doc basename.
pub fn list_forks(cwd: &Path, ns: &NamespaceConfig) -> Result<()> {
    let Some(store) = crate::store::load_merged(cwd) else {
        println!("No forks.");
        return Ok(());
    };
    let p = &ns.prefix;
    let sparql = format!(
        "{pfx}\nSELECT ?h ?project ?status ?doc WHERE {{\n\
           GRAPH ?g {{\n\
             ?h a {p}:Handoff ;\n\
               {p}:kind \"fork\" ;\n\
               {p}:project ?project ;\n\
               {p}:status ?status ;\n\
               {p}:handoffDoc ?doc .\n\
           }}\n\
         }}\n\
         ORDER BY ?status ?h",
        pfx = crud::prefixes(ns)
    );

    if let QueryResults::Solutions(solutions) = crate::store::query(&store, &sparql)? {
        let rows: Vec<Vec<String>> = solutions
            .filter_map(|r| r.ok())
            .map(|row| {
                let get = |k: &str| {
                    row.get(k).map(|t| crud::term_display(t.into())).unwrap_or_default()
                };
                let h = get("h");
                let slug = h.rsplit('/').next().unwrap_or(&h).to_string();
                vec![slug, get("project"), get("status"), get("doc")]
            })
            .collect();

        if rows.is_empty() {
            println!("No forks.");
            return Ok(());
        }

        println!("| title | project | status | doc |");
        println!("|-------|---------|--------|-----|");
        for row in &rows {
            println!("| {} | {} | {} | {} |", row[0], row[1], row[2], row[3]);
        }
    }
    Ok(())
}

/// Snooze a handoff: push `resurfaceAt` to now + `days`, hiding it until then.
/// Applied to every tier file so it works wherever the handoff lives.
pub fn snooze(
    gbl_root: Option<&Path>,
    cwd: &Path,
    ns: &NamespaceConfig,
    slug: &str,
    days: i64,
) -> Result<Vec<String>> {
    let iri = crud::build_iri(ns, "handoff", slug);
    let wake = (chrono::Local::now() + chrono::Duration::days(days))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let p = &ns.prefix;
    let sparql = format!(
        "DELETE {{ GRAPH ?g {{ <{iri}> {p}:resurfaceAt ?old }} }}\n\
         INSERT {{ GRAPH ?g {{ <{iri}> {p}:resurfaceAt \"{wake}\"^^xsd:dateTime }} }}\n\
         WHERE  {{ GRAPH ?g {{ <{iri}> a {p}:Handoff }}\n\
           OPTIONAL {{ GRAPH ?g {{ <{iri}> {p}:resurfaceAt ?old }} }} }}"
    );
    apply_to_tiers(gbl_root, cwd, ns, slug, &sparql)
}

/// Archive a handoff: set status to "archived" so it stops resurfacing.
pub fn archive(
    gbl_root: Option<&Path>,
    cwd: &Path,
    ns: &NamespaceConfig,
    slug: &str,
) -> Result<Vec<String>> {
    let iri = crud::build_iri(ns, "handoff", slug);
    let p = &ns.prefix;
    let sparql = format!(
        "DELETE {{ GRAPH ?g {{ <{iri}> {p}:status ?old }} }}\n\
         INSERT {{ GRAPH ?g {{ <{iri}> {p}:status \"archived\" }} }}\n\
         WHERE  {{ GRAPH ?g {{ <{iri}> a {p}:Handoff }}\n\
           OPTIONAL {{ GRAPH ?g {{ <{iri}> {p}:status ?old }} }} }}"
    );
    apply_to_tiers(gbl_root, cwd, ns, slug, &sparql)
}

/// Run `sparql` against every tier file that actually holds `slug`, and return
/// the tier labels that changed.
///
/// An empty vec means no tier held it — the caller must NOT print success. That
/// is the whole of #72's observable: 0.14.1 ran the UPDATE over each tier file
/// and printed `archived` unconditionally, so a no-op and a real archive were
/// indistinguishable from the outside.
fn apply_to_tiers(
    gbl_root: Option<&Path>,
    cwd: &Path,
    ns: &NamespaceConfig,
    slug: &str,
    sparql: &str,
) -> Result<Vec<String>> {
    let mut changed = Vec::new();
    for f in all_tier_files(gbl_root, cwd) {
        if mutate_file_if_holds(&f, ns, slug, sparql)? {
            changed.push(tier_label(&f, gbl_root).to_string());
        }
    }
    Ok(changed)
}

/// The tier files a lookup searched, for the not-found sentence.
pub fn searched_tiers(gbl_root: Option<&Path>, cwd: &Path) -> Vec<String> {
    all_tier_files(gbl_root, cwd)
        .iter()
        .map(|f| format!("{} ({})", f.display(), tier_label(f, gbl_root)))
        .collect()
}
