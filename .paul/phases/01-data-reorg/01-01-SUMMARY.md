---
phase: 01-data-reorg
plan: 01
subsystem: infra

requires: []
provides:
  - .base/data/ directory for all future data surfaces
  - .base/hooks/ directory for future surface injection hooks
  - Updated path references across all CARL MCP tools, source packages, and hooks
affects: [02-hook-template, 03-base-mcp, 04-surface-conversion]

key-files:
  created:
    - .base/data/psmm.json
    - .base/data/staging.json
  modified:
    - .base/carl-mcp/tools/psmm.js
    - .base/carl-mcp/tools/staging.js
    - .claude/skills/base/packages/carl-mcp/tools/psmm.js
    - .claude/skills/base/packages/carl-mcp/tools/staging.js
    - .claude/hooks/psmm-injector.py
    - .carl/global

key-decisions:
  - "Copy-verify-delete over mv for data migration safety"

patterns-established:
  - "All JSON data surfaces live in .base/data/"
  - "Path helper functions (getPsmmPath, getStagingPath) are the single point of path resolution"

duration: ~10min
started: 2026-03-17T09:36:00-05:00
completed: 2026-03-17T09:41:00-05:00
---

# Phase 1 Plan 01: Data Directory Reorganization Summary

**Migrated psmm.json and staging.json into .base/data/, updated all 6 path references across MCP tools, source packages, hook, and CARL rule — zero regressions.**

## Performance

| Metric | Value |
|--------|-------|
| Duration | ~10 min |
| Started | 2026-03-17T09:36:00-05:00 |
| Completed | 2026-03-17T09:41:00-05:00 |
| Tasks | 2 completed |
| Files modified | 8 (2 moved, 6 path updates) |

## Acceptance Criteria Results

| Criterion | Status | Notes |
|-----------|--------|-------|
| AC-1: Data directory exists with migrated files | Pass | .base/data/ has psmm.json + staging.json, originals removed, JSON validated |
| AC-2: CARL MCP tools read/write from new paths | Pass | getPsmmPath and getStagingPath updated in both installed tools |
| AC-3: PSMM injector hook reads from new path | Pass | PSMM_FILE points to .base/data/psmm.json, confirmed working after session restart |
| AC-4: CARL rule 12 fallback path is current | Pass | Changed from .claude/session-context/psmm.json to .base/data/psmm.json |
| AC-5: Source packages mirror installed changes | Pass | Both source package tools updated identically |

## Accomplishments

- Created .base/data/ and .base/hooks/ directory structure (foundation for all future surfaces)
- Migrated 2 JSON data files with copy-verify-delete safety pattern (zero data loss)
- Updated 6 files across 3 layers (installed tools, source packages, hook + rule)
- CARL MCP server confirmed starting cleanly with 19 tools post-migration
- PSMM hook injection confirmed working in live session after restart

## Files Created/Modified

| File | Change | Purpose |
|------|--------|---------|
| `.base/data/psmm.json` | Moved from `.base/` | Per-session meta memory data |
| `.base/data/staging.json` | Moved from `.base/` | Rule proposal staging data |
| `.base/hooks/` | Created (empty) | Ready for Phase 2 hook template |
| `.base/carl-mcp/tools/psmm.js` | Modified | getPsmmPath() → .base/data/ |
| `.base/carl-mcp/tools/staging.js` | Modified | getStagingPath() → .base/data/ |
| `.claude/skills/base/packages/carl-mcp/tools/psmm.js` | Modified | Source package mirror |
| `.claude/skills/base/packages/carl-mcp/tools/staging.js` | Modified | Source package mirror |
| `.claude/hooks/psmm-injector.py` | Modified | PSMM_FILE → .base/data/ |
| `.carl/global` | Modified | Rule 12 fallback path updated |

## Decisions Made

| Decision | Rationale | Impact |
|----------|-----------|--------|
| Copy-verify-delete over mv | Safer migration — verify integrity before removing originals | Pattern for future data migrations |

## Deviations from Plan

None — plan executed exactly as written.

## Issues Encountered

None.

## Next Phase Readiness

**Ready:**
- .base/data/ exists and is the canonical data directory
- .base/hooks/ exists, ready for hook template (Phase 2)
- All tool path resolution follows the getPsmmPath/getStagingPath pattern — new surfaces follow this convention

**Concerns:**
- None

**Blockers:**
- None

---
*Phase: 01-data-reorg, Plan: 01*
*Completed: 2026-03-17*
