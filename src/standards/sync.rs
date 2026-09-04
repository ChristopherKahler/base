use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::BaseConfig;
use crate::crud;

use crate::changelog::Change;
use super::{StandardDef, StandardsFile, SyncSource, TriggerDef};

// ─── Sync: protocols.md → standards.toml → graph ─────────────
//
// protocols.md is the LIVING canonical library (growth contract: new protocol
// = framework entry + checklist line + base learn). This sync re-derives the
// canonical fields (title / rule / failure / controls) from it on every run —
// the toml never forks the text. Triggers, stacks, and severity are
// annotations protocols.md doesn't carry; they live in standards.toml and
// survive every sync. A newly grown protocol with no annotations yet is
// created inert (no triggers) and reported loudly — annotate to activate.

pub struct StandardsSyncStats {
    pub parsed: usize,
    pub updated: usize,
    pub created: usize,
    pub unannotated: Vec<String>,
    pub graph_standards: usize,
    pub toml_path: PathBuf,
}

#[derive(Debug)]
pub struct ParsedProtocol {
    pub id: String,
    pub title: String,
    pub rule: String,
    pub failure: String,
    pub controls: Vec<String>,
}

const DEFAULT_PROTOCOLS_PATH: &str = ".base-frameworks/midas/frameworks/protocols.md";

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = crate::home::home_root()
    {
        return home.join(rest);
    }
    PathBuf::from(path)
}

/// Full sync: bootstrap-or-load global standards.toml, merge canonical text
/// from protocols.md, write the toml back, then sync entities into the
/// global-tier graph.
pub fn sync_standards(
    config: &BaseConfig,
    source_override: Option<&str>,
) -> Result<StandardsSyncStats> {
    let toml_path = super::global_standards_path()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;

    // Load existing file, or bootstrap from the curated seed.
    let mut file: StandardsFile = if toml_path.exists() {
        let content = std::fs::read_to_string(&toml_path)
            .with_context(|| format!("Failed to read {}", toml_path.display()))?;
        toml::from_str(&content)
            .with_context(|| format!("Failed to parse {}", toml_path.display()))?
    } else {
        seed_file()
    };

    // Resolve protocols.md: CLI override > toml [sync].protocols > default.
    let protocols_path = source_override
        .map(expand_tilde)
        .or_else(|| file.sync.protocols.as_deref().map(expand_tilde))
        .or_else(|| crate::home::home_root().map(|h| h.join(DEFAULT_PROTOCOLS_PATH)));

    let mut stats = StandardsSyncStats {
        parsed: 0,
        updated: 0,
        created: 0,
        unannotated: Vec::new(),
        graph_standards: 0,
        toml_path: toml_path.clone(),
    };

    if let Some(pp) = &protocols_path
        && pp.exists()
    {
        let md = std::fs::read_to_string(pp)
            .with_context(|| format!("Failed to read {}", pp.display()))?;
        let parsed = parse_protocols(&md);
        stats.parsed = parsed.len();
        let source_file = pp
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "protocols.md".into());

        for proto in parsed {
            let source = format!("midas:{source_file}#{}", proto.id);
            match file.standards.iter_mut().find(|s| s.id == proto.id) {
                Some(existing) => {
                    // Canonical fields ALWAYS re-derived; annotations preserved.
                    existing.title = proto.title;
                    existing.rule = proto.rule;
                    existing.failure = proto.failure;
                    existing.controls = proto.controls;
                    existing.source = source;
                    stats.updated += 1;
                }
                None => {
                    stats.unannotated.push(proto.id.clone());
                    file.standards.push(StandardDef {
                        id: proto.id,
                        title: proto.title,
                        rule: proto.rule,
                        failure: proto.failure,
                        severity: "medium".into(),
                        controls: proto.controls,
                        source,
                        triggers: TriggerDef::default(),
                        stacks: Default::default(),
                    });
                    stats.created += 1;
                }
            }
        }
    } else if !toml_path.exists() {
        // No protocols.md AND no toml yet: the seed alone still works —
        // it carries full curated text.
        eprintln!(
            "standards sync: protocols.md not found{} — using seed text only",
            protocols_path
                .as_deref()
                .map(|p| format!(" at {}", p.display()))
                .unwrap_or_default()
        );
    }

    // Write the toml back (atomic).
    if let Some(parent) = toml_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = toml_path.with_extension("toml.tmp");
    std::fs::write(&tmp, toml::to_string_pretty(&file)?)?;
    std::fs::rename(&tmp, &toml_path)?;

    // Graph tier: Standard entities in the global graph.
    if let Some(home) = crate::home::home_root() {
        let global_dir = home.join(".base-gbl");
        if global_dir.join(".base").is_dir() {
            stats.graph_standards =
                sync_standards_to_graph(config, &global_dir, &file.standards)?;
        }
    }

    Ok(stats)
}

