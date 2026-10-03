//! BO-14 (K2, D3, F8): rules carry test prompts, and `base rule test` says where the config fires wrong. Every test
//! drives the real binary on a fake home: the global tier under `home/.base-gbl`, the workspace at `ws/.base`.
//!
//! The names are synthetic because this repository is public: Example 4's client keyword is `tony` here, not the real
//! client's name.

use std::path::{Path, PathBuf};
use std::process::Command;

use base::domain::rules::rule_id;

struct Home {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    ws: PathBuf,
}

impl Home {
    fn ws_toml(&self) -> PathBuf {
        self.ws.join(".base").join("domains.toml")
    }

    fn gbl_toml(&self) -> PathBuf {
        self.home.join(".base-gbl").join("domains.toml")
    }

    fn ws_graph(&self) -> PathBuf {
        self.ws.join(".base").join("graph.nq")
    }

    fn gbl_graph(&self) -> PathBuf {
        self.home.join(".base-gbl").join(".base").join("graph.nq")
    }
}

/// A fake home with `ws` as the workspace's domains.toml and `gbl` as the global one (none when empty).
fn home(ws: &str, gbl: &str) -> Home {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let wsd = tmp.path().join("ws");
    std::fs::create_dir_all(home.join(".base-gbl").join(".base")).unwrap();
    std::fs::create_dir_all(wsd.join(".base")).unwrap();
    std::fs::write(wsd.join(".base").join("domains.toml"), ws).unwrap();
    if !gbl.is_empty() {
        std::fs::write(home.join(".base-gbl").join("domains.toml"), gbl).unwrap();
    }
    Home { _tmp: tmp, home, ws: wsd }
}

