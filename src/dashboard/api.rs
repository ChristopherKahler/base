use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use crate::changelog::Change;

use super::server::AppState;

// ─── Data types ─────────────────────────────────────────────

#[derive(Serialize)]
pub struct GraphNode {
    pub iri: String,
    pub r#type: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_type: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

#[derive(Serialize)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    pub predicate: String,
}

#[derive(Serialize)]
pub struct NodeDetail {
    pub iri: String,
    pub r#type: String,
    pub name: String,
    pub properties: serde_json::Value,
    pub outgoing: Vec<GraphEdge>,
    pub incoming: Vec<GraphEdge>,
    pub notes: Vec<NoteEntry>,
}

#[derive(Serialize, Clone)]
pub struct NoteEntry {
    pub index: u32,
    pub text: String,
    pub created_at: String,
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
}

#[derive(Deserialize)]
pub struct AddNoteBody {
    pub text: String,
}

#[derive(Serialize)]
pub struct OpsProject {
    pub iri: String,
    pub name: String,
    pub status: String,
    pub path: String,
    pub milestones: Vec<OpsMilestone>,
    pub tasks: Vec<OpsTask>,
}

#[derive(Serialize)]
pub struct OpsMilestone {
    pub iri: String,
    pub name: String,
    pub status: String,
}

#[derive(Serialize)]
pub struct OpsTask {
    pub iri: String,
    pub name: String,
    pub status: String,
    pub priority: String,
    pub project: String,
    pub milestone: String,
    pub description: String,
}

#[derive(Serialize)]
pub struct OpsDecision {
    pub iri: String,
    pub name: String,
    pub rationale: String,
    pub domain: String,
    pub created_at: String,
}

#[derive(Serialize)]
pub struct OpsReminder {
    pub iri: String,
    pub name: String,
    pub due: String,
    pub status: String,
    pub overdue: bool,
    pub related_to: Option<String>,
    pub related_name: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateStatusBody {
    pub status: String,
}

// ─── Graph API ──────────────────────────────────────────────

pub async fn nodes(State(state): State<Arc<AppState>>) -> Json<Vec<GraphNode>> {
    let ns = &state.config.namespace;
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let store = state.store_guard();

    let sparql = format!(
        "{pfx}\n\
         SELECT ?s ?type ?name ?status ?path ?docType WHERE {{\n\
           GRAPH ?g {{\n\
             ?s rdf:type ?type .\n\
             OPTIONAL {{ ?s {p}:name ?rawName }}\n\
             OPTIONAL {{ ?s {p}:description ?desc }}\n\
             BIND(COALESCE(?rawName, ?desc) AS ?name)\n\
             OPTIONAL {{ ?s {p}:status ?status }}\n\
             OPTIONAL {{ ?s {p}:path ?path }}\n\
             OPTIONAL {{ ?s {p}:documentType ?docType }}\n\
           }}\n\
         }}"
    );

    let mut nodes = Vec::new();

    if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&sparql) {
        for row in solutions.flatten() {
            let iri = row.get("s").map(|t| term_str(t.into())).unwrap_or_default();
            let type_val = row.get("type").map(|t| term_str(t.into())).unwrap_or_default();
            let name = row.get("name").map(|t| term_str(t.into())).unwrap_or_default();
            let status = row.get("status").map(|t| term_str(t.into()));
            let path = row.get("path").map(|t| term_str(t.into()));
            let document_type = row.get("docType").map(|t| term_str(t.into()));
            let tags = fetch_tags(&store, ns, &iri);

            if !iri.is_empty() {
                nodes.push(GraphNode {
                    iri, r#type: short_type(&type_val), name, status, path, document_type, tags,
                });
            }
        }
    }

    Json(nodes)
}

pub async fn edges(State(state): State<Arc<AppState>>) -> Json<Vec<GraphEdge>> {
    let ns = &state.config.namespace;
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let store = state.store_guard();

    let edge_predicates = [
        "relatedTo", "references", "hasRule", "hasDecision",
        "hasMilestone", "hasTask", "calls", "importsFrom",
        "contains", "hasMethod", "belongsTo", "hasTag",
        "hasSection", "operatorNote",
    ];

    let filter = edge_predicates
        .iter()
        .map(|pred| format!("?p = {p}:{pred}"))
        .collect::<Vec<_>>()
        .join(" || ");

    let sparql = format!(
        "{pfx}\nSELECT ?s ?p ?o WHERE {{ GRAPH ?g {{ ?s ?p ?o . FILTER({filter}) }}\n\
           FILTER(?g != <{ledger}>)\n\
         }}",
        ledger = crate::apply_ops::LEDGER_GRAPH,
    );

    let mut edges = Vec::new();

    if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&sparql) {
        for row in solutions.flatten() {
            let source = row.get("s").map(|t| term_str(t.into())).unwrap_or_default();
            let predicate = row.get("p").map(|t| term_str(t.into())).unwrap_or_default();
            let target = row.get("o").map(|t| term_str(t.into())).unwrap_or_default();

            if !source.is_empty() && !target.is_empty() {
                edges.push(GraphEdge { source, target, predicate: short_predicate(&predicate) });
            }
        }
    }

    Json(edges)
}

pub async fn search(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchQuery>,
) -> Json<Vec<GraphNode>> {
    let Some(q) = params.q.filter(|s| !s.is_empty()) else {
        return Json(vec![]);
    };

    let ns = &state.config.namespace;
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let q_lower = crate::crud::escape_sparql_literal(&q.to_lowercase());
    let store = state.store_guard();

    // Search by name, description, or note text
    let sparql = format!(
        "{pfx}\n\
         SELECT ?s ?type ?name ?status WHERE {{\n\
           {{ GRAPH ?g {{\n\
             ?s rdf:type ?type . ?s {p}:name ?name .\n\
             FILTER(CONTAINS(LCASE(?name), \"{q_lower}\"))\n\
           }} }} UNION {{ GRAPH ?g {{\n\
             ?s rdf:type ?type . ?s {p}:description ?name .\n\
             FILTER(CONTAINS(LCASE(?name), \"{q_lower}\"))\n\
           }} }} UNION {{ GRAPH ?g {{\n\
             ?s rdf:type {p}:OperatorNote . ?s {p}:text ?name .\n\
             BIND({p}:OperatorNote AS ?type)\n\
             FILTER(CONTAINS(LCASE(?name), \"{q_lower}\"))\n\
           }} }}\n\
           OPTIONAL {{ GRAPH ?g {{ ?s {p}:status ?status }} }}\n\
         }}"
    );

    let mut nodes = Vec::new();

    if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&sparql) {
        for row in solutions.flatten() {
            let iri = row.get("s").map(|t| term_str(t.into())).unwrap_or_default();
            let type_val = row.get("type").map(|t| term_str(t.into())).unwrap_or_default();
            let name = row.get("name").map(|t| term_str(t.into())).unwrap_or_default();
            let status = row.get("status").map(|t| term_str(t.into()));

            if !iri.is_empty() {
                nodes.push(GraphNode {
                    iri, r#type: short_type(&type_val), name, status,
                    path: None, document_type: None, tags: vec![],
                });
            }
        }
    }

    Json(nodes)
}

pub async fn node_detail(
    State(state): State<Arc<AppState>>,
    Path(iri): Path<String>,
) -> Json<Option<NodeDetail>> {
    let ns = &state.config.namespace;
    let pfx = crate::crud::prefixes(ns);
    let decoded_iri = urldecode(&iri);
    let store = state.store_guard();

    let props_sparql = format!(
        "{pfx}\nSELECT ?p ?o WHERE {{ GRAPH ?g {{ <{decoded_iri}> ?p ?o }} }}"
    );

    let mut r#type = String::new();
    let mut name = String::new();
    let mut properties = serde_json::Map::new();
    let mut outgoing = Vec::new();

    if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&props_sparql) {
        for row in solutions.flatten() {
            let pred = row.get("p").map(|t| term_str(t.into())).unwrap_or_default();
            let obj = row.get("o").map(|t| term_str(t.into())).unwrap_or_default();

            if pred.contains("rdf:type") || pred.contains("#type") {
                r#type = short_type(&obj);
            } else if pred.contains(":name") {
                name = obj.clone();
                properties.insert("name".into(), serde_json::Value::String(obj));
            } else {
                let short_pred = short_predicate(&pred);
                if is_edge_predicate(&pred, &ns.prefix) {
                    outgoing.push(GraphEdge {
                        source: decoded_iri.clone(), target: obj, predicate: short_pred,
                    });
                } else {
                    properties.insert(short_pred, serde_json::Value::String(obj));
                }
            }
        }
    }

    if r#type.is_empty() {
        return Json(None);
    }

    let incoming_sparql = format!(
        "{pfx}\nSELECT ?s ?p WHERE {{ GRAPH ?g {{ ?s ?p <{decoded_iri}> }} }}"
    );

    let mut incoming = Vec::new();

    if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&incoming_sparql) {
        for row in solutions.flatten() {
            let source = row.get("s").map(|t| term_str(t.into())).unwrap_or_default();
            let pred = row.get("p").map(|t| term_str(t.into())).unwrap_or_default();
            if !source.is_empty() {
                incoming.push(GraphEdge {
                    source, target: decoded_iri.clone(), predicate: short_predicate(&pred),
                });
            }
        }
    }

    let notes = fetch_notes(&store, ns, &decoded_iri);

    Json(Some(NodeDetail {
        iri: decoded_iri, r#type, name,
        properties: serde_json::Value::Object(properties),
        outgoing, incoming, notes,
    }))
}

// ─── OperatorNotes API ──────────────────────────────────────

