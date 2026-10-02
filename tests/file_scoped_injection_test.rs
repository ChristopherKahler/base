//! BO-10 (topic P, F21, D1, D13): the file being touched decides what is injected: that file's project, and its parent
//! only when nested. Every test drives the real handlers on a fake home shaped like the operator's after BO-09's folder
//! list (the brief's examples, with the fake home in place of `C:/Users/Chris`): `vintrix` owns `Documents/Vintryx`,
//! the child project `vintryx-dealer-registry` owns `Documents/Vintryx/dealer-registry`, and two more projects sit
//! under `Documents`, so `Documents` holds four. Home is the workspace root, as on the operator's machine.

use std::path::Path;

use base::config::{BaseConfig, NamespaceConfig};
use base::crud;
use base::crud::project::{ParentChange, ProjectUpdate};
use base::domain::session::SessionState;
use base::hook::{pre_tool_use, user_prompt_submit};

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

/// The one spelling base stores and prints a path in (F25b).
fn spelled(p: &Path) -> String {
    crud::project::absolute_path(&p.display().to_string(), None, None).unwrap()
}

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// A fake home that is also the workspace root: global tier under `.base-gbl/`, workspace tier under `.base/`, and
/// the real files the examples read.
fn home() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".base-gbl").join(".base")).unwrap();
    std::fs::create_dir_all(root.join(".base")).unwrap();
    std::fs::write(
        root.join(".base").join("base.toml"),
        "[namespace]\nprefix = \"ops\"\nuri = \"http://ops-sys.local/ontology#\"\n",
    )
    .unwrap();
    for rel in [
        "Documents/Vintryx/README.md",
        "Documents/Vintryx/updates/CLIENT.md",
        "Documents/Vintryx/updates/EOD-2026-09-29-full.md",
        "Documents/Vintryx/dealer-registry/README.md",
        "Documents/alpha-app/notes.md",
        "Documents/beta-app/notes.md",
        "Documents/Reference Tables 2026-09-17.md",
    ] {
        write(root, rel, "x\n");
    }
    tmp
}

/// Register a project at `rel`, the way `base project add --path` does: the record, and a same-named domain whose
/// path trigger is the folder.
fn register(root: &Path, name: &str, rel: &str) {
    let path = root.join(rel).display().to_string();
    crud::project::add(root, &ns(), name, "active", Some(&path)).unwrap();
}

fn link(root: &Path, child: &str, parent: &str, nested: bool) {
    let change = ProjectUpdate { parent: Some(ParentChange::Set(parent.into())), nested: Some(nested), ..Default::default() };
    crud::project::apply_update(root, &ns(), child, &change).unwrap();
}

fn rule(root: &Path, domain: &str, text: &str) {
    crud::rule::add(root, &ns(), domain, text, None).unwrap();
}

/// Example 1's setup, with `nested` as given: vintrix and its child, each with a rule, and two projects beside them.
fn vintryx(root: &Path, nested: bool) {
    register(root, "vintrix", "Documents/Vintryx");
    register(root, "vintryx-dealer-registry", "Documents/Vintryx/dealer-registry");
    register(root, "alpha-app", "Documents/alpha-app");
    register(root, "beta-app", "Documents/beta-app");
    link(root, "vintryx-dealer-registry", "vintrix", nested);
    rule(root, "vintrix", "Vintryx rule: the EOD goes out twice a day");
    rule(root, "vintryx-dealer-registry", "Dealer registry rule: one row per rooftop");
    rule(root, "alpha-app", "Alpha rule");
    rule(root, "beta-app", "Beta rule");
}

/// One tool call in a fresh session: the domains whose rules were injected, and the text.
fn tool(root: &Path, cwd: &Path, name: &str, input: serde_json::Value) -> (Vec<String>, String) {
    SessionState::clear(&root.join(".base"));
    let config = BaseConfig::load(root);
    let event = serde_json::json!({ "tool_name": name, "tool_input": input, "session_id": "bo10" });
    let (data, context) = pre_tool_use::handle(&config, cwd, &event).unwrap();
    (data.domains_matched, context)
}

