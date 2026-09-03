//! Every app gets a code map on first contact; the home directory never does;
//! a fresh sync is debounced. Chris, 2026-09-01: "anytime a dev project is
//! started, it auto creates the AST map ... base needs to drive this
//! completely without fail." Before 0.13.9 a new project stayed unmapped
//! until someone ran `base sync --ast` by hand, because the Stop hook was
//! opt-in and session-start built nothing. Before 0.13.10 a folder with no
//! `.git` stayed unmapped for ever, a session opened at home that only READ a
//! project never mapped it, and a `.base` workspace of repos could be mapped
//! as one giant app.

use std::path::{Path, PathBuf};

use base::hook::automap::{
    bash_first_contact, bash_paths, ensure_first_map, is_noise_dir, linux_paths, measure_tree,
    never_map_reason, plan_root, session_root, wsl_script, RootFacts, RootPlan,
};
use base::hook::stop::{plan_map, MapPlan};

#[test]
fn first_contact_builds_then_refreshes_never_the_home_dir() {
    let home = Path::new("C:/Users/someone");
    let app = Path::new("C:/Users/someone/dev/app");

    assert_eq!(plan_map(app, Some(home), None, false, false), MapPlan::Build, "no map yet → build it");
    assert_eq!(plan_map(app, Some(home), None, true, false), MapPlan::Refresh, "map exists → refresh it");
    assert_eq!(plan_map(app, Some(home), None, false, true), MapPlan::Debounced, "a sync just ran → wait");
    assert_eq!(plan_map(app, Some(home), None, true, true), MapPlan::Debounced);

    // The home directory is an app root on this machine (it holds .base), and
    // mapping it would walk every project at once. Never, mapped or not.
    assert_eq!(plan_map(home, Some(home), None, false, false), MapPlan::SkipHome);
    assert_eq!(plan_map(home, Some(home), None, true, false), MapPlan::SkipHome);

    // No known home: nothing is special.
    assert_eq!(plan_map(app, None, None, false, false), MapPlan::Build);
}

