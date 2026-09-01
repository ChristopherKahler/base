//! Manifest-facing path handling — one shape on every platform.
//!
//! Handler and `framework_dir` paths come out of a hand-written TOML manifest,
//! and they are compared, logged, and written back into TOML. `Path::join` is
//! the wrong tool for them: it emits `\` on Windows, so one manifest resolves to
//! two different strings depending on the host. `Path::is_absolute` is wrong for
//! them too — on Windows it is false for a POSIX-rooted `/opt/x` (no drive
//! prefix), so a manifest that plainly means "from the root" gets glued onto the
//! manifest's own directory instead.
//!
//! These three helpers fix both. They are string-level and platform-independent
//! by construction: each is a no-op on a path that is already `/`-shaped, which
//! is why the tests below can feed them Windows-shaped input and prove the
//! Windows behaviour from a Linux run.
//!
//! Windows accepts `/` as a separator in every path except a verbatim (`\\?\`)
//! one — which is why `to_manifest_path` strips that prefix instead of carrying
//! it into a manifest. See `resolve_framework_dir`, whose `canonicalize` is
//! where verbatim paths enter.

use std::path::{Path, PathBuf};

/// True when a manifest-declared path is absolute *as the manifest means it*.
///
/// `Path::is_absolute` alone is not enough: on Windows `/opt/x` is rooted but
/// not absolute (no `C:` prefix), and a leading `/` in a manifest always means
/// "from the root". On unix `has_root()` and `is_absolute()` are the same
/// predicate, so this is exactly `is_absolute()` there.
pub(super) fn is_manifest_absolute(p: &Path) -> bool {
    p.is_absolute() || p.has_root()
}

/// Normalise a path into manifest shape: no Windows verbatim prefix, `/`
/// throughout. Identity on a path that is already `/`-shaped.
pub fn to_manifest_path(p: &Path) -> PathBuf {
    PathBuf::from(slashed(p))
}

/// Join a manifest-declared relative path onto a base directory with `/`.
///
/// An empty base yields `rel` unchanged — `Path::parent()` of a bare filename is
/// `""`, and that is what `Path::join` did there before.
pub(super) fn join_manifest(base: &Path, rel: &Path) -> PathBuf {
    let base = slashed(base);
    let rel = slashed(rel);
    if base.is_empty() {
        return PathBuf::from(rel);
    }
    if base.ends_with('/') {
        PathBuf::from(format!("{base}{rel}"))
    } else {
        PathBuf::from(format!("{base}/{rel}"))
    }
}

/// The shared transform: drop a verbatim prefix, then `\` → `/`.
fn slashed(p: &Path) -> String {
    let s = p.to_string_lossy();
    // `\\?\UNC\srv\share` is the verbatim spelling of `\\srv\share`.
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return format!("//{}", rest.replace('\\', "/"));
    }
    s.strip_prefix(r"\\?\").unwrap_or(s.as_ref()).replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every case here feeds Windows-shaped input to whatever host runs the
    // suite. The helpers are string-level, so a Linux run proves the Windows
    // shape — which is the whole point of them being string-level.

    #[test]
    fn join_uses_forward_slash_for_a_backslash_base() {
        assert_eq!(
            join_manifest(Path::new(r"C:\opt\fw"), Path::new("bin/nb.mjs")),
            PathBuf::from("C:/opt/fw/bin/nb.mjs")
        );
    }

    #[test]
    fn join_normalises_a_backslash_relative() {
        assert_eq!(
            join_manifest(Path::new("/opt/fw"), Path::new(r"bin\nb.mjs")),
            PathBuf::from("/opt/fw/bin/nb.mjs")
        );
    }

    #[test]
    fn join_with_empty_base_returns_rel() {
        assert_eq!(join_manifest(Path::new(""), Path::new("bin/c")), PathBuf::from("bin/c"));
    }

    #[test]
    fn join_with_root_base_keeps_the_root() {
        assert_eq!(join_manifest(Path::new("/"), Path::new("bin/c")), PathBuf::from("/bin/c"));
    }

    #[test]
    fn join_does_not_double_a_trailing_separator() {
        assert_eq!(
            join_manifest(Path::new("/opt/fw/"), Path::new("bin/c")),
            PathBuf::from("/opt/fw/bin/c")
        );
        assert_eq!(
            join_manifest(Path::new(r"C:\opt\fw\"), Path::new("bin/c")),
            PathBuf::from("C:/opt/fw/bin/c")
        );
    }

    #[test]
    fn verbatim_prefix_is_stripped() {
        assert_eq!(to_manifest_path(Path::new(r"\\?\C:\x\y")), PathBuf::from("C:/x/y"));
    }

    #[test]
    fn verbatim_unc_becomes_a_plain_unc() {
        assert_eq!(
            to_manifest_path(Path::new(r"\\?\UNC\srv\share\p")),
            PathBuf::from("//srv/share/p")
        );
    }

    #[test]
    fn already_slashed_paths_are_untouched() {
        for p in ["/opt/fw/bin/nb.mjs", "bin/nb.mjs", "./bin/nb.mjs", ""] {
            assert_eq!(to_manifest_path(Path::new(p)), PathBuf::from(p), "{p} must round-trip");
        }
    }

    #[test]
    fn posix_rooted_paths_count_as_absolute() {
        assert!(is_manifest_absolute(Path::new("/opt/x")));
        assert!(!is_manifest_absolute(Path::new("bin/x")));
        assert!(!is_manifest_absolute(Path::new("")));
        assert!(!is_manifest_absolute(Path::new(".")));
    }
}
