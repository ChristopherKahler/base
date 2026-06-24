//! Workspace scope resolution (v0.8 Workspace Scoping, Phase 45).
//!
//! Pure, filesystem-free primitives the milestone is built on:
//! - [`current_workspace`] — which registered workspace the CWD falls under.
//! - [`home`] — which workspace a project belongs to, derived from its `#path`.
//!
//! Both derive by **longest-prefix, component-wise** match against the `[[workspace]]`
//! registry — NOT the named-graph stamp — so scoping is correct even before any
//! `reslot` migration of the existing (CWD-stamped) data.
//!
//! The core is intentionally filesystem-free (no `canonicalize`, no `exists`) so it is
//! trivially unit-testable with literal paths. A caller-side wrapper canonicalizes real
//! paths before calling; that wiring lands in Phase 46 when these meet real reads. Until
//! then this module has zero non-test callers by design (a dormant primitive).

use std::path::Path;

use crate::config::WorkspaceEntry;
use crate::crud::slugify;

/// A project's home workspace, derived from its `#path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Home {
    /// Lives under a registered workspace; carries that workspace's slug name.
    Workspace(String),
    /// No `#path`, or a `#path` under no registered workspace — never silently dropped.
    Unscoped,
}

/// Canonical workspace name = `slugify` of the workspace root's final component.
///
/// Mirrors `crud::workspace_slug` so that `home`-derived names line up exactly with the
/// `graph/ws/{name}` named-graph scheme Phase 47 routes writes to. `ws_path` is expected
/// already trimmed of any trailing slash (see [`normalized`]).
fn registry_name(ws_path: &Path) -> String {
    ws_path
        .file_name()
        .and_then(|n| n.to_str())
        .map(slugify)
        .unwrap_or_else(|| "default".into())
}

/// Strip a single trailing slash so trailing-slash variants compare identically.
/// (`Path` already normalizes most of this, but we trim explicitly for determinism.)
fn normalized(raw: &str) -> &Path {
    Path::new(raw.trim_end_matches('/'))
}

/// The longest (most-components) registry entry whose path is a component-wise prefix of
/// `target`. Component-wise (`Path::starts_with`) is what stops `/x/extendly-archive` from
/// matching `/x/extendly`. `None` when no entry is a prefix.
fn longest_prefix<'a>(target: &Path, registry: &'a [WorkspaceEntry]) -> Option<&'a WorkspaceEntry> {
    registry
        .iter()
        .filter(|e| target.starts_with(normalized(&e.path)))
        .max_by_key(|e| normalized(&e.path).components().count())
}

/// Resolve which registered workspace `cwd` falls under (longest-prefix match), returning
/// its slug name. `None` when `cwd` is under no registered workspace — the caller then
/// falls back to global (today's flat-union behavior).
pub fn current_workspace(cwd: &Path, registry: &[WorkspaceEntry]) -> Option<String> {
    longest_prefix(cwd, registry).map(|e| registry_name(normalized(&e.path)))
}

/// Resolve a project's home workspace from its stored `#path` (longest-prefix match).
/// Returns [`Home::Unscoped`] when the path is absent/empty or under no registered
/// workspace — never a wrong guess, never a silent drop.
pub fn home(project_path: Option<&str>, registry: &[WorkspaceEntry]) -> Home {
    let path = match project_path {
        Some(p) if !p.trim().is_empty() => p,
        _ => return Home::Unscoped,
    };
    match longest_prefix(Path::new(path), registry) {
        Some(e) => Home::Workspace(registry_name(normalized(&e.path))),
        None => Home::Unscoped,
    }
}

/// The view a read-time command wants over the project working set (Phase 46).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectScope {
    /// Default — only projects homed in the current workspace (global fallback when current is None).
    Current,
    /// No filter — today's flat union of every registered workspace.
    All,
    /// Only projects homed in the named workspace (caller passes an already-slugified name).
    Workspace(String),
    /// Only projects with no `#path` / no registry match.
    Unscoped,
}

