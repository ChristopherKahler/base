use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};

// ─── Activation Key ─────────────────────────────────────────
// SHA-256 hash of the activation key. The actual key never appears in source or binary.
// Distributed via Skool classroom only.
const ACTIVATION_KEY_HASH: &str = "389858f21ff026eb17ed26be72d02929d26c0485cbfe2e8e63e980ee3df49d7c";

// ─── Manifest Structs ───────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct Manifest {
    pub chrisai: ChrisAiSection,
    #[serde(default)]
    pub components: HashMap<String, ComponentEntry>,
    #[serde(default)]
    pub update_check: UpdateCheck,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChrisAiSection {
    #[serde(default)]
    pub installed_at: String,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default)]
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentEntry {
    pub version: String,
    pub path: String,
    pub installed_at: String,
    /// Fingerprint of the component's primary content file exactly as shipped.
    ///
    /// Only `base-help` sets it, over `references/qa.md`. It is what separates
    /// "the operator appended their own Q&A pairs" from "the release changed the
    /// bank" — a whole-tree comparison cannot tell those apart, and the two want
    /// opposite handling. Defaulted so every manifest written before this field
    /// existed still round-trips.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheck {
    #[serde(default)]
    pub last_checked: String,
    #[serde(default = "default_ttl")]
    pub ttl_seconds: u64,
    #[serde(default)]
    pub pending_update: String,
    #[serde(default)]
    pub dismissed_until: String,
}

fn default_source() -> String {
    "https://www.skool.com/claude-code-titans-9203".into()
}

fn default_ttl() -> u64 {
    604800 // 7 days
}

impl Default for ChrisAiSection {
    fn default() -> Self {
        Self {
            installed_at: String::new(),
            source: default_source(),
            token: String::new(),
        }
    }
}

impl Default for UpdateCheck {
    fn default() -> Self {
        Self {
            last_checked: String::new(),
            ttl_seconds: default_ttl(),
            pending_update: String::new(),
            dismissed_until: String::new(),
        }
    }
}


// ─── Manifest I/O ───────────────────────────────────────────

impl Manifest {
    /// Resolve path to `~/.base-gbl/manifest.toml`.
    pub fn manifest_path() -> Option<PathBuf> {
        crate::home::home_root().map(|h| h.join(".base-gbl").join("manifest.toml"))
    }

    /// Load manifest from `~/.base-gbl/manifest.toml`. Returns None if missing or unparseable.
    pub fn load() -> Option<Self> {
        let path = Self::manifest_path()?;
        let content = std::fs::read_to_string(&path).ok()?;
        toml::from_str(&content).ok()
    }

    /// Atomic write to `~/.base-gbl/manifest.toml` (temp + rename).
    pub fn save(&self) -> Result<()> {
        let path = Self::manifest_path().context("Cannot determine home directory")?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Creating {}", parent.display()))?;
        }

        let content =
            toml::to_string_pretty(self).context("Serializing manifest to TOML")?;

        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, &content)
            .with_context(|| format!("Writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("Renaming {} → {}", tmp.display(), path.display()))?;

        Ok(())
    }

    /// Check if this install is activated (token hash matches compiled hash).
    pub fn is_activated(&self) -> bool {
        !self.chrisai.token.is_empty() && hash_key(&self.chrisai.token) == ACTIVATION_KEY_HASH
    }
}

/// SHA-256 hash a key string, return hex.
fn hash_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.trim().as_bytes());
    let result = hasher.finalize();
    result.iter().map(|b| format!("{:02x}", b)).collect()
}

// ─── Component Detection ────────────────────────────────────

/// Scan the filesystem for a known component and return its entry if found.
pub fn detect_component(name: &str) -> Option<ComponentEntry> {
    let home = crate::home::home_root()?;
    let now = chrono::Local::now().to_rfc3339();

    match name {
        "base" => {
            let bin = home.join(".local").join("bin").join("base");
            if bin.exists() {
                Some(ComponentEntry {
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    path: "~/.local/bin/base".to_string(),
                    installed_at: now,
                    content_hash: String::new(),
                })
            } else {
                None
            }
        }
        "paul" => detect_skill_component(&home, "paul-framework", &now),
        "seed" => detect_skill_component(&home, "seed", &now),
        "skillsmith" => detect_skill_component(&home, "skillsmith", &now),
        _ => None,
    }
}