/// Which folder a session maps, one rule, every input explicit.
#[test]
fn which_folder_is_the_app() {
    let home = Path::new("/home/u");
    let cwd = Path::new("/home/u/dev/app");
    let facts = |strong: Option<&Path>, weak: Option<&Path>, weak_is_hub, user_folder, child_apps, sources| {
        plan_root(&RootFacts { cwd, home: Some(home), strong, weak, weak_is_hub, never: None, user_folder, child_apps, sources })
    };

    // A repo maps at its root.
    assert_eq!(facts(Some(cwd), None, false, false, false, true), RootPlan::Marked(cwd.into()));

    // Inside a monorepo: the repo root, not the package folder.
    let repo = Path::new("/home/u/dev/mono");
    let pkg = Path::new("/home/u/dev/mono/packages/x");
    assert_eq!(
        plan_root(&RootFacts { cwd: pkg, home: Some(home), strong: Some(repo), weak: None, weak_is_hub: false, never: None, user_folder: false, child_apps: false, sources: true }),
        RootPlan::Marked(repo.into())
    );

    // A `.base` workspace that is itself the app.
    let ws = Path::new("/home/u/dev/ws");
    assert_eq!(
        plan_root(&RootFacts { cwd: ws, home: Some(home), strong: None, weak: Some(ws), weak_is_hub: false, never: None, user_folder: false, child_apps: false, sources: true }),
        RootPlan::Marked(ws.into())
    );

    // A `.base` workspace that only holds repos: a hub, never mapped as one app.
    assert_eq!(
        plan_root(&RootFacts { cwd: ws, home: Some(home), strong: None, weak: Some(ws), weak_is_hub: true, never: None, user_folder: false, child_apps: true, sources: true }),
        RootPlan::Hub
    );

    // A bare folder inside that hub, with its own code: adopted on its own.
    let fresh = Path::new("/home/u/dev/ws/newapp");
    assert_eq!(
        plan_root(&RootFacts { cwd: fresh, home: Some(home), strong: None, weak: Some(ws), weak_is_hub: true, never: None, user_folder: false, child_apps: false, sources: true }),
        RootPlan::Adopt(fresh.into())
    );

    // Brand-new folder, no git yet, has code: adopted. That is "the very first
    // time an application is started".
    assert_eq!(facts(None, None, false, false, false, true), RootPlan::Adopt(cwd.into()));

    // Brand-new folder with nothing in it yet: nothing to map; Stop looks again.
    assert_eq!(facts(None, None, false, false, false, false), RootPlan::Empty);

    // Home, Documents, a folder of repos: never.
    assert_eq!(
        plan_root(&RootFacts { cwd: home, home: Some(home), strong: None, weak: None, weak_is_hub: false, never: None, user_folder: true, child_apps: true, sources: true }),
        RootPlan::Home
    );
    assert_eq!(facts(None, None, false, true, false, true), RootPlan::Hub, "a user folder");
    assert_eq!(facts(None, None, false, false, true, true), RootPlan::Hub, "a folder of apps");

    // Home is never an app even when nothing else is known about it.
    assert_eq!(
        plan_root(&RootFacts { cwd: home, home: Some(home), strong: None, weak: None, weak_is_hub: false, never: None, user_folder: false, child_apps: false, sources: true }),
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

        // A build in flight holds every later caller off, even after the
        // 20 s debounce has passed — the lock, not the marker, is the guard.
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(120);
        let touch_old = |p: &Path| {
            let f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
            f.set_modified(old).unwrap();
        };
        touch_old(&app.join(".base-ast").join(".last-sync"));
        assert_eq!(ensure_first_map(&app), Some(MapPlan::Debounced), ".building holds the second build off");
        std::fs::remove_file(app.join(".base-ast").join(".building")).unwrap();
        assert_eq!(ensure_first_map(&app), Some(MapPlan::Build), "lock released → the next build may start");
        assert!(app.join(".base-ast").join(".building").is_file(), "…and takes the lock");
        // A stale lock (crashed build) no longer counts.
        touch_old(&app.join(".base-ast").join(".building"));
        touch_old(&app.join(".base-ast").join(".last-sync"));
        let f = std::fs::OpenOptions::new().write(true).open(app.join(".base-ast").join(".building")).unwrap();
        f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(31 * 60)).unwrap();
        assert_eq!(ensure_first_map(&app), Some(MapPlan::Build), "a 31-minute-old lock is a crashed build");

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

        // A .base workspace whose repos sit two levels down (toolbox/frameworks/x)
        // is a hub even with no directly-marked child.
        let hub = home.join("dev").join("toolbox");
        std::fs::create_dir_all(hub.join(".base")).unwrap();
        std::fs::create_dir_all(hub.join("frameworks").join("kit").join(".git")).unwrap();
        std::fs::write(hub.join("loose.py"), "x = 1\n").unwrap();
        assert_eq!(session_root(&hub), RootPlan::Hub, "repos two levels down make it a hub");
        // …and a loose file in it resolves to the hub, which first contact refuses.
        assert_eq!(ensure_first_map(&hub), Some(MapPlan::SkipHub));
    });
}

