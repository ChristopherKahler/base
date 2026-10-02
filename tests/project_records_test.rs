//! BO-09: projects know their folder, their parent, whether they inherit, and how old their next step is (F25, F23,
//! P5, D13). Every test drives the real binary from a workspace of its own, the way an operator types the commands.
//!
//! The fixture names are made up (`studio`, `studio-client`): a business and a client project inside it, which is
//! D13's shape.

mod seed;

use std::path::{Path, PathBuf};

use seed::{Seed, run_base};

const NS: &str = "http://ops-sys.local/ontology#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const XSD_DATETIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";

/// A home and a workspace beside it, in a temp folder whose name does not start with a dot (`tempfile`'s default
/// `.tmpXXXX` would read as a dot folder to `project paths`, which skips those on purpose).
fn fixture() -> (tempfile::TempDir, Seed) {
    let tmp = tempfile::Builder::new().prefix("bo09-").tempdir().expect("temp dir");
    let home = tmp.path().join("home");
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(home.join(".base-gbl").join(".base")).expect("global tier");
    std::fs::create_dir_all(ws.join(".base")).expect("workspace tier");
    (tmp, Seed { home, ws })
}

/// `p` the way base stores a folder (F25b): `/`-separated, drive letter upper-cased.
fn stored(p: &Path) -> String {
    let s = p.display().to_string().replace('\\', "/");
    let b = s.as_bytes();
    if b.len() >= 2 && b[1] == b':' { format!("{}{}", s[..1].to_ascii_uppercase(), &s[1..]) } else { s }
}

fn ok(s: &Seed, args: &[&str]) -> String {
    let (code, out, err) = run_base(s, args);
    assert_eq!(code, 0, "base {args:?} failed\nstdout: {out}\nstderr: {err}");
    out
}

fn record(s: &Seed, slug: &str) -> serde_json::Value {
    let out = ok(s, &["project", "get", slug, "--json"]);
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("project get --json: {e}\n{out}"))
}

fn same_path(a: &str, b: &str) -> bool {
    if cfg!(windows) { a.eq_ignore_ascii_case(b) } else { a == b }
}

/// `hay` holds `needle`, case-blind on Windows, where one folder has many spellings.
fn contains(hay: &str, needle: &str) -> bool {
    if cfg!(windows) { hay.to_ascii_lowercase().contains(&needle.to_ascii_lowercase()) } else { hay.contains(needle) }
}

/// Example 1 (D13): a business folder and a client folder inside it; the client says nested.
fn add_studio_pair(s: &Seed) -> (PathBuf, PathBuf) {
    let studio = s.ws.join("Documents").join("Studio");
    let client = studio.join("Studio Client");
    std::fs::create_dir_all(&client).expect("folders");
    ok(s, &["project", "add", "-n", "studio", "-p", "Documents/Studio"]);
    ok(s, &["project", "add", "-n", "studio-client", "-p", "Documents/Studio/Studio Client"]);
    (studio, client)
}

/// The workspace graph's IRI, as the workspace writes it.
fn ws_graph(s: &Seed) -> String {
    format!("{NS}graph/ws/{}", base::crud::workspace_slug(&s.ws))
}

fn append_quads(s: &Seed, quads: &str) {
    use std::io::Write;
    let path = s.ws.join(".base").join("graph.nq");
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path).expect("graph.nq");
    f.write_all(quads.as_bytes()).expect("quads written");
}

fn project_quads(s: &Seed, slug: &str, next: Option<(&str, Option<String>)>) -> String {
    let g = ws_graph(s);
    let p = format!("{NS}project/{slug}");
    let mut q = format!(
        "<{p}> <{RDF_TYPE}> <{NS}Project> <{g}> .\n\
         <{p}> <{NS}name> \"{slug}\" <{g}> .\n\
         <{p}> <{NS}status> \"active\" <{g}> .\n"
    );
    if let Some((step, at)) = next {
        q.push_str(&format!("<{p}> <{NS}nextAction> \"{step}\" <{g}> .\n"));
        if let Some(at) = at {
            q.push_str(&format!("<{p}> <{NS}nextActionAt> \"{at}\"^^<{XSD_DATETIME}> <{g}> .\n"));
        }
    }
    q
}

