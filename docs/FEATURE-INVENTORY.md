---
type: reference
status: active
tags: [base, feature-inventory, knowledge-graph, claude-code, hooks, sparql, ast, extensions, dashboard, operator-kit, pillars, observatory, shipmap, translation-chain]
relatedTo: [base, operator-kit-47, pillars, observatory, shipmap, paul, graphify, open-ontologies]
---

# BASE — Complete Feature Inventory

Exhaustive extraction of everything BASE (`base` v0.8.0, Rust source at `apps/base-v2/`) does — the CLI surface, the graph architecture, the hook-injection pipeline, the matching/query layer, durability, extensions/plugins, the dashboard, distribution/licensing, and ecosystem integration with the toolbox pillars + `operator-kit-47`.

Every feature has a stable ID (`A1`, `B3`, …) so each can be walked through the **Translation Chain** (Feature → Benefit → Outcome → Identity) in phase 2. This file is the raw Feature-level material. Phase 2 output lands in a sibling doc.

> One-liner that frames the whole thing: **BASE is the intelligence layer Claude Code doesn't have — your code, projects, people, and decisions mapped into one ontological graph and wired into every hook, so the right context injects at the right moment and stays silent otherwise.**

---

## A. Core architecture (what it fundamentally is)

- **A1** — BASE is a single self-contained Rust binary (~20MB, embedded dashboard SPA included) that serves Claude, the hook pipeline, and the operator from one surface.
- **A2** — BASE stores everything in one ontological knowledge graph: RDF triples in Oxigraph (embedded, in-memory, loaded from disk per invocation — no standing server).
- **A3** — BASE queries that graph with SPARQL (SELECT + UPDATE) — deterministic, with zero LLM inference cost in the query path.
- **A4** — BASE persists the graph as NQuads text files (`graph.nq`) — git-native, human-diffable, atomic write-back (temp → validate → rename), validated before commit.
- **A5** — BASE runs a two-tier graph: a global tier (`~/.base-gbl/.base/graph.nq`) and a per-workspace tier (`{workspace}/.base/graph.nq`), merged into one store per hook fire so queries span both tiers.
- **A6** — BASE stamps a configurable namespace identity (`ops:` prefix, `ops-sys.local` ontology URI) on every triple it writes.
- **A7** — BASE models typed ontological relationships with real meaning — `calls`, `importsFrom`, `contains`, `hasMethod`, `belongsTo`, `hasMilestone`, `references`, `relatedTo`, `hasSection` — not flat key-value tags.
- **A8** — BASE is CLI-over-MCP by design: `base hook <event>` reads stdin and writes stdout, so it costs zero standing context (no always-on MCP server eating the window).
- **A9** — BASE is parser-independent at the read layer: a corrupt/malformed line is skipped with a warning rather than blanking the whole graph.
- **A10** — BASE ships its own ontology edge definitions (`ontology/ops-edges.ttl`) and JSON schemas (`schemas/{projects,entities,state,carl}.schema.json`) for structural validation.

## B. The hook-injection pipeline (the core mechanism)

- **B1** — BASE wires four Claude Code hooks automatically at install time: SessionStart, UserPromptSubmit, PreToolUse, PostToolUse.
- **B2** — On **SessionStart**, BASE syncs domains, ingests `paul.toml`/`paul.json` projects from every registered workspace, and runs the session-start signal suite.
- **B3** — On **UserPromptSubmit**, BASE matches the prompt's keywords against domain triggers and injects the matching domain's rules, decisions, and notes straight from the graph.
- **B4** — On **PreToolUse**, BASE injects an AST file-map (entities, key symbols + line numbers, imports, imported-by) before Claude reads/edits a source file — Claude knows the file's shape before the bytes load.
- **B5** — On PreToolUse, BASE intercepts a grep/search attempt and injects an `<ast-hint>` nudging Claude to run a single `base ast query` instead of scanning many files.
- **B6** — On PreToolUse, BASE injects domain rules for matched file paths (path-triggered context, not just keyword-triggered).
- **B7** — On PreToolUse of a `.md` Write/Edit, BASE injects the `<mop-markdown>` extraction contract so Claude authors graph-aware markdown (correct frontmatter, typed tags, `relatedTo`, wikilinks, `@`-mentions) by default.
- **B8** — On **PostToolUse**, BASE injects the section-specific call chain (`calls` / `called-by`) for the exact line range Claude just read — not the whole file.
- **B9** — On PostToolUse, BASE updates last-touch timestamps on the entities involved (feeds the active/deferred reconcile).
- **B10** — Every agent inherits the hooks identically — main session, subagents, Explore agents, workflow agents — so the whole fleet sees the same map and the same graph.
- **B11** — Every hook fails open: it catches all errors, logs to stderr, and exits 0 with empty stdout, so a hook failure never blocks Claude or the prompt.
- **B12** — BASE suppresses repeats within a session: touch the same file twice and the AST map doesn't re-inject; rules already in context don't repeat (dedup state in `.base/.session`). The product is the silence, not the detection.