/// What a Bash command touches: cd targets re-base what follows; `~`, Git
/// Bash drive paths, flags, globs and URLs are handled the way a shell reads
/// them. Pure: existence is the caller's job.
#[test]
fn bash_commands_name_the_paths_they_touch() {
    let cwd = Path::new("/home/u/ws");
    let home = Path::new("/home/u");
    let got = bash_paths("cd ~/ops-sys/toolbox/frameworks/kit && cargo test --release 2>&1 | tail -5", cwd, Some(home));
    assert_eq!(got, vec![PathBuf::from("/home/u/ops-sys/toolbox/frameworks/kit")]);

    let got = bash_paths("cat src/main.rs; sed -n '1,5p' ../other/lib.rs", cwd, Some(home));
    assert_eq!(got, vec![PathBuf::from("/home/u/ws/src/main.rs"), PathBuf::from("/home/u/ws/../other/lib.rs")]);

    let got = bash_paths("cd apps/x && grep -rn foo src/", cwd, Some(home));
    assert_eq!(got, vec![PathBuf::from("/home/u/ws/apps/x"), PathBuf::from("/home/u/ws/apps/x/src/")]);

    // flags, globs, variables, URLs and env assignments are not paths
    let got = bash_paths("FOO=1 curl -s https://x.y/z --out $HOME/*.txt", cwd, Some(home));
    assert!(got.is_empty(), "{got:?}");

    if cfg!(windows) {
        let got = bash_paths("cat /c/Users/Chris/dev/app/a.py", Path::new("C:/w"), Some(Path::new("C:/Users/Chris")));
        assert_eq!(got, vec![PathBuf::from("C:/Users/Chris/dev/app/a.py")]);
    }
}

#[test]
fn wsl_commands_hand_their_linux_paths_over() {
    assert_eq!(
        linux_paths("wsl -e bash -lc 'cd ~/ops-sys/toolbox/frameworks/kit && cat /home/u/x.rs'"),
        vec!["~/ops-sys/toolbox/frameworks/kit".to_string(), "/home/u/x.rs".to_string()]
    );
    assert!(linux_paths("cat ~/notes.md").is_empty(), "no wsl, nothing to delegate");
    assert!(linux_paths("wsl -e bash -lc 'echo $HOME'").is_empty());

    // A tilde path reaches the WSL shell as "$HOME/…", where it expands; an absolute one is quoted as-is.
    assert!(wsl_script("~/ops-sys/toolbox/frameworks/kit").contains("ast ensure --wait \"$HOME/ops-sys/toolbox/frameworks/kit\""));
    assert!(wsl_script("/home/u/x.rs").contains("ast ensure --wait '/home/u/x.rs'"));
}


/// Chris, 2026-09-03, after a hook spent thirty minutes extracting `%TEMP%` at
/// a third of a 16-core CPU: "just make it not ast parse shit it obviously
/// shouldn't." The temp directory, the caches, `AppData`, `node_modules` and
/// the other OS's home are never mapped — from any hook, marked or not.
#[test]
fn the_places_a_hook_never_maps() {
    // Called with this process's REAL home, which is not under temp, so both
    // halves are armed exactly as they are in production.
    let why = |p: &str| never_map_reason(Path::new(p));

    // 1. The OS temp directory itself, and a path three levels under it.
    let tmp = std::env::temp_dir();
    assert!(why(&tmp.to_string_lossy()).is_some(), "the temp dir itself: {}", tmp.display());
    let deep = tmp.join("claude").join("sess").join("scratchpad");
    assert!(never_map_reason(&deep).is_some(), "three levels under temp");
    // The exact shape the 11:43 run walked, seen from a WSL hook — where not
    // one of %TEMP% / %TMP% / TMPDIR is set, so only the segment rule fires.
    assert_eq!(why("/mnt/c/Users/Chris/AppData/Local/Temp"), Some("a Windows AppData directory"));
    assert_eq!(why("/mnt/c/Users/Chris/AppData/Local/Temp/claude/x"), Some("a Windows AppData directory"));

    // 2. Caches, the Claude state dir, base's own global tier, node_modules.
    //    Segment-matched, so these hold on Linux and Windows alike.
    assert_eq!(why("/home/u/.cache/uv/wheels"), Some("a cache directory"));
    assert_eq!(why("/home/u/.claude/skills/humanizer"), Some("the Claude Code state directory"));
    assert_eq!(why("/home/u/.base-gbl/forks"), Some("base's own global tier"));
    assert_eq!(why("/home/u/dev/app/node_modules/react"), Some("a node_modules tree"));
    assert_eq!(why("C:/Users/Chris/AppData/Roaming/npm"), Some("a Windows AppData directory"));

    // Case-insensitively, because Windows spells the same folder both ways.
    assert!(why("/mnt/c/Users/Chris/appdata/local/temp").is_some());
    assert!(why("/home/u/dev/app/NODE_MODULES/react").is_some());

    // 1(b). The other OS's home: `/mnt/c/Users/<name>` is a Windows home seen
    // from WSL, whose own real_home() is /home/<user>, so SkipHome never fires
    // for it. Measured 2026-09-03: C:/Users/Chris/.base-ast/ast.ttl, 23.6 MB.
    assert_eq!(why("/mnt/c/Users/Chris"), Some("the other OS's home directory"));
    assert_eq!(why("/mnt/d/Users/someone"), Some("the other OS's home directory"));

    // …matched EXACTLY. Real projects live under that home and still map.
    assert_eq!(why("/mnt/c/Users/Chris/dev/app"), None, "a project under the Windows home still maps");
    // …and the shape is not over-eager: /mnt/wsl and /mnt/c/Users are not homes.
    assert_eq!(why("/mnt/wsl/Users/x"), None, "wsl is not a drive letter");
    assert_eq!(why("/mnt/c/Users"), None, "the Users folder is not a home");

    // Component boundaries: a folder whose name merely starts with a banned one.
    assert_eq!(why("/home/u/dev/tmpfoo"), None, "/tmpfoo is not under /tmp");
    assert_eq!(why("/home/u/dev/cache-server"), None);
    assert_eq!(why("/home/u/dev/Local/app"), None, "Local without AppData above it is a project");
}

