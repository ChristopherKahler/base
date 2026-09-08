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
//! Adding a site to an allow-list is a deliberate act that shows up in review.
//! Both lists are PRINTED on every run, so an exemption is visible rather than
//! assumed.
//!
//! # Two rules, because matching call NAMES was itself the blind spot (#104)
//!
//! The first version of this test recognised three write calls and nothing else.
//! `src/graph_move.rs` uses none of them — it has its own `fs::write` →
//! `fs::rename` path — so the tripwire **never opened that file**. Not "opened it
//! and passed it": the file-level filter skipped it before any function was
//! examined. `doctor::restore_tier` is the same shape behind a file-scoped
//! exemption. Measured on `610636e`, the unwidened test scanned 99 files, checked
//! 10 functions and reported all-clear while TWELVE unlocked whole-file graph
//! writers sat in the tree.
//!
//! So:
//!
//! * **Rule 1 — write calls.** A function reaching the store seam's write must
//!   hold the lock, unless its FILE is on [`ALLOW_FILES`]. File scope is right
//!   here: `src/dashboard/api.rs` is one deliberate entry covering 17 sites.
//! * **Rule 2 — filesystem primitives.** A function that puts bytes over a path
//!   it names as a graph must hold the lock, unless the (FILE, FUNCTION) pair is
//!   on [`ALLOW_FNS`].
//!
//! **Rule 2's exemptions are function-scoped and that is load-bearing.** A
//! file-scoped exemption is precisely what would have let `restore_tier` ship
//! unlocked inside `doctor.rs` while the tripwire certified that same file green.
//! Repeating file scope in the wider rule would rebuild the hole one level up.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Calls that write a graph file back to disk through the store seam.
const WRITE_CALLS: [&str; 3] = ["write_back(", "update_and_write(", "mutate_and_write("];

/// Filesystem primitives that can put bytes over a graph file WITHOUT touching
/// the seam. Enumerated from the codebase, not from imagination: these are the
/// primitives the #104 route sweep counted across `src/` (211 call-site lines at
/// `610636e`), plus `fs::remove_file`, because deleting a graph is a whole-file
/// write of zero bytes.
const FS_PRIMITIVES: [&str; 7] = [
    "fs::write(",
    "fs::rename(",
    "fs::copy(",
    "fs::remove_file(",
    "File::create(",
    "OpenOptions",
    "BufWriter::new(",
];

/// Anything that puts the graph lock in scope for the enclosing function.
const LOCK_TOKENS: [&str; 5] = [
    "with_graph_lock(",
    "lock_graph(",
    "locked_update(",
    "lock_and_load(",
    "mutate_file_if_holds(",
];

/// Spellings by which a function names a graph FILE, for Rule 2.
///
/// `".nq"` covers every path literal (`graph.nq`, `dest.nq`). `"\"nq."` covers
/// the temp-file spelling `with_extension("nq.tmp")` — and that one is not
/// decoration: **`doctor::restore_tier`, 4B in #104, is reached by NO other
/// signal.** Its only tell is `path.with_extension("nq.tmp")`; it names no
/// graph-path variable and calls no graph function. Drop that entry and the
/// widened rule goes blind to the exact site it was widened for.
///
/// The rest are the resolvers a graph path arrives through. `lock_path(`
/// deliberately is NOT here: the relay has a `lock_path` of its own over its
/// spool, and including it flagged `relay::with_lock`, which touches no graph.
const GRAPH_SIGNALS: [&str; 10] = [
    ".nq",
    "\"nq.",
    "graph_path",
    "dest_path",
    "source_path",
    "all_tier_files",
    "tier_paths(",
    "graph_health(",
    "load_graph(",
    "load_or_empty(",
];

