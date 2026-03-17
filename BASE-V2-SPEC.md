# BASE v2 — Data Surfaces, MCP Architecture, and Extensibility

**Version:** 2.0 Design Spec
**Created:** 2026-03-17
**Author:** Chris Kahler
**Status:** Design Phase — Pre-PAUL
**Feeds Into:** `/paul:init` for structured build orchestration
**Supersedes:** BASE-SPEC.md (v1), CARL-MCP-SPEC.md, HANDOFF-BASE-DATA-CONSOLIDATION.md

---

## Executive Summary

BASE v2 evolves from a workspace state tracker into a full workspace orchestration platform with three new capabilities:

1. **Data Surfaces** — A repeatable pattern for converting any structured data into Claude-aware context (JSON + hook + MCP tools)
2. **Dual MCP Architecture** — Separate CARL MCP (rules engine tools) and BASE MCP (generic surface CRUD) servers, both living inside `.base/`
3. **Extensibility Framework** — A scaffolded workflow (`/base:surface`) that lets any Claude Code user create their own data surfaces without manual wiring

This spec is the single source of truth for the entire build. PAUL will plan and orchestrate phases from this document.

---

## Architecture Decisions (Locked)

These decisions were made during the March 17, 2026 design session and are not open for re-evaluation.

| # | Decision | Rationale |
|---|----------|-----------|
| 1 | **Everything lives in `.base/`** — both MCP servers, all data, all generated hooks | Single location, easy to manage, easy to extend, fewer path issues |
| 2 | **Two MCP servers: carl-mcp + base-mcp** | CARL is a standalone tool without MCP. The MCP layer is a BASE capability. Separation allows CARL-only distribution while BASE ships everything. |
| 3 | **One hook per data surface** (not a monolithic hook) | Isolation — editing one surface's hook can't regress another. Each hook is independently testable. |
| 4 | **Hook template with best practices** generates conformant hooks | Claude reads the template, generates a hook that follows the exact contract. No freestyle hook writing. |
| 5 | **All JSON data files live in `.base/data/`** | Both MCP servers read/write from the same directory. No data split, no guessing which server owns what. |
| 6 | **New hooks go in `.base/hooks/`** | Claude Code supports absolute paths in settings.json. Existing hooks in `.claude/hooks/` stay there. New surface hooks live in `.base/hooks/`. Migration of existing hooks is a separate future effort. |
| 7 | **`/base:scaffold` does everything end-to-end** | Creates data dir, installs MCP servers, generates hooks, registers in settings.json, wires workspace.json — fully automated. |
| 8 | **Passive awareness by default** | Hook injections include behavioral directives telling Claude NOT to proactively surface the data. Only respond when asked, with a 24-hour deadline exception. |
| 9 | **CARL standalone ships without MCP** | Users who want MCP tools, hygiene flows, and the full management cycle upgrade to BASE. CARL is `.carl/` config only. |
| 10 | **Surfaces are registered in workspace.json** | Adding a surface = JSON file + hook + MCP tools + workspace.json entry. BASE MCP reads registrations to know what surfaces exist. |

---

## Distribution Tiers

| Tier | Ships With | Who Gets It |
|------|-----------|-------------|
| **CARL** | `.carl/` directory, manifest, domain files, decisions directory, hook template | Anyone who wants JIT dynamic rules for Claude Code |
| **BASE** | `.base/` with both MCP servers, data directory, hooks, surfaces, scaffold, groom, audit, hygiene, extensibility | Anyone who wants full workspace orchestration |

CARL is a dependency of BASE but operates independently. BASE enhances CARL with MCP tools, hygiene flows, and the data surface pattern.

---

## Target Directory Structure

