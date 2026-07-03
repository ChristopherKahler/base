use std::path::Path;

use super::{severity_rank, StandardDef};

// ─── Signal weights ──────────────────────────────────────────
//
// Signal priority (locked in DESIGN): code evidence > semantic class > path
// heuristic > language default. Content matching runs over file bytes + the
// edit payload, so import statements count as code evidence without touching
// the AST store on the hook hot path.
//
// Threshold semantics: min_score defaults to 3, so a bare language match (1)
// or a bare path match (2) never injects on its own — at least one semantic
// class or content hit is required. Whole-catalog injection is context
// pollution; the matcher IS the product.

const W_CONTENT: u32 = 4;
const MAX_CONTENT_HITS: u32 = 2;
const W_SEMANTIC: u32 = 3;
const MAX_SEMANTIC_HITS: u32 = 2;
const W_PATH: u32 = 2;
const W_LANGUAGE: u32 = 1;

/// Cap on how much of a file we read for evidence — enough for imports,
/// class definitions, and route tables without dragging in generated blobs.
const HAYSTACK_FILE_CAP: usize = 64 * 1024;

// ─── Semantic taxonomy ───────────────────────────────────────
//
// Fixed classes: (name, path markers [lowercase], content markers
// [case-sensitive]). A class attaches when any path marker matches the
// lowercased path OR any content marker matches the haystack.

const CLASSES: &[(&str, &[&str], &[&str])] = &[
    (
        "auth",
        &["auth", "login", "session", "middleware/"],
        &[
            "authenticate", "password", "Auth::", "passport", "jwt", "Sanctum",
            "login", "signIn", "sign_in",
        ],
    ),
    (
        "api-route",
        &["routes/", "controller", "api/", "urls.py", "views.py", "endpoints"],
        &[
            "Route::", "router.", "app.get(", "app.post(", "app.put(", "app.delete(",
            "@app.route", "@router.", "#[get(", "#[post(", "urlpatterns",
        ],
    ),
    (
        "migration",
        &["migrations/", "migration"],
        &[
            "CREATE TABLE", "ALTER TABLE", "Schema::create", "Schema::table",
            "createTable", "add_column", "addColumn", "dropColumn", "drop_column",
        ],
    ),
    (
        "subprocess",
        &[],
        &[
            "child_process", "spawn(", "spawnSync", "execSync", "subprocess.",
            "Popen", "os.system", "proc_open(", "shell_exec(", "Command::new",
            "Symfony\\Component\\Process",
        ],
    ),
    (
        "oauth-provider",
        &["oauth"],
        &[
            "oauth", "OAuth", "client_secret", "clientSecret", "authorize_url",
            "token_url", "redirect_uri", "grant_type",
        ],
    ),
    (
        "tenant-model",
        &["models/", "policies/"],
        &[
            "tenant", "org_id", "orgId", "organization_id", "team_id", "workspace_id",
            "forOrg", "belongsTo(Organization", "scopeForOrg",
        ],
    ),
    (
        "ci-deploy",
        &[
            ".github/workflows", "dockerfile", "docker-compose", "railway.json",
            "railway.toml", "procfile", ".gitlab-ci", "jenkinsfile", "fly.toml",
            "deploy",
        ],
        &["runs-on:", "FROM ", "railway up", "docker build"],
    ),
    (
        "config",
        &["config/", ".env", "settings.py", "settings/", "bootstrap/"],
        &["env(", "environ", "getenv", "process.env", "dotenv", "config("],
    ),
    (
        "frontend-mutation",
        &[],
        &[
            "fetch(", "axios", "XMLHttpRequest", ".post(", ".put(", ".patch(",
            ".delete(", "router.post", "router.put", "router.delete",
            "method: 'P", "method: \"P",
        ],
    ),
    (
        "secrets",
        &[".env", "secrets"],
        &[
            "API_KEY", "_SECRET", "SECRET_", "PRIVATE_KEY", "api_key", "apikey",
            "credentials", "access_token",
        ],
    ),
];

