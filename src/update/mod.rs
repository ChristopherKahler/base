//! Self-update from public GitHub releases.
//!
//! This used to run through a license-gated channel at chrisai.cv: every check
//! POSTed `license_key`, `email`, `claude_max_email`, `machine_id`, `version`, and
//! an `activation_token`, and refused to install without a valid entitlement.
//!
//! That is gone. It sent personal data on a routine version check, it gated a tool
//! nobody is charging for, and — worst in practice — it pointed somewhere the
//! release pipeline never publishes to, so `base update` could not deliver the
//! releases actually being cut. GitHub releases are the single source of truth now.
//!
//! No license. No account. No identifiers leave the machine. The only request is an
//! anonymous GET of a public repo's latest release, plus the asset download.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// Public releases API — the same artifacts the release workflow publishes.
const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/ChristopherKahler/base/releases/latest";

/// GitHub rejects API requests without one.
const USER_AGENT: &str = "base-cli-updater";

/// Cap the download so a misbehaving endpoint can't fill the disk.
const MAX_PAYLOAD_BYTES: u64 = 256 * 1024 * 1024;
/// Payload is multi-MB; give it headroom.
const PAYLOAD_TIMEOUT_SECS: u64 = 180;
/// The metadata call is small; fail fast so a network hiccup doesn't hang a hook.
const API_TIMEOUT_SECS: u64 = 20;

/// Release asset for the current platform.
///
/// Names mirror the release workflow's matrix exactly. An unlisted platform is an
/// honest error rather than a silent no-op — the user needs to know no build exists
/// for their machine, not that "nothing happened".
pub fn asset_name() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("base-linux-x86_64.tar.gz"),
        ("macos", "aarch64") => Ok("base-darwin-aarch64.tar.gz"),
        ("macos", "x86_64") => Ok("base-darwin-x86_64.tar.gz"),
        ("windows", "x86_64") => Ok("base-windows-x86_64.zip"),
        (os, arch) => bail!("no base build is published for your platform ({os}-{arch})"),
    }
}

/// Parse `x.y.z` into comparable components (non-numeric/missing parts → 0).
fn semver_tuple(v: &str) -> (u64, u64, u64) {
    let mut it = v.trim().trim_start_matches('v').split('.').map(|p| {
        p.chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse::<u64>()
            .unwrap_or(0)
    });
    (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0))
}

/// True when `current` is at least `latest` (no update needed). Numeric, not string —
/// "0.10.10" is newer than "0.10.9", which a string compare gets backwards.
fn is_current(current: &str, latest: &str) -> bool {
    semver_tuple(current) >= semver_tuple(latest)
}

/// The latest release: its tag (leading `v` stripped) and the download URL for
/// this platform's asset.
fn fetch_latest_release() -> Result<(String, String)> {
    let resp = ureq::get(LATEST_RELEASE_API)
        .timeout(std::time::Duration::from_secs(API_TIMEOUT_SECS))
        .set("user-agent", USER_AGENT)
        .set("accept", "application/vnd.github+json")
        .call()
        .context("could not reach the GitHub releases API")?;

    let json: serde_json::Value = resp.into_json().context("malformed release JSON")?;

    let tag = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .context("release has no tag_name")?
        .trim_start_matches('v')
        .to_string();

    let wanted = asset_name()?;
    let url = json
        .get("assets")
        .and_then(|a| a.as_array())
        .and_then(|assets| {
            assets.iter().find(|a| {
                a.get("name").and_then(|n| n.as_str()) == Some(wanted)
            })
        })
        .and_then(|a| a.get("browser_download_url"))
        .and_then(|u| u.as_str())
        .with_context(|| format!("release v{tag} has no asset named '{wanted}'"))?
        .to_string();

    Ok((tag, url))
}

/// Stream a release asset to disk.
fn download_asset(url: &str, dir: &Path) -> Result<PathBuf> {
    let filename = asset_name()?;
    let dest = dir.join(filename);

    let resp = ureq::get(url)
        .timeout(std::time::Duration::from_secs(PAYLOAD_TIMEOUT_SECS))
        .set("user-agent", USER_AGENT)
        .call()
        .with_context(|| format!("downloading {url}"))?;

    // into_reader() yields a boxed trait object; Read::take needs a sized value.
    let mut reader = std::io::Read::take(resp.into_reader(), MAX_PAYLOAD_BYTES);
    let mut out = std::fs::File::create(&dest)
        .with_context(|| format!("creating {}", dest.display()))?;
    let n = std::io::copy(&mut reader, &mut out).context("streaming asset to disk")?;
    if n == 0 {
        bail!("downloaded asset was empty");
    }
    Ok(dest)
}