```
.base/
├── workspace.json              # Manifest + surface registrations + config
├── STATE.md                    # Health snapshot (drift score, area statuses)
├── ACTIVE.md                   # Working memory (migrates to data/ in future phase)
├── BACKLOG.md                  # Work queue (migrates to data/ in future phase)
├── ROADMAP.md                  # Evolution log
│
├── data/                       # ALL structured JSON data
│   ├── psmm.json               # Per-session meta memory (CARL tool)
│   ├── staging.json            # CARL rule proposals (CARL tool)
│   ├── active.json             # Structured active work (future — BASE tool)
│   ├── backlog.json            # Structured backlog (future — BASE tool)
│   └── {user-surfaces}.json    # User-created surfaces (BASE tool)
│
├── hooks/                      # One hook file per data surface
│   ├── _template.py            # Canonical hook template (best practices)
│   ├── active-hook.py          # Active work injection (future)
│   ├── backlog-hook.py         # Backlog injection (future)
│   └── {surface}-hook.py       # User-created surface hooks
│
├── carl-mcp/                   # CARL MCP server (rules engine tools)
│   ├── index.js
│   ├── package.json
│   ├── node_modules/
│   └── tools/
│       ├── domains.js          # Domain CRUD
│       ├── decisions.js        # Decision logging/recall
│       ├── psmm.js             # Session meta memory
│       └── staging.js          # Rule proposal staging
│
├── base-mcp/                   # BASE MCP server (generic surface CRUD)
│   ├── index.js
│   ├── package.json
│   ├── node_modules/
│   └── tools/
│       └── surfaces.js         # Generic surface operations
│
├── grooming/                   # Weekly groom summaries
└── audits/                     # Deep audit records
```

---

## Component 1: Data Directory Reorganization

### Current State
- `psmm.json` lives at `.base/psmm.json`
- `staging.json` lives at `.base/staging.json`
- Both MCP tool files reference these locations

### Target State
- `psmm.json` moves to `.base/data/psmm.json`
- `staging.json` moves to `.base/data/staging.json`
- All future JSON data files go in `.base/data/`

### What Changes
1. Create `.base/data/` directory
2. Move `.base/psmm.json` → `.base/data/psmm.json`
3. Move `.base/staging.json` → `.base/data/staging.json`
4. Update `.base/carl-mcp/tools/psmm.js` — data path resolution
5. Update `.base/carl-mcp/tools/staging.js` — data path resolution
6. Update `.claude/skills/base/packages/carl-mcp/tools/psmm.js` — source package
7. Update `.claude/skills/base/packages/carl-mcp/tools/staging.js` — source package
8. Update `.claude/hooks/psmm-injector.py` — reads psmm.json for injection
9. Update `.base/carl-mcp/index.js` if it references data paths
10. Update workspace.json if it references data paths

### What Does NOT Change
- `.carl/` directory structure (manifest, domain files, decisions/) — untouched
- Existing hooks in `.claude/hooks/` — stay where they are
- CARL domain files — these are config, not data
- `.base/ACTIVE.md` and `.base/BACKLOG.md` — stay as markdown for now

---

## Component 2: Hook Template and Surface Hooks

### The Hook Template (`.base/hooks/_template.py`)

A canonical template that encodes every best practice for surface injection hooks. Claude reads this template to generate new hooks. The template defines:

**Contract:**
- Reads ONE JSON file from `.base/data/{surface}.json`
- Outputs a compact XML-tagged block to stdout
- Wrapped in `<{surface}-awareness>` tags
- Includes a `BEHAVIOR` directive block
- Exits cleanly — never crashes, never blocks

**Rules (DO):**
- Read the JSON file
- Format a compact summary (IDs, one-line descriptions, grouping by priority/status)
- Include item count summaries
- Include the behavioral directive (passive by default, configurable)
- Handle missing/empty/malformed files gracefully (output nothing, exit 0)
- Use absolute paths for file reads

