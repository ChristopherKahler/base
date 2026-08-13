//! License validation channel — the network seam for the Operator Kit gate.
//!
//! All HTTP lives behind [`UpdateChannel`] so the `base update` command and the
//! session-start hook are testable offline and can fail open. [`HttpChannel`]
//! talks to chrisai.cv (`POST /api/kit/validate`); [`MockChannel`] is a test
//! double. The wire contract mirrors the `chrisai` installer
//! (apps/chrisai-installer/lib/activate.js) field-for-field — snake_case
//! throughout, do not diverge.
//!
//! Two stages: validation gates the pull, and the artifact itself is gated too.
//! `POST /api/kit/validate` re-validates the license + refreshes the activation
//! token; `POST /api/kit/payload?component=base` then streams the per-platform
//! binary tarball (the activation token is the download auth — the base binary is
//! NOT on public npm; it ships from a private repo via the website). `run()` below
//! orchestrates validate → pull → version-check → atomic binary swap.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::license::LicenseFile;

const VALIDATE_PATH: &str = "/api/kit/validate";
const DEFAULT_API_BASE: &str = "https://chrisai.cv";
const HTTP_TIMEOUT_SECS: u64 = 5;

/// Where an unvalidated user purchases a key. Shown in the update-gate message;
/// checkout emails the license, then the buyer runs `base activate <key>`.
pub const CHECKOUT_URL: &str = "https://chrisai.cv/kit";

/// API base, honoring `CHRISAI_API_BASE` for sandbox/testing (matches installer).
fn api_base() -> String {
    std::env::var("CHRISAI_API_BASE").unwrap_or_else(|_| DEFAULT_API_BASE.to_string())
}

/// `POST /api/kit/validate` request body. snake_case mirrors the installer.
#[derive(Debug, Clone, Serialize)]
pub struct ValidateRequest {
    pub license_key: String,
    /// the purchase email (server matches against `Purchase.email`)
    pub email: String,
    /// the confirmed Claude Max account email
    pub claude_max_email: String,
    pub machine_id: String,
    pub version: String,
    /// re-issued on re-validation; omitted on first activation
    #[serde(skip_serializing_if = "String::is_empty")]
    pub activation_token: String,
}

/// `POST /api/kit/validate` response (valid + invalid unified at HTTP 200).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ValidateResponse {
    #[serde(default)]
    pub valid: bool,
    /// set when `!valid`: `not_found` | `not_paid` | `email_mismatch`
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub product: String,
    #[serde(default)]
    pub includes_bump: bool,
    #[serde(default)]
    pub reference: String,
    #[serde(default)]
    pub activated_at: String,
    #[serde(default)]
    pub activation_token: String,
    #[serde(default)]
    pub bound_email: String,
    #[serde(default)]
    pub latest_version: String,
}

/// The network seam. `Err` = transport failure → caller fails open to the
/// cached `[validation]` result; a rejected license is a successful call with
/// `valid: false`.
pub trait UpdateChannel {
    fn validate(&self, req: &ValidateRequest) -> Result<ValidateResponse>;
}

/// Live channel against chrisai.cv.
pub struct HttpChannel;

impl UpdateChannel for HttpChannel {
    fn validate(&self, req: &ValidateRequest) -> Result<ValidateResponse> {
        let url = format!("{}{}", api_base(), VALIDATE_PATH);
        let resp = ureq::post(&url)
            .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
            .send_json(req)
            .context("validate request failed")?;
        resp.into_json().context("parsing validate response")
    }
}

/// In-memory channel for tests and offline development.
pub struct MockChannel {
    pub response: ValidateResponse,
}

impl MockChannel {
    pub fn valid(latest_version: &str) -> Self {
        Self {
            response: ValidateResponse {
                valid: true,
                product: "operator-kit".into(),
                latest_version: latest_version.into(),
                ..Default::default()
            },
        }
    }

    pub fn invalid(reason: &str) -> Self {
        Self {
            response: ValidateResponse {
                valid: false,
                reason: reason.into(),
                ..Default::default()
            },
        }
    }
}

impl UpdateChannel for MockChannel {
    fn validate(&self, _req: &ValidateRequest) -> Result<ValidateResponse> {
        Ok(self.response.clone())
    }
}

