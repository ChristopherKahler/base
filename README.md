```
  ██████╗  █████╗ ███████╗███████╗
  ██╔══██╗██╔══██╗██╔════╝██╔════╝
  ██████╔╝███████║███████╗█████╗
  ██╔══██╗██╔══██║╚════██║██╔══╝
  ██████╔╝██║  ██║███████║███████╗
  ╚═════╝ ╚═╝  ╚═╝╚══════╝╚══════╝

  Builder's Automated State Engine  ·  v2.3.0
  Workspace orchestration for Claude Code

  ✦ Data Surfaces    ✦ Drift Detection    ✦ Auto-Grooming
  ✦ PAUL Satellites  ✦ MCP Integration    ✦ Session Hooks
```

<div align="center">

[![npm](https://img.shields.io/npm/v/base-framework?color=00d8ff&label=npm&style=flat-square)](https://www.npmjs.com/package/base-framework)
[![Node](https://img.shields.io/badge/node-%3E%3D16.7.0-brightgreen?style=flat-square)](https://nodejs.org)
[![License](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](LICENSE)
[![Claude Code](https://img.shields.io/badge/Claude%20Code-compatible-8b5cf6?style=flat-square)](https://claude.ai/code)

</div>

---

## What is BASE?

BASE keeps your Claude Code workspace from becoming a mess. It scaffolds structure, tracks workspace health, surfaces the right context automatically, and tells you when things go stale — so you spend time building, not maintaining.

**The core pattern:** structured JSON files (data surfaces) + lightweight Python hooks that inject them into Claude's context every session. Claude always knows what's active, what's queued, and where satellites stand — without you having to say it.

---

## Install

```bash
npx base-framework --global --workspace
```

| Flag | What it does |
|------|-------------|
| `--global` | Install commands + framework to `~/.claude` |
| `--workspace` | Install workspace layer (`.base/`) in current directory |
| `--local` | Install commands to `./.claude` instead of global |
| `--config-dir <path>` | Custom Claude config directory |
| `--workspace-dir <path>` | Target a specific workspace path |

**Common flows:**

```bash
# Full install — global commands + current workspace
npx base-framework --global --workspace

# Already have global? Just wire a new workspace
npx base-framework --workspace

# Global only — set up each workspace later with /base:scaffold
npx base-framework --global
```

---

## What Gets Installed

```
~/.claude/                          ← --global
├── commands/base/                  11 slash commands
├── skills/base/                    Entry point (base.md)
└── base-framework/
    ├── tasks/                      pulse, groom, audit, scaffold...
    ├── templates/                  workspace.json, STATE.md, surfaces
    ├── context/                    base-principles.md
    ├── frameworks/                 audit-strategies.md
    └── hooks/                      Session hooks (scaffold source)

./.base/                            ← --workspace
├── workspace.json                  Manifest: surfaces, satellites, groom config
├── data/
│   ├── active.json                 Active projects surface
│   └── backlog.json                Backlog surface
├── hooks/                          Surface injection hooks
├── base-mcp/                       BASE MCP server
└── carl-mcp/                       CARL MCP server

./.claude/
├── hooks/
│   ├── base-pulse-check.py         Drift detection (every session)
│   ├── psmm-injector.py            Per-session meta memory
│   └── satellite-detection.py      PAUL project auto-registration
└── settings.json                   Hook registrations (merged)
```

---

## Commands

After install, open Claude Code and run `/base:scaffold` to complete setup.

| Command | Description |
|---------|------------|
| `/base:scaffold` | Set up BASE in a new or existing workspace |
| `/base:pulse` | Daily workspace health briefing |
| `/base:groom` | Weekly maintenance cycle |
| `/base:audit` | Deep workspace optimization |
| `/base:status` | Quick health check |
| `/base:history` | Workspace evolution timeline |
| `/base:audit-claude-md` | Audit CLAUDE.md, generate recommended version |
| `/base:carl-hygiene` | CARL domain maintenance |
| `/base:surface create` | Create a new data surface (guided) |
| `/base:surface convert` | Convert a markdown file to a data surface |
| `/base:surface list` | Show all registered data surfaces |

---

## Data Surfaces

The core primitive. A data surface is a structured JSON file + a Python hook that injects it into Claude's context every session. Any persistent data you want Claude to passively know about becomes a surface.

```
workspace.json registers it → hook reads it → Claude knows it
```

**Built-in surfaces:**
- `active.json` — Current projects, status, blockers, deadlines
- `backlog.json` — Future work queue, ideas, deferred items

**Create your own:**
```
/base:surface create
```
Guided schema builder. Point it at any data you want surfaced — contacts, clients, API keys, anything. BASE generates the JSON schema, the hook, and wires it automatically.

---

## PAUL Satellite Integration

BASE auto-detects [PAUL](https://github.com/ChristopherKahler/paul) projects in your workspace. Every session, `satellite-detection.py` scans for `.paul/paul.json` files and registers any new projects in `workspace.json`. Weekly groom cycles check satellite health: stale loops, abandoned phases, overdue milestones.

No manual registration. It just works.

---

## Ecosystem

BASE is part of a three-layer workspace system:

| System | Role |
|--------|------|
| **BASE** | Workspace lifecycle — surfaces, grooming, drift detection |
| **CARL** | Dynamic rules engine — just-in-time rule injection |
| **PAUL** | Project orchestration — Plan → Apply → Unify loop |

Each is independent. Use one, some, or all.

---

## Requirements

- Node.js ≥ 16.7.0
- [Claude Code](https://claude.ai/code)
- Python 3 (for hooks)

---

## License

MIT — [Chris Kahler](https://github.com/ChristopherKahler)
