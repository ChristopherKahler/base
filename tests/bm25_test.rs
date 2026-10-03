//! BO-18 (K7, D9): rules ranked by how well the prompt fits them (BM25), through the real prompt hook.
//!
//! Each test seeds a home with its own synthetic `domains.toml` (invented text: this repository is public), builds the
//! rule index the way base builds it (`base domain sync`, one of the commands that change prompt matching), and drives
//! `base hook user-prompt-submit` as Claude Code does. The ranking itself (the fit) is pinned in `emit::prompt`'s unit
//! tests, the tokenizer and the scorer in `domain::bm25` and `domain::score_index`.

mod seed;

use std::path::{Path, PathBuf};

use seed::{run_base, run_prompt_submit};

/// Two invented domains: `ledger`'s first rule carries a test prompt (BO-14), so a prompt that shares its words but
/// not the domain's keyword is a near miss (Example 3's shape); `garden` shares nothing with it.
const DOMAINS: &str = r#"
[[domain]]
name = "ledger"
prompt_keywords = ["ledger"]
rules = [
  { text = "Close the books only after the payroll export is reconciled against the bank feed.", fires_on = ["the payroll export never reconciled with the bank feed this month"] },
  "Invoices over the limit need a second approver before they are paid.",
]

[[domain]]
name = "garden"
prompt_keywords = ["garden"]
rules = ["Water the tomatoes before noon in the summer heat."]
"#;

const PAYROLL: &str = "Close the books only after the payroll export is reconciled against the bank feed.";
const INVOICES: &str = "Invoices over the limit need a second approver before they are paid.";
const TOMATOES: &str = "Water the tomatoes before noon in the summer heat.";

fn root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("base-bm25-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    assert!(!root.exists(), "the seed root {} survived its clean", root.display());
    root
}

/// A tiny seed holding [`DOMAINS`], with `toml` appended to its global `base.toml`.
fn seeded(tag: &str, toml: &str) -> seed::Seed {
    let s = seed::write(&root(tag), &seed::TINY, toml);
    std::fs::write(s.ws.join(".base").join("domains.toml"), DOMAINS).expect("domains.toml");
    s
}

fn index_file(s: &seed::Seed) -> PathBuf {
    s.ws.join(".base").join("bm25-index.json")
}

/// Build the index as base does after a matching change: `base domain sync` ends by refreshing it (K7e).
fn build_index(s: &seed::Seed) {
    let (code, out, err) = run_base(s, &["domain", "sync"]);
    assert_eq!(code, 0, "base domain sync failed: {out}{err}");
    assert!(index_file(s).exists(), "base domain sync built no index: {out}{err}");
}

fn prompt(s: &seed::Seed, text: &str, session: &str) -> String {
    let (code, out, err) = run_prompt_submit(s, text, Some(session));
    assert_eq!(code, 0, "the prompt hook failed: {err}");
    out
}

/// The match log's row for `session`.
fn row(s: &seed::Seed, session: &str) -> serde_json::Value {
    let path = s.ws.join(".base").join("match-log.jsonl");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|r| r["session"] == session)
        .unwrap_or_else(|| panic!("no match-log row for {session}"))
}

fn id(domain: &str, text: &str) -> String {
    base::domain::rules::rule_id(domain, text)
}

/// K7d: a keyword hit still always qualifies. `ledger` matched by its keyword serves both its rules, the one that
/// shares no word with the prompt included, and with `min_score` out of reach as well.
#[test]
fn keyword_hit_always_qualifies() {
    for (tag, toml) in [("kw-default", ""), ("kw-out-of-reach", "[match]\nmin_score = 100000.0\n")] {
        let s = seeded(tag, toml);
        build_index(&s);
        let out = prompt(&s, "open the ledger for me", &format!("{tag}-1"));
        assert!(out.contains(PAYROLL) && out.contains(INVOICES), "[{tag}] both ledger rules on its keyword:\n{out}");
        assert!(!out.contains(TOMATOES), "[{tag}] control: garden did not match:\n{out}");
        let r = row(&s, &format!("{tag}-1"));
        assert!(
            r["matched"].as_array().unwrap().iter().any(|m| m["domain"] == "ledger" && m["by"] == "keyword"),
            "[{tag}] matched by keyword: {r}"
        );
    }
}