/// `base <args>` from the workspace: (exit code, stdout, stderr).
fn base(h: &Home, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_base"))
        .args(args)
        .current_dir(&h.ws)
        .env("BASE_HOME", &h.home)
        .env("BASE_NO_AUTO_UPDATE", "1")
        .env("BASE_AST_NO_SPAWN", "1")
        .env_remove("BASE_RELAY_AS")
        .env_remove("BASE_HEADLESS")
        .env_remove("WT_SESSION")
        // `CLAUDECODE=1` (a run inside Claude Code) makes `rule add` require keywords and a test prompt (BO-15).
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .output()
        .expect("the base binary runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// `base <args>` that must exit 0; its stdout.
fn ok(h: &Home, args: &[&str]) -> String {
    let (code, out, err) = base(h, args);
    assert_eq!(code, 0, "base {args:?} exited {code}\n{out}{err}");
    out
}

/// `base.9f2c1a7b`, as `base rule list` prints a rule and `base rule update` takes it.
fn short(domain: &str, text: &str) -> String {
    format!("{domain}.{}", &rule_id(domain, text)[..8])
}

/// The line `base rule test` prints for one test prompt, and the line under it. Since BO-18 the prompt's line ends with
/// its BM25 score (held out of its own rule for a `fires_on` prompt), so it is found by its start.
fn outcome<'a>(out: &'a str, label: &str, prompt: &str) -> (&'a str, Option<&'a str>) {
    let start = format!("  {label:<12}\"{prompt}\"   score ");
    let lines: Vec<&str> = out.lines().collect();
    let at = lines.iter().position(|l| l.starts_with(&start)).unwrap_or_else(|| panic!("no line starting {start:?} in:\n{out}"));
    (lines[at], lines.get(at + 1).copied())
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

const BASE_DOMAINS: &str = r#"[[domain]]
name = "GLOBAL"
mode = "always"
rules = ["Answer first."]

[[domain]]
name = "base"
mode = "triggered"
prompt_keywords = []
rules = []
"#;

const MEASURE: &str = "Measure twice, cut once: read the code path end to end before the first edit.";
const F8_PROMPT: &str = "we're having some issues with the user prompt submit being cut off at a high rate";
const F8_PROMPT_2: &str = "how is the pre-tool hook injection doing";
const QUIET: &str = "what reminders do i need";

/// The base domain with Example 1's tests on one rule and two more rules without tests.
fn f8_home() -> Home {
    let h = home(BASE_DOMAINS, "");
    ok(&h, &["rule", "add", "--domain", "base", "--text", MEASURE, "--fires-on", F8_PROMPT, "--fires-on", F8_PROMPT_2, "--quiet-on", QUIET]);
    ok(&h, &["rule", "add", "--domain", "base", "--text", "Every change is a branch and a PR."]);
    ok(&h, &["rule", "add", "--domain", "base", "--text", "Regenerate the help docs when the CLI changes."]);
    h
}

#[test]
fn rule_test_reports_miss() {
    // Example 2: the base domain has rules and no prompt keywords, so a prompt about its hooks serves none of them.
    let h = f8_home();
    let id = short("base", MEASURE);
    let listing = ok(&h, &["rule", "list", "--domain", "base"]);
    assert!(listing.contains(&format!("[{id}] {MEASURE}")), "rule list prints the id: {listing}");
    assert!(listing.contains("tests: 2 fires-on · 1 quiet-on"), "{listing}");

    let (code, out, err) = base(&h, &["rule", "test", "--domain", "base"]);
    assert_eq!(code, 1, "a miss exits 1: {out}{err}");
    for want in [
        format!("{id}   \"Measure twice"),
        // BO-18: `[match] min_score` is unset by default, so the tests are judged by keyword only, and one line says so.
        "scores: BM25, each fires-on prompt held out of its own rule; [match] min_score is unset, so the tests are judged by keyword only\n".to_string(),
        "1 rule tested, 2 misses, 0 false fires · 2 rules in base have no tests\n".to_string(),
    ] {
        assert!(out.contains(&want), "missing {want:?} in:\n{out}");
    }
    let why = "              matched: GLOBAL(always) · base: has no prompt keywords";
    assert_eq!(outcome(&out, "MISS", F8_PROMPT).1, Some(why), "{out}");
    assert_eq!(outcome(&out, "MISS", F8_PROMPT_2).1, Some(why), "{out}");
    outcome(&out, "ok (quiet)", QUIET);

    // Example 3: the keywords F8 adds, one at a time. Each add-trigger prints the domain's result (K2, "on every
    // config change"), so the first shows one miss left and the second shows none.
    let one = ok(&h, &["domain", "add-trigger", "--domain", "base", "--keyword", "user prompt submit"]);
    assert!(one.contains("rule tests, base: 1 rule tested, 1 miss, 0 false fires (FAILS) · base rule test --domain base"), "{one}");
    let two = ok(&h, &["domain", "add-trigger", "--domain", "base", "--keyword", "pre-tool"]);
    assert!(two.contains("rule tests, base: 1 rule tested, 0 misses, 0 false fires · base rule test --domain base"), "{two}");
    let after = ok(&h, &["rule", "test", "--domain", "base"]);
    assert!(after.contains(&format!("{id}   ok 2 fires · ok 1 quiet")), "{after}");
    assert!(after.contains("1 rule tested, 0 misses, 0 false fires · 2 rules in base have no tests"), "{after}");
}

#[test]
fn rule_test_reports_false_fire() {
    // Example 4, with synthetic names: a client keyword that also names a celebrity fires on the wrong prompt, and the
    // signal is to narrow the keyword.
    let h = home(
        r#"[[domain]]
name = "client-work"
prompt_keywords = ["tony"]
rules = []
"#,
        "",
    );
    let text = "Client updates lead with what the owner sees.";
    ok(&h, &["rule", "add", "--domain", "client-work", "--text", text, "--fires-on", "draft tony's morning update", "--quiet-on", "watch the Tony Hawk documentary tonight"]);
    let (code, out, _) = base(&h, &["rule", "test"]);
    assert_eq!(code, 1, "a false fire exits 1: {out}");
    assert_eq!(
        outcome(&out, "FALSE FIRE", "watch the Tony Hawk documentary tonight").1,
        Some("              matched: client-work(keyword: tony)"),
        "{out}"
    );
    assert!(out.contains("1 rule tested, 0 misses, 1 false fire\n"), "{out}");

    // Narrowed, it passes.
    ok(&h, &["domain", "remove-trigger", "--domain", "client-work", "--keyword", "tony"]);
    let narrowed = ok(&h, &["domain", "add-trigger", "--domain", "client-work", "--keyword", "tony's"]);
    assert!(narrowed.contains("rule tests, client-work: 1 rule tested, 0 misses, 0 false fires"), "{narrowed}");
    ok(&h, &["rule", "test"]);
}

#[test]
fn rule_test_exit_codes() {
    // Nothing has tests: nothing can fail.
    let h = home(BASE_DOMAINS, "");
    let out = ok(&h, &["rule", "test"]);
    assert!(out.starts_with("0 rules tested, 0 misses, 0 false fires\n"), "{out}");

    let h = f8_home();
    let id = short("base", MEASURE);
    assert_eq!(base(&h, &["rule", "test"]).0, 1, "a miss");
    assert_eq!(base(&h, &["rule", "test", "--rule", &id]).0, 1, "a miss, one rule");
    ok(&h, &["domain", "add-trigger", "--domain", "base", "--keyword", "user prompt submit"]);
    ok(&h, &["domain", "add-trigger", "--domain", "base", "--keyword", "pre-tool"]);
    ok(&h, &["rule", "test", "--rule", &id]);
    ok(&h, &["rule", "test", "--rule", &id[5..]]);
    // --domain narrows a bare id, and a <domain>.<id> of another domain is refused (code review, finding 6).
    ok(&h, &["rule", "test", "--domain", "base", "--rule", &id[5..]]);
    assert_eq!(base(&h, &["rule", "test", "--domain", "GLOBAL", "--rule", &id[5..]]).0, 2, "no GLOBAL rule has that id");
    assert_eq!(base(&h, &["rule", "test", "--domain", "GLOBAL", "--rule", &id]).0, 2, "two domains named");
    ok(&h, &["rule", "test"]);
    // A keyword that also fires on the quiet prompt: a false fire.
    ok(&h, &["domain", "add-trigger", "--domain", "base", "--keyword", "reminders"]);
    assert_eq!(base(&h, &["rule", "test"]).0, 1, "a false fire");

    // A run that cannot start exits 2, so a script never reads it as a failed test.
    for args in [
        vec!["rule", "test", "--domain", "nosuch"],
        vec!["rule", "test", "--rule", "base.ffffffff"],
        vec!["rule", "test", "--rule", "not-an-id"],
    ] {
        let (code, out, err) = base(&h, &args);
        assert_eq!(code, 2, "{args:?}: {out}{err}");
        assert!(err.starts_with("Error: "), "{err}");
    }
}

#[test]
fn rule_test_counts_untested_rules() {
    // K2d. GLOBAL's two rules come from domains.toml, base's three from the graph; `quiet` has none and is not listed.
    let h = home(
        r#"[[domain]]
name = "GLOBAL"
mode = "always"
rules = ["Answer first.", "Name the file read."]

[[domain]]
name = "base"
prompt_keywords = ["base"]
rules = []

[[domain]]
name = "quiet"
prompt_keywords = ["quiet"]
rules = []
"#,
        "",
    );
    ok(&h, &["rule", "add", "--domain", "base", "--text", MEASURE, "--fires-on", "base hooks"]);
    ok(&h, &["rule", "add", "--domain", "base", "--text", "Every change is a branch and a PR."]);
    ok(&h, &["rule", "add", "--domain", "base", "--text", "Regenerate the help docs."]);
    let all = ok(&h, &["rule", "test"]);
    assert!(all.contains("1 rule tested, 0 misses, 0 false fires\nrules with no tests, per domain (untested of total):\n"), "{all}");
    assert!(all.contains("\n  GLOBAL  2 of 2\n  base    2 of 3\n"), "{all}");
    assert!(!all.contains("quiet"), "a domain with no rules is not listed: {all}");

    let one = ok(&h, &["rule", "test", "--domain", "base"]);
    assert!(one.ends_with("1 rule tested, 0 misses, 0 false fires · 2 rules in base have no tests\n"), "{one}");
    let global = ok(&h, &["rule", "test", "--domain", "GLOBAL"]);
    assert!(global.ends_with("0 rules tested, 0 misses, 0 false fires · 2 rules in GLOBAL have no tests\n"), "{global}");
    let none = ok(&h, &["rule", "test", "--domain", "quiet"]);
    assert!(none.ends_with(" · 0 rules in quiet have no tests\n"), "{none}");

    // Every rule tested.
    for text in ["Every change is a branch and a PR.", "Regenerate the help docs."] {
        ok(&h, &["rule", "update", &short("base", text), "--fires-on", "base work"]);
    }
    for text in ["Answer first.", "Name the file read."] {
        ok(&h, &["rule", "update", &short("GLOBAL", text), "--fires-on", "anything at all"]);
    }
    let full = ok(&h, &["rule", "test"]);
    assert!(full.ends_with("5 rules tested, 0 misses, 0 false fires\nevery rule has tests\n"), "{full}");
}

const PROBE_DOMAINS: &str = r#"[[domain]]
name = "probe"
prompt_keywords = ["probe"]
rules = ["first rule", "second rule", "third rule"]

[[domain]]
name = "other"
prompt_keywords = ["other"]
rules = ["other rule"]
"#;

#[test]
fn rule_tests_stored_with_rule() {
    let h = home(PROBE_DOMAINS, "");
    // Into base's own written form first, so a byte comparison afterwards compares like with like. A file with no tests
    // round-trips with no new keys (lynx, G0 verdict).
    ok(&h, &["domain", "add-trigger", "--domain", "other", "--keyword", "other2"]);
    let before = read(&h.ws_toml());
    assert!(!before.contains("fires_on") && !before.contains("quiet_on"), "{before}");

    // A domains.toml rule: its tests go on its entry, and only that entry changes (lynx, G0 verdict).
    ok(&h, &["rule", "update", &short("probe", "second rule"), "--fires-on", "a \"quoted\" probe prompt", "--quiet-on", "nothing here"]);
    let after = read(&h.ws_toml());
    let changed: Vec<(&str, &str)> = before.lines().zip(after.lines()).filter(|(a, b)| a != b).collect();
    assert_eq!(before.lines().count(), after.lines().count(), "{after}");
    assert_eq!(
        changed,
        vec![("    \"second rule\",", "    { text = \"second rule\", fires_on = ['a \"quoted\" probe prompt'], quiet_on = [\"nothing here\"] },")],
        "only that entry changes:\n{after}"
    );

    // A CLI rule: its tests go on its graph record, in the same write as the rule.
    ok(&h, &["rule", "add", "--domain", "probe", "--text", "a cli rule", "--fires-on", "probe the cli", "--quiet-on", "unrelated words"]);
    assert_eq!(read(&h.ws_toml()), after, "a CLI rule's tests never touch domains.toml");

    // Survive two syncs and new processes: the same result, the same listing, and the synced copy carries the tests.
    let first = ok(&h, &["rule", "test"]);
    assert!(first.contains(&format!("{}   ok 1 fire · ok 1 quiet", short("probe", "second rule"))), "{first}");
    assert!(first.contains(&format!("{}   ok 1 fire · ok 1 quiet", short("probe", "a cli rule"))), "{first}");
    for _ in 0..2 {
        ok(&h, &["domain", "sync"]);
        assert_eq!(ok(&h, &["rule", "test"]), first);
    }
    let listing = ok(&h, &["rule", "list", "--domain", "probe"]);
    assert_eq!(listing.matches("tests: 1 fires-on · 1 quiet-on").count(), 2, "{listing}");
    // The CLI rule's literals are in the graph once; the domains.toml rule's are in its file only, never on its synced
    // copy, which can go stale.
    let graph = read(&h.ws_graph());
    assert_eq!(graph.matches("firesOn").count(), 1, "the CLI rule's, once, after two syncs:\n{graph}");
    assert!(!graph.contains("quoted"), "a synced copy carries no tests:\n{graph}");

    // Cleared, the entry is a plain string again and the file is what it was.
    ok(&h, &["rule", "update", &short("probe", "second rule"), "--clear-tests"]);
    assert_eq!(read(&h.ws_toml()), before);
    let after_clear = ok(&h, &["rule", "test"]);
    assert!(after_clear.contains("1 rule tested, 0 misses, 0 false fires"), "{after_clear}");
}

#[test]
fn rule_tests_cleared_in_one_tier_stay_cleared() {
    // The workspace graph holds a synced copy of every global-tier rule, and is re-synced only when the WORKSPACE file
    // changes. Clearing a global rule's tests rewrites the global file only, so a reader that trusted the synced copy
    // would keep running the cleared prompts (code review, finding 1).
    let h = home(
        "[[domain]]\nname = \"local\"\nprompt_keywords = [\"local\"]\nrules = []\n",
        "[[domain]]\nname = \"gdom\"\nprompt_keywords = [\"gdom\"]\nrules = [{ text = \"a global rule\", fires_on = [\"gdom please\"] }]\n",
    );
    let first = ok(&h, &["rule", "test"]);
    assert!(first.contains("1 rule tested, 0 misses, 0 false fires"), "{first}");
    assert!(read(&h.ws_graph()).contains("a global rule"), "control: the workspace graph holds the synced copy");
    ok(&h, &["rule", "-g", "update", &short("gdom", "a global rule"), "--clear-tests"]);
    let after = ok(&h, &["rule", "test"]);
    assert!(after.starts_with("0 rules tested, 0 misses, 0 false fires"), "the cleared prompts stay cleared:\n{after}");
}

#[test]
fn rule_update_writes_one_tier() {
    // The same rule in both tiers' domains.toml is one rule, and `rule update` writes one tier: the workspace's, or the
    // global one's with -g. One tier's prompts never land in the other, and the cap counts the tier written (code
    // review, findings 3 and 4).
    let both = "[[domain]]\nname = \"twin\"\nprompt_keywords = [\"twin\"]\nrules = [\"shared rule\"]\n";
    let h = home(both, both);
    let id = short("twin", "shared rule");
    ok(&h, &["rule", "update", &id, "--fires-on", "twin one", "--fires-on", "twin two", "--fires-on", "twin three"]);
    assert!(read(&h.ws_toml()).contains("twin three"), "{}", read(&h.ws_toml()));
    assert_eq!(read(&h.gbl_toml()), both, "without -g the global file is untouched");
    let out = ok(&h, &["rule", "-g", "update", &id, "--fires-on", "twin global"]);
    assert!(out.contains("(in domains.toml (global tier))"), "{out}");
    let gbl = read(&h.gbl_toml());
    assert!(gbl.contains("twin global") && !gbl.contains("twin one"), "{gbl}");
    assert!(!read(&h.ws_toml()).contains("twin global"), "with -g the workspace file is untouched");

    // -g on a rule only the workspace holds.
    let ws_only = home("[[domain]]\nname = \"solo\"\nrules = [\"only here\"]\n", "");
    let (code, _, err) = base(&ws_only, &["rule", "-g", "update", &short("solo", "only here"), "--fires-on", "x"]);
    assert_eq!(code, 1);
    assert!(err.contains("is not in the global tier, only in the workspace; run it without -g"), "{err}");
}

#[test]
fn store_tests_reports_nothing_stored_when_the_rule_changed_after_it_was_read() {
    // `find` reads, then `store_tests` writes; a file edited in between holds no such rule, and the caller must hear
    // that nothing was stored rather than print success (code review, finding 7).
    let h = home(PROBE_DOMAINS, "");
    let ns = base::config::NamespaceConfig::default();
    base::home::with_thread_home(&h.home, || {
        let id = rule_id("probe", "second rule");
        let found = base::crud::rule::find(&h.ws, &ns, Some("probe"), &id[..8]).unwrap();
        assert_eq!(found.len(), 1);
        std::fs::write(h.ws_toml(), PROBE_DOMAINS.replace("second rule", "second rule, reworded")).unwrap();
        let homes: Vec<&base::crud::rule::TestHome> = found[0].homes.iter().map(|(home, _)| home).collect();
        let tests = base::domain::rules::RuleTests { fires_on: vec!["probe it".into()], quiet_on: Vec::new() };
        let wrote = base::crud::rule::store_tests(&ns, &found[0], &homes, &tests).unwrap();
        assert!(wrote.is_empty(), "{wrote:?}");
        assert!(!read(&h.ws_toml()).contains("probe it"));
    });
}

#[test]
fn rule_update_writes_where_the_rule_lives() {
    let h = home(PROBE_DOMAINS, "[[domain]]\nname = \"gbl\"\nprompt_keywords = [\"gbl\"]\nrules = []\n");
    ok(&h, &["rule", "--global", "add", "--domain", "gbl", "--text", "a global cli rule"]);
    ok(&h, &["domain", "sync"]);
    let gbl_toml = read(&h.gbl_toml());

    // The global CLI rule: its tests land in the global graph, and no domains.toml changes.
    let ws_toml = read(&h.ws_toml());
    let out = ok(&h, &["rule", "update", &short("gbl", "a global cli rule"), "--fires-on", "gbl please"]);
    assert!(out.starts_with(&format!("Rule {} tests: 1 fires-on · 0 quiet-on (in graph (global tier))\n", short("gbl", "a global cli rule"))), "{out}");
    assert!(read(&h.gbl_graph()).contains("gbl please"), "global graph");
    assert!(!read(&h.ws_graph()).contains("gbl please"), "the workspace graph does not hold it");
    assert_eq!((read(&h.ws_toml()), read(&h.gbl_toml())), (ws_toml, gbl_toml.clone()));

    // The workspace domains.toml rule: its entry changes, the global file does not, and no graph holds it as its own.
    let out = ok(&h, &["rule", "update", &short("probe", "third rule"), "--quiet-on", "never this"]);
    assert!(out.contains("(in domains.toml (workspace tier))"), "{out}");
    assert!(read(&h.ws_toml()).contains("{ text = \"third rule\", quiet_on = [\"never this\"] }"), "{}", read(&h.ws_toml()));
    assert_eq!(read(&h.gbl_toml()), gbl_toml);
    assert!(!read(&h.gbl_graph()).contains("never this"), "the global graph does not hold it");
}

#[test]
fn rule_update_refusals() {
    let h = home(PROBE_DOMAINS, "");
    let id = short("probe", "first rule");
    ok(&h, &["rule", "update", &id, "--fires-on", "one", "--fires-on", "two", "--fires-on", "three", "--quiet-on", "q1", "--quiet-on", "q2"]);
    let file = read(&h.ws_toml());
    let refused = |args: &[&str], want: &str| {
        let (code, out, err) = base(&h, args);
        assert_eq!(code, 1, "{args:?}: {out}{err}");
        assert!(err.contains(want), "{args:?}: want {want:?} in {err}");
        assert_eq!(read(&h.ws_toml()), file, "{args:?} wrote");
    };
    refused(&["rule", "update", &id, "--fires-on", "four"], "would hold 4 --fires-on prompts; a rule holds at most 3");
    refused(&["rule", "update", &id, "--quiet-on", "q3"], "a rule holds at most 2");
    refused(&["rule", "update", &id], "give --fires-on, --quiet-on or --clear-tests");
    refused(&["rule", "update", "probe.ffffffff", "--fires-on", "x"], "no rule 'probe.ffffffff'");
    refused(&["rule", "update", "probe.xyz", "--fires-on", "x"], "is not a rule id");
    // A prompt in both lists can never pass (code review, finding 9).
    refused(&["rule", "update", &id, "--clear-tests", "--fires-on", "both", "--quiet-on", "both"], "cannot be both a --fires-on and a --quiet-on prompt");
    // A repeat is not a new prompt: the cap counts what would be stored.
    ok(&h, &["rule", "update", &id, "--fires-on", "one"]);

    // On rule add too, and then the rule is not written at all.
    let (code, _, err) = base(&h, &["rule", "add", "--domain", "probe", "--text", "capped", "--quiet-on", "a", "--quiet-on", "b", "--quiet-on", "c"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("at most 2"), "{err}");
    assert!(!ok(&h, &["rule", "list", "--domain", "probe"]).contains("capped"));

    // Two rules whose ids start alike: a prefix that fits both is refused, naming both.
    let mut seen: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let (a, b) = (0..2000)
        .map(|i| format!("twin {i}"))
        .find_map(|t| {
            let prefix = rule_id("twins", &t)[..4].to_string();
            match seen.get(&prefix) {
                Some(first) => Some((first.clone(), t)),
                None => {
                    seen.insert(prefix, t);
                    None
                }
            }
        })
        .expect("two of 2,000 ids share their first 4 hex characters");
    let (a, b) = (a.as_str(), b.as_str());
    let twins = home(&format!("[[domain]]\nname = \"twins\"\nrules = [\"{a}\", \"{b}\"]\n"), "");
    let prefix = format!("twins.{}", &rule_id("twins", a)[..4]);
    let (code, _, err) = base(&twins, &["rule", "update", &prefix, "--fires-on", "x"]);
    assert_eq!(code, 1);
    assert!(err.contains("fits 2 rules") && err.contains(&short("twins", a)) && err.contains(&short("twins", b)), "{err}");

    // A synced copy whose domains.toml line is gone has nowhere to keep tests.
    let stale = home(PROBE_DOMAINS, "");
    ok(&stale, &["domain", "sync"]);
    std::fs::write(stale.ws_toml(), PROBE_DOMAINS.replace("\"third rule\"", "\"a new third\"")).unwrap();
    let (code, _, err) = base(&stale, &["rule", "update", &short("probe", "third rule"), "--fires-on", "x"]);
    assert_eq!(code, 1);
    assert!(err.contains("is a synced copy of a domains.toml line that is no longer there, so it has nowhere to keep tests"), "{err}");
}

#[test]
fn rule_test_rule_with_matchers_follows_select() {
    // A rule with matchers of its own is served on its own score, as `select` serves it, never through its domain's
    // keywords. The plain rule beside it is served through the domain, so the same quiet prompt false-fires it.
    let h = home("[[domain]]\nname = \"relay\"\nprompt_keywords = [\"relay\"]\nrules = []\n", "");
    let topic = "Pings to the operator use line breaks.";
    let plain = "Check the relay board before pinging.";
    ok(&h, &["rule", "add", "--domain", "relay", "--text", topic, "--words", "ping chris", "--fires-on", "please ping chris when done", "--fires-on", "the relay is slow", "--quiet-on", "check the relay board"]);
    ok(&h, &["rule", "add", "--domain", "relay", "--text", plain, "--quiet-on", "check the relay board"]);
    let (code, out, _) = base(&h, &["rule", "test"]);
    assert_eq!(code, 1, "{out}");
    let block = |text: &str| out.split(&short("relay", text)).nth(1).unwrap_or_default().split("\nrelay.").next().unwrap_or_default().to_string();
    let t = block(topic);
    assert!(t.contains("  ok (fires)  \"please ping chris when done\""), "{out}");
    assert_eq!(outcome(&t, "MISS", "the relay is slow").1, Some("              topic score 0.50 under topic_min_score 0.75 (words: relay)"), "{out}");
    assert!(t.contains("  ok (quiet)  \"check the relay board\""), "the domain keyword alone does not serve it:\n{out}");
    let p = block(plain);
    assert_eq!(outcome(&p, "FALSE FIRE", "check the relay board").1, Some("              matched: relay(keyword: relay)"), "{out}");
}

#[test]
fn config_change_line_only_when_the_domain_has_tests() {
    let h = home(BASE_DOMAINS, "");
    let out = ok(&h, &["domain", "add-trigger", "--domain", "base", "--keyword", "hooks"]);
    assert_eq!(out, "Trigger added to domain 'base' (workspace tier)\n", "no tests, no new line");
    let out = ok(&h, &["rule", "add", "--domain", "base", "--text", "plain"]);
    assert_eq!(out.lines().count(), 1, "{out}");
    ok(&h, &["rule", "update", &short("base", "plain"), "--fires-on", "the hooks are slow"]);
    let out = ok(&h, &["domain", "remove-trigger", "--domain", "base", "--keyword", "hooks"]);
    assert!(out.ends_with("rule tests, base: 1 rule tested, 1 miss, 0 false fires (FAILS) · base rule test --domain base\n"), "{out}");
}

#[test]
fn rule_test_fixture_passes_and_its_control_fails() {
    // The rule set CI runs (K2e, scripts/rule-test-fixture.sh), run here too so both platforms' `cargo test` hold it.
    let fixture = read(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("rule-tests").join("domains.toml"));
    let h = home(&fixture, "");
    let out = ok(&h, &["rule", "test"]);
    let summary = out.lines().find(|l| l.contains(" rules tested, ")).unwrap_or_default();
    assert!(summary.ends_with(" rules tested, 0 misses, 0 false fires"), "{out}");
    let tested: usize = summary.split(' ').next().and_then(|n| n.parse().ok()).unwrap_or(0);
    assert!(tested >= 6, "the fixture tests every part of prompt matching: {out}");

    // The control: one rule whose fires_on cannot match. The same run must fail.
    let control = home(&format!("{fixture}\n[[domain]]\nname = \"control\"\nprompt_keywords = [\"zzz-never\"]\nrules = [{{ text = \"control\", fires_on = [\"nothing matches this\"] }}]\n"), "");
    let (code, out, _) = base(&control, &["rule", "test"]);
    assert_eq!(code, 1, "{out}");
    assert_eq!(
        outcome(&out, "MISS", "nothing matches this").1,
        Some("              matched: always-on(always) · control: no keyword matched"),
        "{out}"
    );
}