// ─── protocols.md parser ─────────────────────────────────────

/// Parse protocol sections: `### A<n>. Title` followed by `**Rule:**`,
/// `**Failure it prevents:**`, `**Control satisfied:**` paragraphs. Extra
/// paragraphs (Contrast, Diagnosis trap, Codified specifics) are canonical
/// deep context that stays in the library — injection carries rule + failure.
pub fn parse_protocols(md: &str) -> Vec<ParsedProtocol> {
    let mut out: Vec<ParsedProtocol> = Vec::new();
    let mut cur: Option<ParsedProtocol> = None;
    let mut lines = md.lines().peekable();

    while let Some(line) = lines.next() {
        if let Some(rest) = line.strip_prefix("### ") {
            if let Some(p) = cur.take() {
                out.push(p);
            }
            if let Some((id_part, title)) = rest.split_once(". ") {
                let id = id_part.trim();
                let valid = id.len() >= 2
                    && id.starts_with(|c: char| c.is_ascii_uppercase())
                    && id[1..].chars().all(|c| c.is_ascii_digit());
                if valid {
                    cur = Some(ParsedProtocol {
                        id: id.to_string(),
                        title: title.trim().to_string(),
                        rule: String::new(),
                        failure: String::new(),
                        controls: Vec::new(),
                    });
                }
            }
            continue;
        }
        if line.starts_with("## ") {
            if let Some(p) = cur.take() {
                out.push(p);
            }
            continue;
        }

        let Some(p) = cur.as_mut() else { continue };
        if let Some(rest) = line.strip_prefix("**Rule:**") {
            p.rule = collect_paragraph(rest, &mut lines);
        } else if let Some(rest) = line.strip_prefix("**Failure it prevents:**") {
            p.failure = collect_paragraph(rest, &mut lines);
        } else if let Some(rest) = line.strip_prefix("**Control satisfied:**") {
            let text = collect_paragraph(rest, &mut lines);
            p.controls = text
                .trim_end_matches('.')
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
    }
    if let Some(p) = cur.take() {
        out.push(p);
    }
    out
}

/// Join the wrapped lines of one markdown paragraph, stopping at blank
/// lines, new bold markers, headings, lists, tables, or code fences.
fn collect_paragraph(first: &str, lines: &mut std::iter::Peekable<std::str::Lines>) -> String {
    let mut parts = vec![first.trim().to_string()];
    while let Some(next) = lines.peek() {
        let t = next.trim();
        if t.is_empty()
            || t.starts_with('#')
            || t.starts_with("**")
            || t.starts_with("- ")
            || t.starts_with('|')
            || t.starts_with("```")
        {
            break;
        }
        parts.push(t.to_string());
        lines.next();
    }
    parts.retain(|s| !s.is_empty());
    parts.join(" ")
}

// ─── Graph sync ──────────────────────────────────────────────

/// Sync standards into a tier's graph as ops:Standard entities, mirroring
/// domain sync: GC previously-synced entities by source marker, re-insert.
/// The graph makes standards recallable; hook matching reads the toml.
pub fn sync_standards_to_graph(
    config: &BaseConfig,
    dir: &Path,
    standards: &[StandardDef],
) -> Result<usize> {
    let ns = &config.namespace;
    let p = &ns.prefix;
    let (store, trig_path) = crud::load_workspace_store(dir)?;
    let ws_slug = crud::workspace_slug(dir);
    let graph = crud::workspace_graph_iri(ns, &ws_slug);
    // Snapshot the one graph this writer targets, so the record can carry what
    // actually changed instead of only a label. Scoped to the target graph
    // because this runs often and diffing the whole store would not be free.
    let before = crate::store::snapshot_graphs(&store, std::slice::from_ref(&graph));

    let pfx = crud::prefixes(ns);

    // GC all previously synced standards (marker-scoped, additive graph).
    let gc = format!(
        "{pfx}\n\
         DELETE {{\n\
           GRAPH <{graph}> {{ ?s ?sp ?so . }}\n\
         }}\n\
         WHERE {{\n\
           GRAPH <{graph}> {{\n\
             ?s rdf:type {p}:Standard ;\n\
                {p}:syncSource \"standards.toml\" ;\n\
                ?sp ?so .\n\
           }}\n\
         }}"
    );
    store.update(&gc).context("Failed to GC synced standards")?;

    let now = crud::now_iso();
    for s in standards {
        let iri = crud::build_iri(ns, "standard", &crud::slugify(&s.id));
        let control_triples: String = s
            .controls
            .iter()
            .map(|c| {
                format!(
                    "      {p}:satisfiesControl \"{}\" ;\n",
                    crud::escape_sparql_literal(c)
                )
            })
            .collect();
        let insert = format!(
            "{pfx}\n\
             INSERT DATA {{\n\
               GRAPH <{graph}> {{\n\
                 <{iri}> rdf:type {p}:Standard ;\n\
                   {p}:name \"{}\" ;\n\
                   {p}:standardId \"{}\" ;\n\
                   {p}:ruleText \"{}\" ;\n\
                   {p}:failure \"{}\" ;\n\
                   {p}:severity \"{}\" ;\n\
             {control_triples}\
                   {p}:source \"{}\" ;\n\
                   {p}:syncSource \"standards.toml\" ;\n\
                   {p}:updatedAt \"{now}\"^^xsd:dateTime .\n\
               }}\n\
             }}",
            crud::escape_sparql_literal(&s.title),
            crud::escape_sparql_literal(&s.id),
            crud::escape_sparql_literal(&s.rule),
            crud::escape_sparql_literal(&s.failure),
            crud::escape_sparql_literal(&s.severity),
            crud::escape_sparql_literal(&s.source),
        );
        store
            .update(&insert)
            .with_context(|| format!("Failed to insert standard '{}'", s.id))?;
    }

    let delta = crate::store::delta_since(&store, std::slice::from_ref(&graph), before);
    let ops = delta.to_ops();
    crate::store::write_back(&store, &trig_path, Change::OpWithDelta("standards.sync", &ops))?;
    Ok(standards.len())
}

// ─── Curated seed (A1–A12 annotations + catalog extras) ──────
//
// The annotation layer for the current protocol library: triggers decide WHEN
// each rule surfaces, stacks carry the per-stack idiom. Text fields are
// placeholders overwritten from protocols.md in the same sync run.

fn std_seed(
    id: &str,
    title: &str,
    rule: &str,
    failure: &str,
    severity: &str,
    triggers: TriggerDef,
    stacks: &[(&str, &str)],
) -> StandardDef {
    StandardDef {
        id: id.into(),
        title: title.into(),
        rule: rule.into(),
        failure: failure.into(),
        severity: severity.into(),
        controls: Vec::new(),
        source: format!("midas:protocols.md#{id}"),
        triggers,
        stacks: stacks
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}

fn svec(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

pub fn seed_file() -> StandardsFile {
    let standards = vec![
        std_seed(
            "A1",
            "Reverse-proxy TLS trust",
            "Set trustProxies('*') (or the stack equivalent) + force HTTPS in production whenever the app runs behind a TLS-terminating edge.",
            "The mixed-content blank screen: the edge terminates TLS, the app sees HTTP and generates http:// asset URLs, browsers block them — 200 to curl, white page in every real browser.",
            "high",
            TriggerDef {
                content: svec(&[
                    "trustProxies", "trust proxy", "SECURE_PROXY_SSL_HEADER",
                    "X-Forwarded-Proto", "forceScheme", "APP_URL",
                ]),
                // ci-deploy only — the generic `config` class would fire
                // proxy-trust advice on any file touching process.env/env().
                semantic: svec(&["ci-deploy"]),
                paths: svec(&["bootstrap/", "settings.py", "railway", "dockerfile", "docker-compose"]),
                ..Default::default()
            },
            &[
                ("laravel", "trustProxies('*') in bootstrap + URL::forceScheme('https') in production."),
                ("express", "app.set('trust proxy', true) behind any TLS-terminating edge."),
                ("django", "SECURE_PROXY_SSL_HEADER = ('HTTP_X_FORWARDED_PROTO', 'https') behind any TLS-terminating edge."),
            ],
        ),
        std_seed(
            "A2",
            "CSRF survives session rotation",
            "SPA mutations go through the framework's router — never a raw fetch() carrying a static meta-tag CSRF token.",
            "419s after login: the session token rotates on auth, the meta tag snapshotted at page load no longer matches, every mutation is rejected. Unit tests never see it.",
            "high",
            TriggerDef {
                languages: svec(&["javascript", "typescript", "vue", "svelte", "php"]),
                content: svec(&["csrf", "CSRF", "X-CSRF-TOKEN", "fetch("]),
                semantic: svec(&["frontend-mutation"]),
                ..Default::default()
            },
            &[
                ("laravel", "Inertia router.put/post/delete for mutations — never raw fetch() with the meta-tag CSRF token."),
                ("django", "Django forms/middleware CSRF with rotating token — never a snapshotted static token."),
            ],
        ),
        std_seed(
            "A3",
            "Browser smoke gate is mandatory",
            "Every pipeline runs a headless-browser smoke that logs in, walks every route, and fails on console errors, mixed content, non-2xx same-origin XHR, and an empty app mount.",
            "The entire class of 'passes 107 tests, blank in the browser' bugs — a missing ref import, a mixed-content block, a CSRF break.",
            "high",
            TriggerDef {
                content: svec(&["playwright", "cypress", "puppeteer", "smoke"]),
                semantic: svec(&["ci-deploy"]),
                paths: svec(&[".github/workflows", "playwright", "cypress", "smoke"]),
                ..Default::default()
            },
            &[],
        ),
        std_seed(
            "A4",
            "Explicit spawn environment for child processes",
            "When a job spawns a subprocess, pass an EXPLICIT environment — never rely on ambient env inheritance. Add a timeout to every outbound provider call.",
            "Execution-context auth failures: a child spawned by a differently-launched parent inherits stale/absent env and 401s while the same command succeeds interactively.",
            "critical",
            TriggerDef {
                semantic: svec(&["subprocess"]),
                content: svec(&[
                    "spawn(", "execSync", "child_process", "subprocess.", "Popen",
                    "proc_open(", "shell_exec(", "Command::new",
                ]),
                ..Default::default()
            },
            &[
                ("express", "spawn(cmd, args, { env: {...explicit} }) — build the env object explicitly; never inherit ambient process.env wholesale."),
                ("django", "subprocess.run(cmd, env={...explicit}, timeout=N) — explicit env dict, always a timeout."),
                ("laravel", "Symfony Process with an explicit env array + setTimeout() — never ambient inheritance."),
            ],
        ),
        std_seed(
            "A5",
            "Secrets never touch code, logs, or the session",
            "Provision secrets interactively (hidden prompts, one token at a time). Inject via env only. Never echo, never commit, never paste into an agent session.",
            "Secret leakage into history/logs/transcripts, and the lockdown that follows.",
            "critical",
            TriggerDef {
                semantic: svec(&["secrets", "ci-deploy"]),
                content: svec(&["API_KEY", "_SECRET", "PRIVATE_KEY", "gh secret", "RAILWAY_TOKEN"]),
                paths: svec(&[".env", "secrets", ".github/workflows"]),
                ..Default::default()
            },
            &[],
        ),
        std_seed(
            "A6",
            "Config over code for provider endpoints",
            "Every OAuth/provider integration exposes client_id, client_secret, redirect, authorize_url, token_url as env vars. Controllers never hardcode a provider URL.",
            "A code change + redeploy every time an OAuth app is swapped or a provider moves an endpoint.",
            "high",
            TriggerDef {
                semantic: svec(&["oauth-provider"]),
                content: svec(&["authorize_url", "token_url", "client_secret", "redirect_uri", "oauth"]),
                ..Default::default()
            },
            &[],
        ),
        std_seed(
            "A7",
            "User-consented scope changes only",
            "The platform can never silently widen its own data access — scope changes only through user-consented provider re-authorization, never a server-side grant.",
            "Unauthorized data-access expansion — the thing an enterprise security reviewer probes first.",
            "high",
            TriggerDef {
                semantic: svec(&["oauth-provider"]),
                content: svec(&["scopes", "grant_type", "consent", "reauthorize", "re-authorize"]),
                ..Default::default()
            },
            &[],
        ),
        std_seed(
            "A8",
            "Anti-enumeration status codes",
            "Foreign and non-existent resources return the SAME status (404). 403 is reserved for 'authenticated but lacks the specific ability'.",
            "Tenant/resource enumeration — a 403-vs-404 split tells an attacker which IDs exist.",
            "high",
            TriggerDef {
                semantic: svec(&["api-route", "tenant-model", "auth"]),
                content: svec(&["403", "Forbidden", "findOrFail", "abort(", "NotFound"]),
                ..Default::default()
            },
            &[
                ("laravel", "Scoped route-model binding 404s foreign IDs; abort(403) only for 'authenticated but lacks this ability' — never for foreign resources."),
            ],
        ),
        std_seed(
            "A9",
            "Deny-by-default feature exposure",
            "Features ship dark — 404 when disabled, indistinguishable from non-existent — released deliberately via a code-defined flag registry, globally scoped.",
            "Accidental exposure of half-built surfaces; flags checked in UI only leave the API surface exposed.",
            "medium",
            TriggerDef {
                content: svec(&[
                    "feature_flag", "featureFlag", "Feature::", "flag_enabled",
                    "isEnabled", "launchdarkly", "unleash",
                ]),
                semantic: svec(&["api-route"]),
                ..Default::default()
            },
            &[],
        ),
        std_seed(
            "A10",
            "Migrations rehearse downstream, run automatically, never on prod first",
            "Migrations run automatically on every deploy but reach dev→stage before prod. An add-column migration must sort AFTER the published create-table it alters.",
            "Prod schema failures, and fresh-DB builds that break because file ordering doesn't match dependency ordering.",
            "critical",
            TriggerDef {
                semantic: svec(&["migration"]),
                content: svec(&["CREATE TABLE", "ALTER TABLE", "Schema::", "add_column", "addColumn"]),
                paths: svec(&["migrations/"]),
                ..Default::default()
            },
            &[],
        ),
        std_seed(
            "A11",
            "Defensive config fallbacks",
            "env('X') ?: default, not env('X', default).",
            "An empty-string env var (e.g., from .env.example in CI) is 'set', so the second-arg default never fires — config silently resolves to \"\" and downstream logic misbehaves with no error.",
            "high",
            TriggerDef {
                languages: svec(&["php", "python"]),
                content: svec(&["env(", "environ.get", "getenv"]),
                semantic: svec(&["config"]),
                paths: svec(&["config/", "settings.py"]),
            },
            &[
                ("laravel", "env('X') ?: $default — never env('X', $default); an empty string is 'set' and defeats the second arg."),
                ("django", "os.environ.get('X') or default — never .get('X', default); an empty string is 'set' and defeats the second arg."),
            ],
        ),
        std_seed(
            "A12",
            "Single-service constraint awareness",
            "Know your platform's structural limits before designing the deploy architecture — e.g., Railway volumes attach to ONE service.",
            "A deploy architecture the platform silently won't honor.",
            "medium",
            // Railway-evidence only — "volumes"/"worker" alone would push
            // Railway-specific advice onto every docker-compose user.
            TriggerDef {
                semantic: svec(&["ci-deploy"]),
                content: svec(&["railway", "Railway", "RAILWAY_"]),
                paths: svec(&["railway.json", "railway.toml"]),
                ..Default::default()
            },
            &[],
        ),
        // Catalog extra — security-controls.md domain 6 distilled to the
        // edit-time rule. Not a protocols.md section; sync never overwrites it.
        {
            let mut s = std_seed(
                "SC-IDOR",
                "Tenant-scoped lookups (IDOR prevention)",
                "Scope every model lookup to the authenticated tenant/org at the query level — foreign IDs must resolve 404. An if-check after fetch is not a control; UI-hiding is not a control.",
                "IDOR: tenant B's records don't appear in tenant A's list view, but GET /records/{tenant-B-id} returns 200 with data — the 'control' was a WHERE clause in one view.",
                "critical",
                TriggerDef {
                    semantic: svec(&["tenant-model", "api-route"]),
                    content: svec(&["findOrFail", "org_id", "tenant_id", "team_id", "workspace_id", "belongsTo"]),
                    ..Default::default()
                },
                &[
                    ("laravel", "Scoped route-model binding: resolve bindings through the authenticated org's relation so foreign IDs 404 — never a global find() plus ownership if-check."),
                    ("django", "Custom manager scoping: Model.objects.for_org(request.org) on every lookup — never objects.get(pk) plus an ownership if-check."),
                    ("express", "Query middleware scoping every lookup to req.org — never findById plus an ownership if-check."),
                ],
            );
            s.source = "midas:security-controls.md#6".into();
            s
        },
    ];

    StandardsFile {
        sync: SyncSource {
            protocols: Some(format!("~/{DEFAULT_PROTOCOLS_PATH}")),
        },
        standards,
    }
}

// ─── CLI output helpers ──────────────────────────────────────

pub fn list_standards(cwd: &Path) {
    let standards = super::load_standards(cwd);
    if standards.is_empty() {
        eprintln!("No standards configured. Run `base standards sync` to bootstrap from MIDAS.");
        return;
    }
    println!("| ID | Severity | Title | Lang | Content | Semantic | Paths | Stacks |");
    println!("|----|----------|-------|------|---------|----------|-------|--------|");
    for s in &standards {
        println!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |",
            s.id,
            s.severity,
            s.title,
            s.triggers.languages.len(),
            s.triggers.content.len(),
            s.triggers.semantic.len(),
            s.triggers.paths.len(),
            s.stacks.len(),
        );
    }
    let inert: Vec<&str> = standards
        .iter()
        .filter(|s| {
            s.triggers.content.is_empty()
                && s.triggers.semantic.is_empty()
                && s.triggers.paths.is_empty()
        })
        .map(|s| s.id.as_str())
        .collect();
    if !inert.is_empty() {
        println!(
            "\nUNANNOTATED (will never inject — add triggers): {}",
            inert.join(", ")
        );
    }
}

pub fn get_standard(cwd: &Path, id: &str) {
    let standards = super::load_standards(cwd);
    match standards.iter().find(|s| s.id.eq_ignore_ascii_case(id)) {
        Some(s) => {
            println!("Standard: {} — {}", s.id, s.title);
            println!("Severity: {}", s.severity);
            println!("Source: {}", s.source);
            println!("Rule: {}", s.rule);
            println!("Failure: {}", s.failure);
            if !s.controls.is_empty() {
                println!("Controls: {}", s.controls.join(", "));
            }
            let t = &s.triggers;
            if !t.languages.is_empty() {
                println!("Languages: {}", t.languages.join(", "));
            }
            if !t.content.is_empty() {
                println!("Content triggers: {}", t.content.join(" · "));
            }
            if !t.semantic.is_empty() {
                println!("Semantic classes: {}", t.semantic.join(", "));
            }
            if !t.paths.is_empty() {
                println!("Path triggers: {}", t.paths.join(", "));
            }
            for (stack, idiom) in &s.stacks {
                println!("Stack [{stack}]: {idiom}");
            }
        }
        None => eprintln!("Standard '{id}' not found."),
    }
}

/// Dry-run the matcher against a file — shows every scored standard and the
/// block that would inject. The tuning + verification surface for the matcher.
pub fn test_standard_match(config: &BaseConfig, cwd: &Path, file: &str, content: Option<&str>) {
    let standards = super::load_standards(cwd);
    if standards.is_empty() {
        eprintln!("No standards configured. Run `base standards sync` first.");
        return;
    }
    let path = Path::new(file);
    let ctx = super::matcher::build_context(path, content.unwrap_or(""));

    println!(
        "File: {} | language: {} | stack: {} | classes: [{}]",
        file,
        ctx.language.unwrap_or("?"),
        ctx.stack.unwrap_or("-"),
        ctx.classes.join(", "),
    );

    let mut scored: Vec<(&StandardDef, u32)> = standards
        .iter()
        .filter_map(|s| super::matcher::score(s, &ctx).map(|sc| (s, sc)))
        .collect();
    scored.sort_by_key(|s| std::cmp::Reverse(s.1));

    if scored.is_empty() {
        println!("No standards scored above zero.");
        return;
    }
    println!("\nScores (threshold {}):", config.standards.min_score);
    for (s, sc) in &scored {
        let marker = if *sc >= config.standards.min_score { "✓" } else { " " };
        println!("  {marker} {:>3}  [{}] {}", sc, s.id, s.title);
    }

    let selected = super::matcher::select(
        &standards,
        &ctx,
        config.standards.min_score,
        config.standards.max_inject.min(5),
    );
    if selected.is_empty() {
        println!("\nNothing clears the threshold — no injection.");
    } else {
        let refs: Vec<&super::matcher::Match> = selected.iter().collect();
        println!("\nWould inject:\n{}", super::matcher::render(&refs, &ctx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
## The Protocols

### A1. Reverse-proxy TLS trust

**Rule:** Set `trustProxies('*')` (or the stack equivalent) + force HTTPS in
production whenever the app runs behind a TLS-terminating edge — Railway,
Cloudflare, any PaaS.

**Failure it prevents:** The mixed-content blank screen. The edge terminates TLS
and forwards plain HTTP.

**Control satisfied:** Availability, Processing Integrity.

**Diagnosis trap (codified):** curl does not enforce mixed-content policy.

**Contrast:**
- *Wrong confidence:* "curl returns 200 — deploy is fine."
- *Right check:* headless browser loads the page over https.

### A2. CSRF survives session rotation

**Rule:** SPA mutations go through the framework's router.

**Failure it prevents:** 419s after login.

**Control satisfied:** Security (CSRF protection).

## Anti-Patterns

| Anti-Pattern | Why It Fails | The Protocol |
|--------------|-------------|--------------|
| "Tests pass, ship it" | Unit suites can't see proxy bugs | A1, A2 |
"#;

    #[test]
    fn parses_protocol_sections() {
        let protocols = parse_protocols(SAMPLE);
        assert_eq!(protocols.len(), 2);

        let a1 = &protocols[0];
        assert_eq!(a1.id, "A1");
        assert_eq!(a1.title, "Reverse-proxy TLS trust");
        assert!(a1.rule.starts_with("Set `trustProxies('*')`"));
        // Wrapped lines joined with spaces.
        assert!(a1.rule.contains("production whenever the app runs behind"));
        assert!(a1.failure.starts_with("The mixed-content blank screen."));
        assert_eq!(a1.controls, vec!["Availability", "Processing Integrity"]);

        let a2 = &protocols[1];
        assert_eq!(a2.id, "A2");
        assert_eq!(a2.controls, vec!["Security (CSRF protection)"]);
    }

    #[test]
    fn parser_ignores_non_protocol_sections_and_tables() {
        let md = "### Not a protocol\n\n**Rule:** should be skipped\n\n### A5. Real\n\n**Rule:** real rule.\n";
        let protocols = parse_protocols(md);
        assert_eq!(protocols.len(), 1);
        assert_eq!(protocols[0].id, "A5");
        assert_eq!(protocols[0].rule, "real rule.");
    }

    #[test]
    fn seed_covers_all_twelve_protocols_plus_catalog() {
        let seed = seed_file();
        let ids: Vec<&str> = seed.standards.iter().map(|s| s.id.as_str()).collect();
        for n in 1..=12 {
            assert!(ids.contains(&format!("A{n}").as_str()), "missing A{n}");
        }
        assert!(ids.contains(&"SC-IDOR"));
        // Every seed entry must be annotated (triggers present) — inert seeds
        // would defeat the whole layer.
        for s in &seed.standards {
            assert!(
                !s.triggers.content.is_empty()
                    || !s.triggers.semantic.is_empty()
                    || !s.triggers.paths.is_empty(),
                "seed {} has no triggers",
                s.id
            );
        }
    }

    #[test]
    fn seed_roundtrips_through_toml() {
        let seed = seed_file();
        let toml_str = toml::to_string_pretty(&seed).unwrap();
        let parsed: StandardsFile = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.standards.len(), seed.standards.len());
        let a11 = parsed.standards.iter().find(|s| s.id == "A11").unwrap();
        assert_eq!(a11.triggers.languages, vec!["php", "python"]);
        assert!(a11.stacks.contains_key("laravel"));
    }

    #[test]
    fn merge_preserves_annotations_and_overwrites_canonical() {
        // Simulate: seed entry, then a protocols.md parse with changed text.
        let mut file = seed_file();
        let parsed = parse_protocols(SAMPLE);
        for proto in parsed {
            if let Some(existing) = file.standards.iter_mut().find(|s| s.id == proto.id) {
                existing.title = proto.title;
                existing.rule = proto.rule;
                existing.failure = proto.failure;
                existing.controls = proto.controls;
            }
        }
        let a1 = file.standards.iter().find(|s| s.id == "A1").unwrap();
        // Canonical text now from the "library".
        assert!(a1.rule.starts_with("Set `trustProxies('*')`"));
        // Annotations survived.
        assert!(!a1.triggers.content.is_empty());
        assert!(a1.stacks.contains_key("laravel"));
    }
}
