//! C3: the AI's own marker at the start of a line of its reply (Chris's T2 markers `UPDATED:`, `MISREAD:`,
//! `DEFERRED:`, and base's `CORRECTED:` for every user).
//!
//! WHERE IT IS READ. Only in the text the AI wrote (`transcript::Event::Text`): the T2 rule text itself carries
//! `UPDATED:` and arrives in hundreds of transcripts through CLAUDE.md, hook output and system reminders, none of which
//! is assistant text.
//!
//! WHAT COUNTS, from Chris's transcripts since 2026-09-23: 89 `UPDATED:` lines at a line start (82 bare, 5 in a bullet,
//! 2 in bold) and 19 `MISREAD:`. The same word in backticks (4) or in the middle of a line (5) is the AI quoting or
//! discussing a marker, not using one, and so is anything inside a fenced code block. Case matters: "Updated: the
//! README" is a status line, not a marker.

/// One marker at a line start: its name without the colon (`UPDATED`) and the whole line, trimmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub kind: String,
    pub line: String,
}

/// The first line each configured marker starts in `text`, in text order.
pub fn find(text: &str, markers: &[String]) -> Vec<Found> {
    let mut out: Vec<Found> = Vec::new();
    let mut fence: Option<&'static str> = None;
    for raw in text.lines() {
        let line = raw.trim_start();
        if let Some(open) = fence {
            if line.starts_with(open) {
                fence = None;
            }
            continue;
        }
        if line.starts_with("```") {
            fence = Some("```");
            continue;
        }
        if line.starts_with("~~~") {
            fence = Some("~~~");
            continue;
        }
        let rest = after_lead_in(line);
        for m in markers.iter().map(|m| m.trim()).filter(|m| !m.is_empty()) {
            if starts_with_marker(rest, m) {
                let kind = m.trim_end_matches(':').to_string();
                if !out.iter().any(|f| f.kind == kind) {
                    out.push(Found { kind, line: raw.trim().to_string() });
                }
                break;
            }
        }
    }
    out
}

/// A line without its lead-in: one quote mark, then one bullet (`-`, `*`, `+`, `1.`, `1)`), each with its space.
fn after_lead_in(line: &str) -> &str {
    let mut s = line;
    if let Some(r) = s.strip_prefix('>') {
        s = r.trim_start();
    }
    for bullet in ["- ", "* ", "+ "] {
        if let Some(r) = s.strip_prefix(bullet) {
            return r.trim_start();
        }
    }
    let digits = s.chars().take_while(char::is_ascii_digit).count();
    if digits > 0
        && let Some(r) = s[digits..].strip_prefix(". ").or_else(|| s[digits..].strip_prefix(") "))
    {
        return r.trim_start();
    }
    s
}

/// `rest` begins with `marker`, bare or in bold or italic marks: `UPDATED:`, `**UPDATED:**`, `**UPDATED**:`,
/// `_UPDATED:_`. Never one in backticks: that is a quote.
fn starts_with_marker(rest: &str, marker: &str) -> bool {
    let inner = rest.trim_start_matches(['*', '_']);
    if inner.starts_with(marker) {
        return true;
    }
    // The emphasis closes before the colon: `**UPDATED**:`.
    let Some(word) = marker.strip_suffix(':') else {
        return false;
    };
    inner
        .strip_prefix(word)
        .is_some_and(|after| after.starts_with(['*', '_']) && after.trim_start_matches(['*', '_']).starts_with(':'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn markers() -> Vec<String> {
        crate::config::CorrectionsConfig::default().markers
    }

    fn kinds(text: &str) -> Vec<String> {
        find(text, &markers()).into_iter().map(|f| f.kind).collect()
    }

    #[test]
    fn a_marker_at_a_line_start_in_each_written_form() {
        assert_eq!(kinds("UPDATED: you built and installed base from your dev tree."), ["UPDATED"]);
        assert_eq!(kinds("Intro line.\n- UPDATED: the token holds nothing."), ["UPDATED"]);
        assert_eq!(kinds("**UPDATED:** the new evidence is the call."), ["UPDATED"]);
        assert_eq!(kinds("**MISREAD**: I read this as the other report."), ["MISREAD"]);
        assert_eq!(kinds("> CORRECTED: the path is the project folder."), ["CORRECTED"]);
        assert_eq!(kinds("1. DEFERRED: 38% stands, no new evidence."), ["DEFERRED"]);
    }

    #[test]
    fn quoting_a_marker_is_not_using_one() {
        assert!(kinds("Your CLAUDE.md makes me print `UPDATED:`, `MISREAD:` or `DEFERRED:` when I change.").is_empty());
        assert!(kinds("`UPDATED:` starts the reply.").is_empty());
        assert!(kinds("- **Our own sticker:** UPDATED: the merged layer holds it.").is_empty(), "mid-line");
        assert!(kinds("| view | UPDATED: allowed |").is_empty());
        assert!(kinds("```\nUPDATED: an example reply\n```\nafter the fence").is_empty(), "in a code fence");
        assert!(kinds("Updated: the README.").is_empty(), "case matters");
    }

    #[test]
    fn one_per_kind_in_text_order() {
        let found = find("MISREAD: first.\nUPDATED: second.\nUPDATED: third.", &markers());
        assert_eq!(found.iter().map(|f| f.kind.as_str()).collect::<Vec<_>>(), ["MISREAD", "UPDATED"]);
        assert_eq!(found[1].line, "UPDATED: second.");
    }
}
