//! Harness adapters: the transport between base's hook engine and the agent
//! host that calls it.
//!
//! The engine — matching, budgets, graph writes, every emitter — speaks one
//! dialect: the Claude Code hook payload in, and (mostly) a Claude-shaped JSON
//! envelope out. That dialect is load-bearing and 400 KB of code depends on it.
//! So an adapter is not a second engine; it is a translation at the process
//! boundary:
//!
//! - [`inbound`] turns the host's payload into the shape the engine already
//!   reads (tool names normalized, `tool_input` in the right place).
//! - [`outbound`] turns what the engine printed into what the host reads
//!   (`hookSpecificOutput.additionalContext` for Claude and Cursor,
//!   `additionalContext` for Copilot and OpenCode).
//!
//! Anything that differs beyond those two edges — where the config lives, what
//! the event is called, how the file is merged — lives in the harness module
//! itself, behind [`Binding`].
//!
//! Two rules hold for every adapter:
//!
//! - **Fail open.** A host payload we do not understand must never break the
//!   user's turn. An unknown tool name passes through; an unparseable body
//!   becomes `{}`.
//! - **Never clobber.** `~/.cursor/hooks.json` is the operator's file, and this
//!   machine already has two other tools' hooks in it. Wiring appends and
//!   un-wiring removes only what base wrote.

use std::path::{Path, PathBuf};

pub mod antigravity;
pub mod claude;
pub mod copilot;
pub mod cursor;
pub mod opencode;

/// Every host base can attach to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    Claude,
    OpenCode,
    Cursor,
    Copilot,
    Antigravity,
}

impl Harness {
    pub const ALL: [Harness; 5] = [
        Harness::Claude,
        Harness::OpenCode,
        Harness::Cursor,
        Harness::Copilot,
        Harness::Antigravity,
    ];

    /// The names `--harness` accepts. `claude-code` is the spelling the docs use;
    /// `claude` is what the rest of this tree calls it.
    pub fn parse(s: &str) -> Option<Harness> {
        match s.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" | "claudecode" => Some(Harness::Claude),
            "opencode" | "open-code" => Some(Harness::OpenCode),
            "cursor" => Some(Harness::Cursor),
            "copilot" | "github-copilot" | "copilot-cli" => Some(Harness::Copilot),
            "antigravity" | "antigravity-cli" | "gemini" | "gemini-cli" | "agy" => {
                Some(Harness::Antigravity)
            }
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::OpenCode => "opencode",
            Harness::Cursor => "cursor",
            Harness::Copilot => "copilot",
            Harness::Antigravity => "antigravity",
        }
    }
}

/// The canonical event names the engine understands.
pub const EVENTS: [&str; 6] = [
    "session-start",
    "user-prompt-submit",
    "pre-tool-use",
    "post-tool-use",
    "stop",
    "session-end",
];

/// A host's own event name → the canonical one.
///
/// The generated config files already call `base hook <canonical> --harness …`,
/// so this exists for the other direction: a host that hands base its own event
/// name (an OpenCode plugin, a hand-written hook) still reaches the right arm.
/// Unknown names return `None`; the engine treats those as a no-op event, which
/// is what a hook nobody implements should be.
pub fn canonical_event(harness: Harness, raw: &str) -> Option<&'static str> {
    let name = raw.trim();
    if let Some(hit) = EVENTS.iter().find(|e| e.eq_ignore_ascii_case(name)) {
        return Some(hit);
    }
    let host_name = match harness {
        Harness::Claude => match name {
            "SessionStart" => "session-start",
            "UserPromptSubmit" => "user-prompt-submit",
            "PreToolUse" => "pre-tool-use",
            "PostToolUse" => "post-tool-use",
            "Stop" => "stop",
            "SessionEnd" => "session-end",
            _ => return None,
        },
        Harness::Cursor => match name {
            "sessionStart" => "session-start",
            "beforeSubmitPrompt" => "user-prompt-submit",
            "preToolUse" => "pre-tool-use",
            "postToolUse" => "post-tool-use",
            "stop" => "stop",
            "sessionEnd" => "session-end",
            _ => return None,
        },
        Harness::Copilot => match name {
            "sessionStart" => "session-start",
            "userPromptSubmitted" => "user-prompt-submit",
            "preToolUse" => "pre-tool-use",
            "postToolUse" => "post-tool-use",
            "agentStop" => "stop",
            "sessionEnd" => "session-end",
            _ => return None,
        },
        Harness::Antigravity => match name {
            "SessionStart" => "session-start",
            "PreInvocation" => "user-prompt-submit",
            "PreToolUse" => "pre-tool-use",
            "PostToolUse" => "post-tool-use",
            "Stop" => "stop",
            _ => return None,
        },
        // The OpenCode plugin speaks canonical names; it is code we ship, not a
        // payload we have to guess at.
        Harness::OpenCode => return None,
    };
    Some(host_name)
}