pub async fn get_notes(
    State(state): State<Arc<AppState>>,
    Path(iri): Path<String>,
) -> Json<Vec<NoteEntry>> {
    let ns = &state.config.namespace;
    let decoded_iri = urldecode(&iri);
    let store = state.store_guard();
    Json(fetch_notes(&store, ns, &decoded_iri))
}

pub async fn add_note(
    State(state): State<Arc<AppState>>,
    Path(iri): Path<String>,
    Json(body): Json<AddNoteBody>,
) -> Result<Json<NoteEntry>, StatusCode> {
    let ns = &state.config.namespace;
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let decoded_iri = urldecode(&iri);

    if body.text.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let store = state.store_guard();

    // Get next index
    let existing = fetch_notes(&store, ns, &decoded_iri);
    let next_index = existing.iter().map(|n| n.index).max().unwrap_or(0) + 1;

    let now = crate::crud::now_iso();
    let ws_slug = crate::crud::workspace_slug(&state.cwd);
    let graph = crate::crud::workspace_graph_iri(ns, &ws_slug);

    let note_slug = format!("note-{}-{next_index}",
        decoded_iri.rsplit('/').next().unwrap_or("unknown"));
    let note_iri = crate::crud::build_iri(ns, "note", &note_slug);
    let escaped_text = body.text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");

    let sparql = format!(
        "{pfx}\nINSERT DATA {{\n\
           GRAPH <{graph}> {{\n\
             <{note_iri}> rdf:type {p}:OperatorNote ;\n\
               {p}:text \"{escaped_text}\" ;\n\
               {p}:index {next_index} ;\n\
               {p}:createdAt \"{now}\"^^xsd:dateTime .\n\
             <{decoded_iri}> {p}:operatorNote <{note_iri}> .\n\
           }}\n\
         }}"
    );

    if store.update(&sparql).is_err() {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    // Persist to disk
    crate::store::write_back(&store, &state.trig_path, Change::Sparql(&sparql))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(NoteEntry {
        index: next_index,
        text: body.text,
        created_at: now,
    }))
}

pub async fn update_note(
    State(state): State<Arc<AppState>>,
    Path((iri, index)): Path<(String, u32)>,
    Json(body): Json<AddNoteBody>,
) -> Result<Json<NoteEntry>, StatusCode> {
    let ns = &state.config.namespace;
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let decoded_iri = urldecode(&iri);
    let store = state.store_guard();

    if body.text.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Find the note IRI by matching parent + index
    let find = format!(
        "{pfx}\nSELECT ?note WHERE {{\n\
           GRAPH ?g {{ <{decoded_iri}> {p}:operatorNote ?note . ?note {p}:index {index} }}\n\
         }}"
    );

    let note_iri = if let Ok(oxigraph::sparql::QueryResults::Solutions(mut sol)) = store.query(&find) {
        sol.find_map(|r| r.ok().and_then(|row| row.get("note").map(|t| term_str(t.into()))))
    } else {
        None
    };

    let Some(note_iri) = note_iri else {
        return Err(StatusCode::NOT_FOUND);
    };

    let ws_slug = crate::crud::workspace_slug(&state.cwd);
    let graph = crate::crud::workspace_graph_iri(ns, &ws_slug);
    let escaped = body.text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");

    let sparql = format!(
        "{pfx}\n\
         DELETE {{ GRAPH <{graph}> {{ <{note_iri}> {p}:text ?old }} }}\n\
         INSERT {{ GRAPH <{graph}> {{ <{note_iri}> {p}:text \"{escaped}\" }} }}\n\
         WHERE {{ GRAPH <{graph}> {{ <{note_iri}> {p}:text ?old }} }}"
    );

    if store.update(&sparql).is_err() {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    crate::store::write_back(&store, &state.trig_path, Change::Sparql(&sparql))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Get created_at for response
    let created = {
        let q = format!("{pfx}\nSELECT ?c WHERE {{ GRAPH ?g {{ <{note_iri}> {p}:createdAt ?c }} }}");
        if let Ok(oxigraph::sparql::QueryResults::Solutions(mut s)) = store.query(&q) {
            s.find_map(|r| r.ok().and_then(|row| row.get("c").map(|t| term_str(t.into()))))
                .unwrap_or_default()
        } else { String::new() }
    };

    Ok(Json(NoteEntry { index, text: body.text, created_at: created }))
}

pub async fn delete_note(
    State(state): State<Arc<AppState>>,
    Path((iri, index)): Path<(String, u32)>,
) -> StatusCode {
    let ns = &state.config.namespace;
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let decoded_iri = urldecode(&iri);
    let store = state.store_guard();

    // Find the note IRI
    let find = format!(
        "{pfx}\nSELECT ?note WHERE {{\n\
           GRAPH ?g {{ <{decoded_iri}> {p}:operatorNote ?note . ?note {p}:index {index} }}\n\
         }}"
    );

    let note_iri = if let Ok(oxigraph::sparql::QueryResults::Solutions(mut sol)) = store.query(&find) {
        sol.find_map(|r| r.ok().and_then(|row| row.get("note").map(|t| term_str(t.into()))))
    } else {
        None
    };

    let Some(note_iri) = note_iri else {
        return StatusCode::NOT_FOUND;
    };

    let ws_slug = crate::crud::workspace_slug(&state.cwd);
    let graph = crate::crud::workspace_graph_iri(ns, &ws_slug);

    // Delete all triples for this note + the edge from parent
    let sparql = format!(
        "{pfx}\n\
         DELETE WHERE {{ GRAPH <{graph}> {{ <{note_iri}> ?p ?o }} }};\n\
         DELETE WHERE {{ GRAPH <{graph}> {{ <{decoded_iri}> {p}:operatorNote <{note_iri}> }} }}"
    );

    if store.update(&sparql).is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR;
    }

    if crate::store::write_back(&store, &state.trig_path, Change::Sparql(&sparql)).is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    StatusCode::NO_CONTENT
}

fn fetch_notes(store: &oxigraph::store::Store, ns: &crate::config::NamespaceConfig, iri: &str) -> Vec<NoteEntry> {
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let sparql = format!(
        "{pfx}\n\
         SELECT ?text ?index ?created WHERE {{\n\
           GRAPH ?g {{\n\
             <{iri}> {p}:operatorNote ?note .\n\
             ?note {p}:text ?text ;\n\
               {p}:index ?index .\n\
             OPTIONAL {{ ?note {p}:createdAt ?created }}\n\
           }}\n\
         }} ORDER BY ?index"
    );

    let mut notes = Vec::new();
    if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&sparql) {
        for row in solutions.flatten() {
            let text = row.get("text").map(|t| term_str(t.into())).unwrap_or_default();
            let index: u32 = row.get("index").map(|t| term_str(t.into())).unwrap_or_default()
                .parse().unwrap_or(0);
            let created_at = row.get("created").map(|t| term_str(t.into())).unwrap_or_default();
            notes.push(NoteEntry { index, text, created_at });
        }
    }
    notes
}

// ─── Ops API ────────────────────────────────────────────────

pub async fn ops_projects(State(state): State<Arc<AppState>>) -> Json<Vec<OpsProject>> {
    let ns = &state.config.namespace;
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let store = state.store_guard();

    // Get projects
    let proj_sparql = format!(
        "{pfx}\nSELECT ?s ?name ?status ?path WHERE {{\n\
           GRAPH ?g {{ ?s rdf:type {p}:Project . ?s {p}:name ?name .\n\
             OPTIONAL {{ ?s {p}:status ?status }}\n\
             OPTIONAL {{ ?s {p}:path ?path }}\n\
           }}\n\
         }}"
    );

    let mut projects = Vec::new();

    if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&proj_sparql) {
        for row in solutions.flatten() {
            let iri = row.get("s").map(|t| term_str(t.into())).unwrap_or_default();
            let name = row.get("name").map(|t| term_str(t.into())).unwrap_or_default();
            let status = row.get("status").map(|t| term_str(t.into())).unwrap_or_else(|| "active".into());
            let path = row.get("path").map(|t| term_str(t.into())).unwrap_or_default();

            // Get milestones for this project
            let ms_sparql = format!(
                "{pfx}\nSELECT ?m ?mname ?mstatus WHERE {{\n\
                   GRAPH ?g {{ <{iri}> {p}:hasMilestone ?m . ?m {p}:name ?mname .\n\
                     OPTIONAL {{ ?m {p}:status ?mstatus }}\n\
                   }}\n\
                 }}"
            );
            let mut milestones = Vec::new();
            if let Ok(oxigraph::sparql::QueryResults::Solutions(ms_sol)) = store.query(&ms_sparql) {
                for mrow in ms_sol.flatten() {
                    milestones.push(OpsMilestone {
                        iri: mrow.get("m").map(|t| term_str(t.into())).unwrap_or_default(),
                        name: mrow.get("mname").map(|t| term_str(t.into())).unwrap_or_default(),
                        status: mrow.get("mstatus").map(|t| term_str(t.into())).unwrap_or_else(|| "active".into()),
                    });
                }
            }

            // Get tasks for this project
            let task_sparql = format!(
                "{pfx}\nSELECT ?t ?tname ?tstatus ?tpri ?tdesc WHERE {{\n\
                   GRAPH ?g {{ <{iri}> {p}:hasTask ?t . ?t {p}:name ?tname .\n\
                     OPTIONAL {{ ?t {p}:status ?tstatus }}\n\
                     OPTIONAL {{ ?t {p}:priority ?tpri }}\n\
                     OPTIONAL {{ ?t {p}:description ?tdesc }}\n\
                   }}\n\
                 }}"
            );
            let mut tasks = Vec::new();
            if let Ok(oxigraph::sparql::QueryResults::Solutions(t_sol)) = store.query(&task_sparql) {
                for trow in t_sol.flatten() {
                    tasks.push(OpsTask {
                        iri: trow.get("t").map(|t| term_str(t.into())).unwrap_or_default(),
                        name: trow.get("tname").map(|t| term_str(t.into())).unwrap_or_default(),
                        status: trow.get("tstatus").map(|t| term_str(t.into())).unwrap_or_else(|| "active".into()),
                        priority: trow.get("tpri").map(|t| term_str(t.into())).unwrap_or_else(|| "normal".into()),
                        project: name.clone(),
                        milestone: String::new(),
                        description: trow.get("tdesc").map(|t| term_str(t.into())).unwrap_or_default(),
                    });
                }
            }

            projects.push(OpsProject { iri, name, status, path, milestones, tasks });
        }
    }

    Json(projects)
}

