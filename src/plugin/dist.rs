//! Cross-platform plugin distribution.
//!
//! `base ext add <manifest>` reads the manifest's optional `[dist]` block, detects
//! the host OS/arch, fetches the matching prebuilt binary from the plugin's GitHub
//! release, verifies its sha256, unpacks it into `~/.base-gbl/plugins/<name>/`, and
//! repoints the manifest there — the same end-state as `--bundle`, but the binary
//! comes from a release asset instead of a local build. No operator toolchain.
//!
//! Strictly additive: this is a new code path. `base ext install [--bundle]` (local)
//! is untouched; a manifest with no `[dist]` block can't be `add`ed (clear error).
//!
//! Asset scheme (reused from base's own release matrix, verbatim):
//!   linux   → `<binary>-linux-x86_64.tar.gz`
//!   darwin  → `<binary>-darwin-aarch64.tar.gz`
//!   windows → `<binary>-windows-x86_64.zip`
//!
//! Private-repo releases are supported: a GitHub token (`GITHUB_TOKEN` env →
//! `~/.base-gbl/.env` → `gh auth token`) routes the download through the API asset
//! endpoint. Public repos work token-free via the browser download URL.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

use crate::extension::{self, DistSpec, ExtensionDef};

/// Outcome of a `base ext add` (fetch) install, for CLI reporting.
pub struct DistOutcome {
    pub name: String,
    pub asset: String,
    pub version: String,
    pub dest: PathBuf,
    pub manifest: PathBuf,
    /// true if no host asset existed and we built from local source instead.
    pub source_fallback: bool,
}

/// Map the running host to its release-asset (os, arch, ext) triple, or None if
/// this host/arch combination isn't one base's release matrix builds.
fn host_asset_parts() -> Option<(&'static str, &'static str, &'static str)> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "darwin",
        "windows" => "windows",
        _ => return None,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        _ => return None,
    };
    let ext = if os == "windows" { "zip" } else { "tar.gz" };
    Some((os, arch, ext))
}

/// `<binary>-<os>-<arch>.<ext>` — the release asset name for a given host.
fn asset_name(binary: &str, os: &str, arch: &str, ext: &str) -> String {
    format!("{binary}-{os}-{arch}.{ext}")
}

// ─── GitHub token resolution (private-repo support) ──────────────