/// Detect a skill component by checking known paths and reading package.json for version.
fn detect_skill_component(home: &Path, name: &str, now: &str) -> Option<ComponentEntry> {
    // Check ~/.claude/paul-framework/ (special case for PAUL) or ~/.claude/commands/{name}/
    let (dir, display_path) = if name == "paul-framework" {
        (
            home.join(".claude").join("paul-framework"),
            "~/.claude/paul-framework".to_string(),
        )
    } else {
        (
            home.join(".claude").join("commands").join(name),
            format!("~/.claude/commands/{name}"),
        )
    };

    if !dir.exists() {
        return None;
    }

    // Try package.json first, then skill entry point frontmatter
    let version = read_package_version(&dir)
        .or_else(|| read_skill_version(&dir, name))
        .unwrap_or_else(|| "unknown".to_string());

    Some(ComponentEntry {
        version,
        path: display_path,
        installed_at: now.to_string(),
        content_hash: String::new(),
    })
}

/// The base-help coach, installed at `~/.claude/skills/base-help/`.
///
/// Detected separately from `detect_skill_component` because that one looks
/// under `~/.claude/commands/<name>/` and reads a package.json or frontmatter
/// version — neither of which applies here. The coach's meaningful version is
/// the base release its Q&A bank was verified against, which is the only thing
/// that makes lag detectable: recording the *binary* version would match by
/// construction and could never report drift.
pub fn detect_base_help(home: &Path, now: &str) -> Option<ComponentEntry> {
    let dir = home.join(".claude").join("skills").join("base-help");
    let bank = dir.join("references").join("qa.md");
    if !bank.is_file() {
        return None;
    }
    let text = std::fs::read_to_string(&bank).ok()?;
    Some(ComponentEntry {
        version: bank_version(&text).unwrap_or_else(|| "unstamped".to_string()),
        path: "~/.claude/skills/base-help".to_string(),
        installed_at: now.to_string(),
        content_hash: hash_bytes(text.as_bytes()),
    })
}

/// Parse the base version a Q&A bank is stamped to. PURE.
///
/// The stamp is prose, not frontmatter — "Verified against base v0.12.3 on …"
/// or "Stamped for base v0.13.2 on …" — so match the `base v<semver>` token
/// rather than a fixed sentence, and take the first one so a later mention of an
/// older release in the body cannot win.
pub fn bank_version(text: &str) -> Option<String> {
    let mut rest = text;
    while let Some(i) = rest.find("base v") {
        let tail = &rest[i + "base v".len()..];
        let v: String = tail
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        // The stamp is prose, so the version is usually followed by sentence
        // punctuation: "base v0.13.2." must not parse as a four-part version.
        let v = v.trim_end_matches('.').to_string();
        if v.split('.').count() == 3 && v.split('.').all(|p| !p.is_empty()) {
            return Some(v);
        }
        rest = tail;
    }
    None
}

/// Hex sha256 of arbitrary bytes.
pub fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

/// Read version from a skill's entry point YAML frontmatter (e.g. seed.md, skillsmith.md).
fn read_skill_version(dir: &Path, name: &str) -> Option<String> {
    // Try {name}.md in the dir, or {name}/{name}.md for nested skills like skillsmith
    let candidates = [
        dir.join(format!("{name}.md")),
        dir.join(name).join(format!("{name}.md")),
    ];

    for path in &candidates {
        if let Ok(content) = std::fs::read_to_string(path) {
            // Parse YAML frontmatter between --- delimiters
            if content.starts_with("---")
                && let Some(end) = content[3..].find("---") {
                    let frontmatter = &content[3..3 + end];
                    for line in frontmatter.lines() {
                        let line = line.trim();
                        if let Some(rest) = line.strip_prefix("version:") {
                            let v = rest.trim().trim_matches('"').trim_matches('\'');
                            if !v.is_empty() {
                                return Some(v.to_string());
                            }
                        }
                    }
                }
        }
    }
    None
}

/// Read version from package.json in a directory.
fn read_package_version(dir: &Path) -> Option<String> {
    let pkg = dir.join("package.json");
    let content = std::fs::read_to_string(pkg).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    json.get("version")?.as_str().map(|s| s.to_string())
}

// ─── Activation ─────────────────────────────────────────────