pub async fn ops_decisions(State(state): State<Arc<AppState>>) -> Json<Vec<OpsDecision>> {
    let ns = &state.config.namespace;
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let store = state.store_guard();

    let sparql = format!(
        "{pfx}\nSELECT ?d ?name ?rationale ?created ?dname WHERE {{\n\
           GRAPH ?g {{ ?d rdf:type {p}:Decision . ?d {p}:name ?name .\n\
             OPTIONAL {{ ?d {p}:rationale ?rationale }}\n\
             OPTIONAL {{ ?d {p}:createdAt ?created }}\n\
             OPTIONAL {{ ?d {p}:hasDomain ?dom . ?dom {p}:name ?dname }}\n\
           }}\n\
         }} ORDER BY DESC(?created)"
    );

    let mut decisions = Vec::new();

    if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&sparql) {
        for row in solutions.flatten() {
            decisions.push(OpsDecision {
                iri: row.get("d").map(|t| term_str(t.into())).unwrap_or_default(),
                name: row.get("name").map(|t| term_str(t.into())).unwrap_or_default(),
                rationale: row.get("rationale").map(|t| term_str(t.into())).unwrap_or_default(),
                domain: row.get("dname").map(|t| term_str(t.into())).unwrap_or_default(),
                created_at: row.get("created").map(|t| term_str(t.into())).unwrap_or_default(),
            });
        }
    }

    Json(decisions)
}

pub async fn ops_reminders(State(state): State<Arc<AppState>>) -> Json<Vec<OpsReminder>> {
    let ns = &state.config.namespace;
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let store = state.store_guard();

    let sparql = format!(
        "{pfx}\nSELECT ?r ?name ?due ?rstatus ?relIri ?relName WHERE {{\n\
           GRAPH ?g {{ ?r rdf:type {p}:Reminder . ?r {p}:name ?name .\n\
             OPTIONAL {{ ?r {p}:due ?due }}\n\
             OPTIONAL {{ ?r {p}:status ?rstatus }}\n\
             OPTIONAL {{ ?r {p}:relatedTo ?relIri . ?relIri {p}:name ?relName }}\n\
           }}\n\
         }} ORDER BY ?due"
    );

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let mut reminders = Vec::new();

    if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&sparql) {
        for row in solutions.flatten() {
            let due = row.get("due").map(|t| term_str(t.into())).unwrap_or_default();
            let status = row.get("rstatus").map(|t| term_str(t.into())).unwrap_or_else(|| "active".into());
            let overdue = status != "completed" && !due.is_empty() && due < today;
            reminders.push(OpsReminder {
                iri: row.get("r").map(|t| term_str(t.into())).unwrap_or_default(),
                name: row.get("name").map(|t| term_str(t.into())).unwrap_or_default(),
                due, status, overdue,
                related_to: row.get("relIri").map(|t| term_str(t.into())),
                related_name: row.get("relName").map(|t| term_str(t.into())),
            });
        }
    }

    Json(reminders)
}

// ─── Decision CRUD ──────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateDecisionBody {
    pub name: String,
    pub rationale: Option<String>,
    pub domain: Option<String>,
}

pub async fn create_decision(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateDecisionBody>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ns = &state.config.namespace;
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let slug = crate::crud::slugify(&body.name);
    let iri = crate::crud::build_iri(ns, "decision", &slug);
    let graph = crate::crud::workspace_graph_iri(ns, &crate::crud::workspace_slug(&state.cwd));
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let rationale = body.rationale.as_deref().unwrap_or("");

    let mut domain_triple = String::new();
    if let Some(ref domain) = body.domain
        && !domain.is_empty() {
            let domain_slug = crate::crud::slugify(domain);
            let domain_iri = crate::crud::build_iri(ns, "domain", &domain_slug);
            domain_triple = format!("<{iri}> {p}:hasDomain <{domain_iri}> .\n");
        }

    let store = state.store_guard();
    let insert = format!(
        "{pfx}\nINSERT DATA {{\n  GRAPH <{graph}> {{\n\
           <{iri}> rdf:type {p}:Decision .\n\
           <{iri}> {p}:name \"{}\" .\n\
           <{iri}> {p}:rationale \"{}\" .\n\
           <{iri}> {p}:createdAt \"{today}\" .\n\
           {domain_triple}\
         }}\n}}",
        body.name.replace('"', "\\\""),
        rationale.replace('"', "\\\"").replace('\n', "\\n"),
    );
    store.update(&insert).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    crate::store::write_back(&store, &state.trig_path, Change::Sparql(&insert))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({ "iri": iri, "name": body.name })))
}

#[derive(Deserialize)]
pub struct UpdateDecisionBody {
    pub name: Option<String>,
    pub rationale: Option<String>,
    pub domain: Option<String>,
}

pub async fn update_decision(
    Path(iri): Path<String>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<UpdateDecisionBody>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let iri = urldecode(&iri);
    let ns = &state.config.namespace;
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let store = state.store_guard();

    // Fields are optional, so the delta is whichever updates actually ran.
    let mut applied: Vec<String> = Vec::new();

    if let Some(ref name) = body.name {
        let u = format!(
            "{pfx}\nDELETE {{ GRAPH ?g {{ <{iri}> {p}:name ?old }} }}\n\
             INSERT {{ GRAPH ?g {{ <{iri}> {p}:name \"{}\" }} }}\n\
             WHERE  {{ GRAPH ?g {{ <{iri}> {p}:name ?old }} }}",
            name.replace('"', "\\\"")
        );
        store.update(&u).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        applied.push(u);
    }
    if let Some(ref rationale) = body.rationale {
        let escaped = rationale.replace('"', "\\\"").replace('\n', "\\n");
        let del = format!(
            "{pfx}\nDELETE {{ GRAPH ?g {{ <{iri}> {p}:rationale ?old }} }}\n\
             WHERE  {{ GRAPH ?g {{ <{iri}> {p}:rationale ?old }} }}"
        );
        let _ = store.update(&del);
        applied.push(del);
        if !rationale.is_empty() {
            let graph = crate::crud::workspace_graph_iri(ns, &crate::crud::workspace_slug(&state.cwd));
            let ins = format!(
                "{pfx}\nINSERT DATA {{ GRAPH <{graph}> {{ <{iri}> {p}:rationale \"{escaped}\" }} }}"
            );
            store.update(&ins).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            applied.push(ins);
        }
    }
    if let Some(ref domain) = body.domain {
        // Remove old domain edge
        let del = format!(
            "{pfx}\nDELETE {{ GRAPH ?g {{ <{iri}> {p}:hasDomain ?old }} }}\n\
             WHERE  {{ GRAPH ?g {{ <{iri}> {p}:hasDomain ?old }} }}"
        );
        let _ = store.update(&del);
        applied.push(del);
        if !domain.is_empty() {
            let domain_slug = crate::crud::slugify(domain);
            let domain_iri = crate::crud::build_iri(ns, "domain", &domain_slug);
            let graph = crate::crud::workspace_graph_iri(ns, &crate::crud::workspace_slug(&state.cwd));
            let ins = format!(
                "{pfx}\nINSERT DATA {{ GRAPH <{graph}> {{ <{iri}> {p}:hasDomain <{domain_iri}> }} }}"
            );
            store.update(&ins).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            applied.push(ins);
        }
    }

    crate::store::write_back(&store, &state.trig_path, Change::Sparql(&applied.join(";\n")))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({ "iri": iri, "updated": true })))
}

pub async fn delete_decision(
    Path(iri): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let iri = urldecode(&iri);
    let pfx = crate::crud::prefixes(&state.config.namespace);
    let store = state.store_guard();

    let del = format!(
        "{pfx}\nDELETE {{ GRAPH ?g {{ <{iri}> ?p ?o }} }}\n\
         WHERE  {{ GRAPH ?g {{ <{iri}> ?p ?o }} }}"
    );
    store.update(&del).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let del2 = format!(
        "{pfx}\nDELETE {{ GRAPH ?g {{ ?s ?p <{iri}> }} }}\n\
         WHERE  {{ GRAPH ?g {{ ?s ?p <{iri}> }} }}"
    );
    store.update(&del2).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    crate::store::write_back(&store, &state.trig_path, Change::Sparql(&format!("{del};\n{del2}")))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({ "deleted": true, "iri": iri })))
}

// ─── Reminder CRUD ──────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateReminderBody {
    pub name: String,
    pub due: Option<String>,
    pub related_to: Option<String>,
}

