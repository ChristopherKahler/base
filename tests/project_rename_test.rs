//! BO-24: `base project rename <old> <new>` renames a project and its same-named domain together, in both tiers, and
//! keeps the old name as an alias. Every test drives the real binary from a workspace of its own, building the store
//! with the commands an operator types.
//!
//! The fixture names are made up: a project `studio` (renamed to `atelier`), with a decision in each tier, a task, a
//! milestone, a rule, a note, a handoff and two prompt keywords, one of them the old name and one a speech-to-text
//! spelling of it (R5); and a project `other` that must come through untouched.

mod seed;

use std::path::{Path, PathBuf};

use seed::{Seed, run_base};

const NS: &str = "http://ops-sys.local/ontology#";

fn fixture() -> (tempfile::TempDir, Seed) {
    let tmp = tempfile::Builder::new().prefix("bo24-").tempdir().expect("temp dir");
    let home = tmp.path().join("home");
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(home.join(".base-gbl").join(".base")).expect("global tier");
    std::fs::create_dir_all(ws.join(".base")).expect("workspace tier");
    (tmp, Seed { home, ws })
}

fn ok(s: &Seed, args: &[&str]) -> (String, String) {
    let (code, out, err) = run_base(s, args);
    assert_eq!(code, 0, "base {args:?} failed\nstdout: {out}\nstderr: {err}");
    (out, err)
}

fn refused(s: &Seed, args: &[&str]) -> String {
    let (code, out, err) = run_base(s, args);
    assert_ne!(code, 0, "base {args:?} should have been refused\nstdout: {out}\nstderr: {err}");
    err
}

fn ws_graph(s: &Seed) -> PathBuf {
    s.ws.join(".base").join("graph.nq")
}

fn gbl_graph(s: &Seed) -> PathBuf {
    s.home.join(".base-gbl").join(".base").join("graph.nq")
}