/// Validate an activation key and write token to manifest.
pub fn activate(key: &str) -> Result<()> {
    let key = key.trim();

    if hash_key(key) != ACTIVATION_KEY_HASH {
        println!("════════════════════════════════════════");
        println!("⛔ Invalid activation key.\n");
        println!("Get your key at https://www.skool.com/claude-code-titans-9203");
        println!("════════════════════════════════════════");
        anyhow::bail!("Invalid activation key");
    }

    let mut manifest = Manifest::load().unwrap_or_default();
    manifest.chrisai.token = key.to_string();

    if manifest.chrisai.installed_at.is_empty() {
        manifest.chrisai.installed_at = chrono::Local::now().to_rfc3339();
    }

    manifest.save()?;

    println!("════════════════════════════════════════════════════════════════");
    println!("✓ Activated — attribution removed.\n");
    println!("Thank you for being a ChrisAI member.");
    println!("Chris AI Systems · https://www.skool.com/claude-code-titans-9203");
    println!("════════════════════════════════════════════════════════════════");

    Ok(())
}

// ─── Version Check ──────────────────────────────────────────

/// npm package names for each component
const NPM_PACKAGES: &[(&str, &str)] = &[
    ("paul", "paul-framework"),
    ("seed", "@chrisai/seed"),
    ("skillsmith", "@chrisai/skillsmith"),
];

pub(crate) const GITHUB_REPO: &str = "ChristopherKahler/base";
const HTTP_TIMEOUT_SECS: u64 = 3;

/// Reconcile the recorded base version with the binary that is actually running.
/// Returns true when the manifest was stale and has been corrected in-place
/// (caller is responsible for persisting).
///
/// `[components.base] version` is only ever written by `base install` or
/// `base update`, so replacing the binary by hand — dropping a fresh build into
/// `~/.local/bin/base` — leaves it pinned at the old version indefinitely.
/// Everything downstream then compares against that stale number:
/// `check_for_updates` keeps resolving the remote as newer, `pending_update`
/// stays set, and `check_and_banner` short-circuits on a non-empty
/// `pending_update` before reaching the periodic check that could have cleared
/// it. The banner therefore advertises a version the user already has, forever.
/// `base --version` never explains the discrepancy because it reports
/// `CARGO_PKG_VERSION` from the real binary and never consults the manifest.
pub fn reconcile_running_version(manifest: &mut Manifest) -> bool {
    let running = env!("CARGO_PKG_VERSION");
    let Some(entry) = manifest.components.get_mut("base") else {
        return false;
    };
    if entry.version == running {
        return false;
    }
    entry.version = running.to_string();
    // `pending_update` was computed against the version we just corrected, so it
    // is stale by construction. Clear it rather than let it keep the banner alive
    // until the next periodic check — that check is exactly what it blocks.
    manifest.update_check.pending_update = String::new();
    true
}

/// Check if enough time has passed since last version check.
pub fn should_check(manifest: &Manifest) -> bool {
    if manifest.update_check.last_checked.is_empty() {
        return true;
    }
    let Ok(last) = chrono::DateTime::parse_from_rfc3339(&manifest.update_check.last_checked) else {
        return true;
    };
    let elapsed = chrono::Local::now().signed_duration_since(last);
    elapsed.num_seconds() >= manifest.update_check.ttl_seconds as i64
}

/// Check if the update banner is currently snoozed.
pub fn is_snoozed(manifest: &Manifest) -> bool {
    if manifest.update_check.dismissed_until.is_empty() {
        return false;
    }
    let Ok(until) = chrono::DateTime::parse_from_rfc3339(&manifest.update_check.dismissed_until)
    else {
        return false;
    };
    chrono::Local::now() < until
}

/// Snooze the update banner for 24 hours.
pub fn snooze() -> Result<()> {
    let mut manifest = Manifest::load().unwrap_or_default();
    let dismiss_until = chrono::Local::now() + chrono::Duration::hours(24);
    manifest.update_check.dismissed_until = dismiss_until.to_rfc3339();
    manifest.save()?;

    println!("═══════════════════════════════════════");
    println!("⏸ Update banner snoozed for 24 hours.");
    println!("═══════════════════════════════════════");
    Ok(())
}

