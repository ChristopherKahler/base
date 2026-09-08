use std::path::{Path, PathBuf};

use anyhow::Result;
use oxigraph::sparql::QueryResults;

use crate::config::NamespaceConfig;
use crate::crud;

/// Query AST entities by label (case-insensitive substring match).
/// Returns: file, line, type, calls, called-by for each match.
pub fn contains(cwd: &Path, ns: &NamespaceConfig, name: &str) -> Result<()> {
    let store = load_ast_store_reporting(cwd)?;
    let pfx = ast_prefixes(ns);
    let name_lower = crud::escape_sparql_literal(&name.to_lowercase());

    // Find entities whose label contains the search term
    let sparql = format!(
        "{pfx}\n\
         SELECT ?entity ?label ?file ?line ?type WHERE {{\n\
           ?entity rdfs:label ?label ;\n\
             rdf:type ?type .\n\
           OPTIONAL {{ ?entity ops:sourceFile ?file }}\n\
           OPTIONAL {{ ?entity ops:sourceLine ?line }}\n\
           FILTER(CONTAINS(LCASE(STR(?label)), \"{name_lower}\"))\n\
         }}\n\
         ORDER BY ?file ?line"
    );

    let results = crate::store::query(&store, &sparql)?;
    if let QueryResults::Solutions(solutions) = results {
        let rows: Vec<(String, String, String, String, String)> = solutions
            .filter_map(|r| r.ok())
            .map(|row| {
                let label = get_str(&row, "label");
                let file = get_str(&row, "file");
                let line = get_str(&row, "line");
                let etype = get_type_str(&row, "type");
                let entity_iri = row
                    .get("entity")
                    .map(|t| t.to_string())
                    .unwrap_or_default();
                (label, file, line, etype, entity_iri)
            })
            .collect();

        if rows.is_empty() {
            println!("No AST entities matching '{name}'.");
            return Ok(());
        }

        for (label, file, line, etype, entity_iri) in &rows {
            let loc = if !line.is_empty() {
                format!("{file}:{line}")
            } else if !file.is_empty() {
                file.clone()
            } else {
                "unknown".into()
            };
            println!("{loc}  {etype} {label}");

            // Query calls
            let calls = query_calls(&store, ns, entity_iri);
            if !calls.is_empty() {
                println!("  calls: {}", calls.join(", "));
            }

            // Query called-by
            let callers = query_callers(&store, ns, entity_iri);
            if !callers.is_empty() {
                println!("  called_by: {}", callers.join(", "));
            }
        }
    }
    Ok(())
}

/// List all entities in a source file with their relationships.
pub fn file(cwd: &Path, ns: &NamespaceConfig, file_path: &str) -> Result<()> {
    let store = load_ast_store_reporting(cwd)?;
    let pfx = ast_prefixes(ns);

    // Normalize: accept "src/cli.rs" or "cli.rs" — match by CONTAINS on sourceFile.
    // Separators first: `sourceFile` literals are forward-slash, so a Windows
    // probe has to be reduced to the same form before it is compared.
    let normalized = crud::normalize_path_sep(file_path);
    let file_lower = normalized
        .trim_start_matches("src/")
        .trim_start_matches("./");

    // `ops:sourceFile` sits INSIDE the first basic graph pattern, before the
    // OPTIONAL, exactly as file_map_compact() writes it. Measured 2026-09-01 on
    // a 25,303-entity map: with the sourceFile pattern placed AFTER the OPTIONAL
    // the algebra becomes Join(LeftJoin(BGP, line), BGP{sourceFile}) and
    // oxigraph 0.4 evaluates that join without the index — 112 s for one
    // `base ast query --file`, against 0.3 s for the compact form on the same
    // data and 1.1 s for --contains / --imports. Same map, same file, one line
    // moved. Keep the FILTER's pattern in the BGP it filters.
    let sparql = format!(
        "{pfx}\n\
         SELECT ?entity ?label ?line ?type WHERE {{\n\
           ?entity rdf:type ?type ;\n\
             rdfs:label ?label ;\n\
             ops:sourceFile ?file .\n\
           OPTIONAL {{ ?entity ops:sourceLine ?line }}\n\
           FILTER(CONTAINS({norm}, \"{}\"))\n\
         }}\n\
         ORDER BY ?line",
        crud::escape_sparql_literal(&file_lower.to_lowercase()),
        norm = norm_file_expr("file")
    );

    let results = crate::store::query(&store, &sparql)?;
    if let QueryResults::Solutions(solutions) = results {
        let rows: Vec<(String, String, String, String)> = solutions
            .filter_map(|r| r.ok())
            .map(|row| {
                let label = get_str(&row, "label");
                let line = get_str(&row, "line");
                let etype = get_type_str(&row, "type");
                let entity_iri = row
                    .get("entity")
                    .map(|t| t.to_string())
                    .unwrap_or_default();
                (label, line, etype, entity_iri)
            })
            .collect();

        if rows.is_empty() {
            println!("No AST entities found for '{file_path}'.");
            return Ok(());
        }

        println!("[AST] {file_path} — {} entities", rows.len());
        for (label, line, etype, _) in &rows {
            if !line.is_empty() {
                println!("  {etype} {label} (line {line})");
            } else {
                println!("  {etype} {label}");
            }
        }

        // Query imports
        let imports = query_file_imports(&store, ns, file_lower);
        if !imports.is_empty() {
            println!("  imports: {}", imports.join(", "));
        }

        // Query imported-by
        let importers = query_file_importers(&store, ns, file_lower);
        if !importers.is_empty() {
            println!("  imported_by: {}", importers.join(", "));
        }
    }
    Ok(())
}

