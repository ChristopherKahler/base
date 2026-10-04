//! Trigger-data validity, as D1 (0.16.0, BO-10) has it. Until 0.16.0 a path trigger over two or more registered
//! projects was a "broadcast": inert, and doctor advised narrowing it or setting `auto_inject = false` (F29 step 6).
//! D1 rules out both. A trigger that holds registered projects other than its own project's children is BROAD: doctor
//! names it per tier with the projects and counts it against health, with no `auto_inject` advice (a); `add-trigger`
//! refuses it, naming them, and writes nothing (b); the matcher no longer treats it as inert, so devmode's `inert:`
//! list carries only triggers that cannot fire at all (c).

use std::path::Path;

use base::config::NamespaceConfig;
use base::crud;
use base::domain::matcher::{faulty_triggers, inert_triggers, TriggerFault};

const GLOBAL_DOMAINS: &str = r#"
[[domain]]
name = "GLOBAL"
mode = "always"
rules = ["Never lie"]

[[domain]]
name = "notes"
mode = "triggered"
paths = ["Documents"]
rules = ["The notes rule"]

[[domain]]
name = "drafts"
mode = "triggered"
paths = ["Documents/Studio/drafts"]
rules = ["The drafts rule"]

[[domain]]
name = "globbed"
mode = "triggered"
paths = ["*.md"]
rules = ["The glob rule"]
"#;

fn home() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".base-gbl").join(".base")).unwrap();
    std::fs::write(root.join(".base-gbl").join("domains.toml"), GLOBAL_DOMAINS).unwrap();
    std::fs::create_dir_all(root.join(".base")).unwrap();
    std::fs::write(
        root.join(".base").join("base.toml"),
        "[namespace]\nprefix = \"ops\"\nuri = \"http://ops-sys.local/ontology#\"\n",
    )
    .unwrap();
    tmp
}

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

fn register(root: &Path, name: &str, rel: &str) {
    let path = root.join(rel).display().to_string();
    crud::project::add(root, &ns(), name, "active", Some(&path)).unwrap();
}

/// Three projects under `Documents`, one of them under `Documents/Studio`.
fn register_three(root: &Path) {
    register(root, "alpha-app", "Documents/alpha-app");
    register(root, "beta-app", "Documents/beta-app");
    register(root, "studio", "Documents/Studio");
}

/// (a) doctor names a broad trigger, the domain and the projects it holds, per tier, as advice: since BO-26 (lynx's G0
/// ruling on Q2) a broad trigger fires and is not counted against the verdict; a trigger on a folder inside a project,
/// holding none, is not named; no line advises `auto_inject = false`.
#[test]
fn doctor_names_a_trigger_that_holds_registered_projects() {
    let tmp = home();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        register_three(root);
        let report = base::doctor::diagnose(root);
        let human = base::doctor::format_human(&report);
        assert!(
            human.contains(
                "global tier: path trigger `Documents` on `notes` holds 3 registered projects (alpha-app, beta-app, studio): \
                 a trigger must be one project's own folder or a file (base domain paths --suggest proposes one)"
            ),
            "{human}"
        );
        assert!(!human.contains("path trigger `Documents/Studio/drafts`"), "a folder inside one project holds none: {human}");
        assert!(!human.contains("auto_inject = false"), "D1: never the fix\n{human}");
        // BO-26 replaced "a broad trigger is a fault, not an advisory": it is advice, and only a trigger that cannot fire
        // is a fault (this home's unrooted `*.md` still is).
        assert!(!report.trigger_faults.iter().any(|f| f.contains("`Documents`")), "{:?}", report.trigger_faults);
        assert!(report.trigger_faults.iter().any(|f| f.contains("`*.md` on `globbed` is not a rooted path")), "{:?}", report.trigger_faults);
        assert!(report.trigger_advice.iter().any(|a| a.contains("path trigger `Documents` on `notes` holds 3")), "{:?}", report.trigger_advice);
    });

    let tmp = home();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        std::fs::write(root.join(".base-gbl").join("domains.toml"), "[[domain]]\nname = \"notes\"\npaths = [\"Documents\"]\n").unwrap();
        register(root, "studio", "Documents/Studio");
        let report = base::doctor::diagnose(root);
        let human = base::doctor::format_human(&report);
        assert!(human.contains("path trigger `Documents` on `notes` holds 1 registered project (studio)"), "one is enough: {human}");
    });
}