/// Derive a stable machine fingerprint: `sha256(seed)[:32]`, where `seed` is the
/// `machineID` from `~/.claude.json` (preferred — gives parity with the
/// installer for free) or the hostname. Advisory only; the server does not
/// currently enforce it.
pub fn machine_id() -> String {
    // Computed once per process. The seed comes from ~/.claude.json, which Claude
    // Code rewrites continuously — so two uncached calls could read the file
    // mid-write, fall through to the hostname on one of them, and disagree.
    // A machine id cannot change within a process; caching makes that explicit.
    static CACHED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CACHED
        .get_or_init(|| {
            let seed = claude_machine_id()
                .or_else(hostname)
                .unwrap_or_else(|| "unknown".to_string());
            let digest = Sha256::digest(seed.as_bytes());
            let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            hex[..32].to_string()
        })
        .clone()
}

fn claude_machine_id() -> Option<String> {
    let path = dirs::home_dir()?.join(".claude.json");
    let content = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    json.get("machineID")?.as_str().map(str::to_string)
}

fn hostname() -> Option<String> {
    for var in ["HOSTNAME", "HOST"] {
        if let Ok(h) = std::env::var(var)
            && !h.is_empty()
        {
            return Some(h);
        }
    }
    None
}

/// Build a re-validation request from stored license state.
fn request_from(license: &LicenseFile, current_version: &str) -> ValidateRequest {
    ValidateRequest {
        license_key: license.license.license_key.clone(),
        email: license.license.purchase_email.clone(),
        claude_max_email: license.license.bound_email.clone(),
        machine_id: machine_id(),
        version: current_version.to_string(),
        activation_token: license.license.activation_token.clone(),
    }
}

/// Validate against the channel and fold the result into the license file's
/// `[validation]` section (the fail-open source of truth). Returns the fresh
/// response.
///
/// On transport `Err` the license file is left untouched so the caller can fail
/// open to the existing cache. This mutates `license` in memory only; the caller
/// decides when to [`LicenseFile::save`].
pub fn validate_and_refresh(
    license: &mut LicenseFile,
    channel: &dyn UpdateChannel,
    current_version: &str,
) -> Result<ValidateResponse> {
    let req = request_from(license, current_version);
    let resp = channel.validate(&req)?;

    license.validation.last_checked = now_utc();
    license.validation.last_result = if resp.valid {
        "valid".to_string()
    } else {
        resp.reason.clone()
    };
    if !resp.latest_version.is_empty() {
        license.validation.latest_version = resp.latest_version.clone();
    }
    // Persist a re-issued token / corrected bound email if the server returned one.
    if !resp.activation_token.is_empty() {
        license.license.activation_token = resp.activation_token.clone();
    }
    if !resp.bound_email.is_empty() {
        license.license.bound_email = resp.bound_email.clone();
    }

    Ok(resp)
}

fn now_utc() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

// ─── Payload fetch + binary install (the artifact half) ──────
//
// The base binary ships as a per-platform GitHub Release asset, served by the
// website's license-gated `POST /api/kit/payload?component=base` (the activation
// token is the auth — see apps/chrisai-installer/lib/payload.js + HANDOFF.md).
// `base update` validates, pulls the tarball, version-checks, then ATOMICALLY
// swaps `~/.local/bin/base`. SURGICAL by construction: the only files touched are
// `~/.local/bin/base` and `~/.base-gbl/license.toml` — never user state
// (domains.toml, base.toml, commands.toml, graphs, custom rules, .env) or any
// other shipped file. Refreshing changed shipped docs/templates is a deferred
// follow-up (needs a base-owned-file manifest + checksum differ).

const PAYLOAD_PATH: &str = "/api/kit/payload";
/// Cap the download so a misbehaving endpoint can't fill the disk.
const MAX_PAYLOAD_BYTES: u64 = 256 * 1024 * 1024;
/// Payload is multi-MB; give it more headroom than the small validate call.
const PAYLOAD_TIMEOUT_SECS: u64 = 180;

/// Map the current OS/arch to the kit's platform token (matches installer config.js).
pub fn platform_token() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("linux-x64"),
        ("macos", "aarch64") => Ok("darwin-arm64"),
        ("windows", "x86_64") => Ok("win32-x64"),
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

/// True when `current` is at least `latest` (no update needed). Numeric, not string.
fn is_current(current: &str, latest: &str) -> bool {
    semver_tuple(current) >= semver_tuple(latest)
}

