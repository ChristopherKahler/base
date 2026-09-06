//! The first-run message, in one place.
//!
//! Every install path said something different: `base install` printed "Next
//! steps", a relay paragraph, a CARL line and an attribution block; `base
//! scaffold` printed its own "Next:" and its own banner; a first session start
//! printed neither. Three voices, none of which told a new user what base does
//! for them or what they would see first, and one of which ("type a prompt that
//! matches a domain keyword") assumed they already knew what a domain is.
//!
//! So the message lives here and every path renders it. [`text`] is what a
//! terminal gets. [`markdown`] is what an installing agent gets, and it opens
//! with an instruction to show the thing unchanged rather than summarise it —
//! Claude Code, Codex and Antigravity all install base by running it, and an
//! agent that paraphrases a welcome is an agent writing its own onboarding.
//!
//! The platform list is data. Wiring Codex adds a row, not a branch.

/// A tool base can wire itself into, and whether it wires it today.
struct Platform {
    name: &'static str,
    wired: bool,
}

const PLATFORMS: &[Platform] = &[
    Platform { name: "Claude Code", wired: true },
    Platform { name: "Codex", wired: false },
    Platform { name: "Antigravity", wired: false },
];

/// The three commands worth knowing on day one, each with its purpose in a few
/// words. Not the whole surface — the whole surface is what overloads people.
const COMMANDS: &[(&str, &str)] = &[
    ("base recall <word>", "find what you decided before"),
    ("base learn --text", "save something worth keeping"),
    ("base doctor", "check that base is healthy"),
];

const DOCS: &str = "https://docs.basemode.ai";
const INSTALL_DOCS: &str = "https://docs.basemode.ai/install";
const TRACKER: &str = "https://github.com/ChristopherKahler/base/issues";

/// The one attribution line, for every banner base prints. Four call sites
/// used to hand-roll their own; now there is one place to get it wrong.
pub const PRODUCT_LINE: &str = "base — Built by Chris Kahler";

/// "a", "a and b", "a, b and c".
fn join_names(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn wired() -> Vec<&'static str> {
    PLATFORMS.iter().filter(|p| p.wired).map(|p| p.name).collect()
}

fn not_wired() -> Vec<&'static str> {
    PLATFORMS.iter().filter(|p| !p.wired).map(|p| p.name).collect()
}

/// The platform sentence pair: what base wired, and what it has not wired yet.
/// Returns `None` for the second when everything is wired.
fn platform_lines() -> (String, Option<String>) {
    let on = format!("Wired for {}.", join_names(&wired()));
    let off = match not_wired().as_slice() {
        [] => None,
        [one] => Some(format!("{one} is not wired yet:")),
        many => Some(format!("{} are not wired yet:", join_names(many))),
    };
    (on, off)
}

/// The message as a terminal prints it.
pub fn text() -> String {
    let (on, off) = platform_lines();
    let mut s = String::new();

    s.push_str("base is installed.\n\n");
    s.push_str("It keeps a private record of how you work on this machine. Your decisions, your\n");
    s.push_str("corrections, the shape of your projects. It hands the relevant part back to your\n");
    s.push_str("AI tools when it matters. Nothing leaves this machine.\n\n");

    s.push_str("What you will notice first\n\n");
    s.push_str("  Your next AI session opens with a short block from base. Nothing else changes.\n\n");

    s.push_str("What to do now\n\n");
    s.push_str("  Open a new session and work normally. base learns from what you decide and what\n");
    s.push_str("  you correct. There is nothing to configure first.\n\n");

    s.push_str("When you want more\n\n");
    let width = COMMANDS.iter().map(|(c, _)| c.len()).max().unwrap_or(0);
    for (cmd, purpose) in COMMANDS {
        s.push_str(&format!("  {cmd:<width$}   {purpose}\n"));
    }
    s.push('\n');

    s.push_str(&on);
    if let Some(off) = off {
        s.push(' ');
        s.push_str(&off);
        s.push('\n');
        s.push_str(&format!("{INSTALL_DOCS}\n"));
    } else {
        s.push('\n');
    }
    s.push('\n');

    s.push_str(&format!("Read  {DOCS}\n"));
    s.push_str(&format!("Ask   {TRACKER}\n\n"));
    s.push_str(PRODUCT_LINE);
    s.push('\n');
    s
}