/// (b) `add-trigger --path` refuses a broad path and an unrooted one, and domains.toml is untouched; a folder
/// inside a project is accepted and stored as its full path.
#[test]
fn add_trigger_refuses_a_broad_trigger_and_writes_nothing() {
    let tmp = home();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        register_three(root);
        let toml_path = root.join(".base").join("domains.toml");
        let before = std::fs::read_to_string(&toml_path).unwrap();

        let err = base::domain::add_trigger(root, false, "broad", None, Some("Documents")).unwrap_err();
        let text = format!("{err:#}");
        assert!(text.ends_with("contains 3 registered projects (alpha-app, beta-app, studio). A trigger must be one project's own folder or a file."), "{text}");
        assert!(err.downcast_ref::<base::domain::TriggerRefused>().is_some(), "typed, so project add can tell");

        let err = base::domain::add_trigger(root, false, "glob", None, Some("*.md")).unwrap_err();
        assert_eq!(
            format!("{err:#}"),
            "path trigger `*.md` on `glob` is not a rooted path; write it absolute, ~-relative or relative to the tier root"
        );

        assert_eq!(std::fs::read_to_string(&toml_path).unwrap(), before, "nothing written on refusal");
        assert!(!base::domain::load_domains(root).iter().any(|d| d.name == "broad" || d.name == "glob"));

        base::domain::add_trigger(root, false, "narrow", None, Some("Documents/Studio/drafts")).unwrap();
        let narrow = base::domain::load_domains(root).into_iter().find(|d| d.name == "narrow").unwrap();
        let full = crud::project::absolute_path(&root.join("Documents/Studio/drafts").display().to_string(), None, None).unwrap();
        assert_eq!(narrow.paths, vec![full], "stored as its full path (P3)");
    });
}

/// (b) `project add` under a path that holds other registered projects registers the project, creates no domain,
/// and does not fail.
#[test]
fn project_add_under_a_broad_path_registers_the_project_and_no_domain() {
    let tmp = home();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        register_three(root);
        let umbrella = root.join("Documents").display().to_string();
        crud::project::add(root, &ns(), "umbrella", "active", Some(&umbrella)).unwrap();

        let (projects, _) = crud::project::list_data(root, &base::config::BaseConfig::load(root), &base::scope::ProjectScope::All).unwrap();
        assert!(projects.iter().any(|p| p.name == "umbrella"), "registered: {:?}", projects.iter().map(|p| &p.name).collect::<Vec<_>>());
        assert!(!base::domain::load_domains(root).iter().any(|d| d.name == "umbrella"), "a domain with a refused trigger is not created");
    });
}

/// (c) the matcher's own account: the broad trigger is a fault doctor reads, with its projects, and NOT inert; the
/// `inert:` list devmode prints holds only the trigger that cannot fire.
#[test]
fn a_broad_trigger_is_a_fault_and_only_an_unrooted_one_is_inert() {
    let tmp = home();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        register_three(root);
        let domains = base::domain::load_domains(root);
        let ctx = base::domain::trigger_context(root);
        let inert = inert_triggers(&domains, &ctx);
        assert_eq!(inert.iter().map(|(d, t, _)| (*d, *t)).collect::<Vec<_>>(), vec![("globbed", "*.md")]);
        let faults = faulty_triggers(&domains, &ctx);
        let broad: Vec<_> = faults.iter().filter(|(_, _, f)| matches!(f, TriggerFault::Broad(_))).collect();
        assert_eq!(broad.len(), 1, "{faults:?}");
        let (domain, trigger, fault) = broad[0];
        assert_eq!((*domain, *trigger), ("notes", "Documents"));
        assert_eq!(fault, &TriggerFault::Broad(vec!["alpha-app".into(), "beta-app".into(), "studio".into()]));
    });
}