fn read(root: &Path, rel: &str) -> (Vec<String>, String) {
    tool(root, root, "Read", serde_json::json!({ "file_path": root.join(rel).display().to_string() }))
}

/// Example 2. A Vintryx file brings vintrix's rules and nothing else: not the child's, not a sibling's.
#[test]
fn file_in_project_injects_that_project_only() {
    let tmp = home();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        vintryx(root, true);
        let (matched, context) = read(root, "Documents/Vintryx/updates/CLIENT.md");
        assert_eq!(matched, vec!["vintrix".to_string()], "{context}");
        assert!(context.contains("[FILE MATCH: vintrix]\n  0. Vintryx rule"), "{context}");
        for other in ["Dealer registry rule", "Alpha rule", "Beta rule"] {
            assert!(!context.contains(other), "{other} reached a vintrix file:\n{context}");
        }
        let (matched, _) = read(root, "Documents/alpha-app/notes.md");
        assert_eq!(matched, vec!["alpha-app".to_string()], "a sibling's file brings the sibling only");
    });
}

/// Example 1, `nested = true`: the child's block, then the parent's, labelled.
#[test]
fn nested_child_adds_parent_after_own() {
    let tmp = home();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        vintryx(root, true);
        let (matched, context) = read(root, "Documents/Vintryx/dealer-registry/README.md");
        assert_eq!(matched, vec!["vintryx-dealer-registry".to_string(), "vintrix".to_string()], "{context}");
        let child = context.find("[FILE MATCH: vintryx-dealer-registry]\n  0. Dealer registry rule").expect(&context);
        let parent = context
            .find("[FILE MATCH: vintrix (parent of vintryx-dealer-registry)]\n  0. Vintryx rule")
            .expect(&context);
        assert!(child < parent, "child first, so a tight budget drops the parent first:\n{context}");
        assert!(!context.contains("Alpha rule") && !context.contains("Beta rule"), "{context}");
    });
}

/// Example 1, `nested = false`: the child's block only.
#[test]
fn non_nested_child_does_not_add_parent() {
    let tmp = home();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        vintryx(root, false);
        let (matched, context) = read(root, "Documents/Vintryx/dealer-registry/README.md");
        assert_eq!(matched, vec!["vintryx-dealer-registry".to_string()], "{context}");
        assert!(!context.contains("Vintryx rule"), "the parent's rules came without nested:\n{context}");
    });
}

/// D13, three levels with the middle one `nested = false`: the leaf brings the middle and stops there. With the
/// middle set to true, the top comes too, after both.
#[test]
fn nested_walk_stops_at_first_false() {
    let tmp = home();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        write(root, "Documents/Group/Team/Desk/plan.md", "x\n");
        register(root, "group", "Documents/Group");
        register(root, "team", "Documents/Group/Team");
        register(root, "desk", "Documents/Group/Team/Desk");
        link(root, "team", "group", false);
        link(root, "desk", "team", true);
        for d in ["group", "team", "desk"] {
            rule(root, d, &format!("{d} rule"));
        }
        let (matched, context) = read(root, "Documents/Group/Team/Desk/plan.md");
        assert_eq!(matched, vec!["desk".to_string(), "team".to_string()], "{context}");
        assert!(context.contains("[FILE MATCH: team (parent of desk)]"), "{context}");
        assert!(!context.contains("group rule"), "the walk passed a false:\n{context}");

        link(root, "team", "group", true);
        let (matched, context) = read(root, "Documents/Group/Team/Desk/plan.md");
        assert_eq!(matched, vec!["desk".to_string(), "team".to_string(), "group".to_string()], "{context}");
        assert!(context.contains("[FILE MATCH: group (parent of team)]"), "{context}");
    });
}

/// Example 3. A file directly in Documents: no project folder holds it, so nothing is injected by path.
#[test]
fn file_in_no_project_injects_nothing_by_path() {
    let tmp = home();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        vintryx(root, true);
        let (matched, context) = read(root, "Documents/Reference Tables 2026-09-17.md");
        assert!(matched.is_empty(), "{matched:?}\n{context}");
        assert!(!context.contains("[FILE MATCH"), "{context}");
    });
}