/// Example 3: a prompt with no keyword of `ledger`'s shares words with its rule's test prompt and is served on its
/// score. The knock-out: with `min_score` out of reach, the same prompt serves nothing of `ledger`; nor with it unset,
/// the default (lynx's Q7 ruling on BO-18), while its scores are still logged.
#[test]
fn score_admits_near_miss() {
    let near = "why does the payroll export not reconcile with the bank feed";
    let s = seeded("near", "[match]\nmin_score = 2.0\n");
    build_index(&s);
    let out = prompt(&s, near, "near-1");
    assert!(out.contains("[DOMAIN: ledger]") && out.contains(PAYROLL), "served by score:\n{out}");
    assert!(!out.contains(INVOICES), "only the rule whose score reached min_score, not the whole domain:\n{out}");
    assert!(!out.contains(TOMATOES), "garden shares nothing with it:\n{out}");
    let r = row(&s, "near-1");
    let matched = r["matched"].as_array().unwrap();
    assert!(matched.iter().any(|m| m["domain"] == "ledger" && m["by"] == "score"), "matched by score: {r}");
    let served = r["served"].as_array().unwrap();
    let payroll = id("ledger", PAYROLL);
    assert!(
        served.iter().any(|i| i["id"] == payroll.as_str() && i["by"] == "score" && i["score"].as_f64().unwrap_or(0.0) >= 2.0),
        "served item carries by and score: {r}"
    );

    let s = seeded("near-off", "[match]\nmin_score = 100000.0\n");
    build_index(&s);
    let out = prompt(&s, near, "near-off-1");
    assert!(!out.contains(PAYROLL), "knock-out: under min_score the near miss is not served:\n{out}");

    let s = seeded("near-default", "");
    build_index(&s);
    let out = prompt(&s, near, "near-default-1");
    assert!(!out.contains(PAYROLL), "min_score unset: no rule is served on its score alone:\n{out}");
    let r = row(&s, "near-default-1");
    assert_eq!(r["index"], "ok", "{r}");
    assert!(r.get("min_score").is_none(), "no threshold in force: {r}");
    assert!(!r["matched"].as_array().unwrap().iter().any(|m| m["by"] == "score"), "{r}");
    assert!(
        r["scores"].as_array().unwrap().iter().any(|x| x["id"] == payroll.as_str() && x["by"] == "bm25"),
        "the scores are logged all the same: {r}"
    );
}

/// K7e: the hook loads the index and never builds it. A term planted in the index file (and nowhere in the store)
/// serves its rule through the hook, so the file was what the hook scored with; the file is byte for byte the same
/// after the hook; `rule add` rewrites it from the store; and a refresh with nothing changed writes nothing.
#[test]
fn index_cached_not_rebuilt_in_hook() {
    let s = seeded("cache", "[match]\nmin_score = 0.5\n");
    build_index(&s);
    let path = index_file(&s);
    let mut index: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let garden = id("garden", TOMATOES);
    let at = index["docs"].as_array().unwrap().iter().position(|d| d["id"] == garden.as_str()).expect("garden's rule is a document");
    index["corpus"]["docs"][at]["zzqsentinel"] = serde_json::json!(3);
    index["corpus"]["df"]["zzqsentinel"] = serde_json::json!(1);
    let planted = serde_json::to_string(&index).unwrap();
    std::fs::write(&path, &planted).unwrap();

    let out = prompt(&s, "zzqsentinel", "cache-1");
    assert!(out.contains(TOMATOES), "the planted term served garden's rule, so the hook scored with the file:\n{out}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), planted, "the hook did not write the index");
    assert_eq!(row(&s, "cache-1")["index"], "ok");

    let (code, out, err) = run_base(&s, &["rule", "add", "--domain", "garden", "--text", "Mulch the beds after the first frost."]);
    assert_eq!(code, 0, "rule add: {out}{err}");
    let rebuilt = std::fs::read_to_string(&path).unwrap();
    assert!(!rebuilt.contains("zzqsentinel"), "rule add rebuilt the index from the store");
    assert!(rebuilt.contains(&id("garden", "Mulch the beds after the first frost.")), "with the new rule in it");

    let before = std::fs::metadata(&path).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    build_index(&s);
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), before, "an unchanged refresh writes nothing");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), rebuilt);
}