/// Find all callers of a named entity.
pub fn calls(cwd: &Path, ns: &NamespaceConfig, name: &str) -> Result<()> {
    let store = load_ast_store_reporting(cwd)?;
    let pfx = ast_prefixes(ns);
    let name_lower = crud::escape_sparql_literal(&name.to_lowercase());

    // Find the entity — labels may have () suffix, so use CONTAINS
    let find = format!(
        "{pfx}\n\
         SELECT ?entity ?label ?file ?line WHERE {{\n\
           ?entity rdfs:label ?label .\n\
           OPTIONAL {{ ?entity ops:sourceFile ?file }}\n\
           OPTIONAL {{ ?entity ops:sourceLine ?line }}\n\
           FILTER(CONTAINS(LCASE(STR(?label)), \"{name_lower}\"))\n\
         }}"
    );

    let results = crate::store::query(&store, &find)?;
    if let QueryResults::Solutions(solutions) = results {
        let targets: Vec<(String, String, String, String)> = solutions
            .filter_map(|r| r.ok())
            .map(|row| {
                let entity_iri = row
                    .get("entity")
                    .map(|t| t.to_string())
                    .unwrap_or_default();
                let label = get_str(&row, "label");
                let file = get_str(&row, "file");
                let line = get_str(&row, "line");
                (entity_iri, label, file, line)
            })
            .collect();

        if targets.is_empty() {
            println!("No entity named '{name}' found.");
            return Ok(());
        }

        for (entity_iri, label, file, line) in &targets {
            let loc = if !line.is_empty() {
                format!("{file}:{line}")
            } else {
                file.clone()
            };
            println!("{loc}  {label}");

            // Find all callers
            let callers = query_callers(&store, ns, entity_iri);
            if callers.is_empty() {
                println!("  No callers found.");
            } else {
                println!("  called_by:");
                for caller in &callers {
                    println!("    {caller}");
                }
            }
        }
    }
    Ok(())
}

/// Find all files that import from a given file/module.
pub fn imports(cwd: &Path, ns: &NamespaceConfig, file_path: &str) -> Result<()> {
    let store = load_ast_store_reporting(cwd)?;
    let pfx = ast_prefixes(ns);
    let file_lower = crud::normalize_path_sep(file_path)
        .trim_start_matches("src/")
        .trim_start_matches("./")
        .to_lowercase();
    // Strip extension for IRI matching (imports often reference modules, not files)
    let stem = crud::escape_sparql_literal(
        file_lower.trim_end_matches(".rs").trim_end_matches(".py")
            .trim_end_matches(".js").trim_end_matches(".ts")
    );
    let file_lower = crud::escape_sparql_literal(&file_lower);

    // Match by target IRI containing the stem OR target label containing the filename
    let sparql = format!(
        "{pfx}\n\
         SELECT DISTINCT ?importer_file WHERE {{\n\
           ?importer ops:importsFrom ?target .\n\
           ?importer ops:sourceFile ?importer_file .\n\
           OPTIONAL {{ ?target rdfs:label ?target_label }}\n\
           FILTER(\n\
             CONTAINS(LCASE(STR(?target)), \"{stem}\")\n\
             || (BOUND(?target_label) && CONTAINS(LCASE(STR(?target_label)), \"{file_lower}\"))\n\
           )\n\
         }}\n\
         ORDER BY ?importer_file"
    );

    let results = crate::store::query(&store, &sparql)?;
    if let QueryResults::Solutions(solutions) = results {
        let rows: Vec<String> = solutions
            .filter_map(|r| r.ok())
            .map(|row| get_str(&row, "importer_file"))
            .filter(|s| !s.is_empty())
            .collect();

        if rows.is_empty() {
            println!("No files import from '{file_path}'.");
            return Ok(());
        }

        println!("Files importing from {file_path}:");
        for f in &rows {
            println!("  {f}");
        }
    }
    Ok(())
}