/// Example 4 (P3). `Documents` holds four registered projects: refused, naming them and vintrix's own folder, and
/// domains.toml is left byte for byte as it was.
#[test]
fn add_trigger_refuses_broad_path() {
    let tmp = home();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        vintryx(root, true);
        let toml_path = root.join(".base").join("domains.toml");
        let before = std::fs::read_to_string(&toml_path).unwrap();

        let err = base::domain::add_trigger(root, false, "vintrix", None, Some("Documents")).unwrap_err();
        assert!(err.downcast_ref::<base::domain::TriggerRefused>().is_some(), "typed, so the CLI says Error:");
        assert_eq!(
            format!("{err:#}"),
            format!(
                "{} contains 4 registered projects (alpha-app, beta-app, vintrix, vintryx-dealer-registry). \
                 A trigger must be one project's own folder or a file. vintrix's folder is {}.",
                spelled(&root.join("Documents")),
                spelled(&root.join("Documents/Vintryx"))
            )
        );
        let err = base::domain::add_trigger(root, false, "notes", None, Some("Documents")).unwrap_err();
        assert!(
            format!("{err:#}").ends_with("contains 4 registered projects (alpha-app, beta-app, vintrix, vintryx-dealer-registry). A trigger must be one project's own folder or a file."),
            "a domain with no project of its own gets no folder sentence: {err:#}"
        );
        assert_eq!(std::fs::read_to_string(&toml_path).unwrap(), before, "nothing written on refusal");

        // vintrix's own folder holds its own child, and is fine; so is one project's folder on a topic domain.
        base::domain::add_trigger(root, false, "vintrix", None, Some("Documents/Vintryx")).unwrap();
        base::domain::add_trigger(root, false, "notes", None, Some("Documents/Vintryx")).unwrap();
    });
}

/// P3. A relative path is stored as the full path it names (the tier root's), `~` as home's, and a second spelling
/// of the same place adds nothing.
#[test]
fn add_trigger_stores_absolute() {
    let tmp = home();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        vintryx(root, true);
        base::domain::add_trigger(root, false, "notes", None, Some("Documents/Vintryx/updates")).unwrap();
        base::domain::add_trigger(root, false, "notes", None, Some(&root.join("Documents/Vintryx/updates").display().to_string())).unwrap();
        base::domain::add_trigger(root, false, "notes", None, Some("~/Documents/Vintryx/updates/CLIENT.md")).unwrap();
        let notes = base::domain::load_domains(root).into_iter().find(|d| d.name == "notes").unwrap();
        assert_eq!(
            notes.paths,
            vec![spelled(&root.join("Documents/Vintryx/updates")), spelled(&root.join("Documents/Vintryx/updates/CLIENT.md"))]
        );
        // The CLI's echo of what it stored.
        assert_eq!(
            base::domain::trigger_spelling(root, false, "Documents/Vintryx/updates").as_deref(),
            Some(spelled(&root.join("Documents/Vintryx/updates")).as_str())
        );
    });
}

/// The pre-0.16.0 shape the migration starts from: vintrix on `Documents` with `auto_inject = false` (F29's advice),
/// base-config on `.base-gbl` and the bare `commands.toml`, and a project's relative trigger. A project under
/// `.base-gbl` makes that trigger broad too.
fn old_shape(root: &Path) {
    vintryx(root, true);
    write(root, ".base-gbl/forks/2026-10-01-x.md", "x\n");
    register(root, "fork-log", ".base-gbl/forks");
    for f in ["base.toml", "commands.toml", "domains.toml"] {
        write(root, &format!(".base-gbl/{f}"), "");
    }
    let mut file = std::fs::read_to_string(root.join(".base").join("domains.toml")).unwrap();
    let vintrix = format!("paths = [\"{}\"]", spelled(&root.join("Documents/Vintryx")));
    assert!(file.contains(&vintrix), "project add wrote the folder: {file}");
    file = file.replacen(&vintrix, "auto_inject = false\npaths = [\"Documents\"]", 1);
    let alpha = format!("paths = [\"{}\"]", spelled(&root.join("Documents/alpha-app")));
    file = file.replacen(&alpha, "paths = [\"Documents/alpha-app\"]", 1);
    file.push_str("\n[[domain]]\nname = \"base-config\"\nprompt_keywords = [\"base-gbl\"]\npaths = [\".base-gbl\", \"commands.toml\"]\n");
    std::fs::write(root.join(".base").join("domains.toml"), file).unwrap();
}

