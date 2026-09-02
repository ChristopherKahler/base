//! Every app gets a code map on first contact; the home directory never does;
//! a fresh sync is debounced. Chris, 2026-09-01: "anytime a dev project is
//! started, it auto creates the AST map ... base needs to drive this
//! completely without fail." Before 0.13.9 a new project stayed unmapped
//! until someone ran `base sync --ast` by hand, because the Stop hook was
//! opt-in and session-start built nothing. Before 0.13.10 a folder with no
//! `.git` stayed unmapped for ever, a session opened at home that only READ a
//! project never mapped it, and a `.base` workspace of repos could be mapped
//! as one giant app.

use std::path::Path;

use base::hook::automap::{ensure_first_map, plan_root, session_root, RootFacts, RootPlan};
use base::hook::stop::{plan_map, MapPlan};

#[test]
fn first_contact_builds_then_refreshes_never_the_home_dir() {
    let home = Path::new("C:/Users/someone");
    let app = Path::new("C:/Users/someone/dev/app");

    assert_eq!(plan_map(app, Some(home), false, false), MapPlan::Build, "no map yet → build it");
    assert_eq!(plan_map(app, Some(home), true, false), MapPlan::Refresh, "map exists → refresh it");
    assert_eq!(plan_map(app, Some(home), false, true), MapPlan::Debounced, "a sync just ran → wait");
    assert_eq!(plan_map(app, Some(home), true, true), MapPlan::Debounced);

    // The home directory is an app root on this machine (it holds .base), and
    // mapping it would walk every project at once. Never, mapped or not.
    assert_eq!(plan_map(home, Some(home), false, false), MapPlan::SkipHome);
    assert_eq!(plan_map(home, Some(home), true, false), MapPlan::SkipHome);

    // No known home: nothing is special.
    assert_eq!(plan_map(app, None, false, false), MapPlan::Build);
}

/// Which folder a session maps, one rule, every input explicit.
#[test]
fn which_folder_is_the_app() {
    let home = Path::new("/home/u");
    let cwd = Path::new("/home/u/dev/app");
    let facts = |strong: Option<&Path>, weak: Option<&Path>, weak_is_hub, user_folder, child_apps, sources| {
        plan_root(&RootFacts { cwd, home: Some(home), strong, weak, weak_is_hub, user_folder, child_apps, sources })
    };

    // A repo maps at its root.
    assert_eq!(facts(Some(cwd), None, false, false, false, true), RootPlan::Marked(cwd.into()));

    // Inside a monorepo: the repo root, not the package folder.
    let repo = Path::new("/home/u/dev/mono");
    let pkg = Path::new("/home/u/dev/mono/packages/x");
    assert_eq!(
        plan_root(&RootFacts { cwd: pkg, home: Some(home), strong: Some(repo), weak: None, weak_is_hub: false, user_folder: false, child_apps: false, sources: true }),
        RootPlan::Marked(repo.into())
    );

    // A `.base` workspace that is itself the app.
    let ws = Path::new("/home/u/dev/ws");
    assert_eq!(
        plan_root(&RootFacts { cwd: ws, home: Some(home), strong: None, weak: Some(ws), weak_is_hub: false, user_folder: false, child_apps: false, sources: true }),
        RootPlan::Marked(ws.into())
    );

    // A `.base` workspace that only holds repos: a hub, never mapped as one app.
    assert_eq!(
        plan_root(&RootFacts { cwd: ws, home: Some(home), strong: None, weak: Some(ws), weak_is_hub: true, user_folder: false, child_apps: true, sources: true }),
        RootPlan::Hub
    );

    // A bare folder inside that hub, with its own code: adopted on its own.
    let fresh = Path::new("/home/u/dev/ws/newapp");
    assert_eq!(
        plan_root(&RootFacts { cwd: fresh, home: Some(home), strong: None, weak: Some(ws), weak_is_hub: true, user_folder: false, child_apps: false, sources: true }),
        RootPlan::Adopt(fresh.into())
    );

    // Brand-new folder, no git yet, has code: adopted. That is "the very first
    // time an application is started".
    assert_eq!(facts(None, None, false, false, false, true), RootPlan::Adopt(cwd.into()));

    // Brand-new folder with nothing in it yet: nothing to map; Stop looks again.
    assert_eq!(facts(None, None, false, false, false, false), RootPlan::Empty);

    // Home, Documents, a folder of repos: never.
    assert_eq!(
        plan_root(&RootFacts { cwd: home, home: Some(home), strong: None, weak: None, weak_is_hub: false, user_folder: true, child_apps: true, sources: true }),
        RootPlan::Home
    );
    assert_eq!(facts(None, None, false, true, false, true), RootPlan::Hub, "a user folder");
    assert_eq!(facts(None, None, false, false, true, true), RootPlan::Hub, "a folder of apps");

    // Home is never an app even when nothing else is known about it.
    assert_eq!(
        plan_root(&RootFacts { cwd: home, home: Some(home), strong: None, weak: None, weak_is_hub: false, user_folder: false, child_apps: false, sources: true }),
        RootPlan::Home
    );
}

/// On disk: a folder of code with no `.git` is adopted, adoption leaves the
/// marker, the in-flight build is not doubled, and a landed map ends first
/// contact. The home dir and a folder of apps are never adopted.
#[test]
fn first_contact_on_disk_adopts_a_bare_folder_once() {
    // Decide everything, spawn nothing: the test binary must not launch itself
    // as `base sync`.
    // SAFETY: single-process test env; every test in this crate tolerates it set.
    unsafe { std::env::set_var("BASE_AST_NO_SPAWN", "1") };

    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let app = home.join("dev").join("fresh");
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(app.join("src").join("main.py"), "def f():\n    pass\n").unwrap();

    base::home::with_thread_home(&home, || {
        assert_eq!(session_root(&app), RootPlan::Adopt(app.clone()), "no .git anywhere: the folder is the app");
        assert_eq!(ensure_first_map(&app), Some(MapPlan::Build));
        assert!(app.join(".base-ast").is_dir(), "adoption marks the folder");
        assert!(app.join(".base-ast").join(".last-sync").is_file(), "the build is recorded");

        // Now marked, with a build in flight: not doubled.
        assert_eq!(session_root(&app), RootPlan::Marked(app.clone()));
        assert_eq!(ensure_first_map(&app), Some(MapPlan::Debounced));

        // Once the map lands, first contact costs one stat and does nothing.
        std::fs::write(app.join(".base-ast").join("ast.ttl"), "# map\n").unwrap();
        assert_eq!(ensure_first_map(&app), None);

        // Never home; never the folder that holds the app.
        assert_eq!(session_root(&home), RootPlan::Home);
        assert_eq!(session_root(&home.join("dev")), RootPlan::Hub, "dev/ holds an app, it is not one");

        // An empty new folder: nothing yet, and nothing was created.
        let empty = home.join("dev").join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(session_root(&empty), RootPlan::Empty);
        assert!(!empty.join(".base-ast").exists());
    });
}
