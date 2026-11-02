use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use include_dir::{Dir, include_dir};

use crate::config::BaseConfig;

static DASHBOARD_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/dashboard/dist");

/// The names that mean this machine. Anything else is a network address, and a
/// network bind needs `--allow-remote` plus the startup token.
const LOOPBACK_HOSTS: [&str; 3] = ["127.0.0.1", "localhost", "::1"];

pub fn is_loopback(host: &str) -> bool {
    LOOPBACK_HOSTS.iter().any(|h| h.eq_ignore_ascii_case(host))
}

/// `host` as it appears in a `Host` header (or a bind address): an IPv6 literal
/// is bracketed, a name or an IPv4 address is not.
fn with_port(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Every `Host` header this server answers for, or `None` for "any".
///
/// A browser sends the name it was given, so a DNS name pointed at 127.0.0.1
/// arrives as that name. Refusing it here is what stops a rebinding page from
/// reaching a loopback server that has no accounts and can write the graph.
///
/// A wildcard bind (`0.0.0.0`, `::`) cannot know the names clients will use —
/// the LAN address, a machine name, `localhost` — so there is no list to check
/// against. That mode is never reachable without `--allow-remote`, which always
/// mints a token, and the token is what gates it.
pub fn allowed_hosts(bind_host: &str, port: u16) -> Option<Vec<String>> {
    if is_wildcard(bind_host) {
        return None;
    }
    let mut hosts = vec![with_port(bind_host, port)];
    let also = if is_loopback(bind_host) { LOOPBACK_HOSTS.to_vec() } else { vec!["localhost"] };
    for h in also {
        let h = with_port(h, port);
        if !hosts.contains(&h) {
            hosts.push(h);
        }
    }
    Some(hosts)
}

pub fn is_wildcard(host: &str) -> bool {
    matches!(host, "0.0.0.0" | "::" | "[::]")
}

pub fn host_allowed(host_header: Option<&str>, allowed: Option<&[String]>) -> bool {
    match allowed {
        None => true,
        Some(list) => host_header.is_some_and(|h| list.iter().any(|a| a.eq_ignore_ascii_case(h))),
    }
}

/// The token for a non-loopback bind. It is printed once at startup and lives
/// only in this process, so it has to be unguessable, not memorable.
fn session_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(std::process::id().to_le_bytes());
    h.update(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
            .to_le_bytes(),
    );
    h.update(std::collections::hash_map::RandomState::new().build_hasher().finish().to_le_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// The token may arrive as `?token=` or as `Authorization: Bearer`. A query
/// parameter is what makes the printed URL openable; the header is what a
/// script uses so the token stays out of logs.
fn token_ok(req: &Request, expected: &str) -> bool {
    let in_query = req
        .uri()
        .query()
        .is_some_and(|q| q.split('&').any(|p| p.strip_prefix("token=") == Some(expected)));
    let in_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| t == expected);
    in_query || in_header
}

/// Shared state for all API handlers.
pub struct AppState {
    pub store: Mutex<oxigraph::store::Store>,
    pub config: BaseConfig,
    pub cwd: PathBuf,
    pub trig_path: PathBuf,
    /// Every graph file the store was loaded from, in load order (#41).
    pub sources: Vec<PathBuf>,
    /// `(len, mtime nanos)` of each source at the last load; `None` for a file that was absent.
    pub loaded: Mutex<Vec<Option<(u64, u128)>>>,
}

/// The on-disk identity a reload decision is made on: length and mtime in nanoseconds, the
/// same pair the session-start delta marker uses. One `stat` per file per request.
pub fn file_identity(path: &std::path::Path) -> Option<(u64, u128)> {
    let m = std::fs::metadata(path).ok()?;
    let mtime = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
    Some((m.len(), mtime))
}

impl AppState {
    /// Lock the store, recovering from a poisoned mutex instead of propagating the
    /// panic. The dashboard is a long-running server: one handler that panicked while
    /// holding the lock must not cascade into every later request panicking too. Graph
    /// integrity is guaranteed at the persistence layer (`write_back` validates the
    /// serialized output before the atomic rename), so serving from a recovered guard
    /// is safe.
    pub fn store_guard(&self) -> std::sync::MutexGuard<'_, oxigraph::store::Store> {
        let mut guard = self.store.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        // #41: the snapshot was loaded once at start and every write serialised it back over
        // `graph.nq`, erasing whatever the CLI had written since. Before any handler touches the
        // store, compare each source's identity with the one it was loaded at; on a mismatch,
        // reload from disk first. The dashboard's own write moves the identity too, so the
        // request after a dashboard write pays one reload; that is the price of never losing a
        // CLI write, and `write_back` already made a write cost a full serialisation.
        let now: Vec<Option<(u64, u128)>> = self.sources.iter().map(|p| file_identity(p)).collect();
        let mut loaded = self.loaded.lock().unwrap_or_else(|p| p.into_inner());
        if *loaded != now {
            *guard = Self::load_sources(&self.sources);
            *loaded = now;
        }
        guard
    }

    /// Load every source that exists into one store, plus each source's sibling `ast.ttl`.
    pub fn load_sources(sources: &[PathBuf]) -> oxigraph::store::Store {
        let existing: Vec<&std::path::Path> = sources.iter().filter(|p| p.exists()).map(|p| p.as_path()).collect();
        let store = if existing.is_empty() {
            oxigraph::store::Store::new().expect("in-memory store")
        } else {
            match crate::store::load_graphs(&existing) {
                Ok(store) => store,
                Err(e) => {
                    eprintln!("Failed to load graphs: {e}");
                    oxigraph::store::Store::new().expect("in-memory store")
                }
            }
        };
        for p in sources {
            if let Some(base_dir) = p.parent() {
                let ast_path = base_dir.join("ast.ttl");
                if ast_path.exists()
                    && let Err(e) = crate::store::load_turtle_into(&store, &ast_path)
                {
                    eprintln!("Failed to load {}: {e}", ast_path.display());
                }
            }
        }
        store
    }

    /// The state the server runs with: the store loaded from `sources`, identities recorded.
    pub fn new(config: BaseConfig, cwd: PathBuf, trig_path: PathBuf, sources: Vec<PathBuf>) -> Self {
        let store = Self::load_sources(&sources);
        let loaded = sources.iter().map(|p| file_identity(p)).collect();
        AppState { store: Mutex::new(store), config, cwd, trig_path, sources, loaded: Mutex::new(loaded) }
    }
}