/// Rule 1: files allowed to reach the seam's write without holding the lock.
/// Every entry is a deliberate exemption, not an oversight.
const ALLOW_FILES: [(&str, &str); 12] = [
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

/// Rule 2: (file, function) pairs allowed to call a filesystem primitive while
/// naming a graph path. **Every reason names the file the function actually
/// writes** — an exemption that cannot say what it writes is not an exemption,
/// it is a hole.
const ALLOW_FNS: [(&str, &str, &str); 9] = [
    (
        "src/store.rs",
        "write_back_inner",
        "the seam itself: this IS the atomic temp+rename that every locked write ends in",
    ),
    (
        "src/graph_move.rs",
        "write_validated",
        "#87: writes the graph by its own fs::write + fs::rename path. Takes the lock when the \
         nine Tier C sites migrate; this entry is removed then",
    ),
    (
        "src/graph_move.rs",
        "commit",
        "#87: rollback copies snapshots over BOTH graphs. Takes the lock with the migration; \
         this entry is removed then",
    ),
    (
        "src/doctor.rs",
        "restore_tier",
        "#87: fs::copy + fs::rename straight over the graph. Takes the lock with the migration; \
         this entry is removed then",
    ),
    (
        "src/doctor.rs",
        "repair_tier",
        "writes the .quarantine-<stamp> sidecar, NOT the graph. Its graph write is write_back \
         at :793 and is Rule 1's (#87)",
    ),
    (
        "src/graph.rs",
        "auto_compact_tiers",
        "writes the .last-auto-compact cooldown marker, NOT the graph. Its graph write is \
         compact_tier → write_back at :45 and is Rule 1's (#87)",
    ),
    (
        "src/install.rs",
        "create_global_tier",
        "writes base.toml, standards.toml and the seed docs. Writes no graph",
    ),
    (
        "src/relay/mod.rs",
        "export_nq",
        "writes inbox.nq, an ephemeral read-only export of the relay spool — never a tier graph",
    ),
    (
        "src/scaffold.rs",
        "run",
        "writes domains.toml and base.toml into a new workspace. Writes no graph",
    ),
];

/// The three unlocked filesystem-primitive graph writers on the tree #104 was
/// filed against, by name.
///
/// This list is #104's acceptance leg and it is the reason the instrument had to
/// be fixed before #87: it is derived from a census of the tree, independently of
/// the detector, so a detector that quietly exempts everything cannot satisfy it.
/// **It expires when #87 lands** — those three take the lock, and this test
/// inverts to asserting they are no longer offenders.
const KNOWN_UNLOCKED_FS_WRITERS: [&str; 3] = [
    "src/graph_move.rs::write_validated",
    "src/graph_move.rs::commit",
    "src/doctor.rs::restore_tier",
];

/// One realistic usage per primitive in [`FS_PRIMITIVES`], for the reach test.
/// Law 31: a guard's own suite cannot supply the list of shapes it must reach —
/// these come from the primitives measured across `src/`, and the reach test
/// asserts this table's keys ARE `FS_PRIMITIVES`, so the two cannot drift apart.
const FS_PRIMITIVE_SHAPES: [(&str, &str); 7] = [
    ("fs::write(", "std::fs::write(&tmp, bytes)?;"),
    ("fs::rename(", "std::fs::rename(&tmp, path)?;"),
    ("fs::copy(", "std::fs::copy(backup, &tmp)?;"),
    ("fs::remove_file(", "let _ = std::fs::remove_file(path);"),
    ("File::create(", "let f = std::fs::File::create(&tmp)?;"),
    ("OpenOptions", "let f = std::fs::OpenOptions::new().write(true).open(path)?;"),
    ("BufWriter::new(", "let w = std::io::BufWriter::new(f);"),
];

// ─── the scan ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rule {
    WriteCall,
    FsPrimitive,
}