fn token_from_env_file() -> Option<String> {
    let path = super::global_env_path()?;
    let content = std::fs::read_to_string(path).ok()?;
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        if let Some((k, v)) = line.split_once('=')
            && (k.trim() == "GITHUB_TOKEN" || k.trim() == "GH_TOKEN")
        {
            let mut val = v.trim();
            if val.len() >= 2
                && ((val.starts_with('"') && val.ends_with('"')) || (val.starts_with('\'') && val.ends_with('\'')))
            {
                val = &val[1..val.len() - 1];
            }
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

fn token_from_gh_cli() -> Option<String> {
    let out = Command::new("gh").args(["auth", "token"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let tok = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if tok.is_empty() {
        None
    } else {
        Some(tok)
    }
}

/// GITHUB_TOKEN / GH_TOKEN env → ~/.base-gbl/.env → `gh auth token`. None ⇒ try
/// the unauthenticated public-download path.
fn github_token() -> Option<String> {
    for var in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(v) = std::env::var(var)
            && !v.is_empty()
        {
            return Some(v);
        }
    }
    token_from_env_file().or_else(token_from_gh_cli)
}

// ─── Download (auth-aware, redirect-safe) ────────────────────────

fn read_body(resp: ureq::Response) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    resp.into_reader().read_to_end(&mut buf).context("reading response body")?;
    Ok(buf)
}

/// Download one release asset's bytes. Returns Ok(None) when the asset doesn't
/// exist (404) so the caller can fall back; other failures are hard errors.
fn download_asset(repo: &str, tag: &str, asset: &str, token: Option<&str>) -> Result<Option<Vec<u8>>> {
    match token {
        // Authenticated (works for private + public): resolve the asset via the API,
        // then GET the asset's API url with octet-stream. GitHub 302s to a signed S3
        // URL — fetch that WITHOUT the auth header (S3 rejects it).
        Some(tok) => {
            let rel_url = format!("https://api.github.com/repos/{repo}/releases/tags/{tag}");
            let rel: serde_json::Value = match ureq::get(&rel_url)
                .set("Authorization", &format!("Bearer {tok}"))
                .set("Accept", "application/vnd.github+json")
                .set("User-Agent", "base-cli")
                .set("X-GitHub-Api-Version", "2022-11-28")
                .call()
            {
                Ok(r) => r.into_json().context("parsing release JSON")?,
                Err(ureq::Error::Status(404, _)) => bail!("release {tag} not found in {repo}"),
                Err(e) => return Err(anyhow!("GitHub API error fetching {tag}: {e}")),
            };
            let asset_url = rel["assets"]
                .as_array()
                .and_then(|a| a.iter().find(|x| x["name"].as_str() == Some(asset)))
                .and_then(|x| x["url"].as_str());
            let Some(asset_url) = asset_url else {
                return Ok(None); // asset missing for this host → fallback
            };
            // redirects(0): don't follow GitHub's 302 to S3 automatically — the GH auth
            // header must NOT be forwarded to the signed S3 URL (ureq sets policy on the Agent).
            let no_redirect = ureq::builder().redirects(0).build();
            let resp = no_redirect
                .get(asset_url)
                .set("Authorization", &format!("Bearer {tok}"))
                .set("Accept", "application/octet-stream")
                .set("User-Agent", "base-cli")
                .call();
            match resp {
                // GitHub streams the bytes directly (no redirect).
                Ok(r) if r.status() == 200 => Ok(Some(read_body(r)?)),
                // 302 → signed S3 URL; re-fetch without the GitHub auth header.
                Ok(r) if r.status() / 100 == 3 => {
                    let loc = r
                        .header("location")
                        .ok_or_else(|| anyhow!("redirect without Location for asset {asset}"))?
                        .to_string();
                    let s3 = ureq::get(&loc).set("User-Agent", "base-cli").call().context("downloading asset from storage")?;
                    Ok(Some(read_body(s3)?))
                }
                Ok(r) => bail!("unexpected status {} downloading {asset}", r.status()),
                Err(ureq::Error::Status(404, _)) => Ok(None),
                Err(e) => Err(anyhow!("error downloading {asset}: {e}")),
            }
        }
        // Unauthenticated public download.
        None => {
            let url = format!("https://github.com/{repo}/releases/download/{tag}/{asset}");
            match ureq::get(&url).set("User-Agent", "base-cli").call() {
                Ok(r) => Ok(Some(read_body(r)?)),
                Err(ureq::Error::Status(404, _)) => Ok(None),
                Err(e) => Err(anyhow!(
                    "error downloading {asset} (no GitHub token found — private repos need GITHUB_TOKEN / `gh auth login`): {e}"
                )),
            }
        }
    }
}

// ─── Checksum ────────────────────────────────────────────────────

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Verify `bytes` against a `shasum -a 256` sidecar (`<hex>  <filename>`).
fn verify_sha256(bytes: &[u8], sidecar: &[u8], asset: &str) -> Result<()> {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    let got = hex(&h.finalize());
    let text = String::from_utf8_lossy(sidecar);
    let want = text
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow!("empty .sha256 sidecar for {asset}"))?
        .to_lowercase();
    if got != want {
        bail!("checksum mismatch for {asset}\n  expected {want}\n  got      {got}\n  (supply-chain guard — refusing to install)");
    }
    Ok(())
}

// ─── Unpack ──────────────────────────────────────────────────────

/// Extract a release archive (`.tar.gz` or `.zip`) into `dest`. Shells out to
/// `tar`, which is present on every modern target (GNU tar autodetects gzip;
/// bsdtar on macOS/Windows handles both tar.gz AND zip).
fn extract(bytes: &[u8], asset: &str, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    let tmp = dest.join(format!(".dl-{}-{}", std::process::id(), asset));
    std::fs::write(&tmp, bytes).with_context(|| format!("writing temp archive {}", tmp.display()))?;
    let status = Command::new("tar")
        .arg("-xf")
        .arg(&tmp)
        .arg("-C")
        .arg(dest)
        .status();
    let _ = std::fs::remove_file(&tmp);
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => bail!("tar failed to extract {asset} (exit {})", s.code().unwrap_or(-1)),
        Err(e) => bail!("could not run `tar` to extract {asset}: {e} (tar is required to unpack release assets)"),
    }
}

// ─── Install ─────────────────────────────────────────────────────

/// `base ext add <manifest>` — fetch + verify + unpack + repoint.
pub fn dist_install(manifest_path: &Path) -> Result<DistOutcome> {
    let ext = extension::validate_extension(manifest_path).map_err(|v| anyhow!("invalid manifest: {}", v.join("; ")))?;
    let dist = ext
        .dist
        .clone()
        .ok_or_else(|| anyhow!("manifest has no [dist] block — `base ext add` needs one (use `base ext install --bundle` for a local build, or add [dist] with repo/version/binary)"))?;

    let home = crate::home::home_root().ok_or_else(|| anyhow!("cannot determine home directory"))?;
    let dest = home.join(".base-gbl").join("plugins").join(&ext.name);
    let tag = format!("v{}", dist.version);

    let Some((os, arch, ext_kind)) = host_asset_parts() else {
        return source_fallback(manifest_path, &ext, &dist, "unsupported host OS/arch for prebuilt assets");
    };
    let asset = asset_name(&dist.binary, os, arch, ext_kind);
    let token = github_token();

    let bytes = download_asset(&dist.repo, &tag, &asset, token.as_deref())?;
    let Some(bytes) = bytes else {
        return source_fallback(
            manifest_path,
            &ext,
            &dist,
            &format!("no prebuilt asset '{asset}' in {}@{tag}", dist.repo),
        );
    };
    // Checksum is mandatory when the sidecar exists; absence is tolerated (warn).
    match download_asset(&dist.repo, &tag, &format!("{asset}.sha256"), token.as_deref())? {
        Some(sidecar) => verify_sha256(&bytes, &sidecar, &asset)?,
        None => eprintln!("base: warning — no {asset}.sha256 in the release; installing unverified"),
    }

    if dest.exists() {
        std::fs::remove_dir_all(&dest)?; // re-add = clean replace
    }
    extract(&bytes, &asset, &dest)?;

    // chmod +x the handler binary on unix (the asset packs it at bin/<binary>).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let bin = dest.join("bin").join(&dist.binary);
        if bin.exists() {
            let _ = std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755));
        }
    }

    let manifest = repoint_installed_manifest(manifest_path, &ext.name, &dest, &home)?;
    Ok(DistOutcome {
        name: ext.name,
        asset,
        version: dist.version,
        dest,
        manifest,
        source_fallback: false,
    })
}