/// F25a, Example 1: `--path`, `--parent` and `--nested` on `update`; the list shows all three.
#[test]
fn project_update_sets_path_parent_nested() {
    let (_tmp, s) = fixture();
    let (studio, client) = add_studio_pair(&s);
    let moved = s.ws.join("Documents").join("Studio").join("Clients").join("Studio Client");
    std::fs::create_dir_all(&moved).expect("folder");

    let out = ok(
        &s,
        &["project", "update", "studio-client", "--path", "Documents/Studio/Clients/Studio Client", "--parent", "studio", "--nested", "true"],
    );
    assert!(out.contains("Project 'studio-client' updated"), "{out}");
    assert!(out.contains(&format!("path: {} → {}", stored(&client), stored(&moved))), "the move is reported: {out}");

    let r = record(&s, "studio-client");
    assert!(same_path(r["path"].as_str().unwrap(), &stored(&moved)), "{r}");
    assert_eq!(r["parent"], "studio");
    assert_eq!(r["nested"], true);
    let parent = record(&s, "studio");
    assert!(same_path(parent["path"].as_str().unwrap(), &stored(&studio)), "{parent}");
    assert_eq!(parent["parent"], serde_json::Value::Null);
    assert_eq!(parent["nested"], false, "nested defaults to false (F25d)");

    // F25e: the table carries path, parent and nested on the child's row.
    let list = ok(&s, &["project", "list", "--all"]);
    assert!(list.contains("| name | status | path | parent | nested | next | lastActive |"), "{list}");
    let row = list.lines().find(|l| l.starts_with("| studio-client |")).unwrap_or_else(|| panic!("{list}"));
    let want = format!("| {} | studio | true |", stored(&moved));
    assert!(contains(row, &want), "{row}\nwants {want}");
    // `--json` carries them too.
    let json: serde_json::Value = serde_json::from_str(&ok(&s, &["project", "list", "--all", "--json"])).expect("json");
    let child = json.as_array().unwrap().iter().find(|r| r["id"] == "studio-client").expect("child in --json");
    assert_eq!((child["parent"].as_str(), child["nested"].as_bool()), (Some("studio"), Some(true)));

    // `--parent none` removes the link.
    ok(&s, &["project", "update", "studio-client", "--parent", "none", "--nested", "false"]);
    let r = record(&s, "studio-client");
    assert_eq!((r["parent"].clone(), r["nested"].clone()), (serde_json::Value::Null, serde_json::Value::Bool(false)));
}

/// An update reaches a project filed under another workspace's graph in this workspace's file (a `project move`
/// leftover, a PAUL project homed elsewhere). Before 0.16.0 `update` wrote only to this workspace's graph, matched
/// nothing, and printed "updated": measured on the operator's store with `seed`, filed under `graph/ws/toolbox`.
#[test]
fn project_update_writes_where_the_project_lives() {
    let (_tmp, s) = fixture();
    let other = format!("{NS}graph/ws/elsewhere");
    let quads = project_quads(&s, "filed-elsewhere", Some(("old step", None))).replace(&ws_graph(&s), &other);
    append_quads(&s, &quads);
    std::fs::create_dir_all(s.ws.join("apps").join("there")).expect("folder");
    ok(&s, &["project", "add", "-n", "holder", "-p", "apps"]);

    ok(
        &s,
        &["project", "update", "filed-elsewhere", "--next-action", "new step", "--path", "apps/there", "--parent", "holder", "--nested", "true"],
    );
    let r = record(&s, "filed-elsewhere");
    assert_eq!(r["next_action"], "new step");
    assert_eq!(r["next_action_age_days"], 0);
    assert!(same_path(r["path"].as_str().unwrap(), &stored(&s.ws.join("apps").join("there"))), "{r}");
    assert_eq!((r["parent"].as_str(), r["nested"].as_bool()), (Some("holder"), Some(true)));

    // Every field landed in the project's own graph, and no second copy of it was started in this workspace's.
    let graph = std::fs::read_to_string(s.ws.join(".base").join("graph.nq")).expect("graph");
    let subject = format!("<{NS}project/filed-elsewhere>");
    let quads: Vec<&str> = graph.lines().filter(|l| l.starts_with(&subject)).collect();
    assert!(quads.iter().all(|l| l.ends_with(&format!("<{other}> ."))), "{quads:#?}");
    for pred in ["nextAction", "nextActionAt", "path", "parentProject", "nested"] {
        assert_eq!(quads.iter().filter(|l| l.contains(&format!("#{pred}>"))).count(), 1, "{pred}: {quads:#?}");
    }
}

