//! F29 — the prompt hook injects what THIS session and THIS prompt call for, nothing
//! standing (fork `base-injection-scope`). Every test drives the real hook handlers on a
//! fake home shaped like the operator's: home is the workspace root, the global tier
//! carries an always-on GLOBAL domain and path-triggered domains, and the workspace graph
//! holds registered projects whose paths sit under those triggers. Red on 0.14.0, where
//! path triggers read every path ever active on the store and matched by substring.

use std::path::Path;

use base::config::{BaseConfig, NamespaceConfig};
use base::crud;
use base::domain::session::SessionState;
use base::hook::{pre_tool_use, user_prompt_submit};

const GLOBAL_DOMAINS: &str = r#"
[[domain]]
name = "GLOBAL"
mode = "always"
rules = ["Never lie"]

[[domain]]
name = "base-config"
mode = "triggered"
prompt_keywords = ["base"]
paths = [".base-gbl"]
rules = ["The base config rule"]

[[domain]]
name = "meet-caddy"
mode = "triggered"
prompt_keywords = ["caddy"]
paths = ["Documents/Meet Caddy"]
rules = ["Never say a floor out loud"]

[[domain]]
name = "vintrix"
mode = "triggered"
prompt_keywords = ["vintrix"]
paths = ["Documents"]
rules = ["Twelve thousand a month and ten percent"]

[[domain]]
name = "old-docs"
mode = "triggered"
paths = ["Documents-old"]
rules = ["The old docs rule"]
"#;

/// The confidential shape: an always-on domain and a narrowly-triggered one, both marked
/// never-auto-inject. The trigger covers one project, so only the flag keeps it out.
const CONFIDENTIAL_DOMAINS: &str = r#"
[[domain]]
name = "secret"
mode = "always"
auto_inject = false
rules = ["Never say a floor out loud"]

[[domain]]
name = "terms"
mode = "triggered"
auto_inject = false
prompt_keywords = ["terms"]
paths = ["genai/vp-operators"]
rules = ["Twelve thousand a month and ten percent"]
"#;

const QUIET_DOMAIN: &str = r#"
[[domain]]
name = "quiet"
mode = "always"
exclude = ["haiku"]
rules = ["The quiet rule"]
"#;

/// A fake home that is also the workspace root, the operator's shape. Global tier under
/// `.base-gbl/`, workspace tier under `.base/`.
fn home(global_domains: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".base-gbl").join(".base")).unwrap();
    std::fs::write(root.join(".base-gbl").join("domains.toml"), global_domains).unwrap();
    std::fs::create_dir_all(root.join(".base")).unwrap();
    std::fs::write(
        root.join(".base").join("base.toml"),
        "[namespace]\nprefix = \"ops\"\nuri = \"http://ops-sys.local/ontology#\"\n",
    )
    .unwrap();
    tmp
}

fn ns() -> NamespaceConfig {
    NamespaceConfig::default()
}

/// Register a project at `rel` under the root, the way `base project add --path` does:
/// the record carries the path, and a same-named domain with that path trigger is created.
fn register(root: &Path, name: &str, rel: &str) {
    let path = root.join(rel).display().to_string();
    crud::project::add(root, &ns(), name, "active", Some(&path)).unwrap();
}

/// The operator's shape: several projects under `Documents`, one under `.base-gbl`, one
/// beside `Documents-old`, one on its own.
fn register_operator_projects(root: &Path) {
    register(root, "agentic-os", "Documents/agentic-os");
    register(root, "first-client-kit", "Documents/first-client-kit");
    register(root, "renda-group", "Documents/Meet Caddy/renda-group");
    register(root, "handoffs", ".base-gbl/handoffs");
    register(root, "old-thing", "Documents-old/thing");
    register(root, "vp-operators", "genai/vp-operators");
}

/// One prompt in session `sid`; the domains whose block was injected.
fn prompt(config: &BaseConfig, root: &Path, sid: &str, text: &str) -> Vec<String> {
    let event = serde_json::json!({ "prompt": text, "session_id": sid });
    user_prompt_submit::handle(config, root, &event).unwrap().domains_matched
}

/// A fresh session: the per-workspace dedup state is cleared so every prompt below is
/// judged on its own, as the first prompt of a new session is.
fn fresh(root: &Path) {
    SessionState::clear(&root.join(".base"));
}

