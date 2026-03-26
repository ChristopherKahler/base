# BASE Principles

## Core Laws

1. **If it's not current, it's harmful.** Stale context documents feed AI bad information. Maintenance isn't optional.
2. **Every file earns its place.** If you can't explain why it's here in 5 seconds, it moves or dies.
3. **Archive > delete.** When in doubt, archive. You can always delete later. You can't un-delete.
4. **The workspace is the product.** Treat it like production code, not a scratch pad.
5. **Clean as you go.** The best time to file something correctly is when you create it. The second best time is now.
6. **Scaffold generates manifest. Manifest drives everything.** One configuration point. No manual bookkeeping.
7. **Tools register themselves.** PAUL projects auto-register with BASE. No human memory required.

## Drift Score

Drift is the gap between documented state and actual state. Measured in days-overdue across all tracked areas.

- **0** — Everything current. Workspace is clean.
- **1-7** — Minor drift. Normal during execution sprints. Fix at next groom.
- **8-14** — Moderate drift. Context documents are likely misleading AI. Groom soon.
- **15+** — Critical drift. Sessions are operating on stale context. Groom NOW.

## Maintenance Cadence

| What | Default Cadence | Override |
|------|----------------|---------|
| active.json | Every session or weekly | workspace.json |
| backlog.json | Weekly groom | workspace.json |
| Project directory | Monthly | workspace.json |
| Tools/MCP | Monthly | workspace.json |
| System layer | Monthly | workspace.json |
| Full audit | Quarterly or after major shifts | On demand |

## Backlog Rules

Items have time-based properties enforced by grooming:

- **Added** — auto-set when item enters backlog
- **Review-by** — priority-based: High=7d, Medium=14d, Low=30d
- **Staleness** — 2x review-by threshold. Auto-archive if reached without action.

During groom: items past review-by surface as "decide or kill." Items past staleness auto-archive with a note.

## Graduation Flow

Backlog items don't sit forever. They graduate to active.json when the operator is ready to work on them.

```
BACKLOG (waiting review)
  → active.json TASKS (standalone, bounded work items) via base_add_item MCP
  → active.json PROJECT (complex work warranting its own project entry) via base_add_item MCP
  → DONE (closed with outcome + date)
```

**TASKS vs PROJECTS:** A task is bounded — it has a finish line. "Extract .mcp.json secrets" is a task. "Build the CARL MCP server" might start as a task but could become a project if it grows. The operator decides during groom.

**Graduation is never automatic.** The groom flow asks explicitly: "Ready to work on any backlog items?" The operator decides what graduates and where it lands.

**Items can also move backward:** An active task that loses priority can return to backlog. A project that stalls can move to DEFERRED. Nothing is permanent.

## Scaffold Modes

BASE scaffold operates in two modes:

- **Standard** (`/base:scaffold`) — Data layer only. Creates `.base/` with workspace.json, `.base/data/state.json`, ROADMAP.md. Scans and tracks what exists. Framework-agnostic.
- **Full** (`/base:scaffold --full`) — Data layer + data surfaces. Creates `.base/data/active.json`, `.base/data/backlog.json` from templates. Offers CLAUDE.md audit. The "batteries included" version for AI builders who want the full system.

Standard mode works for any workspace. Full mode provides Chris's proven operational structure.

## File Location

BASE operates strictly out of `.base/`. All data surfaces (active.json, backlog.json, state.json) live in `.base/data/`. Configuration (workspace.json) and documentation (ROADMAP.md) live in `.base/`. Data is accessed via MCP tools (`base_get_surface`, `base_add_item`, `base_update_item`). `.base/data/` is the canonical location for all structured data.