pub async fn create_reminder(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateReminderBody>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ns = &state.config.namespace;
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let slug = crate::crud::slugify(&body.name);
    let iri = crate::crud::build_iri(ns, "reminder", &slug);
    let graph = crate::crud::workspace_graph_iri(ns, &crate::crud::workspace_slug(&state.cwd));

    let mut extra_triples = String::new();
    if let Some(ref due) = body.due
        && !due.is_empty() {
            extra_triples.push_str(&format!("<{iri}> {p}:due \"{due}\" .\n"));
        }
    if let Some(ref related) = body.related_to
        && !related.is_empty() {
            let related_slug = crate::crud::slugify(related);
            let related_iri = crate::crud::build_iri(ns, "project", &related_slug);
            extra_triples.push_str(&format!("<{iri}> {p}:relatedTo <{related_iri}> .\n"));
        }

    let store = state.store_guard();
    let insert = format!(
        "{pfx}\nINSERT DATA {{\n  GRAPH <{graph}> {{\n\
           <{iri}> rdf:type {p}:Reminder .\n\
           <{iri}> {p}:name \"{}\" .\n\
           <{iri}> {p}:status \"active\" .\n\
           {extra_triples}\
         }}\n}}",
        body.name.replace('"', "\\\""),
    );
    store.update(&insert).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    crate::store::write_back(&store, &state.trig_path, Change::Sparql(&insert))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({ "iri": iri, "name": body.name })))
}

pub async fn complete_reminder(
    Path(iri): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let iri = urldecode(&iri);
    let ns = &state.config.namespace;
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let store = state.store_guard();

    // Delete old status, insert completed
    let del = format!(
        "{pfx}\nDELETE {{ GRAPH ?g {{ <{iri}> {p}:status ?old }} }}\n\
         WHERE  {{ GRAPH ?g {{ <{iri}> {p}:status ?old }} }}"
    );
    let _ = store.update(&del);
    let graph = crate::crud::workspace_graph_iri(ns, &crate::crud::workspace_slug(&state.cwd));
    let ins = format!(
        "{pfx}\nINSERT DATA {{ GRAPH <{graph}> {{ <{iri}> {p}:status \"completed\" }} }}"
    );
    store.update(&ins).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    crate::store::write_back(&store, &state.trig_path, Change::Sparql(&format!("{del};\n{ins}")))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({ "iri": iri, "status": "completed" })))
}

pub async fn delete_reminder(
    Path(iri): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let iri = urldecode(&iri);
    let pfx = crate::crud::prefixes(&state.config.namespace);
    let store = state.store_guard();

    let del = format!(
        "{pfx}\nDELETE {{ GRAPH ?g {{ <{iri}> ?p ?o }} }}\n\
         WHERE  {{ GRAPH ?g {{ <{iri}> ?p ?o }} }}"
    );
    store.update(&del).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let del2 = format!(
        "{pfx}\nDELETE {{ GRAPH ?g {{ ?s ?p <{iri}> }} }}\n\
         WHERE  {{ GRAPH ?g {{ ?s ?p <{iri}> }} }}"
    );
    store.update(&del2).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    crate::store::write_back(&store, &state.trig_path, Change::Sparql(&format!("{del};\n{del2}")))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({ "deleted": true, "iri": iri })))
}

// ─── Project Status Update ──────────────────────────────────

pub async fn update_project_status(
    Path(iri): Path<String>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<UpdateStatusBody>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let iri = urldecode(&iri);
    let ns = &state.config.namespace;
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);

    let valid = ["active", "blocked", "completed", "pending", "deferred"];
    if !valid.contains(&body.status.as_str()) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let store = state.store_guard();

    let check = format!("{pfx}\nASK {{ GRAPH ?g {{ <{iri}> rdf:type ?t }} }}");
    match store.query(&check) {
        Ok(oxigraph::sparql::QueryResults::Boolean(true)) => {}
        _ => return Err(StatusCode::NOT_FOUND),
    }

    let update = format!(
        "{pfx}\n\
         DELETE {{ GRAPH ?g {{ <{iri}> {p}:status ?old }} }}\n\
         INSERT {{ GRAPH ?g {{ <{iri}> {p}:status \"{}\" }} }}\n\
         WHERE  {{ GRAPH ?g {{ <{iri}> {p}:status ?old }} }}",
        body.status
    );
    store.update(&update).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    crate::store::write_back(&store, &state.trig_path, Change::Sparql(&update))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({ "iri": iri, "status": body.status })))
}

// ─── Ledger API (cost attribution) ──────────────────────────

#[derive(Serialize)]
pub struct OpsLedgerEntry {
    pub iri: String,
    pub action: String,
    pub phase: String,
    pub plan: String,
    pub timestamp: String,
    pub note: String,
    pub project: String,
    pub session_tokens: Option<u64>,
    pub session_cost: Option<f64>,
}

/// TOML structs for direct ledger.toml parsing — supports both formats
#[derive(serde::Deserialize)]
struct LedgerToml {
    entry: Option<Vec<LedgerTomlEntry>>,
    session: Option<Vec<LedgerTomlSession>>,
}

/// Format 1: [[entry]] — base-v2 style (action/phase/plan/at/note)
#[derive(serde::Deserialize)]
struct LedgerTomlEntry {
    action: String,
    phase: Option<u32>,
    plan: Option<String>,
    at: String,
    note: Option<String>,
}

/// Format 2: [[session]] — seed/toolbox style (date/phase/plan/action/duration_min/outcome)
#[derive(serde::Deserialize)]
struct LedgerTomlSession {
    action: Option<String>,
    phase: Option<String>,
    plan: Option<String>,
    date: Option<String>,
    duration_min: Option<u32>,
    outcome: Option<String>,
}

#[derive(serde::Deserialize)]
struct PaulToml {
    name: Option<String>,
}

/// Collect all workspace roots — current workspace + all registered in ~/.base-gbl/base.toml
fn collect_workspace_roots(primary: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut roots = vec![primary.to_path_buf()];

    if let Some(home) = crate::home::home_root() {
        let base_toml = home.join(".base-gbl/base.toml");
        if let Ok(content) = std::fs::read_to_string(&base_toml)
            && let Ok(table) = content.parse::<toml::Table>()
                && let Some(workspaces) = table.get("workspace").and_then(|v| v.as_array()) {
                    for ws in workspaces {
                        if let Some(path_str) = ws.get("path").and_then(|v| v.as_str()) {
                            let p = std::path::PathBuf::from(path_str);
                            if p.exists() && !roots.contains(&p) {
                                roots.push(p);
                            }
                        }
                    }
                }
    }

    roots
}

/// Recursively find files matching a relative path pattern under a root directory.
/// Skips node_modules, .git, target, dist, and hidden dirs (except .paul).
fn find_files_recursive(dir: &std::path::Path, target: &str, max_depth: u32, results: &mut Vec<std::path::PathBuf>) {
    if max_depth == 0 { return; }
    let candidate = dir.join(target);
    if candidate.exists() {
        results.push(candidate);
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if !entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) { continue; }
            let name = entry.file_name();
            let name_str = name.to_str().unwrap_or("");
            // Skip heavy/irrelevant dirs
            if matches!(name_str, "node_modules" | ".git" | "target" | "dist" | ".next" | "__pycache__") { continue; }
            // Skip hidden dirs except .paul
            if name_str.starts_with('.') && name_str != ".paul" { continue; }
            find_files_recursive(&entry.path(), target, max_depth - 1, results);
        }
    }
}