/// Compact file map for hook injection. Returns None if no AST data found.
pub fn file_map_compact(cwd: &Path, ns: &NamespaceConfig, file_path: &str) -> Option<String> {
    // Resolve the AST map from the FILE's app root, not the session cwd. A file
    // inside a sub-app (apps/X/src/y.rs) reads apps/X's OWN sidecar map — whose
    // paths are rooted at the app ("src/y.rs") — even when the session runs from
    // the parent workspace. Without this, touching a sub-app file from the parent
    // queries the parent's (stale, differently-rooted) map and injects nothing.
    let app_root = crate::config::ast_app_root(Path::new(file_path))
        .unwrap_or_else(|| cwd.to_path_buf());
    let store = load_ast_store(&app_root).ok()?;
    let pfx = ast_prefixes(ns);
    // Hook passes absolute paths; strip to app-root-relative for CONTAINS matching.
    // AST graph stores paths like "src/hook/pre_tool_use.rs".
    // Both sides normalized: on Windows the root and the probe are backslashed,
    // so `strip_prefix` would still work but `trim_start_matches('/')` would not.
    let file_norm = crud::normalize_path_sep(file_path);
    let root_norm = crud::normalize_path_sep(&app_root.to_string_lossy());
    let relative = file_norm
        .strip_prefix(&root_norm)
        .map(|p| p.trim_start_matches('/'))
        .unwrap_or(&file_norm);
    let file_lower = relative
        .trim_start_matches("./")
        .to_lowercase();

    let escaped = crate::crud::escape_sparql_literal(&file_lower);

    let sparql = format!(
        "{pfx}\n\
         SELECT ?entity ?label ?line ?type WHERE {{\n\
           ?entity rdf:type ?type ;\n\
             rdfs:label ?label ;\n\
             ops:sourceFile ?file .\n\
           OPTIONAL {{ ?entity ops:sourceLine ?line }}\n\
           FILTER(CONTAINS({norm}, \"{escaped}\"))\n\
         }}\n\
         ORDER BY ?line",
        norm = norm_file_expr("file")
    );

    let results = crate::store::query(&store, &sparql).ok()?;
    if let QueryResults::Solutions(solutions) = results {
        // (entity_iri, label, line, type)
        let rows: Vec<(String, String, String, String)> = solutions
            .filter_map(|r| r.ok())
            .map(|row| {
                let iri = row.get("entity").map(|t| t.to_string()).unwrap_or_default();
                let label = get_str(&row, "label");
                let line = get_str(&row, "line");
                let etype = get_type_str(&row, "type");
                (iri, label, line, etype)
            })
            .collect();

        if rows.is_empty() {
            return None;
        }

        let mut out = format!("[AST] {} — {} entities\n", file_path, rows.len());

        // Key entities (first 10)
        let key: Vec<String> = rows
            .iter()
            .take(10)
            .map(|(_iri, label, line, etype)| {
                if !line.is_empty() {
                    format!("{etype} {label} (line {line})")
                } else {
                    format!("{etype} {label}")
                }
            })
            .collect();
        out.push_str(&format!("  Key: {}\n", key.join(", ")));

        if rows.len() > 10 {
            out.push_str(&format!("  ... and {} more\n", rows.len() - 10));
        }

        // Imports / imported-by
        let imps = query_file_imports(&store, ns, &file_lower);
        if !imps.is_empty() {
            out.push_str(&format!("  Imports: {}\n", imps.join(", ")));
        }
        let importers = query_file_importers(&store, ns, &file_lower);
        if !importers.is_empty() {
            out.push_str(&format!("  Imported by: {}\n", importers.join(", ")));
        }

        // Call neighborhood: what this file's entities call, and who calls them.
        // The high-value "if I change this, here's what's connected" signal.
        let mut calls_out: Vec<String> = Vec::new();
        let mut called_by: Vec<String> = Vec::new();
        for (iri, ..) in rows.iter().take(12) {
            if iri.is_empty() {
                continue;
            }
            for c in query_calls(&store, ns, iri) {
                if !calls_out.contains(&c) {
                    calls_out.push(c);
                }
            }
            for c in query_callers(&store, ns, iri) {
                if !called_by.contains(&c) {
                    called_by.push(c);
                }
            }
        }
        if !calls_out.is_empty() {
            calls_out.truncate(15);
            out.push_str(&format!("  Calls out: {}\n", calls_out.join(", ")));
        }
        if !called_by.is_empty() {
            called_by.truncate(15);
            out.push_str(&format!("  Called by: {}\n", called_by.join(", ")));
        }

        Some(out)
    } else {
        None
    }
}

