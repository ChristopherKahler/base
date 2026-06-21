//! Operator Kit license state — the per-buyer license + validation cache.
//!
//! Written by the `chrisai` installer on activation, read by BASE before gated
//! updates. Distinct from the shared Skool activation key in [`crate::manifest`]
//! (`chrisai.token`, attribution-only): this is the per-buyer update/upgrade gate.
//!
//! File: `~/.base-gbl/license.toml`. The schema mirrors the installer
//! byte-for-byte (apps/chrisai-installer/lib/license-store.js). Field names are
//! the contract — do not diverge.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// `~/.base-gbl/license.toml` — two sections, flat scalars.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LicenseFile {
    #[serde(default)]
    pub license: LicenseSection,
    #[serde(default)]
    pub validation: ValidationSection,
}

/// `[license]` — written by the installer on activation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LicenseSection {
    #[serde(default)]
    pub license_key: String,
    #[serde(default)]
    pub bound_email: String,
    #[serde(default)]
    pub purchase_email: String,
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
}

/// `[validation]` — the fail-open source of truth, refreshed on each re-validate.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ValidationSection {
    #[serde(default)]
    pub last_checked: String,
    /// `"valid"` on success, otherwise the rejection reason
    /// (`not_found` | `not_paid` | `email_mismatch`).
    #[serde(default)]
    pub last_result: String,
    #[serde(default)]
    pub latest_version: String,
}

const HEADER: &str = "# ChrisAI Operator Kit — license + validation state.\n\
                      # Written by `chrisai`; read by BASE for re-validation. Do not edit by hand.\n\n";

impl LicenseFile {
    /// Resolve path to `~/.base-gbl/license.toml`.
    pub fn license_path() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".base-gbl").join("license.toml"))
    }

    /// Load from `~/.base-gbl/license.toml`. None if missing or unparseable
    /// (an unactivated install simply has no file).
    pub fn load() -> Option<Self> {
        let path = Self::license_path()?;
        let content = std::fs::read_to_string(&path).ok()?;
        toml::from_str(&content).ok()
    }

    /// Atomic write (temp + rename), preserving the installer's header comment.
    pub fn save(&self) -> Result<()> {
        let path = Self::license_path().context("Cannot determine home directory")?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Creating {}", parent.display()))?;
        }

        let body = toml::to_string_pretty(self).context("Serializing license.toml")?;
        let content = format!("{HEADER}{body}");

        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, &content)
            .with_context(|| format!("Writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("Renaming {} → {}", tmp.display(), path.display()))?;

        Ok(())
    }

    /// Whether this install holds a validated per-buyer license (the update gate).
    ///
    /// Reflects the cached validation result; callers refresh it through the
    /// update channel before relying on it for a fresh gate decision. Orthogonal
    /// to [`crate::manifest::Manifest::is_activated`] (the attribution gate).
    pub fn is_licensed(&self) -> bool {
        !self.license.license_key.is_empty() && self.validation.last_result == "valid"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> LicenseFile {
        LicenseFile {
            license: LicenseSection {
                license_key: "OK-ABC123".into(),
                bound_email: "buyer@gmail.com".into(),
                purchase_email: "buyer@gmail.com".into(),
                product: "operator-kit".into(),
                includes_bump: true,
                reference: "OK-00042".into(),
                activated_at: "2026-06-19T17:20:00Z".into(),
                activation_token: "act_xyz".into(),
            },
            validation: ValidationSection {
                last_checked: "2026-06-19T17:20:00Z".into(),
                last_result: "valid".into(),
                latest_version: "1.0.0".into(),
            },
        }
    }

    #[test]
    fn toml_round_trip_preserves_fields() {
        let lf = sample();
        let body = toml::to_string_pretty(&lf).expect("serialize");
        let back: LicenseFile = toml::from_str(&body).expect("deserialize");
        assert_eq!(back.license.license_key, "OK-ABC123");
        assert_eq!(back.license.bound_email, "buyer@gmail.com");
        assert_eq!(back.license.purchase_email, "buyer@gmail.com");
        assert_eq!(back.license.product, "operator-kit");
        assert!(back.license.includes_bump);
        assert_eq!(back.license.reference, "OK-00042");
        assert_eq!(back.license.activation_token, "act_xyz");
        assert_eq!(back.validation.last_result, "valid");
        assert_eq!(back.validation.latest_version, "1.0.0");
    }

    #[test]
    fn parses_installer_written_file_with_header_and_comments() {
        // Exactly what apps/chrisai-installer serializes.
        let raw = "# ChrisAI Operator Kit — license + validation state.\n\
                   # Written by `chrisai`; read by BASE for re-validation. Do not edit by hand.\n\n\
                   [license]\n\
                   license_key = \"OK-REAL\"\n\
                   bound_email = \"a@b.com\"\n\
                   purchase_email = \"a@b.com\"\n\
                   product = \"operator-kit\"\n\
                   includes_bump = false\n\
                   reference = \"OK-00001\"\n\
                   activated_at = \"2026-06-19T17:20:00Z\"\n\
                   activation_token = \"act_1\"\n\n\
                   [validation]\n\
                   last_checked = \"2026-06-19T17:20:00Z\"\n\
                   last_result = \"valid\"\n\
                   latest_version = \"1.0.0\"\n";
        let lf: LicenseFile = toml::from_str(raw).expect("parse installer file");
        assert_eq!(lf.license.license_key, "OK-REAL");
        assert!(!lf.license.includes_bump);
        assert!(lf.is_licensed());
    }

    #[test]
    fn missing_sections_default_cleanly() {
        // A partial file (e.g. pre-validation) must not fail to parse.
        let lf: LicenseFile = toml::from_str("[license]\nlicense_key = \"OK-X\"\n").expect("parse");
        assert_eq!(lf.license.license_key, "OK-X");
        assert_eq!(lf.validation.last_result, "");
        assert!(!lf.is_licensed()); // key present but not yet validated
    }

    #[test]
    fn is_licensed_requires_key_and_valid_result() {
        let mut lf = sample();
        assert!(lf.is_licensed());

        lf.validation.last_result = "not_paid".into();
        assert!(!lf.is_licensed(), "rejected result is not licensed");

        let empty = LicenseFile::default();
        assert!(!empty.is_licensed(), "no key is not licensed");
    }
}