/// F25c, Example 1: a link that closes a loop is refused, says which link closes it, and writes nothing.
#[test]
fn project_parent_loop_refused() {
    let (_tmp, s) = fixture();
    add_studio_pair(&s);
    ok(&s, &["project", "update", "studio-client", "--parent", "studio", "--nested", "true"]);

    let (code, out, err) = run_base(&s, &["project", "update", "studio", "--parent", "studio-client"]);
    assert_eq!(code, 1, "refused: {out}{err}");
    assert_eq!(err.trim(), "Error: loop: studio-client already has parent studio");
    assert_eq!(record(&s, "studio")["parent"], serde_json::Value::Null, "nothing was written");

    // A longer loop names every link on the way; a project is never its own parent.
    ok(&s, &["project", "add", "-n", "studio-site", "-p", "Documents/Studio/Studio Client"]);
    ok(&s, &["project", "update", "studio-site", "--parent", "studio-client"]);
    let (code, _, err) = run_base(&s, &["project", "update", "studio", "--parent", "studio-site"]);
    assert_eq!(code, 1);
    assert_eq!(err.trim(), "Error: loop: studio-site already has parent studio-client, studio-client already has parent studio");
    let (code, _, err) = run_base(&s, &["project", "update", "studio", "--parent", "studio"]);
    assert_eq!((code, err.trim()), (1, "Error: loop: studio cannot be its own parent"));
}

/// F25c: a parent must be a registered project, on `update` and on `add`; a refused `add` registers nothing.
#[test]
fn project_parent_must_exist() {
    let (_tmp, s) = fixture();
    add_studio_pair(&s);
    let (code, _, err) = run_base(&s, &["project", "update", "studio-client", "--parent", "no-such-project"]);
    assert_eq!(code, 1);
    assert!(err.starts_with("Error: no project 'no-such-project'"), "{err}");
    assert_eq!(record(&s, "studio-client")["parent"], serde_json::Value::Null);

    let (code, _, err) = run_base(&s, &["project", "add", "-n", "orphan", "-p", "Documents", "--parent", "no-such-project"]);
    assert_eq!(code, 1);
    assert!(err.starts_with("Error: no project 'no-such-project'"), "{err}");
    let all = ok(&s, &["project", "list", "--all", "--json"]);
    assert!(!all.contains("\"orphan\""), "a refused add registers nothing: {all}");

    // And `add --parent` with a real one sets it in the same command.
    let out = ok(&s, &["project", "add", "-n", "studio-shop", "-p", "Documents/Studio", "--parent", "studio", "--nested", "true"]);
    assert!(out.contains("parent: studio"), "{out}");
    let r = record(&s, "studio-shop");
    assert_eq!((r["parent"].as_str(), r["nested"].as_bool()), (Some("studio"), Some(true)));
}

/// F25d: `--nested true` with no parent is allowed, stored, and warns that it does nothing yet.
#[test]
fn nested_without_parent_warns() {
    let (_tmp, s) = fixture();
    add_studio_pair(&s);
    let (code, out, err) = run_base(&s, &["project", "update", "studio", "--nested", "true"]);
    assert_eq!(code, 0, "allowed: {out}{err}");
    assert!(
        err.contains("warning: studio has no parent, so nested = true does nothing until one is set: base project update studio --parent <slug>"),
        "{err}"
    );
    assert_eq!(record(&s, "studio")["nested"], true, "stored anyway");
    // Said once, when the setting is written: an update that touches neither parent nor nested says nothing.
    let (_, _, err) = run_base(&s, &["project", "update", "studio", "--status", "blocked"]);
    assert!(!err.contains("warning"), "{err}");

    // With a parent there is nothing to warn about.
    let (_, _, err) = run_base(&s, &["project", "update", "studio-client", "--parent", "studio", "--nested", "true"]);
    assert!(!err.contains("warning"), "{err}");
}