/// Scan for all .paul/ledger.toml files across all registered workspaces.
fn scan_ledger_files(cwd: &std::path::Path) -> Vec<OpsLedgerEntry> {
    let mut entries = Vec::new();

    let workspace_roots = collect_workspace_roots(cwd);

    // Recursively find ALL .paul/ledger.toml under every registered workspace
    let mut ledger_paths: Vec<std::path::PathBuf> = Vec::new();
    for root in &workspace_roots {
        find_files_recursive(root, ".paul/ledger.toml", 6, &mut ledger_paths);
    }

    for ledger_path in &ledger_paths {
        let ledger_path = ledger_path.clone();
        if !ledger_path.exists() { continue; }

        let content = match std::fs::read_to_string(&ledger_path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let ledger: LedgerToml = match toml::from_str(&content) {
            Ok(l) => l,
            Err(_) => continue,
        };

        // Read project name from paul.toml (sibling of ledger.toml)
        let paul_dir = ledger_path.parent().unwrap_or(std::path::Path::new("."));
        let project_dir = paul_dir.parent().unwrap_or(std::path::Path::new("."));
        let project_name = std::fs::read_to_string(paul_dir.join("paul.toml"))
            .ok()
            .and_then(|c| toml::from_str::<PaulToml>(&c).ok())
            .and_then(|p| p.name)
            .unwrap_or_else(|| project_dir.file_name().and_then(|n| n.to_str()).unwrap_or("unknown").to_string());

        // Format 1: [[entry]]
        if let Some(raw_entries) = ledger.entry {
            for e in raw_entries {
                entries.push(OpsLedgerEntry {
                    iri: String::new(),
                    action: e.action,
                    phase: e.phase.map(|p| p.to_string()).unwrap_or_default(),
                    plan: e.plan.unwrap_or_default(),
                    timestamp: e.at,
                    note: e.note.unwrap_or_default(),
                    project: project_name.clone(),
                    session_tokens: None,
                    session_cost: None,
                });
            }
        }

        // Format 2: [[session]]
        if let Some(sessions) = ledger.session {
            for s in sessions {
                entries.push(OpsLedgerEntry {
                    iri: String::new(),
                    action: s.action.unwrap_or_else(|| "session".into()),
                    phase: s.phase.unwrap_or_default(),
                    plan: s.plan.unwrap_or_default(),
                    timestamp: s.date.unwrap_or_default(),
                    note: s.outcome.unwrap_or_default(),
                    project: project_name.clone(),
                    session_tokens: None,
                    session_cost: s.duration_min.map(|d| d as f64),
                });
            }
        }
    }

    // Sort by timestamp descending
    entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    entries
}

pub async fn ops_ledger(
    State(_state): State<Arc<AppState>>,
) -> Json<Vec<OpsLedgerEntry>> {
    // Read ledger.toml directly from filesystem — no graph sync needed
    let cwd = std::env::current_dir().unwrap_or_default();
    let workspace_dir = crate::config::find_workspace_base(&cwd)
        .and_then(|b| b.parent().map(|p| p.to_path_buf()))
        .unwrap_or(cwd);

    let mut entries = scan_ledger_files(&workspace_dir);

    // Join with session JSONL data via timestamp matching
    let events = collect_all_events(365);
    if !events.is_empty() {
        // Group events by session (cwd acts as session grouping key)
        // Each event has a timestamp — find sessions whose time range contains the ledger entry
        for entry in &mut entries {
            if let Ok(entry_dt) = chrono::DateTime::parse_from_rfc3339(&entry.timestamp)
                .or_else(|_| chrono::DateTime::parse_from_str(&entry.timestamp, "%Y-%m-%dT%H:%M:%S%:z"))
            {
                let entry_ts = entry_dt.timestamp();
                // Find events within ±30 minutes of this ledger entry
                let window = 30 * 60; // 30 minutes
                let mut matched_tokens: u64 = 0;
                let mut matched_cost: f64 = 0.0;
                let mut found = false;

                for ev in &events {
                    if let Ok(ev_dt) = chrono::DateTime::parse_from_rfc3339(&ev.timestamp)
                        .or_else(|_| chrono::DateTime::parse_from_str(&ev.timestamp, "%Y-%m-%dT%H:%M:%S%:z"))
                    {
                        let diff = (ev_dt.timestamp() - entry_ts).abs();
                        if diff <= window {
                            matched_tokens += ev.input_tokens + ev.output_tokens;
                            matched_cost += ev.cost;
                            found = true;
                        }
                    }
                }

                if found {
                    entry.session_tokens = Some(matched_tokens);
                    entry.session_cost = Some((matched_cost * 100.0).round() / 100.0);
                }
            }
        }
    }

    Json(entries)
}

#[derive(Serialize)]
pub struct PhaseActionCost {
    pub action: String,
    pub cost: f64,
    pub count: u32,
}

#[derive(Serialize)]
pub struct PhaseCost {
    pub phase: String,
    pub total_cost: f64,
    pub session_count: u32,
    pub actions: Vec<PhaseActionCost>,
}

#[derive(Serialize)]
pub struct CostSummary {
    pub project: String,
    pub total_cost: f64,
    pub total_entries: u32,
    pub phases: Vec<PhaseCost>,
}

#[derive(Deserialize)]
pub struct CostQuery {
    pub project: Option<String>,
}

pub async fn ops_cost_summary(
    Query(query): Query<CostQuery>,
    State(state): State<Arc<AppState>>,
) -> Json<CostSummary> {
    // Get ledger entries (reuse the handler logic)
    let ledger = ops_ledger(State(state)).await.0;

    let project_filter = query.project.unwrap_or_default();
    let filtered: Vec<&OpsLedgerEntry> = if project_filter.is_empty() {
        ledger.iter().collect()
    } else {
        ledger.iter().filter(|e| e.project == project_filter).collect()
    };

    // Group by phase
    let mut phase_map: std::collections::BTreeMap<String, Vec<&OpsLedgerEntry>> = std::collections::BTreeMap::new();
    for entry in &filtered {
        let key = if entry.phase.is_empty() { "unknown".to_string() } else { entry.phase.clone() };
        phase_map.entry(key).or_default().push(entry);
    }

    let mut phases = Vec::new();
    let mut total_cost = 0.0;

    for (phase, entries) in &phase_map {
        let mut action_map: std::collections::BTreeMap<String, (f64, u32)> = std::collections::BTreeMap::new();
        let mut phase_cost = 0.0;

        for e in entries {
            let cost = e.session_cost.unwrap_or(0.0);
            phase_cost += cost;
            let entry = action_map.entry(e.action.clone()).or_insert((0.0, 0));
            entry.0 += cost;
            entry.1 += 1;
        }

        total_cost += phase_cost;
        phases.push(PhaseCost {
            phase: phase.clone(),
            total_cost: (phase_cost * 100.0).round() / 100.0,
            session_count: entries.len() as u32,
            actions: action_map.into_iter().map(|(action, (cost, count))| PhaseActionCost {
                action,
                cost: (cost * 100.0).round() / 100.0,
                count,
            }).collect(),
        });
    }

    Json(CostSummary {
        project: project_filter,
        total_cost: (total_cost * 100.0).round() / 100.0,
        total_entries: filtered.len() as u32,
        phases,
    })
}

// ─── Task status update ─────────────────────────────────────

pub async fn update_task_status(
    Path(iri): Path<String>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<UpdateStatusBody>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let iri = urldecode(&iri);
    let ns = &state.config.namespace;
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);

    let valid = ["active", "in_progress", "blocked", "completed", "pending"];
    if !valid.contains(&body.status.as_str()) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let store = state.store_guard();

    // Verify entity exists
    let check = format!(
        "{pfx}\nASK {{ GRAPH ?g {{ <{iri}> rdf:type ?t }} }}"
    );
    match store.query(&check) {
        Ok(oxigraph::sparql::QueryResults::Boolean(true)) => {}
        _ => return Err(StatusCode::NOT_FOUND),
    }

    // Delete old status, insert new
    let update = format!(
        "{pfx}\n\
         DELETE {{ GRAPH ?g {{ <{iri}> {p}:status ?old }} }}\n\
         INSERT {{ GRAPH ?g {{ <{iri}> {p}:status \"{}\" }} }}\n\
         WHERE  {{ GRAPH ?g {{ <{iri}> {p}:status ?old }} }}",
        body.status
    );
    store.update(&update).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Write back to disk
    crate::store::write_back(&store, &state.trig_path, Change::Sparql(&update))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({
        "iri": iri,
        "status": body.status,
    })))
}

// ─── Helpers ────────────────────────────────────────────────

fn fetch_tags(store: &oxigraph::store::Store, ns: &crate::config::NamespaceConfig, iri: &str) -> Vec<String> {
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let sparql = format!(
        "{pfx}\nSELECT ?tag WHERE {{ GRAPH ?g {{ <{iri}> {p}:hasTag ?tag }} }}"
    );
    let mut tags = Vec::new();
    if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&sparql) {
        for row in solutions.flatten() {
            if let Some(t) = row.get("tag") {
                let val = term_str(t.into());
                if !val.is_empty() { tags.push(val); }
            }
        }
    }
    tags
}

fn term_str(term: oxigraph::model::TermRef<'_>) -> String {
    match term {
        oxigraph::model::TermRef::NamedNode(n) => n.as_str().to_string(),
        oxigraph::model::TermRef::Literal(l) => l.value().to_string(),
        oxigraph::model::TermRef::BlankNode(b) => b.as_str().to_string(),
        _ => String::new(),
    }
}

fn short_type(full: &str) -> String {
    full.rsplit_once('#').or_else(|| full.rsplit_once(':')).or_else(|| full.rsplit_once('/'))
        .map(|(_, short)| short.to_string()).unwrap_or_else(|| full.to_string())
}

fn short_predicate(full: &str) -> String { short_type(full) }

fn is_edge_predicate(pred: &str, prefix: &str) -> bool {
    let edge_preds = [
        "relatedTo", "references", "hasRule", "hasDecision",
        "hasMilestone", "hasTask", "calls", "importsFrom",
        "contains", "hasMethod", "belongsTo", "operatorNote",
        "hasFileChange", "hasACResult", "hasAC", "dependsOn", "affects",
    ];
    edge_preds.iter().any(|ep| pred.contains(&format!("{prefix}:{ep}")) || pred.ends_with(&format!("#{ep}")))
}

fn urldecode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let hi = chars.next().unwrap_or(b'0');
            let lo = chars.next().unwrap_or(b'0');
            let hex = [hi, lo];
            if let Ok(s) = std::str::from_utf8(&hex)
                && let Ok(val) = u8::from_str_radix(s, 16) {
                    result.push(val as char);
                    continue;
                }
            result.push('%');
            result.push(hi as char);
            result.push(lo as char);
        } else {
            result.push(b as char);
        }
    }
    result
}

// ─── WebSocket: Session Activity ────────────────────────────

/// WebSocket upgrade handler for live session activity feed.
pub async fn ws_session(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws_session(socket, state))
}

/// Tail hook-events.jsonl and push new lines to the WebSocket client.
async fn handle_ws_session(mut socket: WebSocket, state: Arc<AppState>) {
    use tokio::time::Duration;

    let log_path = match state.trig_path.parent() {
        Some(base_dir) => base_dir.join("hook-events.jsonl"),
        None => return,
    };

    // Read current file and backfill last 100 lines
    let mut last_line_count = 0;
    if let Ok(content) = std::fs::read_to_string(&log_path) {
        let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
        last_line_count = lines.len();
        let start = lines.len().saturating_sub(100);
        for line in &lines[start..] {
            if socket.send(Message::Text((*line).to_string().into())).await.is_err() {
                return;
            }
        }
    }

    // Tail loop: check file for new lines every 500ms
    loop {
        match tokio::time::timeout(Duration::from_millis(500), socket.recv()).await {
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(_))) | Ok(None) => return,
            Err(_) => {} // Timeout — check the file
        }

        if let Ok(content) = std::fs::read_to_string(&log_path) {
            let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
            if lines.len() > last_line_count {
                for line in &lines[last_line_count..] {
                    if socket.send(Message::Text((*line).to_string().into())).await.is_err() {
                        return;
                    }
                }
                last_line_count = lines.len();
            }
        }
    }
}