/// Examples 5 and the vintrix line (P6): a project's broad or relative trigger becomes its folder, base-config's
/// become base's config files, vintrix's `auto_inject` goes back on, and a second run proposes nothing.
#[test]
fn migration_rewrites_broad_and_relative_triggers() {
    let tmp = home();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        old_shape(root);
        let report = base::domain::paths::suggest(root);
        let change = |name: &str| report.changes.iter().find(|c| c.domain == name).unwrap_or_else(|| panic!("{name}: {report:#?}"));

        let v = change("vintrix");
        assert_eq!(v.after, vec![spelled(&root.join("Documents/Vintryx"))]);
        assert!(!v.auto_inject_before && v.auto_inject_after, "{v:?}");
        // The fake home has a workspace base.toml, which the operator's store does not; it is a config file too.
        let gbl = root.join(".base-gbl");
        assert_eq!(
            change("base-config").after,
            vec![
                spelled(&gbl.join("base.toml")),
                spelled(&gbl.join("commands.toml")),
                spelled(&gbl.join("domains.toml")),
                spelled(&root.join(".base").join("domains.toml")),
                spelled(&root.join(".base").join("base.toml")),
            ]
        );

        // A topic domain with a broad trigger (listed for review, kept) and a relative file trigger (written out), and
        // a project's relative trigger outside its own folder (written out, not replaced by the folder).
        let toml_path = root.join(".base").join("domains.toml");
        let mut file = std::fs::read_to_string(&toml_path).unwrap();
        file.push_str("\n[[domain]]\nname = \"notes\"\npaths = [\"Documents\", \"Documents/Reference Tables 2026-09-17.md\"]\n");
        assert!(file.contains("paths = [\"Documents/alpha-app\"]"), "old_shape made it relative: {file}");
        file = file.replacen("paths = [\"Documents/alpha-app\"]", "paths = [\"Documents/alpha-app\", \"Documents/beta-app/notes.md\"]", 1);
        std::fs::write(&toml_path, file).unwrap();
        let report = base::domain::paths::suggest(root);
        let change = |name: &str| report.changes.iter().find(|c| c.domain == name).unwrap_or_else(|| panic!("{name}: {report:#?}"));
        assert_eq!(change("notes").after, vec!["Documents".to_string(), spelled(&root.join("Documents/Reference Tables 2026-09-17.md"))]);
        assert_eq!(report.review.iter().map(|r| (r.domain.as_str(), r.trigger.as_str())).collect::<Vec<_>>(), vec![("notes", "Documents")]);
        assert_eq!(change("alpha-app").after, vec![spelled(&root.join("Documents/alpha-app")), spelled(&root.join("Documents/beta-app/notes.md"))]);

        let list = root.join("paths.toml");
        std::fs::write(&list, base::domain::paths::format_list(&report)).unwrap();
        base::domain::paths::apply_cmd(root, &list, true, true).unwrap();
        assert_eq!(base::domain::paths::suggest(root).changes.len(), report.changes.len(), "--dry-run wrote nothing");
        base::domain::paths::apply_cmd(root, &list, false, true).unwrap();

        let domains = base::domain::load_domains(root);
        let get = |name: &str| domains.iter().find(|d| d.name == name).unwrap();
        assert_eq!(get("vintrix").paths, v.after);
        assert!(get("vintrix").auto_inject, "auto_inject is back on");
        assert_eq!(get("base-config").paths, change("base-config").after);
        assert_eq!(get("base-config").prompt_keywords, vec!["base-gbl".to_string()], "the rest of the domain kept");
        let again = base::domain::paths::suggest(root);
        assert!(again.changes.is_empty(), "nothing left to rewrite: {:#?}", again.changes);

        // A reviewed list with a relative or broad path is refused whole, and writes nothing.
        let toml_path = root.join(".base").join("domains.toml");
        let before = std::fs::read_to_string(&toml_path).unwrap();
        std::fs::write(&list, "[[domain]]\ntier = \"workspace\"\nname = \"vintrix\"\npaths = [\"Documents\"]\n").unwrap();
        let err = base::domain::paths::apply_cmd(root, &list, false, true).unwrap_err();
        assert!(err.downcast_ref::<base::domain::paths::Refused>().is_some() && format!("{err:#}").contains("is not a full path"), "{err:#}");
        let broad = format!("[[domain]]\ntier = \"workspace\"\nname = \"vintrix\"\npaths = [{}]\n", toml::Value::String(spelled(&root.join("Documents"))));
        std::fs::write(&list, broad).unwrap();
        let err = base::domain::paths::apply_cmd(root, &list, false, true).unwrap_err();
        assert!(format!("{err:#}").contains("holds 4 registered projects"), "{err:#}");
        assert_eq!(std::fs::read_to_string(&toml_path).unwrap(), before);
    });
}