/// F25b: a relative path is resolved against the workspace root and stored absolute with `/`; `..` and `.` fold;
/// a relative path already in the store (written before 0.16.0) reads back absolute too.
#[test]
fn relative_path_stored_absolute() {
    let (_tmp, s) = fixture();
    std::fs::create_dir_all(s.ws.join("apps").join("site")).expect("folder");
    ok(&s, &["project", "add", "-n", "site", "-p", "apps/./tools/../site"]);
    let want = stored(&s.ws.join("apps").join("site"));
    let r = record(&s, "site");
    let got = r["path"].as_str().unwrap();
    assert!(same_path(got, &want), "{got} vs {want}");
    assert!(!got.contains('\\'), "forward slashes only: {got}");

    // The graph itself holds the absolute form, not just the read.
    let graph = std::fs::read_to_string(s.ws.join(".base").join("graph.nq")).expect("graph");
    let line = graph.lines().find(|l| l.contains("project/site>") && l.contains("#path>")).expect("path quad");
    assert!(contains(line, &format!("\"{want}\"")), "{line}");
    assert!(!line.contains("apps/./tools"), "{line}");

    // `update --path` takes a relative path the same way.
    std::fs::create_dir_all(s.ws.join("apps").join("web")).expect("folder");
    ok(&s, &["project", "update", "site", "--path", "apps/web"]);
    assert!(same_path(record(&s, "site")["path"].as_str().unwrap(), &stored(&s.ws.join("apps").join("web"))));

    // A pre-0.16.0 relative path in the store reads back absolute, against the workspace root.
    append_quads(
        &s,
        &format!(
            "{}<{NS}project/legacy> <{NS}path> \"apps\\\\old\" <{}> .\n",
            project_quads(&s, "legacy", None),
            ws_graph(&s)
        ),
    );
    let legacy = record(&s, "legacy");
    assert!(same_path(legacy["path"].as_str().unwrap(), &stored(&s.ws.join("apps").join("old"))), "{legacy}");

    // Its domain's trigger, written relative the way 0.15 wrote it, moves with the folder: the old spelling is
    // matched by the place it names and replaced, not left beside the new one.
    let toml = s.ws.join(".base").join("domains.toml");
    let mut text = std::fs::read_to_string(&toml).unwrap_or_default();
    text.push_str("\n[[domain]]\nname = \"legacy\"\npaths = [\"apps/./old\"]\n");
    std::fs::write(&toml, text).expect("domains.toml");
    let out = ok(&s, &["project", "update", "legacy", "--path", "apps/web"]);
    assert!(out.contains("domain 'legacy' trigger updated"), "{out}");
    let domains: toml::Table = toml::from_str(&std::fs::read_to_string(&toml).unwrap()).expect("domains.toml");
    let legacy_domain = domains["domain"].as_array().unwrap().iter().find(|d| d["name"].as_str() == Some("legacy")).unwrap();
    let paths: Vec<&str> = legacy_domain["paths"].as_array().unwrap().iter().filter_map(|p| p.as_str()).collect();
    assert_eq!(paths.len(), 1, "{paths:?}");
    assert!(same_path(paths[0], &stored(&s.ws.join("apps").join("web"))), "{paths:?}");

    // A folder that is not there is stored as asked, and said.
    let (code, _, err) = run_base(&s, &["project", "update", "legacy", "--path", "apps/nowhere"]);
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("apps/nowhere does not exist on this machine"), "{err}");
}

