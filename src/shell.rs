//! Which `bash` base starts (F19, BO-08).
//!
//! ON WINDOWS A BARE `bash` IS THE WRONG PROGRAM. `Command::new("bash")` goes through `CreateProcess`, which searches
//! the application directory and `System32` before `PATH`, and `C:\Windows\System32\bash.exe` is the WSL launcher.
//! Measured 2026-09-21 (flint, at 566c753): from a Git Bash parent and from a PowerShell parent, a Rust child spawning
//! `bash` landed in WSL's bash 5.2 with `HOME=/home/<user>`, while `which -a bash` in both shells listed Git's
//! `usr/bin/bash.exe` first. A plugin's `prepare.sh` started that way ran inside Linux, with Linux paths; on a machine
//! with neither WSL nor Git it failed with an error that named neither.
//!
//! So on Windows [`host_bash`] walks `PATH` itself, skips every folder under `%SystemRoot%` (the WSL launcher) and every
//! `WindowsApps` folder (the Store's alias stubs), and returns the absolute path of the first `bash.exe` left, so
//! `Command` cannot re-resolve it.
//!
//! A DEFAULT GIT FOR WINDOWS INSTALL PUTS NO `bash.exe` ON `PATH`. Its installer adds only `Git\cmd`, which holds
//! `git.exe`; bash sits in `Git\bin` and `Git\usr\bin`, and only a Git Bash or Claude Code session has those on `PATH`.
//! Checked on Chris's machine 2026-10-02: the machine and user `PATH` hold `C:\Program Files\Git\cmd` and nothing else of
//! Git's, so `base` started from a plain PowerShell found no bash. So when no `bash.exe` is on `PATH`, the search takes
//! `<Git>\bin\bash.exe` beside a `git.exe` that is. That one, not `usr\bin\bash.exe`: `bin\bash.exe` is Git's wrapper,
//! which puts Git's own tools on the child's `PATH`. Started from a parent with only `Git\cmd` on `PATH`, `bin\bash.exe`
//! ran `uname -s` and found `sed`; `usr\bin\bash.exe` answered `uname: command not found` (measured the same day).
//!
//! It never falls back to the launcher: when nothing is found, the error says what to install ([`NO_BASH`]). Elsewhere
//! `bash` on `PATH` is the real thing, and the bare name is returned.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// What [`host_bash`] says when Windows has no bash it may use (F19b).
pub const NO_BASH: &str =
    "base needs Git Bash to run prepare.sh on Windows. Install Git for Windows, or run this from WSL.";

/// The bash base runs a plugin's `prepare.sh` with: on Windows the absolute path of the first `bash.exe` on `PATH`
/// outside `%SystemRoot%` and `WindowsApps`, else Git's `bin\bash.exe` beside a `git.exe` on `PATH`, else [`NO_BASH`];
/// elsewhere `bash`. Every product caller runs `prepare.sh`, which is why the error names it (F19b's wording).
pub fn host_bash() -> anyhow::Result<PathBuf> {
    if cfg!(windows) {
        windows_bash(std::env::var_os("PATH").as_deref(), std::env::var_os("SystemRoot").as_deref())
    } else {
        Ok(PathBuf::from("bash"))
    }
}

/// The Windows search over an explicit `PATH` and `SystemRoot`, so it runs on every platform and a test can hand it
/// any folders it likes. `system_root` unset means `C:\Windows`. A relative `PATH` entry is skipped, because only an
/// absolute path keeps `Command` from searching again.
pub fn windows_bash(path: Option<&OsStr>, system_root: Option<&OsStr>) -> anyhow::Result<PathBuf> {
    let root = system_root
        .map(|r| normalized(&r.to_string_lossy()))
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| r"c:\windows".to_string());
    let dirs: Vec<PathBuf> = path
        .map(std::env::split_paths)
        .into_iter()
        .flatten()
        .filter(|dir| dir.is_absolute() && !skipped(dir, &root))
        .collect();
    let on_path = dirs.iter().map(|dir| dir.join("bash.exe")).find(|candidate| candidate.is_file());
    // No bash.exe on PATH: Git's own `bin\bash.exe`, one or two folders above a git.exe that is (`Git\cmd\git.exe`,
    // `Git\mingw64\bin\git.exe`). The same skips hold, so a git.exe under the Windows folder leads nowhere.
    let beside_git = || {
        dirs.iter()
            .filter(|dir| dir.join("git.exe").is_file())
            .flat_map(|dir| dir.ancestors().skip(1).take(2))
            .map(|git_root| git_root.join("bin").join("bash.exe"))
            .find(|candidate| candidate.is_file() && candidate.parent().is_some_and(|p| !skipped(p, &root)))
    };
    on_path.or_else(beside_git).ok_or_else(|| anyhow::anyhow!(NO_BASH))
}

