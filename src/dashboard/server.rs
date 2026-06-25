use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::response::Html;
use axum::routing::{delete, get, patch, post, put};
use include_dir::{Dir, include_dir};
use tower_http::cors::CorsLayer;

use crate::config::BaseConfig;

static DASHBOARD_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/dashboard/dist");

/// Shared state for all API handlers.
pub struct AppState {
    pub store: Mutex<oxigraph::store::Store>,
    pub config: BaseConfig,
    pub cwd: PathBuf,
    pub trig_path: PathBuf,
}

impl AppState {
    /// Lock the store, recovering from a poisoned mutex instead of propagating the
    /// panic. The dashboard is a long-running server: one handler that panicked while
    /// holding the lock must not cascade into every later request panicking too. Graph
    /// integrity is guaranteed at the persistence layer (`write_back` validates the
    /// serialized output before the atomic rename), so serving from a recovered guard
    /// is safe.
    pub fn store_guard(&self) -> std::sync::MutexGuard<'_, oxigraph::store::Store> {
        self.store.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Start the dashboard server on the given port.
pub async fn start(port: u16, cwd: PathBuf) {
    let config = BaseConfig::load(&cwd);

    // Collect all workspace graph.nq paths — primary + registered workspaces
    let trig_path = crate::config::find_workspace_base(&cwd)
        .unwrap_or_else(|| cwd.join(".base"))
        .join("graph.nq");

    let mut trig_paths: Vec<PathBuf> = vec![trig_path.clone()];

    // Read registered workspaces from BASE's own registry (~/.base-gbl/base.toml)
    if let Some(home) = dirs::home_dir() {
        let base_toml = home.join(".base-gbl/base.toml");
        if let Ok(content) = std::fs::read_to_string(&base_toml)
            && let Ok(table) = content.parse::<toml::Table>()
                && let Some(workspaces) = table.get("workspace").and_then(|v| v.as_array()) {
                    for ws in workspaces {
                        if let Some(path_str) = ws.get("path").and_then(|v| v.as_str()) {
                            let candidate = PathBuf::from(path_str).join(".base/graph.nq");
                            if candidate.exists() && candidate != trig_path {
                                trig_paths.push(candidate);
                            }
                        }
                    }
                }

        // Also check global tier
        let global_trig = home.join(".base-gbl/graph.nq");
        if global_trig.exists() {
            trig_paths.push(global_trig);
        }
    }

    let existing_paths: Vec<&std::path::Path> = trig_paths.iter()
        .filter(|p| p.exists())
        .map(|p| p.as_path())
        .collect();

    let store = if existing_paths.is_empty() {
        eprintln!("No graph.nq files found. Run `base scaffold` then `base sync`.");
        oxigraph::store::Store::new().expect("in-memory store")
    } else {
        println!("Loading {} graph(s):", existing_paths.len());
        for p in &existing_paths {
            println!("  • {}", p.display());
        }
        match crate::store::load_graphs(&existing_paths) {
            Ok(store) => store,
            Err(e) => {
                eprintln!("Failed to load graphs: {e}");
                oxigraph::store::Store::new().expect("in-memory store")
            }
        }
    };

    // AST entities live in ast.ttl (never merged into graph.nq — AUDIT C10).
    // Load each workspace's ast.ttl alongside so the graph explorer sees code entities.
    for p in &trig_paths {
        if let Some(base_dir) = p.parent() {
            let ast_path = base_dir.join("ast.ttl");
            if ast_path.exists() {
                if let Err(e) = crate::store::load_turtle_into(&store, &ast_path) {
                    eprintln!("Failed to load {}: {e}", ast_path.display());
                } else {
                    println!("  • {} (AST)", ast_path.display());
                }
            }
        }
    }

    // Rotate hook event log if oversized
    super::api::rotate_hook_log(&trig_path);

    let state = Arc::new(AppState {
        store: Mutex::new(store),
        config,
        cwd,
        trig_path,
    });

    let app = Router::new()
        .route("/", get(serve_index))
        // Graph API
        .route("/api/graph/nodes", get(super::api::nodes))
        .route("/api/graph/edges", get(super::api::edges))
        .route("/api/graph/search", get(super::api::search))
        .route("/api/graph/node/{iri}", get(super::api::node_detail))
        .route("/api/graph/node/{iri}/notes", get(super::api::get_notes))
        .route("/api/graph/node/{iri}/notes", post(super::api::add_note))
        .route("/api/graph/node/{iri}/notes/{index}", put(super::api::update_note))
        .route("/api/graph/node/{iri}/notes/{index}", delete(super::api::delete_note))
        // Ops API
        .route("/api/ops/projects", get(super::api::ops_projects))
        .route("/api/ops/decisions", get(super::api::ops_decisions))
        .route("/api/ops/reminders", get(super::api::ops_reminders))
        // Task CRUD
        .route("/api/ops/task/{iri}/status", patch(super::api::update_task_status))
        .route("/api/ops/task/{iri}", patch(super::api::update_task))
        .route("/api/ops/task/{iri}", delete(super::api::delete_task))
        // Decision CRUD
        .route("/api/ops/decision", post(super::api::create_decision))
        .route("/api/ops/decision/{iri}", patch(super::api::update_decision))
        .route("/api/ops/decision/{iri}", delete(super::api::delete_decision))
        // Reminder CRUD
        .route("/api/ops/reminder", post(super::api::create_reminder))
        .route("/api/ops/reminder/{iri}/complete", patch(super::api::complete_reminder))
        .route("/api/ops/reminder/{iri}", delete(super::api::delete_reminder))
        // Project status
        .route("/api/ops/project/{iri}/status", patch(super::api::update_project_status))
        // Ledger (cost attribution)
        .route("/api/ops/ledger", get(super::api::ops_ledger))
        .route("/api/ops/cost-summary", get(super::api::ops_cost_summary))
        // Usage Analytics
        .route("/api/usage/summary", get(super::api::usage_summary))
        .route("/api/usage/sessions", get(super::api::usage_sessions))
        .route("/api/usage/projects", get(super::api::usage_projects))
        // Graph management
        .route("/api/graph/reload", post(super::api::reload_graph))
        .route("/api/graph/entity", post(super::api::create_entity))
        // Task creation
        .route("/api/ops/task", post(super::api::create_task))
        // Domain rules
        .route("/api/domains", get(super::api::get_domains))
        .route("/api/domains/rule", post(super::api::add_rule))
        .route("/api/domains/rule", delete(super::api::delete_rule))
        // Export
        .route("/api/export/usage-csv", get(super::api::export_usage_csv))
        .route("/api/export/graph-json", get(super::api::export_graph_json))
        // WebSocket
        .route("/api/ws/session", get(super::api::ws_session))
        .fallback(get(serve_static))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let bind_addr = format!("0.0.0.0:{port}");
    let url = format!("http://localhost:{port}");

    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .unwrap_or_else(|e| {
            eprintln!("Failed to bind {bind_addr}: {e}");
            std::process::exit(1);
        });

    println!("Dashboard: {url}");
    println!("Press Ctrl+C to stop\n");

    if let Err(e) = open::that(&url) {
        eprintln!("Could not open browser: {e}");
        println!("Open manually: {url}");
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("server error");

    println!("\nDashboard stopped.");
}

async fn serve_index() -> axum::response::Response {
    use axum::http::header;
    use axum::response::IntoResponse;
    match DASHBOARD_DIR.get_file("index.html") {
        Some(file) => (
            [(header::CONTENT_TYPE, "text/html"), (header::CACHE_CONTROL, "no-cache")],
            file.contents_utf8().unwrap_or(""),
        ).into_response(),
        None => Html("<h1>Dashboard assets not found</h1>".to_string()).into_response(),
    }
}

async fn serve_static(uri: axum::http::Uri) -> axum::response::Response {
    use axum::http::{StatusCode, header};
    use axum::response::IntoResponse;

    let path = uri.path().trim_start_matches('/');

    match DASHBOARD_DIR.get_file(path) {
        Some(file) => {
            let mime = match path.rsplit('.').next() {
                Some("js") => "application/javascript",
                Some("css") => "text/css",
                Some("html") => "text/html",
                Some("json") => "application/json",
                Some("svg") => "image/svg+xml",
                Some("png") => "image/png",
                Some("woff2") => "font/woff2",
                Some("woff") => "font/woff",
                _ => "application/octet-stream",
            };
            (StatusCode::OK, [(header::CONTENT_TYPE, mime)], file.contents().to_vec()).into_response()
        }
        None => {
            match DASHBOARD_DIR.get_file("index.html") {
                Some(file) => Html(file.contents_utf8().unwrap_or("").to_string()).into_response(),
                None => (StatusCode::NOT_FOUND, "Not found").into_response(),
            }
        }
    }
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl+c");
}