fn ws_toml(s: &Seed) -> PathBuf {
    s.ws.join(".base").join("domains.toml")
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

/// A graph file's lines, sorted: the same quads whatever order a write dumped them in.
fn quads(p: &Path) -> Vec<String> {
    let mut v: Vec<String> = read(p).lines().filter(|l| !l.trim().is_empty()).map(String::from).collect();
    v.sort();
    v
}

/// Every IRI in `text` that is an ID base builds from the name `name` (the forms R3 re-keys).
fn ids_of(text: &str, name: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in text.split('<').skip(1) {
        let Some(iri) = part.split('>').next() else { continue };
        let Some(rest) = iri.strip_prefix(NS) else { continue };
        let Some((kind, slug)) = rest.split_once('/') else { continue };
        let hit = match kind {
            "project" | "domain" => slug == name,
            "rule" => slug.starts_with(&format!("{name}/")),
            "document" | "handoff" | "graph" | "workspace" | "codemap" => false,
            _ => slug.starts_with(&format!("{name}.")),
        };
        if hit {
            out.push(rest.to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// `.BAK-` files in either tier's `.base`.
fn backups(s: &Seed) -> Vec<String> {
    let mut out = Vec::new();
    for dir in [s.ws.join(".base"), s.home.join(".base-gbl").join(".base"), s.home.join(".base-gbl")] {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.contains(".BAK-") {
                out.push(n);
            }
        }
    }
    out
}

/// The studio fixture, built with the commands an operator types.
fn studio(s: &Seed) {
    let folder = s.ws.join("Documents").join("Studio");
    std::fs::create_dir_all(&folder).expect("project folder");
    std::fs::create_dir_all(s.ws.join("Documents").join("Other")).expect("other folder");
    ok(s, &["project", "add", "-n", "studio", "-p", "Documents/Studio"]);
    ok(s, &["project", "add", "-n", "other", "-p", "Documents/Other"]);
    ok(s, &["domain", "add-trigger", "--domain", "studio", "--keyword", "studio"]);
    ok(s, &["domain", "add-trigger", "--domain", "studio", "--keyword", "studyo"]);
    ok(s, &["decision", "log", "--domain", "studio", "--decision", "Use the blue logo", "--rationale", "brand"]);
    ok(s, &["decision", "--global", "log", "--domain", "studio", "--decision", "Studio shares the global printer", "--rationale", "cost"]);
    ok(s, &["decision", "log", "--domain", "other", "--decision", "Other keeps its name", "--rationale", "control"]);
    ok(s, &["milestone", "add", "--project", "studio", "--name", "Launch"]);
    ok(s, &["task", "add", "--project", "studio", "--name", "Ship the site", "--milestone", "studio.launch"]);
    ok(s, &["rule", "add", "--domain", "studio", "--text", "Studio work uses the brand kit"]);
    ok(s, &["learn", "--text", "The studio prints on Fridays", "--domain", "studio"]);
    let doc = s.ws.join("studio-handoff.md");
    std::fs::write(&doc, "# studio handoff\n").expect("handoff doc");
    ok(s, &["handoff", "create", "--project", "studio", "--doc", &doc.display().to_string()]);
}

/// Example 1: the preview names what would change and writes nothing.
#[test]
fn rename_preview_writes_nothing() {
    let (_tmp, s) = fixture();
    studio(&s);
    let files = [ws_graph(&s), gbl_graph(&s), ws_toml(&s)];
    let before: Vec<Vec<u8>> = files.iter().map(|f| std::fs::read(f).unwrap_or_default()).collect();
    // A preview writes nothing, so it takes no lock: it runs while a rename holds one (this test's own, live,
    // process), and leaves that lock as it was.
    let lock = s.home.join(".base-gbl").join(".base").join("project-rename.lock");
    std::fs::write(&lock, format!("{}\n", std::process::id())).expect("lock");

    let (out, _) = ok(&s, &["project", "rename", "studio", "atelier"]);
    assert_eq!(read(&lock), format!("{}\n", std::process::id()), "the preview touched another rename's lock");
    std::fs::remove_file(&lock).expect("unlock");
    assert!(out.starts_with("PREVIEW (nothing written; add --yes to rename)"), "{out}");
    assert!(out.contains("project   studio -> atelier  (workspace graph)"), "{out}");
    assert!(out.contains("domain    studio -> atelier  ("), "{out}");
    assert!(out.contains("decisions 1") && out.contains("tasks 1") && out.contains("milestones 1") && out.contains("rules 1"), "{out}");
    assert!(out.contains("alias     studio stays as an alias of atelier"), "{out}");
    assert!(out.contains("graph.nq.BAK-") && out.contains("-pre-rename-studio"), "{out}");

    for (f, b) in files.iter().zip(&before) {
        assert_eq!(&std::fs::read(f).unwrap_or_default(), b, "{} changed under a preview", f.display());
    }
    assert!(backups(&s).is_empty(), "a preview took backups: {:?}", backups(&s));
    assert!(!s.home.join(".base-gbl").join(".base").join("project-rename.lock").exists(), "the rename lock was left behind");
}

/// R3: every record keyed by the old name is under the new one, every edge follows, the triple count is unchanged
/// but for the aliases, and the other project is untouched.
#[test]
fn rename_moves_every_record_and_edge() {
    let (_tmp, s) = fixture();
    studio(&s);
    let ws_before = read(&ws_graph(&s));
    let gbl_before = read(&gbl_graph(&s));
    let old_ids: Vec<String> = ids_of(&ws_before, "studio").into_iter().chain(ids_of(&gbl_before, "studio")).collect();
    for want in ["project/studio", "domain/studio", "decision/studio.use-the-blue-logo", "task/studio.ship-the-site", "milestone/studio.launch", "rule/studio/cli-0"] {
        assert!(old_ids.iter().any(|i| i == want), "control: the fixture holds {want}: {old_ids:?}");
    }
    let other_before: Vec<String> = quads(&ws_graph(&s)).into_iter().filter(|l| l.contains("other")).collect();

    ok(&s, &["project", "rename", "studio", "atelier", "--yes"]);
    let ws_after = read(&ws_graph(&s));
    let gbl_after = read(&gbl_graph(&s));

    assert!(ids_of(&ws_after, "studio").is_empty(), "old IDs left in the workspace graph: {:?}", ids_of(&ws_after, "studio"));
    assert!(ids_of(&gbl_after, "studio").is_empty(), "old IDs left in the global graph: {:?}", ids_of(&gbl_after, "studio"));
    for id in &old_ids {
        let moved = id.replacen("studio", "atelier", 1);
        assert!(ws_after.contains(&format!("{NS}{moved}>")) || gbl_after.contains(&format!("{NS}{moved}>")), "{id} did not move to {moved}");
    }

    let alias = format!("<{NS}alias> \"studio\"");
    let aliases = ws_after.lines().chain(gbl_after.lines()).filter(|l| l.contains(&alias)).count();
    assert_eq!(aliases, 2, "one alias on the project, one on the domain:\n{ws_after}");
    assert_eq!(
        ws_before.lines().count() + gbl_before.lines().count() + aliases,
        ws_after.lines().count() + gbl_after.lines().count(),
        "a rename moves quads, it never adds or drops one (besides the aliases)"
    );
    // Edges from records that keep their ID: the note's domain link, the handoff's.
    assert!(ws_after.contains(&format!("<{NS}hasDomain> <{NS}domain/atelier>")), "{ws_after}");
    assert!(ws_after.contains(&format!("<{NS}project> \"atelier\"")), "the handoff's project follows:\n{ws_after}");
    assert!(ws_after.contains(&format!("<{NS}project/atelier> <{NS}name> \"atelier\"")), "{ws_after}");

    let other_after: Vec<String> = quads(&ws_graph(&s)).into_iter().filter(|l| l.contains("other")).collect();
    assert_eq!(other_before, other_after, "the other project changed");
}

/// R3: a decision under the old domain in the global graph is renamed too.
#[test]
fn rename_covers_both_tiers() {
    let (_tmp, s) = fixture();
    studio(&s);
    let decision = "decision/studio.studio-shares-the-global-printer";
    assert!(read(&gbl_graph(&s)).contains(&format!("{NS}{decision}>")), "control: the global tier holds {decision}");
    assert!(!read(&ws_graph(&s)).contains(&format!("{NS}{decision}>")), "control: only the global tier holds it");

    let (out, _) = ok(&s, &["project", "rename", "studio", "atelier", "--yes"]);
    assert!(out.contains("(global graph)"), "the global tier is named in the report:\n{out}");
    let gbl = read(&gbl_graph(&s));
    assert!(gbl.contains(&format!("<{NS}domain/atelier> <{NS}hasDecision> <{NS}decision/atelier.studio-shares-the-global-printer>")), "{gbl}");
    assert!(ids_of(&gbl, "studio").is_empty(), "{gbl}");
}

/// R2: only the renamed domain's name and alias change in domains.toml; comments, order, other domains and the file's
/// CRLF line endings stay byte for byte.
#[test]
fn rename_keeps_domains_toml_byte_for_byte_elsewhere() {
    let (_tmp, s) = fixture();
    studio(&s);
    let hand_written = "# Operator's domains, edited by hand.\r\n\
                        \r\n\
                        [[domain]]\r\n\
                        name = \"alpha\"   # first\r\n\
                        prompt_keywords = [\"a1\", \"a2\"]\r\n\
                        \r\n\
                        [[ domain ]]\r\n\
                        name = \"Studio\"  # display spelling, as `project add -n Studio` writes it\r\n\
                        mode = \"triggered\"\r\n\
                        prompt_keywords = [\r\n    \"studio\",\r\n    \"studyo\",\r\n]\r\n\
                        rules = [{ text = \"name = \\\"studio\\\" inside a rule stays\" }]\r\n\
                        \r\n\
                        # a comment between domains\r\n\
                        [[domain]]\r\n\
                        name = \"other\"\r\n\
                        paths = [\"Documents/Other\"]\r\n";
    std::fs::write(ws_toml(&s), hand_written).expect("domains.toml");

    ok(&s, &["project", "rename", "studio", "atelier", "--yes"]);
    let after = read(&ws_toml(&s));
    let want = hand_written.replace(
        "name = \"Studio\"  # display spelling, as `project add -n Studio` writes it\r\n",
        "name = \"atelier\"  # display spelling, as `project add -n Studio` writes it\r\naliases = [\"studio\"]\r\n",
    );
    assert_eq!(after, want, "only the name line and one alias line change");
}

/// R5: a rename never drops a prompt keyword, the old name included (on Chris's store `vintrix` and `ventrix` are how
/// his speech-to-text spells the name). Pinned in the file, in `domain get`, and in the graph after `domain sync`.
#[test]
fn rename_never_drops_a_keyword() {
    let (_tmp, s) = fixture();
    studio(&s);
    ok(&s, &["project", "rename", "studio", "atelier", "--yes"]);

    let (get, _) = ok(&s, &["domain", "get", "atelier"]);
    assert!(get.contains("Prompt Keywords: studio, studyo"), "{get}");
    assert!(get.contains("Aliases: studio"), "{get}");
    ok(&s, &["domain", "sync"]);
    let ws = read(&ws_graph(&s));
    for kw in ["studio", "studyo"] {
        assert!(ws.contains(&format!("<{NS}domain/atelier> <{NS}promptKeyword> \"{kw}\"")), "keyword {kw} dropped:\n{ws}");
    }
    assert!(ws.contains(&format!("<{NS}domain/atelier> <{NS}alias> \"studio\"")), "sync keeps the alias in the graph");
    assert!(ids_of(&ws, "studio").is_empty(), "sync brought an old ID back: {:?}", ids_of(&ws, "studio"));
}

/// R4 and Example 3: the old name still works for `--domain`, `project get`, `domain get` and `decision show`, lands
/// new writes under the new name, and says so in one line.
#[test]
fn old_name_resolves_as_alias() {
    let (_tmp, s) = fixture();
    studio(&s);
    ok(&s, &["project", "rename", "studio", "atelier", "--yes"]);
    let notice = "studio is now atelier";

    let (out, err) = ok(&s, &["decision", "log", "--domain", "studio", "--decision", "Logged by an old lane", "--rationale", "alias"]);
    assert!(out.contains("slug: atelier.logged-by-an-old-lane"), "{out}");
    assert!(err.contains(notice), "{err}");
    assert!(!read(&ws_graph(&s)).contains(&format!("{NS}decision/studio.")), "a new write landed under the old name");

    let (out, err) = ok(&s, &["project", "get", "studio"]);
    assert!(out.starts_with("Project: atelier"), "{out}");
    assert!(err.contains(notice), "{err}");

    let (out, err) = ok(&s, &["domain", "get", "studio"]);
    assert!(out.starts_with("Domain: atelier"), "{out}");
    assert!(err.contains(notice), "{err}");

    let (out, err) = ok(&s, &["decision", "show", "studio.use-the-blue-logo"]);
    assert!(out.starts_with("Decision: atelier.use-the-blue-logo"), "{out}");
    assert!(out.contains("domain: atelier"), "{out}");
    assert!(err.contains(notice), "{err}");

    let (_, err) = ok(&s, &["learn", "--text", "Atelier keeps a supply list", "--domain", "atelier", "--project", "studio"]);
    assert!(err.contains(notice), "{err}");
    assert!(read(&ws_graph(&s)).contains(&format!("<{NS}relatedTo> <{NS}project/atelier>")), "the note links the renamed project");
    assert!(!read(&ws_graph(&s)).contains(&format!("<{NS}project/studio>")), "an edge to the old project ID came back");

    let (out, err) = ok(&s, &["task", "get", "studio.ship-the-site"]);
    assert!(out.contains("atelier.ship-the-site"), "{out}");
    assert!(err.contains(notice), "{err}");

    let doc = s.ws.join("late-handoff.md");
    std::fs::write(&doc, "# late\n").expect("doc");
    let (_, err) = ok(&s, &["handoff", "create", "--project", "studio", "--doc", &doc.display().to_string()]);
    assert!(err.contains(notice), "{err}");
    assert!(!read(&ws_graph(&s)).contains(&format!("<{NS}project> \"studio\"")), "a handoff was filed under the old name");

    // The new name answers directly, with no notice.
    let (out, err) = ok(&s, &["project", "get", "atelier"]);
    assert!(out.starts_with("Project: atelier") && !err.contains(notice), "{out}\n{err}");
}

/// R7 and Example 4: a taken name, a bad name, an unknown old name and a rename already running are refused, and
/// nothing is written.
#[test]
fn rename_refuses_taken_or_bad_names() {
    let (_tmp, s) = fixture();
    studio(&s);
    // Written with capitals and never synced: only its slug says it is `domain/taken-domain`.
    ok(&s, &["domain", "create", "--name", "Taken-Domain"]);
    let files = [ws_graph(&s), gbl_graph(&s), ws_toml(&s)];
    let before: Vec<Vec<u8>> = files.iter().map(|f| std::fs::read(f).unwrap_or_default()).collect();

    let err = refused(&s, &["project", "rename", "studio", "other", "--yes"]);
    assert!(err.contains("Error: 'other' is already a project"), "{err}");
    let err = refused(&s, &["project", "rename", "studio", "taken-domain", "--yes"]);
    assert!(err.contains("'taken-domain' is already a domain"), "{err}");
    let err = refused(&s, &["project", "rename", "studio", "Not A Slug", "--yes"]);
    assert!(err.contains("'Not A Slug' is not a valid name") && err.contains("'not-a-slug'"), "{err}");
    let err = refused(&s, &["project", "rename", "ghost", "atelier", "--yes"]);
    assert!(err.contains("no project 'ghost'"), "{err}");
    let err = refused(&s, &["project", "rename", "studio", "studio", "--yes"]);
    assert!(err.contains("nothing to rename"), "{err}");

    // Another rename holds the lock: this test's own process, which is alive.
    let lock = s.home.join(".base-gbl").join(".base").join("project-rename.lock");
    std::fs::write(&lock, format!("{}\n", std::process::id())).expect("lock");
    let err = refused(&s, &["project", "rename", "studio", "atelier", "--yes"]);
    assert!(err.contains("another `base project rename` is running") && err.contains(&std::process::id().to_string()), "{err}");
    std::fs::remove_file(&lock).expect("unlock");

    for (f, b) in files.iter().zip(&before) {
        assert_eq!(&std::fs::read(f).unwrap_or_default(), b, "{} changed under a refusal", f.display());
    }
    assert!(backups(&s).is_empty(), "a refusal took backups: {:?}", backups(&s));

    // Once renamed, the old name is not a new project's name either.
    ok(&s, &["project", "rename", "studio", "atelier", "--yes"]);
    let err = refused(&s, &["project", "add", "-n", "studio", "-p", "Documents/Studio"]);
    assert!(err.contains("'studio' is an old name of project 'atelier'"), "{err}");
    let err = refused(&s, &["project", "rename", "studio", "atelier2", "--yes"]);
    assert!(err.contains("'studio' is an old name of project 'atelier'"), "{err}");
}

/// R6: a failure after the first file puts back what was written, and says so; the backups are there.
#[test]
fn rename_failure_leaves_store_whole() {
    let (_tmp, s) = fixture();
    studio(&s);
    let (ws_before, gbl_before, toml_before) = (quads(&ws_graph(&s)), quads(&gbl_graph(&s)), std::fs::read(ws_toml(&s)).expect("toml"));
    // The domains.toml write goes through `domains.toml.tmp`; a folder there makes it fail after both graphs landed.
    std::fs::create_dir_all(s.ws.join(".base").join("domains.toml.tmp")).expect("blocker");

    let err = refused(&s, &["project", "rename", "studio", "atelier", "--yes"]);
    assert!(err.contains("rename stopped writing") && err.contains("domains.toml"), "{err}");
    assert!(err.contains("Put back as they were:") && err.contains("graph.nq"), "{err}");
    assert!(err.contains("The store is as it was before the rename."), "{err}");

    assert_eq!(quads(&ws_graph(&s)), ws_before, "the workspace graph was not put back");
    assert_eq!(quads(&gbl_graph(&s)), gbl_before, "the global graph was not put back");
    assert_eq!(std::fs::read(ws_toml(&s)).expect("toml"), toml_before, "domains.toml changed");
    assert_eq!(backups(&s).len(), 3, "a backup of each file it meant to write: {:?}", backups(&s));
    assert!(!s.home.join(".base-gbl").join(".base").join("project-rename.lock").exists(), "the rename lock was left behind");

    // Nothing is half-renamed: the old name still answers without a notice, the new one does not exist.
    let (out, err) = ok(&s, &["project", "get", "studio"]);
    assert!(out.starts_with("Project: studio") && !err.contains("is now"), "{out}\n{err}");
}

/// Edges whose object is a record ID (project, domain, decision, task, milestone, rule) that no quad types: what a
/// botched re-key leaves behind.
fn dangling(texts: &[String]) -> usize {
    let typed: std::collections::HashSet<String> = texts
        .iter()
        .flat_map(|t| t.lines())
        .filter(|l| l.contains("<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>"))
        .filter_map(|l| l.split('>').next().map(|s| s.trim_start_matches('<').to_string()))
        .collect();
    let mut n = 0;
    for l in texts.iter().flat_map(|t| t.lines()) {
        let parts: Vec<&str> = l.split("> <").collect();
        if parts.len() < 3 || l.contains("rdf-syntax-ns#type") {
            continue;
        }
        let object = parts[2].trim_start_matches('<');
        let kind = object.strip_prefix(NS).and_then(|r| r.split_once('/')).map(|(k, _)| k);
        if matches!(kind, Some("project" | "domain" | "decision" | "task" | "milestone" | "rule")) && !typed.contains(object) {
            n += 1;
        }
    }
    n
}

fn orphans(doctor_json: &str) -> usize {
    let v: serde_json::Value = serde_json::from_str(doctor_json).unwrap_or_else(|e| panic!("doctor --json: {e}\n{doctor_json}"));
    let mut n = 0;
    for tier in v["tiers"].as_array().expect("tiers") {
        assert_eq!(tier["status"], "healthy", "{tier}");
        for pair in tier["domain_orphans"].as_array().expect("domain_orphans") {
            n += pair[1].as_u64().expect("count") as usize;
        }
    }
    n
}

/// R8: `base doctor` shows no new orphans and the rename leaves no dangling edge.
#[test]
fn doctor_clean_after_rename() {
    let (_tmp, s) = fixture();
    studio(&s);
    let (before, _) = run_doctor(&s);
    let dangling_before = dangling(&[read(&ws_graph(&s)), read(&gbl_graph(&s))]);

    ok(&s, &["project", "rename", "studio", "atelier", "--yes"]);
    let (after, text) = run_doctor(&s);
    let dangling_after = dangling(&[read(&ws_graph(&s)), read(&gbl_graph(&s))]);
    assert_eq!(orphans(&after), orphans(&before), "doctor orphans changed:\nbefore {before}\nafter {after}");
    assert_eq!(dangling_after, dangling_before, "the rename left dangling edges");
    assert!(!text.contains("studio"), "doctor still names the old project:\n{text}");
}

fn run_doctor(s: &Seed) -> (String, String) {
    let (_, json, _) = run_base(s, &["doctor", "--json"]);
    let (_, text, _) = run_base(s, &["doctor"]);
    (json, text)
}

/// A workspace with no global tier: the rename works, and creates no global tier on the way (a preview least of all).
#[test]
fn rename_without_a_global_tier_creates_none() {
    let tmp = tempfile::Builder::new().prefix("bo24-").tempdir().expect("temp dir");
    let s = Seed { home: tmp.path().join("home"), ws: tmp.path().join("ws") };
    std::fs::create_dir_all(s.ws.join(".base")).expect("workspace tier");
    std::fs::create_dir_all(&s.home).expect("home");
    std::fs::create_dir_all(s.ws.join("Documents").join("Studio")).expect("folder");
    ok(&s, &["project", "add", "-n", "studio", "-p", "Documents/Studio"]);
    ok(&s, &["decision", "log", "--domain", "studio", "--decision", "Use the blue logo", "--rationale", "brand"]);
    let global = s.home.join(".base-gbl").join(".base");
    assert!(!global.exists(), "control: the fixture has no global tier");

    let (out, _) = ok(&s, &["project", "rename", "studio", "atelier"]);
    assert!(out.contains("project   studio -> atelier  (workspace graph)") && !out.contains("global graph"), "{out}");
    assert!(!global.exists(), "the preview created a global tier");
    ok(&s, &["project", "rename", "studio", "atelier", "--yes"]);
    assert!(!global.exists(), "the rename created a global tier");
    assert!(read(&ws_graph(&s)).contains(&format!("{NS}decision/atelier.use-the-blue-logo>")));
    assert!(!s.ws.join(".base").join("project-rename.lock").exists(), "the rename lock was left behind");
}