impl Rule {
    fn label(self) -> &'static str {
        match self {
            Rule::WriteCall => "rule 1, write call",
            Rule::FsPrimitive => "rule 2, fs primitive",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Finding {
    site: String,
    rule: Rule,
    token: String,
}

/// What the scan actually visited. Printed every run, and asserted non-zero:
/// a count of zero means the scan proved NOTHING and is a failure, never a pass.
#[derive(Debug, Default, Clone, Copy)]
struct Counts {
    files_scanned: usize,
    functions_visited: usize,
    test_functions_skipped: usize,
    write_call_functions_checked: usize,
    fs_primitive_sites_seen: usize,
    fs_primitive_graph_functions: usize,
    fs_primitive_functions_flagged: usize,
}

struct Func {
    name: String,
    body: String,
    is_test: bool,
}

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

/// Split a file into functions. Deliberately crude: it splits on `fn` lines,
/// which is enough to ask "does the function containing this write also take the
/// lock?" without carrying a parser.
///
/// It also marks test code, which the previous version's comment CLAIMED to do
/// and did not — the code beneath that comment skipped comment LINES, not test
/// modules. Harmless while only three named calls were matched; fatal with
/// filesystem primitives in scope, because **626 of the 1,903 functions in
/// `src/` are test code** and fixtures legitimately write graph files directly.
/// `src/graph_move.rs` alone carries 8 `fs::write` calls past its
/// `#[cfg(test)]`, and `src/hook/session_start.rs` — the sentinel — writes a
/// `graph.nq` fixture. The cheap way to green that would be to re-add files to
/// an allow-list, which re-blinds the exact files under repair.
fn functions(src: &str) -> Vec<Func> {
    let lines: Vec<&str> = src.lines().collect();
    let cfg_test_at = lines
        .iter()
        .position(|l| l.trim_start().starts_with("#[cfg(test)]"));

    let mut out: Vec<Func> = Vec::new();
    let mut name = String::from("<file scope>");
    let mut body = String::new();
    let mut is_test = false;
    let mut pending_test_attr = false;

    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        let is_fn =
            t.starts_with("fn ") || t.starts_with("pub fn ") || t.starts_with("pub(crate) fn ");
        if is_fn {
            out.push(Func {
                name: name.clone(),
                body: std::mem::take(&mut body),
                is_test,
            });
            name = t
                .split("fn ")
                .nth(1)
                .and_then(|r| r.split(['(', '<']).next())
                .unwrap_or("?")
                .to_string();
            // Everything after a `#[cfg(test)]` line is test code, as is anything
            // directly under a `#[test]` / `#[tokio::test]` attribute run.
            is_test = pending_test_attr || cfg_test_at.is_some_and(|c| i > c);
            pending_test_attr = false;
        } else if t.starts_with("#[") {
            if t.contains("test]") {
                pending_test_attr = true;
            }
        } else if !t.is_empty() && !t.starts_with("//") {
            pending_test_attr = false;
        }
        body.push('\n');
        body.push_str(line);
    }
    out.push(Func { name, body, is_test });
    out
}

/// The body's non-comment lines.
///
/// Dropping comment lines is not tidiness: in this round's route sweep, three
/// DOC COMMENTS naming a primitive in prose were the entire difference between
/// two counts of the same tree (214 vs 211) — one of them inside
/// `with_graph_lock`'s own documentation.
fn code_only(body: &str) -> String {
    body.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn first_match<'a>(hay: &str, needles: &[&'a str]) -> Option<&'a str> {
    needles.iter().copied().find(|n| hay.contains(n))
}

/// Scan one file. `allow_files` and `allow_fns` are parameters rather than
/// constants so the acceptance leg can run the same detector with them EMPTY —
/// a detector that can only be run with its own exemptions applied cannot be
/// asked whether it sees anything.
fn scan_file(
    rel: &str,
    src: &str,
    allow_files: &BTreeSet<&str>,
    allow_fns: &BTreeSet<(&str, &str)>,
    counts: &mut Counts,
) -> Vec<Finding> {
    let mut found = Vec::new();
    for f in functions(src) {
        if f.is_test {
            counts.test_functions_skipped += 1;
            continue;
        }
        counts.functions_visited += 1;
        let code = code_only(&f.body);
        let locked = first_match(&code, &LOCK_TOKENS).is_some();
        let site = format!("{rel}::{}", f.name);

        // Rule 1 — the seam's write calls, exempted by FILE.
        if let Some(tok) = first_match(&code, &WRITE_CALLS) {
            if !allow_files.contains(rel) {
                counts.write_call_functions_checked += 1;
                if !locked {
                    found.push(Finding {
                        site: site.clone(),
                        rule: Rule::WriteCall,
                        token: tok.to_string(),
                    });
                }
            }
        }

        // Rule 2 — filesystem primitives over a path the function names as a
        // graph, exempted by (FILE, FUNCTION).
        //
        // NOT YET ENFORCED in this commit. The candidates are measured and
        // PRINTED so the red-first run shows the detector naming the very sites
        // it then fails to report as findings — the widening lands next.
        let sites = code
            .lines()
            .filter(|l| first_match(l, &FS_PRIMITIVES).is_some())
            .count();
        counts.fs_primitive_sites_seen += sites;
        if sites > 0 && !locked {
            if let Some(sig) = first_match(&code, &GRAPH_SIGNALS) {
                counts.fs_primitive_graph_functions += 1;
                if !allow_fns.contains(&(rel, f.name.as_str())) {
                    counts.fs_primitive_functions_flagged += 1;
                    println!("  rule 2 candidate (not yet enforced): {site}  [{sig}]");
                }
            }
        }
    }
    found
}

