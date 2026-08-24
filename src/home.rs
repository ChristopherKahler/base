//! The one answer to "where is the user's home directory".
//!
//! Every `~`-rooted path in this crate resolves through [`home_root`]. A raw
//! `dirs::home_dir()` is banned — `clippy.toml` denies it and
//! `tests/guard_test.rs` greps for it — because reaching for the home directory
//! at the call site is what let `cargo test` write to the operator's real global
//! graph. See the fork `base-tests-write-live-graph`.

use std::path::PathBuf;

/// Explicit override, consulted before the OS.
pub const HOME_ENV: &str = "BASE_HOME";

/// `BASE_HOME` when set, else the OS home directory.
///
/// Re-read on every call rather than cached: a cached value makes thread
/// ordering matter under parallel `cargo test`, where the first caller would
/// win and every later one would silently inherit its answer.
///
/// `BASE_HOME` rather than `$HOME`, because `dirs` never consults `$HOME` on
/// Windows — `dirs-5.0.1/src/win.rs:5` is `known_folder_profile()`, i.e.
/// `FOLDERID_Profile`. A `$HOME`-based fake therefore isolates under WSL and
/// silently fails to isolate on Windows, which is the worse of the two failure
/// modes and the one this seam exists to remove.
pub fn home_root() -> Option<PathBuf> {
    #[cfg(feature = "isolation-guard")]
    if let Some(t) = thread_home() {
        return Some(t);
    }
    if let Some(over) = std::env::var_os(HOME_ENV)
        && !over.is_empty()
    {
        return Some(PathBuf::from(over));
    }
    #[cfg(feature = "isolation-guard")]
    {
        // Test builds never fall through to the real home. `cfg(test)` cannot
        // do this job: `tests/*.rs` link the library compiled WITHOUT
        // `cfg(test)`, so an integration test that forgot to isolate would read
        // the real home with nothing to stop it. The feature is enabled by the
        // self-dev-dependency in Cargo.toml, so it covers both test binaries
        // and is off for every `cargo build` / `cargo run`.
        return Some(test_root());
    }
    #[cfg(not(feature = "isolation-guard"))]
    real_home()
}

/// The OS home directory with no override applied.
///
/// The only sanctioned caller outside this module is the write tripwire in
/// [`crate::store`], which must compare against the real profile no matter what
/// the override says.
#[allow(clippy::disallowed_methods)]
pub fn real_home() -> Option<PathBuf> {
    dirs::home_dir()
}

/// True when this process is running isolated — either the override is set or
/// the crate was built for tests. Arms the write tripwire; see
/// [`assert_isolated_write`].
pub fn isolation_active() -> bool {
    cfg!(feature = "isolation-guard") || std::env::var_os(HOME_ENV).is_some()
}

/// Per-thread home override, for tests that each need their OWN fake home
/// rather than the one shared process-wide root.
///
/// `cargo test` runs every test on its own thread, so a thread-local is exact
/// here: no mutex, no poisoning, and no way for one test's root to leak into a
/// test running beside it. The process-global `BASE_HOME` cannot do this job —
/// it is one value for the whole process, so the relay tests, which assert that
/// distinct sessions get distinct inboxes, would all collide on one directory.
///
/// This is deliberately NOT how the graph write path is isolated. There the
/// root is a function parameter, because the compiler refusing to build a
/// forgotten argument is worth more than the convenience — see
/// `crud::handoff::all_tier_files`.
#[cfg(feature = "isolation-guard")]
mod thread_override {
    use std::cell::RefCell;
    use std::path::PathBuf;

    thread_local! {
        pub(super) static HOME: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    }
}

#[cfg(feature = "isolation-guard")]
fn thread_home() -> Option<PathBuf> {
    thread_override::HOME.with(|h| h.borrow().clone())
}

/// Run `f` with this thread's home pointed at `root`, then restore the previous
/// value. No unwind guard: a panicking test takes its own thread down with it,
/// so there is nothing left to restore and — unlike the mutex this replaced —
/// nothing for a sibling test to trip over.
#[cfg(feature = "isolation-guard")]
pub fn with_thread_home<T>(root: &std::path::Path, f: impl FnOnce() -> T) -> T {
    let prev = thread_override::HOME.with(|h| h.replace(Some(root.to_path_buf())));
    let out = f();
    thread_override::HOME.with(|h| *h.borrow_mut() = prev);
    out
}

/// Panics when a graph write is about to land on a real global tier while this
/// process is isolated. Armed by [`isolation_active`] — the feature alone is
/// enough, so a redirected test with no `BASE_HOME` set is still covered.
///
/// Keyed on `<home>/.base-gbl`, deliberately NOT on the home directory itself.
/// On Windows `%TEMP%` is `C:\Users\<user>\AppData\Local\Temp`, i.e. UNDER
/// the profile directory, so every legitimate isolated write there sits under
/// the real home. Widening this to `starts_with(real_home)` would panic on all
/// of them, on the one platform this fork exists to protect. Do not "simplify"
/// it back.
pub fn assert_isolated_write(path: &std::path::Path) {
    if !isolation_active() {
        return;
    }

    // The operator's real global tier is never a legitimate target under
    // isolation, whatever the override happens to say.
    if let Some(real) = real_home()
        && path.starts_with(real.join(".base-gbl"))
    {
        panic!(
            "isolation breach: graph write to {} — the REAL global tier — while isolated. \
             Some path resolved the home directory without going through home::home_root().",
            path.display()
        );
    }

    // General case: any write under the real profile that is not in the system
    // temp directory. Catches real-home targets other than the global tier
    // while still allowing Windows `%TEMP%`, which lives UNDER the profile.
    //
    // Deliberately not "any `.base-gbl` outside the isolated home": a test may
    // legitimately build its own fake global tier inside its own temp workspace
    // — `changelog_test::workspace_and_global_tiers_each_get_their_own_log`
    // does exactly that — and flagging it confuses a sandbox with the real one.
    if let Some(real) = real_home()
        && path.starts_with(&real)
        && !path.starts_with(std::env::temp_dir())
    {
        panic!(
            "isolation breach: graph write to {} — under the real home — while isolated. \
             Some path resolved the home directory without going through home::home_root().",
            path.display()
        );
    }
}

/// One fake home per test process, shared by every thread in it.
///
/// A plain directory under the system temp dir rather than a `TempDir`, because
/// a `TempDir` parked in a `static` never runs `Drop` — it would leak exactly
/// the same directory while pulling `tempfile` into the non-dev dependency list
/// to do it. The OS cleans its own temp dir.
#[cfg(feature = "isolation-guard")]
fn test_root() -> PathBuf {
    static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("base-test-home-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&p);
        p
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_wins_over_the_os_home() {
        // Set and read on this thread only via a child-free check: the override
        // is process-global, so this test asserts the precedence rule using the
        // value the harness already installed rather than mutating it.
        if let Some(set) = std::env::var_os(HOME_ENV) {
            assert_eq!(home_root(), Some(PathBuf::from(set)));
        }
    }

    #[test]
    fn never_resolves_to_the_real_global_tier_under_test() {
        let root = home_root().expect("a test build always resolves a home");
        if let Some(real) = real_home() {
            assert_ne!(
                root.join(".base-gbl"),
                real.join(".base-gbl"),
                "a test build must never resolve the operator's real global tier"
            );
        }
    }

    #[test]
    fn isolation_is_active_in_a_test_build() {
        assert!(
            isolation_active(),
            "the isolation-guard feature must be on for `cargo test`; \
             without it the tripwire is unarmed and this whole fork is inert"
        );
    }
}