/// Section-specific entities for a line range. Returns None if no matches.
pub fn section_entities(
    cwd: &Path,
    ns: &NamespaceConfig,
    file_path: &str,
    offset: u64,
    limit: u64,
) -> Option<String> {
    let store = load_ast_store(cwd).ok()?;
    let pfx = ast_prefixes(ns);
    // App-root-relative when the root is known (the hook passes absolute
    // paths), else whatever was given; the tail match in file_match_expr() covers a
    // map rooted differently. Until 2026-09-01 this compared the BASENAME for
    // equality against a stored relative path, which matched only files at
    // the app root.
    let file_norm_probe = crud::normalize_path_sep(file_path);
    let relative = crate::config::ast_app_root(Path::new(file_path))
        .map(|r| crud::normalize_path_sep(&r.to_string_lossy()))
        .and_then(|root| file_norm_probe.strip_prefix(&root).map(|p| p.trim_start_matches('/').to_string()))
        .unwrap_or_else(|| file_norm_probe.clone());
    let file_lower = relative
        .trim_start_matches("src/")
        .trim_start_matches("./")
        .to_lowercase();
    let probe = crud::escape_sparql_literal(&file_lower);
    let end_line = offset + limit;

    let sparql = format!(
        "{pfx}\n\
         SELECT ?entity ?label ?line ?type WHERE {{\n\
           ?entity rdf:type ?type ;\n\
             rdfs:label ?label ;\n\
             ops:sourceFile ?file ;\n\
             ops:sourceLine ?line .\n\
           FILTER({is_file})\n\
           FILTER(?line >= {offset} && ?line <= {end_line})\n\
         }}\n\
         ORDER BY ?line",
        is_file = file_match_expr("file", &probe)
    );

    let results = crate::store::query(&store, &sparql).ok()?;
    if let QueryResults::Solutions(solutions) = results {
        let rows: Vec<(String, String, String, String)> = solutions
            .filter_map(|r| r.ok())
            .map(|row| {
                let entity_iri = row
                    .get("entity")
                    .map(|t| t.to_string())
                    .unwrap_or_default();
                let label = get_str(&row, "label");
                let line = get_str(&row, "line");
                let etype = get_type_str(&row, "type");
                (entity_iri, label, line, etype)
            })
            .collect();

        if rows.is_empty() {
            return None;
        }

        let mut out = format!(
            "[AST] Lines {}-{} of {}:\n",
            offset, end_line, file_path
        );
        for (entity_iri, label, line, etype) in &rows {
            out.push_str(&format!("  {etype} {label} (line {line})\n"));

            let calls = query_calls(&store, ns, entity_iri);
            if !calls.is_empty() {
                out.push_str(&format!("    calls: {}\n", calls.join(", ")));
            }
            let callers = query_callers(&store, ns, entity_iri);
            if !callers.is_empty() {
                out.push_str(&format!("    called_by: {}\n", callers.join(", ")));
            }
        }
        Some(out)
    } else {
        None
    }
}