/// The same message for an installing agent to relay. The opening instruction
/// is the whole point: without it an agent summarises, and the user gets a
/// paraphrase of onboarding written by something that has never used base.
pub fn markdown() -> String {
    let (on, off) = platform_lines();
    let mut s = String::new();

    // A blockquote, not an HTML comment: renderers strip comments, and an agent
    // that cannot see the instruction is an agent that will paraphrase.
    s.push_str("> To the installing agent: show the block below to the user exactly as\n");
    s.push_str("> written, then stop. Do not summarise it, shorten it, or start configuring.\n\n");

    s.push_str("base is installed.\n\n");
    s.push_str("It keeps a private record of how you work on this machine. Your decisions, your ");
    s.push_str("corrections, the shape of your projects. It hands the relevant part back to your ");
    s.push_str("AI tools when it matters. Nothing leaves this machine.\n\n");

    s.push_str("### What you will notice first\n\n");
    s.push_str("Your next AI session opens with a short block from base. Nothing else changes.\n\n");

    s.push_str("### What to do now\n\n");
    s.push_str("Open a new session and work normally. base learns from what you decide and what ");
    s.push_str("you correct. There is nothing to configure first.\n\n");

    s.push_str("### When you want more\n\n```\n");
    let width = COMMANDS.iter().map(|(c, _)| c.len()).max().unwrap_or(0);
    for (cmd, purpose) in COMMANDS {
        s.push_str(&format!("{cmd:<width$}   {purpose}\n"));
    }
    s.push_str("```\n\n");

    s.push_str(&on);
    if let Some(off) = off {
        s.push(' ');
        s.push_str(&off);
        s.push(' ');
        s.push_str(INSTALL_DOCS);
    }
    s.push_str("\n\n");

    s.push_str(&format!("Read: {DOCS}\n\n"));
    s.push_str(&format!("Ask: {TRACKER}\n\n"));
    s.push_str(PRODUCT_LINE);
    s.push('\n');
    s
}

/// Everything the install and scaffold banners used to print at people before
/// they had done anything: relay, the CARL migration, the star commands, the
/// workspace-specific next steps. None of it belongs in a first-run message --
/// it is what you read once you have a reason to.
///
/// A const rather than a built String, so clap can carry it as the
/// `getting-started` long help and `base help getting-started` is the address
/// the first-run message can honestly hand out.
pub const GETTING_STARTED: &str = "\
What to read once base is installed.

Working in a workspace

  base scaffold                    set a folder up as a workspace
  base domain create               add a trigger for this workspace
  base rule add --domain X --text  give that trigger something to say

  Workspace triggers live in .base/domains.toml. The rules themselves live in
  the graph, so they are searchable rather than pasted.

Running more than one session

  Relay is on. Every session gets a codename and a wake contract, and all of it
  stays in ~/.base-gbl/.base/relay-inbox/ on this machine.

  base config set relay.enabled false      turn it off
  base config set relay.wake_nudge false   keep pings, drop the arming block

Star commands

  Short operator commands you type as *name. base install offers a starter pack;
  base commands list shows what you have.

Coming from CARL

  base install --carl ~/.carl/carl.json    bring old decisions across

The rest: https://docs.basemode.ai";


#[cfg(test)]
mod tests {
    use super::*;

    /// Prose sentences only: the command table and any line carrying a URL are
    /// not prose, and counting the words in a URL measures nothing.
    fn sentences(body: &str) -> Vec<String> {
        body.lines()
            .filter(|l| !l.contains("http") && !l.trim_start().starts_with("base "))
            .collect::<Vec<_>>()
            .join(" ")
            .split('.')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    #[test]
    fn no_exclamation_marks() {
        assert!(!text().contains('!'), "first-run text shouts");
        assert!(!markdown().contains('!'), "first-run markdown shouts");
    }

    #[test]
    fn no_banned_words() {
        for banned in ["leverage", "seamless", "powerful", "unlock"] {
            let t = text().to_lowercase();
            assert!(!t.contains(banned), "first-run text says {banned:?}");
        }
    }

    #[test]
    fn every_sentence_is_under_twenty_words() {
        for s in sentences(&text()) {
            let n = s.split_whitespace().count();
            assert!(n < 20, "{n}-word sentence: {s:?}");
        }
    }

    #[test]
    fn at_most_five_things_to_do() {
        assert!(COMMANDS.len() <= 5, "{} commands is a tutorial", COMMANDS.len());
    }

    #[test]
    fn only_the_docs_site_and_the_tracker() {
        let allowed = [DOCS, INSTALL_DOCS, TRACKER];
        for body in [text(), markdown()] {
            for word in body.split_whitespace() {
                let url = word.trim_end_matches(['.', ',', ')']);
                if url.starts_with("http") {
                    assert!(allowed.contains(&url), "unexpected URL {url:?}");
                }
            }
        }
    }

    #[test]
    fn carries_no_company_brand() {
        for body in [text(), markdown()] {
            let lower = body.to_lowercase();
            for gone in ["chris ai systems", "skool", "chrisai.cv", "chrisai"] {
                assert!(!lower.contains(gone), "first-run message still says {gone:?}");
            }
        }
        assert!(text().contains("Built by Chris Kahler"), "author credit was dropped");
    }

    #[test]
    fn names_every_platform_exactly_once() {
        let body = text();
        for p in PLATFORMS {
            assert_eq!(body.matches(p.name).count(), 1, "{} named twice or not at all", p.name);
        }
    }

    #[test]
    fn joins_names_the_way_english_does() {
        assert_eq!(join_names(&[]), "");
        assert_eq!(join_names(&["a"]), "a");
        assert_eq!(join_names(&["a", "b"]), "a and b");
        assert_eq!(join_names(&["a", "b", "c"]), "a, b and c");
    }
}