/// Fetch latest versions from npm registry + GitHub API, compare against installed.
/// Updates manifest in-place. Returns the pending_update string if updates found.
pub fn check_for_updates(manifest: &mut Manifest) -> Result<Option<String>> {
    let mut updates: Vec<String> = Vec::new();

    // Check npm components
    for &(component, package) in NPM_PACKAGES {
        if let Some(installed) = manifest.components.get(component)
            && let Some(remote) = fetch_npm_version(package)
                && version_newer(&remote, &installed.version) {
                    updates.push(format!("{component} {}→{remote}", installed.version));
                }
    }

    // Check BASE via GitHub releases
    if let Some(installed) = manifest.components.get("base")
        && let Some(remote) = fetch_github_version()
            && version_newer(&remote, &installed.version) {
                updates.push(format!("base {}→{remote}", installed.version));
            }

    // Update last_checked
    manifest.update_check.last_checked = chrono::Local::now().to_rfc3339();

    if updates.is_empty() {
        manifest.update_check.pending_update = String::new();
        Ok(None)
    } else {
        let pending = updates.join(", ");
        manifest.update_check.pending_update = pending.clone();
        Ok(Some(pending))
    }
}

/// Format the persistent update banner.
pub fn format_update_banner(pending: &str) -> String {
    format!(
        "\n═══════════════════════════════════════════════════════════════════\n\
         🔄 ChrisAI update available\n\
         \x20  {pending}\n\
         \n\
         \x20  Run: base update\n\
         \x20  Snooze 24h: base update --snooze\n\
         \x20  Chris AI Systems · https://www.skool.com/claude-code-titans-9203\n\
         ═══════════════════════════════════════════════════════════════════\n"
    )
}

/// Fetch latest version of an npm package. Returns None on any error.
fn fetch_npm_version(package: &str) -> Option<String> {
    let url = format!("https://registry.npmjs.org/{package}/latest");
    let resp = ureq::get(&url)
        .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
        .call()
        .ok()?;
    let json: serde_json::Value = resp.into_json().ok()?;
    json.get("version")?.as_str().map(|s| s.to_string())
}

/// Fetch latest BASE version from GitHub releases. Returns None on any error.
fn fetch_github_version() -> Option<String> {
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");
    let resp = ureq::get(&url)
        .set("User-Agent", "base-update-check")
        .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
        .call()
        .ok()?;
    let json: serde_json::Value = resp.into_json().ok()?;
    let tag = json.get("tag_name")?.as_str()?;
    Some(tag.strip_prefix('v').unwrap_or(tag).to_string())
}

/// Simple semver comparison: returns true if remote is newer than local.
fn version_newer(remote: &str, local: &str) -> bool {
    let parse = |v: &str| -> (u32, u32, u32) {
        let parts: Vec<u32> = v.split('.').filter_map(|s| s.parse().ok()).collect();
        (
            parts.first().copied().unwrap_or(0),
            parts.get(1).copied().unwrap_or(0),
            parts.get(2).copied().unwrap_or(0),
        )
    };
    let r = parse(remote);
    let l = parse(local);
    r > l
}

