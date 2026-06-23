//! Secure secret store — `~/.base-gbl/.env`, the base-framework key vault.
//!
//! Secrets NEVER pass through the Claude conversation. `base secret set <KEY>`
//! prompts in the terminal with echo disabled (masked, paste-friendly), writes
//! the value to `~/.base-gbl/.env`, sets the file to `0600`, and never echoes
//! the value back. The plugin layer injects every `KEY=VALUE` here into command
//! plugins' environments (see `crate::plugin`).

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};

/// Path to the secret store, `~/.base-gbl/.env`.
pub fn env_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("cannot determine home directory")?;
    Ok(home.join(".base-gbl").join(".env"))
}

/// Set (upsert) a key by prompting for the value with echo disabled.
/// Reads from stdin (rpassword) so it's masked at a TTY and pipe-friendly for
/// automation (`echo val | base secret set KEY`). The value is never printed.
pub fn set_interactive(key: &str) -> Result<()> {
    validate_key(key)?;
    let value = rpassword::prompt_password(format!("Enter value for {key} (input hidden): "))
        .context("failed to read secret from terminal")?;
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!("empty value — nothing written");
    }
    upsert(key, value)?;
    println!("✓ {key} saved to {} (value not displayed)", env_path()?.display());
    Ok(())
}

/// Upsert `KEY=VALUE` into the store, preserving all other lines + comments.
/// Creates the file if absent and enforces `0600` perms on every write.
pub fn upsert(key: &str, value: &str) -> Result<()> {
    let path = env_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let existing = std::fs::read_to_string(&path).unwrap_or_default();

    let line = format!("{key}={value}");
    let mut out = String::new();
    let mut replaced = false;
    for raw in existing.lines() {
        let trimmed = raw.trim_start();
        let stripped = trimmed.strip_prefix("export ").unwrap_or(trimmed);
        let matches_key = stripped
            .split_once('=')
            .is_some_and(|(k, _)| k.trim() == key);
        if matches_key && !trimmed.starts_with('#') {
            out.push_str(&line);
            out.push('\n');
            replaced = true;
        } else {
            out.push_str(raw);
            out.push('\n');
        }
    }
    if !replaced {
        out.push_str(&line);
        out.push('\n');
    }

    write_secure(&path, &out)?;
    Ok(())
}

/// Remove a key. Returns true if a line was removed.
pub fn remove(key: &str) -> Result<bool> {
    let path = env_path()?;
    let Ok(existing) = std::fs::read_to_string(&path) else {
        return Ok(false);
    };
    let mut out = String::new();
    let mut removed = false;
    for raw in existing.lines() {
        let trimmed = raw.trim_start();
        let stripped = trimmed.strip_prefix("export ").unwrap_or(trimmed);
        let matches_key = stripped
            .split_once('=')
            .is_some_and(|(k, _)| k.trim() == key);
        if matches_key && !trimmed.starts_with('#') {
            removed = true; // drop the line
        } else {
            out.push_str(raw);
            out.push('\n');
        }
    }
    if removed {
        write_secure(&path, &out)?;
    }
    Ok(removed)
}

/// List the KEY names present, with masked values (never the full secret).
pub fn list() -> Result<()> {
    let path = env_path()?;
    let Ok(content) = std::fs::read_to_string(&path) else {
        println!("No secrets set. Use: base secret set <KEY>");
        return Ok(());
    };
    let mut any = false;
    for raw in content.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let line = trimmed.strip_prefix("export ").unwrap_or(trimmed);
        if let Some((k, v)) = line.split_once('=') {
            let k = k.trim();
            if k.is_empty() {
                continue;
            }
            println!("{:<28} {}", k, mask(v.trim()));
            any = true;
        }
    }
    if !any {
        println!("No secrets set. Use: base secret set <KEY>");
    } else {
        println!("\nStored in {} (0600). Values are masked.", path.display());
    }
    Ok(())
}

/// Mask a value for display: show length only, never characters.
fn mask(v: &str) -> String {
    let v = v.trim_matches(['"', '\'']);
    if v.is_empty() {
        "(empty)".into()
    } else {
        format!("•••••••• (set, {} chars)", v.chars().count())
    }
}

/// Write content and enforce owner-only perms (0600 on Unix).
fn write_secure(path: &std::path::Path, content: &str) -> Result<()> {
    let mut f = std::fs::File::create(path)
        .with_context(|| format!("cannot write {}", path.display()))?;
    f.write_all(content.as_bytes())?;
    f.flush()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("cannot chmod 600 {}", path.display()))?;
    }
    Ok(())
}

/// Keys are env-var names: letters, digits, underscore; not starting with a digit.
fn validate_key(key: &str) -> Result<()> {
    if key.is_empty() {
        anyhow::bail!("key is required");
    }
    let mut chars = key.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_alphabetic() || first == '_') {
        anyhow::bail!("invalid key '{key}': must start with a letter or underscore");
    }
    if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        anyhow::bail!("invalid key '{key}': only letters, digits, underscore allowed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_key_rules() {
        assert!(validate_key("GEMINI_API_KEY").is_ok());
        assert!(validate_key("_X").is_ok());
        assert!(validate_key("1BAD").is_err());
        assert!(validate_key("has-dash").is_err());
        assert!(validate_key("").is_err());
    }

    #[test]
    fn mask_never_reveals() {
        let m = mask("supersecretvalue");
        assert!(!m.contains("super"));
        assert!(m.contains("16 chars"));
        assert_eq!(mask(""), "(empty)");
    }

    // Upsert logic tested via a pure helper on text (avoids touching ~/.base-gbl).
    fn upsert_text(existing: &str, key: &str, value: &str) -> String {
        let line = format!("{key}={value}");
        let mut out = String::new();
        let mut replaced = false;
        for raw in existing.lines() {
            let trimmed = raw.trim_start();
            let stripped = trimmed.strip_prefix("export ").unwrap_or(trimmed);
            let matches_key = stripped.split_once('=').is_some_and(|(k, _)| k.trim() == key);
            if matches_key && !trimmed.starts_with('#') {
                out.push_str(&line);
                out.push('\n');
                replaced = true;
            } else {
                out.push_str(raw);
                out.push('\n');
            }
        }
        if !replaced {
            out.push_str(&line);
            out.push('\n');
        }
        out
    }

    #[test]
    fn upsert_appends_new_key() {
        let out = upsert_text("# header\nFOO=1\n", "GEMINI_API_KEY", "abc");
        assert!(out.contains("FOO=1"));
        assert!(out.contains("# header"));
        assert!(out.contains("GEMINI_API_KEY=abc"));
    }

    #[test]
    fn upsert_replaces_existing_key() {
        let out = upsert_text("GEMINI_API_KEY=old\nFOO=1\n", "GEMINI_API_KEY", "new");
        assert!(out.contains("GEMINI_API_KEY=new"));
        assert!(!out.contains("old"));
        assert!(out.contains("FOO=1"));
    }

    #[test]
    fn upsert_skips_commented_key() {
        // a commented placeholder must not be treated as the live key
        let out = upsert_text("# GEMINI_API_KEY=placeholder\n", "GEMINI_API_KEY", "real");
        assert!(out.contains("# GEMINI_API_KEY=placeholder"), "comment preserved");
        assert!(out.contains("GEMINI_API_KEY=real"), "real key appended");
    }
}