/// The host payload in the shape the engine reads.
pub fn inbound(harness: Harness, event: &str, raw: serde_json::Value) -> serde_json::Value {
    match harness {
        Harness::Claude => claude::inbound(raw),
        Harness::OpenCode => opencode::inbound(raw),
        Harness::Cursor => cursor::inbound(event, raw),
        Harness::Copilot => copilot::inbound(event, raw),
        Harness::Antigravity => antigravity::inbound(event, raw),
    }
}

/// What the engine printed, in the shape the host reads.
///
/// `None` means "print nothing": an empty emission on a host that would treat an
/// empty envelope as an instruction.
pub fn outbound(harness: Harness, event: &str, text: &str) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    match harness {
        Harness::Claude => claude::outbound(text),
        Harness::OpenCode => opencode::outbound(event, text),
        Harness::Cursor => cursor::outbound(event, text),
        Harness::Copilot => copilot::outbound(event, text),
        Harness::Antigravity => antigravity::outbound(event, text),
    }
}

/// The model-facing text inside whatever the engine printed.
///
/// The engine emits two shapes today: plain text (session start, prompt) and a
/// Claude envelope (`{"hookSpecificOutput":{"additionalContext":…}}`, the tool
/// hooks). Adapters that need to re-wrap the text need it out of the envelope
/// first — and must not re-wrap the envelope itself, which would put JSON inside
/// the model's context.
pub fn context_of(text: &str) -> String {
    let trimmed = text.trim();
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(ctx) = json
            .get("hookSpecificOutput")
            .and_then(|h| h.get("additionalContext"))
            .and_then(|v| v.as_str())
        {
            return ctx.to_string();
        }
        if let Some(ctx) = json.get("additionalContext").and_then(|v| v.as_str()) {
            return ctx.to_string();
        }
        if let Some(ctx) = json.get("systemMessage").and_then(|v| v.as_str()) {
            return ctx.to_string();
        }
    }
    trimmed.to_string()
}

// ─── Install bindings ───────────────────────────────────────

/// What one harness's wiring did, in the terms `base install` prints.
#[derive(Debug, Clone)]
pub struct WireOutcome {
    pub path: PathBuf,
    /// Entries added (`wire`) or removed (`unwire`). Zero means "already right".
    pub changed: usize,
    /// One line for the install/uninstall transcript. Empty for "nothing to say".
    pub note: String,
}

impl WireOutcome {
    pub fn unchanged(path: PathBuf, note: impl Into<String>) -> Self {
        Self { path, changed: 0, note: note.into() }
    }
}

/// One host's config file and the rules for changing it.
///
/// Object-safe on purpose: [`registry`] hands back `&'static dyn Binding` so the
/// install path does not need a `match` that has to grow every time a host is
/// added — and so each adapter lives in its own file, which is what lets three
/// people implement three hosts without touching each other's code.
pub trait Binding: Sync {
    fn harness(&self) -> Harness;

    /// Whether this host looks present. Deliberately cheap and non-committal:
    /// a config directory is evidence, not proof, and a host installed after
    /// base must still be wireable by re-running `base install`.
    fn detected(&self, home: &Path) -> bool;

    /// Where the wiring goes. Printed whether or not it exists, so `base
    /// doctor` can say "not wired" about the right file.
    fn config_path(&self, home: &Path) -> PathBuf;

    /// Add base's entries. Idempotent: a second call changes nothing.
    fn wire(&self, home: &Path, exe: &Path) -> anyhow::Result<WireOutcome>;

    /// Remove base's entries and nothing else. Idempotent.
    fn unwire(&self, home: &Path) -> anyhow::Result<WireOutcome>;
}