// ─── File context ────────────────────────────────────────────

#[derive(Debug)]
pub struct FileContext {
    /// Lowercased full path — for path-marker matching.
    pub path_lower: String,
    /// Basename for the rendered block header.
    pub display: String,
    pub language: Option<&'static str>,
    /// Detected framework stack (laravel / django / rails / express).
    pub stack: Option<&'static str>,
    /// Semantic classes detected on this file.
    pub classes: Vec<&'static str>,
    /// File bytes (capped) + edit payload — the evidence body.
    pub haystack: String,
}

/// Build match context for a touched file: language from extension, stack
/// from marker files walking up, classes from path + content evidence.
/// The haystack combines on-disk bytes with the edit payload so brand-new
/// files (Write) match before they exist.
pub fn build_context(file_path: &Path, edit_payload: &str) -> FileContext {
    let path_str = file_path.to_string_lossy().to_string();
    let path_lower = path_str.to_lowercase();
    let display = file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path_str.clone());

    let mut haystack = String::new();
    if let Ok(bytes) = std::fs::read(file_path) {
        let cap = bytes.len().min(HAYSTACK_FILE_CAP);
        haystack.push_str(&String::from_utf8_lossy(&bytes[..cap]));
        haystack.push('\n');
    }
    haystack.push_str(edit_payload);

    let language = detect_language(&path_lower);
    let stack = detect_stack(file_path);
    let classes = classify(&path_lower, &haystack);

    FileContext {
        path_lower,
        display,
        language,
        stack,
        classes,
        haystack,
    }
}

pub fn detect_language(path_lower: &str) -> Option<&'static str> {
    let basename = path_lower.rsplit('/').next().unwrap_or(path_lower);
    if basename == "dockerfile" || basename.starts_with("dockerfile.") {
        return Some("dockerfile");
    }
    let ext = basename.rsplit('.').next()?;
    match ext {
        "php" => Some("php"),
        "py" => Some("python"),
        "js" | "mjs" | "cjs" | "jsx" => Some("javascript"),
        "ts" | "tsx" | "mts" => Some("typescript"),
        "vue" => Some("vue"),
        "svelte" => Some("svelte"),
        "rb" => Some("ruby"),
        "go" => Some("go"),
        "rs" => Some("rust"),
        "java" => Some("java"),
        "cs" => Some("csharp"),
        "sql" => Some("sql"),
        "sh" | "bash" => Some("shell"),
        "yml" | "yaml" => Some("yaml"),
        "toml" => Some("toml"),
        "json" => Some("json"),
        _ => None,
    }
}

/// Walk up from the touched file looking for framework marker files.
/// Bounded — stops at the filesystem root or after 10 levels.
pub fn detect_stack(file_path: &Path) -> Option<&'static str> {
    let mut dir = file_path.parent()?;
    for _ in 0..10 {
        if dir.join("artisan").exists() {
            return Some("laravel");
        }
        if dir.join("manage.py").exists() {
            return Some("django");
        }
        if dir.join("Gemfile").exists() {
            return Some("rails");
        }
        let pkg = dir.join("package.json");
        if pkg.exists()
            && let Ok(content) = std::fs::read_to_string(&pkg)
            && content.contains("\"express\"")
        {
            return Some("express");
        }
        dir = match dir.parent() {
            Some(p) => p,
            None => break,
        };
    }
    None
}

fn classify(path_lower: &str, haystack: &str) -> Vec<&'static str> {
    CLASSES
        .iter()
        .filter(|(_, path_markers, content_markers)| {
            path_markers.iter().any(|m| path_lower.contains(m))
                || content_markers.iter().any(|m| haystack.contains(m))
        })
        .map(|(name, ..)| *name)
        .collect()
}

// ─── Scoring & selection ─────────────────────────────────────

#[derive(Debug)]
pub struct Match<'a> {
    pub standard: &'a StandardDef,
    pub score: u32,
}

