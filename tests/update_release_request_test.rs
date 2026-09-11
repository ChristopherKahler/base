//! #158 leg D: `base update` sends a token the operator already has, and a 403
//! that is a rate limit says so instead of blaming the network.
//!
//! ── WHY THIS FILE IS SHAPED THE WAY IT IS ──
//!
//! `GITHUB_TOKEN` and `GH_TOKEN` are ALREADY in the shipped binary, twice each,
//! from `src/plugin/dist.rs`. A `strings` or `git grep` detector for this leg
//! therefore passes on a tree where this leg does not exist. Nothing in this file
//! greps for a token name, and nothing here would pass before the change.
//!
//! The grading criterion, from `tanager`: a test must observe the request THE
//! PRODUCT CONSTRUCTS on its real code path, not one the test built by calling a
//! helper. Calling a header-building helper and inspecting what it handed back is
//! still a helper test, and the wiring gap survives it untouched.
//!
//! So `fetch_latest_release` takes its transport as a parameter. Production
//! passes `|req| req.call()`. Every test here passes a closure that captures the
//! `ureq::Request` the product itself just built, asserts on that object, and
//! answers it from memory. No test in this file opens a socket.
//!
//! ── THE HOLE A SEAM TEST LEAVES, AND HOW IT IS CLOSED ──
//!
//! A seam proves the header is right and that the builder gets called. It cannot
//! prove production still routes through the builder, and a delegating one-liner
//! is exactly the kind of thing a later refactor drops. `auk` ruled that closed
//! by construction rather than by another test: there is exactly ONE place in
//! `src/update/mod.rs` that builds a release-API request, so there is nowhere
//! else for a second request to come from. `the_release_request_has_exactly_one_
//! construction_site` is that guard.
//!
//! That guard counts over CODE lines, with comment lines stripped first. A
//! whole-file count would also match this module's own prose, so it could read 3
//! on a correct tree and 1 on a broken one — worse than no guard at all.
//!
//! ── THE MUTATION THAT SETTLES IT ──
//!
//! Strip the `authorization` wiring out of `fetch_latest_release`, rebuild cold,
//! and the three token tests below must go RED. If they stay green they are
//! reading a helper rather than the product, and leg D is not done however green
//! it looks.
//!
//! ── ISOLATION ──
//!
//! `GITHUB_TOKEN` and `GH_TOKEN` are process-global and every test here runs in
//! one binary, so the tests that touch them take `ENV_LOCK` first and put the
//! variables back afterwards — including restoring "was not set at all", which is
//! a different state from "was set to empty".
//!
//! Nothing here reads or writes a file, a home directory, or a config. The
//! update path resolves its token from two environment variables and nothing
//! else, so there is no other state for a test to reach or to have to isolate.

use std::cell::RefCell;
use std::sync::{Mutex, MutexGuard};

use base::update::{asset_name, fetch_latest_release};

/// `std::env::set_var` changes the whole process. Without this lock two tests in
/// this binary would read each other's environment, and the failure would look
/// like a bug in the code under test rather than in the harness.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn env_lock() -> MutexGuard<'static, ()> {
    // A panicking test must not poison the lock for every test after it: that
    // turns one real failure into a screenful of unrelated ones.
    ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// Both token variables, cleared for the duration of a test and put back exactly