/// A PAUL project's folder and next step come from its `.paul`, which the next session start or sync writes again:
/// `update` still writes what it is asked, and says it will not last; doctor points at the file, not at a command
/// whose result would be replaced.
#[test]
fn paul_projects_say_where_their_folder_and_step_come_from() {
    let (_tmp, s) = fixture();
    let phased = s.ws.join("apps").join("phased");
    std::fs::create_dir_all(phased.join(".paul")).expect("folders");
    std::fs::create_dir_all(s.ws.join("apps").join("other")).expect("folders");
    std::fs::write(phased.join(".paul").join("paul.toml"), "name = \"phased\"\n").expect("paul.toml");
    let g = ws_graph(&s);
    append_quads(
        &s,
        &format!(
            "{}<{NS}project/phased> <{NS}path> \"{}\" <{g}> .\n",
            project_quads(&s, "phased", Some(("Phase 2: ship [active]", None))),
            stored(&phased)
        ),
    );
    let paul = format!("{}/.paul/paul.toml", stored(&phased));

    let (_, out, _) = run_base(&s, &["doctor"]);
    let line = out.lines().find(|l| l.contains("project phased:")).unwrap_or_else(|| panic!("{out}"));
    assert!(contains(line, &format!("update: the phase in {paul} (base rewrites this step from it)")), "{line}");

    let (code, _, err) = run_base(&s, &["project", "update", "phased", "--path", "apps/other", "--next-action", "mine"]);
    assert_eq!(code, 0, "{err}");
    assert!(contains(&err, &format!("warning: phased's folder comes from {paul}")), "{err}");
    assert!(contains(&err, &format!("warning: phased's next step comes from {paul}")), "{err}");
    assert_eq!(record(&s, "phased")["next_action"], "mine", "written as asked");
}

/// F23a, F23b, Example 3: writing a next step records when; the list shows its age; an old one shows its days.
#[test]
fn next_action_records_age() {
    let (_tmp, s) = fixture();
    add_studio_pair(&s);
    ok(&s, &["project", "update", "studio", "--next-action", "send the \"site\" draft | review"]);
    let r = record(&s, "studio");
    assert_eq!(r["next_action"], "send the \"site\" draft | review", "quotes survive the write");
    let at = r["next_action_at"].as_str().expect("dated");
    chrono::DateTime::parse_from_rfc3339(at).expect("an RFC 3339 time");
    assert_eq!(r["next_action_age_days"], 0);
    let list = ok(&s, &["project", "list", "--all"]);
    let row = list.lines().find(|l| l.starts_with("| studio |")).unwrap_or_else(|| panic!("{list}"));
    assert!(row.contains("send the \"site\" draft \\| review (0 days)"), "{row}");

    // An undated step (written before 0.16.0) and a 20-day-old one.
    let old = (chrono::Local::now() - chrono::Duration::days(20)).to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    append_quads(&s, &project_quads(&s, "undated-one", Some(("WSL is down (C: hit 0 bytes; boot it", None))));
    append_quads(&s, &project_quads(&s, "old-one", Some(("ship slice 4", Some(old)))));
    let list = ok(&s, &["project", "list", "--all"]);
    assert!(list.lines().any(|l| l.starts_with("| undated-one |") && l.contains("WSL is down (C: hit 0 bytes; boot it (undated)")), "{list}");
    assert!(list.lines().any(|l| l.starts_with("| old-one |") && l.contains("ship slice 4 (20 days)")), "{list}");
    assert_eq!(record(&s, "old-one")["next_action_age_days"], 20);
    assert_eq!(record(&s, "undated-one")["next_action_at"], serde_json::Value::Null);

    // Writing it again dates it today.
    ok(&s, &["project", "update", "undated-one", "--next-action", "boot WSL"]);
    assert_eq!(record(&s, "undated-one")["next_action_age_days"], 0);
}