/// Start the dashboard server on the given port and host.
///
/// `host` is loopback by default. A non-loopback host has no accounts in front
/// of it and can write the graph, so it takes an explicit `allow_remote` and a
/// per-run token; without the flag the server refuses to start.
pub async fn start(port: u16, host: String, allow_remote: bool, cwd: PathBuf) {
    if !is_loopback(&host) && !allow_remote {
        eprintln!(
            "base dashboard: {host} is not a loopback address. The dashboard can read and \
             write your graph and has no accounts, so exposing it takes --allow-remote. \
             A token is generated and printed at startup when you do."
        );
        std::process::exit(1);
    }
    let token = (!is_loopback(&host)).then(|| std::sync::Arc::new(session_token()));

    let config = BaseConfig::load(&cwd);

    // Collect all workspace graph.nq paths — primary + registered workspaces
    let trig_path = crate::config::find_workspace_base(&cwd)
        .unwrap_or_else(|| cwd.join(".base"))
        .join("graph.nq");

    let mut trig_paths: Vec<PathBuf> = vec![trig_path.clone()];

    // Read registered workspaces from BASE's own registry (~/.base-gbl/base.toml)
    if let Some(home) = crate::home::home_root() {
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

    println!("Loading {} graph(s):", trig_paths.iter().filter(|p| p.exists()).count());
    for p in trig_paths.iter().filter(|p| p.exists()) {
        println!("  • {}", p.display());
    }

    // Rotate hook event log if oversized
    super::api::rotate_hook_log(&trig_path);

    let state = Arc::new(AppState::new(config, cwd, trig_path, trig_paths));

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
        .layer(middleware::from_fn(guard(
            std::sync::Arc::new(allowed_hosts(&host, port)),
            token.clone(),
        )))        .with_state(state);

    let bind_addr = with_port(&host, port);
    let mut url = format!("http://{bind_addr}");
    if let Some(t) = &token {
        url.push_str(&format!("/?token={t}"));
    }

    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .unwrap_or_else(|e| {
            eprintln!("Failed to bind {bind_addr}: {e}");
            std::process::exit(1);
        });

    println!("Dashboard: {url}");
    if token.is_some() {
        println!("⚠  reachable on the network — the URL above carries the token that protects it");
    }
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

/// The request gate: the `Host` header must name this server, and a network
/// bind must also carry the startup token. Applied to every route, including
/// the static assets, so there is no unguarded path in.
fn guard(
    allowed: std::sync::Arc<Option<Vec<String>>>,
    token: Option<std::sync::Arc<String>>,
) -> impl Fn(Request, Next) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>
       + Clone
       + Send
       + Sync
       + 'static {
    move |req: Request, next: Next| {
        let allowed = allowed.clone();
        let token = token.clone();
        Box::pin(async move {
            let host = req.headers().get(header::HOST).and_then(|v| v.to_str().ok());
            if !host_allowed(host, allowed.as_deref()) {
                return (StatusCode::FORBIDDEN, "forbidden: unexpected Host header").into_response();
            }
            if let Some(expected) = token
                && !token_ok(&req, &expected)
            {
                return (StatusCode::UNAUTHORIZED, "unauthorized: missing or wrong token").into_response();
            }
            next.run(req).await
        })
    }
}

async fn serve_index() -> axum::response::Response {
    match DASHBOARD_DIR.get_file("index.html") {
        Some(file) => (
            [(header::CONTENT_TYPE, "text/html"), (header::CACHE_CONTROL, "no-cache")],
            file.contents_utf8().unwrap_or(""),
        ).into_response(),
        None => Html("<h1>Dashboard assets not found</h1>".to_string()).into_response(),
    }
}

async fn serve_static(uri: axum::http::Uri) -> axum::response::Response {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn req(uri: &str) -> Request {
        Request::builder().uri(uri).body(axum::body::Body::empty()).unwrap()
    }

    fn list(host: &str, port: u16) -> Vec<String> {
        allowed_hosts(host, port).expect("a named bind has an allowlist")
    }

    #[test]
    fn loopback_binds_answer_to_every_loopback_name() {
        let allowed = list("127.0.0.1", 3741);
        assert!(host_allowed(Some("127.0.0.1:3741"), Some(&allowed)));
        assert!(host_allowed(Some("localhost:3741"), Some(&allowed)));
        assert!(host_allowed(Some("[::1]:3741"), Some(&allowed)));
    }

    #[test]
    fn a_rebinding_host_is_refused() {
        let allowed = list("127.0.0.1", 3741);
        assert!(
            !host_allowed(Some("evil.example:3741"), Some(&allowed)),
            "a DNS name pointed at loopback must not be served"
        );
        assert!(!host_allowed(Some("127.0.0.1:3742"), Some(&allowed)));
        assert!(!host_allowed(None, Some(&allowed)));
    }

    #[test]
    fn a_network_bind_answers_only_to_its_own_name() {
        let allowed = list("192.168.1.10", 3741);
        assert!(host_allowed(Some("192.168.1.10:3741"), Some(&allowed)));
        assert!(host_allowed(Some("localhost:3741"), Some(&allowed)));
        assert!(!host_allowed(Some("127.0.0.1:3741"), Some(&allowed)));
    }

    #[test]
    fn a_wildcard_bind_has_no_allowlist_so_the_token_is_the_gate() {
        assert!(allowed_hosts("0.0.0.0", 3741).is_none());
        assert!(allowed_hosts("::", 3741).is_none());
        assert!(host_allowed(Some("192.168.1.10:3741"), None));
    }

    #[test]
    fn ipv6_binds_are_bracketed_in_the_host_header() {
        assert_eq!(with_port("::1", 3741), "[::1]:3741");
        assert_eq!(with_port("127.0.0.1", 3741), "127.0.0.1:3741");
    }

    #[test]
    fn the_token_is_accepted_from_the_query_or_a_bearer_header() {
        assert!(token_ok(&req("/api/x?token=abc"), "abc"));
        assert!(!token_ok(&req("/api/x?token=abd"), "abc"));
        assert!(!token_ok(&req("/api/x"), "abc"));
        let with_header = Request::builder()
            .header("authorization", "Bearer abc")
            .body(axum::body::Body::empty())
            .unwrap();
        assert!(token_ok(&with_header, "abc"));
    }

    #[test]
    fn a_token_is_not_a_substring_match() {
        assert!(!token_ok(&req("/api/x?token=abcd"), "abc"));
        assert!(!token_ok(&req("/api/x?mytoken=abc"), "abc"));
    }

    #[test]
    fn session_tokens_differ() {
        assert_ne!(session_token(), session_token());
        assert_eq!(session_token().len(), 64);
    }
}