/// Prefer the manifest shipped INSIDE the asset (authoritative); fall back to the
/// passed-in manifest. Repoint its framework_dir → the unpacked dir, install to
/// `~/.base-gbl/extensions/<name>.toml`.
fn repoint_installed_manifest(passed: &Path, name: &str, dest: &Path, home: &Path) -> Result<PathBuf> {
    let packed = dest.join("base-extension.toml");
    let src_manifest = if packed.exists() { packed.as_path() } else { passed };
    let original = std::fs::read_to_string(src_manifest)?;
    let rewritten = super::rewrite_framework_dir(&original, &dest.to_string_lossy())?;
    let ext_dir = home.join(".base-gbl").join("extensions");
    std::fs::create_dir_all(&ext_dir)?;
    let manifest = ext_dir.join(format!("{name}.toml"));
    std::fs::write(&manifest, rewritten)?;
    Ok(manifest)
}

/// Graceful fallback when no host asset exists: if the manifest's framework_dir is
/// a local source dir with a `prepare.sh`, build it then `--bundle` install. Else a
/// clear error naming the missing target.
fn source_fallback(manifest_path: &Path, ext: &ExtensionDef, dist: &DistSpec, why: &str) -> Result<DistOutcome> {
    let local = ext
        .framework_dir
        .as_deref()
        .map(|fw| super::resolve_framework_dir(manifest_path, fw))
        .filter(|p| p.join("prepare.sh").is_file());
    let Some(src) = local else {
        bail!(
            "{why}. No local source to build from either.\n  Add a prebuilt asset for this host to {}@v{}, or install from a checked-out repo with prepare.sh.",
            dist.repo,
            dist.version
        );
    };
    eprintln!("base: {why} — building from local source ({}) …", src.display());
    let status = Command::new("bash").arg("prepare.sh").current_dir(&src).status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => bail!("source build failed: prepare.sh exited {}", s.code().unwrap_or(-1)),
        Err(e) => bail!("source build failed: could not run prepare.sh ({e})"),
    }
    let outcome = super::bundle_install(manifest_path)?;
    Ok(DistOutcome {
        name: outcome.name,
        asset: "(built from source)".to_string(),
        version: dist.version.clone(),
        dest: outcome.dest,
        manifest: outcome.manifest,
        source_fallback: true,
    })
}

/// Human-readable report.
pub fn format_dist_human(o: &DistOutcome) -> String {
    if o.source_fallback {
        format!(
            "✓ Added '{}' (built from source — no host asset)\n  → {}\n  manifest → {}\n  Now repo-independent.",
            o.name,
            o.dest.display(),
            o.manifest.display()
        )
    } else {
        format!(
            "✓ Added '{}' v{} (fetched {})\n  → {}\n  manifest → {}\n  Now repo-independent; cross-platform install via base.",
            o.name,
            o.version,
            o.asset,
            o.dest.display(),
            o.manifest.display()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_name_follows_scheme() {
        assert_eq!(asset_name("meta", "linux", "x86_64", "tar.gz"), "meta-linux-x86_64.tar.gz");
        assert_eq!(asset_name("highlevel", "darwin", "aarch64", "tar.gz"), "highlevel-darwin-aarch64.tar.gz");
        assert_eq!(asset_name("nano-banana", "windows", "x86_64", "zip"), "nano-banana-windows-x86_64.zip");
    }

    #[test]
    fn host_parts_present_for_this_host() {
        // CI/dev hosts are always one of the supported triples.
        let parts = host_asset_parts();
        assert!(parts.is_some(), "expected a known host triple");
        let (_os, _arch, ext) = parts.unwrap();
        assert!(ext == "tar.gz" || ext == "zip");
    }

    #[test]
    fn verify_sha256_matches_and_rejects() {
        let data = b"hello highlevel";
        // sha256("hello highlevel")
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(data);
        let good = super::hex(&h.finalize());
        let sidecar = format!("{good}  meta-linux-x86_64.tar.gz\n");
        assert!(verify_sha256(data, sidecar.as_bytes(), "asset").is_ok());
        let bad = "0000000000000000000000000000000000000000000000000000000000000000  asset\n";
        assert!(verify_sha256(data, bad.as_bytes(), "asset").is_err());
    }
}
