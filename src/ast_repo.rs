//! Repo wiring for the "gitignore + regenerate on checkout" model: an app's
//! generated AST map is never committed, and every clone / branch-switch / pull
//! regenerates it locally — so every dev shares an up-to-date map without repo
//! churn or merge conflicts.

use std::path::Path;

/// Idempotently ensure an app's `.base-ast/` map self-ignores in git and, for a
/// real git repo, that checkout/merge regenerate it. Safe to call on every
/// foreground sync; never clobbers a user's existing git hooks.
pub fn ensure_repo_wiring(app_root: &Path) {
    // 1. Self-contained gitignore — keep the generated map out of the repo in
    //    any app, without touching the app's root .gitignore.
    let gi = app_root.join(".base-ast").join(".gitignore");
    if !gi.exists() {
        let _ = std::fs::write(
            &gi,
            "# Generated AST map — regenerated on checkout, never committed.\n*\n",
        );
    }

    // 2. Regenerate-on-checkout hooks (real git repos only).
    if app_root.join(".git").is_dir() {
        install_hook(app_root, "post-checkout", true);
        install_hook(app_root, "post-merge", false);
    }
}

/// Write a git hook that backgrounds an AST refresh. `branch_switch_only` guards
/// post-checkout so file-level checkouts don't trigger a reparse. Skips silently
/// if a non-base hook already exists (never clobbers).
fn install_hook(app_root: &Path, name: &str, branch_switch_only: bool) {
    const MARKER: &str = "# base-ast-regen";
    let hooks_dir = app_root.join(".git").join("hooks");
    if std::fs::create_dir_all(&hooks_dir).is_err() {
        return;
    }
    let path = hooks_dir.join(name);
    if let Ok(existing) = std::fs::read_to_string(&path)
        && !existing.contains(MARKER)
    {
        return; // respect a pre-existing user hook
    }
    // post-checkout args: <old-ref> <new-ref> <branch-flag>; 1 == branch switch.
    let guard = if branch_switch_only {
        "[ \"$3\" = \"1\" ] || exit 0\n"
    } else {
        ""
    };
    // Background + skip registration: a teammate just wants a fresh local map,
    // not to write into a workspace graph they may not have.
    let body = format!(
        "#!/bin/sh\n{MARKER}\n{guard}BASE_AST_SKIP_REGISTER=1 base sync --ast --yes --target . >/dev/null 2>&1 &\n"
    );
    if std::fs::write(&path, &body).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
        }
    }
}