/// Pure membership test: does a project with the given `home` belong in `scope`, viewed from
/// `current` (the current workspace, or `None` when the CWD is under no registered workspace)?
///
/// `Current` + `None` falls back to "everything" (the backward-compatible global view) so a
/// session run outside any registered workspace never hides work. Phase 49 will extend the
/// `Current`/`Workspace` arms to also accept a `peerWorkspace` match.
pub fn in_scope(home: &Home, peers: &[String], current: Option<&str>, scope: &ProjectScope) -> bool {
    // A project belongs to workspace `ws` if it's homed there OR lists `ws` as a peer (Phase 49).
    let here = |ws: &str| *home == Home::Workspace(ws.to_string()) || peers.iter().any(|pw| pw == ws);
    match scope {
        ProjectScope::All => true,
        ProjectScope::Unscoped => *home == Home::Unscoped,
        ProjectScope::Workspace(w) => here(w),
        ProjectScope::Current => match current {
            Some(c) => here(c),
            None => true, // global fallback — never hide work outside a registered workspace
        },
    }
}

// ── FS-aware wrappers ───────────────────────────────────────────────────────
// The matching core above is filesystem-free and unit-tested with literal paths.
// These wrappers canonicalize real paths before matching (symlinks, /home vs /mnt,
// macOS /private, …); they fall back to the raw string when a path can't be resolved
// (a registered workspace or moved project dir that no longer exists).

/// Canonicalize a path string; raw-string fallback when it can't be resolved.
pub fn canonical_str(p: &str) -> String {
    std::fs::canonicalize(p)
        .map(|pb| pb.to_string_lossy().into_owned())
        .unwrap_or_else(|_| p.to_string())
}

