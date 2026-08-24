use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

// ─── Operator Profile ───────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct OperatorProfile {
    pub name: String,
    #[serde(default)]
    pub north_star: String,
    #[serde(default)]
    pub deep_why: String,
    #[serde(default)]
    pub values: Vec<String>,
    #[serde(default)]
    pub vision: String,
    #[serde(default)]
    pub pitch: String,
    #[serde(default)]
    pub active: bool,
}

impl Default for OperatorProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            north_star: String::new(),
            deep_why: String::new(),
            values: Vec::new(),
            vision: String::new(),
            pitch: String::new(),
            active: true,
        }
    }
}

// ─── Load ───────────────────────────────────────────────────

/// Load operator profile from ~/.base-gbl/operator.toml.
/// Returns None if file missing or inactive.
pub fn load() -> Option<OperatorProfile> {
    let home = crate::home::home_root()?;
    let path = home.join(".base-gbl").join("operator.toml");
    load_from(&path)
}

pub fn load_from(path: &Path) -> Option<OperatorProfile> {
    let content = std::fs::read_to_string(path).ok()?;
    let profile: OperatorProfile = toml::from_str(&content).ok()?;
    if !profile.active || profile.name.is_empty() {
        return None;
    }
    Some(profile)
}

// ─── Emit ───────────────────────────────────────────────────

/// Format the operator block for session-start injection.
pub fn format_block(profile: &OperatorProfile) -> String {
    let values_str = if profile.values.is_empty() {
        "Not set".to_string()
    } else {
        profile.values.join(", ")
    };

    format!(
        "<operator>\n\
         North Star: {}\n\
         Deep Why: {}\n\
         Values: {}\n\
         Vision: {}\n\
         Pitch: {}\n\
         </operator>",
        if profile.north_star.is_empty() { "Not set" } else { &profile.north_star },
        if profile.deep_why.is_empty() { "Not set" } else { &profile.deep_why },
        values_str,
        if profile.vision.is_empty() { "Not set" } else { &profile.vision },
        if profile.pitch.is_empty() { "Not set" } else { &profile.pitch },
    )
}

// ─── CLI: show ──────────────────────────────────────────────

pub fn show() {
    match load() {
        Some(profile) => {
            println!("Operator: {}", profile.name);
            println!("North Star: {}", if profile.north_star.is_empty() { "Not set" } else { &profile.north_star });
            println!("Deep Why: {}", if profile.deep_why.is_empty() { "Not set" } else { &profile.deep_why });
            println!("Values: {}", if profile.values.is_empty() { vec!["Not set".into()] } else { profile.values.clone() }.join(", "));
            println!("Vision: {}", if profile.vision.is_empty() { "Not set" } else { &profile.vision });
            println!("Pitch: {}", if profile.pitch.is_empty() { "Not set" } else { &profile.pitch });
        }
        None => {
            eprintln!("No operator profile found. Run: base operator init");
        }
    }
}

// ─── CLI: init ──────────────────────────────────────────────

pub fn init(name: &str) -> Result<()> {
    let home = crate::home::home_root().context("Cannot determine home directory")?;
    let global_dir = home.join(".base-gbl");
    std::fs::create_dir_all(&global_dir)?;

    let path = global_dir.join("operator.toml");
    if path.exists() {
        println!("Operator profile already exists at {}", path.display());
        println!("Edit directly or use base operator show to view.");
        return Ok(());
    }

    std::fs::write(
        &path,
        format!(
            r#"# BASE — Operator Profile
# Built by Chris Kahler · Chris AI Systems
# Community: https://www.skool.com/claude-code-titans-9203
#
# This profile injects on every session start.
# Edit these fields to match your identity and goals.
# Set active = false to disable injection.

name = "{name}"
active = true

# Your primary metric + timeframe
north_star = ""

# The deepest reason you do what you do
deep_why = ""

# Core values (list)
values = []

# One-line vision of the life you're building
vision = ""

# Elevator pitch — who you are, what you do, for whom
pitch = ""
"#
        ),
    )?;

    println!("Operator profile created: {}", path.display());
    println!("Edit it to fill in your north star, deep why, values, vision, and pitch.");
    Ok(())
}