/// Score one standard against the context. None = excluded or zero signal.
pub fn score(standard: &StandardDef, ctx: &FileContext) -> Option<u32> {
    let t = &standard.triggers;

    // Language gate: a declared language list excludes non-matching (or
    // unidentifiable) files outright.
    let lang_match = if t.languages.is_empty() {
        false
    } else {
        match ctx.language {
            Some(lang) if t.languages.iter().any(|l| l == lang) => true,
            _ => return None,
        }
    };

    let content_hits = t
        .content
        .iter()
        .filter(|p| ctx.haystack.contains(p.as_str()))
        .count() as u32;
    let semantic_hits = t
        .semantic
        .iter()
        .filter(|c| ctx.classes.iter().any(|fc| fc == c))
        .count() as u32;
    let path_hit = t.paths.iter().any(|p| ctx.path_lower.contains(&p.to_lowercase()));

    let mut s = 0u32;
    s += W_CONTENT * content_hits.min(MAX_CONTENT_HITS);
    s += W_SEMANTIC * semantic_hits.min(MAX_SEMANTIC_HITS);
    if path_hit {
        s += W_PATH;
    }
    if lang_match {
        s += W_LANGUAGE;
    }

    (s > 0).then_some(s)
}

/// Rank all standards against the context: score desc → severity → id.
/// Returns at most `max_inject` entries at or above `min_score`.
pub fn select<'a>(
    standards: &'a [StandardDef],
    ctx: &FileContext,
    min_score: u32,
    max_inject: usize,
) -> Vec<Match<'a>> {
    let mut matches: Vec<Match<'a>> = standards
        .iter()
        .filter_map(|s| {
            let sc = score(s, ctx)?;
            (sc >= min_score).then_some(Match { standard: s, score: sc })
        })
        .collect();

    matches.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| severity_rank(&b.standard.severity).cmp(&severity_rank(&a.standard.severity)))
            .then_with(|| a.standard.id.cmp(&b.standard.id))
    });
    matches.truncate(max_inject);
    matches
}

// ─── Rendering ───────────────────────────────────────────────

/// First sentence(s) of the failure, capped — the "why" clause stays one line.
fn distill_failure(failure: &str, cap: usize) -> String {
    let f = failure.trim();
    if f.len() <= cap {
        return f.to_string();
    }
    // Cut at the last sentence end before the cap; fall back to a hard cut.
    match f[..cap].rfind ('.') {
        Some(i) if i > 40 => f[..=i].to_string(),
        _ => {
            let mut end = cap;
            while !f.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}…", &f[..end])
        }
    }
}

/// Whether a stack's idiom fits the touched file. A stack idiom is written in
/// its home language — Symfony Process advice on a .js file is actively wrong.
/// Exception: standards that trigger on frontend-mutation carry frontend-side
/// idioms (e.g. A2's "Inertia router.put"), which DO belong on js/ts/vue files
/// inside that stack's repo.
fn stack_idiom_applies(standard: &StandardDef, stack: &str, language: Option<&str>) -> bool {
    let home: &[&str] = match stack {
        "laravel" => &["php"],
        "django" => &["python"],
        "rails" => &["ruby"],
        "express" => &["javascript", "typescript"],
        _ => &[],
    };
    let Some(lang) = language else { return false };
    if home.contains(&lang) {
        return true;
    }
    let is_frontend = matches!(lang, "javascript" | "typescript" | "vue" | "svelte");
    is_frontend
        && standard
            .triggers
            .semantic
            .iter()
            .any(|c| c == "frontend-mutation")
}