/// F23b, F23c, Example 3: doctor flags an undated step and one older than `[doctor] stale_next_days` (14), each
/// with the command that rewrites it; a fresh step, a step inside the limit and done work are left alone.
#[test]
fn doctor_flags_stale_next_action() {
    let (_tmp, s) = fixture();
    let ago = |d: i64| (chrono::Local::now() - chrono::Duration::days(d)).to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    append_quads(&s, &project_quads(&s, "undated-one", Some(("WSL is down (C: hit 0 bytes; ext4.vhdx 249 GB): boot it", None))));
    append_quads(&s, &project_quads(&s, "old-one", Some(("ship slice 4", Some(ago(20))))));
    append_quads(&s, &project_quads(&s, "recent-one", Some(("ship slice 5", Some(ago(3))))));
    let done = project_quads(&s, "done-one", Some(("nothing left", None))).replace("\"active\"", "\"complete\"");
    append_quads(&s, &done);

    let (_, out, _) = run_base(&s, &["doctor"]);
    assert!(out.contains("─── project next steps ───"), "{out}");
    assert!(
        out.contains("⚠ project undated-one: next step undated, probably stale: \"WSL is down (C: hit 0 bytes; ...\" · update: base project update undated-one --next-action \"...\""),
        "{out}"
    );
    assert!(
        out.contains("⚠ project old-one: next step 20 days old: \"ship slice 4\" · update: base project update old-one --next-action \"...\""),
        "{out}"
    );
    assert!(!out.contains("recent-one"), "3 days is inside 14: {out}");
    assert!(!out.contains("done-one"), "done work is left alone: {out}");
    assert_eq!(out.matches("project undated-one:").count(), 1, "flagged once: {out}");

    // `--json` carries the same lines; they never make the store unhealthy on their own.
    let (_, json, _) = run_base(&s, &["doctor", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&json).expect("doctor --json");
    assert_eq!(v["next_steps"].as_array().map(Vec::len), Some(2), "{v}");

    // The limit is `[doctor] stale_next_days`.
    let toml = s.home.join(".base-gbl").join("base.toml");
    std::fs::write(&toml, "[doctor]\nstale_next_days = 30\n").expect("base.toml");
    let (_, out, _) = run_base(&s, &["doctor"]);
    assert!(!out.contains("project old-one"), "20 days is inside 30: {out}");
    assert!(out.contains("project undated-one: next step undated"), "undated is flagged whatever the limit: {out}");

    // Rewritten, it is fresh and doctor says nothing about it.
    ok(&s, &["project", "update", "undated-one", "--next-action", "boot WSL"]);
    let (_, out, _) = run_base(&s, &["doctor"]);
    assert!(!out.contains("undated-one"), "{out}");
}

/// P5, Example 2: a project whose docs point into two folders named like it is listed with both, never guessed;
/// the reviewed list writes both commented out; applying it untouched sets nothing; keeping one sets that one.
#[test]
fn paths_suggest_lists_two_candidates() {
    let (_tmp, s) = fixture();
    let docs = s.ws.join("Documents");
    let nested = docs.join("Studio").join("Studio Client");
    let repo = docs.join("studio-client");
    for d in [&nested, &repo.join("src"), &repo.join(".git")] {
        std::fs::create_dir_all(d).expect("folders");
    }
    ok(&s, &["project", "add", "-n", "studio", "-p", "Documents/Studio"]);
    // A project with no folder on record, and handoff and fork docs filed under it.
    append_quads(&s, &project_quads(&s, "studio-client", None));
    let notes = s.ws.join("notes");
    std::fs::create_dir_all(&notes).expect("notes");
    let g = ws_graph(&s);
    let mut quads = String::new();
    for (i, target) in [
        stored(&nested.join("brief.md")),
        stored(&nested.join("contacts.md")),
        stored(&repo.join("src").join("main.rs")),
        stored(&repo.join("README.md")),
        stored(&repo.join("src")),
    ]
    .iter()
    .enumerate()
    {
        let doc = notes.join(format!("h{i}.md"));
        std::fs::write(&doc, format!("# handoff {i}\n\nWork is in `{target}`, see there.\n")).expect("doc");
        let h = format!("{NS}handoff/studio-client-h{i}");
        quads.push_str(&format!(
            "<{h}> <{RDF_TYPE}> <{NS}Handoff> <{g}> .\n\
             <{h}> <{NS}project> \"studio-client\" <{g}> .\n\
             <{h}> <{NS}status> \"open\" <{g}> .\n\
             <{h}> <{NS}handoffDoc> \"{}\" <{g}> .\n",
            stored(&doc)
        ));
    }
    append_quads(&s, &quads);

    let list = s.ws.join("paths.toml");
    let out = ok(&s, &["project", "paths", "--suggest", "--out", list.to_str().unwrap()]);
    assert!(out.contains("| project | now | suggested folder | evidence |"), "{out}");
    assert!(out.contains("| studio-client | (no folder) | TWO CANDIDATES: | |"), "{out}");
    let nested_row = format!("| | | {} | 2 handoff and fork docs point inside it |", stored(&nested));
    let repo_row = format!("| | | {} | 3 handoff and fork docs point inside it (looks like a code repo) |", stored(&repo));
    assert!(out.lines().any(|l| l.eq_ignore_ascii_case(&repo_row)), "the repo, best first:\n{out}");
    assert!(out.lines().any(|l| l.eq_ignore_ascii_case(&nested_row)), "{out}");
    assert!(!out.contains("| studio |"), "a project whose folder stands is not listed: {out}");

    // The list: both candidates commented out, so an untouched list picks neither.
    let text = std::fs::read_to_string(&list).expect("list written");
    assert!(text.contains("# studio-client: 2 CANDIDATES, keep one (now: no folder)"), "{text}");
    let commented: Vec<&str> = text.lines().filter(|l| l.starts_with("# \"studio-client\" = ")).collect();
    assert_eq!(commented.len(), 2, "{text}");
    let out = ok(&s, &["project", "paths", "--apply", list.to_str().unwrap()]);
    assert!(out.contains("0 project folder(s) set."), "{out}");
    assert_eq!(record(&s, "studio-client")["path"], serde_json::Value::Null);

    // Keeping both is refused whole (one project, one folder); keeping one sets it.
    let both = text.replace("# \"studio-client\" = ", "\"studio-client\" = ");
    std::fs::write(&list, &both).expect("edited");
    let (code, _, err) = run_base(&s, &["project", "paths", "--apply", list.to_str().unwrap()]);
    assert_eq!(code, 1, "{err}");
    assert!(err.starts_with("Error: "), "{err}");
    assert_eq!(record(&s, "studio-client")["path"], serde_json::Value::Null, "nothing written");

    let keep = commented.iter().find(|l| l.contains("code repo")).expect("the repo line").trim_start_matches("# ");
    std::fs::write(&list, format!("{keep}\n")).expect("edited");
    let out = ok(&s, &["project", "paths", "--apply", list.to_str().unwrap(), "--dry-run"]);
    assert!(out.contains("would set studio-client: (none) → "), "{out}");
    assert_eq!(record(&s, "studio-client")["path"], serde_json::Value::Null, "a dry run writes nothing");
    let out = ok(&s, &["project", "paths", "--apply", list.to_str().unwrap()]);
    assert!(out.contains("1 project folder(s) set."), "{out}");
    assert!(same_path(record(&s, "studio-client")["path"].as_str().unwrap(), &stored(&repo)));

    // A folder that does not exist is refused, naming the line.
    std::fs::write(&list, "\"studio\" = \"Documents/Nowhere\"\n").expect("edited");
    let (code, _, err) = run_base(&s, &["project", "paths", "--apply", list.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(err.contains("studio: ") && err.contains("Nowhere does not exist"), "{err}");

    // A Unix-style path that is not a WSL one names no folder Windows can open, and is refused there; elsewhere it
    // is a folder that does not exist.
    std::fs::write(&list, "\"studio\" = \"/Users/nobody/proj\"\n").expect("edited");
    let (code, _, err) = run_base(&s, &["project", "paths", "--apply", list.to_str().unwrap()]);
    assert_eq!(code, 1, "{err}");
    let why = if cfg!(windows) { "is a Unix-style path Windows cannot open" } else { "does not exist" };
    assert!(err.contains("studio: /Users/nobody/proj") && err.contains(why), "{err}");
}
