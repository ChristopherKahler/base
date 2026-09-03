//! The domain-sync guard: `domains.toml` is synced into the graph only when it
//! is newer than `.domain-sync-ts`, and a good sync must leave the marker NEWER
//! than the toml — on every filesystem.
//!
//! The regression this file exists for: the marker used to be "touched" with an
//! empty write. On NTFS, `fs::write(path, "")` onto an existing empty file does
//! not move LastWriteTime at all (measured 2026-09-03 with a compiled probe on a
//! copy of a real marker), so once a `domains.toml` edit opened the guard it
//! never closed again, and every prompt paid a full rewrite of both graphs. ext4
//! bumps mtime on that call, which is why the Linux suite never saw it — the
//! `existing_empty_marker` test below is the one that fails on Windows with the
//! old code and passes with the new.
//!
//! Every tier is built inside the test's tempdir under `with_thread_home`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use base::config::BaseConfig;
use base::hook::user_prompt_submit::{self, needs_sync_check};

const NEW_RULE: &str = "NEW-RULE-7f3a reached the graph";

fn domains_toml(extra_rule: Option<&str>) -> String {
    let mut rules = vec!["\"Markertest rule one\"".to_string()];
    if let Some(r) = extra_rule {
        rules.push(format!("\"{r}\""));
    }
    format!(
        "[[domain]]\nname = \"markertest\"\nmode = \"triggered\"\nkeywords = [\"markertest\"]\nrules = [{}]\n",
        rules.join(", ")
    )
}

/// Both tiers, the shape `ensure_domain_sync` walks: `<home>/.base-gbl/{domains.toml,.base/}`
/// and `<home>/proj/.base/domains.toml`. Returns (global .base dir, workspace root).
fn tiers(home: &Path) -> (PathBuf, PathBuf) {
    let global = home.join(".base-gbl");
    fs::create_dir_all(global.join(".base")).unwrap();
    fs::write(global.join("domains.toml"), domains_toml(None)).unwrap();
    let ws = home.join("proj");
    fs::create_dir_all(ws.join(".base")).unwrap();
    fs::write(ws.join(".base").join("domains.toml"), domains_toml(None)).unwrap();
    (global.join(".base"), ws)
}

fn mtime(p: &Path) -> SystemTime {
    fs::metadata(p).unwrap().modified().unwrap()
}

fn set_mtime(p: &Path, t: SystemTime) {
    fs::File::options().write(true).open(p).unwrap().set_modified(t).unwrap();
}

fn ago(secs: u64) -> SystemTime {
    SystemTime::now() - Duration::from_secs(secs)
}

/// (toml, marker) for a tier's `.base` dir — global: toml sits one level up.
fn global_pair(global_base: &Path) -> (PathBuf, PathBuf) {
    (global_base.parent().unwrap().join("domains.toml"), global_base.join(".domain-sync-ts"))
}
fn ws_pair(ws: &Path) -> (PathBuf, PathBuf) {
    (ws.join(".base").join("domains.toml"), ws.join(".base").join(".domain-sync-ts"))
}

fn graph_state(base_dir: &Path) -> (Vec<u8>, SystemTime, usize) {
    let g = base_dir.join("graph.nq");
    let log = base_dir.join("changes.jsonl");
    let lines = fs::read_to_string(&log).map(|s| s.lines().filter(|l| !l.trim().is_empty()).count()).unwrap_or(0);
    (fs::read(&g).unwrap(), mtime(&g), lines)
}

fn rule_texts(ws: &Path, config: &BaseConfig) -> Vec<String> {
    let ns = &config.namespace;
    let p = &ns.prefix;
    let domain_iri = format!("{}domain/markertest", ns.uri);
    let results = base::crud::load_and_query(
        ws,
        ns,
        &format!("SELECT ?text WHERE {{ GRAPH ?g {{ <{domain_iri}> {p}:hasRule ?r . ?r {p}:ruleText ?text }} }}"),
    )
    .unwrap();
    let oxigraph::sparql::QueryResults::Solutions(solutions) = results else {
        panic!("expected solutions");
    };
    solutions
        .filter_map(|r| r.ok())
        .filter_map(|row| {
            row.get("text").map(|t| match oxigraph::model::TermRef::from(t) {
                oxigraph::model::TermRef::Literal(l) => l.value().to_string(),
                _ => String::new(),
            })
        })
        .collect()
}

// ─── (1) a sync closes the guard on both tiers ───────────────

