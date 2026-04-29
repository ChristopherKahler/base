<div align="center">
  <img src="terminal.svg" alt="BASE terminal" width="740"/>
</div>

<div align="center">

[![npm](https://img.shields.io/npm/v/@chrisai/base?color=00d8ff&label=npm&style=flat-square)](https://www.npmjs.com/package/@chrisai/base)
[![Node](https://img.shields.io/badge/node-%3E%3D16.7.0-brightgreen?style=flat-square)](https://nodejs.org)
[![License](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](LICENSE)
[![Claude Code](https://img.shields.io/badge/Claude%20Code-compatible-8b5cf6?style=flat-square)](https://claude.ai/code)

**Your AI builder operating system.**<br/>
Turn Claude Code from a per-session tool into a workspace that remembers, maintains itself, and never goes stale.


</div>

---

## Contents

- [The Problem Every Claude Code User Hits](#the-problem-every-claude-code-user-hits)
- [What BASE Actually Does](#what-base-actually-does)
- [How It Works](#how-it-works)
- [Install](#install)
- [Upgrading from v2](#upgrading-from-v2)
- [What Gets Installed](#what-gets-installed)
- [The Maintenance Cycle](#the-maintenance-cycle)
- [Config Alignment](#config-alignment--claude-directory-audit)
- [MCP Server](#mcp-server--claude-operates-on-your-data)
- [Operator Profile](#operator-profile--who-you-are-always-in-context)
- [PSMM — Session Intelligence](#per-session-meta-memory-psmm--session-intelligence)
- [Apex Insights](#apex-insights--workspace-analytics)
- [Multi-Project Workspaces — BASE + PAUL](#multi-project-workspaces--base--paul)
- [Creating Custom Surfaces](#creating-custom-surfaces)
- [How the Ecosystem Fits Together](#how-the-ecosystem-fits-together)
- [Design Principles](#design-principles)
- [Quick Start](#quick-start)

---

## Ecosystem

BASE is part of a broader Claude Code extension ecosystem:

| System | What It Does | Link |
|--------|-------------|------|
| **AEGIS** | Multi-agent codebase auditing — diagnosis + controlled evolution | [GitHub](https://github.com/ChristopherKahler/aegis) |
| **BASE** | Builder's Automated State Engine — workspace lifecycle, health tracking, drift prevention | You are here |
| **CARL** | Context Augmentation & Reinforcement Layer — dynamic rules loaded JIT by intent | [GitHub](https://github.com/ChristopherKahler/carl) |
| **PAUL** | Project orchestration — Plan, Apply, Unify Loop | [GitHub](https://github.com/ChristopherKahler/paul) |
| **SEED** | Typed project incubator — guided ideation through graduation into buildable projects | [GitHub](https://github.com/ChristopherKahler/seed) |
| **Skillsmith** | Skill builder — standardized syntax specs + guided workflows for Claude Code skills | [GitHub](https://github.com/ChristopherKahler/skillsmith) |
| **CC Strategic AI** | Skool community — courses, community, live support | [Skool](https://chrisai.cv/skool) |

---

## The Problem Every Claude Code User Hits

You start a Claude Code session. Claude doesn't know:

- What you're working on
- What's blocked
- What you worked on yesterday
- What's overdue
- Which projects need attention

So you repeat yourself. Every. Single. Session.

Maybe you've tried fixing this with a massive `CLAUDE.md` file, `@`-mentions pointing at markdown docs, or manual context dumps at session start. It works... until it doesn't. Files go stale. Your CLAUDE.md becomes a junk drawer. You forget to update things. Claude starts making decisions based on outdated information. And the bigger your workspace gets, the worse it breaks.

**This is the duct tape phase.** Everyone goes through it. BASE is what comes after.

---

## What BASE Actually Does

BASE turns your Claude Code workspace into a managed operating system. Instead of scattered markdown files and manual context loading, you get:

**Structured data that Claude reads automatically.** Your active projects, backlog items, client lists — anything you want Claude to passively know about — lives in structured JSON files. Lightweight hooks inject compact summaries into every session automatically. You never type "here's what I'm working on" again.

**Health monitoring that catches drift before it hurts.** A drift score tracks how far your workspace state has drifted from reality. When things go stale, BASE tells you. When grooming is overdue, BASE tells you. You fix it with a guided maintenance cycle, not a weekend of cleanup.

**A manifest that drives everything.** One config file (`workspace.json`) declares what your workspace contains, how each area should be maintained, and what projects are in play. Every command reads from it. Your workspace is self-describing.

### What This Looks Like in Practice

You open Claude Code. Before you type anything, Claude already knows:

```xml
<active-awareness items="5">
[URGENT]
- [ACT-001] Client Portal Launch (Blocked: API auth)
  DUE: 2026-03-20
[HIGH]
- [ACT-002] Course Module 3 (In Progress)
- [ACT-003] MCP Server Refactor (In Review)

BEHAVIOR: PASSIVE AWARENESS ONLY.
Do NOT proactively mention unless user asks or deadline < 24h.
</active-awareness>
```

Claude doesn't nag. It doesn't start the session with "here are your tasks." But the moment you ask "what should I work on?" — the answer is instant and accurate. No file reading. No context window wasted. No stale data.

---

## How It Works

### Data Surfaces — The Core Concept

A "data surface" is just a structured JSON file paired with a hook that injects it into Claude's context. That's it.

```
JSON file (your data)  →  Hook (reads + summarizes)  →  Claude knows it
```

BASE ships with four built-in data files:

| Data | What It Tracks |
|------|---------------|
| **Projects** | Unified project tracking — initiatives, projects, tasks, backlog, PAUL integration, categories, revenue |
| **Entities** | People and organizations — contacts, stakeholders, collaborators |
| **State** | Workspace health — drift score, area statuses, groom tracking |
| **PSMM** | Per-session meta memory — decisions, corrections, insights within a session |

But you can create surfaces for anything — clients, contacts, content pipelines, API keys, whatever persistent data you want Claude to passively know about. The `/base:surface create` command walks you through it: define a schema, pick an injection format, and BASE generates the JSON file, the hook, and the wiring automatically.

### The Manifest — One File Rules Everything

`workspace.json` is the brain. It registers:

- Every data surface and its schema
- Every tracked area in your workspace (projects, tools, content, clients...)
- Grooming schedules per area
- Audit strategies per area type
- Connected projects and their health status

Every BASE command reads from this manifest. You configure it once during setup, and the system maintains itself from there.

### Hooks — The Glue

BASE uses Claude Code's [hook system](https://docs.anthropic.com/en/docs/claude-code/hooks) to inject context automatically. There are two types:

**Every prompt** (`UserPromptSubmit`) — Fire on every message you send, keeping Claude's awareness current throughout the session:

| Hook | What It Does |
|------|-------------|
| **Pulse check** | Calculates workspace drift score, warns if grooming is overdue |
| **PSMM injector** | Re-injects important session moments (decisions, corrections, insights) into Claude's context so they don't get buried in a long session. [Details below.](#per-session-meta-memory-psmm--session-intelligence) |
| **Operator** | Injects a compact identity summary from your operator profile — north star, values, vision — so Claude stays aligned with who you are and what you're building toward |
| **Surface hooks** | One per data surface (active, backlog, or custom). Reads the JSON, outputs a compact summary so Claude passively knows the current state |

**Session start** (`SessionStart`) — Runs once when a Claude Code session begins:

| Hook | What It Does |
|------|-------------|
| **PAUL project detection** | Scans your workspace for [PAUL](https://github.com/ChristopherKahler/paul) project files (`.paul/paul.json`) and auto-registers new ones into `workspace.json`. Only needs to run once — your project list doesn't change mid-session. (More on PAUL below.) |

**On demand** — Invoked by specific commands, not auto-fired:

| Hook | What It Does |
|------|-------------|
| **Apex insights** | Workspace analytics engine — velocity tracking, stall detection, blocking analysis, revenue exposure, dependency chains. Invoked by `/apex:insights`. |

All hooks are lightweight Python — they read JSON files and output compact XML summaries. No network calls, no heavy dependencies, no noticeable latency. A hook that has nothing to report outputs nothing and exits silently.

---

## Install

```bash
npx @chrisai/base --global --workspace
```

One command. Two layers:

- `--global` installs commands and the framework to `~/.claude` (shared across all your workspaces)
- `--workspace` installs the data layer to `.base/` in your current directory

Then open Claude Code and run `/base:scaffold` to configure your workspace with a guided setup.

```bash
# Full install — most users start here
npx @chrisai/base --global --workspace

# Already installed globally? Wire up a new workspace
npx @chrisai/base --workspace

# Global only — set up workspaces later with /base:scaffold
npx @chrisai/base --global
```

| Flag | What It Does |
|------|-------------|
| `--global` | Commands + framework to `~/.claude` (shared) |
| `--workspace` | Data layer to `.base/` in current directory |
| `--local` | Commands to `./.claude` instead of global |
| `--config-dir <path>` | Custom Claude config directory |
| `--workspace-dir <path>` | Target a specific workspace path |

### Upgrading from v2

If you're upgrading from BASE v2.x, the installer detects old artifacts and offers to archive them before proceeding. Nothing is deleted — everything moves to `.base/_archive/upgrade-v3/` where you can recover it if needed.

| What Gets Archived | Why |
|-------------------|-----|
| `.base/carl-mcp/` | CARL MCP is no longer bundled with BASE. Install [carl-core](https://github.com/ChristopherKahler/carl) separately for CARL MCP tools. |
| `.claude/hooks/base-pulse-check.py`, `psmm-injector.py`, `satellite-detection.py` | Session hooks moved to `.base/hooks/`. Old copies in `.claude/hooks/` cause double-fire. |
| `.mcp.json` carl-mcp entry | Removed to prevent startup errors from missing server. |
| `base-framework/templates/active-md.md`, `backlog-md.md`, `state-md.md` | Replaced by JSON templates. Scaffold no longer references these. |

The upgrade prompt looks like this:
```
=== UPGRADE DETECTED ===
Found 8 artifact(s) from a previous BASE version:

! .base/carl-mcp/
  CARL MCP no longer ships with BASE — install carl-core separately
! .claude/hooks/base-pulse-check.py
  Hooks now live in .base/hooks/ — duplicate here causes double-fire
...

These will be archived to: .base/_archive/upgrade-v3/
Nothing is deleted — you can recover from the archive.

Archive these artifacts? [Y/n]:
```

Answering `n` skips cleanup and installs v3 alongside existing artifacts. You can clean up manually later.

---

## What Gets Installed

```
~/.claude/                              Shared across all workspaces
├── commands/base/                      Slash commands (/base:pulse, /base:groom, etc.)
├── skills/base/                        Skill entry point + package sources
└── base-framework/
    ├── tasks/                          How each command works (pulse, groom, audit...)
    ├── templates/                      Schemas for workspace.json, surfaces
    ├── context/                        Core principles
    ├── frameworks/                     Audit strategies, config alignment, project registration
    ├── utils/                          Scanner utilities (scan-claude-dirs.py)
    └── hooks/                          All hook sources (for scaffold reference)

.base/                                  Per-workspace
├── workspace.json                      The manifest — everything is registered here
├── operator.json                       Operator profile — north star, values, vision, pitch
├── data/
│   ├── projects.json                   Project tracking (initiatives, projects, tasks, backlog)
│   ├── entities.json                   People and organizations
│   ├── state.json                      Workspace health state
│   └── psmm.json                       Per-session meta memory
├── hooks/
│   ├── _template.py                    Hook template for creating new surfaces
│   ├── active-hook.py                  Injects active work into Claude's context
│   ├── backlog-hook.py                 Injects backlog into Claude's context
│   ├── base-pulse-check.py             Drift score + groom reminders
│   ├── psmm-injector.py                Session meta memory
│   ├── satellite-detection.py          PAUL project auto-discovery
│   ├── operator.py                     Operator identity context
│   └── apex-insights.py                Workspace analytics (on-demand)
└── base-mcp/                           MCP server for surface + project operations
```

---

## The Maintenance Cycle

Most workspace management tools are set-and-forget. BASE is designed around the reality that workspaces are living things that drift.

### Pulse — Session Start Health Check

`/base:pulse` runs automatically via hook. It reads your manifest, checks filesystem timestamps, and calculates a drift score:

| Drift Score | What It Means | What to Do |
|-------------|--------------|------------|
| **0** | Everything is current | Work normally |
| **1-7** | Minor drift | Fix at next groom |
| **8-14** | Moderate — Claude may be acting on stale info | Groom soon |
| **15+** | Critical — workspace context is unreliable | Groom now |

No stop hooks. No unreliable session-end tracking. Pulse always starts from filesystem ground truth.

### Groom — Weekly Maintenance

`/base:groom` is a guided, voice-friendly walkthrough of your entire workspace. It reviews one area at a time, oldest-first:

1. **Active work** — "Still active? Status changed? Anything done?" — walks through each project and task
2. **Backlog** — Enforces time-based rules:
   - High priority items get 7 days before they demand a decision
   - Medium gets 14 days. Low gets 30 days.
   - Items that sit past 2x their review window get auto-archived.
   - "Decide or kill" — nothing sits in limbo forever.
3. **Graduation** — "Ready to work on any backlog items?" Items move to active work. Always explicit, never automatic.
4. **Directories** — Scans tracked directories (projects, clients, tools) for orphaned or new items
5. **Connected projects** — Checks project health across your workspace (more on this below)
6. **System layer** — Quick scan for dead hooks, unused commands, stale rules

Result: drift score resets to 0, summary gets logged, next groom date is set.

### Audit — Deep Optimization

`/base:audit` goes deeper than grooming. Each tracked area maps to a configurable audit strategy:

| Strategy | Applies To | What It Does |
|----------|-----------|-------------|
| `staleness` | Working memory files | Checks file age against thresholds |
| `classify` | Directories (projects/, clients/) | Lists items for triage: active, archive, or delete |
| `cross-reference` | Tools with config files | Finds orphaned tools and broken config references |
| `dead-code` | System directories | Finds unused hooks, commands, skills |
| `pipeline-status` | Content or task workflows | Flags stuck items and bottlenecks |

The number of audit phases is dynamic — generated from your manifest, not hardcoded. A small workspace gets 3 phases. A large one gets 12. Same command, adapted to your reality.

### Config Alignment — `.claude/` Directory Audit

`/base:audit-claude` solves a problem that grows silently: `.claude/` directory sprawl.

As you work across multiple projects, each one accumulates its own `.claude/` directory — hooks copied from another project, skills installed locally before you went global, settings files with MCP server lists from three months ago. Over time you end up with duplicated hooks running twice, stale config referencing tools that don't exist, and skills taking up space in five project directories when they should live in one global location.

#### How it works

The audit runs in two stages:

**Stage 1: Scanner** — A Python utility (`scan-claude-dirs.py`) runs first and produces a complete JSON dataset of your workspace. It recursively discovers every `.claude/` directory, catalogs every file inside them, and generates an MD5 fingerprint for each one. It also builds baselines of your global `~/.claude/` and workspace root `.claude/` — same treatment, every file fingerprinted. The result is a structured JSON file saved to `.base/audits/data-sets/`.

An MD5 fingerprint is a unique identifier generated from a file's contents. If two files produce the same fingerprint, they are byte-for-byte identical — not "similar," not "probably the same," but provably exact. If the fingerprints differ, the files are different. This means every classification in the audit is based on evidence, not assumption.

**Stage 2: Classification + Report** — Claude reads the scanner's JSON dataset and classifies every item against both baselines:

| Classification | What It Means |
|---------------|---------------|
| **DUPLICATE** | Fingerprint matches a baseline file — provably identical, safe to remove |
| **DIVERGED** | Same name exists in a baseline but different fingerprint — needs a decision |
| **GLOBAL_CANDIDATE** | Verified not to exist in any baseline — suggest promotion |
| **PROJECT_SPECIFIC** | Legitimately belongs in this project — leave it alone |
| **STALE** | References things that no longer exist — clean up |
| **ACCIDENTAL** | Nested `.claude/.claude/` dirs, empty dirs — delete |
| **TEMPLATE** | Lives in a `_template/` directory — intentional, skip |

The scanner produces the data. Claude produces the judgment. Separating these means the data is deterministic and complete — Claude can't accidentally skip a file or forget to check a baseline.

#### Remediation

Findings are grouped by risk level (lowest first) and every change requires explicit approval. The workflow never batch-deletes, never assumes a messy config is wrong, and never modifies your global `~/.claude/` without asking. After changes, it verifies no broken references were created and recommends which projects to test.

The full audit report is written to `.base/audits/` as a structured markdown file — readable in any markdown viewer, not buried in terminal output. The scanner's raw JSON dataset is preserved in `.base/audits/data-sets/` for reference.

The strategy is defined in a standalone framework file (`claude-config-alignment.md`) that any audit workflow can compose in at runtime — it doesn't modify existing audit strategies.

---

## MCP Server — Claude Operates on Your Data

BASE ships one MCP server so Claude can read and write your workspace data through structured tool calls instead of raw file edits.

### BASE MCP — Projects, Entities, State, Operator, PSMM

A unified interface for all workspace data. 20 tools across 5 modules:

| Module | Tools | What They Do |
|--------|-------|-------------|
| **Projects** | `base_list_projects`, `base_get_project`, `base_add_project`, `base_update_project`, `base_archive_project`, `base_search_projects` | Hierarchy-aware CRUD — initiatives, projects, tasks. Auto-ID by type (INI/PRJ/TSK). Filter by status, priority, parent, category. |
| **Entities** | `base_list_entities`, `base_add_entity`, `base_update_entity`, `base_link_entity` | People and organization management with relational links to projects |
| **State** | `base_get_state`, `base_update_drift`, `base_record_groom`, `base_update_area` | Workspace health, drift tracking, groom scheduling |
| **Operator** | `base_get_operator`, `base_update_operator` | Read/update operator profile (north star, values, vision, pitch) |
| **PSMM** | `base_psmm_log`, `base_psmm_get`, `base_psmm_list`, `base_psmm_clean` | Per-session meta memory — log and manage session moments |

### CARL Integration

[CARL](https://github.com/ChristopherKahler/carl) (Context Augmentation & Reinforcement Layer) is a dynamic rules engine for Claude Code. It stores behavioral rules in domain files — groups of rules that load automatically based on what you're doing. Say "check Skool" and CARL loads your Skool community rules. Start coding and it loads your development standards. The rules are just config files in `.carl/`.

**CARL is fully independent — it works without BASE, and BASE works without CARL.** CARL ships its own MCP server (`carl-mcp`) as part of the [carl-core](https://github.com/ChristopherKahler/carl) package. Install it separately if you want CARL's tools. If you use both, they complement each other — CARL handles rules and decisions, BASE handles workspace data and project tracking.

When CARL is installed alongside BASE, Claude gets programmatic access to three powerful systems (all provided by CARL's MCP, not BASE's):

#### Dynamic Rules

Claude can read, search, and manage your rule domains through tool calls instead of file edits:

| Tool | What It Does |
|------|-------------|
| `carl_list_domains` | List all rule domains and their status |
| `carl_get_domain_rules` | Read rules for a specific domain |
| `carl_stage_proposal` | Stage a new rule proposal for review (more on this below) |

#### Decision Logger

Decisions get lost. You make a call in one session — "we're using OAuth, not API keys" — and three sessions later Claude asks you the same question. Or worse, it makes the opposite choice because it has no memory of what you decided.

CARL's decision logger fixes this. Decisions are stored per domain (e.g., `decisions/development.json`, `decisions/global.json`) and **load automatically alongside domain rules**. When CARL loads the "development" domain because you're coding, every decision you've ever logged in that domain loads with it as lightweight metadata. Claude reads your decisions before acting on the prompt itself. It never misses a key decision again — not because it searches for it, but because the decision is already in context the moment the domain is relevant.

| Tool | What It Does |
|------|-------------|
| `carl_log_decision` | Record a decision with domain, rationale, and recall keywords |
| `carl_search_decisions` | Search across all domains when you need to find something specific |

#### The Rule Staging Pipeline

This is where CARL connects to BASE's [PSMM](#per-session-meta-memory-psmm--session-intelligence) system. Session moments logged via PSMM can graduate into permanent CARL rules:

1. **During a session** — Claude notices a pattern worth codifying (a correction you gave, a decision that should become policy, an insight about how you work)
2. **Stage it** — `carl_stage_proposal` creates a draft rule in staging, not in your live rules
3. **Review during hygiene** — BASE's `/base:carl-hygiene` command walks you through staged proposals: approve, edit, or kill each one
4. **Approved rules go live** — They become part of your CARL domains, loaded automatically in future sessions

This means your AI assistant gets smarter over time — not by accumulating a massive prompt, but by distilling session learnings into clean, targeted rules. And because staging exists, nothing goes live without your review. The hygiene cycle (part of BASE's groom flow) prevents staged proposals from going stale — they get reviewed or they get killed.

---

## Operator Profile — Who You Are, Always in Context

`operator.json` is a structured identity document that gives Claude persistent alignment with your goals, values, and vision. Instead of re-explaining who you are and what you're building toward each session, your operator profile loads automatically.

The profile is built through a guided questionnaire (via `/base:scaffold`) that walks through five layers:

| Section | What It Captures |
|---------|-----------------|
| **Deep Why** | Five increasingly deep questions about your motivation — not a mission statement, but the real reason you do what you do |
| **North Star** | One measurable metric with a timeframe — the thing you're optimizing for right now |
| **Key Values** | Rank-ordered values with concrete meanings — not platitudes, but actionable principles |
| **Elevator Pitch** | A layered pitch (1-4 floors) that describes what you do at increasing depth |
| **Surface Vision** | Concrete scenes of what success looks like — not abstract goals, but vivid snapshots |

The operator hook injects a compact summary into every session. Claude doesn't quote it back to you — it just stays aligned. When you're making decisions, Claude's suggestions naturally reflect your north star and values without being asked.

---

## Per-Session Meta Memory (PSMM) — Session Intelligence

Here's the problem with long Claude Code sessions: Claude's context window is huge (up to 1M tokens), but important moments — a design decision you made at minute 5, a correction at minute 20, a key insight at minute 45 — get buried under thousands of lines of tool output and code. By the time you're deep into the session, Claude has technically "seen" these moments but they've drifted so far back in context that they stop influencing behavior.

PSMM fixes this. Both logging and injection are built into BASE:

**Logging** — When something significant happens, Claude logs it via the BASE MCP:

| Tool | What It Does |
|------|-------------|
| `base_psmm_log` | Log a session meta-memory entry (type: DECISION, CORRECTION, SHIFT, INSIGHT, COMMITMENT) |
| `base_psmm_get` | Retrieve entries for a specific session |
| `base_psmm_list` | List all sessions with entry counts |
| `base_psmm_clean` | Remove stale session data |

**Injection** — The PSMM hook re-injects the current session's entries into Claude's context on every prompt. Important moments stay hot for the entire session, no matter how long it runs.

**Graduation** — When a session insight should become a permanent rule, it can be staged as a [CARL](https://github.com/ChristopherKahler/carl) rule proposal via `carl_stage_proposal`. This is the only point where PSMM connects to CARL — everything else is self-contained in BASE.

No CARL required. PSMM works standalone as session memory. CARL adds the optional path from "session insight" to "permanent behavioral rule."

---

## Apex Insights — Workspace Analytics

`/apex:insights` is your portfolio dashboard. It reads `projects.json` and `workspace.json` to compute:

| Analysis | What It Shows |
|----------|--------------|
| **Velocity** | Projects sorted by plan age, phase progress, and loop position |
| **Stall detection** | Projects with plan age > 14 days that aren't completed or deferred |
| **Blocking analysis** | Groups blocked projects by blocker, flags revenue at risk |
| **Cross-project dependencies** | Visualizes dependency chains between projects |
| **Workload by category** | Active project count per category |
| **Revenue exposure** | All revenue-tied projects with blocked flags |
| **Pending handoffs** | PAUL satellites awaiting handoff processing |

Use it during weekly groom cycles, when deciding what to work on next, before stakeholder updates, or whenever a project feels stalled.

---

## Multi-Project Workspaces — BASE + PAUL

Here's where BASE really separates from "just another CLAUDE.md helper."

Most Claude Code users work on one project at a time. But real workspaces have multiple projects — apps, client work, tools, content pipelines — each in their own directory, sometimes their own git repo. Without something managing the workspace level, you lose track. Projects stall silently. Work gets abandoned. Nobody notices until it's a problem.

### What Is PAUL?

[PAUL](https://github.com/ChristopherKahler/paul) is a project orchestration framework for Claude Code. It manages individual project builds through a structured **Plan → Apply → Unify** loop:

- **Plan** — Define what you're building, break it into phases, get alignment before writing code
- **Apply** — Execute the plan phase by phase with built-in progress tracking
- **Unify** — Reconcile what was planned vs what was built, close the loop, start the next milestone

Each PAUL project lives in its own directory with a `.paul/` config folder that tracks the project's state, milestones, and phase history. PAUL is excellent at managing a single project's lifecycle. But it doesn't know about your other projects, your backlog, or your workspace health.

### Where BASE Comes In

BASE is designed to work alongside PAUL as the workspace layer that ties everything together. Think of it as the difference between **project management** and **portfolio management:**

- **PAUL** manages each project: "What phase am I in? What's the plan? What's left to build?"
- **BASE** manages your workspace: "Which of my 6 projects needs attention? Which ones are stalling? What should I work on today?"

### How They Connect

BASE automatically detects and registers PAUL projects across your workspace:

- On session start, a hook scans your workspace for `.paul/paul.json` files and registers any new PAUL projects in `workspace.json` automatically
- If you started using PAUL before BASE, update PAUL to the latest version and run `/paul:register` in any PAUL project directory to generate the `paul.json` manifest. The next time you start a session in your BASE workspace, it picks it up automatically.
- Activity timestamps from each project flow into the workspace manifest so BASE always knows when each project was last touched
- During weekly groom, BASE checks each registered project's health:
  - **Stuck?** — Planning done but implementation stalled for 7+ days
  - **Abandoned?** — No activity for 14+ days with work still incomplete
  - **Drifting?** — Milestone marked complete but no new work started
- You can configure health checks per project — enable or disable them in the manifest

BASE never modifies your projects. It only reads and reports. Each project manages itself through PAUL. BASE manages the workspace those projects live in.

```json
{
  "satellites": {
    "my-saas-app": {
      "path": "apps/my-saas-app",
      "engine": "paul",
      "state": "apps/my-saas-app/.paul/STATE.md",
      "registered": "2026-03-15",
      "groom_check": true,
      "last_activity": "2026-03-17T14:30:00-05:00",
      "phase_name": "Auth System",
      "phase_number": 3,
      "phase_status": "in_progress",
      "loop_position": "APPLY",
      "handoff": false,
      "last_plan_completed_at": "2026-03-15T10:00:00-05:00"
    }
  }
}
```

### Without PAUL

Don't use PAUL? BASE still works as a standalone workspace framework. You still get:

- Data surfaces for tracking any structured information
- Drift detection and grooming for all workspace areas
- Audit strategies for directories, tools, and system files
- The full MCP server for surface CRUD
- Custom surface creation for anything you need

The project detection hook simply has nothing to find. Everything else operates independently.

---

## Creating Custom Surfaces

The five built-in surfaces cover common workspace needs. The real power is creating surfaces for your specific needs.

### `/base:surface create`

A guided workflow that generates everything:

```
> /base:surface create

What does this surface track? → "Client projects and their current phase"

What fields does each item need?
  - name (string, required)
  - company (string, required)
  - phase (enum: discovery, proposal, active, maintenance)
  - monthly_value (number)
  - next_action (string)

How should this appear in Claude's context?
  - Group by: phase
  - Summary format: "[ID] name — company (phase)"
  - Behavior: passive (silent unless asked)

Generating...
  + .base/data/clients.json
  + .base/hooks/clients-hook.py
  + workspace.json updated
  + Hook registered in settings.json
```

Next session, Claude passively knows your client roster without you doing anything.

### `/base:surface convert`

Already have a markdown file with structured data? This command reads it, detects the structure, proposes a JSON schema, migrates the content, and generates everything. Your old `@CLIENTS.md` file becomes a proper data surface with full MCP support.

---

## How the Ecosystem Fits Together

```
┌─────────────────────────────────┐
│  PAUL   (per-project lifecycle) │  Plan → Apply → Unify
├─────────────────────────────────┤
│  CARL   (per-session rules)     │  Load rules based on intent
├─────────────────────────────────┤
│  BASE   (workspace layer)       │  Surfaces, projects, analytics, health
│         + Operator profile      │  Identity alignment across sessions
└─────────────────────────────────┘
```

**All tools are fully independent.** No dependencies between them. Use one, some, or all. References to uninstalled tools are silent — no errors, no noise. If you only install BASE, you'll never see a PAUL or CARL warning.

Together, they turn Claude Code from a per-session coding tool into a managed operating system for AI builders.

<details>
<summary><strong>How they enhance each other when combined</strong></summary>

- **BASE + PAUL** — PAUL projects auto-register with BASE on session start, giving you workspace-level visibility across all your builds. BASE groom checks project health. PAUL handles the project. BASE handles the portfolio.
- **BASE + CARL** — CARL brings dynamic rules and decision memory. BASE brings workspace data and project tracking. Together, Claude has both behavioral guidance (CARL) and situational awareness (BASE). BASE groom can optionally check CARL rule health and surface staged proposals for review.
- **CARL + PAUL** — Independent. Each operates in its own scope (session rules vs project builds).

</details>

**See also:** [AEGIS](https://github.com/ChristopherKahler/aegis) — an optional companion for deep codebase auditing (security, architecture, scalability, compliance). 12 AI agent personas across 14 audit domains. Works standalone or pairs with PAUL for structured remediation.

---

## Design Principles

1. **If it's not current, it's harmful.** Stale context feeds Claude bad information. Maintenance isn't optional — it's the whole point.
2. **Every file earns its place.** Can't explain why it's here in 5 seconds? It moves or dies.
3. **Archive over delete.** When in doubt, archive. You can always delete later. You can't un-delete.
4. **The workspace is the product.** Treat it like production code, not a scratch pad.
5. **One manifest drives everything.** `workspace.json` is the single source of truth. No manual bookkeeping.
6. **Tools register themselves.** Projects auto-register. Surfaces auto-discover. Zero human memory required.
7. **Passive by default.** Claude has awareness. Claude does not nag.

---

## Quick Start

```bash
# 1. Install globally + wire current workspace
npx @chrisai/base --global --workspace

# 2. Open Claude Code
claude

# 3. Run guided workspace setup
/base:scaffold

# 4. Check workspace health anytime
/base:pulse

# 5. Weekly maintenance
/base:groom

# 6. Create a custom surface for anything you want Claude to know about
/base:surface create
```

---

## Requirements

- **Node.js** >= 16.7.0
- **Python 3** (for hooks)
- **[Claude Code](https://claude.ai/code)**

---

## License

MIT — [Chris Kahler](https://github.com/ChristopherKahler)