/// The row `log_hook_event` (hook/mod.rs) writes for a PreToolUse on `rel`, in this
/// tier's log: what a real tool call in session `sid` leaves behind.
fn touch(root: &Path, sid: &str, rel: &str) {
    use std::io::Write;
    let row = serde_json::json!({
        "ts": "2026-09-07T12:00:00-05:00", "hook": "pre-tool-use", "success": true,
        "session_id": sid, "tool_name": "Read", "file_path": root.join(rel).display().to_string(),
    });
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join(".base").join("hook-events.jsonl"))
        .unwrap();
    writeln!(f, "{row}").unwrap();
}

/// D1. Three prompts with no domain vocabulary, each the first of its own session, in a
/// home whose registered projects sit under every path trigger: only the always-on domain
/// reaches the prompt. On 0.14.0 every path-triggered domain came too.
#[test]
fn an_unrelated_prompt_injects_only_the_always_on_domain() {
    let tmp = home(GLOBAL_DOMAINS);
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        register_operator_projects(root);
        let config = BaseConfig::load(root);
        for (i, text) in [
            "what is the weather like today",
            "please summarise this pdf for my mother",
            "write a haiku about tea",
        ]
        .iter()
        .enumerate()
        {
            fresh(root);
            let matched = prompt(&config, root, &format!("unrelated-{i}"), text);
            assert_eq!(matched, vec!["GLOBAL".to_string()], "prompt {text:?}");
        }
    });
}

/// A keyword prompt injects that domain and nothing path-triggered, and a keyword inside a
/// longer word does not fire (`base` in `database`).
#[test]
fn a_keyword_prompt_injects_that_domain_and_nothing_path_triggered() {
    let tmp = home(GLOBAL_DOMAINS);
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        register_operator_projects(root);
        let config = BaseConfig::load(root);

        fresh(root);
        let matched = prompt(&config, root, "kw-1", "how is the caddy deal going");
        assert_eq!(matched, vec!["GLOBAL".to_string(), "meet-caddy".to_string()]);

        fresh(root);
        let matched = prompt(&config, root, "kw-2", "show me the database schema");
        assert_eq!(matched, vec!["GLOBAL".to_string()], "`base` inside `database` must not fire");
    });
}

/// A file this session touched fires the domain whose trigger covers it and nothing
/// broader: `Documents/Meet Caddy` covers one registered project and fires; `Documents`
/// covers three and is inert; a session that touched nothing gets GLOBAL only.
#[test]
fn a_file_this_session_touched_fires_its_domain_and_nothing_broader() {
    let tmp = home(GLOBAL_DOMAINS);
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        register_operator_projects(root);
        let config = BaseConfig::load(root);

        fresh(root);
        touch(root, "touched-caddy", "Documents/Meet Caddy/renda-group/notes.md");
        let matched = prompt(&config, root, "touched-caddy", "hello there");
        assert_eq!(matched, vec!["GLOBAL".to_string(), "meet-caddy".to_string()]);

        fresh(root);
        touch(root, "touched-gbl", ".base-gbl/handoffs/2026-09-07-x.md");
        let matched = prompt(&config, root, "touched-gbl", "hello there");
        assert_eq!(matched, vec!["GLOBAL".to_string(), "base-config".to_string()]);

        fresh(root);
        let matched = prompt(&config, root, "touched-nothing", "hello there");
        assert_eq!(matched, vec!["GLOBAL".to_string()], "another session's touches are not mine");
    });
}

/// D3. `auto_inject = false` keeps a domain out of every automatic path — always-on,
/// keyword, a touched file, and the tool hook — whatever else would have fired it.
#[test]
fn auto_inject_false_keeps_a_domain_out_of_every_automatic_path() {
    let tmp = home(&format!("{GLOBAL_DOMAINS}{CONFIDENTIAL_DOMAINS}"));
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        register(root, "vp-operators", "genai/vp-operators");
        let config = BaseConfig::load(root);

        fresh(root);
        assert_eq!(prompt(&config, root, "conf-1", "hello"), vec!["GLOBAL".to_string()], "always-on, flagged");
        fresh(root);
        assert_eq!(prompt(&config, root, "conf-2", "what are the terms"), vec!["GLOBAL".to_string()], "keyword, flagged");
        fresh(root);
        touch(root, "conf-3", "genai/vp-operators/x.md");
        assert_eq!(prompt(&config, root, "conf-3", "hello"), vec!["GLOBAL".to_string()], "touched file, flagged");

        fresh(root);
        let event = serde_json::json!({
            "tool_name": "Read",
            "tool_input": { "file_path": root.join("genai/vp-operators/x.md").display().to_string() },
        });
        let (data, context) = pre_tool_use::handle(&config, root, &event).unwrap();
        assert!(!data.domains_matched.iter().any(|d| d == "terms"), "{:?}", data.domains_matched);
        assert!(!context.contains("Twelve thousand"), "the tool hook is the other automatic path:\n{context}");
    });
}