/// Example 6 (P7). A rule scoped to `Vintryx/updates` fires on a file in that folder and not on `Vintryx/README.md`;
/// a path outside the project, inside its child, or holding the child is refused.
#[test]
fn file_scoped_rule_fires_only_inside_its_folder() {
    let tmp = home();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        vintryx(root, true);
        let updates = root.join("Documents/Vintryx/updates");
        let places = crud::rule::scoped_places(root, "vintrix", &[updates.display().to_string()]).unwrap();
        assert_eq!(places, vec![spelled(&updates)]);
        let matchers: Vec<_> = places.iter().map(|p| base::domain::rules::Matcher::for_place(p)).collect();
        let text = "EOD files follow updates/EOD-TEMPLATE.md; two versions daily.";
        crud::rule::add_with_matchers(root, &ns(), "vintrix", text, None, None, &matchers).unwrap();

        let (_, context) = read(root, "Documents/Vintryx/updates/EOD-2026-09-29-full.md");
        assert!(context.contains(text), "the scoped rule fires inside its folder:\n{context}");
        let (matched, context) = read(root, "Documents/Vintryx/README.md");
        assert!(!context.contains(text), "and not elsewhere in the project:\n{context}");
        assert_eq!(matched, vec!["vintrix".to_string()], "the project's other rules still come");

        let refused = |p: &str| crud::rule::scoped_places(root, "vintrix", &[root.join(p).display().to_string()]).unwrap_err();
        assert!(refused("Documents/alpha-app").contains("is not inside vintrix's folder"));
        assert!(refused("Documents/Vintryx/dealer-registry/README.md").contains("belongs to vintryx-dealer-registry"));
        assert!(refused("Documents/Vintryx").contains("holds 1 registered project (vintryx-dealer-registry)"));
        assert_eq!(crud::rule::scoped_places(root, "vintrix", &["Documents/Vintryx/updates".into()]).unwrap(), vec![spelled(&updates)], "relative input is the workspace root's");
    });
}

