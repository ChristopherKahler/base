//! Session-start write-reconcile: the mechanical active⇄deferred engine.
//!
//! Every prior hook was read-only. This is the first that MUTATES the graph before
//! signals surface, so the state Claude renders is already true. For each project it:
//!   1. overwrites `lastActive` with the real folder last-touch (de-fakes the
//!      session-stamped timestamp), then
//!   2. decays status mechanically: an `active` project untouched past `stale_days`
//!      becomes `deferred` ("auto: cold Nd"); a `deferred` project touched within the
//!      window revives to `active`.
//!
//! Pinned projects — those carrying a *future* `resurfaceAt` (an explicit operator
//! "defer until / keep active") — are exempt from mechanical status changes; only
//! their `lastActive` is refreshed. This is the resurfaceAt-pin override: the operator
//! signal wins, everything else decays from the working signals alone.
//!
//! Gated entirely on `[protocol] enabled`. Fail-open: any error leaves state as-is.

use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Local};
use oxigraph::model::Term;
use oxigraph::sparql::QueryResults;
use oxigraph::store::Store;

use crate::config::BaseConfig;
use crate::crud;
use crate::store;

#[derive(Debug, Default)]
pub struct ReconcileStats {
    pub scanned: usize,
    pub refreshed: usize,
    pub deferred: usize,
    pub revived: usize,
}

impl ReconcileStats {
    /// Did any *status* actually change? lastActive refreshes alone stay silent.
    pub fn changed(&self) -> bool {
        self.deferred > 0 || self.revived > 0
    }
}

struct Row {
    iri: String,
    g: String,
    status: String,
    path: String,
    resurface_at: Option<String>,
}

/// Run the reconcile pass over the workspace graph. No-op (`Ok` default) when the
/// protocol is disabled or no workspace graph exists.
pub fn reconcile(cwd: &Path, config: &BaseConfig) -> Result<ReconcileStats> {
    let mut stats = ReconcileStats::default();
    if !config.protocol.enabled {
        return Ok(stats);
    }

    let Some(base_dir) = crate::config::find_workspace_base(cwd) else {
        return Ok(stats);
    };
    let ws_root = base_dir.parent().unwrap_or(cwd).to_path_buf();
    let trig_path = base_dir.join("graph.nq");
    if !trig_path.exists() {
        return Ok(stats);
    }

    let store = store::load_graph(&trig_path)?;
    let ns = &config.namespace;
    let p = &ns.prefix;
    let pfx = crud::prefixes(ns);
    let stale_days = config.protocol.stale_days as i64;

    // Every project-like entity that declares a folder path.
    let select = format!(
        "{pfx}\n\
         SELECT ?iri ?g ?status ?path ?resurfaceAt WHERE {{\n\
           GRAPH ?g {{\n\
             ?iri a ?type ;\n\
               {p}:status ?status ;\n\
               {p}:path ?path .\n\
             OPTIONAL {{ ?iri {p}:resurfaceAt ?resurfaceAt }}\n\
             FILTER(?type IN ({p}:Project, {p}:App, {p}:Framework, {p}:TrackingProject))\n\
           }}\n\
         }}"
    );

    let rows = collect_rows(&store, &select)?;
    let now = Local::now();
    let mut ops: Vec<String> = Vec::new();

    for row in &rows {
        stats.scanned += 1;
        let abs = resolve_path(&ws_root, &row.path);
        let Some(touch) = crate::protocol::touch::folder_last_touch(&abs) else {
            continue; // folder gone or empty — leave state untouched
        };
        let touch_iso = touch.to_rfc3339_opts(chrono::SecondsFormat::Secs, false);

        // (1) De-fake lastActive → real touch.
        ops.push(crud::field_update(
            &row.g,
            &row.iri,
            &format!("{p}:lastActive"),
            &format!("\"{touch_iso}\"^^xsd:dateTime"),
        ));
        stats.refreshed += 1;

        // A future resurfaceAt pins the project — exempt from mechanical decay.
        let pinned = row
            .resurface_at
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Local) > now)
            .unwrap_or(false);
        if pinned {
            continue;
        }

        // (2) Mechanical decay / revive.
        let days = now.signed_duration_since(touch).num_days();
        if row.status == "active" && days >= stale_days {
            ops.push(crud::field_update(&row.g, &row.iri, &format!("{p}:status"), "\"deferred\""));
            ops.push(crud::field_update(
                &row.g,
                &row.iri,
                &format!("{p}:deferredReason"),
                &format!("\"auto: cold {days}d\""),
            ));
            stats.deferred += 1;
        } else if row.status == "deferred" && days < stale_days {
            ops.push(crud::field_update(&row.g, &row.iri, &format!("{p}:status"), "\"active\""));
            // Clear the auto-deferral marker on revive.
            ops.push(format!(
                "DELETE {{ GRAPH <{g}> {{ <{s}> {p}:deferredReason ?r }} }}\n\
                 WHERE {{ GRAPH <{g}> {{ <{s}> {p}:deferredReason ?r }} }}",
                g = row.g,
                s = row.iri
            ));
            stats.revived += 1;
        }
    }

    if !ops.is_empty() {
        for op in &ops {
            let _ = store.update(&format!("{pfx}\n{op}"));
        }
        store::write_back(&store, &trig_path)?;
    }

    Ok(stats)
}

/// Extract rows, keeping full IRIs for `?iri`/`?g` (needed verbatim in follow-up
/// updates) rather than the prefix-stripped display form.
fn collect_rows(store: &Store, sparql: &str) -> Result<Vec<Row>> {
    let QueryResults::Solutions(solutions) = store::query(store, sparql)? else {
        return Ok(Vec::new());
    };

    let full = |t: Option<&Term>| -> Option<String> {
        t.map(|term| match term {
            Term::NamedNode(n) => n.as_str().to_string(),
            Term::Literal(l) => l.value().to_string(),
            other => other.to_string(),
        })
    };

    let mut out = Vec::new();
    for sol in solutions.filter_map(|r| r.ok()) {
        let (Some(iri), Some(g), Some(status), Some(path)) = (
            full(sol.get("iri")),
            full(sol.get("g")),
            full(sol.get("status")),
            full(sol.get("path")),
        ) else {
            continue;
        };
        out.push(Row {
            iri,
            g,
            status,
            path,
            resurface_at: full(sol.get("resurfaceAt")),
        });
    }
    Ok(out)
}

fn resolve_path(ws_root: &Path, path: &str) -> PathBuf {
    let pb = PathBuf::from(path);
    if pb.is_absolute() {
        pb
    } else {
        ws_root.join(pb)
    }
}
