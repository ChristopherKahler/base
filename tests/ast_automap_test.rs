//! Every app root gets a code map on first contact; the home directory never
//! does; a fresh sync is debounced. Chris, 2026-09-01: "anytime a dev project
//! is started, it auto creates the AST map ... base needs to drive this
//! completely without fail." Before 0.13.9 a new project stayed unmapped
//! until someone ran `base sync --ast` by hand, because the Stop hook was
//! opt-in and session-start built nothing.

use std::path::Path;

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