/// Example 7 (P1). A quoted file in a Bash command behaves like Example 1; a command that names no path touches the
/// session's folder, which brings that folder's project and nothing outside one.
#[test]
fn bash_tokens_resolve_to_files() {
    let tmp = home();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        vintryx(root, true);
        let file = root.join("Documents/Vintryx/dealer-registry/README.md").display().to_string();
        let (matched, context) = tool(root, root, "Bash", serde_json::json!({ "command": format!("cat \"{file}\" | head") }));
        assert_eq!(matched, vec!["vintryx-dealer-registry".to_string(), "vintrix".to_string()], "{context}");

        let (matched, _) = tool(root, root, "PowerShell", serde_json::json!({ "command": format!("Get-Content '{file}'") }));
        assert_eq!(matched, vec!["vintryx-dealer-registry".to_string(), "vintrix".to_string()], "PowerShell is the same shell here");

        let vintryx_dir = root.join("Documents/Vintryx");
        let (matched, _) = tool(root, &vintryx_dir, "Bash", serde_json::json!({ "command": "ls" }));
        assert_eq!(matched, vec!["vintrix".to_string()], "`ls` in a project folder touches it");
        let (matched, _) = tool(root, root, "Bash", serde_json::json!({ "command": "ls" }));
        assert!(matched.is_empty(), "`ls` at home touches no project: {matched:?}");
        let (matched, _) = tool(root, &vintryx_dir, "Bash", serde_json::json!({ "command": "cat dealer-registry/README.md" }));
        assert_eq!(matched, vec!["vintryx-dealer-registry".to_string(), "vintrix".to_string()], "relative to the session's folder");
        let (matched, _) = tool(root, root, "Bash", serde_json::json!({ "command": "cat Documents/no-such-file.md" }));
        assert!(matched.is_empty(), "a word that names nothing on disk is not a touched path: {matched:?}");

        let toml_path = root.join(".base").join("domains.toml");
        let mut file = std::fs::read_to_string(&toml_path).unwrap();
        file.push_str("\n[[domain]]\nname = \"kw\"\nfile_keywords = [\"dealer-registry\"]\nrules = [\"kw rule\"]\n");
        std::fs::write(&toml_path, file).unwrap();
        let registry = root.join("Documents/Vintryx/dealer-registry");
        let (matched, _) = tool(root, &registry, "Bash", serde_json::json!({ "command": "echo hi" }));
        assert!(!matched.contains(&"kw".to_string()), "a file keyword never matches the session's folder: {matched:?}");
        let (matched, _) = tool(root, &registry, "Bash", serde_json::json!({ "command": "cat README.md" }));
        assert!(matched.contains(&"kw".to_string()), "it matches a path the command names: {matched:?}");
    });
}

/// P8. The old shape has broad triggers and doctor names them (with no `auto_inject = false` advice); after the
/// migration doctor reports none.
#[test]
fn doctor_reports_zero_broad_triggers_after_migration() {
    let tmp = home();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        old_shape(root);
        let report = base::doctor::diagnose(root);
        let human = base::doctor::format_human(&report);
        assert!(
            human.contains("workspace tier: path trigger `Documents` on `vintrix` holds 4 registered projects (alpha-app, beta-app, vintrix, vintryx-dealer-registry)"),
            "{human}"
        );
        assert!(human.contains("path trigger `.base-gbl` on `base-config` holds 1 registered project (fork-log)"), "{human}");
        assert!(!human.contains("auto_inject = false"), "D1: never the fix\n{human}");
        assert!(!report.healthy, "a broad trigger counts against the verdict");

        let list = root.join("paths.toml");
        std::fs::write(&list, base::domain::paths::format_list(&base::domain::paths::suggest(root))).unwrap();
        base::domain::paths::apply_cmd(root, &list, false, true).unwrap();
        let report = base::doctor::diagnose(root);
        assert_eq!(report.trigger_faults, Vec::<String>::new(), "{}", base::doctor::format_human(&report));
    });
}

/// The prompt hook reads paths this session touched through the same seam: a child file brings the child, and the
/// parent only when nested, after the child.
#[test]
fn prompt_hook_follows_the_same_owner_and_nested_rule() {
    use std::io::Write;
    for nested in [true, false] {
        let tmp = home();
        base::home::with_thread_home(tmp.path(), || {
            let root = tmp.path();
            vintryx(root, nested);
            SessionState::clear(&root.join(".base"));
            let row = serde_json::json!({
                "ts": "2026-10-02T12:00:00-05:00", "hook": "pre-tool-use", "success": true, "session_id": "bo10-prompt",
                "tool_name": "Read", "file_path": root.join("Documents/Vintryx/dealer-registry/README.md").display().to_string(),
            });
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(root.join(".base").join("hook-events.jsonl")).unwrap();
            writeln!(f, "{row}").unwrap();
            let config = BaseConfig::load(root);
            let event = serde_json::json!({ "prompt": "hello there", "session_id": "bo10-prompt" });
            let mut out = String::new();
            let matched = user_prompt_submit::handle(&config, root, &event, &mut out).unwrap().domains_matched;
            let want: Vec<String> =
                if nested { vec!["vintryx-dealer-registry".into(), "vintrix".into()] } else { vec!["vintryx-dealer-registry".into()] };
            assert_eq!(matched, want, "nested = {nested}");
            assert_eq!(out.contains("[DOMAIN: vintrix (parent of vintryx-dealer-registry)]"), nested, "{out}");
        });
    }
}