/// Federate the per-app AST map (resolved from `cwd`) into a node/edge form the
/// concept graph can traverse. Returns `(nodes, edges)` where a node is
/// `(iri, label, type, source_file)` and an edge is `(from_iri, to_iri, relation)`
/// for `ops:calls` and `ops:importsFrom`. Best-effort: empty if no map is found,
/// so callers can unconditionally fold this into their graph.
#[allow(clippy::type_complexity)]
pub fn code_graph(
    cwd: &Path,
    ns: &NamespaceConfig,
) -> (Vec<(String, String, String, String)>, Vec<(String, String, String)>) {
    let Ok(store) = load_ast_store(cwd) else { return (Vec::new(), Vec::new()) };
    let pfx = ast_prefixes(ns);

    let mut nodes = Vec::new();
    let ent_q = format!(
        "{pfx}\n\
         SELECT ?e ?label ?type ?file WHERE {{\n\
           ?e rdfs:label ?label .\n\
           OPTIONAL {{ ?e rdf:type ?type }}\n\
           OPTIONAL {{ ?e ops:sourceFile ?file }}\n\
         }}"
    );
    if let Ok(QueryResults::Solutions(sols)) = crate::store::query(&store, &ent_q) {
        for row in sols.filter_map(|r| r.ok()) {
            let Some(id) = row.get("e").map(|t| t.to_string()) else { continue };
            let label = row.get("label").map(|t| crud::term_display(t.into())).unwrap_or_default();
            let ntype = row.get("type").map(|t| crud::term_display(t.into())).unwrap_or_default();
            let file = row.get("file").map(|t| crud::term_display(t.into())).unwrap_or_default();
            nodes.push((id, label, ntype, file));
        }
    }

    let mut edges = Vec::new();
    for (pattern, rel) in [("ops:calls", "calls"), ("ops:importsFrom", "imports")] {
        let eq = format!("{pfx}\nSELECT ?a ?b WHERE {{ ?a {pattern} ?b }}");
        if let Ok(QueryResults::Solutions(sols)) = crate::store::query(&store, &eq) {
            for row in sols.filter_map(|r| r.ok()) {
                if let (Some(a), Some(b)) =
                    (row.get("a").map(|t| t.to_string()), row.get("b").map(|t| t.to_string()))
                {
                    edges.push((a, b, rel.to_string()));
                }
            }
        }
    }

    (nodes, edges)
}

// ─── Internal helpers ────────────────────────────────────────

/// The stored `ops:sourceFile` literal with its separators reduced to `/`, as
/// a SPARQL expression. The extractor writes OS-native separators — `ui\deck.js`
/// on Windows — while every probe below is normalised to `/` before it is
/// compared, so until 2026-09-01 no path with a directory in it matched on
/// Windows: the hook injected a file map for ROOT-LEVEL files only, and
/// `--file ui/deck.js` found nothing while `--file deck.js` found 72 entities.
/// Normalising the stored side here keeps every existing map valid on both
/// OSes. REPLACE takes a regex; the SPARQL literal `"\\\\"` is the regex `\\`,
/// one backslash.
fn norm_file_expr(var: &str) -> String {
    format!("LCASE(REPLACE(STR(?{var}), \"\\\\\\\\\", \"/\"))")
}

/// A probe against the normalised stored path: equal to it, or its tail after
/// a `/`. Probes arrive app-root-relative (the hook) or as whatever the
/// operator typed (`deck.js`, `ui/deck.js`), and a map may be rooted one
/// level differently from the probe, so a tail match is the honest test.
fn file_match_expr(var: &str, probe_lower_escaped: &str) -> String {
    let norm = norm_file_expr(var);
    format!("({norm} = \"{probe_lower_escaped}\" || STRENDS({norm}, \"/{probe_lower_escaped}\"))")
}

fn ast_prefixes(ns: &NamespaceConfig) -> String {
    format!(
        "PREFIX {p}: <{u}>\n\
         PREFIX code: <http://ops-sys.local/code#>\n\
         PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\n\
         PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
         PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>",
        p = ns.prefix,
        u = ns.uri
    )
}

/// List registered per-app code maps from the workspace graph — what maps
/// exist, where, and how big. Lets Claude discover an app's map outside a dev
/// session, then query it with `base ast query --target <app>`.
pub fn list(cwd: &Path, ns: &NamespaceConfig) -> Result<()> {
    let p = &ns.prefix;
    let sparql = format!(
        "SELECT ?name ?count ?path ?synced WHERE {{\n\
           GRAPH ?g {{\n\
             ?m a {p}:CodeMap ;\n\
               {p}:name ?name ;\n\
               {p}:hasCodeMap ?path .\n\
             OPTIONAL {{ ?m {p}:astEntityCount ?count }}\n\
             OPTIONAL {{ ?m {p}:lastSynced ?synced }}\n\
           }}\n\
         }} ORDER BY ?name"
    );

    let results = crud::load_and_query(cwd, ns, &sparql)?;
    if let QueryResults::Solutions(solutions) = results {
        let rows: Vec<Vec<String>> = solutions
            .filter_map(|r| r.ok())
            .map(|row| {
                vec![
                    row.get("name").map(|t| crud::term_display(t.into())).unwrap_or_default(),
                    row.get("count").map(|t| crud::term_display(t.into())).unwrap_or_else(|| "-".into()),
                    row.get("path").map(|t| crud::term_display(t.into())).unwrap_or_default(),
                    row.get("synced").map(|t| crud::term_display(t.into())).unwrap_or_else(|| "-".into()),
                ]
            })
            .collect();

        if rows.is_empty() {
            println!("No code maps registered. Run `base sync --ast --target <app>`.");
            return Ok(());
        }

        println!("| app | entities | map | last synced |");
        println!("|-----|----------|-----|-------------|");
        for r in &rows {
            println!("| {} | {} | {} | {} |", r[0], r[1], r[2], r[3]);
        }
    }
    Ok(())
}