// ─── Usage Analytics ────────────────────────────────────────

/// Normalized usage event — all providers produce these.
/// Phase 9 will add parse_codex(), parse_gemini(), etc. that return Vec<UsageEvent>.
#[derive(Clone)]
struct UsageEvent {
    provider: String,
    model: String,
    input_tokens: u64,
    output_tokens: u64,
    cache_read: u64,
    cache_write: u64,
    cost: f64,
    timestamp: String,
    project: String,
    session_id: String,
}

#[derive(Serialize, Clone)]
pub struct UsageSummary {
    pub total_input: u64,
    pub total_output: u64,
    pub total_cache_read: u64,
    pub total_cache_write: u64,
    pub estimated_cost_usd: f64,
    pub session_count: usize,
    pub active_days: u32,
    pub cost_per_day: f64,
    pub top_model: String,
    pub models: std::collections::HashMap<String, ModelUsage>,
    pub daily: Vec<DailyUsage>,
}

#[derive(Serialize, Clone, Default)]
pub struct ModelUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: f64,
    pub count: usize,
}

#[derive(Serialize, Clone)]
pub struct DailyUsage {
    pub date: String,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub sources: usize,
    pub cost: f64,
}

#[derive(Serialize, Clone)]
pub struct SessionUsageEntry {
    pub session_id: String,
    pub project: String,
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cost: f64,
    pub started: String,
    pub messages: usize,
}

#[derive(Serialize, Clone)]
pub struct ProjectUsage {
    pub project: String,
    pub provider: String,
    pub total_tokens: u64,
    pub cost: f64,
    pub event_count: usize,
    pub last_active: String,
}

fn estimate_cost(model: &str, input: u64, output: u64, cache_read: u64, cache_write: u64) -> f64 {
    let (inp_rate, out_rate, cr_rate, cw_rate) = if model.contains("opus") {
        (15.0, 75.0, 1.5, 18.75)
    } else if model.contains("haiku") {
        (0.80, 4.0, 0.08, 1.0)
    } else {
        // sonnet or unknown
        (3.0, 15.0, 0.30, 3.75)
    };
    (input as f64 * inp_rate + output as f64 * out_rate
        + cache_read as f64 * cr_rate + cache_write as f64 * cw_rate)
        / 1_000_000.0
}

// ─── Provider: Claude Code ─────────────────────────────────
// Each provider returns Vec<UsageEvent>. Phase 9 adds more providers here.

/// Resolve project name from cwd path: `/home/user/chris-ai-systems/apps/base-v2` → `base-v2`
fn resolve_project_name(cwd: &str) -> String {
    let path = std::path::Path::new(cwd);
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Fallback: parse directory name like `-home-chriskahler-chris-ai-systems` → `chris-ai-systems`
fn project_from_dir_name(dir_name: &str) -> String {
    // Directory format: path with `/` → `-` and leading `-`
    // Reconstruct path, take last component
    let trimmed = dir_name.trim_start_matches('-');
    // Best-effort: try to find a recognizable project boundary
    // Look for common parent dirs (apps-, clients-, home-user-)
    for marker in ["apps-", "clients-", "production-"] {
        if let Some(pos) = trimmed.rfind(marker) {
            let after = &trimmed[pos + marker.len()..];
            if !after.is_empty() { return after.to_string(); }
        }
    }
    // Fallback: take everything after the last path-like separator
    // Heuristic: find the last segment that looks like a project name
    trimmed.rsplit('-').take(1).next().unwrap_or("unknown").to_string()
}

fn parse_claude_code(days: u32) -> Vec<UsageEvent> {
    let claude_dir = crate::home::home_root()
        .map(|h| h.join(".claude").join("projects"))
        .unwrap_or_default();

    let cutoff = chrono::Local::now() - chrono::Duration::days(days as i64);
    let cutoff_str = cutoff.format("%Y-%m-%dT").to_string();

    let mut events = Vec::new();
    let Ok(projects) = std::fs::read_dir(&claude_dir) else { return events };

    for project_dir in projects.flatten() {
        if !project_dir.path().is_dir() { continue; }

        let dir_fallback = project_from_dir_name(
            &project_dir.file_name().to_string_lossy()
        );

        let Ok(files) = std::fs::read_dir(project_dir.path()) else { continue };

        for file in files.flatten() {
            let path = file.path();
            if path.extension().is_none_or(|e| e != "jsonl") { continue; }

            // No mtime filter — read ALL files, filter by content timestamp

            let session_id = path.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();

            let Ok(content) = std::fs::read_to_string(&path) else { continue };

            let mut session_project = dir_fallback.clone();
            let mut cwd_resolved = false;

            for line in content.lines() {
                let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else { continue };

                // Extract cwd from any event for project name (first one wins)
                if !cwd_resolved
                    && let Some(cwd) = val.get("cwd").and_then(|c| c.as_str())
                        && !cwd.is_empty() {
                            session_project = resolve_project_name(cwd);
                            cwd_resolved = true;
                        }

                let Some(msg) = val.get("message") else { continue };
                let Some(usage) = msg.get("usage") else { continue };

                let ts = val.get("timestamp").and_then(|t| t.as_str()).unwrap_or("");

                // Filter by timestamp — skip events outside the time window
                if !ts.is_empty() && ts < cutoff_str.as_str() { continue; }

                let model = msg.get("model").and_then(|m| m.as_str()).unwrap_or("unknown").to_string();
                let inp = usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                let out = usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                let cr = usage.get("cache_read_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                let cw = usage.get("cache_creation_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                let cost = estimate_cost(&model, inp, out, cr, cw);

                events.push(UsageEvent {
                    provider: "Claude Code".to_string(),
                    model,
                    input_tokens: inp,
                    output_tokens: out,
                    cache_read: cr,
                    cache_write: cw,
                    cost,
                    timestamp: ts.to_string(),
                    project: session_project.clone(),
                    session_id: session_id.clone(),
                });
            }
        }
    }

    events
}

// ─── Multi-provider dispatcher ─────────────────────────────

fn collect_all_events(days: u32) -> Vec<UsageEvent> {
    let mut all = parse_claude_code(days);
    // Phase 9: all.extend(parse_codex(days));
    // Phase 9: all.extend(parse_gemini(days));
    // Phase 9: all.extend(parse_open_code(days));
    // Phase 9: all.extend(parse_cursor(days));
    all.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    all
}

// ─── Provider-agnostic aggregation ─────────────────────────

fn aggregate_usage(events: &[UsageEvent], days: u32) -> (UsageSummary, Vec<SessionUsageEntry>, Vec<ProjectUsage>) {
    use std::collections::HashMap;

    let cutoff = chrono::Local::now() - chrono::Duration::days(days as i64);
    let cutoff_str = cutoff.format("%Y-%m-%d").to_string();

    let mut total_input = 0u64;
    let mut total_output = 0u64;
    let mut total_cache_read = 0u64;
    let mut total_cache_write = 0u64;
    let mut total_cost = 0.0f64;
    let mut models: HashMap<String, ModelUsage> = HashMap::new();
    // daily_map: date -> (input, output, cache_read, cache_write, sources, cost)
    let mut daily_map: HashMap<String, (u64, u64, u64, u64, usize, f64)> = HashMap::new();
    // session_map: session_id -> accumulated session data
    // (provider, start, input, output, cache_read, cache_write, last_ts, events)
    type SessionAccum = (String, String, u64, u64, u64, u64, String, usize);
    let mut session_map: HashMap<String, SessionAccum> = HashMap::new();
    // project_map: project -> (provider, total_tokens, cost, event_count, last_active)
    let mut project_map: HashMap<String, (String, u64, f64, usize, String)> = HashMap::new();

    for ev in events {
        total_input += ev.input_tokens;
        total_output += ev.output_tokens;
        total_cache_read += ev.cache_read;
        total_cache_write += ev.cache_write;
        total_cost += ev.cost;

        // Model aggregation
        let m = models.entry(ev.model.clone()).or_default();
        m.input += ev.input_tokens;
        m.output += ev.output_tokens;
        m.cache_read += ev.cache_read;
        m.cache_write += ev.cache_write;
        m.cost += ev.cost;
        m.count += 1;

        // Daily aggregation
        if ev.timestamp.len() >= 10 {
            let date = &ev.timestamp[..10];
            if date >= cutoff_str.as_str() {
                let entry = daily_map.entry(date.to_string()).or_insert((0, 0, 0, 0, 0, 0.0));
                entry.0 += ev.input_tokens;
                entry.1 += ev.output_tokens;
                entry.2 += ev.cache_read;
                entry.3 += ev.cache_write;
                entry.4 += 1;
                entry.5 += ev.cost;
            }
        }

        // Session aggregation
        let sess = session_map.entry(ev.session_id.clone()).or_insert_with(|| {
            (ev.project.clone(), ev.model.clone(), 0, 0, 0, 0, ev.timestamp.clone(), 0)
        });
        sess.2 += ev.input_tokens;
        sess.3 += ev.output_tokens;
        sess.4 += ev.cache_read;
        sess.5 += ev.cache_write;
        sess.7 += 1;
        if sess.6.is_empty() && !ev.timestamp.is_empty() {
            sess.6 = ev.timestamp.clone();
        }

        // Project aggregation
        let proj = project_map.entry(ev.project.clone()).or_insert_with(|| {
            (ev.provider.clone(), 0, 0.0, 0, String::new())
        });
        proj.1 += ev.input_tokens + ev.output_tokens;
        proj.2 += ev.cost;
        proj.3 += 1;
        if ev.timestamp > proj.4 {
            proj.4 = ev.timestamp.clone();
        }
    }

    // Build daily vec
    let mut daily: Vec<DailyUsage> = daily_map.into_iter()
        .map(|(date, (input, output, cache_read, cache_write, sources, cost))| {
            DailyUsage { date, input, output, cache_read, cache_write, sources, cost }
        })
        .collect();
    daily.sort_by(|a, b| a.date.cmp(&b.date));

    // Build sessions vec
    let mut sessions: Vec<SessionUsageEntry> = session_map.into_iter()
        .map(|(session_id, (project, model, input, output, cache_read, cache_write, started, messages))| {
            SessionUsageEntry {
                session_id, project, model, input, output, cache_read,
                cost: estimate_cost("", input, output, cache_read, cache_write),
                started, messages,
            }
        })
        .collect();
    sessions.sort_by(|a, b| b.started.cmp(&a.started));

    // Build projects vec
    let mut projects: Vec<ProjectUsage> = project_map.into_iter()
        .map(|(project, (provider, total_tokens, cost, event_count, last_active))| {
            ProjectUsage { project, provider, total_tokens, cost, event_count, last_active }
        })
        .collect();
    projects.sort_by(|a, b| b.cost.partial_cmp(&a.cost).unwrap_or(std::cmp::Ordering::Equal));

    // Compute derived summary fields
    let active_days = daily.len() as u32;
    let cost_rounded = (total_cost * 100.0).round() / 100.0;
    let cost_per_day = if active_days > 0 {
        (cost_rounded / active_days as f64 * 100.0).round() / 100.0
    } else {
        0.0
    };
    let top_model = models.iter()
        .max_by_key(|(_, m)| m.input + m.output)
        .map(|(name, _)| name.clone())
        .unwrap_or_default();

    let summary = UsageSummary {
        total_input,
        total_output,
        total_cache_read,
        total_cache_write,
        estimated_cost_usd: cost_rounded,
        session_count: sessions.len(),
        active_days,
        cost_per_day,
        top_model,
        models,
        daily,
    };

    (summary, sessions, projects)
}

#[derive(Deserialize)]
pub struct UsageQuery {
    pub days: Option<u32>,
}

pub async fn usage_summary(Query(q): Query<UsageQuery>) -> Json<UsageSummary> {
    let days = q.days.unwrap_or(30);
    let events = collect_all_events(days);
    let (summary, _, _) = aggregate_usage(&events, days);
    Json(summary)
}

pub async fn usage_sessions(Query(q): Query<UsageQuery>) -> Json<Vec<SessionUsageEntry>> {
    let days = q.days.unwrap_or(30);
    let events = collect_all_events(days);
    let (_, sessions, _) = aggregate_usage(&events, days);
    Json(sessions)
}

pub async fn usage_projects(Query(q): Query<UsageQuery>) -> Json<Vec<ProjectUsage>> {
    let days = q.days.unwrap_or(30);
    let events = collect_all_events(days);
    let (_, _, projects) = aggregate_usage(&events, days);
    Json(projects)
}

// ─── Graph Reload ───────────────────────────────────────────

pub async fn reload_graph(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let new_store = crate::store::load_graph(&state.trig_path)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut store = state.store_guard();
    *store = new_store;

    Ok(Json(serde_json::json!({ "reloaded": true })))
}

// ─── Task Creation ──────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateTaskBody {
    pub name: String,
    pub project: String,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub description: Option<String>,
}

pub async fn create_task(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateTaskBody>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ns = &state.config.namespace;
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let status = body.status.as_deref().unwrap_or("active");
    let priority = body.priority.as_deref().unwrap_or("normal");
    let slug = crate::crud::slugify(&body.name);
    let task_iri = crate::crud::build_iri(ns, "task", &slug);
    let project_slug = crate::crud::slugify(&body.project);
    let project_iri = crate::crud::build_iri(ns, "project", &project_slug);

    let mut extra_triples = String::new();
    extra_triples.push_str(&format!("<{task_iri}> {p}:priority \"{priority}\" .\n"));
    if let Some(ref desc) = body.description
        && !desc.is_empty() {
            extra_triples.push_str(&format!(
                "           <{task_iri}> {p}:description \"{}\" .\n",
                desc.replace('"', "\\\"").replace('\n', "\\n")
            ));
        }

    let store = state.store_guard();

    let insert = format!(
        "{pfx}\nINSERT DATA {{\n  GRAPH <{graph}> {{\n\
           <{task_iri}> rdf:type {p}:Task .\n\
           <{task_iri}> {p}:name \"{name}\" .\n\
           <{task_iri}> {p}:status \"{status}\" .\n\
           {extra_triples}\
           <{project_iri}> {p}:hasTask <{task_iri}> .\n\
           <{task_iri}> {p}:belongsTo <{project_iri}> .\n\
         }}\n}}",
        graph = crate::crud::workspace_graph_iri(ns, &crate::crud::workspace_slug(&state.cwd)),
        name = body.name.replace('"', "\\\""),
    );

    store.update(&insert).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    crate::store::write_back(&store, &state.trig_path, Change::Sparql(&insert))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({ "iri": task_iri, "name": body.name, "status": status, "priority": priority })))
}