/// Canonicalize every entry path in a `[[workspace]]` registry once, for prefix matching.
pub fn canonical_registry(reg: &[WorkspaceEntry]) -> Vec<WorkspaceEntry> {
    reg.iter()
        .map(|e| WorkspaceEntry {
            path: canonical_str(&e.path),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg(paths: &[&str]) -> Vec<WorkspaceEntry> {
        paths
            .iter()
            .map(|p| WorkspaceEntry { path: (*p).into() })
            .collect()
    }

    // AC-1: current_workspace resolves the CWD to its registered workspace.
    #[test]
    fn current_workspace_resolves_cwd_to_registered_workspace() {
        let r = reg(&["/home/c/chris-ai-systems", "/home/c/ops-sys/extendly"]);
        assert_eq!(
            current_workspace(Path::new("/home/c/chris-ai-systems/apps/base-v2"), &r),
            Some("chris-ai-systems".to_string())
        );
    }

    // AC-2: longest-prefix tie-break — the most specific (nested) workspace wins,
    // regardless of registry order.
    #[test]
    fn longest_prefix_tie_break_most_specific_wins() {
        let r = reg(&["/home/c/ops-sys", "/home/c/ops-sys/extendly"]);
        assert_eq!(
            current_workspace(Path::new("/home/c/ops-sys/extendly/rev-ops"), &r),
            Some("extendly".to_string())
        );
        // Reverse order — result must not depend on registry ordering.
        let r2 = reg(&["/home/c/ops-sys/extendly", "/home/c/ops-sys"]);
        assert_eq!(
            current_workspace(Path::new("/home/c/ops-sys/extendly/rev-ops"), &r2),
            Some("extendly".to_string())
        );
    }

    // AC-3: component-boundary matching — a string-prefix sibling is NOT a match.
    #[test]
    fn component_boundary_no_false_sibling_prefix() {
        let r = reg(&["/home/c/ops-sys/extendly"]);
        assert_eq!(
            home(Some("/home/c/ops-sys/extendly-archive/proj"), &r),
            Home::Unscoped
        );
    }

    // AC-4: trailing slash on a registry path is irrelevant to the result.
    #[test]
    fn trailing_slash_normalization_is_irrelevant() {
        let with_slash = reg(&["/home/c/chris-ai-systems/"]);
        let without = reg(&["/home/c/chris-ai-systems"]);
        let target = Some("/home/c/chris-ai-systems/apps/base-v2");
        assert_eq!(
            home(target, &with_slash),
            Home::Workspace("chris-ai-systems".to_string())
        );
        assert_eq!(home(target, &with_slash), home(target, &without));
    }

    // AC-5: no match → None (current) / Unscoped (home); never a wrong guess.
    #[test]
    fn no_match_returns_none_and_unscoped() {
        let r = reg(&["/home/c/chris-ai-systems"]);
        assert_eq!(current_workspace(Path::new("/tmp/somewhere"), &r), None);
        assert_eq!(home(Some("/tmp/proj"), &r), Home::Unscoped);
        assert_eq!(home(None, &r), Home::Unscoped); // project with no #path
        assert_eq!(home(Some(""), &r), Home::Unscoped); // empty #path
    }

    // ── Phase 46: ProjectScope / in_scope ──────────────────────────────────

    fn ws(name: &str) -> Home {
        Home::Workspace(name.to_string())
    }

    // AC-1: Current scope keeps only the current workspace's projects.
    #[test]
    fn in_scope_current_keeps_only_current_workspace() {
        assert!(in_scope(&ws("chris-ai-systems"), &[], Some("chris-ai-systems"), &ProjectScope::Current));
        assert!(!in_scope(&ws("extendly"), &[], Some("chris-ai-systems"), &ProjectScope::Current));
        assert!(!in_scope(&Home::Unscoped, &[], Some("chris-ai-systems"), &ProjectScope::Current));
    }

    // AC-5: Current + None (CWD under no registered workspace) → global fallback (keep all).
    #[test]
    fn in_scope_current_with_no_workspace_falls_back_to_global() {
        assert!(in_scope(&ws("extendly"), &[], None, &ProjectScope::Current));
        assert!(in_scope(&Home::Unscoped, &[], None, &ProjectScope::Current));
    }

    // AC-2: All keeps everything regardless of home or current.
    #[test]
    fn in_scope_all_keeps_everything() {
        assert!(in_scope(&ws("extendly"), &[], Some("chris-ai-systems"), &ProjectScope::All));
        assert!(in_scope(&Home::Unscoped, &[], None, &ProjectScope::All));
    }

    // AC-3: Workspace(w) keeps only that workspace.
    #[test]
    fn in_scope_workspace_targets_named() {
        let s = ProjectScope::Workspace("extendly".to_string());
        assert!(in_scope(&ws("extendly"), &[], Some("chris-ai-systems"), &s));
        assert!(!in_scope(&ws("chris-ai-systems"), &[], Some("chris-ai-systems"), &s));
    }

    // AC-4 + AC-6: Unscoped keeps only un-homed; an un-homed project is hidden under Current
    // but visible under All and Unscoped (no silent loss).
    #[test]
    fn in_scope_unscoped_surfaces_only_unhomed() {
        assert!(in_scope(&Home::Unscoped, &[], Some("chris-ai-systems"), &ProjectScope::Unscoped));
        assert!(!in_scope(&ws("chris-ai-systems"), &[], Some("chris-ai-systems"), &ProjectScope::Unscoped));
        // AC-6: the un-homed project — hidden by Current, surfaced by All + Unscoped.
        let unhomed = Home::Unscoped;
        assert!(!in_scope(&unhomed, &[], Some("chris-ai-systems"), &ProjectScope::Current));
        assert!(in_scope(&unhomed, &[], Some("chris-ai-systems"), &ProjectScope::All));
        assert!(in_scope(&unhomed, &[], Some("chris-ai-systems"), &ProjectScope::Unscoped));
    }

    // Phase 49: a peerWorkspace edge surfaces a home-A project in workspace B too.
    #[test]
    fn in_scope_peer_surfaces_in_named_peer() {
        let peers = vec!["b".to_string()];
        // From B (default Current, or explicit --workspace b): home a + peer b → shown.
        assert!(in_scope(&ws("a"), &peers, Some("b"), &ProjectScope::Current));
        assert!(in_scope(&ws("a"), &peers, Some("x"), &ProjectScope::Workspace("b".to_string())));
        // Still shown in its home a.
        assert!(in_scope(&ws("a"), &peers, Some("a"), &ProjectScope::Current));
        // A workspace it neither homes in nor peers to → hidden.
        assert!(!in_scope(&ws("a"), &peers, Some("c"), &ProjectScope::Current));
        // No peers → home-only.
        assert!(!in_scope(&ws("a"), &[], Some("b"), &ProjectScope::Current));
    }
}
