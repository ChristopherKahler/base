# BASE

The intelligence layer Claude Code doesn't have.

Your codebase, your projects, your people, your decisions, your business - mapped into one ontological graph and wired directly into Claude's hook pipeline. When Claude touches a file, it already knows what that file contains, what calls it, and what depends on it. When Claude greps for code, the graph intercepts and says "I already know where that is." When you ask about a project, it knows the milestones, the tasks, who's involved, and what decisions led to the current state.

This isn't a CLAUDE.md file. It isn't a prompt you paste. It's a live knowledge graph that tracks everything across every workspace and injects exactly the right slice of context at exactly the right moment - then goes silent until something changes.

Your code. Your projects. Your people. Your decisions. One graph. Every session. Every agent. Automatic.

```
base p l                          # list your projects
base a q -c "auth"                # find any entity named "auth" in the graph
base t a -p myapp -n "Fix login"  # add a task to a project
```

You can run these yourself in a Claude Code session with `!` prefix, or Claude runs them automatically through hooks. Same binary, same graph, same data.

## Why this exists

Claude Code is the best coding agent on the planet. But it starts every session blind. It doesn't know your codebase structure. It doesn't know your active projects. It doesn't know the decision you made last week about auth patterns. It doesn't know your teammate is blocked on the API. It doesn't know what files depend on the module you're about to change.

CLAUDE.md helps. But it's a static document you wrote once. It doesn't know what you're touching right now. It doesn't adapt. It doesn't suppress itself when irrelevant.

BASE is the layer underneath - the one that turns Claude Code from a coding assistant into a business-aware operating system. It maps your entire operation into an ontological graph (code structure, projects, milestones, tasks, people, decisions, domain rules) and wires that graph into every hook in the pipeline. Session start, prompt submit, pre-tool-use, post-tool-use - at every stage, the graph surfaces what's relevant and suppresses what isn't.

Every agent gets it. Main session, subagents, explore agents, workflow agents - they all inherit the same hooks, the same graph, the same intelligence. Your whole fleet sees the same map.

You can run these yourself in a Claude Code session with `!` prefix, or Claude runs them automatically through hooks. Same binary, same graph, same data.

## What the graph knows

BASE maps three layers into one queryable graph:

**Your code** - every function, struct, class, import, and call relationship across 35+ languages. Tree-sitter extracts the structure, SPARQL makes it queryable. "What calls this function?" is a graph query, not a grep.

**Your business** - projects with milestones and tasks. People and their roles. Decisions and the rationale behind them. Goals and deadlines. Domain rules that fire when you're working in the right context. This is the stuff that lives in your head or in scattered docs - BASE puts it in the graph where every agent can reach it.

**Your documentation** - every markdown file with frontmatter becomes a connected node in the same graph. Not just metadata in a YAML block - the body is parsed too. Headings become navigable sections. `[links](path/to/file.md)` become document-to-document edges. `[[wikilinks]]` become entity references. `@path/to/file` mentions become file edges. Tags become individual queryable nodes, not a comma-separated blob. When Claude writes or edits a markdown file, the pre-tool-use hook injects the extraction contract so Claude authors graph-aware markdown by default - it knows what patterns the graph will pick up downstream.

**Your operations** - which domains are active, what rules apply where, which files are stale, what changed since last session. Signals that fire at session start to orient Claude before you type a word.

All of it is relational. A project has milestones. A milestone has tasks. A task touches files. Those files contain functions. Those functions call other functions in other files that belong to other projects. One graph. Cross-cutting. Queryable.

## What this looks like in practice

You tell Claude to fix a bug in the auth module. Claude reaches for `auth.rs`. Before the file content even loads, the pre-tool-use hook fires and injects:

```
[AST] auth.rs - 8 entities
  Key: fn login (line 12), fn validate_token (line 45), struct AuthConfig (line 3)
  Imports: config.rs, database.rs
  Imported by: api.rs, middleware.rs, cli.rs
```

Claude didn't ask for that. No tool call, no extra prompt. The hook saw the file path, queried the graph, and gave Claude the full shape of the file before it read a single line. Claude now knows there are 8 entities, what they are, where they sit, and which other files depend on this one.