/// Every adapter, in the order `base install` reports them.
pub fn registry() -> &'static [&'static dyn Binding] {
    static REGISTRY: [&'static dyn Binding; 5] = [
        &claude::BINDING,
        &opencode::BINDING,
        &cursor::BINDING,
        &copilot::BINDING,
        &antigravity::BINDING,
    ];
    &REGISTRY
}

pub fn binding(h: Harness) -> &'static dyn Binding {
    registry()
        .iter()
        .copied()
        .find(|b| b.harness() == h)
        .expect("every Harness has a Binding in the registry")
}

/// Wire every detected host, or just `only` when the operator named one.
///
/// A host that is not detected is skipped, not an error: `base install` on a
/// machine with no Cursor must not fail because Cursor is missing. A host that
/// IS detected and cannot be wired is reported and does not stop the others —
/// one broken config file must not cost the user the rest of the install.
pub fn wire_detected(
    home: &Path,
    exe: &Path,
    only: Option<Harness>,
) -> Vec<(Harness, anyhow::Result<WireOutcome>)> {
    let mut out = Vec::new();
    for b in registry() {
        let h = b.harness();
        if only.is_some_and(|o| o != h) {
            continue;
        }
        if only.is_none() && !b.detected(home) {
            continue;
        }
        out.push((h, b.wire(home, exe)));
    }
    out
}

/// Un-wire every host base could have wired, or just `only`.
pub fn unwire_all(
    home: &Path,
    only: Option<Harness>,
) -> Vec<(Harness, anyhow::Result<WireOutcome>)> {
    let mut out = Vec::new();
    for b in registry() {
        let h = b.harness();
        if only.is_some_and(|o| o != h) {
            continue;
        }
        out.push((h, b.unwire(home)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_harness_parses_from_its_own_name() {
        for h in Harness::ALL {
            assert_eq!(Harness::parse(h.name()), Some(h), "{} did not round-trip", h.name());
        }
        assert_eq!(Harness::parse("Claude-Code"), Some(Harness::Claude));
        assert_eq!(Harness::parse("github-copilot"), Some(Harness::Copilot));
        assert_eq!(Harness::parse("emacs"), None);
    }

    #[test]
    fn every_harness_has_a_binding() {
        for h in Harness::ALL {
            assert_eq!(binding(h).harness(), h);
        }
    }

    #[test]
    fn canonical_names_pass_through_for_every_host() {
        for h in Harness::ALL {
            for e in EVENTS {
                assert_eq!(canonical_event(h, e), Some(e), "{h:?} lost {e}");
            }
        }
    }

    #[test]
    fn host_event_names_reach_the_canonical_arm() {
        assert_eq!(canonical_event(Harness::Claude, "PreToolUse"), Some("pre-tool-use"));
        assert_eq!(canonical_event(Harness::Cursor, "beforeSubmitPrompt"), Some("user-prompt-submit"));
        assert_eq!(canonical_event(Harness::Copilot, "userPromptSubmitted"), Some("user-prompt-submit"));
        assert_eq!(canonical_event(Harness::Copilot, "agentStop"), Some("stop"));
        assert_eq!(canonical_event(Harness::Cursor, "sessionStart"), Some("session-start"));
        assert_eq!(canonical_event(Harness::Antigravity, "PreInvocation"), Some("user-prompt-submit"));
        assert_eq!(canonical_event(Harness::Antigravity, "SessionStart"), Some("session-start"));
        assert_eq!(canonical_event(Harness::Cursor, "noSuchEvent"), None);
    }

    #[test]
    fn an_envelope_is_unwrapped_and_plain_text_is_not() {
        let envelope = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":"ctx"}}"#;
        assert_eq!(context_of(envelope), "ctx");
        assert_eq!(context_of("  plain text\n"), "plain text");
    }

    #[test]
    fn nothing_is_printed_for_an_empty_emission() {
        for h in Harness::ALL {
            assert_eq!(outbound(h, "session-start", ""), None);
            assert_eq!(outbound(h, "session-start", "   \n"), None);
        }
    }

    #[test]
    fn a_registry_without_a_host_still_reports_the_others() {
        // The property the install path relies on: one adapter's failure is one
        // line, not a dead install.
        let tmp = tempfile::tempdir().unwrap();
        let out = unwire_all(tmp.path(), Some(Harness::Cursor));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, Harness::Cursor);
    }
}