/// D3, the third automatic surface (plover's review): the session-start cheat-sheet lists
/// every keyword domain by name with its first four keywords, every session. A flagged
/// domain is not listed; an unflagged one still is.
#[test]
fn auto_inject_false_keeps_a_domain_out_of_the_session_start_cheat_sheet() {
    let tmp = home(&format!("{GLOBAL_DOMAINS}{CONFIDENTIAL_DOMAINS}"));
    base::home::with_thread_home(tmp.path(), || {
        let sheet = base::domain::query::context_triggers_block(tmp.path());
        assert!(sheet.contains("meet-caddy: caddy"), "an unflagged keyword domain is listed:\n{sheet}");
        assert!(!sheet.contains("terms"), "a flagged domain is not named, nor its keywords:\n{sheet}");
    });
}

/// An exclude vetoes an always-on domain; on 0.14.0 `always` returned before the check.
#[test]
fn an_always_on_domain_with_an_exclude_is_vetoed() {
    let tmp = home(&format!("{GLOBAL_DOMAINS}{QUIET_DOMAIN}"));
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        let config = BaseConfig::load(root);

        fresh(root);
        let matched = prompt(&config, root, "quiet-1", "what is the weather");
        assert!(matched.iter().any(|d| d == "quiet"), "{matched:?}");

        fresh(root);
        let matched = prompt(&config, root, "quiet-2", "write a haiku about tea");
        assert!(!matched.iter().any(|d| d == "quiet"), "{matched:?}");
    });
}

/// Stub guard. The domain `project add` creates has one neighbour, the project itself;
/// that block is not emitted, and its domain does not count as injected. A decision on
/// the domain keeps the block, with both rows.
#[test]
fn a_context_block_of_only_the_domains_own_project_is_not_emitted() {
    let tmp = home(GLOBAL_DOMAINS);
    base::home::with_thread_home(tmp.path(), || {
        let root = tmp.path();
        register(root, "vp-operators", "genai/vp-operators");
        let config = BaseConfig::load(root);

        fresh(root);
        touch(root, "stub", "genai/vp-operators/x.md");
        // Prompts 1 and 2 are lean (rules only); the neighbourhood arrives on prompt 3.
        prompt(&config, root, "stub", "hello");
        prompt(&config, root, "stub", "hello again");
        let matched = prompt(&config, root, "stub", "and once more");
        assert!(!matched.iter().any(|d| d == "vp-operators"), "a stub block counted as injected: {matched:?}");

        let store = base::store::load_merged(root).expect("the merged store loads");
        let domain = base::domain::load_domains(root)
            .into_iter()
            .find(|d| d.name == "vp-operators")
            .expect("project add created the domain");
        let (_, neighbourhood, served) = base::domain::query::query_domain_from_graph(&store, &config, &domain);
        assert_eq!(neighbourhood, "", "a block of only `- Project: vp-operators`");
        assert!(served.is_empty(), "a dropped block marks nothing as served: {served:?}");

        crud::decision::log(root, &ns(), "vp-operators", "Use Seedance for b-roll", "cheapest per clip", None).unwrap();
        let store = base::store::load_merged(root).expect("the merged store loads");
        // #65. The neighbourhood SPARQL projected `?name ?type` only, so `row.get("related")`
        // was never bound and this leg marked nothing as served (0.14.0, 0.14.1). Served now
        // carries both rows AND the domain itself, each in the walk's own key form
        // (`<full-iri>`); until 0.14.2 the list held `term_display` suffixes the walk could
        // never match, so the dedup it fed had never fired.
        let (_, neighbourhood, served) = base::domain::query::query_domain_from_graph(&store, &config, &domain);
        assert!(neighbourhood.contains("Decision: Use Seedance for b-roll"), "{neighbourhood}");
        assert!(neighbourhood.contains("Project: vp-operators"), "{neighbourhood}");
        let key = |kind: &str, slug: &str| format!("<{}>", crud::build_iri(&config.namespace, kind, slug));
        assert_eq!(served.len(), 3, "{served:?}");
        for k in [
            key("decision", "vp-operators.use-seedance-for-b-roll"),
            key("project", "vp-operators"),
            key("domain", "vp-operators"),
        ] {
            assert!(served.contains(&k), "served lacks {k}: {served:?}");
        }
    });
}