Claude reads lines 45-80 to look at `validate_token`. Post-tool-use fires:

```
[AST] Lines 45-80: fn validate_token
  Calls: decode_jwt, check_expiry, load_user
  Called by: middleware.rs -> require_auth()
```

Just for those 35 lines. Not the whole file. Not a report. Claude gets the exact call chain for the exact code it's looking at. It already knows that changing `validate_token` will affect `middleware.rs` without having to search for it.

Claude tries to grep for a related function. The hook intercepts:

```
<ast-hint>
AST graph available for this workspace. Try:
  base ast query --contains "validate_token"
The graph knows file locations, line numbers, and call relationships.
</ast-hint>
```

One SPARQL query instead of Claude scanning 15 files looking for a match. The graph already mapped the entire codebase - Claude just needs to ask it.

You ask Claude to create a design doc. Claude reaches for Write on `docs/auth-redesign.md`. Before it writes a single byte, the pre-tool-use hook fires:

```
<mop-markdown>
This markdown file feeds a knowledge graph. Structure it for extraction:

FRONTMATTER (between --- delimiters):
  type: doc|decision|note|spec|plan|summary
  status: draft|active|complete|archived
  tags: [specific, searchable, terms]
  relatedTo: [entity-slug-1, entity-slug-2]

BODY PATTERNS (extracted as graph edges — use intentionally):
  ## Headings        → hasSection edges (document structure + search)
  [text](path.md)    → references edges to other documents
  [[entity-name]]    → references edges to named entities
  @path/to/file      → references edges to documents
  Tags become individual graph edges — be specific, not generic
  relatedTo links to real entity slugs — check existing entities
</mop-markdown>
```

Claude didn't read a style guide. It didn't memorize a convention. The hook taught it the extraction contract at the exact moment it was about to write. So the doc it creates has proper frontmatter with typed tags, `relatedTo` edges pointing to real entities, `[[wikilinks]]` to connect concepts, and `@src/auth.rs` references to the code it's documenting. Next sync, that document becomes a fully connected graph node - linked to its tags, its related entities, the code it references, and the other docs it mentions. Every agent that touches that file later inherits those connections.

## How this compares

**LSP** tells you where a symbol is defined. You point at a symbol, it answers. One language at a time. No project context, no business context, no session memory, no idea what Claude already looked at this session.

**Graphify** (58k stars, ~2 months old, YC S26) extracts code into a NetworkX JSON graph and generates a report. For code-only projects, it's tree-sitter AST output organized into communities via Leiden clustering - functions, classes, imports, call edges. The "knowledge graph" part only appears when you feed it docs/papers/images and pay for an LLM pass (Claude, Gemini, OpenAI, etc.) to annotate semantic relationships.

Querying is keyword matching against node labels (TF-IDF weighted string matching - exact, prefix, substring), not semantic search. `graphify query "how does auth work"` splits that into words and matches nodes with "auth" in the label. It doesn't understand the question. And every query is manual - you type `/graphify query` or the AI has to remember to ask. The PreToolUse hook nudges toward queries on grep, but only on Bash calls - if Claude uses Read to explore files, the hook doesn't fire.

**BASE** doesn't wait for anyone to ask. The graph is wired into every hook in the pipeline. Claude touches a file, the map injects. Claude reads a section, the call chain injects. Claude greps, the intercept fires. No manual queries needed, no LLM in the query path, no tokens spent on lookups.

|  | LSP | Graphify | BASE |
|---|---|---|---|
| What it maps | Symbols in one language | Code structure (AST) + LLM annotations on docs | Code + projects + milestones + tasks + people + decisions + rules |
| How you query | Point at a symbol | Keyword match against node labels | SPARQL - deterministic, zero inference cost |
| How context flows | You ask, it answers | You ask, it answers | Automatic - hooks inject on file touch |
| When it speaks | When asked | When asked | When relevant. Silent otherwise. |
| Section awareness | Symbol-level | None | Knows the exact lines Claude just read |
| Session memory | None | None | Tracks what it injected, never repeats |
| Subagent support | None | Manual per-agent | Automatic - every agent inherits hooks |
| Graph model | Language server index | NetworkX JSON (nodes + edges) | RDF triples with typed ontological relationships |
| Business context | None | None | Projects, milestones, tasks, people, decisions, domain rules |
| Doc extraction | None | None | Frontmatter + body (headings, links, wikilinks, @-mentions, tags, relatedTo) |
| Authoring guidance | None | None | Hook injects extraction contract on Write/Edit so agents author graph-aware docs by default |
| Query cost | Free (local) | Free for code-only; LLM tokens for semantic pass | Free (SPARQL, no LLM) |