/// The sandbox exemption, both directions. Every test here builds its fixture
/// under the system temp dir, so the temp half has to stand down somewhere —
/// but ONLY for a process whose home is itself redirected under temp. Keying it
/// on the `isolation-guard` feature instead was the first attempt and was
/// wrong: that feature reaches the ordinary binary through the crate's
/// self-dev-dependency, and disarmed the guard in a plain `base` too.
#[test]
fn the_temp_rule_stands_down_only_inside_a_redirected_home() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let app = home.join("dev").join("app");

    // Outside any redirected home this process's home is the real one, so the
    // guard is armed exactly as it is in a shipped binary.
    assert!(never_map_reason(&app).is_some(), "a temp path is never mappable in production");

    // Inside a home that has itself been redirected under temp, the temp half
    // stands down so fixtures can be adopted — and nothing else does.
    base::home::with_thread_home(&home, || {
        assert_eq!(never_map_reason(&app), None, "a fixture inside the sandbox home is mappable");
        assert_eq!(
            never_map_reason(&app.join("node_modules").join("react")),
            Some("a node_modules tree"),
            "the segment rules stay armed in the sandbox too"
        );
        assert_eq!(never_map_reason(&app.join(".claude")), Some("the Claude Code state directory"));

        // A temp path OUTSIDE the sandbox home is still refused, even from an
        // isolated process: the exemption is this sandbox, not the temp dir.
        let elsewhere = std::env::temp_dir().join("some-other-tree");
        assert!(never_map_reason(&elsewhere).is_some(), "only this sandbox is exempt");
    });
}

