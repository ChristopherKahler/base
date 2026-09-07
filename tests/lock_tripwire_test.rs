//! Tripwire: no NEW graph writer may bypass the write lock.
//!
//! `graph.nq` is a whole-file read-modify-write. A writer that loads, mutates and
//! writes it back without holding the lock silently destroys a concurrent
//! writer's change and reports success — that is #72/#73/#74, measured on 0.14.1
//! as two of eight rows surviving eight concurrent `fork create`.
//!
//! The eleven hot-path writers now take the lock. The point of this test is the
//! TWELFTH: a writer added later, by someone who does not know the lock exists.
//!
//! It is a source scan rather than a runtime check on purpose. A runtime assert
//! only fires for a call site some test happens to execute, so a new unlocked
//! writer with no test would sail through. Reading the source catches it whether
//! or not anything calls it.
//!
//! Adding a site to the allow-list is a deliberate act that shows up in review.
//! The list is PRINTED on every run, so an exemption is visible rather than
//! assumed.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Calls that write a graph file back to disk.
const WRITE_CALLS: [&str; 3] = ["write_back(", "update_and_write(", "mutate_and_write("];

/// Anything that puts the graph lock in scope for the enclosing function.
const LOCK_TOKENS: [&str; 5] = [
    "with_graph_lock(",
    "lock_graph(",
    "locked_update(",
    "lock_and_load(",
    "mutate_file_if_holds(",
];

/// Files allowed to write a graph without holding the lock, each with the reason.
/// Every entry is a deliberate exemption, not an oversight.
const ALLOW: [(&str, &str); 12] = [
    ("src/store.rs", "the seam itself: it defines the lock and the write"),
    (
        "src/dashboard/api.rs",
        "the dashboard holds a long-lived in-memory Store (server.rs:62,68) that these mutate \
         and write back, so reload-under-lock would diverge that cache from the file. It is \
         being deprecated and is not getting the lock (Chris, 2026-09-07)",
    ),
    ("src/extract/mod.rs", "one-shot bulk rebuild (#87)"),
    ("src/extract/paul_toml.rs", "one-shot bulk rebuild (#87)"),
    ("src/extension/ingest.rs", "one-shot bulk ingest (#87)"),
    ("src/standards/sync.rs", "one-shot bulk sync (#87)"),
    ("src/domain/sync.rs", "one-shot bulk sync (#87)"),
    ("src/apply_ops.rs", "remote op application, bulk (#87)"),
    ("src/doctor.rs", "doctor.repair, whole-file rebuild (#87)"),
    ("src/graph.rs", "graph.compact, whole-file rebuild (#87)"),
    ("src/migrate.rs", "migration, one-shot (#87)"),
    ("src/hook/session_start.rs", "no graph write; listed if a call appears"),
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Split a file into (function name, body) pairs. Deliberately crude: it splits
/// on top-level `fn` lines, which is enough to ask "does the function containing
/// this write also take the lock?" without carrying a parser.
fn functions(src: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut name = String::from("<file scope>");
    let mut body = String::new();
    for line in src.lines() {
        let t = line.trim_start();
        let is_fn = t.starts_with("fn ") || t.starts_with("pub fn ") || t.starts_with("pub(crate) fn ");
        if is_fn {
            out.push((name.clone(), std::mem::take(&mut body)));
            name = t
                .split("fn ")
                .nth(1)
                .and_then(|r| r.split(['(', '<']).next())
                .unwrap_or("?")
                .to_string();
        }
        body.push('\n');
        body.push_str(line);
    }
    out.push((name, body));
    out
}

#[test]
fn every_graph_writer_outside_the_allow_list_takes_the_lock() {
    let root = repo_root();
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    files.sort();
    assert!(!files.is_empty(), "no source files scanned — the tripwire measured nothing");

    let allowed: BTreeSet<&str> = ALLOW.iter().map(|(f, _)| *f).collect();

    println!("tripwire: scanned {} files under src/", files.len());
    println!("allow-list ({} entries), each an explicit exemption:", ALLOW.len());
    for (f, why) in ALLOW {
        println!("  {f}\n      {why}");
    }

    let mut offenders: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for path in &files {
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let src = std::fs::read_to_string(path).unwrap_or_default();
        if !WRITE_CALLS.iter().any(|c| src.contains(c)) {
            continue;
        }
        if allowed.contains(rel.as_str()) {
            continue;
        }
        for (fname, body) in functions(&src) {
            // Skip the test modules: fixtures legitimately write graphs directly.
            let writes = WRITE_CALLS.iter().any(|c| {
                body.lines().any(|l| {
                    let t = l.trim_start();
                    l.contains(c) && !t.starts_with("//") && !t.starts_with("///")
                })
            });
            if !writes {
                continue;
            }
            checked += 1;
            let locked = LOCK_TOKENS.iter().any(|t| body.contains(t));
            if !locked {
                offenders.push(format!("{rel}::{fname}"));
            }
        }
    }

    println!("tripwire: {checked} graph-writing function(s) required to hold the lock");
    assert!(
        checked > 0,
        "the tripwire found no locked writers to check — it would pass on an empty scan"
    );
    assert!(
        offenders.is_empty(),
        "these write a graph without taking the lock, and are not on the allow-list:\n  {}\n\
         Either take the lock (store::with_graph_lock / lock_graph / crud::lock_and_load) or add \
         the file to ALLOW in this test with the reason.",
        offenders.join("\n  ")
    );
}