**Rules (DO NOT):**
- Never write to any file
- Never make network calls
- Never import heavy dependencies
- Never read multiple data files (one hook = one surface)
- Never include full item details in injection (that's what MCP tools are for)
- Never include dynamic logic that changes based on time of day, session count, etc.

**Behavioral Directive Template:**
```
BEHAVIOR: This context is PASSIVE AWARENESS ONLY.
Do NOT proactively mention these items unless:
  - User explicitly asks (e.g., "what should I work on?", "what's next?")
  - A deadline is within 24 hours AND user hasn't acknowledged it this session
For details on any item, use base_get_item("{surface}", id).
```

### Hook Registration

When a new surface hook is created:
1. Hook file written to `.base/hooks/{surface}-hook.py`
2. Hook command added to `.claude/settings.json` UserPromptSubmit array:
   ```json
   {
     "type": "command",
     "command": "python3 /absolute/path/to/.base/hooks/{surface}-hook.py"
   }
   ```
3. Surface registration in `workspace.json` updated with `"hook": true`

---

## Component 3: BASE MCP Server

### Purpose
Generic CRUD operations for any registered data surface. Unlike CARL MCP which has purpose-built tools for specific concerns (domains, decisions, PSMM, staging), BASE MCP operates on any surface through a uniform interface.

### Tools

| Tool | Params | What It Does |
|------|--------|-------------|
| `base_list_surfaces` | none | List all registered surfaces from workspace.json with item counts |
| `base_get_surface` | `surface` | Read all items from a surface's JSON file |
| `base_get_item` | `surface, id` | Get a specific item by ID from a surface |
| `base_add_item` | `surface, data` | Add a new item to a surface (auto-generates ID) |
| `base_update_item` | `surface, id, data` | Update an existing item's fields |
| `base_archive_item` | `surface, id` | Move an item to the surface's `archived` array |
| `base_search` | `query, surface?` | Search across one or all surfaces by keyword |

### Surface Registration (workspace.json)

```json
{
  "surfaces": {
    "active": {
      "file": "data/active.json",
      "description": "Active work items and projects",
      "hook": true,
      "silent": true,
      "schema": {
        "id_prefix": "ACT",
        "required_fields": ["title", "status", "priority"],
        "priority_levels": ["urgent", "high", "medium", "low", "ongoing"],
        "status_values": ["active", "blocked", "deferred", "done"]
      }
    },
    "backlog": {
      "file": "data/backlog.json",
      "description": "Future work queue and ideas",
      "hook": true,
      "silent": true,
      "schema": {
        "id_prefix": "BL",
        "required_fields": ["title", "priority"],
        "priority_levels": ["high", "medium", "low"],
        "time_rules": {
          "review_by": { "high": 7, "medium": 14, "low": 30 },
          "staleness_multiplier": 2
        }
      }
    }
  }
}
```

### How BASE MCP Uses Registrations

1. `base_list_surfaces()` → reads `workspace.json` surfaces section, counts items in each file
2. `base_get_item("backlog", "BL-003")` → reads `workspace.json` to find `data/backlog.json`, reads file, finds item with matching ID
3. `base_add_item("backlog", {...})` → reads schema to validate required fields, auto-generates ID from `id_prefix`, writes to file
4. Schema drives validation — BASE MCP doesn't need to know what a "backlog" is, it just enforces the declared schema

### Data File Format (Generic Surface JSON)

Every surface JSON follows the same envelope:

```json
{
  "surface": "backlog",
  "version": 1,
  "last_modified": "2026-03-17T09:00:00-05:00",
  "items": [
    {
      "id": "BL-001",
      "title": "Framework teaching notes for all tools",
      "priority": "high",
      "description": "Before course lessons can be produced, each framework needs teaching notes...",
      "location": "projects/skool-recalibration/",
      "added": "2026-03-16",
      "review_by": "2026-03-23",
      "checklist": [
        { "text": "CARL teaching notes", "done": true },
        { "text": "BASE teaching notes", "done": false },
        { "text": "PAUL teaching notes", "done": false }
      ],
      "notes": "CARL's is done. These feed into Module 2-8 lesson scripts."
    }
  ],
  "archived": []
}
```

---

## Component 4: Surface Extensibility (`/base:surface`)

### `/base:surface create {name}`

Guided workflow for creating a new data surface:

1. **Define** — "What does this surface track?" → sets description
2. **Schema** — "What fields does each item need?" → guided schema builder
   - Required fields vs optional
   - Field types (string, number, boolean, date, enum)
   - Priority/status enums if applicable
   - ID prefix
   - Time-based rules (review-by, staleness) if applicable
3. **Injection** — "How should this appear in Claude's context?" → injection format
   - Grouping strategy (by priority, by status, by date)
   - Summary line format (what fields to show in the compact view)
   - Behavioral mode (silent, proactive, threshold-based)
4. **Tools** — "What operations should Claude have?" → tool selection
   - Read operations (always included)
   - Write operations (add, update — opt-in)
   - Archive (opt-in)
   - Search (opt-in)
5. **Generate:**
   - `.base/data/{name}.json` — empty, seeded from schema
   - `.base/hooks/{name}-hook.py` — generated from `_template.py` + injection config
   - Surface registration in `workspace.json`
   - BASE MCP auto-discovers new surface from registration (no code changes needed)
   - Hook registered in `.claude/settings.json`

### `/base:surface convert {file}`

Converts an existing `@`-mentioned markdown file into a data surface:

1. **Read** — Parse the markdown file, detect structure (sections, items, priorities)
2. **Propose** — Present a JSON schema based on detected patterns
3. **Confirm** — User adjusts schema if needed
4. **Generate** — Same outputs as `create`
5. **Migrate** — Parse markdown content into JSON items
6. **Clean up** — Offer to remove `@` reference from CLAUDE.md

### `/base:surface list`

Shows all registered surfaces with item counts and hook status.

---

## Component 5: Scaffold Updates

`/base:scaffold` needs to be updated to support the v2 structure:

### New Scaffold Steps

After existing scan/configure/install steps:

1. **Create data directory** — `.base/data/`
2. **Create hooks directory** — `.base/hooks/`
3. **Install hook template** — `.base/hooks/_template.py` from source package
4. **Install BASE MCP** — Copy from source package to `.base/base-mcp/`, run npm install
5. **Install CARL MCP** — Copy from source package to `.base/carl-mcp/`, run npm install (if CARL detected)
6. **Register MCP servers** — Add both to `.mcp.json`
7. **Initialize default surfaces** — If `--full` mode, offer to create active + backlog surfaces
8. **Wire hooks** — Register any surface hooks in `.claude/settings.json`

### Source Packages

```
.claude/skills/base/packages/
├── carl-mcp/               # CARL MCP source (copied to .base/carl-mcp/)
│   ├── index.js
│   ├── package.json
│   └── tools/
│       ├── domains.js
│       ├── decisions.js
│       ├── psmm.js
│       └── staging.js
├── base-mcp/               # BASE MCP source (NEW — copied to .base/base-mcp/)
│   ├── index.js
│   ├── package.json
│   └── tools/
│       └── surfaces.js
└── hooks/                  # Hook templates (NEW)
    └── _template.py
```

---

## Component 6: Existing Reference Updates

### CARL Rules That Need Updating

| Rule | Current Reference | New Reference |
|------|------------------|---------------|
| GLOBAL_RULE_12 (PSMM) | References `.claude/session-context/psmm.json` as fallback | Update to `.base/data/psmm.json` |

### Hooks That Need Path Updates

| Hook | Current Data Path | New Data Path |
|------|------------------|---------------|
| `.claude/hooks/psmm-injector.py` | `.base/psmm.json` | `.base/data/psmm.json` |

### MCP Tool Files That Need Path Updates

| File | What Changes |
|------|-------------|
| `.base/carl-mcp/tools/psmm.js` | Data path resolution → `.base/data/psmm.json` |
| `.base/carl-mcp/tools/staging.js` | Data path resolution → `.base/data/staging.json` |
| `.claude/skills/base/packages/carl-mcp/tools/psmm.js` | Source package mirror |
| `.claude/skills/base/packages/carl-mcp/tools/staging.js` | Source package mirror |

### Skill Files That Need Updates

| File | What Changes |
|------|-------------|
| `.claude/skills/base/base.md` | Add `/base:surface` commands |
| `.claude/skills/base/tasks/scaffold.md` | Add data dir, hooks dir, MCP server install steps |
| `.claude/skills/base/context/base-principles.md` | Add Data Surface principles |
| `.claude/skills/base/templates/workspace-json.md` | Add surfaces schema section |

### CLAUDE.md Updates

| Section | What Changes |
|---------|-------------|
| Quick Reference | Add `/base:surface` |
| Systems Architecture > BASE | Update description to reflect v2 |

---

## Behavioral Design: Passive Awareness

### The Problem
Claude sees urgents with deadlines and its helpfulness instinct kicks in. It nags: "By the way, you have X urgent that's due in two days." The user already knows. They've been told in previous sessions. The nagging adds friction without adding value.

### The Solution
Hook injections carry their own behavioral directives. The directive travels WITH the data, not in a separate CARL rule. This means:

1. **Default: Silent.** Claude has the awareness but doesn't mention it.
2. **On ask: Instant.** "What should I do next?" → Claude already has the data, responds immediately with prioritized items.
3. **Exception: 24-hour deadline.** If a deadline is within 24 hours AND the user hasn't acknowledged it in the current session, Claude may mention it once.
4. **Configurable per surface.** Some surfaces might warrant proactive mention (e.g., a "blockers" surface). The `silent` flag in workspace.json controls this.

### Implementation
The behavioral directive is part of the hook output, not a CARL rule:

```xml
<backlog-awareness silent="true" deadline_threshold="24h">
BACKLOG: 1 high, 4 medium, 3 low
  H: BL-001 | Framework teaching notes
  M: BL-002 | CARL context dedup review
  ...

BEHAVIOR: PASSIVE AWARENESS ONLY.
Do NOT proactively mention unless user asks or deadline < 24h unacknowledged.
Use base_get_item("backlog", id) for details.
</backlog-awareness>
```

---

## Open Questions (For PAUL Planning)

These should be resolved during PAUL's PLAN phase:

1. **active.json and backlog.json schemas** — The generic surface format above is a starting point. PAUL PLAN should finalize exact field definitions by analyzing current ACTIVE.md and BACKLOG.md content.
2. **CARL MCP workspace path resolution** — Currently resolves `../..` from `.base/carl-mcp/`. Moving data to `.base/data/` may require path adjustments. Verify during implementation.
3. **BASE MCP workspace path resolution** — New server, needs to resolve workspace root from `.base/base-mcp/`. Same pattern as CARL MCP.
4. **Hook performance** — Each surface adds a hook to UserPromptSubmit. With 5+ surfaces, measure cumulative latency. If too slow, consider consolidation strategies later.
5. **Surface schema validation** — How strict should BASE MCP be? Reject writes that don't match schema? Warn but allow? Start permissive, tighten later?
6. **Markdown file fate** — After active.json and backlog.json are operational, do ACTIVE.md and BACKLOG.md become auto-generated views, get archived, or get deleted? Decide during implementation.

---

## Build Phases (Suggested for PAUL)

These are suggested phase groupings. PAUL's PLAN phase will refine and may split/merge.

### Phase 1: Data Directory Reorganization
- Create `.base/data/` and `.base/hooks/`
- Move psmm.json and staging.json into `.base/data/`
- Update all file path references (carl-mcp tools, source packages, hooks, CARL rules)
- Verify all CARL MCP tools still work after move
- Verify PSMM hook injection still works

### Phase 2: Hook Template
- Design and write `.base/hooks/_template.py`
- Document the hook contract (DOs and DONTs)
- Create source package copy at `.claude/skills/base/packages/hooks/_template.py`
- Test by generating a sample hook from the template

### Phase 3: BASE MCP Server
- Build `.base/base-mcp/` with surfaces.js
- Implement: list_surfaces, get_surface, get_item, add_item, update_item, archive_item, search
- Register in `.mcp.json`
- Add surface registration schema to workspace.json
- Create source package at `.claude/skills/base/packages/base-mcp/`
- Test all tools with a dummy surface

### Phase 4: Active + Backlog Surface Conversion
- Design active.json schema (from ACTIVE.md analysis)
- Design backlog.json schema (from BACKLOG.md analysis)
- Build migration script/process to parse markdown into JSON
- Migrate current ACTIVE.md content → active.json
- Migrate current BACKLOG.md content → backlog.json
- Generate hooks from template (active-hook.py, backlog-hook.py)
- Register hooks in settings.json
- Register surfaces in workspace.json
- Test: verify hook injection, test MCP CRUD, verify passive behavior

### Phase 5: Surface Extensibility
- Build `/base:surface create` command (task file)
- Build `/base:surface convert` command (task file)
- Build `/base:surface list` command (task file)
- Update `/base:scaffold` to include v2 steps
- Update base.md entry point with new commands
- Update base-principles.md with Data Surface concepts
- Update workspace-json.md template with surfaces schema
- End-to-end test: create a custom surface from scratch

### Phase 6: Documentation and Course Integration
- Update all handoff docs to reflect v2 architecture
- Create BASE teaching notes for the course
- Document the Data Surface pattern as a standalone concept
- Update CLAUDE.md references

---

## Success Criteria

- [ ] All JSON data lives in `.base/data/` — no loose JSON files in `.base/` root
- [ ] CARL MCP tools work from `.base/carl-mcp/` reading `.base/data/`
- [ ] BASE MCP server operational with generic surface CRUD
- [ ] Hook template exists and encodes all best practices
- [ ] active.json and backlog.json exist with migrated content
- [ ] Surface hooks inject compact awareness into Claude context
- [ ] Passive behavior works — Claude doesn't nag, but responds instantly when asked
- [ ] `/base:surface create` generates a complete surface end-to-end
- [ ] `/base:surface convert` can transform an `@`-mentioned markdown file
- [ ] `/base:scaffold` installs the full v2 structure
- [ ] A new Claude Code user can create a custom data surface without editing code

---

## What This Spec Does NOT Cover

- Migration of existing hooks from `.claude/hooks/` to `.base/hooks/` (separate effort)
- CARL standalone distribution packaging
- PSMM data ownership debate (settled: PSMM stays in `.base/data/`, CARL MCP reads it)
- Decisions redesign (per-domain JSON files in `.carl/decisions/`) — that's a separate spec
- CARL hygiene implementation — that's a separate spec
- Course content production — this spec feeds the teaching notes, not the lessons themselves