/// A map that already exists inside a never-mapped root buys nothing. This is
/// the loop measured on this machine: the 11:43 run planted `.base-ast/` in
/// `%TEMP%`, which made `ast_app_root` resolve every later temp path to Temp,
/// which made every later hook refresh a thirty-minute extraction for ever.
#[test]
fn an_existing_map_inside_a_never_mapped_root_is_not_refreshed() {
    let home = Path::new("/home/u");
    let temp_app = Path::new("/mnt/c/Users/Chris/AppData/Local/Temp");
    let never = never_map_reason(temp_app);
    assert!(never.is_some());

    assert_eq!(plan_map(temp_app, Some(home), never, false, false), MapPlan::SkipNeverMap("a Windows AppData directory"));
    assert_eq!(
        plan_map(temp_app, Some(home), never, true, false),
        MapPlan::SkipNeverMap("a Windows AppData directory"),
        "a landed ast.ttl does not license a refresh"
    );

    // The guard sits ahead of the debounce, so it cannot be raced open either.
    assert_eq!(plan_map(temp_app, Some(home), never, true, true), MapPlan::SkipNeverMap("a Windows AppData directory"));

    // …and an ordinary app is untouched by any of it.
    let app = Path::new("/home/u/dev/app");
    assert_eq!(plan_map(app, Some(home), None, false, false), MapPlan::Build);
    assert_eq!(plan_map(app, Some(home), None, true, false), MapPlan::Refresh);
}

/// The negative control for the whole fork: an unmarked folder of source is
/// still adopted. Chris's 2026-09-01 rule ("do not lock it to only scaffolded
/// workspaces") outranks every guard added here.
#[test]
fn an_unmarked_folder_of_code_is_still_adopted() {
    // SAFETY: single-process test env; every test in this crate tolerates it set.
    unsafe { std::env::set_var("BASE_AST_NO_SPAWN", "1") };

    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let app = home.join("dev").join("scratch-app");
    std::fs::create_dir_all(&app).unwrap();
    for i in 0..40 {
        std::fs::write(app.join(format!("m{i}.py")), "x = 1\n").unwrap();
    }

    base::home::with_thread_home(&home, || {
        assert_eq!(session_root(&app), RootPlan::Adopt(app.clone()), "no .git, no .base: still an app");
        assert_eq!(ensure_first_map(&app), Some(MapPlan::Build), "40 files, well under any fuse");
    });
}

/// Windows spells the same folder `Temp` and `temp`, `Build` and `build`. "Has
/// code" here has to agree with "has files to extract" in detect.py, which
/// lowercases the name before its own lookup.
#[test]
fn noise_directories_match_whatever_the_case() {
    for name in ["node_modules", "NODE_MODULES", "Node_Modules", "temp", "Temp", "TEMP", "build", "Build", "target", "Target", ".Cache"] {
        assert!(is_noise_dir(name), "{name} is noise");
    }
    for name in ["src", "lib", "tests", "buildscripts", "tempo"] {
        assert!(!is_noise_dir(name), "{name} is not noise");
    }
}


/// The backstop for whatever the never-map list misses. Chris, 2026-09-03:
/// "Base is set to automatically make graphs, this will blow up people's
/// computers." A tree too big to extract unattended is not extracted at all —
/// `--yes` from a hook cannot get past this, because the fuse decides before
/// anything is spawned.
#[test]
fn a_tree_too_big_to_build_unattended_is_not_built() {
    // SAFETY: single-process test env; every test in this crate tolerates it set.
    unsafe { std::env::set_var("BASE_AST_NO_SPAWN", "1") };

    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let huge = home.join("dev").join("huge");
    std::fs::create_dir_all(&huge).unwrap();
    for i in 0..6_000 {
        std::fs::write(huge.join(format!("m{i}.py")), "x = 1\n").unwrap();
    }

    base::home::with_thread_home(&home, || {
        assert_eq!(ensure_first_map(&huge), Some(MapPlan::NeedsConfirm), "6,000 source files: ask first");

        let note = std::fs::read_to_string(huge.join(".base-ast").join(".needs-confirm"))
            .expect("the fuse records why");
        assert!(note.contains("5000"), "the limit is named: {note}");
        assert!(note.contains("source files"), "the counts are named: {note}");
        assert!(
            note.contains(&format!("base sync --ast --target {}", huge.display())),
            "the exact command to run by hand is spelled out: {note}"
        );

        // Nothing was spawned and no build was recorded — the fuse decides
        // ahead of every write `ensure_app_map` would otherwise make.
        assert!(!huge.join(".base-ast").join(".last-sync").exists(), "no build was recorded");
        assert!(!huge.join(".base-ast").join(".building").exists(), "no build lock was taken");

        // A tripped fuse is believed rather than re-walked every turn.
        assert_eq!(ensure_first_map(&huge), Some(MapPlan::NeedsConfirm));
    });
}

