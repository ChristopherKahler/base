use std::path::Path;

use anyhow::Result;
use oxigraph::sparql::QueryResults;

use crate::config::NamespaceConfig;
use crate::crud;

/// Query AST entities by label (case-insensitive substring match).
/// Returns: file, line, type, calls, called-by for each match.
pub fn contains(cwd: &Path, ns: &NamespaceConfig, name: &str) -> Result<()> {
    let store = load_ast_store(cwd)?;
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
    let store = load_ast_store(cwd)?;
    let pfx = ast_prefixes(ns);

    // Normalize: accept "src/cli.rs" or "cli.rs" — match by CONTAINS on sourceFile
    let file_lower = file_path
        .trim_start_matches("src/")
        .trim_start_matches("./");

    let sparql = format!(
        "{pfx}\n\
         SELECT ?entity ?label ?line ?type WHERE {{\n\
           ?entity rdf:type ?type ;\n\
             rdfs:label ?label .\n\
           OPTIONAL {{ ?entity ops:sourceLine ?line }}\n\
           ?entity ops:sourceFile ?file .\n\
           FILTER(CONTAINS(LCASE(STR(?file)), \"{}\"))\n\
         }}\n\
         ORDER BY ?line",
        crud::escape_sparql_literal(&file_lower.to_lowercase())
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
    let store = load_ast_store(cwd)?;
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
    let store = load_ast_store(cwd)?;
    let pfx = ast_prefixes(ns);
    let file_lower = file_path
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
    let store = load_ast_store(cwd).ok()?;
    let pfx = ast_prefixes(ns);
    // Hook passes absolute paths; strip to workspace-relative for CONTAINS matching.
    // AST graph stores paths like "src/hook/pre_tool_use.rs".
    let relative = cwd
        .to_str()
        .and_then(|cwd_str| file_path.strip_prefix(cwd_str))
        .map(|p| p.trim_start_matches('/'))
        .unwrap_or(file_path);
    let file_lower = relative
        .trim_start_matches("./")
        .to_lowercase();

    let escaped = crate::crud::escape_sparql_literal(&file_lower);

    let sparql = format!(
        "{pfx}\n\
         SELECT ?label ?line ?type WHERE {{\n\
           ?entity rdf:type ?type ;\n\
             rdfs:label ?label ;\n\
             ops:sourceFile ?file .\n\
           OPTIONAL {{ ?entity ops:sourceLine ?line }}\n\
           FILTER(CONTAINS(LCASE(STR(?file)), \"{escaped}\"))\n\
         }}\n\
         ORDER BY ?line"
    );

    let results = crate::store::query(&store, &sparql).ok()?;
    if let QueryResults::Solutions(solutions) = results {
        let rows: Vec<(String, String, String)> = solutions
            .filter_map(|r| r.ok())
            .map(|row| {
                let label = get_str(&row, "label");
                let line = get_str(&row, "line");
                let etype = get_type_str(&row, "type");
                (label, line, etype)
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
            .map(|(label, line, etype)| {
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
    let file_lower = file_path
        .trim_start_matches("src/")
        .trim_start_matches("./")
        .to_lowercase();
    let filename = file_lower.rsplit('/').next().unwrap_or(&file_lower);
    let end_line = offset + limit;

    let sparql = format!(
        "{pfx}\n\
         SELECT ?entity ?label ?line ?type WHERE {{\n\
           ?entity rdf:type ?type ;\n\
             rdfs:label ?label ;\n\
             ops:sourceFile ?file ;\n\
             ops:sourceLine ?line .\n\
           FILTER(LCASE(STR(?file)) = \"{filename}\")\n\
           FILTER(?line >= {offset} && ?line <= {end_line})\n\
         }}\n\
         ORDER BY ?line"
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

// ─── Internal helpers ────────────────────────────────────────

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

fn load_ast_store(cwd: &Path) -> Result<oxigraph::store::Store> {
    let ast_path = crate::config::find_ast_ttl(cwd)
        .ok_or_else(|| anyhow::anyhow!("No ast.ttl found. Run `base sync --ast` first."))?;
    let store = oxigraph::store::Store::new()?;
    crate::store::load_turtle_into(&store, &ast_path)?;
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
    // Strip namespace prefix: "Function" from "ops:Function" or full IRI
    raw.strip_prefix("Function")
        .map(|_| "fn")
        .or_else(|| raw.strip_prefix("Struct").map(|_| "struct"))
        .or_else(|| raw.strip_prefix("Class").map(|_| "class"))
        .or_else(|| raw.strip_prefix("Method").map(|_| "method"))
        .or_else(|| raw.strip_prefix("Module").map(|_| "mod"))
        .or_else(|| raw.strip_prefix("Rationale").map(|_| "const"))
        .unwrap_or("entity")
        .to_string()
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
    let filename = file_lower.rsplit('/').next().unwrap_or(file_lower);
    let sparql = format!(
        "{pfx}\n\
         SELECT DISTINCT ?target_label WHERE {{\n\
           ?entity ops:sourceFile ?file ;\n\
             ops:importsFrom ?target .\n\
           ?target rdfs:label ?target_label .\n\
           FILTER(LCASE(STR(?file)) = \"{filename}\")\n\
         }}"
    );
    extract_labels(store, &sparql, "target_label")
}

fn query_file_importers(store: &oxigraph::store::Store, ns: &NamespaceConfig, file_lower: &str) -> Vec<String> {
    let pfx = ast_prefixes(ns);
    let filename = crud::escape_sparql_literal(file_lower.rsplit('/').next().unwrap_or(file_lower));
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
