//! Commit 4c: the matcher model and its storage, and the selection seam. No hook serves by matcher yet (that is
//! 4d), so every test here drives the model, the store or `select` directly, and all of them are green at 4c.
//! The rulings they follow are in the lane brief, section COMMIT 4, part 4.

use std::collections::HashMap;
use std::path::Path;

use base::config::{BaseConfig, NamespaceConfig, RulesConfig};
use base::crud;
use base::domain::rules::{self, Converted, Event, Matcher, SelectContext, ServedRule, Why};
use base::domain::session::{Bracket, SessionState};
use base::domain::DomainDef;

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

fn workspace(root: &Path, toml: &str) {
    std::fs::create_dir_all(root.join(".base")).unwrap();
    std::fs::write(root.join(".base").join("graph.nq"), "").unwrap();
    std::fs::write(root.join(".base").join("domains.toml"), toml).unwrap();
}

const TOML: &str = r#"[[domain]]
name = "base-config"
prompt_keywords = ["base config"]

[[domain.rules]]
text = "Relay pings to chris use line breaks."
rationale = "unreadable on a phone"

  [[domain.rules.match]]
  kind = "action"
  command = "base relay ping --to chris"

  [[domain.rules.match]]
  kind = "topic"
  words = ["ping chris", "relay ping"]

[[domain.rules]]
text = "An unconverted rule keeps its domain path."
"#;

#[derive(serde::Deserialize, serde::Serialize)]
struct File {
    domain: Vec<DomainDef>,
}

fn converted(domain: &str, text: &str, matchers: Vec<Matcher>) -> Converted {
    let rendered = text.to_string();
    Converted {
        rule: ServedRule {
            id: rules::rule_id(domain, text),
            domain: domain.into(),
            text: text.into(),
            rationale: None,
            content_hash: rules::content_hash(&rendered),
            rendered,
            iri: None,
        },
        matchers,
    }
}

fn cx<'a>(bracket: Bracket, now: u64, keywords: &'a HashMap<String, Vec<String>>, cfg: &'a RulesConfig) -> SelectContext<'a> {
    SelectContext { bracket, now, home: None, keywords, rules: cfg }
}

fn session() -> SessionState {
    let mut s = SessionState::default();
    s.set_active(Some("sid-4c"));
    s
}

// ── storage ──────────────────────────────────────────────────────────────────

#[test]
fn the_match_table_parses_and_a_rule_without_one_writes_no_match_key() {
    let f: File = toml::from_str(TOML).unwrap();
    let rs = &f.domain[0].rules;
    assert_eq!(rs.len(), 2, "positive control: both rules parsed");
    assert_eq!(rs[0].matchers().len(), 2, "two [[domain.rules.match]] tables: {:?}", rs[0].matchers());
    assert!(rs[1].matchers().is_empty(), "the second rule carries none");

    let again: File = toml::from_str(&toml::to_string(&f).unwrap()).unwrap();
    assert_eq!(again.domain[0].rules, f.domain[0].rules, "a round trip keeps every rule and every matcher");

    let plain: File = toml::from_str("[[domain]]\nname = \"d\"\n\n[[domain.rules]]\ntext = \"t\"\nrationale = \"r\"\n").unwrap();
    let written = toml::to_string(&plain).unwrap();
    assert!(written.contains("text = \"t\""), "control: the rule was written: {written}");
    assert!(!written.contains("match"), "a rule with no matchers writes no match key (J9.5): {written}");
}

