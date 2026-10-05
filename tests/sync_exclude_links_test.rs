//! #164: `sync.exclude` holds against links. The glob follows a symlink (or a Windows junction) under an allowed path,
//! so `notes/alias -> ../private` carried an excluded folder into the graph. Now each file's resolved path gets the
//! exclude test too, the file is not read, and the sync says so once per link.
//!
//! Which legs run: a junction (no admin needed) on every Windows run, a directory symlink on Windows only where the
//! account may create one (Developer Mode or admin; the test prints which), and directory and file symlinks on Unix.

use std::path::Path;
use std::process::{Command, Stdio};

use base::config::BaseConfig;
use base::extract;

const BIN: &str = env!("CARGO_BIN_EXE_base");

/// `ws/private/secret.md` (excluded) and `ws/notes/public.md`, each naming its canary in its title.
fn workspace(root: &Path) {
    std::fs::create_dir_all(root.join(".base")).unwrap();
    std::fs::create_dir_all(root.join("private")).unwrap();
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(root.join("private").join("secret.md"), "---\ntitle: CANARY-PRIVATE-1\n---\n# secret\n").unwrap();
    std::fs::write(root.join("notes").join("public.md"), "---\ntitle: CANARY-PUBLIC-1\n---\n# public\n").unwrap();
}

fn config() -> BaseConfig {
    let mut c = BaseConfig::default();
    c.sync.exclude = vec!["private/".into()];
    c
}

fn graph(ws: &Path) -> String {
    std::fs::read_to_string(ws.join(".base").join("graph.nq")).unwrap_or_default()
}

/// One leg: sync, then the excluded canary is nowhere in the graph, the public one is, and the link is named once.
fn assert_excluded_through(ws: &Path, link_rel: &str, leg: &str) {
    let report = extract::sync(ws, &config(), false).unwrap();
    let g = graph(ws);
    assert!(g.contains("CANARY-PUBLIC-1"), "{leg}: control, the public note is synced");
    assert!(!g.contains("CANARY-PRIVATE-1"), "{leg}: the excluded note came in through the link");
    assert!(!g.to_lowercase().contains("secret"), "{leg}: nothing of the excluded note is in the graph");
    assert_eq!(report.excluded_links.len(), 1, "{leg}: one line per link: {:?}", report.excluded_links);
    let line = &report.excluded_links[0];
    assert_eq!(
        line,
        &format!(
            "base sync skipped {link_rel}: it links into private, which sync.exclude keeps out of the graph, so the 1 file under it was not read."
        ),
        "{leg}"
    );
}

#[cfg(windows)]
fn junction(link: &Path, target: &Path) -> bool {
    Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[test]
fn exclude_holds_through_a_link() {
    let mut legs_run: Vec<&str> = Vec::new();

    #[cfg(windows)]
    {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        workspace(ws);
        assert!(junction(&ws.join("notes").join("alias"), &ws.join("private")), "mklink /J needs no admin");
        assert!(
            std::fs::symlink_metadata(ws.join("notes").join("alias")).unwrap().file_type().is_symlink(),
            "a junction reads as a link"
        );
        assert_excluded_through(ws, "notes/alias", "junction");
        legs_run.push("junction");

        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        workspace(ws);
        match std::os::windows::fs::symlink_dir(ws.join("private"), ws.join("notes").join("alias")) {
            Ok(()) => {
                assert_excluded_through(ws, "notes/alias", "directory symlink");
                legs_run.push("directory symlink");
            }
            // ERROR_PRIVILEGE_NOT_HELD: this account may not create symlinks.
            Err(e) if e.raw_os_error() == Some(1314) => {}
            Err(e) => panic!("symlink_dir failed for another reason: {e}"),
        }
    }

    #[cfg(unix)]
    {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        workspace(ws);
        std::os::unix::fs::symlink("../private", ws.join("notes").join("alias")).unwrap();
        assert_excluded_through(ws, "notes/alias", "directory symlink");
        legs_run.push("directory symlink");

        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        workspace(ws);
        std::os::unix::fs::symlink("../private/secret.md", ws.join("notes").join("secret-link.md")).unwrap();
        assert_excluded_through(ws, "notes/secret-link.md", "file symlink");
        legs_run.push("file symlink");
    }

    eprintln!("exclude_holds_through_a_link: legs run: {}", legs_run.join(", "));
    assert!(!legs_run.is_empty());
}

#[test]
fn a_link_into_an_allowed_folder_is_still_followed() {
    // Only an excluded target is refused: a link to a folder sync may read is read, as before.
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    workspace(ws);
    std::fs::create_dir_all(ws.join("shared")).unwrap();
    std::fs::write(ws.join("shared").join("s.md"), "---\ntitle: CANARY-SHARED-1\n---\n").unwrap();
    #[cfg(windows)]
    assert!(junction(&ws.join("notes").join("shared-link"), &ws.join("shared")));
    #[cfg(unix)]
    std::os::unix::fs::symlink("../shared", ws.join("notes").join("shared-link")).unwrap();
    let report = extract::sync(ws, &config(), false).unwrap();
    assert!(report.excluded_links.is_empty(), "{:?}", report.excluded_links);
    assert!(graph(ws).contains("CANARY-SHARED-1"));
}

#[test]
fn base_sync_prints_one_line_for_the_link() {
    // The reporter's shape, through the binary: the exclude comes from the workspace's own base.toml.
    let home = tempfile::tempdir().unwrap();
    let ws = home.path().join("ws");
    workspace(&ws);
    std::fs::write(ws.join(".base").join("base.toml"), "[sync]\nexclude = [\"private/\"]\n").unwrap();
    #[cfg(windows)]
    assert!(junction(&ws.join("notes").join("alias"), &ws.join("private")));
    #[cfg(unix)]
    std::os::unix::fs::symlink("../private", ws.join("notes").join("alias")).unwrap();

    let out = Command::new(BIN)
        .args(["sync", "--yes"])
        .current_dir(&ws)
        .env("BASE_HOME", home.path())
        .env("BASE_NO_AUTO_UPDATE", "1")
        .env("BASE_AST_NO_SPAWN", "1")
        .env_remove("BASE_RELAY_AS")
        .env_remove("CLAUDECODE")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    let lines: Vec<&str> = err.lines().filter(|l| l.starts_with("base sync skipped notes/alias")).collect();
    assert_eq!(lines.len(), 1, "{err}");
    let g = graph(&ws);
    assert!(g.contains("CANARY-PUBLIC-1") && !g.contains("CANARY-PRIVATE-1"));
}