/// Render the injection block. The rule text arrives in the touched file's
/// stack idiom when a variant exists AND fits the file's language. Phrased as
/// a directive to apply in THIS edit — not a reference dump.
pub fn render(matches: &[&Match<'_>], ctx: &FileContext) -> String {
    let stack_attr = ctx
        .stack
        .map(|s| format!(" stack=\"{s}\""))
        .unwrap_or_default();
    let mut out = format!("<standards file=\"{}\"{stack_attr}>\n", ctx.display);
    out.push_str("Apply IN this edit — each prevents a real production failure:\n");
    for (i, m) in matches.iter().enumerate() {
        let s = m.standard;
        let text = ctx
            .stack
            .filter(|st| stack_idiom_applies(s, st, ctx.language))
            .and_then(|st| s.stacks.get(st))
            .map(String::as_str)
            .unwrap_or(&s.rule);
        out.push_str(&format!("  {}. [{} · {}] {}", i + 1, s.id, s.severity, text.trim()));
        if !s.failure.is_empty() {
            out.push_str(&format!(" Why: {}", distill_failure(&s.failure, 220)));
        }
        out.push('\n');
    }
    out.push_str("</standards>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::standards::TriggerDef;

    fn std_with_triggers(id: &str, severity: &str, triggers: TriggerDef) -> StandardDef {
        StandardDef {
            id: id.into(),
            title: format!("Standard {id}"),
            rule: format!("Rule text for {id}."),
            failure: format!("Failure for {id}."),
            severity: severity.into(),
            controls: vec![],
            source: String::new(),
            triggers,
            stacks: Default::default(),
        }
    }

    fn ctx(path: &str, haystack: &str) -> FileContext {
        let path_lower = path.to_lowercase();
        let classes = super::classify(&path_lower, haystack);
        FileContext {
            language: detect_language(&path_lower),
            path_lower,
            display: path.rsplit('/').next().unwrap_or(path).to_string(),
            stack: None,
            classes,
            haystack: haystack.to_string(),
        }
    }

    #[test]
    fn language_detection_covers_key_types() {
        assert_eq!(detect_language("app/http/kernel.php"), Some("php"));
        assert_eq!(detect_language("src/main.ts"), Some("typescript"));
        assert_eq!(detect_language("dockerfile"), Some("dockerfile"));
        assert_eq!(detect_language(".github/workflows/ci.yml"), Some("yaml"));
        assert_eq!(detect_language("readme.md"), None);
    }

    #[test]
    fn classifier_detects_migration_by_path_and_content() {
        let c = ctx("database/migrations/2026_01_01_create_users.php", "Schema::create('users', ...)");
        assert!(c.classes.contains(&"migration"));

        let c2 = ctx("db/schema_change.sql", "ALTER TABLE users ADD COLUMN email text;");
        assert!(c2.classes.contains(&"migration"));
    }

    #[test]
    fn classifier_detects_subprocess_from_content_only() {
        let c = ctx("jobs/generate.js", "const { spawn } = require('child_process');");
        assert!(c.classes.contains(&"subprocess"));
    }

    #[test]
    fn language_gate_excludes_wrong_language() {
        let s = std_with_triggers(
            "A11",
            "high",
            TriggerDef {
                languages: vec!["php".into()],
                content: vec!["env(".into()],
                ..Default::default()
            },
        );
        // Rust file using env( — language gate must exclude.
        let c = ctx("src/config.rs", "std::env::var(\"X\")");
        assert_eq!(score(&s, &c), None);

        // PHP file with env( — content (4) + language (1) = 5.
        let c2 = ctx("config/services.php", "'key' => env('SERVICES_KEY', 'default')");
        assert_eq!(score(&s, &c2), Some(5));
    }

    #[test]
    fn bare_path_or_language_never_clears_threshold() {
        // Path-only trigger: 2 < 3 — never injects without class/content.
        let s = std_with_triggers(
            "X1",
            "high",
            TriggerDef {
                paths: vec!["config/".into()],
                ..Default::default()
            },
        );
        let c = ctx("config/empty.txt", "nothing relevant");
        let selected = select(std::slice::from_ref(&s), &c, 3, 3);
        assert!(selected.is_empty());
    }

    #[test]
    fn ranking_orders_by_score_then_severity() {
        let strong = std_with_triggers(
            "S1",
            "medium",
            TriggerDef {
                content: vec!["spawn(".into(), "child_process".into()],
                semantic: vec!["subprocess".into()],
                ..Default::default()
            },
        );
        let weak = std_with_triggers(
            "W1",
            "critical",
            TriggerDef {
                semantic: vec!["subprocess".into()],
                ..Default::default()
            },
        );
        let tie_a = std_with_triggers(
            "T-A",
            "high",
            TriggerDef {
                semantic: vec!["subprocess".into()],
                ..Default::default()
            },
        );

        let standards = vec![weak, strong, tie_a];
        let c = ctx("worker.js", "const { spawn } = require('child_process'); spawn('gen');");
        let selected = select(&standards, &c, 3, 3);

        // strong: content 2×4 + semantic 3 = 11. weak/tie: semantic 3.
        assert_eq!(selected[0].standard.id, "S1");
        assert_eq!(selected[0].score, 11);
        // Equal scores → severity: critical (W1) before high (T-A).
        assert_eq!(selected[1].standard.id, "W1");
        assert_eq!(selected[2].standard.id, "T-A");
    }

    #[test]
    fn budget_caps_selection() {
        let standards: Vec<StandardDef> = (0..6)
            .map(|i| {
                std_with_triggers(
                    &format!("B{i}"),
                    "medium",
                    TriggerDef {
                        semantic: vec!["subprocess".into()],
                        ..Default::default()
                    },
                )
            })
            .collect();
        let c = ctx("run.py", "subprocess.run(['ls'])");
        assert_eq!(select(&standards, &c, 3, 3).len(), 3);
    }

    #[test]
    fn render_uses_stack_idiom_when_available() {
        let mut s = std_with_triggers(
            "A11",
            "high",
            TriggerDef {
                content: vec!["env(".into()],
                ..Default::default()
            },
        );
        s.stacks.insert("laravel".into(), "Use env('X') ?: $default.".into());

        let mut c = ctx("config/app.php", "env('X', 'y')");
        c.stack = Some("laravel");
        let m = Match { standard: &s, score: 4 };
        let out = render(&[&m], &c);
        assert!(out.contains("stack=\"laravel\""));
        assert!(out.contains("Use env('X') ?: $default."));
        assert!(out.contains("[A11 · high]"));
        assert!(out.contains("Why: Failure for A11."));
    }

    #[test]
    fn backend_stack_idiom_never_lands_on_frontend_file() {
        // A4-style: backend standard with a laravel idiom, touched file is JS
        // in a laravel repo → generic rule, not Symfony advice.
        let mut backend = std_with_triggers(
            "A4",
            "critical",
            TriggerDef {
                semantic: vec!["subprocess".into()],
                ..Default::default()
            },
        );
        backend.stacks.insert("laravel".into(), "Symfony Process with explicit env.".into());

        let mut c = ctx("jobs/generate.js", "spawn('x')");
        c.stack = Some("laravel");
        let m = Match { standard: &backend, score: 3 };
        let out = render(&[&m], &c);
        assert!(!out.contains("Symfony"));
        assert!(out.contains("Rule text for A4."));

        // A2-style: frontend-facing standard (frontend-mutation trigger) on a
        // vue file in the same repo → the laravel idiom DOES apply.
        let mut frontend = std_with_triggers(
            "A2",
            "high",
            TriggerDef {
                semantic: vec!["frontend-mutation".into()],
                ..Default::default()
            },
        );
        frontend.stacks.insert("laravel".into(), "Inertia router.put/post/delete.".into());

        let mut c2 = ctx("resources/js/Pages/Settings.vue", "router.put('/x')");
        c2.stack = Some("laravel");
        let m2 = Match { standard: &frontend, score: 3 };
        let out2 = render(&[&m2], &c2);
        assert!(out2.contains("Inertia router.put/post/delete."));
    }

    #[test]
    fn distill_failure_cuts_at_sentence() {
        let long = format!(
            "The mixed-content blank screen strikes when the edge terminates TLS. {}",
            "x".repeat(300)
        );
        let d = distill_failure(&long, 220);
        assert!(d.ends_with('.'));
        assert!(d.len() <= 220);

        let short = "Short failure.";
        assert_eq!(distill_failure(short, 220), short);
    }
}