#[test]
fn sync_writes_the_matchers_and_the_graph_reads_back_what_the_toml_says() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root, TOML);
        let config = BaseConfig { namespace: ns(), ..Default::default() };
        base::domain::sync::sync_domains_to_graph(&config, root, None).unwrap();
        let domains = base::domain::load_domains(root);
        let domain = domains.iter().find(|d| d.name == "base-config").expect("control: the domain loaded");
        let store = base::store::load_merged(root);
        assert!(store.is_some(), "control: the synced store loads");

        let from_graph = rules::rules_with_matchers(store.as_ref(), &config, &domains);
        let from_toml = rules::rules_with_matchers(None, &config, &domains);
        assert_eq!(from_graph.len(), 1, "one converted rule, and the unconverted one is not converted: {from_graph:?}");
        assert!(from_graph[0].rule.iri.is_some(), "this copy really came from the graph");
        assert_eq!(from_graph[0].matchers, from_toml[0].matchers, "the graph and the toml read the same matchers");

        let all = rules::rules_for_domain(store.as_ref(), &config, domain);
        assert_eq!(all.len(), 2, "the domain path still carries both rules, converted or not (K4): {all:?}");
    });
}

#[test]
fn rule_add_writes_matchers_and_two_separate_reads_give_one_id() {
    let tmp = tempfile::tempdir().unwrap();
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        workspace(root, "[[domain]]\nname = \"probe\"\n");
        let config = BaseConfig { namespace: ns(), ..Default::default() };
        let m = rules::matchers_from_flags(&[], &[], &["mcp__slack__send".into()], &[], None).unwrap();
        crud::rule::add_with_matchers(root, &ns(), "probe", "SHARE, NEVER SOLICIT", None, None, &m).unwrap();

        let domains = base::domain::load_domains(root);
        let first = rules::rules_with_matchers(base::store::load_merged(root).as_ref(), &config, &domains);
        let second = rules::rules_with_matchers(base::store::load_merged(root).as_ref(), &config, &domains);
        assert_eq!(first.len(), 1, "control: the CLI rule reads back as converted: {first:?}");
        assert_eq!(first[0].matchers, m);
        assert_eq!(first[0].rule.id, second[0].rule.id, "two separate reads, one id (A6)");
        assert_eq!(first[0].rule.id, rules::rule_id("probe", "SHARE, NEVER SOLICIT"));

        let by_text = crud::rule::matchers_by_text(root, &ns(), "probe");
        assert_eq!(by_text.get("SHARE, NEVER SOLICIT"), Some(&m), "rule list's reader sees them too: {by_text:?}");
    });
}

#[test]
fn every_tier_label_round_trips() {
    // petrel's F1-b on b76f2b5: the stored tier is read back through from_label.
    for tier in [Bracket::Fresh, Bracket::Moderate, Bracket::Depleted, Bracket::Critical] {
        assert_eq!(Bracket::from_label(&tier.to_string()), Some(tier), "{tier}");
    }
    assert_eq!(Bracket::from_label("fresh"), None, "labels are exactly what Display writes");
    assert_eq!(Bracket::from_label(""), None);
}

// ── select ───────────────────────────────────────────────────────────────────

#[test]
fn two_matchers_hitting_one_tool_call_show_the_rule_once() {
    let rule = converted(
        "base-config",
        "commands.toml is WSL-owned",
        vec![Matcher::for_place("commands.toml"), Matcher::for_command("base relay ping")],
    );
    let (kw, cfg) = (HashMap::new(), RulesConfig::default());
    let paths = vec!["C:/Users/Chris/.base-gbl/commands.toml".to_string()];
    let ev = Event::PreTool { tool: "PowerShell", paths: &paths, command: Some("base relay ping --to chris") };
    let mut s = session();
    let got = rules::select(std::slice::from_ref(&rule), &ev, &mut s, &cx(Bracket::Fresh, 1_000, &kw, &cfg));
    assert_eq!(got.served.len(), 1, "a place and an action hit at once: shown once (F3)");
    assert_eq!(got.served[0].why, Why::Place("commands.toml".into()));
    let again = rules::select(std::slice::from_ref(&rule), &ev, &mut s, &cx(Bracket::Fresh, 1_010, &kw, &cfg));
    assert!(again.served.is_empty(), "the same place again in the same session: nothing new (F8)");
}

