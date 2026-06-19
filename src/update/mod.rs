//! License validation channel — the network seam for the Operator Kit gate.
//!
//! All HTTP lives behind [`UpdateChannel`] so the `base update` command and the
//! session-start hook are testable offline and can fail open. [`HttpChannel`]
//! talks to chrisai.cv (`POST /api/kit/validate`); [`MockChannel`] is a test
//! double. The wire contract mirrors the `chrisai` installer
//! (apps/chrisai-installer/lib/activate.js) field-for-field — snake_case
//! throughout, do not diverge.
//!
//! The gate sits at *validation*, not at the artifact source: kit packages ship
//! on the npm registry, so a valid license is what unlocks `base update`'s pull,
//! not a signed download URL.

use anyhow::{Context, Result};
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
    let seed = claude_machine_id()
        .or_else(hostname)
        .unwrap_or_else(|| "unknown".to_string());
    let digest = Sha256::digest(seed.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    hex[..32].to_string()
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
}