## C. Session-start signals (proactive orientation)

- **C1** — `active_awareness` signal injects `[Active Projects]` / `[Active Tasks]` — your true current working set.
- **C2** — `pulse` signal injects `<base-pulse>` workspace-grooming health (clean / stale / needs-groom thresholds).
- **C3** — `flow` resurfacing injects items just unblocked because their blocker completed (blocked-by scan).
- **C4** — `flow` resurfacing injects deferred items past their resurface date (deferred-orphan scan).
- **C5** — `flow` mention-threshold scan surfaces recurring ideas once they cross a mention count, prompting promotion to a real project.
- **C6** — `handoff_scan` injects `[Pick up where you left off]` — registered handoff docs that resurface until handled.
- **C7** — `reminder_scan` injects `[Reminders]` that are due now.
- **C8** — BASE injects the operator identity profile (`operator.toml` → North Star, Deep Why, Values, Vision, Pitch) at session start.
- **C9** — BASE enforces a session-start injection budget (`signal.max_chars`) and truncates past it so the opener never floods the window.
- **C10** — BASE scales injection volume by context-bracket (FRESH 1–3 → lean, MODERATE 4–10 → full, DEPLETED 11–20 → force-refresh dedup, CRITICAL 21+) — heavy when context has been compacted, lean when it's fresh.
- **C11** — `flow` protocol injects the static status-lifecycle behavioral rules (backlog → todo → in_progress → blocked/deferred → in_review → completed) governing how Claude moves work.

## D. AST / code-graph engine