/// A `PATH` folder the search must not take bash from: `%SystemRoot%` or anything under it, and any `WindowsApps`
/// folder. (An entry whose `%SystemRoot%` was never expanded has no drive, so the relative-entry rule already drops it.)
fn skipped(dir: &Path, root: &str) -> bool {
    let d = normalized(&dir.to_string_lossy());
    let under_root = d == root || d.strip_prefix(root).is_some_and(|rest| rest.starts_with('\\'));
    under_root || d.split('\\').any(|part| part == "windowsapps")
}

/// Lower case, backslashes, no trailing separator: Windows compares paths without case, and `PATH` mixes both
/// separators.
fn normalized(p: &str) -> String {
    p.to_lowercase().replace('/', "\\").trim_end_matches('\\').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("folder");
        std::fs::write(path, b"").expect("file");
    }

    fn joined(dirs: &[&Path]) -> OsString {
        std::env::join_paths(dirs).expect("folders join into a PATH")
    }

    /// F19b: a `PATH` with no bash gives the error that says what to install, and never the launcher, even with a
    /// `bash.exe` in `System32` and another in a `WindowsApps` folder ahead of it.
    #[test]
    fn missing_bash_gives_clear_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("Windows");
        let system32 = root.join("System32");
        let apps = tmp.path().join("Users").join("me").join("AppData").join("Local").join("Microsoft").join("WindowsApps");
        let empty = tmp.path().join("tools");
        touch(&system32.join("bash.exe"));
        touch(&apps.join("bash.exe"));
        std::fs::create_dir_all(&empty).expect("folder");

        let err = windows_bash(Some(&joined(&[&system32, &apps, &empty])), Some(root.as_os_str()))
            .expect_err("only the launcher and a Store stub are on PATH");
        assert_eq!(err.to_string(), NO_BASH);
        assert_eq!(
            err.to_string(),
            "base needs Git Bash to run prepare.sh on Windows. Install Git for Windows, or run this from WSL."
        );

        let err = windows_bash(None, Some(root.as_os_str())).expect_err("no PATH at all");
        assert_eq!(err.to_string(), NO_BASH);
    }

    /// The search takes the first `bash.exe` outside the Windows folder and the Store folders, as an absolute path,
    /// whatever case or separators `PATH` and `SystemRoot` use.
    #[test]
    fn windows_search_skips_the_launcher_and_returns_an_absolute_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("Windows");
        let system32 = root.join("System32");
        let apps = tmp.path().join("WindowsApps");
        let git = tmp.path().join("Git").join("usr").join("bin");
        let later = tmp.path().join("later");
        for dir in [&system32, &apps, &git, &later] {
            touch(&dir.join("bash.exe"));
        }

        let got = windows_bash(Some(&joined(&[&system32, &apps, &git, &later])), Some(root.as_os_str()))
            .expect("Git's bash is on PATH");
        assert_eq!(got, git.join("bash.exe"));
        assert!(got.is_absolute(), "{}", got.display());

        // SystemRoot written in another case, with a trailing separator: still the Windows folder.
        let shouty = OsString::from(format!("{}\\", root.to_string_lossy().to_uppercase()));
        let got = windows_bash(Some(&joined(&[&system32, &git])), Some(&shouty)).expect("Git's bash");
        assert_eq!(got, git.join("bash.exe"));

        // A folder whose name only starts like the Windows folder is not under it.
        let lookalike = tmp.path().join("Windows-tools");
        touch(&lookalike.join("bash.exe"));
        let got = windows_bash(Some(&joined(&[&system32, &lookalike])), Some(root.as_os_str())).expect("a bash");
        assert_eq!(got, lookalike.join("bash.exe"));
    }

    /// `to` written relative to `from`, or `None` when they share no root (two drives).
    fn relative(from: &Path, to: &Path) -> Option<PathBuf> {
        let (from, to): (Vec<_>, Vec<_>) = (from.components().collect(), to.components().collect());
        let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
        if common == 0 {
            return None;
        }
        let mut rel: PathBuf = std::iter::repeat_n("..", from.len() - common).collect();
        rel.extend(&to[common..]);
        Some(rel)
    }

    /// A relative `PATH` entry is never taken, even when it names a real `bash.exe` from the current folder: only an
    /// absolute path keeps `Command` from searching again. The folder sits beside this test binary, so it shares a
    /// drive with the current folder on CI and on a dev machine alike.
    #[test]
    fn windows_search_skips_relative_entries() {
        let cwd = std::env::current_dir().expect("cwd");
        let exe = std::env::current_exe().expect("the test binary");
        let tmp = tempfile::tempdir_in(exe.parent().expect("its folder")).expect("tempdir");
        let near = tmp.path().join("near");
        touch(&near.join("bash.exe"));
        let Some(rel) = relative(&cwd, &near) else {
            panic!("{} and {} share no root, so no relative entry can name the folder", cwd.display(), near.display());
        };
        assert!(rel.is_relative() && rel.join("bash.exe").is_file(), "the relative entry names a real file: {}", rel.display());

        let err = windows_bash(Some(&joined(&[&rel])), Some(OsStr::new(r"C:\Windows")))
            .expect_err("a relative entry is skipped");
        assert_eq!(err.to_string(), NO_BASH);

        let far = tmp.path().join("far");
        touch(&far.join("bash.exe"));
        let got = windows_bash(Some(&joined(&[&rel, &far])), Some(OsStr::new(r"C:\Windows"))).expect("the absolute entry");
        assert_eq!(got, far.join("bash.exe"));
    }

    /// A default Git for Windows install: only `Git\cmd` (git.exe) on PATH. The search takes Git's `bin\bash.exe`, not
    /// `usr\bin\bash.exe` and not the launcher; `Git\mingw64\bin\git.exe` leads to the same file; a bash.exe on PATH
    /// still comes first; and a git.exe whose Git folder has no `bin\bash.exe` gives the F19b error.
    #[test]
    fn default_git_install_finds_bash_beside_git_exe() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("Windows");
        let system32 = root.join("System32");
        let git = tmp.path().join("Program Files").join("Git");
        touch(&system32.join("bash.exe"));
        touch(&git.join("cmd").join("git.exe"));
        touch(&git.join("mingw64").join("bin").join("git.exe"));
        touch(&git.join("bin").join("bash.exe"));
        touch(&git.join("usr").join("bin").join("bash.exe"));
        let want = git.join("bin").join("bash.exe");

        let got = windows_bash(Some(&joined(&[&system32, &git.join("cmd")])), Some(root.as_os_str())).expect("Git's bash");
        assert_eq!(got, want);
        let got = windows_bash(Some(&joined(&[&git.join("mingw64").join("bin")])), Some(root.as_os_str())).expect("Git's bash");
        assert_eq!(got, want);

        let tools = tmp.path().join("tools");
        touch(&tools.join("bash.exe"));
        let got = windows_bash(Some(&joined(&[&git.join("cmd"), &tools])), Some(root.as_os_str())).expect("a bash");
        assert_eq!(got, tools.join("bash.exe"), "a bash.exe on PATH comes before one found through git.exe");

        let bare = tmp.path().join("BareGit");
        touch(&bare.join("cmd").join("git.exe"));
        let err = windows_bash(Some(&joined(&[&bare.join("cmd")])), Some(root.as_os_str())).expect_err("no bin\\bash.exe");
        assert_eq!(err.to_string(), NO_BASH);

        // A git.exe in the Windows folder is skipped like a bash.exe there, so it cannot lead to a bash beside it.
        touch(&system32.join("git.exe"));
        touch(&root.join("bin").join("bash.exe"));
        let err = windows_bash(Some(&joined(&[&system32])), Some(root.as_os_str())).expect_err("the Windows folder");
        assert_eq!(err.to_string(), NO_BASH);
    }

    /// Off Windows the bare name is the real bash.
    #[test]
    fn host_bash_is_the_bare_name_off_windows() {
        let got = host_bash();
        if cfg!(windows) {
            if let Ok(p) = got {
                assert!(p.is_absolute(), "{}", p.display());
            }
            return;
        }
        assert_eq!(got.expect("bash"), PathBuf::from("bash"));
    }
}