BASE uses the same tree-sitter extraction pipeline as Graphify for the AST pass (we forked their extractor). What happens after extraction is completely different. Graphify stores nodes and edges in a flat JSON file and queries via string matching. BASE loads typed triples into an RDF ontological graph where relationships have meaning (calls, importsFrom, contains, hasMethod, belongsTo, hasMilestone) and queries via SPARQL - the same query language that powers Wikidata and every serious knowledge graph. And the graph isn't just code. It's your entire operation - projects, people, decisions, rules - all relational, all cross-cutting, all queryable the same way.

## Quick start

```bash
# build
git clone <repo-url>
cd base-v2
cargo build --release

# install (copies binary, creates config, wires hooks)
./target/release/base install

# scaffold your workspace
cd ~/my-workspace
base scaffold
```

That's three commands. `base install` handles everything: binary goes to `~/.local/bin/base`, global config goes to `~/.base-gbl/`, hooks get wired into `~/.claude/settings.json`, and your CLAUDE.md gets a CLI reference section so Claude knows the commands exist.

`base scaffold` creates `.base/` in your workspace with domains.toml and default config.

## Quick win - see it work in 5 minutes

After install, try this sequence:

**1. Add a project**
```bash
base p a -n "My App" -p "src"
```

**2. Extract the codebase structure**
```bash
base sync --ast --target src
```

This runs tree-sitter across your source files (supports 35+ languages) and loads every function, struct, class, import, and call relationship into the graph.

**3. Query the graph**
```bash
# what's in a specific file?
base a q -f "main.rs"

# where does a function live?
base a q -c "handle_request"

# what imports from this module?
base a q -i "database.rs"

# who calls this function?
base a q --calls "validate"
```

**4. Open a Claude Code session in your workspace**

Read any source file. Watch the AST map appear automatically in the hook output. That's the graph doing its job without you asking.

**5. Run commands inside your Claude session**

Type `!` followed by any base command:

```
! base p l                    # see your projects
! base a q -c "auth"          # find entities
! base t a -p myapp -n "Todo" # add a task
! base m a -p myapp -n "MVP"  # add a milestone
```

You manage your projects, tasks, milestones, and codebase graph from inside the same session where Claude is working. No switching tools. No separate dashboard.

## Command reference

Every command has a short alias. Long flags have short versions too.

### Projects (initiative level)

```bash
base project list                            # or: base p l
base project add --name "X" --path "src/x"   # or: base p a -n "X" -p "src/x"
base project update myapp --status blocked    # or: base p u myapp -s blocked
base project get myapp                       # accepts slug OR display name
```

### Milestones (epic level)

```bash
base milestone list --project myapp          # or: base m l -p myapp
base milestone add --project myapp \
  --name "MVP" --description "Core features" # or: base m a -p myapp -n "MVP" -d "Core features"
base milestone update myapp.mvp --status done # or: base m u myapp.mvp -s done
```

### Tasks

```bash
base task list                               # or: base t l
base task list --project myapp               # or: base t l -p myapp
base task list --milestone myapp.mvp         # or: base t l -m myapp.mvp
base task add --project myapp --name "Fix X" # or: base t a -p myapp -n "Fix X"
base task add --project myapp --name "Fix X" \
  --milestone myapp.mvp                      # or: base t a -p myapp -n "Fix X" -m myapp.mvp
base task done myapp.fix-x                   # mark complete
```

### AST graph queries

```bash
base ast query --contains "auth"             # or: base a q -c "auth"
base ast query --file "main.rs"              # or: base a q -f "main.rs"
base ast query --imports "config.rs"         # or: base a q -i "config.rs"
base ast query --calls "handle_request"      # find callers
```

### Decisions, rules, memory

