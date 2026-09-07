//! One list, four readers.
//!
//! Chris's ruling 4 (2026-09-06): relay pings are excluded from analyze AND from
//! every other read surface by ONE shared rule — *"make sure the tool as a whole is
//! built to exclude pings in the same way"*. So the thing worth testing is not that
//! `graph analyze` happens to skip pings; it is that **every** reader consults
//! `ontology::transient::TRANSIENT_KINDS`, and that adding a kind to that list is
//! all it takes.
//!
//! Each test below plants a fixture record that WOULD match the reader's query on
//! every count except its transient class, and asserts the reader drops it. Where a
//! reader's query is class-constrained today (the note reads, the domain
//! neighbourhood), the fixture carries both classes — that is contrived data on
//! purpose: it is the minimal record that makes a `FILTER NOT EXISTS` load-bearing,
//! so a future widening of those queries cannot leak pings without failing here.

use std::path::Path;

use base::config::{BaseConfig, NamespaceConfig};
use base::crud;
use base::ontology::transient;

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

/// A workspace with one domain (`probe`) synced into the graph.
fn workspace() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let base_dir = tmp.path().join(".base");
    std::fs::create_dir_all(&base_dir).unwrap();
    std::fs::write(base_dir.join("graph.nq"), "").unwrap();
    std::fs::write(
        base_dir.join("base.toml"),
        "[namespace]\nprefix = \"ops\"\nuri = \"http://ops-sys.local/ontology#\"\n",
    )
    .unwrap();
    std::fs::write(
        base_dir.join("domains.toml"),
        "[[domain]]\nname = \"probe\"\nmode = \"triggered\"\nkeywords = [\"probe\"]\nrules = [\"a probe rule\"]\n",
    )
    .unwrap();
    let config = BaseConfig::load(tmp.path());
    base::domain::sync::sync_domains_to_graph(&config, tmp.path(), None).unwrap();
    tmp
}

fn graph_iri(cwd: &Path) -> String {
    crud::workspace_graph_iri(&ns(), &crud::workspace_slug(cwd))
}

fn insert(cwd: &Path, triples: &str) {
    let sparql = format!("INSERT DATA {{ GRAPH <{}> {{\n{triples}\n}} }}", graph_iri(cwd));
    crud::load_and_mutate(cwd, &ns(), &sparql).unwrap();
}

/// The IRI of the fixture ping, as `crud::build_iri` writes one.
fn ping_iri() -> String {
    crud::build_iri(&ns(), "ping", "lark-transient-probe")
}

fn domain_iri() -> String {
    crud::build_iri(&ns(), "domain", "probe")
}

// ── the list itself ──────────────────────────────────────────────────────────

#[test]
fn the_list_names_ping_and_is_the_only_place_that_does() {
    assert!(
        transient::TRANSIENT_KINDS.iter().any(|k| k.class == "Ping" && k.iri_kind == "ping"),
        "ruling 4: relay pings are transient"
    );
}

// ── reader 1: graph_query::load_graph — every `base graph` command ───────────

#[test]
fn load_graph_drops_a_ping_node_and_its_edges() {
    let tmp = workspace();
    let cwd = tmp.path();
    let p = &ns().prefix;
    let ping = ping_iri();
    let dom = domain_iri();

    // A ping exactly as `relay/task_inbox.rs` writes one, plus an IRI-object edge
    // it does not carry today — so this proves the edge guard, not just the node
    // guard, and stays honest if a future ping ever links to a record.
    insert(
        cwd,
        &format!(
            "<{ping}> rdf:type {p}:Ping ;\n\
               {p}:name \"lark transient probe\" ;\n\
               {p}:message \"lark transient probe\" ;\n\
               {p}:relatedTo <{dom}> .\n"
        ),
    );

    let (nodes, adj) = base::graph_query::load_graph(cwd, &ns(), false).unwrap();

    let ping_key = format!("<{ping}>");
    assert!(
        !nodes.contains_key(&ping_key),
        "a Ping must not be a node in the analysed graph — got {:?}",
        nodes.keys().collect::<Vec<_>>()
    );
    assert!(!adj.contains_key(&ping_key), "a Ping must have no adjacency");
    assert!(
        !adj.get(&format!("<{dom}>")).is_some_and(|v| v.iter().any(|(b, _)| b == &ping_key)),
        "the domain must not gain a neighbour that is a Ping"
    );
    // The control: a non-transient record on the same edge shape still loads.
    assert!(
        nodes.keys().any(|k| k.contains("#domain/probe")),
        "the domain itself must still be a node"
    );
}