fn scan_tree(
    allow_files: &BTreeSet<&str>,
    allow_fns: &BTreeSet<(&str, &str)>,
) -> (Vec<Finding>, Counts) {
    let root = repo_root();
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    files.sort();

    let mut counts = Counts::default();
    let mut found = Vec::new();
    for path in &files {
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let src = std::fs::read_to_string(path).unwrap_or_default();
        counts.files_scanned += 1;
        found.extend(scan_file(&rel, &src, allow_files, allow_fns, &mut counts));
    }
    (found, counts)
}

fn allow_file_set() -> BTreeSet<&'static str> {
    ALLOW_FILES.iter().map(|(f, _)| *f).collect()
}

fn allow_fn_set() -> BTreeSet<(&'static str, &'static str)> {
    ALLOW_FNS.iter().map(|(f, n, _)| (*f, *n)).collect()
}

fn report(found: &[Finding]) -> String {
    found
        .iter()
        .map(|f| format!("{}  [{}: {}]", f.site, f.rule.label(), f.token))
        .collect::<Vec<_>>()
        .join("\n  ")
}

// ─── the tests ───────────────────────────────────────────────

#[test]
fn every_graph_writer_outside_the_allow_list_takes_the_lock() {
    let allow_files = allow_file_set();
    let allow_fns = allow_fn_set();

    println!("rule 1 allow-list — {} FILES, each an explicit exemption:", ALLOW_FILES.len());
    for (f, why) in ALLOW_FILES {
        println!("  {f}\n      {why}");
    }
    println!(
        "rule 2 allow-list — {} (FILE, FUNCTION) pairs, each naming what it actually writes:",
        ALLOW_FNS.len()
    );
    for (f, n, why) in ALLOW_FNS {
        println!("  {f}::{n}\n      {why}");
    }

    let (found, counts) = scan_tree(&allow_files, &allow_fns);

    println!("tripwire: {} files scanned under src/", counts.files_scanned);
    println!("tripwire: {} non-test functions visited", counts.functions_visited);
    println!("tripwire: {} test functions skipped", counts.test_functions_skipped);
    println!(
        "tripwire: {} write-call function(s) required to hold the lock",
        counts.write_call_functions_checked
    );
    println!(
        "tripwire: {} fs write-primitive site(s) seen in non-test functions",
        counts.fs_primitive_sites_seen
    );
    println!(
        "tripwire: {} unlocked function(s) name a graph path and call a primitive; {} outside the rule 2 allow-list",
        counts.fs_primitive_graph_functions, counts.fs_primitive_functions_flagged
    );

    // Control legs. A zero here means this run proved NOTHING; it is a failure,
    // never a pass. (An earlier isolation tripwire in this project printed PASS
    // having opened zero files.)
    assert!(
        counts.files_scanned > 0,
        "scanned 0 files — this run proved NOTHING about the tree"
    );
    assert!(
        counts.functions_visited > 0,
        "visited 0 non-test functions — this run proved NOTHING about the tree"
    );
    assert!(
        counts.write_call_functions_checked > 0,
        "checked 0 write-call functions — rule 1 proved NOTHING"
    );
    assert!(
        counts.fs_primitive_sites_seen > 0,
        "saw 0 fs write-primitive sites — rule 2 proved NOTHING"
    );

    assert!(
        found.is_empty(),
        "these write a graph without taking the lock, and are not on an allow-list:\n  {}\n\
         Either take the lock (store::with_graph_lock / lock_graph / crud::lock_and_load) or add \
         the entry to ALLOW_FILES (rule 1) or ALLOW_FNS (rule 2) in this test, with a reason that \
         names what it writes.",
        report(&found)
    );
}