/// The map that answers for `cwd`, and where it came from.
///
/// The error names the directory. "No ast.ttl found" described the tool's own
/// bookkeeping; the reader needs to know WHICH directory has no map, because
/// the answer to that is what tells them whether to map it or to stand
/// somewhere else.
fn resolve_ast_store(cwd: &Path) -> Result<(oxigraph::store::Store, PathBuf)> {
    let ast_path = crate::config::find_ast_ttl(cwd).ok_or_else(|| {
        anyhow::anyhow!(
            "No code map covers {}. Run `base sync --ast --target {}` to map it.",
            cwd.display(),
            cwd.display()
        )
    })?;
    let store = oxigraph::store::Store::new()?;
    crate::store::load_turtle_into(&store, &ast_path)?;
    Ok((store, ast_path))
}

fn load_ast_store(cwd: &Path) -> Result<oxigraph::store::Store> {
    Ok(resolve_ast_store(cwd)?.0)
}

/// As [`load_ast_store`], and says which map answered when it is not `cwd`'s own.
///
/// Climbing to the app root is intended — a query from `src/crud/` answers from
/// the repo's map — but it must not be SILENT, or a reader cannot tell whose
/// code they are being shown. On stderr, so a caller parsing rows from stdout is
/// unaffected. The banner is provenance for the legitimate case; it is not what
/// makes the resolution correct, which is [`crate::config::find_ast_ttl`]'s
/// boundary.
fn load_ast_store_reporting(cwd: &Path) -> Result<oxigraph::store::Store> {
    let (store, ast_path) = resolve_ast_store(cwd)?;
    if ast_path.parent().and_then(Path::parent) != Some(cwd) {
        eprintln!("map: {}", ast_path.display());
    }
    Ok(store)
}

fn get_str(row: &oxigraph::sparql::QuerySolution, var: &str) -> String {
    row.get(var)
        .map(|t| crud::term_display(t.into()))
        .unwrap_or_default()
}

fn get_type_str(row: &oxigraph::sparql::QuerySolution, var: &str) -> String {
    let raw = row
        .get(var)
        .map(|t| crud::term_display(t.into()))
        .unwrap_or_default();
    kind_label(&raw).to_string()
}

/// The short kind a row prints, from the class the map declares.
///
/// #105: this was a chain of `strip_prefix` calls, which is a PREFIX test
/// doing an EQUALITY job — `Struct` also matches a `Structure`, and on an
/// exact hit it returns `Some("")` and works by accident. Exact match, so a
/// class added later cannot be silently absorbed by a shorter name.
///
/// `Rationale` used to render as `const`. A docstring is not a constant: it is
/// prose, and #563 had already established in-tree that rationale labels are
/// not identifiers. It renders as `note`.
///
/// The import kinds are deliberately three different words. A reader has to be
/// able to tell "this import resolved", "this import is BROKEN" and "this is a
/// third-party package" apart at a glance, because a map that cannot say which
/// one it means is the defect this function is part of.
fn kind_label(raw: &str) -> &'static str {
    match raw {
        "Function" => "fn",
        "Struct" => "struct",
        "Class" => "class",
        "Method" => "method",
        "Module" => "mod",
        "Rationale" => "note",
        "Heading" => "heading",
        "CodeBlock" => "code",
        "Property" => "prop",
        "ExternalModule" => "ext",
        "UnresolvedImport" => "unresolved",
        "UnparsedFile" => "file",
        "Import" => "import",
        "Entity" => "entity",
        _ => "entity",
    }
}

fn query_calls(store: &oxigraph::store::Store, ns: &NamespaceConfig, entity_iri: &str) -> Vec<String> {
    let pfx = ast_prefixes(ns);
    let sparql = format!(
        "{pfx}\n\
         SELECT ?target_label WHERE {{\n\
           {entity_iri} ops:calls ?target .\n\
           ?target rdfs:label ?target_label .\n\
         }}"
    );
    extract_labels(store, &sparql, "target_label")
}