/// K7f: every score above zero is in the match log with what scored it, and the row says which threshold and index
/// were in force. With no index yet the row says so; with `bm25 = false` there is no BM25 at all.
#[test]
fn scores_logged() {
    let s = seeded("logged", "[match]\nmin_score = 2.0\n");
    let out = prompt(&s, "why does the payroll export not reconcile with the bank feed", "logged-0");
    assert!(!out.contains(PAYROLL), "control: no index yet, so keyword-only:\n{out}");
    let r = row(&s, "logged-0");
    assert_eq!(r["index"], "missing", "{r}");
    assert!(r.get("min_score").is_none(), "{r}");

    build_index(&s);
    prompt(&s, "why does the payroll export not reconcile with the bank feed", "logged-1");
    let r = row(&s, "logged-1");
    assert_eq!((r["index"].as_str(), r["min_score"].as_f64()), (Some("ok"), Some(2.0)), "{r}");
    let scores = r["scores"].as_array().unwrap();
    let payroll = id("ledger", PAYROLL);
    let mine = scores.iter().find(|x| x["id"] == payroll.as_str()).unwrap_or_else(|| panic!("no score for the payroll rule: {r}"));
    assert_eq!((mine["by"].as_str(), mine["domain"].as_str()), (Some("bm25"), Some("ledger")));
    assert!(mine["score"].as_f64().unwrap() > 2.0, "{mine}");
    assert!(scores.iter().all(|x| x["score"].as_f64().unwrap() > 0.0), "only scores above zero: {r}");
    assert!(!scores.iter().any(|x| x["id"] == id("garden", TOMATOES).as_str()), "no shared term, no score: {r}");

    let s = seeded("logged-off", "[match]\nbm25 = false\n");
    let (code, out, err) = run_base(&s, &["domain", "sync"]);
    assert_eq!(code, 0, "base domain sync failed: {out}{err}");
    prompt(&s, "why does the payroll export not reconcile with the bank feed", "logged-off-1");
    let r = row(&s, "logged-off-1");
    assert!(r.get("index").is_none() && r.get("min_score").is_none(), "bm25 off: {r}");
    assert!(!index_file(&s).exists(), "bm25 off: no index is built");
}

// ── lynx's G0 verdict, Q1 condition 2: the path with no BM25 is what 07277bc printed ─────────────────────────────

fn replay_fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("replay").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The replay corpus (BO-00's, over budget on several prompts) run through the prompt hook on its own seed: per prompt
/// in `lines`, what was printed and the session's blocks file, without its write time, with the seed's root replaced by
/// `<root>`. With `index`, the rule index is built first and then `extra` is appended to the config, so a setting that
/// turns BM25 off meets an index that is there.
fn corpus_outputs(tag: &str, extra: &str, lines: &[usize], index: bool) -> Vec<serde_json::Value> {
    let root = root(tag);
    let s = seed::write(&root, &seed::TINY, &replay_fixture("base.toml"));
    std::fs::write(s.ws.join(".base").join("domains.toml"), replay_fixture("domains.toml")).expect("domains.toml");
    if index {
        build_index(&s);
    }
    if !extra.is_empty() {
        let config = s.home.join(".base-gbl").join("base.toml");
        let text = std::fs::read_to_string(&config).expect("the seed's base.toml");
        std::fs::write(&config, format!("{text}{extra}")).expect("base.toml");
    }
    let prompts: Vec<String> = replay_fixture("prompts.txt")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect();
    let norm = |t: &str| {
        let r = root.display().to_string();
        t.replace(&r, "<root>").replace(&r.replace('\\', "/"), "<root>").replace(&r.replace('\\', "\\\\"), "<root>")
    };
    lines
        .iter()
        .map(|&i| {
            let session = format!("bm25-off-{i:02}");
            let out = prompt(&s, &prompts[i], &session);
            let blocks_path = s.ws.join(".base").join("hook-output").join(&session).join("prompt-blocks.json");
            let mut blocks: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&blocks_path).expect("blocks file")).expect("blocks JSON");
            blocks.as_object_mut().expect("an object").remove("written_at");
            serde_json::json!({ "line": i, "stdout": norm(&out), "blocks": serde_json::from_str::<serde_json::Value>(&norm(&blocks.to_string())).unwrap() })
        })
        .collect()
}

/// With `[match] bm25 = false`, and with BM25 on but no index built yet, the prompt hook prints exactly what `07277bc`
/// (main before BO-18) printed, and writes the same blocks file, on corpus prompts that go over the budget. The golden
/// file was written by `07277bc`'s own binary through this same seed (how: `tests/fixtures/bm25/README.md`).
#[test]
fn bm25_off_and_no_index_print_what_07277bc_printed() {
    let golden_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("bm25").join("off-path-golden.json");
    let golden: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&golden_path).expect("the golden file")).expect("golden JSON");
    let runs = golden["runs"].as_array().expect("runs");
    let lines: Vec<usize> = runs.iter().map(|r| r["line"].as_u64().expect("line") as usize).collect();
    assert!(
        runs.iter().any(|r| r["stdout"].as_str().unwrap_or_default().contains("[base: withheld ")),
        "control: the golden holds prompts that went over the budget"
    );
    for (tag, extra, index) in [("off", "\n[match]\nbm25 = false\n", true), ("no-index", "", false)] {
        let got = corpus_outputs(tag, extra, &lines, index);
        for (want, got) in runs.iter().zip(&got) {
            assert_eq!(got["stdout"], want["stdout"], "[{tag}] line {}: the printed output differs from 07277bc's", want["line"]);
            assert_eq!(got["blocks"], want["blocks"], "[{tag}] line {}: the blocks file differs from 07277bc's", want["line"]);
        }
    }
}