/// Extract an archive with system `tar` and locate the `base` executable inside
/// (direct child, or nested — release tarballs may wrap in a top-level dir).
///
/// `tar` handles both forms: `-xzf` for gzip, `-xf` for zip (bsdtar, shipped with
/// Windows 10+ and macOS).
fn extract_and_locate(archive: &Path, dest_dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dest_dir)?;
    let is_zip = archive
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"));

    let mut cmd = std::process::Command::new("tar");
    if is_zip {
        cmd.arg("-xf");
    } else {
        cmd.arg("-xzf");
    }
    let status = cmd
        .arg(archive)
        .arg("-C")
        .arg(dest_dir)
        .status()
        .context("running system tar")?;
    if !status.success() {
        bail!("extraction failed for {}", archive.display());
    }
    find_base_binary(dest_dir, 3)
        .ok_or_else(|| anyhow::anyhow!("extracted archive did not contain a `base` binary"))
}

/// Bounded recursive search for a file named `base` (or `base.exe`).
fn find_base_binary(dir: &Path, depth: u32) -> Option<PathBuf> {
    let target = if cfg!(windows) { "base.exe" } else { "base" };
    let direct = dir.join(target);
    if direct.is_file() {
        return Some(direct);
    }
    if depth == 0 {
        return None;
    }
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let p = entry.path();
        if p.is_dir()
            && let Some(found) = find_base_binary(&p, depth - 1)
        {
            return Some(found);
        }
    }
    None
}

/// `<binary> --version` → the version token (e.g. "base 0.9.0" → "0.9.0").
fn binary_version(bin: &Path) -> Option<String> {
    let out = std::process::Command::new(bin).arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .nth(1)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Canonical install location: `~/.local/bin/base` (`base.exe` on Windows).
fn install_dest() -> Result<PathBuf> {
    let name = if cfg!(windows) { "base.exe" } else { "base" };
    dirs::home_dir()
        .map(|h| h.join(".local").join("bin").join(name))
        .context("cannot resolve home directory")
}

/// Atomic swap: stage beside the target, set the exec bit, rename over it. rename
/// over a running binary succeeds (the live process keeps its inode); a plain copy
/// over it fails with ETXTBSY ("Text file busy").
fn atomic_swap(new_bin: &Path, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let staged = dest.with_file_name(".base.update.new");
    std::fs::copy(new_bin, &staged)
        .with_context(|| format!("staging new binary at {}", staged.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
            .context("setting exec bit on staged binary")?;
    }
    std::fs::rename(&staged, dest)
        .with_context(|| format!("atomic rename {} → {}", staged.display(), dest.display()))?;
    Ok(())
}

/// Marker for an in-flight background update. Without it, every session start
/// during a slow download would spawn another updater.
fn inflight_marker() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".base-gbl").join(".update-inflight"))
}

/// How long a marker is trusted before we assume the updater died mid-download.
const INFLIGHT_STALE_SECS: u64 = 600;

fn inflight_fresh(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|e| e.as_secs() < INFLIGHT_STALE_SECS)
}