/// #104's acceptance leg: the widened rule must actually SEE the unlocked
/// filesystem-primitive writers, by name.
///
/// Run with both allow-lists EMPTY, because the question is what the detector can
/// see, not what it has been told to ignore. A rule too loose (exempts
/// everything) and a rule too narrow (never fires) are both GREEN in the main
/// test — this is the leg that separates them.
#[test]
fn the_widened_rule_sees_the_unlocked_fs_writers() {
    let no_files: BTreeSet<&str> = BTreeSet::new();
    let no_fns: BTreeSet<(&str, &str)> = BTreeSet::new();
    let (found, counts) = scan_tree(&no_files, &no_fns);

    assert!(
        counts.functions_visited > 0,
        "visited 0 non-test functions — this leg proved NOTHING"
    );

    let seen: BTreeSet<&str> = found
        .iter()
        .filter(|f| f.rule == Rule::FsPrimitive)
        .map(|f| f.site.as_str())
        .collect();

    let missed: Vec<&str> = KNOWN_UNLOCKED_FS_WRITERS
        .iter()
        .copied()
        .filter(|w| !seen.contains(w))
        .collect();

    assert!(
        missed.is_empty(),
        "the widened rule does not see {} of the {} known unlocked fs-primitive graph writers:\n  \
         {}\nIt saw {} fs-primitive site(s) in total. A rule that cannot name these is not \
         widened, whatever the suite says.",
        missed.len(),
        KNOWN_UNLOCKED_FS_WRITERS.len(),
        missed.join("\n  "),
        seen.len()
    );
}

/// Law 31: prove the rule REACHES every shape of the thing it claims to guard,
/// with the shapes enumerated from the codebase rather than from this test.
///
/// Law 26's other half is here too: the negative controls. A rule that flagged
/// everything would pass the positive half of this test identically.
#[test]
fn every_fs_primitive_shape_reaches_the_rule() {
    // The shape table cannot drift away from the constant it claims to cover.
    let shape_keys: Vec<&str> = FS_PRIMITIVE_SHAPES.iter().map(|(k, _)| *k).collect();
    assert_eq!(
        shape_keys,
        FS_PRIMITIVES.to_vec(),
        "FS_PRIMITIVE_SHAPES no longer covers FS_PRIMITIVES — a primitive was added to the rule \
         without a shape proving the rule reaches it"
    );

    let no_files: BTreeSet<&str> = BTreeSet::new();
    let no_fns: BTreeSet<(&str, &str)> = BTreeSet::new();

    let scan = |src: &str| -> Vec<Finding> {
        let mut c = Counts::default();
        scan_file("src/probe.rs", src, &no_files, &no_fns, &mut c)
    };

    // Positive: every primitive, in a function that names a graph path.
    for (prim, usage) in FS_PRIMITIVE_SHAPES {
        let src = format!(
            "fn writes_a_graph(path: &Path) -> Result<()> {{\n    \
             let tmp = path.with_extension(\"nq.tmp\");\n    {usage}\n    Ok(())\n}}\n"
        );
        let found = scan(&src);
        assert!(
            found.iter().any(|f| f.rule == Rule::FsPrimitive),
            "rule 2 does not reach the {prim} shape:\n{src}"
        );
    }

    // Negative 1 — a primitive with no graph path in sight is NOT a graph write.
    let unrelated = "fn writes_a_log(p: &Path) -> Result<()> {\n    \
                     std::fs::write(p, \"hello\")?;\n    Ok(())\n}\n";
    assert!(
        scan(unrelated).is_empty(),
        "rule 2 flags a primitive that names no graph path — it is flagging everything, \
         which is indistinguishable from working"
    );

    // Negative 2 — holding the lock is the whole point; it must clear the flag.
    let locked = "fn writes_a_graph(path: &Path) -> Result<()> {\n    \
                  let _g = store::lock_graph(path)?;\n    \
                  let tmp = path.with_extension(\"nq.tmp\");\n    \
                  std::fs::rename(&tmp, path)?;\n    Ok(())\n}\n";
    assert!(
        scan(locked).is_empty(),
        "rule 2 flags a function that DOES hold the lock"
    );

    // Negative 3 — test fixtures legitimately write graph files directly.
    let fixture = "#[cfg(test)]\nmod tests {\n    \
                   fn seed(dir: &Path) {\n        \
                   std::fs::write(dir.join(\"graph.nq\"), \"\").unwrap();\n    }\n}\n";
    assert!(
        scan(fixture).is_empty(),
        "rule 2 flags a #[cfg(test)] fixture — every test module in src/ becomes an offender \
         and the cheap fix re-blinds the files under repair"
    );
}