```bash
# decisions with rationale (base d)
base decision log --domain myapp \
  --decision "Use Postgres" --rationale "Team knows it"  # or: base d log ...
base decision search --keyword "database"                 # or: base d search ...

# rules live in the graph, not config files
base rule add --domain DEVELOPMENT --text "Always validate inputs at boundaries"
base rule list --domain GLOBAL

# structured memory with relational edges
base learn --text "Auth uses JWT with 15min expiry" --domain myapp --type decision
base recall --keyword "auth"
```

### Entities, goals, reminders

```bash
base entity add --name "Alice" --type person --domain myapp  # or: base e a ...
base entity list                                              # or: base e l

base goal add --name "Ship v1" --target "2026-07-01"          # or: base g a ...
base goal list                                                # or: base g l

base reminder add --name "Review PR" --due 2026-06-10         # or: base r a ...
base reminder list                                            # or: base r l
```

### All command aliases

| Full command | Short form |
|---|---|
| `base project` | `base p` |
| `base milestone` | `base m` |
| `base task` | `base t` |
| `base ast` | `base a` |
| `base decision` | `base d` |
| `base entity` | `base e` |
| `base goal` | `base g` |
| `base reminder` | `base r` |

Subcommands: `add` = `a`, `list` = `l`, `update` = `u`, `query` = `q`

### Sync and AST extraction

```bash
base sync                        # extract markdown metadata + body structure into graph
base sync --incremental          # only changed files
base sync --ast                  # extract code structure (tree-sitter, 35+ languages)
base sync --ast --target src     # target a specific directory
```

Markdown sync extracts frontmatter fields (type, status, tags, relatedTo) AND body structure (headings, markdown links, wikilinks, @-mentions). Every markdown file becomes a connected graph node with real edges to other documents and entities - not just an isolated blob with a title.

## Hierarchy: projects, milestones, tasks

Three levels. Projects are the top (an initiative or app). Milestones group work inside a project (like an epic). Tasks are the individual things to do.

```
Project: "My App"
  └── Milestone: "MVP"
       ├── Task: "Build auth"
       ├── Task: "Create API"
       └── Task: "Write tests"
  └── Milestone: "Launch"
       ├── Task: "Deploy to prod"
       └── Task: "Write docs"
```

Slugs use dot notation: project slug is `my-app`, milestone is `my-app.mvp`, tasks are `my-app.build-auth`.

Every command that takes a slug also accepts the display name. The system tries three things in order: exact slug match, slugify your input, then case-insensitive name lookup against the graph. All three resolve to the same entity. So these are identical:

```bash
base p u my-app -s blocked          # slug
base p u "My App" -s blocked        # display name
base p u "MY APP" -s blocked        # case doesn't matter
```

## How the hooks work

Four hooks, all wired automatically by `base install`:

| Hook | Fires when | What it does |
|------|-----------|--------------|
| SessionStart | New session opens | Syncs domains, ingests projects, runs signals |
| UserPromptSubmit | You type a prompt | Matches keywords against domains, injects rules from graph |
| PreToolUse | Claude is about to read/edit a file | Injects AST file map for source files. Intercepts grep with graph hint. Injects domain rules for matched paths. Injects markdown extraction contract on Write/Edit of .md files so Claude authors graph-aware documents by default. |
| PostToolUse | Claude finishes reading | Updates timestamps. Injects section-specific AST context for partial reads. |

All hooks fail open. If anything errors, it logs to stderr and exits with empty stdout. Claude is never blocked by a hook failure.

## What lives where

```
~/.base-gbl/                  # global tier (all workspaces)
├── .base/graph.nq            # global tier graph
├── domains.toml              # global domain triggers
├── operator.toml             # your identity profile
├── base.toml                 # global config
└── docs/                     # reference docs

~/my-workspace/.base/         # workspace tier (one per workspace)
├── graph.nq                  # the knowledge graph (NQuads)
├── domains.toml              # workspace domain triggers
├── ast.ttl                   # raw AST extraction output
├── base.toml                 # workspace config
└── .session                  # ephemeral (dedup tracking, prompt count)
```

Global domains.toml loads first, workspace overlays by name. Hooks load the global and workspace graphs into one merged store, so queries span both tiers.

## Domain matching

`domains.toml` defines when a domain fires:

