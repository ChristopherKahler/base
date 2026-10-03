//! BO-12: `base doctor --fix` repairs what `base doctor` reports (F15, F16, F24), the upgrade path runs the same repair
//! (F15e), and a move between tiers keeps a record's dates (F22c). BO-25 (D18): a correction `--fix` cannot link stays a
//! correction, and doctor offers `--fix` for corrections only when it would link one.
//!
//! Every test drives the real binary on a fake home it builds: a global tier at `<home>/.base-gbl/.base/graph.nq` and a
//! workspace `ws` beside it, whose own graph is `graph/ws/ws`. Each one states its before in the same run, so what it
//! asserts after is a difference and not a claim.

mod seed;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use seed::{files_under, run_base, Seed, NS};

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const XSD_DATETIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";

/// N-Quads for one named graph.
struct Quads {
    graph: String,
    out: String,
}

impl Quads {
    fn ws(slug: &str) -> Self {
        Quads { graph: format!("{NS}graph/ws/{slug}"), out: String::new() }
    }
    fn typ(&mut self, s: &str, ty: &str) -> &mut Self {
        let _ = writeln!(self.out, "<{NS}{s}> <{RDF_TYPE}> <{NS}{ty}> <{}> .", self.graph);
        self
    }
    /// `v` is written with its quotes and backslashes escaped.
    fn lit(&mut self, s: &str, p: &str, v: &str) -> &mut Self {
        let v = v.replace('\\', "\\\\").replace('"', "\\\"");
        let _ = writeln!(self.out, "<{NS}{s}> <{NS}{p}> \"{v}\" <{}> .", self.graph);
        self
    }
    fn date(&mut self, s: &str, p: &str, v: &str) -> &mut Self {
        let _ = writeln!(self.out, "<{NS}{s}> <{NS}{p}> \"{v}\"^^<{XSD_DATETIME}> <{}> .", self.graph);
        self
    }
    fn iri(&mut self, s: &str, p: &str, o: &str) -> &mut Self {
        let _ = writeln!(self.out, "<{NS}{s}> <{NS}{p}> <{NS}{o}> <{}> .", self.graph);
        self
    }
    fn note(&mut self, slug: &str, kind: &str, text: &str, created: &str) -> &mut Self {
        let s = format!("note/{slug}");
        self.typ(&s, "Note").lit(&s, "noteText", text).lit(&s, "noteType", kind).lit(&s, "status", "active");
        self.date(&s, "createdAt", created)
    }
}

