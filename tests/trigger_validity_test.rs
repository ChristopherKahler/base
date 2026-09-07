//! F29 step 6 — trigger-data validity (fork `base-injection-scope`, G0 amendment). A path
//! trigger that is unrooted, or a prefix of two or more registered projects' paths, is a
//! broadcast: doctor names it per tier and counts it against health (a), `add-trigger`
//! refuses it with the same sentence and writes nothing (b), and the matcher treats it as
//! inert while devmode names it (c). Red on 0.14.0, where every one of these was accepted
//! and fired.

use std::path::Path;

use base::config::NamespaceConfig;
use base::crud;
use base::domain::matcher::{inert_triggers, TriggerFault};

const GLOBAL_DOMAINS: &str = r#"
[[domain]]
name = "GLOBAL"
mode = "always"
rules = ["Never lie"]

[[domain]]
name = "vintrix"
mode = "triggered"
paths = ["Documents"]
rules = ["Twelve thousand a month and ten percent"]

[[domain]]
name = "meet-caddy"
mode = "triggered"
paths = ["Documents/Meet Caddy"]
rules = ["Never say a floor out loud"]
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

/// Three projects under `Documents`, one of them under `Documents/Meet Caddy`.
fn register_three(root: &Path) {
    register(root, "agentic-os", "Documents/agentic-os");
    register(root, "first-client-kit", "Documents/first-client-kit");
    register(root, "renda-group", "Documents/Meet Caddy/renda-group");
}

/// (a) doctor names the trigger, the domain and the projects it covers, per tier, and the
/// verdict is UNHEALTHY; a home where every trigger covers at most one project is clean.
#[test]
fn doctor_names_a_trigger_that_covers_two_or_more_registered_projects() {
    let tmp = home();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        register_three(root);
        let report = base::doctor::diagnose(root);
        let human = base::doctor::format_human(&report);
        assert!(
            human.contains("global tier: path trigger `Documents` on `vintrix` covers 3 registered projects ("),
            "{human}"
        );
        for name in ["agentic-os", "first-client-kit", "renda-group"] {
            assert!(human.contains(name), "{human}");
        }
        assert!(human.contains("narrow it or set auto_inject = false"), "{human}");
        assert!(!human.contains("on `meet-caddy`"), "one project under it, live: {human}");
        assert!(!report.healthy, "an inert trigger is a fault, not an advisory");
    });

    let tmp = home();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        register(root, "agentic-os", "Documents/agentic-os");
        let report = base::doctor::diagnose(root);
        let human = base::doctor::format_human(&report);
        assert!(!human.contains("path trigger"), "{human}");
        assert!(report.healthy, "{human}");
    });
}

/// (b) `add-trigger --path` refuses a broadcast and an unrooted trigger with the sentence,
/// and domains.toml is untouched; a trigger over one project is accepted.
#[test]
fn add_trigger_refuses_an_inert_trigger_and_writes_nothing() {
    let tmp = home();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        register_three(root);
        let toml_path = root.join(".base").join("domains.toml");
        let before = std::fs::read_to_string(&toml_path).unwrap();

        let err = base::domain::add_trigger(root, "broad", None, Some("Documents")).unwrap_err();
        let text = format!("{err:#}");
        assert!(text.starts_with("path trigger `Documents` on `broad` covers 3 registered projects ("), "{text}");
        assert!(err.downcast_ref::<base::domain::TriggerRefused>().is_some(), "typed, so project add can tell");

        let err = base::domain::add_trigger(root, "glob", None, Some("*.md")).unwrap_err();
        assert_eq!(
            format!("{err:#}"),
            "path trigger `*.md` on `glob` is not a rooted path; write it absolute, ~-relative or relative to the tier root"
        );

        assert_eq!(std::fs::read_to_string(&toml_path).unwrap(), before, "nothing written on refusal");
        assert!(!base::domain::load_domains(root).iter().any(|d| d.name == "broad" || d.name == "glob"));

        base::domain::add_trigger(root, "narrow", None, Some("Documents/Meet Caddy/renda-group")).unwrap();
        let narrow = base::domain::load_domains(root).into_iter().find(|d| d.name == "narrow").unwrap();
        assert_eq!(narrow.paths, vec!["Documents/Meet Caddy/renda-group".to_string()]);
    });
}

/// (b) `project add` under a path that covers other registered projects registers the
/// project, creates no domain, and does not fail.
#[test]
fn project_add_under_a_broadcast_path_registers_the_project_and_no_domain() {
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

/// (c) the matcher's own account of inert triggers — the list the prompt hook prints one
/// `inert:` line from in devmode, and doctor reads per tier — names the broadcast and the
/// projects it covers, and nothing else.
#[test]
fn the_inert_trigger_list_names_the_broadcast_and_its_projects() {
    let tmp = home();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        register_three(root);
        let domains = base::domain::load_domains(root);
        let ctx = base::domain::trigger_context(root);
        let inert = inert_triggers(&domains, &ctx);
        assert_eq!(inert.len(), 1, "{inert:?}");
        let (domain, trigger, fault) = &inert[0];
        assert_eq!((*domain, *trigger), ("vintrix", "Documents"));
        match fault {
            TriggerFault::Covers(names) => {
                let mut names = names.clone();
                names.sort();
                assert_eq!(names, vec!["agentic-os", "first-client-kit", "renda-group"]);
            }
            other => panic!("expected Covers, got {other:?}"),
        }
    });
}