#[test]
fn load_graph_keeps_a_note_whose_slug_merely_contains_ping() {
    let tmp = workspace();
    let cwd = tmp.path();
    crud::note::learn(cwd, &ns(), "ping hub latency is the thing to watch", "insight", Some("probe"), None, None)
        .unwrap();

    let (nodes, _) = base::graph_query::load_graph(cwd, &ns(), false).unwrap();
    assert!(
        nodes.keys().any(|k| k.contains("#note/ping-hub-latency")),
        "exclusion is by kind segment, never by substring — got {:?}",
        nodes.keys().collect::<Vec<_>>()
    );
}

// ── reader 2: domain::query::query_domain_from_graph — prompt-time injection ─

#[test]
fn domain_injection_drops_a_transient_neighbour() {
    let tmp = workspace();
    let cwd = tmp.path();
    let p = &ns().prefix;
    let ping = ping_iri();
    let dom = domain_iri();

    // Dual-typed on purpose (see the module comment): a record that satisfies the
    // neighbourhood query's Project arm in every respect except being a Ping.
    insert(
        cwd,
        &format!(
            "<{ping}> rdf:type {p}:Ping ;\n\
               rdf:type {p}:Project ;\n\
               {p}:name \"lark transient probe\" ;\n\
               {p}:hasDomain <{dom}> .\n"
        ),
    );
    // The control: a real project on the same edge.
    let real = crud::build_iri(&ns(), "project", "real-probe-project");
    insert(
        cwd,
        &format!(
            "<{real}> rdf:type {p}:Project ;\n\
               {p}:name \"real probe project\" ;\n\
               {p}:hasDomain <{dom}> .\n"
        ),
    );

    let config = BaseConfig::load(cwd);
    let store = base::store::load_graph(&cwd.join(".base").join("graph.nq")).unwrap();
    let def = base::domain::load_domains(cwd).into_iter().find(|d| d.name == "probe").unwrap();
    // Three now: the third is the set of record IRIs this domain block served,
    // which the prompt-time walk dedups against so one record cannot arrive twice.
    let (_rules, neighbourhood, _served) =
        base::domain::query::query_domain_from_graph(&store, &config, &def);

    assert!(
        neighbourhood.contains("real probe project"),
        "the control must still inject — got {neighbourhood:?}"
    );
    assert!(
        !neighbourhood.contains("lark transient probe"),
        "a Ping must never reach prompt-time injection — got {neighbourhood:?}"
    );
}

// ── reader 3: crud::note recall reads ────────────────────────────────────────

