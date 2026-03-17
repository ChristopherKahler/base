---
phase: 04-surface-conversion
plan: 01
subsystem: data

requires:
  - phase: 02-hook-template
    provides: Hook template contract
  - phase: 03-base-mcp
    provides: BASE MCP server with CRUD tools, workspace.json surfaces section
provides:
  - Active work surface (active.json + active-hook.py + workspace.json registration)
  - Backlog surface (backlog.json + backlog-hook.py + workspace.json registration)
  - Staleness detection in hook injections (priority-based thresholds)
affects: [05-extensibility, 06-scaffold]

key-files:
  created:
    - .base/data/active.json
    - .base/data/backlog.json
    - .base/hooks/active-hook.py
    - .base/hooks/backlog-hook.py
    - .claude/skills/base/packages/hooks/active-hook.py
    - .claude/skills/base/packages/hooks/backlog-hook.py
  modified:
    - .base/workspace.json
    - .claude/settings.json

key-decisions:
  - "Archived items use ACT-ARC-NNN prefix to distinguish from active IDs"
  - "CoreCal mapped as 'medium' priority despite 'high' section placement — text says 'low urgency'"
  - "Staleness thresholds are priority-based: urgent=3d, high=5d, medium=7d, ongoing=14d, deferred=30d"
  - "Staleness uses 'updated' field (set by base_update_item) with fallback to 'added' — real-time maintenance is primary loop, hygiene is fallback"
  - "Original ACTIVE.md and BACKLOG.md preserved — not deleted"

patterns-established:
  - "Data surfaces follow: JSON file + hook + workspace.json registration + settings.json hook entry"
  - "Hooks show staleness indicators per item based on priority-specific thresholds"
  - "base_update_item automatically resets staleness clock via 'updated' timestamp"

duration: ~15min
started: 2026-03-17T10:48:00-05:00
completed: 2026-03-17T11:23:00-05:00
---

# Phase 4 Plan 01: Active + Backlog Surface Conversion Summary

**Migrated ACTIVE.md (12 items + 10 archived) and BACKLOG.md (8 items) into structured JSON data surfaces with priority-grouped injection hooks and staleness detection.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | ~15 min (including ad-hoc staleness feature) |
| Started | 2026-03-17T10:48:00-05:00 |
| Completed | 2026-03-17T11:23:00-05:00 |
| Tasks | 3 completed + 1 ad-hoc modification |
| Files created | 6 (2 data + 2 hooks + 2 source packages) |
| Files modified | 2 (workspace.json, settings.json) |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: active.json with all items | Pass | 12 items + 10 archived, all fields populated |
| AC-2: backlog.json with all items | Pass | 8 items with checklists preserved |
| AC-3: Hooks generate correct output | Pass | Both output priority-grouped XML with passive awareness + staleness indicators |
| AC-4: Surfaces registered in workspace.json | Pass | Both surfaces with schemas (id_prefix, required_fields, priority_levels) |
| AC-5: Hooks registered in settings.json | Pass | Absolute paths for both hooks |
| AC-6: Source package mirrors | Pass | Identical copies including staleness feature |

## Accomplishments

- Migrated 30 total entries (12 active + 10 archived + 8 backlog) with zero data loss
- Both hooks fire live in context with priority grouping, blockers, deadlines, review dates
- BASE MCP `base_list_surfaces` returns both surfaces with correct item counts
- Ad-hoc feature: staleness detection with priority-based thresholds
  - Hooks show days since last update per item
  - Items exceeding threshold flagged as STALE
  - `base_update_item` automatically resets staleness via `updated` timestamp
  - Real-time maintenance encouraged; hygiene/groom becomes the fallback

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `.base/data/active.json` | Created | 12 active items + 10 archived from ACTIVE.md |
| `.base/data/backlog.json` | Created | 8 backlog items from BACKLOG.md |
| `.base/hooks/active-hook.py` | Created | Priority-grouped injection with staleness detection |
| `.base/hooks/backlog-hook.py` | Created | Priority-grouped injection with staleness detection |
| `.base/workspace.json` | Modified | Surface registrations with schemas |
| `.claude/settings.json` | Modified | Hook entries with absolute paths |
| `.claude/skills/base/packages/hooks/*` | Created | Source package mirrors |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| Priority-based staleness thresholds | Urgent items go stale fast (3d), deferred items are patient (30d) | Natural pressure to maintain high-priority items |
| `updated` field over `added` for staleness | MCP tool already sets `updated` on every write — staleness resets automatically | Zero-friction maintenance loop |
| Keep original markdown files | Reference value, future cleanup decision | No data loss risk, slight disk overhead |

## Deviations from Plan

| Deviation | Reason |
|-----------|--------|
| Added staleness detection to hooks | Ad-hoc request during apply — priority-based thresholds added to both hooks post-creation |

## Issues Encountered

None.

## Next Phase Readiness

**Ready:**
- Two proven surface examples for Phase 5 extensibility to reference
- Hook pattern with staleness detection established
- BASE MCP tools operational with registered surfaces
- Full data pipeline proven: JSON → hook injection → MCP CRUD → staleness tracking

**Concerns:**
- get-backlog-stale.py hook (in .claude/hooks/) is now redundant with backlog-hook.py — cleanup deferred

**Blockers:**
- None

---
*Phase: 04-surface-conversion, Plan: 01*
*Completed: 2026-03-17*