/// A fresh fake home under the system temp folder, cleaned first.
fn home(tag: &str) -> Seed {
    let root = std::env::temp_dir().join(format!("base-bo12-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let s = Seed { home: root.join("home"), ws: root.join("home").join("ws") };
    std::fs::create_dir_all(s.home.join(".base-gbl").join(".base")).expect("global tier");
    std::fs::create_dir_all(s.ws.join(".base")).expect("workspace tier");
    s
}

fn ws_graph(s: &Seed) -> PathBuf {
    s.ws.join(".base").join("graph.nq")
}

fn gbl_graph(s: &Seed) -> PathBuf {
    s.home.join(".base-gbl").join(".base").join("graph.nq")
}

fn gbl_toml(s: &Seed) -> PathBuf {
    s.home.join(".base-gbl").join("base.toml")
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("folder");
    std::fs::write(path, text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

fn text(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// Run `base` from the workspace and require it to succeed.
fn ok(s: &Seed, args: &[&str]) -> String {
    let (code, out, err) = run_base(s, args);
    assert_eq!(code, 0, "base {args:?} failed:\n{out}\n{err}");
    out
}

/// Own records enough that every foreign graph in a fixture is the smaller part of its tier.
fn own_records(q: &mut Quads, n: usize) {
    for i in 0..n {
        q.note(&format!("own-note-{i}"), "insight", &format!("own record {i}"), "2026-09-01T10:00:00-05:00");
    }
}

/// The shapes `base doctor` reports on the measured machine, one of each, in both tiers.
fn full_fixture(tag: &str) -> Seed {
    let s = home(tag);
    let mut w = Quads::ws("ws");
    own_records(&mut w, 6);
    // Example 2: a correction that names the record it corrects by slug, and one that names nothing.
    w.typ("decision/base-config.hub-port", "Decision")
        .lit("decision/base-config.hub-port", "name", "the hub port is 7410")
        .lit("decision/base-config.hub-port", "status", "active")
        .date("decision/base-config.hub-port", "createdAt", "2026-09-01T09:00:00-05:00");
    w.note(
        "the-hub-port-is-7420",
        "correction",
        "the hub port is 7420 not 7410 (supersedes base-config.hub-port)",
        "2026-09-02T09:00:00-05:00",
    );
    w.note("that-was-wrong", "correction", "that was wrong, use the other one", "2026-09-02T09:00:00-05:00");
    // F15d: one record a successor already names, one nothing supersedes.
    w.note("old-claim", "insight", "an old claim", "2026-09-01T09:00:00-05:00")
        .lit("note/old-claim", "status", "superseded")
        .iri("note/new-claim", "supersedes", "note/old-claim");
    w.note("new-claim", "insight", "the new claim", "2026-09-03T09:00:00-05:00");
    w.typ("document/stale-doc", "Document").lit("document/stale-doc", "status", "superseded");
    // F15c: another workspace's record, registered nowhere.
    let mut gone = Quads::ws("gone");
    gone.typ("project/gone-project", "Project")
        .lit("project/gone-project", "name", "Gone Project")
        .date("project/gone-project", "createdAt", "2026-08-21T10:03:45-05:00");
    write(&ws_graph(&s), &(w.out + &gone.out));
    // Five backups, the oldest first.
    for i in 0..5 {
        let b = s.ws.join(".base").join(format!("graph.nq.bak-compact-2026-09-2{i}-080000"));
        write(&b, "<http://x/s> <http://x/p> <http://x/o> .\n");
        set_age(&b, 10 - i as u64);
    }
    let mut g = Quads::ws("base-gbl");
    own_records(&mut g, 4);
    g.note("global-lesson", "correction", "never pipe cargo through head", "2026-09-05T09:00:00-05:00");
    write(&gbl_graph(&s), &g.out);
    write(&gbl_toml(&s), "# my settings\n[signal]\nmax_chars = 2000 # the old cap\nenabled = true\n");
    s
}

/// Set a file's modified time `days` days back, so backups order by age the same way on every platform.
fn set_age(path: &Path, days: u64) {
    let when = std::time::SystemTime::now() - std::time::Duration::from_secs(days * 86_400);
    std::fs::File::options()
        .write(true)
        .open(path)
        .and_then(|f| f.set_modified(when))
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

/// Everything `--fix` may change: both tiers' folders and the global base.toml.
fn state(s: &Seed) -> Vec<std::collections::BTreeMap<PathBuf, (u64, std::time::SystemTime)>> {
    vec![
        files_under(&s.ws.join(".base")),
        files_under(&s.home.join(".base-gbl").join(".base")),
        files_under(&s.home.join(".base-gbl")).into_iter().filter(|(p, _)| p.ends_with("base.toml")).collect(),
    ]
}

fn doctor(s: &Seed) -> String {
    run_base(s, &["doctor"]).1
}

/// F15a: the plan names every repair and how many records it touches, and nothing on disk changes without `--yes`.
#[test]
fn fix_prints_plan_without_changing() {
    let s = full_fixture("plan");
    let before = state(&s);
    let doctor_before = doctor(&s);
    for want in ["ANOTHER workspace", "correction(s) name nothing", "supersession disagreement", "legacy: [signal] max_chars"] {
        assert!(doctor_before.contains(want), "control: doctor reports {want:?} before:\n{doctor_before}");
    }
    assert!(doctor_before.contains("`base doctor --fix` plans the repair of"), "doctor points at the repair:\n{doctor_before}");
    assert!(doctor_before.contains("corrections to link to what they correct"), "the workspace has one to link:\n{doctor_before}");

    let plan = ok(&s, &["doctor", "--fix"]);
    for want in [
        "plan (nothing changed yet; add --yes to apply):",
        "move foreign records",
        "gone: project/gone-project (1 Project) ->",
        "foreign-gone.nq (no registered workspace named gone)",
        "link corrections to what they correct",
        "2 found: 1 linked, 1 stays a correction (it names no single record)",
        "note/the-hub-port-is-7420 corrects decision/base-config.hub-port",
        "nothing to do: 1 found, it stays a correction (it names no single record)",
        "supersession disagreement",
        "(by the repair's write)",
        "keep 3, remove 3",
        "[signal] max_chars = 2000 -> [budget] memory_chars = 2000 (memory_chars was unset",
        "nothing changed: base doctor --fix --yes applies this plan",
    ] {
        assert!(plan.contains(want), "the plan does not say {want:?}:\n{plan}");
    }
    assert!(!plan.contains("plain note"), "{plan}");
    assert_eq!(state(&s), before, "a plan wrote:\n{plan}");

    let json = ok(&s, &["doctor", "--fix", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&json).expect("doctor --fix --json");
    assert_eq!(v["applied"], false);
    assert_eq!(state(&s), before, "a JSON plan wrote");
}

/// The lines in `text` whose subject is `<NS><subject>`.
fn lines_about(text: &str, subject: &str) -> std::collections::BTreeSet<String> {
    let head = format!("<{NS}{subject}> ");
    text.lines().filter(|l| l.starts_with(&head)).map(String::from).collect()
}

/// Both tiers' graphs, byte for byte.
fn graphs(s: &Seed) -> (Vec<u8>, Vec<u8>) {
    (std::fs::read(ws_graph(s)).unwrap_or_default(), std::fs::read(gbl_graph(s)).unwrap_or_default())
}

/// F15b under D18 (BO-25), Examples 1 and 2. Replaces BO-12's `fix_links_or_relabels_corrections`: the correction naming
/// `base-config.hub-port` gets the edge, as before; the one naming nothing and the one naming two records stay exactly as
/// they were, still corrections, with no line added or removed. Nothing is deleted.
#[test]
fn fix_links_corrections_and_keeps_the_rest() {
    let s = full_fixture("corrections");
    let mut extra = Quads::ws("ws");
    extra.note("an-insight-a", "insight", "first record", "2026-09-01T09:00:00-05:00");
    extra.note("an-insight-b", "insight", "second record", "2026-09-01T09:00:00-05:00");
    extra.note("two-names", "correction", "see an-insight-a and an-insight-b", "2026-09-04T09:00:00-05:00");
    let graph = ws_graph(&s);
    write(&graph, &(text(&graph) + &extra.out));
    let before = text(&graph);
    let n = |s: &str| format!("<{NS}{s}>");
    let kept = ["note/that-was-wrong", "note/two-names"];
    for k in kept {
        assert!(
            lines_about(&before, k).contains(&format!("{} {} \"correction\" <{NS}graph/ws/ws> .", n(k), n("noteType"))),
            "control: {k} is a correction before:\n{before}"
        );
    }

    let done = ok(&s, &["doctor", "--fix", "--yes"]);
    assert!(done.contains("3 found: 1 linked, 2 stay corrections (they name no single record; 1 names more than one)"), "{done}");
    assert!(!done.contains("plain note"), "{done}");
    let after = text(&graph);
    let has = |line: &str| after.lines().any(|l| l.starts_with(line));
    assert!(has(&format!("{} {} {}", n("note/the-hub-port-is-7420"), n("supersedes"), n("decision/base-config.hub-port"))), "{after}");
    assert!(has(&format!("{} {} {}", n("decision/base-config.hub-port"), n("supersededBy"), n("note/the-hub-port-is-7420"))));
    assert!(has(&format!("{} {} \"superseded\"", n("decision/base-config.hub-port"), n("status"))));
    for k in kept {
        assert_eq!(lines_about(&after, k), lines_about(&before, k), "{k} changed:\n{done}");
    }
    let gbl = text(&gbl_graph(&s));
    assert!(!after.contains("formerNoteType") && !gbl.contains("formerNoteType"), "{after}");
    assert!(gbl.contains(&format!("{} {} \"correction\"", n("note/global-lesson"), n("noteType"))), "{gbl}");
    // Nothing deleted: every line before is still there, but for the other repairs' (the fixture's foreign record moved
    // out, its document's stray status cleared).
    let after_lines: std::collections::BTreeSet<&str> = after.lines().collect();
    let gone: Vec<&str> = before
        .lines()
        .filter(|l| !after_lines.contains(l))
        .filter(|l| !l.contains("graph/ws/gone") && !l.contains("document/stale-doc"))
        .collect();
    assert!(gone.is_empty(), "lines lost: {gone:#?}");
    let report = doctor(&s);
    assert!(report.contains("2 correction(s) name nothing they correct"), "the count line stays:\n{report}");
}

/// Example 3 (BO-25, R3): a store whose only corrections name nothing. Doctor keeps its count line for each tier and
/// does not offer `--fix` for them; `--fix` says there is nothing to do, and `--fix --yes` leaves both graphs byte for
/// byte with no snapshot taken. Control in the same store: one correction that names a record brings the offer back,
/// and `--fix` links exactly that one.
#[test]
fn doctor_does_not_offer_to_fix_unlinkable_corrections() {
    let s = home("unlinkable");
    let mut w = Quads::ws("ws");
    own_records(&mut w, 3);
    w.note("read-the-folder-first", "correction", "Stop guessing at file paths; read the folder first.", "2026-09-02T09:00:00-05:00");
    write(&ws_graph(&s), &w.out);
    let mut g = Quads::ws("base-gbl");
    own_records(&mut g, 2);
    g.note("global-lesson", "correction", "never pipe cargo through head", "2026-09-05T09:00:00-05:00");
    write(&gbl_graph(&s), &g.out);

    let report = doctor(&s);
    assert_eq!(report.matches("1 correction(s) name nothing they correct").count(), 2, "the count line, per tier:\n{report}");
    assert!(!report.contains("`base doctor --fix` plans the repair of"), "doctor offers a repair --fix does not make:\n{report}");

    let before = graphs(&s);
    let plan = ok(&s, &["doctor", "--fix"]);
    assert_eq!(plan.matches("nothing to do: 1 found, it stays a correction (it names no single record)").count(), 2, "{plan}");
    let done = ok(&s, &["doctor", "--fix", "--yes"]);
    assert_eq!(done.matches("nothing to do: 1 found, it stays a correction (it names no single record)").count(), 2, "{done}");
    assert!(graphs(&s) == before, "--fix --yes wrote a graph with nothing to do:\n{done}");
    for p in [ws_graph(&s), gbl_graph(&s)] {
        assert!(base::store::backups(&p).is_empty(), "a snapshot was taken of {} with nothing to do", p.display());
    }

    // Control: a correction that names one record by its slug.
    let mut more = Quads::ws("ws");
    more.note("own-note-0-was-wrong", "correction", "own-note-0 was wrong (supersedes note/own-note-0)", "2026-09-06T09:00:00-05:00");
    write(&ws_graph(&s), &(text(&ws_graph(&s)) + &more.out));
    let report = doctor(&s);
    assert!(report.contains("2 correction(s) name nothing they correct"), "{report}");
    assert!(report.contains("`base doctor --fix` plans the repair of: corrections to link to what they correct"), "{report}");
    let plan = ok(&s, &["doctor", "--fix"]);
    assert!(plan.contains("2 found: 1 linked, 1 stays a correction (it names no single record)"), "{plan}");
    assert!(plan.contains("note/own-note-0-was-wrong corrects note/own-note-0"), "{plan}");
}

/// R3: once `--fix --yes` has linked what it can, a second `--fix` has nothing to do for corrections in either tier, and
/// a second `--fix --yes` leaves both graphs byte for byte.
#[test]
fn fix_twice_leaves_corrections_alone() {
    let s = full_fixture("twice");
    let first = ok(&s, &["doctor", "--fix", "--yes"]);
    assert!(first.contains("2 found: 1 linked, 1 stays a correction (it names no single record)"), "{first}");
    let report = doctor(&s);
    assert_eq!(report.matches("1 correction(s) name nothing they correct").count(), 2, "{report}");
    assert!(!report.contains("corrections to link"), "{report}");

    let plan = ok(&s, &["doctor", "--fix"]);
    assert_eq!(plan.matches("nothing to do: 1 found, it stays a correction (it names no single record)").count(), 2, "{plan}");
    let before = graphs(&s);
    let again = ok(&s, &["doctor", "--fix", "--yes"]);
    assert_eq!(again.matches("nothing to do: 1 found, it stays a correction (it names no single record)").count(), 2, "{again}");
    assert!(!again.contains("corrects decision/"), "a second run linked again:\n{again}");
    assert!(graphs(&s) == before, "a second --fix --yes changed a graph:\n{again}");
}

/// R5: the upgrade path (`base graph migrate --yes`, which runs this same repair) links the correction that names a
/// record and keeps every other one a correction, in both tiers.
#[test]
fn upgrade_keeps_corrections() {
    let s = full_fixture("upgrade-keeps");
    let migrated = ok(&s, &["graph", "migrate", "--yes"]);
    assert!(migrated.contains("base graph migrate --yes\ndone:"), "{migrated}");
    assert!(migrated.contains("2 found: 1 linked, 1 stays a correction (it names no single record)"), "{migrated}");
    assert!(migrated.contains("nothing to do: 1 found, it stays a correction (it names no single record)"), "{migrated}");
    let (ws, gbl) = (text(&ws_graph(&s)), text(&gbl_graph(&s)));
    let n = |s: &str| format!("<{NS}{s}>");
    assert!(ws.contains(&format!("{} {} {}", n("note/the-hub-port-is-7420"), n("supersedes"), n("decision/base-config.hub-port"))), "{ws}");
    assert!(ws.contains(&format!("{} {} \"correction\"", n("note/that-was-wrong"), n("noteType"))), "{ws}");
    assert!(!ws.contains(&format!("{} {} \"insight\"", n("note/that-was-wrong"), n("noteType"))), "{ws}");
    assert!(gbl.contains(&format!("{} {} \"correction\"", n("note/global-lesson"), n("noteType"))), "{gbl}");
    assert!(!ws.contains("formerNoteType") && !gbl.contains("formerNoteType"));
}

/// F15c: a registered, reachable workspace's records move into its own graph; an unregistered one's go to
/// `.base/foreign-<name>.nq`. Both leave this tier, verbatim, dates and all.
#[test]
fn fix_moves_foreign_records() {
    let s = home("foreign");
    let other = s.home.join("other");
    write(&other.join(".base").join("graph.nq"), &{
        let mut q = Quads::ws("other");
        q.note("other-own", "insight", "already in other", "2026-09-01T09:00:00-05:00");
        q.out
    });
    write(&gbl_toml(&s), &format!("[[workspace]]\npath = \"{}\"\n", other.display().to_string().replace('\\', "/")));
    let mut w = Quads::ws("ws");
    own_records(&mut w, 5);
    let mut theirs = Quads::ws("other");
    theirs
        .typ("project/other-project", "Project")
        .lit("project/other-project", "name", "Other Project")
        .date("project/other-project", "createdAt", "2026-08-21T10:03:45-05:00");
    let mut gone = Quads::ws("gone");
    gone.note("gone-note", "insight", "a note from a workspace that is not here", "2026-08-22T10:00:00-05:00");
    write(&ws_graph(&s), &(w.out.clone() + &theirs.out + &gone.out));
    assert!(doctor(&s).contains("ANOTHER workspace"), "control");

    let done = ok(&s, &["doctor", "--fix", "--yes"]);
    let here = text(&ws_graph(&s));
    assert!(!here.contains("graph/ws/other>") && !here.contains("graph/ws/gone>"), "foreign quads left:\n{here}");
    let into_other = text(&other.join(".base").join("graph.nq"));
    let foreign_file = text(&s.ws.join(".base").join("foreign-gone.nq"));
    for line in theirs.out.lines() {
        assert!(into_other.lines().any(|l| l == line), "not moved into the registered workspace verbatim: {line}\n{done}");
    }
    for line in gone.out.lines() {
        assert!(foreign_file.lines().any(|l| l == line), "not moved into foreign-gone.nq verbatim: {line}\n{done}");
    }
    assert!(into_other.contains("already in other"), "the destination's own records survive the move");
    let report = doctor(&s);
    assert!(!report.contains("ANOTHER workspace"), "{report}");
    // A second run finds nothing to move and repeats nothing in the file.
    ok(&s, &["doctor", "--fix", "--yes"]);
    assert_eq!(text(&s.ws.join(".base").join("foreign-gone.nq")), foreign_file);
}

/// F15c's one refusal: a tier whose every quad sits in one graph under another name is this workspace under an earlier
/// folder name (doctor's "most likely renamed"), and moving it out would empty the workspace. It stays.
#[test]
fn fix_leaves_a_renamed_workspace_in_place() {
    let s = home("renamed");
    let mut old = Quads::ws("what-it-used-to-be-called");
    own_records(&mut old, 3);
    write(&ws_graph(&s), &old.out);
    let plan = ok(&s, &["doctor", "--fix"]);
    assert!(plan.contains("left in place") && plan.contains("earlier folder name"), "{plan}");
    ok(&s, &["doctor", "--fix", "--yes"]);
    assert_eq!(text(&ws_graph(&s)).lines().filter(|l| l.contains("what-it-used-to-be-called")).count(), old.out.lines().count());
    assert!(!s.ws.join(".base").join("foreign-what-it-used-to-be-called.nq").exists());
}

/// F15d, both branches, and the reverse disagreement: the edge is added when a record already names this one as what it
/// supersedes, the status is cleared when nothing does, and an edge with no status gets the status.
#[test]
fn fix_resolves_supersession_disagreement() {
    let s = home("supersession");
    let mut w = Quads::ws("ws");
    own_records(&mut w, 2);
    w.note("old-claim", "insight", "an old claim", "2026-09-01T09:00:00-05:00")
        .lit("note/old-claim", "status", "superseded")
        .iri("note/new-claim", "supersedes", "note/old-claim");
    w.note("new-claim", "insight", "the new claim", "2026-09-03T09:00:00-05:00");
    w.typ("document/stale-doc", "Document").lit("document/stale-doc", "status", "superseded");
    w.note("edge-only", "insight", "superseded by edge only", "2026-09-01T09:00:00-05:00")
        .iri("note/edge-only", "supersededBy", "note/new-claim");
    write(&ws_graph(&s), &w.out);
    let before = doctor(&s);
    assert!(before.contains("supersession disagreement: 2 with the status and no edge, 1 with the edge and no status"), "{before}");

    let done = ok(&s, &["doctor", "--fix", "--yes"]);
    for want in [
        "note/old-claim: edge added (note/new-claim supersedes it)",
        "document/stale-doc: status cleared, no replacing record found (now active)",
        "note/edge-only: status added (superseded by note/new-claim)",
    ] {
        assert!(done.contains(want), "{want:?} missing:\n{done}");
    }
    let after = text(&ws_graph(&s));
    let n = |s: &str| format!("<{NS}{s}>");
    assert!(after.contains(&format!("{} {} {}", n("note/old-claim"), n("supersededBy"), n("note/new-claim"))));
    assert!(!after.contains(&format!("{} {} \"superseded\"", n("document/stale-doc"), n("status"))));
    assert!(after.contains(&format!("{} {} \"active\"", n("document/stale-doc"), n("status"))));
    assert!(after.contains(&format!("{} {} \"superseded\"", n("note/edge-only"), n("status"))));
    let report = doctor(&s);
    assert!(!report.contains("supersession disagreement"), "{report}");
}

/// F16, both branches: unset `memory_chars` takes the legacy value; a set one keeps its own and the old key goes. Every
/// comment and every other key stays where it was, and doctor's legacy line goes.
#[test]
fn fix_migrates_signal_max_chars() {
    let moved = home("max-chars-moved");
    write(&ws_graph(&moved), &{
        let mut q = Quads::ws("ws");
        own_records(&mut q, 1);
        q.out
    });
    let before = "# my settings\n[signal]\n# the old cap\nmax_chars = 1500\nenabled = true\n\n# memory next\n[memory]\nmode = \"both\"\n";
    write(&gbl_toml(&moved), before);
    assert!(doctor(&moved).contains("legacy: [signal] max_chars"), "control");
    let done = ok(&moved, &["doctor", "--fix", "--yes"]);
    assert!(done.contains("[signal] max_chars = 1500 -> [budget] memory_chars = 1500 (memory_chars was unset"), "{done}");
    assert_eq!(
        text(&gbl_toml(&moved)),
        "# my settings\n[signal]\n# the old cap\nenabled = true\n\n# memory next\n[memory]\nmode = \"both\"\n\n[budget]\nmemory_chars = 1500\n"
    );
    assert!(!doctor(&moved).contains("legacy: [signal] max_chars"));

    let removed = home("max-chars-removed");
    write(&ws_graph(&removed), &text(&ws_graph(&moved)));
    let before = "# mine\n[signal]\nmax_chars = 2000\n\n[budget]\n# readable\nmemory_chars = 3000\n";
    write(&gbl_toml(&removed), before);
    let done = ok(&removed, &["doctor", "--fix", "--yes"]);
    assert!(done.contains("[signal] max_chars = 2000 removed ([budget] memory_chars = 3000 is set)"), "{done}");
    assert_eq!(text(&gbl_toml(&removed)), "# mine\n[signal]\n\n[budget]\n# readable\nmemory_chars = 3000\n");
    assert!(!doctor(&removed).contains("legacy: [signal] max_chars"));
}

/// F24a: after the repairs each tier is compacted by the existing `base graph compact`, which refuses an unhealthy graph,
/// so it runs on the repaired one: duplicate lines go, a compact snapshot is taken, and the tier parses.
#[test]
fn fix_compacts_after_repair() {
    let s = full_fixture("compact");
    let graph = ws_graph(&s);
    let dup = text(&graph).lines().next().expect("a line").to_string();
    write(&graph, &format!("{}{dup}\n{dup}\n", text(&graph)));
    let lines_before = text(&graph).lines().count();

    let done = ok(&s, &["doctor", "--fix", "--yes"]);
    let after = text(&graph);
    let unique: std::collections::BTreeSet<&str> = after.lines().collect();
    assert_eq!(unique.len(), after.lines().count(), "duplicate lines survived compaction");
    let row = format!("{lines_before} -> {} lines (by the repair's write)", after.lines().count());
    assert!(done.lines().any(|l| l.trim_start().starts_with("compact ") && l.ends_with(&row)), "no `{row}` row:\n{done}");
    let snaps = |p: &Path| -> Vec<String> {
        base::store::backups(p).iter().map(|b| b.path.file_name().unwrap().to_string_lossy().into_owned()).collect()
    };
    assert!(snaps(&graph).iter().any(|n| n.starts_with("graph.nq.bak-fix-")), "the repair took no snapshot first");
    assert_eq!(base::store::graph_health(&graph), base::store::GraphHealth::Healthy);

    // A re-run on the repaired store has nothing to do: no snapshot, and no backup rotated out (review finding 1).
    let before_rerun = snaps(&graph);
    let again = ok(&s, &["doctor", "--fix", "--yes"]);
    assert!(again.contains("already compact") && again.contains("keep 3 (nothing to remove)"), "{again}");
    assert_eq!(snaps(&graph), before_rerun, "a re-run with nothing to fix changed the backups");

    // A tier the repair leaves alone, with duplicate lines, runs the existing `base graph compact`.
    let plain = home("compact-plain");
    let mut w = Quads::ws("ws");
    own_records(&mut w, 2);
    let dup = w.out.lines().next().expect("a line").to_string();
    write(&ws_graph(&plain), &format!("{}{dup}\n", w.out));
    let done = ok(&plain, &["doctor", "--fix", "--yes"]);
    assert!(done.contains("(base graph compact)"), "{done}");
    assert!(snaps(&ws_graph(&plain)).iter().any(|n| n.starts_with("graph.nq.bak-compact-")), "{done}");
}

/// F24b: ten backups and `keep_backups = 3` on a store with nothing to repair or compact, so the run takes no snapshot of
/// its own: the plan names the seven oldest, and the apply removes exactly those. A copy made by hand under another name
/// is never touched.
#[test]
fn fix_keeps_n_backups() {
    let s = home("backups");
    let mut w = Quads::ws("ws");
    own_records(&mut w, 2);
    write(&ws_graph(&s), &w.out);
    write(&gbl_toml(&s), "[graph]\nkeep_backups = 3\n");
    let dir = s.ws.join(".base");
    let mut names = Vec::new();
    for i in 0..10 {
        let name = format!("graph.nq.bak-compact-2026-09-{:02}-080000", 10 + i);
        write(&dir.join(&name), "<http://x/s> <http://x/p> <http://x/o> .\n");
        set_age(&dir.join(&name), 30 - i as u64);
        names.push(name);
    }
    write(&dir.join("graph.nq.BAK-by-hand"), "kept\n");
    let report = doctor(&s);
    assert!(report.contains("keeps 10 backups (0 MB), more than [graph] keep_backups = 3"), "{report}");

    let plan = ok(&s, &["doctor", "--fix", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&plan).expect("json");
    let removed: Vec<String> = v["tiers"]
        .as_array()
        .expect("tiers")
        .iter()
        .find(|t| t["tier"] == "workspace")
        .expect("workspace tier")["backups"]["removed"]
        .as_array()
        .expect("removed")
        .iter()
        .map(|r| Path::new(r[0].as_str().unwrap()).file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    let oldest: Vec<String> = names[..7].to_vec();
    let mut sorted = removed.clone();
    sorted.sort();
    assert_eq!(sorted, oldest, "the plan names the seven oldest");

    ok(&s, &["doctor", "--fix", "--yes"]);
    let left: Vec<String> = base::store::backups(&ws_graph(&s))
        .iter()
        .map(|b| b.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left.len(), 3, "{left:?}");
    for kept in &names[7..] {
        assert!(left.contains(kept), "{kept} was the newest and is gone: {left:?}");
    }
    for gone in &oldest {
        assert!(!dir.join(gone).exists(), "{gone} survived");
    }
    assert!(dir.join("graph.nq.BAK-by-hand").exists(), "a hand-made copy was removed");
    let after = doctor(&s);
    assert!(after.contains("keeps 3 backup(s)") && !after.contains("more than [graph] keep_backups"), "{after}");
}

/// F15e: the upgrade path (`base graph migrate`) plans the same repair `base doctor --fix` plans, row for row, and with
/// `--yes` leaves the store byte for byte as `doctor --fix --yes` does.
#[test]
fn upgrade_path_calls_same_repair() {
    let (a, b) = (full_fixture("upgrade-doctor"), full_fixture("upgrade-migrate"));
    // The domain backfill first on both, so the only thing left between them is the repair.
    ok(&a, &["graph", "migrate"]);
    ok(&b, &["graph", "migrate"]);
    let rows = |out: &str, s: &Seed| -> Vec<String> {
        let root = s.home.parent().unwrap().display().to_string();
        out.lines()
            .skip_while(|l| !l.starts_with("plan ") && !l.starts_with("done:"))
            .filter(|l| !l.starts_with("nothing changed:"))
            .map(|l| l.replace(&root, "<root>").replace(&root.replace('\\', "/"), "<root>"))
            .collect()
    };
    let doctor_plan = ok(&a, &["doctor", "--fix"]);
    let migrate_plan = ok(&b, &["graph", "migrate"]);
    assert!(!rows(&doctor_plan, &a).is_empty());
    assert_eq!(rows(&doctor_plan, &a), rows(&migrate_plan, &b), "\n{doctor_plan}\n---\n{migrate_plan}");

    ok(&a, &["doctor", "--fix", "--yes"]);
    let migrated = ok(&b, &["graph", "migrate", "--yes"]);
    assert!(migrated.contains("base graph migrate --yes\ndone:"), "{migrated}");
    for (pa, pb) in [
        (ws_graph(&a), ws_graph(&b)),
        (gbl_graph(&a), gbl_graph(&b)),
        (gbl_toml(&a), gbl_toml(&b)),
        (a.ws.join(".base").join("foreign-gone.nq"), b.ws.join(".base").join("foreign-gone.nq")),
    ] {
        assert!(pa.exists(), "{} was not written", pa.display());
        assert_eq!(text(&pa), text(&pb), "{} and {} differ", pa.display(), pb.display());
    }
}

/// F22c: a handoff and a fork moved from the global tier to the workspace (BO-11's re-register path, the F22b sweep's
/// commands) keep createdAt, resurfaceAt and lastActive; so do the records `--fix` moves (F15c). A slug the workspace
/// already holds still re-points to now, as a re-register always has.
#[test]
fn moving_a_handoff_keeps_its_dates() {
    let s = home("dates");
    let docs = s.home.join(".base-gbl").join("handoffs");
    let (created, resurface, active) = ("2026-09-21T15:40:00-05:00", "2026-09-22T08:00:00-05:00", "2026-09-29T11:00:00-05:00");
    let mut g = Quads::ws("base-gbl");
    own_records(&mut g, 2);
    for (slug, kind) in [("2026-09-21-1540-meerkat-operator", "handoff"), ("chris-finances-system", "fork")] {
        let h = format!("handoff/{slug}");
        let doc = docs.join(format!("{slug}.md"));
        write(&doc, &format!("# {slug}\n"));
        g.typ(&h, "Handoff")
            .lit(&h, "project", "operator")
            .lit(&h, "kind", kind)
            .lit(&h, "status", "open")
            .lit(&h, "handoffDoc", &doc.display().to_string().replace('\\', "/"))
            .date(&h, "createdAt", created)
            .date(&h, "resurfaceAt", resurface)
            .date(&h, "lastActive", active);
    }
    // An archived record that a new handoff's doc name happens to match: not a move, so its age is not taken.
    let old = "handoff/2026-08-01-old-archived";
    g.typ(old, "Handoff").lit(old, "project", "operator").lit(old, "kind", "handoff").lit(old, "status", "archived");
    g.date(old, "createdAt", created).date(old, "resurfaceAt", resurface).date(old, "lastActive", active);
    write(&docs.join("2026-08-01-old-archived.md"), "# reused name\n");
    write(&gbl_graph(&s), &g.out);
    // A record of another workspace in this one, carrying the same three dates, for `--fix` to move.
    let mut w = Quads::ws("ws");
    own_records(&mut w, 4);
    let mut theirs = Quads::ws("elsewhere");
    theirs
        .typ("handoff/elsewhere-handoff", "Handoff")
        .lit("handoff/elsewhere-handoff", "status", "open")
        .date("handoff/elsewhere-handoff", "createdAt", created)
        .date("handoff/elsewhere-handoff", "resurfaceAt", resurface)
        .date("handoff/elsewhere-handoff", "lastActive", active);
    write(&ws_graph(&s), &(w.out + &theirs.out));

    let doc = |slug: &str| docs.join(format!("{slug}.md")).display().to_string();
    ok(&s, &["handoff", "create", "--project", "operator", "--doc", &doc("2026-09-21-1540-meerkat-operator"),
        "--slug", "2026-09-21-1540-meerkat-operator", "--lane", "meerkat"]);
    ok(&s, &["fork", "create", "--project", "operator", "--doc", &doc("chris-finances-system"), "--slug", "chris-finances-system"]);
    let dates_of = |path: &Path, slug: &str| -> Vec<(String, String)> {
        let subject = format!("<{NS}handoff/{slug}> <{NS}");
        let mut out: Vec<(String, String)> = text(path)
            .lines()
            .filter_map(|l| l.strip_prefix(&subject))
            .filter_map(|rest| {
                let (p, v) = rest.split_once("> \"")?;
                let v = v.split('"').next()?;
                ["createdAt", "resurfaceAt", "lastActive"].contains(&p).then(|| (p.to_string(), v.to_string()))
            })
            .collect();
        out.sort();
        out
    };
    let want = vec![
        ("createdAt".to_string(), created.to_string()),
        ("lastActive".to_string(), active.to_string()),
        ("resurfaceAt".to_string(), resurface.to_string()),
    ];
    assert_eq!(dates_of(&ws_graph(&s), "2026-09-21-1540-meerkat-operator"), want, "the moved handoff's dates");
    assert_eq!(dates_of(&ws_graph(&s), "chris-finances-system"), want, "the moved fork's dates");

    ok(&s, &["handoff", "create", "--project", "operator", "--doc", &doc("2026-08-01-old-archived"), "--lane", "new-work"]);
    assert_ne!(dates_of(&ws_graph(&s), "2026-08-01-old-archived"), want, "a new handoff took an archived record's age");

    // Control: a re-register of a slug this tier already holds re-points it to now.
    ok(&s, &["fork", "create", "--project", "operator", "--doc", &doc("chris-finances-system"), "--slug", "chris-finances-system"]);
    assert_ne!(dates_of(&ws_graph(&s), "chris-finances-system"), want, "a re-register in the same tier kept old dates");

    ok(&s, &["doctor", "--fix", "--yes"]);
    let moved = s.ws.join(".base").join("foreign-elsewhere.nq");
    assert_eq!(dates_of(&moved, "elsewhere-handoff"), want, "--fix moved a record without its dates");
}