// ─── Task Update (fields) ───────────────────────────────────

#[derive(Deserialize)]
pub struct UpdateTaskBody {
    pub name: Option<String>,
    pub priority: Option<String>,
    pub description: Option<String>,
}

pub async fn update_task(
    Path(iri): Path<String>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<UpdateTaskBody>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let iri = urldecode(&iri);
    let ns = &state.config.namespace;
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);

    let store = state.store_guard();

    // Verify entity exists
    let check = format!(
        "{pfx}\nASK {{ GRAPH ?g {{ <{iri}> rdf:type ?t }} }}"
    );
    match store.query(&check) {
        Ok(oxigraph::sparql::QueryResults::Boolean(true)) => {}
        _ => return Err(StatusCode::NOT_FOUND),
    }

    // Fields are optional, so the delta is whichever updates actually ran.
    let mut applied: Vec<String> = Vec::new();

    // Update each provided field
    if let Some(ref name) = body.name {
        let update = format!(
            "{pfx}\n\
             DELETE {{ GRAPH ?g {{ <{iri}> {p}:name ?old }} }}\n\
             INSERT {{ GRAPH ?g {{ <{iri}> {p}:name \"{}\" }} }}\n\
             WHERE  {{ GRAPH ?g {{ <{iri}> {p}:name ?old }} }}",
            name.replace('"', "\\\"")
        );
        store.update(&update).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        applied.push(update);
    }

    if let Some(ref priority) = body.priority {
        let valid = ["low", "normal", "high", "urgent"];
        if !valid.contains(&priority.as_str()) {
            return Err(StatusCode::BAD_REQUEST);
        }
        let update = format!(
            "{pfx}\n\
             DELETE {{ GRAPH ?g {{ <{iri}> {p}:priority ?old }} }}\n\
             INSERT {{ GRAPH ?g {{ <{iri}> {p}:priority \"{priority}\" }} }}\n\
             WHERE  {{ GRAPH ?g {{ OPTIONAL {{ <{iri}> {p}:priority ?old }} }} }}"
        );
        store.update(&update).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        applied.push(update);
    }

    if let Some(ref description) = body.description {
        let escaped = description.replace('"', "\\\"").replace('\n', "\\n");
        // Delete any existing description first (may not exist)
        let del = format!(
            "{pfx}\nDELETE {{ GRAPH ?g {{ <{iri}> {p}:description ?old }} }}\n\
             WHERE  {{ GRAPH ?g {{ <{iri}> {p}:description ?old }} }}"
        );
        let _ = store.update(&del);
        applied.push(del);
        // Insert new description
        if !description.is_empty() {
            let ins = format!(
                "{pfx}\nINSERT DATA {{ GRAPH <{graph}> {{ <{iri}> {p}:description \"{escaped}\" }} }}",
                graph = crate::crud::workspace_graph_iri(ns, &crate::crud::workspace_slug(&state.cwd)),
            );
            store.update(&ins).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            applied.push(ins);
        }
    }

    crate::store::write_back(&store, &state.trig_path, Change::Sparql(&applied.join(";\n")))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({
        "iri": iri,
        "name": body.name,
        "priority": body.priority,
        "description": body.description,
    })))
}

// ─── Task Delete ────────────────────────────────────────────

pub async fn delete_task(
    Path(iri): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let iri = urldecode(&iri);
    let ns = &state.config.namespace;
    let pfx = crate::crud::prefixes(ns);

    let store = state.store_guard();

    // Verify entity exists
    let check = format!(
        "{pfx}\nASK {{ GRAPH ?g {{ <{iri}> rdf:type ?t }} }}"
    );
    match store.query(&check) {
        Ok(oxigraph::sparql::QueryResults::Boolean(true)) => {}
        _ => return Err(StatusCode::NOT_FOUND),
    }

    // Delete all triples where task is subject
    let del_subject = format!(
        "{pfx}\nDELETE {{ GRAPH ?g {{ <{iri}> ?p ?o }} }}\n\
         WHERE  {{ GRAPH ?g {{ <{iri}> ?p ?o }} }}"
    );
    store.update(&del_subject).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Delete all triples where task is object (hasTask edges from project)
    let del_object = format!(
        "{pfx}\nDELETE {{ GRAPH ?g {{ ?s ?p <{iri}> }} }}\n\
         WHERE  {{ GRAPH ?g {{ ?s ?p <{iri}> }} }}"
    );
    store.update(&del_object).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    crate::store::write_back(&store, &state.trig_path, Change::Sparql(&format!("{del_subject};\n{del_object}")))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({ "deleted": true, "iri": iri })))
}