```toml
[[domain]]
name = "BACKEND"
mode = "triggered"
prompt_keywords = ["api", "endpoint", "database", "query"]
file_keywords = ["use crate", "impl", "async fn"]
paths = ["src/api/", "src/db/"]
rules = ["Always validate inputs at API boundaries"]
query = "backend-context"        # optional: fire .base/queries/backend-context.sparql on match
query_format = "list"            # table | list | prose

[[domain]]
name = "FRONTEND"
mode = "triggered"
prompt_keywords = ["component", "page", "style", "layout"]
paths = ["src/ui/", "src/components/"]

[[domain]]
name = "GLOBAL"
mode = "always"
```

When you mention "api" in a prompt, the BACKEND domain fires and its rules inject. When Claude opens a file under `src/api/`, same thing. The graph provides the rules, decisions, and notes associated with that domain. Matching is deterministic - keywords and paths, nothing fuzzy.

## DEVMODE

Set `DEVMODE=true` in your environment and every prompt injection includes a telemetry block:

```
🔧 DEVMODE
Bracket: [FRESH] (prompt 3)
Loaded: GLOBAL [always] (13 rules), BACKEND [keyword] (4 rules)
Available: FRONTEND, TESTING, DEPLOYMENT
Dedup: 2 skipped
Tools: Read, Edit
```

Shows you exactly which domains fired, why they fired, what was deduped, and what's available but didn't match. You use this to tune your domains.toml until the right context fires at the right time.

## Context brackets

BASE tracks how deep you are in a session:

| Bracket | Prompts | Behavior |
|---------|---------|----------|
| FRESH | 1-3 | Lean injection, skip verbose context |
| MODERATE | 4-10 | Full injection |
| DEPLETED | 11-20 | Force-refresh dedup on interval |
| CRITICAL | 21+ | Same as depleted |

Thresholds are configurable in `base.toml`. The idea: early in a session you don't need heavy injection (Claude has fresh context). Later, context has been compacted or summarized, so the system re-injects what matters.

## Command Center Dashboard

```bash
base dashboard
```

One command. Browser opens. Your entire operation - visualized.

**Graph Explorer** - your knowledge graph rendered as a live, interactive force-directed network. Nodes color-coded by type (code in blue, projects in green, people in orange, decisions in yellow). Click any node to see its properties, relationships, incoming and outgoing edges. Search across entities. Filter by type. Add operator notes that persist in the graph. This isn't a static diagram you exported from a tool - it's your actual running graph, queryable and navigable.

**Operations** - your projects, milestones, and tasks as a kanban board or sortable table. Four columns: active, blocked, completed, pending. Drag a card between columns and the status updates in the graph instantly. Filter by project. See recent decisions with rationale. Check overdue reminders. The operations panel you'd build in Notion or Linear - except the data is your graph, not a separate SaaS database you have to keep in sync.

**Session Activity** - live WebSocket feed of every hook event across all your Claude Code sessions. Events grouped by session boundaries: how many prompts, how many tool calls, which domains matched, how many rules injected, how many deduped. The current session gets a live badge. Errors surface immediately. Click to expand individual events. This is the window into what BASE is actually doing - which hooks fire, what context flows, what gets suppressed.

**Usage Analytics** - token usage, cost tracking, model distribution across sessions. *(Coming in Plan 04.)*

The dashboard is compiled into the binary. No npm install, no separate server, no configuration. `base dashboard` starts an embedded HTTP server on localhost, opens your browser, and serves the SPA from memory. The graph loads from your workspace's `graph.trig`. WebSocket tails your hook event log in real-time. Everything runs local, everything is your data.

Every panel reads from the same SPARQL-backed API. The same graph that powers your hooks powers your dashboard. Add a note in the Graph Explorer and it shows up in your next Claude session. Drag a task to "completed" in the kanban and `base task list` reflects it. One graph, multiple surfaces.

## Design principles

**Suppression over detection.** Detection is trivial. The product is the gate that stays silent until something changes. If you touch the same file twice, the AST map doesn't re-inject. If the same domain rules are already in context, they don't repeat. The value is in what BASE doesn't say.

