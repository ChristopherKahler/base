use std::path::Path;

use anyhow::Result;

use crate::config::NamespaceConfig;
use crate::crud;

/// Register (idempotent upsert) a pointer to an app's AST map in the workspace
/// graph, so anything that references the app knows a code map exists and where
/// to reach it — without loading the AST itself into `graph.nq`. The actual
/// code triples stay in `ast.ttl`; only this lightweight pointer + metadata
/// lives in the knowledge graph (AUDIT C10: never append turtle into nquads).
pub fn register(cwd: &Path, ns: &NamespaceConfig, app_root: &Path, ttl: &Path) -> Result<()> {
    let app_name = app_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("app");
    let slug = crud::slugify(app_name);
    let iri = crud::build_iri(ns, "codemap", &slug);
    let graph = crud::workspace_graph_iri(ns, &crud::workspace_slug(cwd));
    let now = crud::now_iso();
    let p = &ns.prefix;

    let entity_count = std::fs::read_to_string(ttl)
        .map(|s| s.matches("a ops:").count())
        .unwrap_or(0);
    let ttl_path = crud::escape_sparql_literal(&ttl.display().to_string());
    let name = crud::escape_sparql_literal(app_name);

    // Idempotent: drop any prior pointer for this app, then insert the fresh one.
    let sparql = format!(
        "DELETE WHERE {{ GRAPH <{graph}> {{ <{iri}> ?p ?o }} }};\n\
         INSERT DATA {{\n\
           GRAPH <{graph}> {{\n\
             <{iri}> rdf:type {p}:CodeMap ;\n\
               {p}:name \"{name}\" ;\n\
               {p}:hasCodeMap \"{ttl_path}\" ;\n\
               {p}:astEntityCount {entity_count} ;\n\
               {p}:lastSynced \"{now}\"^^xsd:dateTime .\n\
           }}\n\
         }}"
    );

    crud::load_and_mutate(cwd, ns, &sparql)
}