// ─── Entity Creation ────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateEntityBody {
    pub name: String,
    pub r#type: String,
    pub domain: Option<String>,
}

pub async fn create_entity(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateEntityBody>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ns = &state.config.namespace;
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let slug = crate::crud::slugify(&body.name);
    let entity_iri = crate::crud::build_iri(ns, "entity", &slug);
    let ws_slug = crate::crud::workspace_slug(&state.cwd);
    let graph = crate::crud::workspace_graph_iri(ns, &ws_slug);

    let mut domain_triple = String::new();
    if let Some(ref domain) = body.domain {
        let domain_slug = crate::crud::slugify(domain);
        let domain_iri = crate::crud::build_iri(ns, "domain", &domain_slug);
        domain_triple = format!("<{entity_iri}> {p}:hasDomain <{domain_iri}> .\n");
    }

    let store = state.store_guard();

    let insert = format!(
        "{pfx}\nINSERT DATA {{\n  GRAPH <{graph}> {{\n\
           <{entity_iri}> rdf:type {p}:{etype} .\n\
           <{entity_iri}> {p}:name \"{name}\" .\n\
           {domain_triple}\
         }}\n}}",
        etype = body.r#type,
        name = body.name.replace('"', "\\\""),
    );

    store.update(&insert).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    crate::store::write_back(&store, &state.trig_path, Change::Sparql(&insert))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({ "iri": entity_iri, "name": body.name, "type": body.r#type })))
}

// ─── Domain Rules ───────────────────────────────────────────

#[derive(Serialize)]
pub struct DomainInfo {
    pub name: String,
    pub mode: String,
    pub prompt_keywords: Vec<String>,
    pub paths: Vec<String>,
    pub rules: Vec<String>,
}

pub async fn get_domains(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<DomainInfo>> {
    let domains = crate::domain::load_domains(&state.cwd);
    let store = state.store_guard();
    let ns = &state.config.namespace;
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);

    let result: Vec<DomainInfo> = domains.iter().map(|d| {
        // Merge TOML rules (rendered with rationale) with graph-native rules
        let mut rules = d.rendered_rules();

        let domain_slug = crate::crud::slugify(&d.name);
        let domain_iri = crate::crud::build_iri(ns, "domain", &domain_slug);
        let sparql = format!(
            "{pfx}\nSELECT ?text WHERE {{\n\
               GRAPH ?g {{ <{domain_iri}> {p}:hasRule ?rule . ?rule {p}:ruleText ?text }}\n\
             }} ORDER BY ?text"
        );

        if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&sparql) {
            for row in solutions.flatten() {
                if let Some(t) = row.get("text") {
                    let text = term_str(t.into());
                    if !text.is_empty() && !rules.contains(&text) {
                        rules.push(text);
                    }
                }
            }
        }

        DomainInfo {
            name: d.name.clone(),
            mode: if d.is_always() { "always".into() } else { "triggered".into() },
            prompt_keywords: d.prompt_keywords.clone(),
            paths: d.paths.clone(),
            rules,
        }
    }).collect();
    Json(result)
}

// ─── Rule CRUD ──────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AddRuleBody {
    pub domain: String,
    pub text: String,
}

pub async fn add_rule(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AddRuleBody>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ns = &state.config.namespace;
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let domain_slug = crate::crud::slugify(&body.domain);
    let domain_iri = crate::crud::build_iri(ns, "domain", &domain_slug);
    let rule_slug = crate::crud::slugify(&body.text);
    let rule_iri = crate::crud::build_iri(ns, "rule", &rule_slug);
    let ws_slug = crate::crud::workspace_slug(&state.cwd);
    let graph = crate::crud::workspace_graph_iri(ns, &ws_slug);

    let store = state.store_guard();

    let insert = format!(
        "{pfx}\nINSERT DATA {{\n  GRAPH <{graph}> {{\n\
           <{rule_iri}> rdf:type {p}:Rule .\n\
           <{rule_iri}> {p}:ruleText \"{}\" .\n\
           <{domain_iri}> {p}:hasRule <{rule_iri}> .\n\
         }}\n}}",
        body.text.replace('"', "\\\""),
    );

    store.update(&insert).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    crate::store::write_back(&store, &state.trig_path, Change::Sparql(&insert))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({ "rule_iri": rule_iri, "domain": body.domain, "text": body.text })))
}

#[derive(Deserialize)]
pub struct DeleteRuleBody {
    pub domain: String,
    pub text: String,
}

pub async fn delete_rule(
    State(state): State<Arc<AppState>>,
    Json(body): Json<DeleteRuleBody>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let ns = &state.config.namespace;
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    let rule_slug = crate::crud::slugify(&body.text);
    let rule_iri = crate::crud::build_iri(ns, "rule", &rule_slug);

    let store = state.store_guard();

    let delete = format!(
        "{pfx}\nDELETE WHERE {{ GRAPH ?g {{ <{rule_iri}> ?p ?o }} }};\n\
         DELETE WHERE {{ GRAPH ?g {{ ?s {p}:hasRule <{rule_iri}> }} }}"
    );

    store.update(&delete).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    crate::store::write_back(&store, &state.trig_path, Change::Sparql(&delete))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({ "deleted": true })))
}

// ─── Export ─────────────────────────────────────────────────

pub async fn export_usage_csv(Query(q): Query<UsageQuery>) -> axum::response::Response {
    use axum::http::header;

    let days = q.days.unwrap_or(30);
    let events = collect_all_events(days);
    let (_, sessions, _) = aggregate_usage(&events, days);
    let mut csv = String::from("session_id,project,model,input_tokens,output_tokens,cost_usd,started,messages\n");
    for s in &sessions {
        csv.push_str(&format!(
            "{},{},{},{},{},{:.4},{},{}\n",
            s.session_id, s.project, s.model, s.input, s.output, s.cost, s.started, s.messages
        ));
    }

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/csv"),
            (header::CONTENT_DISPOSITION, "attachment; filename=\"usage.csv\""),
        ],
        csv,
    ).into_response()
}

pub async fn export_graph_json(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let ns = &state.config.namespace;
    let store = state.store_guard();

    // Reuse existing nodes + edges queries
    let nodes_sparql = build_nodes_sparql(ns);
    let edges_sparql = build_edges_sparql(ns);

    let mut nodes_list = Vec::new();
    if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&nodes_sparql) {
        for row in solutions.flatten() {
            let name = row.get("name").map(|t| term_str(t.into())).unwrap_or_default();
            let iri = row.get("entity").map(|t| term_str(t.into())).unwrap_or_default();
            nodes_list.push(serde_json::json!({"iri": iri, "name": name}));
        }
    }

    let mut edges_list = Vec::new();
    if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&edges_sparql) {
        for row in solutions.flatten() {
            let s = row.get("s").map(|t| term_str(t.into())).unwrap_or_default();
            let p_val = row.get("p").map(|t| term_str(t.into())).unwrap_or_default();
            let o = row.get("o").map(|t| term_str(t.into())).unwrap_or_default();
            edges_list.push(serde_json::json!({"source": s, "predicate": p_val, "target": o}));
        }
    }

    Json(serde_json::json!({
        "nodes": nodes_list,
        "edges": edges_list,
        "exported_at": chrono::Local::now().to_rfc3339(),
    }))
}

/// Build nodes SPARQL (extracted for reuse by export)
fn build_nodes_sparql(ns: &crate::config::NamespaceConfig) -> String {
    let p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    format!(
        "{pfx}\nSELECT DISTINCT ?entity ?type ?name ?status ?path WHERE {{\n\
           GRAPH ?g {{\n\
             ?entity rdf:type ?type .\n\
             OPTIONAL {{ ?entity {p}:name ?name }}\n\
             OPTIONAL {{ ?entity {p}:status ?status }}\n\
             OPTIONAL {{ ?entity {p}:path ?path }}\n\
           }}\n\
         }}"
    )
}

/// Build edges SPARQL (extracted for reuse by export)
fn build_edges_sparql(ns: &crate::config::NamespaceConfig) -> String {
    let _p = &ns.prefix;
    let pfx = crate::crud::prefixes(ns);
    format!(
        "{pfx}\nSELECT ?s ?p ?o WHERE {{\n\
           GRAPH ?g {{ ?s ?p ?o }}\n\
           FILTER(isIRI(?o))\n\
           FILTER(?p != rdf:type)\n\
           FILTER(?g != <{ledger}>)\n\
         }}",
        ledger = crate::apply_ops::LEDGER_GRAPH,
    )
}

// ─── Log Rotation ───────────────────────────────────────────

/// Truncate hook-events.jsonl if > 10MB, keeping last 5000 lines.
pub fn rotate_hook_log(trig_path: &std::path::Path) {
    let Some(base_dir) = trig_path.parent() else { return };
    let log_path = base_dir.join("hook-events.jsonl");

    let Ok(meta) = std::fs::metadata(&log_path) else { return };
    if meta.len() < 10 * 1024 * 1024 { return; }

    let Ok(content) = std::fs::read_to_string(&log_path) else { return };
    let lines: Vec<&str> = content.lines().collect();
    let keep = lines.len().saturating_sub(5000);
    let tail: String = lines[keep..].join("\n") + "\n";
    let _ = std::fs::write(&log_path, tail);
}
