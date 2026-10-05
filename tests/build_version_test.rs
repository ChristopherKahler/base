//! `base --version` must carry the build's provenance, not just the package
//! version.
//!
//! On 2026-09-20 two binaries built from different branches both printed
//! `base 0.15.2`. One install silently reverted the other's fix. Nothing on the
//! machine could tell them apart, so the revert went unnoticed until a surface
//! changed back. A version string without a build id cannot answer the only
//! question an operator asks of it.

#[test]
fn version_leads_with_the_package_version_and_carries_a_build_id() {
    let v = base::BUILD_VERSION;

    assert!(
        v.starts_with(env!("CARGO_PKG_VERSION")),
        "--version must lead with the package version so the updater's          comparison stays readable by a human; got `{v}`"
    );
    assert!(
        v.contains("(build "),
        "--version must carry a build id; got `{v}`"
    );
    assert!(
        v.len() > env!("CARGO_PKG_VERSION").len(),
        "the build id is missing entirely; got `{v}`"
    );
}

/// The build id may legitimately be "unknown" -- a tree copied without `.git`
/// cannot know its own commit, and admitting that is the point. What must never
/// happen is the suffix being absent, because then the two cases are once again
/// indistinguishable.
#[test]
fn an_unprovenanced_build_still_says_so_rather_than_going_silent() {
    let v = base::BUILD_VERSION;
    let id = v
        .split("(build ")
        .nth(1)
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or_else(|| panic!("no build id segment in `{v}`"));
    assert!(!id.is_empty(), "build id segment is empty in `{v}`");
}
