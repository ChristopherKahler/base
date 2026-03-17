# Project: BASE

## What This Is

BASE (Builder's Automated State Engine) is a workspace orchestration framework for Claude Code. It manages the lifecycle of an AI builder's workspace — scaffolding, auditing, grooming, maintaining health, and providing structured data surfaces that give Claude passive awareness of work state without manual context management.

## Core Value

AI builders can maintain workspace health automatically, convert expensive @-mentioned markdown files into cheap structured data surfaces, and extend their Claude Code environment with custom data pipelines — all without manual bookkeeping or code editing.

## Current State

| Attribute | Value |
|-----------|-------|
| Version | 0.1 (v1 operational, v2 in design) |
| Status | Prototype → MVP |
| Last Updated | 2026-03-17 |

## Requirements

### Validated (Shipped)

- [x] Workspace manifest (workspace.json) — v0.1
- [x] State tracking (STATE.md, drift score, area statuses) — v0.1
- [x] Pulse hook (session-start health check) — v0.1
- [x] Groom workflow (weekly maintenance cycle) — v0.1
- [x] Audit workflow (deep workspace optimization) — v0.1
- [x] Scaffold workflow (guided workspace setup) — v0.1
- [x] CARL MCP server (domains, decisions, PSMM, staging tools) — v0.1
- [x] Satellite registration (PAUL projects auto-register) — v0.1
- [x] CARL hygiene workflow (rule lifecycle management) — v0.1

### Active (In Progress)

- [x] Data directory reorganization (.base/data/) — v2, Phase 1
- [x] Hook template with best practices — v2, Phase 2
- [x] BASE MCP server (generic surface CRUD) — v2, Phase 3
- [x] Active/Backlog JSON conversion (data surfaces) — v2, Phase 4
- [ ] Surface extensibility (/base:surface create/convert) — v2
- [ ] Passive awareness behavioral directives — v2

### Planned (Next)

- [ ] Scaffold v2 (end-to-end MCP + hook + surface installation)
- [ ] Distribution packaging for Skool/GitHub
- [ ] Teaching notes for "From Zero to Dangerous" course

### Out of Scope

- Migration of existing .claude/hooks/ to .base/hooks/ — separate future effort
- CARL standalone distribution packaging — CARL is its own project
- Course content production — this project feeds teaching notes, not lessons
- Decisions redesign (per-domain JSON migration) — uses BASE infrastructure but is a CARL concern

## Target Users

**Primary:** AI builders using Claude Code who want structured workspace management
- Operate complex workspaces with multiple projects, tools, and content pipelines
- Currently use @-mentioned markdown files for context (expensive, clunky, incomplete)
- Want Claude to have passive awareness of their work state without nagging

**Secondary:** Students of "From Zero to Dangerous" course
- Learning Claude Code methodology from scratch
- Need guided scaffold experience
- Want the "batteries included" version of Chris's system

## Context

**Business Context:**
- BASE is a core module in C&C Strategic Consulting's "From Zero to Dangerous" course (Module 1 + Module 8)
- Ships via Skool community (CC Strategic AI) and GitHub
- Part of a tool chain: BASE → CARL → PAUL → AEGIS → Skillsmith
- Revenue play: free tier gets CARL, paid tier gets BASE (full orchestration)

**Technical Context:**
- Claude Code skill (.claude/skills/base/)
- Two MCP servers: carl-mcp (rules engine tools), base-mcp (generic surface CRUD)
- Python hooks for data surface injection
- JSON data files in .base/data/
- Node.js MCP servers using @modelcontextprotocol/sdk

## Constraints

### Technical Constraints
- Hooks must use absolute paths (Claude Code requirement)
- One hook per data surface (isolation principle)
- MCP servers resolve workspace path from their own location
- Hook output goes to stdout as system-reminder content
- All JSON data lives in .base/data/ (single data directory)

### Business Constraints
- Must be workspace-agnostic (works for any Claude Code user, not just Chris)
- CARL must remain independently distributable (no BASE dependency)
- Scaffold must be fully automated (no manual wiring)

## Key Decisions

| Decision | Rationale | Date | Status |
|----------|-----------|------|--------|
| Everything lives in .base/ | Single location, easy to manage and extend | 2026-03-17 | Active |
| Two MCP servers (carl-mcp + base-mcp) | CARL ships standalone, BASE wraps it | 2026-03-17 | Active |
| One hook per data surface | Isolation — editing one can't regress another | 2026-03-17 | Active |
| Hook template generates conformant hooks | Consistent quality, no freestyle hook writing | 2026-03-17 | Active |
| All JSON data in .base/data/ | Both MCP servers share one data directory | 2026-03-17 | Active |
| New hooks in .base/hooks/ | Existing .claude/hooks/ untouched, new hooks in BASE | 2026-03-17 | Active |
| Passive awareness by default | Hook injections include behavioral directives against nagging | 2026-03-17 | Active |
| CARL standalone ships without MCP | Users upgrade to BASE for MCP tools and hygiene | 2026-03-17 | Active |
| Named BASE | Builder's Automated State Engine — outermost workspace layer | 2026-03-15 | Active |

## Success Metrics

| Metric | Target | Current | Status |
|--------|--------|---------|--------|
| Data surfaces operational | active + backlog converted | 2 surfaces (active: 12, backlog: 8) | Complete |
| Hook injection latency | <500ms per surface | N/A | Not measured |
| MCP tools working | All CRUD operations pass | carl-mcp + base-mcp (7 tools) | Partial |
| Scaffold end-to-end | Creates full v2 structure | v1 scaffold only | Partial |
| External user test | 1 non-Chris user scaffolds successfully | 0 | Not started |

## Tech Stack

| Layer | Technology | Notes |
|-------|------------|-------|
| Skill Framework | Claude Code Skills (.claude/skills/) | Skillsmith-compatible |
| MCP Servers | Node.js + @modelcontextprotocol/sdk | ES modules, stdio transport |
| Hooks | Python 3 | Lightweight, fast startup |
| Data | JSON files in .base/data/ | Schema-validated by BASE MCP |
| Configuration | workspace.json | Manifest-driven |

## Links

| Resource | URL |
|----------|-----|
| Spec (v2) | apps/base/BASE-V2-SPEC.md |
| Spec (v1) | apps/base/BASE-V1-SPEC.md |
| Live installation | .base/ (Chris's workspace) |
| Skill source | .claude/skills/base/ |
| CARL MCP source | .base/carl-mcp/ |

---
*PROJECT.md — Updated when requirements or context change*
*Last updated: 2026-03-17 after Phase 4*