#[test]
fn a_sync_closes_the_guard_on_both_tiers_and_the_marker_says_when() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let (global_base, ws) = tiers(tmp.path());
        let config = BaseConfig::default();
        let (gt, gm) = global_pair(&global_base);
        let (wt, wm) = ws_pair(&ws);
        assert!(needs_sync_check(&gt, &gm) && needs_sync_check(&wt, &wm), "precondition: no marker, guard open");

        user_prompt_submit::ensure_domain_sync_pub(&config, &ws);

        for (toml, marker) in [(&gt, &gm), (&wt, &wm)] {
            assert!(marker.exists(), "sync writes {}", marker.display());
            assert!(mtime(marker) > mtime(toml), "the marker must land newer than domains.toml: {}", marker.display());
            assert!(!needs_sync_check(toml, marker), "the guard is closed after a good sync");
            let content = fs::read_to_string(marker).unwrap();
            chrono::DateTime::parse_from_rfc3339(content.trim())
                .unwrap_or_else(|e| panic!("the marker carries the sync time as RFC 3339, got {content:?}: {e}"));
        }
    });
}

// ─── (2) the NTFS regression: an existing empty marker ───────

#[test]
fn an_existing_empty_marker_older_than_domains_toml_is_closed_by_the_next_sync() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let (global_base, ws) = tiers(tmp.path());
        let config = BaseConfig::default();
        let pairs = [global_pair(&global_base), ws_pair(&ws)];
        // Exactly the state found on the operator's machine: a 0-byte marker from
        // a sync months ago, and a domains.toml edited since.
        for (toml, marker) in &pairs {
            fs::write(marker, "").unwrap();
            set_mtime(marker, ago(10_000));
            set_mtime(toml, ago(5_000));
            assert!(needs_sync_check(toml, marker), "precondition: the guard is open");
        }

        user_prompt_submit::ensure_domain_sync_pub(&config, &ws);

        for (toml, marker) in &pairs {
            assert!(
                mtime(marker) > mtime(toml),
                "an existing empty marker must still move past domains.toml (NTFS leaves an empty rewrite's mtime untouched): {}",
                marker.display()
            );
            assert!(!needs_sync_check(toml, marker), "the guard closes: {}", marker.display());
        }
    });
}

// ─── (3) a closed guard writes nothing ───────────────────────

#[test]
fn a_second_call_with_the_guard_closed_writes_no_graph_and_no_record() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let (global_base, ws) = tiers(tmp.path());
        let config = BaseConfig::default();
        user_prompt_submit::ensure_domain_sync_pub(&config, &ws);
        let before = [graph_state(&global_base), graph_state(&ws.join(".base"))];
        assert!(before.iter().all(|(_, _, lines)| *lines == 1), "precondition: one sync record per tier");

        user_prompt_submit::ensure_domain_sync_pub(&config, &ws);

        let after = [graph_state(&global_base), graph_state(&ws.join(".base"))];
        assert_eq!(before, after, "bytes, mtime and changelog length all unchanged on both tiers");
    });
}

// ─── (4) an edit reopens the guard and the new rule lands ────

#[test]
fn editing_domains_toml_reopens_the_guard_and_the_new_rule_reaches_the_graph() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let (_global_base, ws) = tiers(tmp.path());
        let config = BaseConfig::default();
        user_prompt_submit::ensure_domain_sync_pub(&config, &ws);
        let (wt, wm) = ws_pair(&ws);
        assert!(!needs_sync_check(&wt, &wm), "precondition: closed");
        assert!(!rule_texts(&ws, &config).iter().any(|t| t == NEW_RULE), "precondition: the rule is not there yet");

        // The sync happened a minute ago; the operator edits domains.toml now.
        set_mtime(&wm, ago(60));
        fs::write(&wt, domains_toml(Some(NEW_RULE))).unwrap();
        assert!(needs_sync_check(&wt, &wm), "an edit reopens the guard");

        // The next prompt: the hook syncs before it loads the graph it injects from.
        let event = serde_json::json!({ "prompt": "markertest please" });
        user_prompt_submit::handle(&config, &ws, &event).unwrap();

        assert!(
            rule_texts(&ws, &config).iter().any(|t| t == NEW_RULE),
            "the new rule is in the graph the prompt injection reads from"
        );
        assert!(mtime(&wm) > mtime(&wt), "and the guard is closed again");
        assert!(!needs_sync_check(&wt, &wm));
    });
}