fn query_callers(store: &oxigraph::store::Store, ns: &NamespaceConfig, entity_iri: &str) -> Vec<String> {
    let pfx = ast_prefixes(ns);
    let sparql = format!(
        "{pfx}\n\
         SELECT ?caller_label ?caller_file WHERE {{\n\
           ?caller ops:calls {entity_iri} .\n\
           ?caller rdfs:label ?caller_label .\n\
           OPTIONAL {{ ?caller ops:sourceFile ?caller_file }}\n\
         }}"
    );
    let results = crate::store::query(store, &sparql);
    match results {
        Ok(QueryResults::Solutions(solutions)) => solutions
            .filter_map(|r| r.ok())
            .map(|row| {
                let label = get_str(&row, "caller_label");
                let file = get_str(&row, "caller_file");
                if !file.is_empty() {
                    format!("{file} → {label}")
                } else {
                    label
                }
            })
            .collect(),
        _ => vec![],
    }
}

fn query_file_imports(store: &oxigraph::store::Store, ns: &NamespaceConfig, file_lower: &str) -> Vec<String> {
    let pfx = ast_prefixes(ns);
    let normalized = crud::normalize_path_sep(file_lower);
    let filename =
        crud::escape_sparql_literal(normalized.rsplit('/').next().unwrap_or(&normalized));
    let sparql = format!(
        "{pfx}\n\
         SELECT DISTINCT ?target_label WHERE {{\n\
           ?entity ops:sourceFile ?file ;\n\
             ops:importsFrom ?target .\n\
           ?target rdfs:label ?target_label .\n\
           FILTER({is_file})\n\
         }}",
        is_file = file_match_expr("file", &filename.to_lowercase())
    );
    extract_labels(store, &sparql, "target_label")
}

fn query_file_importers(store: &oxigraph::store::Store, ns: &NamespaceConfig, file_lower: &str) -> Vec<String> {
    let pfx = ast_prefixes(ns);
    let normalized = crud::normalize_path_sep(file_lower);
    let filename =
        crud::escape_sparql_literal(normalized.rsplit('/').next().unwrap_or(&normalized));
    let sparql = format!(
        "{pfx}\n\
         SELECT DISTINCT ?importer_file WHERE {{\n\
           ?importer ops:importsFrom ?target .\n\
           ?target rdfs:label ?target_label .\n\
           ?importer ops:sourceFile ?importer_file .\n\
           FILTER(CONTAINS(LCASE(STR(?target_label)), \"{filename}\"))\n\
         }}"
    );
    extract_labels(store, &sparql, "importer_file")
}

fn extract_labels(store: &oxigraph::store::Store, sparql: &str, var: &str) -> Vec<String> {
    match crate::store::query(store, sparql) {
        Ok(QueryResults::Solutions(solutions)) => solutions
            .filter_map(|r| r.ok())
            .filter_map(|row| {
                row.get(var).map(|t| crud::term_display(t.into()))
            })
            .filter(|s| !s.is_empty())
            .collect(),
        _ => vec![],
    }
}

// ── #107: query an arbitrary relation ────────────────────────────────────────
//
// Before this, `--calls` and `--imports` were the only two relations the CLI
// could reach, and `RELATION_MAP` only mapped 8 of the 27 the extractor emits,
// so class hierarchy was unanswerable twice over: the triples were dropped, and
// there was no way to ask for them if they had not been.
//
// The valid names are read FROM THE MAP, never from a Rust-side list. A second
// hand-kept vocabulary, in a second language, with no contract between them, is
// #107 one layer up — and it would drift the moment `scripts/ast/relations.py`
// gained an entry. The map is the only copy that is always current, and it also
// lets an unknown name answer with the names that ARE present instead of an
// empty result that cannot distinguish "no such relation" from "no such entity".

/// Relation-shaped predicates present in a map: `ops:` predicates whose object
/// is an IRI. Node attributes (`sourceFile`, `sourceLine`, `language`,
/// `signature`) all carry literals, so they fall out without being named.
fn map_relations(store: &oxigraph::store::Store, ns: &NamespaceConfig) -> Vec<String> {
    let pfx = ast_prefixes(ns);
    let ops_ns = &ns.uri;
    let sparql = format!(
        "{pfx}\n\
         SELECT DISTINCT ?p WHERE {{\n\
           ?s ?p ?o .\n\
           FILTER(isIRI(?o))\n\
           FILTER(STRSTARTS(STR(?p), \"{ops_ns}\"))\n\
         }}"
    );
    let mut names: Vec<String> = match crate::store::query(store, &sparql) {
        Ok(QueryResults::Solutions(solutions)) => solutions
            .filter_map(|r| r.ok())
            .filter_map(|row| {
                row.get("p")
                    .map(|t| t.to_string().trim_matches(['<', '>']).to_string())
            })
            .filter_map(|iri| iri.rsplit('#').next().map(str::to_string))
            .collect(),
        _ => vec![],
    };
    names.sort();
    names.dedup();
    names
}