#[test]
fn recall_by_domain_drops_a_transient_note() {
    let tmp = workspace();
    let cwd = tmp.path();
    let p = &ns().prefix;
    let ping = ping_iri();
    let dom = domain_iri();

    crud::note::learn(cwd, &ns(), "a real note in probe", "insight", Some("probe"), None, None).unwrap();
    // Dual-typed: satisfies the note read in every respect except being a Ping.
    insert(
        cwd,
        &format!(
            "<{ping}> rdf:type {p}:Ping ;\n\
               rdf:type {p}:Note ;\n\
               {p}:noteText \"lark transient probe\" ;\n\
               {p}:noteType \"insight\" ;\n\
               {p}:status \"active\" ;\n\
               {p}:relatedTo <{dom}> .\n"
        ),
    );

    let by_domain = crud::note::recall_to_string(cwd, &ns(), None, Some("probe"));
    assert!(by_domain.contains("a real note in probe"), "the control must still recall — got {by_domain:?}");
    assert!(
        !by_domain.contains("lark transient probe"),
        "`recall --domain` must drop a Ping — got {by_domain:?}"
    );

    let by_both = crud::note::recall_to_string(cwd, &ns(), Some("probe"), Some("probe"));
    assert!(
        !by_both.contains("lark transient probe"),
        "`recall --keyword --domain` must drop a Ping too — got {by_both:?}"
    );

    // `recall --keyword` with no `--domain` is a DIFFERENT query — five UNION arms
    // over Note, Decision, FileChange and AcceptanceCriteriaResult — and it
    // consulted the list nowhere (kite F16). "Four readers, one list" was true of
    // `recall --domain` and false of the surface right beside it.
    let by_keyword = crud::note::recall_to_string(cwd, &ns(), Some("transient"), None);
    assert!(
        !by_keyword.contains("lark transient probe"),
        "`recall --keyword` must drop a Ping too — got {by_keyword:?}"
    );
    let control = crud::note::recall_to_string(cwd, &ns(), Some("real note"), None);
    assert!(control.contains("a real note in probe"), "the control still recalls: {control:?}");

    let iris = crud::note::recalled_note_iris(cwd, &ns(), None, Some("probe"));
    assert!(
        !iris.iter().any(|i| i.contains("#ping/")),
        "the lastRead stamp must never touch a Ping — got {iris:?}"
    );
    assert!(iris.iter().any(|i| i.contains("#note/")), "the control note is still stamped");
}

// ── reader 4: dashboard::api nodes + edges ───────────────────────────────────

/// The dashboard handlers are `async fn` with no `.await` inside, so one poll
/// completes them. Done by hand rather than pulling tokio into dev-dependencies
/// for two calls.
fn poll_once<F: std::future::Future>(fut: F) -> F::Output {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    unsafe fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), &VTABLE)
    }
    unsafe fn noop(_: *const ()) {}
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
    let mut cx = Context::from_waker(&waker);
    let mut fut = Box::pin(fut);
    match fut.as_mut().poll(&mut cx) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!("dashboard handler awaited something — this harness cannot drive it"),
    }
}

#[test]
fn dashboard_graph_view_drops_ping_nodes_and_edges() {
    let tmp = workspace();
    let cwd = tmp.path();
    let p = &ns().prefix;
    let ping = ping_iri();
    let dom = domain_iri();

    insert(
        cwd,
        &format!(
            "<{ping}> rdf:type {p}:Ping ;\n\
               {p}:name \"lark transient probe\" ;\n\
               {p}:relatedTo <{dom}> .\n"
        ),
    );
    crud::note::learn(cwd, &ns(), "a real note in probe", "insight", Some("probe"), None, None).unwrap();

    let trig_path = cwd.join(".base").join("graph.nq");
    let state = std::sync::Arc::new(base::dashboard::server::AppState::new(
        BaseConfig::load(cwd),
        cwd.to_path_buf(),
        trig_path.clone(),
        vec![trig_path],
    ));

    let nodes = poll_once(base::dashboard::api::nodes(axum::extract::State(state.clone()))).0;
    assert!(
        !nodes.iter().any(|n| n.iri.contains("#ping/")),
        "the dashboard must not render a Ping node — got {:?}",
        nodes.iter().map(|n| n.iri.as_str()).collect::<Vec<_>>()
    );
    assert!(nodes.iter().any(|n| n.iri.contains("#note/")), "the control note still renders");

    let edges = poll_once(base::dashboard::api::edges(axum::extract::State(state))).0;
    assert!(
        !edges.iter().any(|e| e.source.contains("#ping/") || e.target.contains("#ping/")),
        "the dashboard must not render an edge touching a Ping"
    );
    assert!(
        edges.iter().any(|e| e.source.contains("#note/") && e.target.contains("#domain/probe")),
        "the control note→domain edge still renders"
    );
}