// ─── Tests ──────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_toml_round_trip() {
        let mut components = HashMap::new();
        components.insert(
            "base".to_string(),
            ComponentEntry {
                version: "0.1.0".to_string(),
                path: "~/.local/bin/base".to_string(),
                installed_at: "2026-06-03T15:00:00-05:00".to_string(),
                content_hash: String::new(),
            },
        );
        components.insert(
            "paul".to_string(),
            ComponentEntry {
                version: "1.4.0".to_string(),
                path: "~/.claude/paul-framework".to_string(),
                installed_at: "2026-06-03T15:00:00-05:00".to_string(),
                content_hash: String::new(),
            },
        );

        let manifest = Manifest {
            chrisai: ChrisAiSection {
                installed_at: "2026-06-03T15:00:00-05:00".to_string(),
                source: "https://www.skool.com/claude-code-titans-9203".to_string(),
                token: String::new(),
            },
            components,
            update_check: UpdateCheck {
                last_checked: "2026-06-03T15:00:00-05:00".to_string(),
                ttl_seconds: 604800,
                pending_update: String::new(),
                dismissed_until: String::new(),
            },
        };

        let serialized = toml::to_string_pretty(&manifest).expect("serialize");
        let deserialized: Manifest = toml::from_str(&serialized).expect("deserialize");

        assert_eq!(deserialized.chrisai.installed_at, manifest.chrisai.installed_at);
        assert_eq!(deserialized.chrisai.source, manifest.chrisai.source);
        assert_eq!(deserialized.chrisai.token, manifest.chrisai.token);
        assert_eq!(deserialized.update_check.ttl_seconds, 604800);
        assert_eq!(deserialized.components.len(), 2);
        assert_eq!(
            deserialized.components["base"].version,
            manifest.components["base"].version
        );
        assert_eq!(
            deserialized.components["paul"].version,
            manifest.components["paul"].version
        );
    }

    #[test]
    fn hash_key_is_deterministic() {
        let h1 = hash_key("test-input");
        let h2 = hash_key("test-input");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 = 64 hex chars
        assert_ne!(h1, hash_key("different-input"));
    }

    #[test]
    fn is_activated_with_empty_token() {
        let manifest = Manifest::default();
        assert!(!manifest.is_activated());
    }

    #[test]
    fn is_activated_with_wrong_token() {
        let manifest = Manifest {
            chrisai: ChrisAiSection {
                token: "wrong-key".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!manifest.is_activated());
    }

    #[test]
    fn version_newer_works() {
        assert!(version_newer("1.1.0", "1.0.0"));
        assert!(version_newer("2.0.0", "1.9.9"));
        assert!(version_newer("1.0.1", "1.0.0"));
        assert!(!version_newer("1.0.0", "1.0.0"));
        assert!(!version_newer("0.9.0", "1.0.0"));
        assert!(!version_newer("1.0.0", "1.0.1"));
    }

    #[test]
    fn should_check_empty_last_checked() {
        let manifest = Manifest::default();
        assert!(should_check(&manifest));
    }

    #[test]
    fn is_snoozed_empty() {
        let manifest = Manifest::default();
        assert!(!is_snoozed(&manifest));
    }

    #[test]
    fn is_snoozed_future() {
        let future = (chrono::Local::now() + chrono::Duration::hours(1)).to_rfc3339();
        let manifest = Manifest {
            update_check: UpdateCheck {
                dismissed_until: future,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(is_snoozed(&manifest));
    }

    #[test]
    fn is_snoozed_past() {
        let past = (chrono::Local::now() - chrono::Duration::hours(1)).to_rfc3339();
        let manifest = Manifest {
            update_check: UpdateCheck {
                dismissed_until: past,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!is_snoozed(&manifest));
    }

    /// Build a manifest whose recorded base version is whatever the caller says,
    /// with a pending_update string as if a check had already run against it.
    fn manifest_recording(version: &str, pending: &str) -> Manifest {
        let mut components = HashMap::new();
        components.insert(
            "base".to_string(),
            ComponentEntry {
                version: version.to_string(),
                path: "~/.local/bin/base".to_string(),
                installed_at: "2026-06-03T15:00:00-05:00".to_string(),
                content_hash: String::new(),
            },
        );
        Manifest {
            components,
            update_check: UpdateCheck {
                pending_update: pending.to_string(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn reconcile_rewrites_stale_version_and_clears_pending() {
        let mut m = manifest_recording("0.0.1", "base 0.0.1→9.9.9");
        assert!(reconcile_running_version(&mut m), "stale manifest must report a change");
        assert_eq!(m.components["base"].version, env!("CARGO_PKG_VERSION"));
        assert!(
            m.update_check.pending_update.is_empty(),
            "pending_update was computed against the stale version and must be dropped"
        );
    }

    #[test]
    fn reconcile_is_a_noop_when_already_current() {
        let mut m = manifest_recording(env!("CARGO_PKG_VERSION"), "base x→y");
        assert!(!reconcile_running_version(&mut m), "current manifest must not report a change");
        assert_eq!(
            m.update_check.pending_update, "base x→y",
            "a genuine pending update must survive when the version is already correct"
        );
    }

    #[test]
    fn reconcile_handles_manifest_without_base_component() {
        let mut m = Manifest::default();
        assert!(!reconcile_running_version(&mut m));
    }

    /// A rollback is the same shape as an upgrade: record what is running, drop
    /// the stale pending string, and let the next periodic check decide.
    #[test]
    fn reconcile_handles_downgrade() {
        let mut m = manifest_recording("99.0.0", "base 99.0.0→99.1.0");
        assert!(reconcile_running_version(&mut m));
        assert_eq!(m.components["base"].version, env!("CARGO_PKG_VERSION"));
        assert!(m.update_check.pending_update.is_empty());
    }
}

#[cfg(test)]
mod base_help_component_tests {
    use super::*;

    /// The stamp is prose and its wording changed between releases ("Verified
    /// against base v0.12.3", "Stamped for base v0.13.2"). Match the token.
    #[test]
    fn bank_version_reads_either_stamp_wording() {
        assert_eq!(
            bank_version("**Verified against base v0.12.3 on 2026-08-19.** …").as_deref(),
            Some("0.12.3")
        );
        assert_eq!(
            bank_version("**Stamped for base v0.13.2 on 2026-08-25 (sha `7dea613`).**").as_deref(),
            Some("0.13.2")
        );
    }

    /// A later mention of an older release in the body must not win over the
    /// header stamp.
    #[test]
    fn bank_version_takes_the_first_stamp() {
        let text = "Stamped for base v0.13.2.\n\nFixed in base v0.12.0, see base v0.11.0.";
        assert_eq!(bank_version(text).as_deref(), Some("0.13.2"));
    }

    #[test]
    fn bank_version_is_none_when_unstamped() {
        assert_eq!(bank_version("# A bank with no version stamp at all"), None);
        assert_eq!(bank_version("base v0.13 is not a semver triple"), None);
    }

    /// The whole point of recording the BANK's version rather than the binary's:
    /// a coach one release behind must be detectable.
    #[test]
    fn detect_base_help_records_the_bank_stamp_and_hash() {
        let home = tempfile::tempdir().unwrap();
        let refs = home.path().join(".claude").join("skills").join("base-help").join("references");
        std::fs::create_dir_all(&refs).unwrap();
        let body = "**Verified against base v0.12.3 on 2026-08-19.**\n";
        std::fs::write(refs.join("qa.md"), body).unwrap();

        let e = detect_base_help(home.path(), "2026-08-25T00:00:00-05:00").expect("detected");
        assert_eq!(e.version, "0.12.3", "records the bank stamp, not the binary");
        assert_eq!(e.content_hash, hash_bytes(body.as_bytes()));
        assert_eq!(e.path, "~/.claude/skills/base-help");
    }

    #[test]
    fn detect_base_help_is_none_when_not_installed() {
        let home = tempfile::tempdir().unwrap();
        assert!(detect_base_help(home.path(), "now").is_none());
    }

    #[test]
    fn detect_base_help_marks_an_unstamped_bank() {
        let home = tempfile::tempdir().unwrap();
        let refs = home.path().join(".claude").join("skills").join("base-help").join("references");
        std::fs::create_dir_all(&refs).unwrap();
        std::fs::write(refs.join("qa.md"), "no stamp here").unwrap();
        assert_eq!(detect_base_help(home.path(), "now").unwrap().version, "unstamped");
    }

    /// A manifest written before `content_hash` existed must still load. If this
    /// breaks, every existing install loses its component registry.
    #[test]
    fn manifest_round_trips_without_content_hash() {
        let toml = r#"
[chrisai]
installed_at = "2026-06-03T15:00:00-05:00"
source = "https://example.invalid"
token = ""

[components.base]
version = "0.13.1"
path = "~/.local/bin/base"
installed_at = "2026-06-03T15:00:00-05:00"

[update_check]
last_checked = ""
ttl_seconds = 86400
pending_update = ""
dismissed_until = ""
"#;
        let m: Manifest = toml::from_str(toml).expect("pre-content_hash manifest must still parse");
        assert_eq!(m.components["base"].version, "0.13.1");
        assert_eq!(m.components["base"].content_hash, "", "defaults empty");
    }

    /// An empty hash must not be written back out, so manifests stay clean for
    /// every component that has no content file.
    #[test]
    fn empty_content_hash_is_not_serialized() {
        let e = ComponentEntry {
            version: "0.13.2".to_string(),
            path: "~/.local/bin/base".to_string(),
            installed_at: "now".to_string(),
            content_hash: String::new(),
        };
        let out = toml::to_string(&e).unwrap();
        assert!(!out.contains("content_hash"), "got: {out}");
    }
}