/// The fuse measures what the extractor will actually read: noise directories
/// and dot-directories skipped, nested marked apps left to their own maps.
/// Otherwise a repo with a vendored `node_modules` would be fused off its map.
#[test]
fn the_fuse_measures_what_the_extractor_will_read() {
    let tmp = tempfile::tempdir().unwrap();
    let app = tmp.path().join("app");
    std::fs::create_dir_all(app.join("src")).unwrap();
    for i in 0..5 {
        std::fs::write(app.join("src").join(format!("s{i}.rs")), "fn f() {}\n").unwrap();
    }

    let bare = measure_tree(&app);
    assert_eq!(bare.sources, 5);
    assert!(!bare.over_fuse());

    // A vendored dependency tree does not count — the extractor never walks it.
    std::fs::create_dir_all(app.join("node_modules").join("react")).unwrap();
    for i in 0..50 {
        std::fs::write(app.join("node_modules").join("react").join(format!("r{i}.js")), "1\n").unwrap();
    }
    // Nor does a nested repo, which keeps its own map.
    let nested = app.join("vendored-repo");
    std::fs::create_dir_all(nested.join(".git")).unwrap();
    for i in 0..50 {
        std::fs::write(nested.join(format!("n{i}.rs")), "fn g() {}\n").unwrap();
    }
    // Nor does a build directory, whatever its case.
    std::fs::create_dir_all(app.join("Target")).unwrap();
    for i in 0..50 {
        std::fs::write(app.join("Target").join(format!("t{i}.rs")), "fn h() {}\n").unwrap();
    }

    let after = measure_tree(&app);
    assert_eq!(after.sources, 5, "150 skipped files stayed out of the count");
}


/// A command's `cd` is transient; a session's cwd is a choice. Walking through
/// an unmarked folder never adopts it — but booting a session in that same
/// folder still does, because Chris's 2026-09-01 rule outranks this one.
#[test]
fn a_cd_never_adopts_an_unmarked_folder_but_a_session_cwd_still_does() {
    // SAFETY: single-process test env; every test in this crate tolerates it set.
    unsafe { std::env::set_var("BASE_AST_NO_SPAWN", "1") };

    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let loose = home.join("dev").join("passing-through");
    std::fs::create_dir_all(&loose).unwrap();
    std::fs::write(loose.join("main.py"), "x = 1\n").unwrap();

    base::home::with_thread_home(&home, || {
        // The path a `cd` names, exactly as pre-tool-use derives it.
        let named = bash_paths(
            &format!("cd {} && python main.py", loose.display()),
            &home,
            Some(&home),
        );
        assert_eq!(named, vec![loose.clone()], "the cd target is what gets contacted");

        assert_eq!(bash_first_contact(&loose), None, "walking through it adopts nothing");
        assert!(!loose.join(".base-ast").exists(), "…and leaves no marker behind");

        // THE NEGATIVE CONTROL. The same folder, as the session's own cwd, is
        // still adopted — this rule must not narrow session-start adoption.
        assert_eq!(session_root(&loose), RootPlan::Adopt(loose.clone()));
        assert_eq!(ensure_first_map(&loose), Some(MapPlan::Build));
    });

    // Once something marks it, a `cd` reaches it like any other app.
    let marked = home.join("dev").join("real-repo");
    std::fs::create_dir_all(marked.join(".git")).unwrap();
    std::fs::write(marked.join("main.py"), "x = 1\n").unwrap();
    base::home::with_thread_home(&home, || {
        assert_eq!(bash_first_contact(&marked), Some(MapPlan::Build), "a marked repo is contacted normally");
    });
}