/// `imports_from`, `importsFrom` and `importsfrom` all name the same predicate.
/// The map stores the camelCase spelling; the extractor and the issues use the
/// snake_case one, and a user reading either should not have to know which.
fn relation_key(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '_' && *c != '-')
        .flat_map(char::to_lowercase)
        .collect()
}

/// Query one relation in both directions for a named entity.
pub fn relation(cwd: &Path, ns: &NamespaceConfig, rel: &str, name: &str) -> Result<()> {
    let store = load_ast_store_reporting(cwd)?;
    let pfx = ast_prefixes(ns);

    let present = map_relations(&store, ns);
    let wanted = relation_key(rel);
    let Some(pred) = present.iter().find(|p| relation_key(p) == wanted) else {
        // Honest about which half is missing. An empty result set here would
        // read identically to "the entity has no such edges", which is a
        // different fact and would send the reader looking in the wrong place.
        if present.is_empty() {
            println!("This map carries no relations at all — it may predate the map format.");
        } else {
            println!("No 'ops:{rel}' in this map. Present: {}", present.join(", "));
        }
        return Ok(());
    };

    let name_lower = crud::escape_sparql_literal(&name.to_lowercase());
    let find = format!(
        "{pfx}\n\
         SELECT ?entity ?label ?file ?line WHERE {{\n\
           ?entity rdfs:label ?label .\n\
           OPTIONAL {{ ?entity ops:sourceFile ?file }}\n\
           OPTIONAL {{ ?entity ops:sourceLine ?line }}\n\
           FILTER(CONTAINS(LCASE(STR(?label)), \"{name_lower}\"))\n\
         }}\n\
         ORDER BY ?file ?line"
    );

    let QueryResults::Solutions(solutions) = crate::store::query(&store, &find)? else {
        return Ok(());
    };
    let entities: Vec<(String, String, String, String)> = solutions
        .filter_map(|r| r.ok())
        .map(|row| {
            (
                row.get("entity").map(|t| t.to_string()).unwrap_or_default(),
                get_str(&row, "label"),
                get_str(&row, "file"),
                get_str(&row, "line"),
            )
        })
        .collect();

    if entities.is_empty() {
        println!("No entity named '{name}' found. ops:{pred} IS present in this map.");
        return Ok(());
    }

    for (iri, label, file, line) in &entities {
        let loc = if line.is_empty() {
            file.clone()
        } else {
            format!("{file}:{line}")
        };
        println!("{loc}  {label}");
        let out = query_related(&store, ns, pred, iri, true);
        let inn = query_related(&store, ns, pred, iri, false);
        if out.is_empty() && inn.is_empty() {
            println!("  No ops:{pred} edges.");
        }
        if !out.is_empty() {
            println!("  {pred}:");
            for row in &out {
                println!("    {row}");
            }
        }
        if !inn.is_empty() {
            println!("  {pred}_by:");
            for row in &inn {
                println!("    {row}");
            }
        }
    }
    Ok(())
}

/// One direction of one relation. `outgoing` selects `entity -> ?other` versus
/// `?other -> entity`; both are printed because "what does X inherit" and "what
/// inherits X" are the same question asked from two ends, and `--calls` already
/// answers both.
fn query_related(
    store: &oxigraph::store::Store,
    ns: &NamespaceConfig,
    pred: &str,
    entity_iri: &str,
    outgoing: bool,
) -> Vec<String> {
    let pfx = ast_prefixes(ns);
    let pattern = if outgoing {
        format!("{entity_iri} ops:{pred} ?other .")
    } else {
        format!("?other ops:{pred} {entity_iri} .")
    };
    let sparql = format!(
        "{pfx}\n\
         SELECT ?other_label ?other_file WHERE {{\n\
           {pattern}\n\
           ?other rdfs:label ?other_label .\n\
           OPTIONAL {{ ?other ops:sourceFile ?other_file }}\n\
         }}\n\
         ORDER BY ?other_file ?other_label"
    );
    match crate::store::query(store, &sparql) {
        Ok(QueryResults::Solutions(solutions)) => solutions
            .filter_map(|r| r.ok())
            .map(|row| {
                let label = get_str(&row, "other_label");
                let file = get_str(&row, "other_file");
                if file.is_empty() {
                    label
                } else {
                    format!("{file} → {label}")
                }
            })
            .collect(),
        _ => vec![],
    }
}