/// Fire `base update` in a detached child and return immediately.
///
/// Session start must never wait on a multi-megabyte download, and the update
/// itself must never be the reason a session is slow to come up. The child
/// swaps the binary by atomic rename, so the session that spawned it keeps
/// running the old inode and the NEXT session starts on the new version.
///
/// Silent by contract: stdout and stderr are discarded. A failed update is not
/// something to interrupt someone's work with — the next session tries again.
pub fn spawn_background_update() {
    let Some(marker) = inflight_marker() else {
        return;
    };
    if marker.exists() && inflight_fresh(&marker) {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&marker, b"");

    // Spawned and never waited on: the hook process exits immediately, the
    // child is reparented and finishes the download on its own. All three
    // stdio handles are null so nothing it does can print into a session.
    let _ = std::process::Command::new(exe)
        .arg("update")
        .env("BASE_UPDATE_BACKGROUND", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Clear the in-flight marker. Called by the child when it finishes, so a
/// genuinely stuck updater is the only thing that ever waits out the TTL.
fn clear_inflight() {
    if let Some(m) = inflight_marker() {
        let _ = std::fs::remove_file(m);
    }
}

/// `base update` — check GitHub releases, pull this platform's binary, atomic-swap.
/// `check_only` stops after the version check; `force` installs even when current.
pub fn run(check_only: bool, force: bool) -> Result<()> {
    let background = std::env::var_os("BASE_UPDATE_BACKGROUND").is_some();
    let out = run_inner(check_only, force, background);
    if background {
        clear_inflight();
    }
    out
}

fn run_inner(check_only: bool, force: bool, quiet: bool) -> Result<()> {
    if quiet {
        return run_quiet(force);
    }
    run_verbose(check_only, force)
}

/// The background path: no output, no ceremony, just get current.
fn run_quiet(force: bool) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let (latest, url) = fetch_latest_release()?;
    if !force && is_current(current, &latest) {
        return Ok(());
    }
    let work = std::env::temp_dir().join(format!("base-update-{}", std::process::id()));
    std::fs::create_dir_all(&work)?;
    let outcome = (|| -> Result<()> {
        let archive = download_asset(&url, &work)?;
        let new_bin = extract_and_locate(&archive, &work.join("x"))?;
        let new_ver = binary_version(&new_bin).unwrap_or_default();
        if !force && !new_ver.is_empty() && is_current(current, &new_ver) {
            return Ok(());
        }
        atomic_swap(&new_bin, &install_dest()?)
    })();
    let _ = std::fs::remove_dir_all(&work);
    outcome
}

fn run_verbose(check_only: bool, force: bool) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    println!("base {current} — checking GitHub releases …");

    let (latest, url) = fetch_latest_release()?;

    if !force && is_current(current, &latest) {
        println!("✓ already up to date (base {current}).");
        return Ok(());
    }
    println!("→ update available: {current} → {latest}");
    if check_only {
        println!("(--check) not installing.");
        return Ok(());
    }

    let work = std::env::temp_dir().join(format!("base-update-{}", std::process::id()));
    std::fs::create_dir_all(&work).with_context(|| format!("creating {}", work.display()))?;
    let outcome = (|| -> Result<()> {
        let archive = download_asset(&url, &work)?;
        let new_bin = extract_and_locate(&archive, &work.join("x"))?;

        // Authoritative version guard against the extracted binary — the tag is
        // metadata, the binary is the thing being installed.
        let new_ver = binary_version(&new_bin).unwrap_or_default();
        if !force && !new_ver.is_empty() && is_current(current, &new_ver) {
            println!("✓ already running the latest base ({current}).");
            return Ok(());
        }

        let dest = install_dest()?;
        atomic_swap(&new_bin, &dest)?;
        let shown = if new_ver.is_empty() { "(new)".to_string() } else { new_ver };
        println!("✓ updated: {} is now base {shown}", dest.display());
        println!("  (only the binary was touched — shipped docs/hooks unchanged;");
        println!("   run `base install` if a release adds new ones.)");
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&work);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_compares_numerically_not_lexically() {
        // The bug a string compare would introduce: "0.10.9" > "0.10.10" as text.
        assert!(!is_current("0.10.9", "0.10.10"));
        assert!(is_current("0.10.10", "0.10.9"));
        assert!(is_current("1.0.0", "0.99.99"));
        assert!(is_current("0.10.10", "0.10.10"));
    }

    #[test]
    fn semver_tolerates_v_prefix_and_junk() {
        assert_eq!(semver_tuple("v0.10.10"), (0, 10, 10));
        assert_eq!(semver_tuple("0.10.10-rc1"), (0, 10, 10));
        assert_eq!(semver_tuple("garbage"), (0, 0, 0));
    }

    #[test]
    fn asset_name_matches_the_release_workflow() {
        // If this drifts from .github/workflows/release.yml, update finds nothing.
        match asset_name() {
            Ok(a) => assert!(
                [
                    "base-linux-x86_64.tar.gz",
                    "base-darwin-aarch64.tar.gz",
                    "base-darwin-x86_64.tar.gz",
                    "base-windows-x86_64.zip",
                ]
                .contains(&a),
                "unexpected asset name {a}"
            ),
            // Unsupported platforms must say so rather than pretend.
            Err(e) => assert!(e.to_string().contains("no base build is published")),
        }
    }

    #[test]
    fn install_dest_is_platform_correct() {
        let dest = install_dest().unwrap();
        let name = dest.file_name().unwrap().to_string_lossy().to_string();
        if cfg!(windows) {
            assert_eq!(name, "base.exe");
        } else {
            assert_eq!(name, "base");
        }
    }

    #[test]
    fn find_base_binary_locates_nested() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("base-0.1.0").join("bin");
        std::fs::create_dir_all(&nested).unwrap();
        let name = if cfg!(windows) { "base.exe" } else { "base" };
        std::fs::write(nested.join(name), b"stub").unwrap();
        assert!(find_base_binary(tmp.path(), 3).is_some());
    }

    #[test]
    fn find_base_binary_respects_depth() {
        let tmp = tempfile::tempdir().unwrap();
        let deep = tmp.path().join("a").join("b").join("c").join("d");
        std::fs::create_dir_all(&deep).unwrap();
        let name = if cfg!(windows) { "base.exe" } else { "base" };
        std::fs::write(deep.join(name), b"stub").unwrap();
        assert!(find_base_binary(tmp.path(), 2).is_none());
    }
}