**CLI over MCP server.** One surface for Claude, hooks, and you. `base hook <event>` reads stdin, writes stdout. Same binary handles everything. Zero standing context cost.

**Deterministic matching.** Keywords, paths, excludes. No embeddings, no semantic search, no fuzzy matching in the core loop. You configure a trigger, it fires exactly when that trigger matches. Predictable.

**Mandatory edges.** Every entity connects to something. `base learn` requires `--domain`. `base entity add` requires `--domain`. `base project add` requires `--path` (which auto-creates a domain trigger). No orphans in the graph.

**Fail open.** Every hook catches all errors, logs to stderr, exits 0 with empty stdout. If the graph is corrupt, if SPARQL fails, if the file doesn't exist - Claude keeps working. The system never blocks the prompt.

## Graph durability & recovery

The graph is the durable record, so corruption has to be loud, diagnosable, and
self-healing — never a silent outage. BASE writes atomically (temp → validate → rename),
fails loud on a bad parse (non-zero exit + session-start warning), and splits read/write
behavior: **reads** fall back to a lenient parse that skips malformed lines and warns, so
one bad line never blanks your context; **writes** stay strict and refuse to run on an
unhealthy graph, so it's never silently rewritten with data dropped.

```bash
base doctor                 # parser-independent health scan (both tiers); --json for agents
base doctor --repair        # quarantine malformed lines + atomic rewrite of the good set
base doctor --restore       # list snapshots; --restore <backup> rolls one back

base graph compact          # dedup + canonicalize (atomic, idempotent)
base graph purge --stale    # PREVIEW notes unread > 21 days; --apply to delete, --days N to tune
```

Every repair/restore/compact/purge **snapshots first** to `graph.nq.bak-<op>-<date>` and
keeps the newest 10. `base recall` stamps `lastRead`, so a note's purge clock resets every
time it's used — only notes you never reach for age out (and dry-run is the default).

> **Never hand-edit `graph.nq`.** Use `base graph` / `base doctor`. Every corruption
> incident came from an interrupted hand-run edit — the commands above are atomic and
> validated. Full model + recovery runbook: [`docs/graph-durability.md`](docs/graph-durability.md).

## Extensions

BASE supports declarative extensions that let frameworks plug into the context engine.
Drop a TOML manifest in `~/.base-gbl/extensions/` and BASE wires it in automatically.

### Quick start

```bash
# Copy the template
cp ~/.base-gbl/extensions/_template.toml ~/.base-gbl/extensions/my-framework.toml

# Edit to declare your hooks
$EDITOR ~/.base-gbl/extensions/my-framework.toml

# Manage extensions
base extension list
base extension validate my-framework.toml
base extension install my-framework.toml
base extension remove my-framework
```

### What extensions can do

- **Session start** — inject status lines, run SPARQL queries, ingest JSON state files into the graph
- **User prompt** — declare domains with keywords/rules that merge into the normal matching pool
- **Pre-tool** — inject context when files in specific paths are accessed
- **Post-tool** — react to file writes (reingest state, log events)

### How it works

Extensions are flat TOML files in `~/.base-gbl/extensions/`. Each file declares which hooks it binds to and what data it provides. BASE scans the directory on every hook fire — file exists = active, delete = disabled.

Extension domains get `source = "ext:{name}"` markers in the graph for independent garbage collection. Ingested JSON state files become RDF entities queryable via SPARQL.

### Extension contract

See `~/.base-gbl/extensions/_template.toml` for the full contract specification with inline documentation.

## Stack

| Layer | Tech |
|-------|------|
| Language | Rust (single binary, ~20MB — includes embedded dashboard SPA) |
| Graph | Oxigraph (embedded, in-memory, loaded from disk per invocation) |
| Query | SPARQL SELECT and UPDATE |
| Persistence | NQuads text files (git-native, atomic write-back via temp+rename, validated before commit) |
| Config | TOML (domains.toml, base.toml) |
| AST extraction | Tree-sitter (Python scripts, 35+ languages) |
| Hooks | Claude Code settings.json (stdin/stdout JSON) |

---

Built by Chris Kahler
[Chris AI Systems](https://chrisai.cv) / [Community](https://chrisai.cv/skool) / [YouTube](https://www.youtube.com/@chris-ai-systems)