#[test]
fn a_topic_rule_the_cap_cut_is_not_marked_shown() {
    let a = converted("d", "rule a", vec![Matcher::for_topic(vec!["ping chris".into()])]);
    let b = converted("d", "rule b", vec![Matcher::for_topic(vec!["relay ping".into()])]);
    let kw = HashMap::new();
    let cfg = RulesConfig { topic_max: 1, ..RulesConfig::default() };
    let ev = Event::Prompt { text: "ping chris about the relay ping" };
    let mut s = session();
    let first = rules::select(&[a.clone(), b.clone()], &ev, &mut s, &cx(Bracket::Fresh, 1, &kw, &cfg));
    assert_eq!(first.served.len(), 1, "capped at topic_max: {:?}", first.served);
    assert_eq!(first.topic_withheld, vec![("d".to_string(), 1)], "the pointer line's count (F6)");
    let second = rules::select(&[a, b], &ev, &mut s, &cx(Bracket::Fresh, 2, &kw, &cfg));
    assert_eq!(second.served.len(), 1, "the rule the cap cut was never marked shown, so it arrives now");
    assert_ne!(second.served[0].rule.id, first.served[0].rule.id);
}

#[test]
fn an_action_rule_shows_again_only_after_its_throttle() {
    let rule = converted("base-config", "pings use line breaks", vec![Matcher::for_command("base relay ping --to chris")]);
    let (kw, cfg) = (HashMap::new(), RulesConfig::default());
    let ev = Event::PreTool { tool: "Bash", paths: &[], command: Some("base relay ping --from shrike --to chris --msg x") };
    let mut s = session();
    let at = |now: u64, s: &mut SessionState| {
        rules::select(std::slice::from_ref(&rule), &ev, s, &cx(Bracket::Fresh, now, &kw, &cfg)).served.len()
    };
    assert_eq!(at(1_000, &mut s), 1, "first ping: shown");
    assert_eq!(at(1_599, &mut s), 0, "within 10 minutes: not again (F5)");
    assert_eq!(at(1_600, &mut s), 1, "10 minutes later: shown again");
}

#[test]
fn the_minimum_score_is_what_stops_one_rule_text_word() {
    let rule = converted(
        "base-config",
        "Before any base write, resolve and state the tier the working directory lands in",
        vec![Matcher::for_topic(Vec::new())],
    );
    let kw = HashMap::new();
    let ev = Event::Prompt { text: "which tier are we on" };
    let default_cfg = RulesConfig::default();
    let held = rules::select(std::slice::from_ref(&rule), &ev, &mut session(), &cx(Bracket::Fresh, 1, &kw, &default_cfg));
    assert!(held.served.is_empty(), "one shared text word does not fire a rule at the default minimum (A5)");
    // Knock-out: the same case with the minimum at 0 must fire, so the minimum is what held it back.
    let open_cfg = RulesConfig { topic_min_score: 0.0, ..RulesConfig::default() };
    let fired = rules::select(std::slice::from_ref(&rule), &ev, &mut session(), &cx(Bracket::Fresh, 1, &kw, &open_cfg));
    assert_eq!(fired.served.len(), 1, "with the minimum at 0 the one-word case fires");
}

#[test]
fn an_always_rule_shows_on_the_first_prompt_and_again_on_a_tier_change() {
    let rule = converted("base-config", "fork only on Chris's command", vec![Matcher::always()]);
    let (kw, cfg) = (HashMap::new(), RulesConfig::default());
    let ev = Event::Prompt { text: "anything at all" };
    let mut s = session();
    let n = |b: Bracket, s: &mut SessionState| rules::select(std::slice::from_ref(&rule), &ev, s, &cx(b, 1, &kw, &cfg)).served.len();
    assert_eq!(n(Bracket::Fresh, &mut s), 1, "first prompt");
    assert_eq!(n(Bracket::Fresh, &mut s), 0, "same tier: silent");
    assert_eq!(n(Bracket::Moderate, &mut s), 1, "the tier changed: once more (F8, J7.2)");
    assert_eq!(n(Bracket::Moderate, &mut s), 0, "then silent");
}
