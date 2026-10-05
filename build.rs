//! Stamps the build's provenance into the binary.
//!
//! `base --version` is the only thing on the machine that can answer "which
//! fixes are in this binary". On 2026-09-20 it could not: two seats installed
//! over each other, one silently reverting the other's fix, and nothing on the
//! box could tell the two binaries apart. Both said `base 0.15.2`. Installing
//! is a file copy, not a merge, so there was no conflict and no warning.
//!
//! Resolution order. The fallback is deliberate:
//!   1. `BASE_BUILD_SHA` -- set by a build that knows its own source. The
//!      Windows path copies the tree WITHOUT `.git`, so git cannot answer there
//!      and this is the only route that works for it.
//!   2. `git rev-parse` -- CI and any in-tree build.
//!   3. `"unknown"` -- and that is a USEFUL answer, not a failure. It says the
//!      binary came from a tree with no provenance, which is exactly the
//!      condition that made the incident invisible. A build id that guessed
//!      would be worse than one that admits it does not know.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=BASE_BUILD_SHA");
    println!("cargo:rerun-if-changed=.git/HEAD");

    let sha = std::env::var("BASE_BUILD_SHA")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            Command::new("git")
                .args(["rev-parse", "--short=12", "HEAD"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=BASE_BUILD_SHA_RESOLVED={sha}");
}
