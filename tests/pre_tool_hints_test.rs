//! BO-07: pre-tool hints fire only where they fit the file or command (F20, F26).
//!
//! Measured on 2026-10-01 in session 5b860473: the AST hint rode 45 of the 119 tool calls base's pre-tool hook saw,
//! on `base … | grep`, on TOML and markdown searches and in folders no map covers, suggesting queries such as
//! `--contains "\[budget\]"`; and writing a markdown fork doc drew standards A4 (subprocess environments) and A8
//! (404 versus 403). Every check here drives the built binary's pre-tool hook the way Claude Code does.

mod seed;

use std::path::{Path, PathBuf};

use seed::{run_pre_tool_use_at, Apps, Seed};

fn world(tag: &str) -> (Seed, Apps) {
    let root = std::env::temp_dir().join(format!("base-bo07-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    assert!(!root.exists(), "the seed root {} survived its clean", root.display());
    let s = seed::write(&root, &seed::TINY, "");
    let apps = seed::write_apps(&s);
    (s, apps)
}

/// A path as a Bash command spells it: forward slashes, which Git Bash and every Unix shell read the same.
fn sh(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

/// What the hook adds to the model's context for one tool call.
fn context(s: &Seed, cwd: &Path, tool: &str, input: serde_json::Value, session: &str) -> String {
    let (code, stdout, stderr) = run_pre_tool_use_at(s, cwd, tool, input, session, &[]);
    assert_eq!(code, 0, "the pre-tool hook failed: {stderr}");
    let stdout = stdout.trim();
    if stdout.is_empty() {
        return String::new();
    }
    let v: serde_json::Value = serde_json::from_str(stdout).unwrap_or_else(|e| panic!("not the JSON envelope ({e}): {stdout}"));
    v["hookSpecificOutput"]["additionalContext"].as_str().unwrap_or_default().to_string()
}

fn bash(s: &Seed, cwd: &Path, command: &str) -> String {
    context(s, cwd, "Bash", serde_json::json!({ "command": command }), "bo07-bash")
}

/// The standards ids a Write of `content` to `path` draws, in the order injected.
fn standards_for(s: &Seed, path: &Path, content: &str, session: &str) -> Vec<String> {
    let cwd = path.parent().expect("a folder");
    std::fs::create_dir_all(cwd).expect("the file's folder");
    let out = context(s, cwd, "Write", serde_json::json!({ "file_path": path.display().to_string(), "content": content }), session);
    let Some(block) = out.split("<standards").nth(1) else {
        return Vec::new();
    };
    block
        .lines()
        .filter_map(|l| l.trim().split_once(". [").and_then(|(_, rest)| rest.split(" · ").next()).map(str::to_string))
        .collect()
}

const A4: &str = r#"
[[standard]]
id = "A4"
title = "Explicit spawn environment for child processes"
rule = "When a job spawns a subprocess, pass an EXPLICIT environment."
failure = "Execution-context auth failures."
severity = "critical"

[standard.triggers]
content = ["spawn(", "execSync", "child_process", "subprocess.", "Popen", "Command::new"]
semantic = ["subprocess"]
"#;

const A8: &str = r#"
[[standard]]
id = "A8"
title = "Anti-enumeration status codes"
rule = "Foreign and non-existent resources return the SAME status (404)."
failure = "Tenant/resource enumeration."
severity = "high"

[standard.triggers]
content = ["403", "Forbidden", "findOrFail", "abort(", "NotFound"]
semantic = ["api-route", "tenant-model", "auth"]
"#;

fn write_standards(s: &Seed, extra: &str) {
    std::fs::write(s.home.join(".base-gbl").join("standards.toml"), format!("{A4}\n{A8}\n{extra}")).expect("standards.toml");
}

/// F20a, F20b, Example 1: the hint fires on a code search over source files or inside an app with a code map, and on
/// nothing else.
#[test]
fn ast_hint_only_on_code_search() {
    let (s, apps) = world("only-code");
    let home = sh(&s.home);
    // The global tier carries a code map, as on the machine measured (it holds scripts), so the handoffs row below
    // is decided by what the folder holds, not by the absence of a map.
    let gbl_map = s.home.join(".base-gbl").join(".base-ast");
    std::fs::create_dir_all(&gbl_map).unwrap();
    std::fs::write(gbl_map.join("ast.ttl"), "# stub\n").unwrap();
    let none: [(&Path, String); 14] = [
        // A folder of docs inside an app with a map is a docs search (F20b), and so is a docs folder of a mapped tier.
        (&apps.mapped, r#"grep -rn "select" docs/"#.into()),
        (&apps.mapped, format!(r#"grep -rln -i "DUE NOW" {home}/.base-gbl/handoffs"#)),
        (&apps.mapped, "base fork --help".into()),
        (&s.ws, format!("ls -1t {home}/.base-gbl/handoffs/*.md | head -8")),
        (&s.ws, format!("grep -rn -l '\\[budget\\]' {home}/.base-gbl/*.toml")),
        (&apps.mapped, "base relay sessions 2>&1 | grep -i -E 'kite|title' | head -10".into()),
        (&apps.mapped, "cat src/lib.rs".into()),
        (&apps.mapped, "head -12 src/lib.rs | grep '//'".into()),
        (&apps.mapped, "git grep -n select".into()),
        (&apps.mapped, "cargo test 2>&1 | grep -E 'test result'".into()),
        (&s.ws, format!("grep -rn -i 'DUE NOW' {home}/.base-gbl/handoffs {home}/notes/plan.md")),
        (&apps.mapped, "grep -rn --include=*.md select .".into()),
        (&apps.mapped, "rg -t md select".into()),
        (&apps.mapped, "find . -name '*.toml'".into()),
    ];
    for (cwd, command) in &none {
        let out = bash(&s, cwd, command);
        assert!(!out.contains("<ast-hint>"), "{command:?} drew a hint:\n{out}");
    }

    let hinted: [(&str, &str); 4] = [
        (r#"grep -rn "fn select(" src/"#, r#"base ast query --contains "select""#),
        (r#"rg "user_prompt_submit::handle" src/hook"#, r#"base ast query --contains "user_prompt_submit""#),
        (r#"grep -rn "TODO" src/"#, r#"base ast query --contains "TODO""#),
        ("find . -name 'lib*.rs'", r#"base ast query --file "lib""#),
    ];
    for (command, line) in hinted {
        let out = bash(&s, &apps.mapped, command);
        assert!(out.contains("<ast-hint>") && out.contains(line), "{command:?} should suggest {line:?}:\n{out}");
        assert!(!out.contains("--target"), "the cwd's own map needs no --target:\n{out}");
    }

    // From a folder with no map, the map named is the searched folder's, and the query says so.
    let out = bash(&s, &s.ws, &format!(r#"grep -rn "fn select(" {}/src"#, sh(&apps.mapped)));
    assert!(out.contains("--target \"") && out.contains("mapped-app\" --contains \"select\""), "{out}");

    // PowerShell and context-mode searches follow the same rule.
    let out = context(&s, &apps.mapped, "PowerShell", serde_json::json!({ "command": "Select-String -Path src/*.rs -Pattern 'handler'" }), "bo07-ps");
    assert!(out.contains(r#"--contains "handler""#), "{out}");
    let out = context(&s, &apps.mapped, "PowerShell", serde_json::json!({ "command": "Get-Content src/lib.rs | Select-String 'select'" }), "bo07-ps");
    assert!(!out.contains("<ast-hint>"), "Select-String over piped text is a filter:\n{out}");
    let batch = serde_json::json!({ "commands": [{ "label": "x", "command": "cat src/lib.rs" }, { "label": "y", "command": "grep -rn 'fn select' src" }] });
    let out = context(&s, &apps.mapped, "mcp__plugin_context-mode_context-mode__ctx_batch_execute", batch, "bo07-ctx");
    assert!(out.contains(r#"--contains "select""#), "{out}");
    let read = serde_json::json!({ "path": sh(&apps.mapped.join("src/lib.rs")), "code": "console.log(1)" });
    let out = context(&s, &apps.mapped, "mcp__plugin_context-mode_context-mode__ctx_execute_file", read, "bo07-ctx");
    assert!(!out.contains("<ast-hint>"), "reading one file is not a search:\n{out}");
}

/// F20c: the suggested query is a plain name, regex syntax stripped, the longest name-like token kept; with none, the
/// generic hint and no query.
#[test]
fn ast_hint_query_is_a_plain_name() {
    let (s, apps) = world("plain-name");
    for (command, name) in [
        (r#"grep -n -A12 '^\s*\$scrub\s*=' src/lib.rs"#, "scrub"),
        (r#"grep -n -E 'tabArgs|wt\.exe|Start-Process' src/lib.rs"#, "tabArgs"),
        (r#"grep -rn '\[budget\]' src/"#, "budget"),
        (r#"rg -e '\bselect\b' -e 'sel' src"#, "select"),
        (r#"grep -rn '^paths|Documents' src/"#, "Documents"),
    ] {
        let out = bash(&s, &apps.mapped, command);
        let line = format!(r#"base ast query --contains "{name}""#);
        assert!(out.contains(&line), "{command:?} should suggest {line:?}:\n{out}");
    }
    for command in [r#"for p in A B; do grep -rn "$p" src/; done"#, "grep -rn 'rc' src/", "grep -rn '^$' src/"] {
        let out = bash(&s, &apps.mapped, command);
        assert!(out.contains("Try `base ast query` for code navigation."), "{command:?}: no name, so the generic hint:\n{out}");
        assert!(!out.contains("--contains \""), "{command:?}: no query:\n{out}");
    }
}

/// The no-map hint is true for the folder it fires in (BO-00's review): a build is under way in an app base maps on
/// its own; a cache folder is never mapped automatically, so it says so and how to map it by hand; a folder in no
/// app, or a search that names no source file, hears nothing.
#[test]
fn ast_hint_without_a_map_says_what_is_true() {
    let (s, apps) = world("no-map");
    let out = bash(&s, &apps.plain, r#"grep -rn --include=*.rs "fn main" ."#);
    assert!(out.contains("No code map covers") && out.contains("plain-app") && out.contains("building one in the background"), "{out}");
    assert!(!out.contains("nothing to run"), "the old claim:\n{out}");
    let out = bash(&s, &apps.cached, r#"grep -rn "fn main" src/*.rs"#);
    assert!(out.contains("never maps it automatically (a cache directory)") && out.contains("base sync --ast --target"), "{out}");
    // Said once per session per app: the same search again in the same session hears nothing; another session hears
    // it. (Run from the workspace, where the session's record is kept.)
    let again = format!(r#"grep -rn "fn main" {}/src/main.rs"#, sh(&apps.cached));
    let input = || serde_json::json!({ "command": again });
    let first = context(&s, &s.ws, "Bash", input(), "bo07-once");
    assert!(first.contains("No code map covers"), "first time in the session:\n{first}");
    let second = context(&s, &s.ws, "Bash", input(), "bo07-once");
    assert!(!second.contains("<ast-hint>"), "the same session is not told twice:\n{second}");
    let other = context(&s, &s.ws, "Bash", input(), "bo07-other");
    assert!(other.contains("No code map covers"), "another session is told:\n{other}");
    let out = bash(&s, &apps.plain, r#"grep -rn "fn main" src/"#);
    assert!(!out.contains("<ast-hint>"), "a folder with no map and no source file named:\n{out}");
    let out = bash(&s, &s.ws, &format!("grep -n fn {}/x.rs", sh(&apps.loose)));
    assert!(!out.contains("<ast-hint>"), "a file in no app:\n{out}");
}

/// F26b, Example 2's markdown row: a document never draws a standard that declares no scope, however much its prose
/// quotes code.
#[test]
fn standards_skip_docs() {
    let (s, _apps) = world("skip-docs");
    write_standards(&s, "");
    let prose = "The hook spawns `claude -p` via Command::new and spawn(; the API answers 403 Forbidden on findOrFail.";
    let doc = s.home.join(".base").join("forks").join("2026-10-01-hook-injection-audit.md");
    std::fs::create_dir_all(doc.parent().unwrap()).unwrap();
    assert_eq!(standards_for(&s, &doc, prose, "bo07-docs"), Vec::<String>::new());
    // Control: the same text in a Rust file draws both, so the markdown row is the scope and not the scoring.
    let code = s.home.join("dev").join("mapped-app").join("src").join("prose.rs");
    let got = standards_for(&s, &code, prose, "bo07-docs-control");
    assert!(got.contains(&"A4".to_string()) && got.contains(&"A8".to_string()), "control: {got:?}");
}

/// F26a, F26c, Example 2's code rows: a declared scope decides by extension, language and path.
#[test]
fn standards_respect_declared_scope() {
    let (s, apps) = world("declared");
    write_standards(
        &s,
        r#"
[[standard]]
id = "S-TS"
title = "TypeScript only"
rule = "Typed route handlers."
severity = "medium"
applies_to = { extensions = [".ts"] }
[standard.triggers]
content = ["router.get"]

[[standard]]
id = "S-API"
title = "Anything under api"
rule = "Document every route."
severity = "medium"
applies_to = { paths = ["**/api/**"] }
[standard.triggers]
content = ["ROUTE-NOTE"]

[[standard]]
id = "S-DEPLOY"
title = "Code and deploy config"
rule = "One service per volume."
severity = "medium"
applies_to = { code = true, paths = ["railway.toml", ".github/workflows/**"] }
[standard.triggers]
content = ["RAILWAY_VOLUME"]

[[standard]]
id = "S-PHP"
title = "Declared by language"
rule = "env() with a fallback."
severity = "medium"
[standard.triggers]
languages = ["php"]
content = ["getenv("]
"#,
    );
    let llm = apps.mapped.join("src").join("llm.rs");
    let users = apps.mapped.join("api").join("routes").join("users.ts");
    // Example 2: src/llm.rs spawns `claude -p`, so the unscoped A4 applies; the TypeScript-only S-TS does not, though
    // its trigger text is in the file.
    let got = standards_for(&s, &llm, r#"let out = Command::new("claude").arg("-p").spawn(); // router.get"#, "bo07-scope-1");
    assert!(got.contains(&"A4".to_string()) && !got.contains(&"S-TS".to_string()), "{got:?}");
    // api/routes/users.ts: A8 (unscoped, a code file) and S-TS (declared .ts).
    let got = standards_for(&s, &users, "router.get('/users/:id', (req, res) => res.status(403).send('Forbidden'));", "bo07-scope-2");
    assert!(got.contains(&"A8".to_string()) && got.contains(&"S-TS".to_string()), "{got:?}");
    // A path pattern admits any kind of file under it, a document too, and nothing outside it.
    let readme = apps.mapped.join("api").join("README.md");
    assert_eq!(standards_for(&s, &readme, "ROUTE-NOTE", "bo07-scope-3"), vec!["S-API"]);
    assert_eq!(standards_for(&s, &llm, "// ROUTE-NOTE", "bo07-scope-4"), Vec::<String>::new());
    // Code plus deploy config: railway.toml and a workflow are in, a markdown note is not.
    for (path, want) in [
        (apps.mapped.join("railway.toml"), true),
        (apps.mapped.join(".github").join("workflows").join("ci.yml"), true),
        (apps.mapped.join("src").join("deploy.rs"), true),
        (apps.mapped.join("docs").join("deploy.md"), false),
    ] {
        let got = standards_for(&s, &path, "RAILWAY_VOLUME", &format!("bo07-deploy-{}", path.display()));
        assert_eq!(got.contains(&"S-DEPLOY".to_string()), want, "{}: {got:?}", path.display());
    }
    // `triggers.languages` is a declaration too.
    assert_eq!(standards_for(&s, &apps.mapped.join("config.php"), "getenv('X')", "bo07-php-1"), vec!["S-PHP"]);
    assert_eq!(standards_for(&s, &apps.mapped.join("notes.md"), "getenv('X')", "bo07-php-2"), Vec::<String>::new());
}

/// F26b: a standard with no declared scope applies to code files only, never to .md, .txt, .json, .toml, .yaml or
/// .csv documents.
#[test]
fn unscoped_standard_applies_to_code_only() {
    let (s, apps) = world("unscoped");
    write_standards(
        &s,
        r#"
[[standard]]
id = "S-ANY"
title = "No declared scope"
rule = "Close what you open."
severity = "medium"
[standard.triggers]
content = ["OPEN_HANDLE"]
"#,
    );
    let at = |name: &str| -> PathBuf { apps.mapped.join("files").join(name) };
    for name in ["notes.md", "notes.txt", "data.json", "conf.toml", "conf.yaml", "conf.yml", "rows.csv", "Dockerfile", ".env"] {
        assert_eq!(standards_for(&s, &at(name), "OPEN_HANDLE", &format!("bo07-doc-{name}")), Vec::<String>::new(), "{name}");
    }
    for name in ["main.rs", "job.py", "app.ts", "run.sh", "Kernel.php"] {
        assert_eq!(standards_for(&s, &at(name), "OPEN_HANDLE", &format!("bo07-code-{name}")), vec!["S-ANY"], "{name}");
    }
}