/// `POST /api/kit/payload` — stream the platform-gated tarball to `dir/base.tar.gz`.
fn fetch_base_payload(license: &LicenseFile, platform: &str, dir: &Path) -> Result<PathBuf> {
    let url = format!("{}{PAYLOAD_PATH}", api_base());
    let body = serde_json::json!({
        "license_key": license.license.license_key,
        "activation_token": license.license.activation_token,
        "component": "base",
        "platform": platform,
    });

    let resp = match ureq::post(&url)
        .timeout(std::time::Duration::from_secs(PAYLOAD_TIMEOUT_SECS))
        .set("accept", "application/octet-stream")
        .send_json(body)
    {
        Ok(r) => r,
        Err(ureq::Error::Status(404, _)) => {
            bail!("no base build available for platform '{platform}' (HTTP 404)")
        }
        Err(ureq::Error::Status(code, resp)) => {
            let reason = resp
                .into_json::<serde_json::Value>()
                .ok()
                .and_then(|j| {
                    j.get("reason")
                        .or_else(|| j.get("error"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| format!("http_{code}"));
            bail!("payload fetch refused: {reason} (HTTP {code}) — license gate");
        }
        Err(e) => bail!("payload request failed: {e}"),
    };

    let tarball = dir.join("base.tar.gz");
    let mut reader = resp.into_reader().take(MAX_PAYLOAD_BYTES);
    let mut out = std::fs::File::create(&tarball)
        .with_context(|| format!("creating {}", tarball.display()))?;
    let n = std::io::copy(&mut reader, &mut out).context("streaming payload to disk")?;
    if n == 0 {
        bail!("payload was empty");
    }
    Ok(tarball)
}

/// Extract a tar.gz with system `tar` and locate the `base` executable inside
/// (direct child, or nested — GitHub tarballs wrap in a single top-level dir).
fn extract_and_locate(tarball: &Path, dest_dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dest_dir)?;
    let status = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(tarball)
        .arg("-C")
        .arg(dest_dir)
        .status()
        .context("running system tar")?;
    if !status.success() {
        bail!("tar extraction failed for {}", tarball.display());
    }
    find_base_binary(dest_dir, 3)
        .ok_or_else(|| anyhow::anyhow!("extracted payload did not contain a `base` binary"))
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

/// The canonical install location the chrisai installer manages.
fn install_dest() -> Result<PathBuf> {
    dirs::home_dir()
        .map(|h| h.join(".local").join("bin").join("base"))
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

/// `base update` — validate the license, pull the platform binary, atomic-swap.
/// `check_only` stops after the version check; `force` installs even when current.
pub fn run(check_only: bool, force: bool) -> Result<()> {
    let mut lf = LicenseFile::load()
        .filter(|l| !l.license.license_key.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no license at ~/.base-gbl/license.toml — purchase + activate first: {CHECKOUT_URL}"
            )
        })?;

    let current = env!("CARGO_PKG_VERSION");
    println!("base {current} — checking {} for updates …", api_base());

    // Re-validate. For an explicit command a transport failure is a hard stop
    // (unlike the session-start hook, which fails open to the cached result).
    let resp = validate_and_refresh(&mut lf, &HttpChannel, current)
        .context("could not reach the validation service")?;
    let _ = lf.save();
    if !resp.valid {
        let reason = if resp.reason.is_empty() { "invalid".into() } else { resp.reason };
        bail!("license validation failed: {reason} — base update is gated ({CHECKOUT_URL})");
    }

    // Decide from the server's reported latest version (saves a download).
    if !resp.latest_version.is_empty() {
        if !force && is_current(current, &resp.latest_version) {
            println!("✓ already up to date (base {current}).");
            return Ok(());
        }
        println!("→ update available: {current} → {}", resp.latest_version);
    } else {
        println!("→ server did not report a latest version; fetching to compare.");
    }
    if check_only {
        println!("(--check) not installing.");
        return Ok(());
    }

    let platform = platform_token()?;
    let work = std::env::temp_dir().join(format!("base-update-{}", std::process::id()));
    std::fs::create_dir_all(&work).with_context(|| format!("creating {}", work.display()))?;
    let outcome = (|| -> Result<()> {
        let tarball = fetch_base_payload(&lf, platform, &work)?;
        let new_bin = extract_and_locate(&tarball, &work.join("x"))?;

        // Authoritative version guard against the extracted binary.
        let new_ver = binary_version(&new_bin).unwrap_or_default();
        if !force && !new_ver.is_empty() && is_current(current, &new_ver) {
            println!("✓ already running the latest base ({current}).");
            return Ok(());
        }

        let dest = install_dest()?;
        atomic_swap(&new_bin, &dest)?;
        let shown = if new_ver.is_empty() { "(new)".to_string() } else { new_ver };
        println!("✓ updated: {} is now base {shown}", dest.display());
        println!("  (only the binary + license.toml were touched — shipped docs/hooks unchanged;");
        println!("   run `base install` if a release adds new ones.)");
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&work);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::license::{LicenseFile, LicenseSection};

    fn licensed_but_unchecked() -> LicenseFile {
        LicenseFile {
            license: LicenseSection {
                license_key: "OK-ABC".into(),
                purchase_email: "buyer@gmail.com".into(),
                bound_email: "buyer@gmail.com".into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn machine_id_is_32_hex_and_deterministic() {
        let a = machine_id();
        let b = machine_id();
        assert_eq!(a, b);
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn validate_request_omits_empty_token() {
        let req = request_from(&licensed_but_unchecked(), "0.5.0");
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["license_key"], "OK-ABC");
        assert_eq!(json["claude_max_email"], "buyer@gmail.com");
        assert_eq!(json["version"], "0.5.0");
        assert!(json.get("activation_token").is_none(), "empty token is skipped");
    }

    #[test]
    fn response_tolerates_missing_and_extra_fields() {
        let raw = r#"{"valid": true, "latest_version": "1.2.0", "surprise": 7}"#;
        let resp: ValidateResponse = serde_json::from_str(raw).expect("parse");
        assert!(resp.valid);
        assert_eq!(resp.latest_version, "1.2.0");
        assert_eq!(resp.reason, "");
    }

    #[test]
    fn refresh_on_valid_marks_validated_and_caches_version() {
        let mut lf = licensed_but_unchecked();
        let resp = validate_and_refresh(&mut lf, &MockChannel::valid("1.0.0"), "0.5.0").unwrap();
        assert!(resp.valid);
        assert_eq!(lf.validation.last_result, "valid");
        assert_eq!(lf.validation.latest_version, "1.0.0");
        assert!(!lf.validation.last_checked.is_empty());
        assert!(lf.is_licensed());
    }

    #[test]
    fn refresh_on_invalid_records_reason_and_blocks_gate() {
        let mut lf = licensed_but_unchecked();
        let resp = validate_and_refresh(&mut lf, &MockChannel::invalid("not_paid"), "0.5.0").unwrap();
        assert!(!resp.valid);
        assert_eq!(lf.validation.last_result, "not_paid");
        assert!(!lf.is_licensed());
    }

    // ─── Artifact half: version + platform + locate ──────────

    #[test]
    fn semver_parsing_and_ordering() {
        assert_eq!(semver_tuple("0.9.0"), (0, 9, 0));
        assert_eq!(semver_tuple("v1.2.3"), (1, 2, 3));
        assert_eq!(semver_tuple("0.10.0"), (0, 10, 0));
        assert_eq!(semver_tuple("1"), (1, 0, 0));
        assert_eq!(semver_tuple("0.8.0-rc1"), (0, 8, 0));
    }

    #[test]
    fn is_current_compares_numerically_not_lexically() {
        assert!(is_current("0.9.0", "0.9.0")); // equal → current
        assert!(is_current("0.10.0", "0.9.0")); // 10 > 9 numerically (string compare fails)
        assert!(is_current("1.0.0", "0.9.9"));
        assert!(!is_current("0.8.0", "0.9.0")); // behind → update
        assert!(!is_current("0.9.0", "0.9.1"));
    }

    #[test]
    fn platform_token_resolves_or_errors_clearly() {
        match platform_token() {
            Ok(t) => assert!(["linux-x64", "darwin-arm64", "win32-x64"].contains(&t)),
            Err(e) => assert!(e.to_string().contains("no base build")),
        }
    }

    #[test]
    fn find_base_binary_direct_and_nested() {
        let tmp = tempfile::tempdir().unwrap();
        // nested one level: tmp/wrap/base (mirrors a GitHub tarball's single child dir)
        let wrap = tmp.path().join("wrap");
        std::fs::create_dir_all(&wrap).unwrap();
        let bin = wrap.join(if cfg!(windows) { "base.exe" } else { "base" });
        std::fs::write(&bin, b"#!/bin/sh\n").unwrap();
        assert_eq!(find_base_binary(tmp.path(), 3), Some(bin));
        // depth 0 from the top can't reach the nested file
        assert_eq!(find_base_binary(tmp.path(), 0), None);
    }
}