/// as they were when it ends.
struct TokenEnv {
    saved: Vec<(&'static str, Option<String>)>,
}

impl TokenEnv {
    fn cleared() -> Self {
        let saved: Vec<(&'static str, Option<String>)> = ["GITHUB_TOKEN", "GH_TOKEN"]
            .iter()
            .map(|k| (*k, std::env::var(k).ok()))
            .collect();
        for (k, _) in &saved {
            unsafe { std::env::remove_var(k) };
        }
        Self { saved }
    }

    fn set(&self, key: &str, value: &str) {
        unsafe { std::env::set_var(key, value) };
    }
}

impl Drop for TokenEnv {
    fn drop(&mut self) {
        for (k, v) in &self.saved {
            match v {
                Some(val) => unsafe { std::env::set_var(k, val) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
    }
}

/// A releases-API answer the real parser can read: a tag, and an asset named for
/// whatever platform this test is running on.
fn ok_release_response() -> ureq::Response {
    let asset = asset_name().expect("this platform has a published asset name");
    let body = format!(
        r#"{{"tag_name":"v9.9.9","assets":[{{"name":"{asset}","browser_download_url":"https://example.invalid/{asset}"}}]}}"#
    );
    format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\r\n{body}")
        .parse()
        .expect("canned 200 parses")
}

/// A 403 that is out of API budget. `x-ratelimit-remaining: 0` is the only thing
/// that distinguishes this from a plain refusal.
fn rate_limited_403() -> ureq::Error {
    let resp: ureq::Response = "HTTP/1.1 403 Forbidden\r\nx-ratelimit-remaining: 0\r\n\r\n"
        .parse()
        .expect("canned 403 parses");
    ureq::Error::Status(403, resp)
}

/// A 403 with budget to spare: a real refusal, not a rate limit.
fn plain_403() -> ureq::Error {
    let resp: ureq::Response = "HTTP/1.1 403 Forbidden\r\nx-ratelimit-remaining: 4999\r\n\r\n"
        .parse()
        .expect("canned 403 parses");
    ureq::Error::Status(403, resp)
}

/// A genuine `ureq` transport failure.
///
/// `ureq::Error::Transport` cannot be constructed from outside ureq —
/// `ErrorKind::msg` and `ErrorKind::new` are both `pub(crate)` in 2.12.1 — so
/// this raises a real one instead. It opens no socket: `Request::call` runs
/// `parse_url()?` before it constructs a `Unit` or calls `unit::connect`, and a
/// URL with no host fails that parse. ureq's own `disallow_empty_host` test
/// asserts this same call yields `ErrorKind::InvalidUrl`.
fn transport_failure() -> ureq::Error {
    let err = ureq::get("file:///some/path")
        .call()
        .expect_err("a url with no host cannot succeed");
    assert!(
        matches!(err, ureq::Error::Transport(_)),
        "expected a transport failure, got {err:?}"
    );
    err
}

/// Drive the product and collect the `authorization` header off every request it
/// actually built, in order.
fn auth_headers_seen<F>(answer: F) -> (Vec<Option<String>>, anyhow::Result<(String, String)>)
where
    F: Fn(usize) -> std::result::Result<ureq::Response, ureq::Error>,
{
    let seen: RefCell<Vec<Option<String>>> = RefCell::new(Vec::new());
    let out = fetch_latest_release(|req| {
        let n = {
            let mut s = seen.borrow_mut();
            s.push(req.header("authorization").map(str::to_string));
            s.len()
        };
        answer(n)
    });
    (seen.into_inner(), out)
}

// ── D-2: the token reaches the request the product builds ──────────────────

#[test]
fn token_in_the_environment_reaches_the_request_the_updater_builds() {
    let _lock = env_lock();
    let env = TokenEnv::cleared();
    env.set("GITHUB_TOKEN", "gho_shrike_github_token");

    let seen: RefCell<Vec<(Option<String>, Option<String>, Option<String>)>> =
        RefCell::new(Vec::new());
    let out = fetch_latest_release(|req| {
        seen.borrow_mut().push((
            req.header("authorization").map(str::to_string),
            req.header("user-agent").map(str::to_string),
            req.header("accept").map(str::to_string),
        ));
        Ok(ok_release_response())
    });

    let calls = seen.into_inner();
    assert_eq!(calls.len(), 1, "the healthy path must send exactly one request");
    assert_eq!(
        calls[0].0.as_deref(),
        Some("Bearer gho_shrike_github_token"),
        "the request the updater built carried no Authorization header"
    );
    // HEALTHY control: the headers that were already right must survive the change.
    assert_eq!(calls[0].1.as_deref(), Some("base-cli-updater"), "user-agent was lost");
    assert_eq!(
        calls[0].2.as_deref(),
        Some("application/vnd.github+json"),
        "accept was lost"
    );
    assert_eq!(out.expect("canned 200 parses into a release").0, "9.9.9");
}

#[test]
fn gh_token_is_read_when_github_token_is_not_set() {
    let _lock = env_lock();
    let env = TokenEnv::cleared();
    env.set("GH_TOKEN", "gho_shrike_gh_token");

    let (calls, out) = auth_headers_seen(|_| Ok(ok_release_response()));

    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].as_deref(),
        Some("Bearer gho_shrike_gh_token"),
        "GH_TOKEN was not read when GITHUB_TOKEN was absent"
    );
    assert!(out.is_ok());
}

#[test]
fn github_token_wins_when_both_are_set() {
    let _lock = env_lock();
    let env = TokenEnv::cleared();
    env.set("GITHUB_TOKEN", "gho_first");
    env.set("GH_TOKEN", "gho_second");

    let (calls, _) = auth_headers_seen(|_| Ok(ok_release_response()));

    assert_eq!(
        calls[0].as_deref(),
        Some("Bearer gho_first"),
        "resolution order must be GITHUB_TOKEN then GH_TOKEN"
    );
}

/// ABSENT control. Without it the suite could not tell "sends the token" from
/// "sends an Authorization header no matter what".
#[test]
fn no_token_in_the_environment_sends_no_authorization_header() {
    let _lock = env_lock();
    let _env = TokenEnv::cleared();

    let (calls, out) = auth_headers_seen(|_| Ok(ok_release_response()));

    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0], None,
        "an anonymous check must send no Authorization header at all"
    );
    assert!(out.is_ok(), "the anonymous path must still work");
}

/// An empty variable is not a token. Exporting `GITHUB_TOKEN=` is a common way to
/// mean "no token", and sending `Bearer ` would turn a working anonymous check
/// into a 401.
#[test]
fn an_empty_token_variable_is_treated_as_no_token() {
    let _lock = env_lock();
    let env = TokenEnv::cleared();
    env.set("GITHUB_TOKEN", "");

    let (calls, _) = auth_headers_seen(|_| Ok(ok_release_response()));

    assert_eq!(calls[0], None, "an empty GITHUB_TOKEN must not become a Bearer header");
}

// ── DoD 17: `base update` spawns no subprocess on the common path ──────────

/// One request per check, whatever the outcome.
///
/// `base update` runs from `hook/session_start.rs` on EVERY session start, so
/// anything per-check is paid on every session. A retry is the shape that would
/// reintroduce a second request and, with it, the temptation to reach for a
/// token source that shells out. Counting requests pins that shut: the update
/// path resolves its token from two environment variables, which cannot spawn
/// anything, and it asks once.
#[test]
fn every_outcome_sends_exactly_one_request() {
    let _lock = env_lock();
    let _env = TokenEnv::cleared();

    let (healthy, out) = auth_headers_seen(|_| Ok(ok_release_response()));
    assert_eq!(healthy.len(), 1, "a healthy check made {} requests", healthy.len());
    assert!(out.is_ok());

    let (limited, err) = auth_headers_seen(|_| Err(rate_limited_403()));
    assert_eq!(
        limited.len(),
        1,
        "a rate-limited check made {} requests; there is no retry on this path",
        limited.len()
    );
    assert!(err.is_err());
}

// ── D-1: a 403 says what actually happened ─────────────────────────────────

#[test]
fn a_rate_limited_403_says_rate_limit_and_does_not_blame_the_network() {
    let _lock = env_lock();
    let env = TokenEnv::cleared();
    // A token is present, so the rate limit is terminal: there is nothing wider
    // left to consult and no retry to confuse the message being measured.
    env.set("GITHUB_TOKEN", "gho_already_using_a_token");

    let err = fetch_latest_release(|_req| Err(rate_limited_403()))
        .expect_err("a 403 cannot yield a release");
    let msg = err.to_string();

    assert!(msg.contains("rate limit"), "a 403 rate limit did not say rate limit: {msg}");
    assert!(
        !msg.contains("could not reach"),
        "a 403 rate limit still blamed the network: {msg}"
    );
    assert!(
        msg.contains("GITHUB_TOKEN") || msg.contains("GH_TOKEN"),
        "the message names no way out of the rate limit: {msg}"
    );
}

#[test]
fn a_403_that_is_not_a_rate_limit_says_forbidden_rather_than_rate_limit() {
    let _lock = env_lock();
    let env = TokenEnv::cleared();
    env.set("GITHUB_TOKEN", "gho_token");

    let err = fetch_latest_release(|_req| Err(plain_403())).expect_err("403 is not a release");
    let msg = err.to_string();

    assert!(msg.contains("403") || msg.contains("forbidden"), "unclear 403 message: {msg}");
    assert!(!msg.contains("rate limit"), "a plain 403 was reported as a rate limit: {msg}");
    assert!(!msg.contains("could not reach"), "a plain 403 blamed the network: {msg}");
}

#[test]
fn a_status_that_is_not_403_names_the_status() {
    let _lock = env_lock();
    let env = TokenEnv::cleared();
    env.set("GITHUB_TOKEN", "gho_token");

    let err = fetch_latest_release(|_req| {
        let resp: ureq::Response = "HTTP/1.1 500 Internal Server Error\r\n\r\n"
            .parse()
            .expect("canned 500 parses");
        Err(ureq::Error::Status(500, resp))
    })
    .expect_err("500 is not a release");
    let msg = err.to_string();

    assert!(msg.contains("500"), "a 500 did not name its status: {msg}");
    assert!(!msg.contains("rate limit"), "a 500 was reported as a rate limit: {msg}");
}

/// HEALTHY control for the sentence that was there before. "could not reach the
/// GitHub releases API" is true of a transport failure and of nothing else, so it
/// has to survive this change for exactly that case.
#[test]
fn a_transport_failure_still_says_could_not_reach() {
    let _lock = env_lock();
    let env = TokenEnv::cleared();
    env.set("GITHUB_TOKEN", "gho_token");

    let err =
        fetch_latest_release(|_req| Err(transport_failure())).expect_err("no response, no release");
    let msg = err.to_string();

    assert!(
        msg.contains("could not reach the GitHub releases API"),
        "a transport failure lost the only message that is true of it: {msg}"
    );
    assert!(!msg.contains("rate limit"), "a transport failure claimed a rate limit: {msg}");
}

/// The case the user actually hits: no token set, rate limit reached.
///
/// This is the whole reason the message matters. An operator with no token, on a
/// shared or busy IP, gets refused — and the old sentence told them their network
/// was down. The new one tells them what is true and what fixes it.
#[test]
fn a_rate_limit_with_no_token_set_reports_the_rate_limit_and_points_at_the_fix() {
    let _lock = env_lock();
    let _env = TokenEnv::cleared();

    let (calls, out) = auth_headers_seen(|_| Err(rate_limited_403()));

    assert_eq!(calls.len(), 1, "one check, one request");
    assert_eq!(calls[0], None, "no token was set, so the request must be anonymous");

    let msg = out.expect_err("a rate limit is not a release").to_string();
    assert!(msg.contains("rate limit"), "the rate limit was not reported: {msg}");
    assert!(!msg.contains("could not reach"), "it still blamed the network: {msg}");
    assert!(
        msg.contains("GITHUB_TOKEN") || msg.contains("GH_TOKEN"),
        "the operator is not told what would fix it: {msg}"
    );
}

// ── The guard that makes a second request impossible ───────────────────────

/// There must be exactly ONE place in `src/update/mod.rs` that builds a
/// release-API request.
///
/// This is the only structural assertion in the file, and it earns its place:
/// the seam tests above prove the header is built correctly and that production
/// calls the builder, but they cannot prove production has not grown a SECOND
/// request that skips it. With one construction site that question cannot arise,
/// because there is nowhere else to build one.
///
/// It counts occurrences rather than presence, which is what makes it safe here.
/// A presence check on a token name would pass today on a tree with no leg D at
/// all, because `GITHUB_TOKEN` and `GH_TOKEN` already appear in the binary from
/// `src/plugin/dist.rs`. A uniqueness check on a symbol that exists in exactly
/// one module cannot pass by accident.
///
/// Comment lines are stripped before counting. The module header and the
/// constant's own doc comment discuss this request in prose, so a whole-file
/// count would mix the author's writing in with the code and could read 3 on a
/// correct tree.
#[test]
fn the_release_request_has_exactly_one_construction_site() {
    let src = include_str!("../src/update/mod.rs");
    let code_only: String = src
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");

    let sites = code_only.matches("ureq::get(LATEST_RELEASE_API)").count();
    assert_eq!(
        sites, 1,
        "expected exactly 1 release-API request construction site in code, found {sites}. \
         More than one means production can build a request that skips the Authorization \
         wiring; zero means the request moved and this guard no longer measures anything."
    );

    // The guard must be measuring code, not prose. If stripping comments removed
    // every occurrence, the count above would be meaningless.
    assert!(
        code_only.contains("fn fetch_latest_release"),
        "comment stripping removed the code region this guard is supposed to scan"
    );
}