- **D1** — BASE extracts code structure with Tree-sitter across 35+ languages (forked from Graphify's extractor) into typed graph triples.
- **D2** — BASE maps every function, struct, class, import, and call relationship across the codebase.
- **D3** — `base ast query --contains <name>` finds any entity by name (the grep replacement for code search).
- **D4** — `base ast query --file <file>` lists every entity defined in a file.
- **D5** — `base ast query --imports <file>` finds everything that imports from a module.
- **D6** — `base ast query --calls <fn>` finds every caller of a function.
- **D7** — `base sync --ast [--target <dir>]` runs the extraction (whole repo or a targeted directory); raw output lands in `ast.ttl` with a `.base-ast-cache`.
- **D8** — "What calls this?" / "what depends on this?" is a SPARQL graph query in BASE, not a multi-file scan — one query, zero inference cost.

## E. Business graph — projects, milestones, tasks, decisions, people, goals, reminders, rules, memory

- **E1** — BASE manages **projects** (`base p`): list / add / update / get — the initiative level.
- **E2** — `base project add` requires `--path`, which auto-creates a domain trigger (mandatory-edges principle: no orphan projects).
- **E3** — BASE resolves any slug-taking command three ways — exact slug, slugified input, then case-insensitive display-name lookup — so `my-app`, `"My App"`, and `"MY APP"` all hit the same entity.
- **E4** — BASE manages **milestones** (`base m`): list / add / update — epics inside a project, dot-notation slugs (`my-app.mvp`).
- **E5** — BASE manages **tasks** (`base t`): list (filter by `--project` or `--milestone`) / add / done — dot slugs (`my-app.fix-x`).
- **E6** — BASE enforces a three-level hierarchy (Project → Milestone → Task) that is fully relational and cross-cutting in the graph.
- **E7** — BASE logs **decisions** with rationale (`base d log --domain --decision --rationale`) as first-class graph entities, searchable via `base d search --keyword`.
- **E8** — BASE manages **entities** (`base e`: add / list) — people and organizations — with a mandatory `--domain` edge.
- **E9** — BASE manages **goals** (`base g`: add / list / update) with targets/deadlines and a metric model (target / current / metric) that powers goal-health surfacing.
- **E10** — BASE manages **reminders** (`base r`: add / list) with due dates; reminders resurface at session start, including time-gated and persistent variants.
- **E11** — BASE manages **rules** (`base rule`: add / list / remove) that live in the graph (not config files) and fire by domain.
- **E12** — `base learn` writes structured memory typed as `insight | correction | decision | commitment | shift`, with a required `--domain` edge plus optional `--project` / `--entity` edges.
- **E13** — `base learn` also records mentions of an existing note (`--mention <slug> --context`), and supports `--update`, `--remove`, and `--list`.
- **E14** — `base recall` searches notes by `--keyword` / `--domain` / slug and stamps `lastRead`, which resets that note's purge clock (only notes you never reach for age out).
- **E15** — BASE enforces mandatory edges everywhere (`learn`, `entity add`, `project add` all require a link) — the graph can't accumulate disconnected orphans.

## F. Documentation graph (markdown becomes a connected node)

- **F1** — `base sync` extracts markdown frontmatter (`type`, `status`, `tags`, `relatedTo`) into the graph.
- **F2** — BASE parses the markdown **body**, not just the YAML: headings become `hasSection` edges (navigable structure + search).
- **F3** — BASE turns `[text](path.md)` links into document-to-document `references` edges.
- **F4** — BASE turns `[[wikilinks]]` into entity-reference edges.
- **F5** — BASE turns `@path/to/file` mentions into file-reference edges.
- **F6** — BASE turns each tag into an individual queryable node (not a comma-separated blob).
- **F7** — BASE defines the Markdown Ontology Protocol (MOP) and teaches it just-in-time via the pre-tool hook, so every doc Claude writes is graph-ready on the next sync.
- **F8** — `base sync --incremental` re-extracts only changed files.

## G. Domains (the deterministic matching layer)

- **G1** — `domains.toml` declares when a domain fires: `prompt_keywords`, `file_keywords`, `paths`, `exclude`, `rules`, `query`, `query_format`, and `mode` (`always` | `triggered`).
- **G2** — BASE matching is deterministic — keywords, paths, excludes — no embeddings, no semantic/fuzzy matching in the core loop; a trigger fires exactly when it matches.
- **G3** — BASE loads the global `domains.toml` first and overlays the workspace `domains.toml` by name (two-tier domain config).
- **G4** — BASE supports an `always`-mode domain (GLOBAL) plus any number of `triggered` domains (e.g. DEVELOPMENT, PROJECTS, BACKLOG).
- **G5** — `base domain` manages domains: `add-trigger`, `remove-trigger`, `list`, `get`, `create`, `remove`, and `sync` (push `domains.toml` → graph, optionally migrating `carl.json` decisions).
- **G6** — A domain can fire a SPARQL query on match (`query = "name"`, `query_format = list | table | prose`), injecting live graph results as context.
- **G7** — BASE keeps rule *content* in the graph and `domains.toml` as triggers-only — config stays thin, knowledge stays relational.

## H. On-demand context + the SPARQL query library

- **H1** — `base context --keyword <kw>` pulls targeted graph context on demand, using the exact same engine as the automatic hook injection.
- **H2** — BASE resolves query files from `{workspace}/.base/queries/*.sparql` then `~/.base-gbl/queries/*.sparql` (workspace wins).
- **H3** — BASE substitutes `{{prefix}}` / `{{uri}}` into queries before execution and formats results by `?label`/`?name`/`?text` + `?detail`/`?value` conventions into list / table / prose.
- **H4** — BASE supports both static capability maps (SPARQL `VALUES` blocks, no graph lookup) and live graph queries from the same contract.
- **H5** — BASE never interpolates user input into queries (only `{{prefix}}`/`{{uri}}`), so query-triggered injection is injection-safe; empty results inject nothing; parse errors fail open with a stderr warning.
- **H6** — BASE ships a wired offer/audience query pack: `icp-context`, `offer-brief`, `offer-capabilities`, `offer-objections`, `offer-tiers`, `offer-transformation` — domain keywords fire the matching SPARQL and inject the relevant slice of the offer graph.

## I. Graph durability, recovery & maintenance

- **I1** — BASE writes the graph atomically (temp → validate → rename) so an interrupted write can't corrupt it.
- **I2** — BASE splits read/write strictness: reads are lenient (skip malformed lines, warn) so one bad line never blanks context; writes are strict and refuse to run on an unhealthy graph so it's never silently rewritten with data dropped.
- **I3** — `base doctor` runs a parser-independent health scan across both tiers; `--json` for agents; exits nonzero when unhealthy.
- **I4** — `base doctor --repair` self-heals: quarantines malformed lines and atomically rewrites the good set (snapshots first).
- **I5** — `base doctor --restore` lists backup snapshots; `--restore <backup>` rolls one back.
- **I6** — `base graph compact` dedups + canonicalizes the graph (atomic, idempotent).
- **I7** — `base graph purge --stale` previews notes unread past N days (default 21, `--days` to tune); `--apply` deletes — and a note's clock resets every time `recall` reaches it.
- **I8** — Every repair / restore / compact / purge snapshots first to `graph.nq.bak-<op>-<date>` and keeps the newest 10.
- **I9** — BASE guarantees "never hand-edit the graph" is enforceable — all maintenance routes through atomic, validated commands.

## J. Memory persistence

- **J1** — BASE persists Claude's auto-memory in one of three modes: `claude` (flat files), `base` (graph only), or `both` (mirror).
- **J2** — `base memory list` reviews Claude's flat-file memories (name, type, description, path); `base memory purge` removes flat files already confirmed in the graph — the flat-files → graph migration path.

## K. Operator identity

- **K1** — `base operator init --name` creates the operator profile at `~/.base-gbl/operator.toml`; `base operator show` prints it.
- **K2** — `operator.toml` (name, north_star, deep_why, values, vision, pitch, `active` toggle) injects on every session start so every agent knows who it works for and what the objective is.

## L. Handoffs & session continuity

- **L1** — `base handoff create --project --doc` registers a handoff doc and archives any prior open handoff for that project.
- **L2** — `base handoff list` lists handoffs across global + workspace tiers; `snooze` hides one for N days; `archive` stops it resurfacing.
- **L3** — BASE resurfaces the registered handoff as `[Pick up where you left off]` at session start until it's picked up, snoozed, or archived.
- **L4** — The `*handoff` star command runs the full flow: PAUL-aware (defers to `/paul:handoff` when `.paul/` exists), else synthesizes a PAUL-style handoff doc, writes it, and registers it — no confirmation needed.

## M. Config & active/deferred protocol

- **M1** — `base.toml` is fully sectioned and self-documenting: `namespace`, `devmode`, `bracket`, `signal`, `sync`, `flow`, `memory`, `protocol`, plus the `[[workspace]]` registry — every section toggleable with `enabled = false`.
- **M2** — `base config get / set / list` reads and writes config via dot-notation (`section.key`).
- **M3** — DEVMODE injects a per-prompt telemetry block (bracket, which domains loaded and why, dedup count, tools) so you can tune `domains.toml` until the right context fires at the right time.
- **M4** — The `protocol` engine reconciles active⇄deferred: at session start it sets each project's `lastActive` from its folder's newest file, auto-defers working projects gone cold past `stale_days`, and revives touched ones — keeping `[Active Projects]` honest.
- **M5** — `base reconcile` runs that active/deferred reconcile on demand from real folder last-touch.

## N. Workspace registry & scaffolding

- **N1** — The `[[workspace]]` registry in `base.toml` lists every workspace root; BASE scans them all for `paul.toml` projects at session start (registry-aware resolution across all workspaces).
- **N2** — `base workspace list` shows the registry; `base workspace sync` regenerates the registered-workspaces block inside `~/.claude/CLAUDE.md` from `base.toml`.
- **N3** — `base scaffold [path]` stands up a new workspace: creates `.base/`, writes `domains.toml` + default config, and registers it globally.

## O. Star commands (operator working modes)

- **O1** — BASE ships a `commands.toml`-driven star-command system: typing `*NAME` injects a packaged behavioral ruleset. Managed by `base commands list / show / add / remove / import` (import is append-only, never alters preceding content).
- **O2** — Cognitive/working modes: `*BLUNT` (answer-first, no hedging), `*ANALYTICAL` (tables + cited reasoning), `*STEELMAN` (strongest-version + counter), `*AUDIT` (skeptical, find problems first), `*MENTOR` (teach-first), `*OPERATOR` (ROI/business framing), `*EDITOR` (tighten-only, voice-matched), `*DEBUG` (hypothesis-test loop), `*DISCUSS` (explore before acting), `*MENTOR`/`*META` (work ON it, extract systems).
- **O3** — Action modes: `*BRIEF` (session report), `*DEV` (force DEVELOPMENT domain), `*JERRY` (sermon transcribe + YouTube metadata), `*handoff` (run the handoff flow).
- **O4** — Meta-OS modes: `*OBSERVE` runs the workspace observatory; `*SHIPMAP` runs the 13-pillar shippability audit — both are BASE-native operators surfaced as star commands.

## P. Extensions (frameworks plug into the hook pipeline)

- **P1** — A framework wires itself into BASE's hooks by dropping one TOML file in `~/.base-gbl/extensions/{name}.toml` — auto-discovered, no registration step; file exists = active, delete = disabled; rescanned on every hook fire.
- **P2** — `base extension list / validate / install / remove` manages extensions (alias `base ext`).
- **P3** — An extension can bind **session_start**: run SPARQL `queries`, `ingest` JSON state files into the graph as RDF entities (`upsert`/`replace`), and `inject` a templated status line (e.g. "Outpost: {piece_count} pieces").
- **P4** — An extension can bind **user_prompt**: declare `domains` (keywords + rules) that merge into the normal matching pool and inherit dedup, bracketing, and matching for free.
- **P5** — An extension can bind **pre_tool**: declare path `triggers` that inject context when files under those paths are touched.
- **P6** — An extension can bind **post_tool**: `handlers` that react to file writes with `action = reingest | log | query | inject`.
- **P7** — The post_tool `inject` action is the verify-reflex (v0.6.0): nudge Claude to do something *after* it writes a matching file, `once_per_session` (e.g. "design work detected → run /design-humanizer scan"); ships a built-in `designset` design-file detector.
- **P8** — Extension domains carry `source = "ext:{name}"` markers in the graph for independent garbage collection, and ingested JSON state becomes SPARQL-queryable RDF.
- **P9** — Live shipped extensions: `outpost` (content pipeline — multi-query session-start, 8 JSON ingests, 3 domains, pre/post handlers), `design-humanizer` (verify-reflex), `nano-banana` (command plugin).

## Q. Command plugins (drop-in `base <foo>` subcommands)

- **Q1** — An extension manifest's `[[commands]]` entry contributes a brand-new `base <name>` subcommand routed to an executable handler (script or binary, any language), with args forwarded.
- **Q2** — Core commands always win — a plugin can never shadow a built-in.
- **Q3** — BASE injects an env contract to every plugin: `BASE_WORKSPACE`, `BASE_GRAPH_PATH`, `BASE_GLOBAL_DIR`, `BASE_BIN`, plus every secret in `~/.base-gbl/.env`.
- **Q4** — Plugins mutate state only by calling back through `$BASE_BIN`, so BASE stays the sole graph writer; stdio is inherited so a `--json` handler line flows straight to the caller.
- **Q5** — `base ext run <name> …` is the explicit collision-proof invocation; `base ext list` shows installed plugin commands.
- **Q6** — Plugins install **linked** (run from the repo — for dev) or **packaged** (`base ext install --bundle <manifest>` copies the handler into `~/.base-gbl/plugins/<name>/` and repoints the manifest — repo-independent, the shippable artifact).

## R. Secrets

- **R1** — `base secret set` prompts with echo OFF (masked, paste-friendly), writes `~/.base-gbl/.env` at `0600`, and never echoes the value.
- **R2** — `base secret list` shows stored key names with masked values (never the full secret); `base secret rm` removes one. Plugins read these from their environment — secrets never get typed into chat.

## S. Command Center dashboard

- **S1** — `base dashboard` (`base dash`) starts an embedded HTTP server (SPA compiled into the binary — no npm, no separate server, no config), opens the browser, and serves entirely from localhost.
- **S2** — **Graph Explorer** renders the live graph as an interactive force-directed network, color-coded by node type; click any node for properties + incoming/outgoing edges; search and filter by type; add operator notes that persist into the graph (and show up in the next Claude session).
- **S3** — **Operations** shows projects/milestones/tasks as a kanban (active / blocked / completed / pending) or sortable table; drag a card and the status updates in the graph instantly; shows decisions with rationale and overdue reminders.
- **S4** — **Session Activity** is a live WebSocket feed of every hook event across all sessions, grouped by session boundary (prompts, tool calls, domains matched, rules injected, deduped), with a live badge on the current session and surfaced errors.
- **S5** — **Usage Analytics** (token/cost/model distribution) — planned (Plan 04). Every panel reads the same SPARQL-backed API as the hooks — one graph, multiple surfaces.

## T. Install, distribution, updates & licensing

- **T1** — `base install` copies the binary to `~/.local/bin/base`, creates `~/.base-gbl/`, wires the four hooks into `~/.claude/settings.json`, adds a CLI-reference section to `CLAUDE.md`, and writes the component manifest.
- **T2** — `base install` flags: `--carl <path>` (migrate `carl.json` decisions), `--skip-hooks` (skip settings wiring), `--full` (register all ChrisAI components — PAUL, SEED, SKILLSMITH).
- **T3** — `base uninstall` removes the hooks from `settings.json`, the binary, and the CLAUDE.md section.
- **T4** — `base update` checks for, snoozes, or installs available updates; the check is TTL'd (`manifest.toml.update_check`, default 7-day) and tracks pending component updates.
- **T5** — `manifest.toml` is the ChrisAI installer registry: components (base, paul, seed, …) with versions/paths/install timestamps, an install token, and update-check state — the backbone for the `npx chrisai@latest` installer.
- **T6** — `base activate` takes a Skool classroom key to remove attribution (free → registered tier).
- **T7** — `license.toml` holds the paid Operator-Kit license + validation state (license key, bound email, activation token, product, last validation result) — BASE re-validates it; this is the monetization/gating layer.

## U. Ecosystem integration (how BASE ties the whole operating system together)

- **U1** — **PAUL**: BASE ingests `paul.toml`/`paul.json` as projects and satellites at session start (`extract/paul_json.rs`, `extract/ledger.rs`), and the handoff flow defers to `/paul:handoff` whenever a `.paul/` directory exists — PAUL is the planning engine, BASE is the memory/context layer underneath it.
- **U2** — **CARL**: `base install --carl` and `base domain sync` migrate `carl.json` decisions into the graph — BASE absorbs the predecessor decision system.
- **U3** — **operator-kit-47** is the shipped product BASE comes with: the **$47 Operator Kit**, installed via `npx chrisai@latest` and license-gated, sitting at the front of the funnel. It runs an operator chain — `/business-context` → `/os-config` → `/claude-architect` → `/calibrate-voice` → `/brand-design` → `/leverage-score` — that configures a complete BASE + Claude Code operating system in ~90 minutes.
- **U4** — operator-kit-47 ships four BASE frameworks (`base-frameworks/{brand-design, business-context, claude-architect, os-config}`), each a full skill bundle (tasks / templates / frameworks / checklists / context).
- **U5** — **os-config** is the BASE-wiring operator: it scaffolds BASE, writes `domains.toml` + rules, classifies the operator's domain profile (Creator / Agency / SaaS / Custom), runs v1→v2 migration, and applies `settings.json` templates per profile.
- **U6** — `os-config wire` runs the wire-up layer: it reads each active pillar's `connections.toml` and provisions external connections (install MCP servers, OAuth/`gcloud` auth, deploy Railway templates) across three infra tiers — 0 native (local), 1 connect (free SaaS via MCP/CLI/OAuth), 2 self-host (~$5 Railway box).
- **U7** — **Pillars**: the toolbox carries pillars `00`–`07` on disk against a universal **13-pillar** schema (`scripts/shipmap/pillars.toml`, the "Solo Operator Business Kit"); Pillar 13 (Meta-OS) is the orchestration layer that ties the other 12 together. A pillar is a *container of frameworks*, frameworks typed by `kind` + `composes`.
- **U8** — `connections.toml` (per pillar) declares 3rd-party dependencies — `tier`, `wire_method` (mcp · cli · oauth · railway-template · skill), `wire_ref`, `required`, `purpose` — and the **Leverage Score** judges connection completeness (is Claude wired to your business's "hands"), not just whether frameworks are installed.
- **U9** — **Framework SOP trio**: every shippable framework ships `component.json` (install contract — what installs where), `bin/install.js` (the standard installer, copied from `_framework-template`, never hand-rolled), and `hygiene.json` (orphan contract — version + append-only `retired[]`). This is how skills/frameworks graduate `apps/` → `toolbox` and install to the flat runtime (`~/.claude/`, `~/.base-frameworks/`).
- **U10** — **observatory** (Pillar 13 generator, `~/.base-gbl/scripts/observatory`, surfaced via `*OBSERVE`): inventories every registered base workspace and flags v1-cleanup, unregistered, git-local-only, uncommitted, and stale-planning states — reads the `[[workspace]]` registry so it's portable to any install.
- **U11** — **shipmap** (Pillar 13 generator, `scripts/shipmap`, surfaced via `*SHIPMAP`): audits 13-pillar shippability by reading the universal `pillars.toml` + the operator's `assets.toml` tags and mechanically computing ship-readiness (toolbox-vs-apps location, packaging, PAUL-complete) → the ship-now list vs what's stranded in `apps/`.
- **U12** — **graphify** (toolbox app): BASE forked Graphify's Tree-sitter extractor for its AST pass; graphify itself also lives in the toolbox as a graph-visualization app.
- **U13** — **open-ontologies** (toolbox MCP server, Rust/Oxigraph): the heavyweight ontology sibling to BASE's embedded graph — 40+ OWL/RDF/SPARQL MCP tools (validate, load, query, reason, SHACL, embeddings, alignment) plus an index pipeline that discovers PAUL `paul.json` projects and compiles markdown into an Obsidian-compatible entity registry. Same Oxigraph foundation as BASE.
- **U14** — **Component registry**: BASE registers and version-tracks the sibling ChrisAI tools (PAUL, SEED, SKILLSMITH) via `manifest.toml`, positioning itself as the meta-OS layer (Pillar 00 / 13) beneath the entire framework ecosystem.
- **U15** — Legacy bridges (`.base/base-mcp/`, `.base/carl-mcp/`): BASE began with thin MCP wrappers but moved to the CLI-over-MCP design (A8); the MCP bridges remain for backward compatibility while the binary is the source of truth.

---

## Translation Chain — phase 2 (pending)

Each feature above is the **Feature** rung. Phase 2 walks the highest-leverage IDs through:

```
Feature → Benefit → Outcome → Identity
```

Worked reference (BASE query-triggered injection, ≈ H1/B3):

| Level | Statement |
|---|---|
| **Feature** | Query-triggered injection runs live SPARQL against your knowledge graph when domain keywords or filepaths match, injecting formatted results before Claude acts. |
| **Benefit** | Claude automatically knows what it needs to know — decisions, patterns, rules, prior art — without you remembering to tell it. |
| **Outcome** | You stop repeating yourself across sessions and stop catching Claude making the same mistake twice. The institutional memory is just *there*. |
| **Identity** | You're the operator whose AI remembers everything and applies it without being asked — your system gets smarter every time you use it. |

> Next: pick the IDs that carry the positioning weight (the "too powerful to describe" ones) and run the chain on each. Technical people stop at Feature; this doc starts there so phase 2 can climb to Identity.
