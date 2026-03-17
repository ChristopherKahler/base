# PAUL Session Handoff

**Session:** 2026-03-17 09:00 - 14:00 CDT (~5 hours)
**Project:** BASE v2.0 (apps/base/)
**Context:** Massive build session — BASE v2 from spec to distributable app in one session

---

## Session Accomplishments

**7 PAUL phases planned, applied, and unified:**

1. **Phase 1: Data Directory Reorganization** — Created .base/data/ and .base/hooks/, moved psmm.json + staging.json, updated all path references (6 files)
2. **Phase 2: Hook Template & Contract** — Created .base/hooks/_template.py with full DO/DON'T contract
3. **Phase 3: BASE MCP Server** — Built .base/base-mcp/ with 7 CRUD tools (list, get_surface, get_item, add, update, archive, search). Registered in .mcp.json
4. **Phase 4: Active + Backlog Surface Conversion** — Migrated ACTIVE.md → active.json (12 items + 10 archived), BACKLOG.md → backlog.json (8 items). Created injection hooks with staleness detection. Registered surfaces in workspace.json
5. **Phase 5: Surface Extensibility** — Created /base:surface-create, /base:surface-convert, /base:surface-list skill task files + global command wrappers
6. **Phase 6: Package as Distributable App** — Copied all 36 source files into apps/base/src/, created package.json + bin/install.js
7. **Phase 7: Command Wrappers** — Created 8 command wrappers for existing BASE commands (pulse, groom, audit, scaffold, status, history, audit-claude-md, carl-hygiene). Total: 11 /base:* commands

**Cleanup completed:**
- Deleted redundant get-backlog-stale.py + removed from settings.json
- Moved base-pulse-check.py and psmm-injector.py from .claude/hooks/ to .base/hooks/ + updated settings.json paths
- Verified decisions redesign is complete (5 domain files, 34 decisions)
- Confirmed CARL hygiene task file exists (just never been run)

**Ad-hoc additions:**
- Staleness detection in hooks (priority-based thresholds, uses `updated` field from base_update_item)
- Consolidated two src/hooks/ directories into one in apps/base/

---

## Decisions Made

| Decision | Rationale | ID |
|----------|-----------|-----|
| Everything in .base/ | Single location, easy to manage | Spec #1 |
| Two MCP servers (carl-mcp + base-mcp) | CARL ships standalone, BASE wraps it | Spec #2 |
| One hook per surface | Isolation — editing one can't regress another | Spec #3 |
| All hooks live in .base/hooks/ | Separation from user's .claude/hooks/, easier to manage | Session cleanup |
| paul.json satellite manifest | Standardized file for BASE to detect PAUL projects | paul-012 |
| Session-start satellite detection hook | Auto-registers new PAUL projects in workspace.json | global-002 |
| Groom checks satellite health (configurable) | Default on, per-project opt-out | global-003 |
| BASE framework to global (PENDING) | Multi-workspace support — Chris leaning yes, not confirmed | global-004 |
| Bidirectional staleness (PAUL→BASE) | PAUL updates trigger timestamps BASE can track | Deferred |
| No symlinks for dev workflow | Breaks hook __file__ resolution, adds complexity | Session correction |
| Staleness uses `updated` field, fallback to `added` | base_update_item already sets `updated` automatically | Session ad-hoc |

---

## What's Operational Right Now

- `<active-awareness>` and `<backlog-awareness>` hooks fire every prompt
- BASE MCP server (7 tools) live and callable
- CARL MCP (19 tools) paths updated, working
- 11 `/base:*` slash commands visible in skill list
- 2 data surfaces registered in workspace.json
- Staleness detection active in hook injections
- apps/base/ repo with 9 commits, 44+ source files

---

## Discussed Not Built (v2.1 items)

| Item | Owner | Dependency | Status |
|------|-------|-----------|--------|
| paul.json satellite manifest + template | PAUL framework | None | **Build next — in apps/paul-framework/** |
| Session-start satellite detection hook | BASE | paul.json exists | Blocked by paul.json |
| Groom satellite health checks | BASE | satellite detection hook | Blocked by detection hook |
| Bidirectional staleness (PAUL→BASE) | Both | paul.json + detection hook | Blocked |
| BASE framework to global | BASE | Chris's decision | **Pending confirmation** |

**Critical path:** paul.json → detection hook → groom health checks → bidirectional staleness

---

## Open Questions

1. **BASE framework global migration** — Move skill + commands to ~/.claude/base-framework/? Chris leaning yes for multi-workspace support. Needs final call.
2. **apps/base/src/hooks/ vs .base/hooks/ sync** — Source files in apps/base/src/hooks/ and installed copies in .base/hooks/ can drift. install.js handles new users, but Chris needs a workflow for keeping his own install current when developing in apps/base/.
3. **CLAUDE.md references ACTIVE.md and BACKLOG.md** — These are now data surfaces (active.json, backlog.json). CLAUDE.md still references the markdown files. Should be updated.

---

## Reference Files for Next Session

```
# BASE project
@apps/base/.paul/STATE.md
@apps/base/.paul/ROADMAP.md
@apps/base/.paul/PROJECT.md
@apps/base/BASE-V2-SPEC.md

# PAUL project (for paul.json work)
@apps/paul-framework/.paul/STATE.md
@apps/paul-framework/src/templates/
@apps/paul-framework/src/workflows/init-project.md

# Workspace config
@.base/workspace.json
@.base/data/active.json
@.base/data/backlog.json
@.mcp.json
@.claude/settings.json
```

---

## Prioritized Next Actions

| Priority | Action | Effort | Where |
|----------|--------|--------|-------|
| 1 | Build paul.json template + update PAUL init | M | apps/paul-framework/ |
| 2 | Build satellite detection hook (session-start) | S | apps/base/ → .base/hooks/ |
| 3 | Update groom task file with satellite health checks | M | apps/base/src/skill/tasks/groom.md |
| 4 | Decide: BASE framework to global or stay workspace-local | Decision | Discussion |
| 5 | Update CLAUDE.md to reference data surfaces instead of ACTIVE.md/BACKLOG.md | S | CLAUDE.md |
| 6 | Commit parent workspace changes from this session | S | chris-ai-systems/ |

---

## State Summary

**BASE (apps/base/):** v2.0 milestone complete (7/7 phases). 9 commits. Ready for v2.1 planning.
**PAUL (apps/paul-framework/):** v1.0 complete. Needs new milestone for paul.json satellite manifest.
**Parent workspace:** Has uncommitted changes from this session (hook moves, settings.json, deleted files).

**Next session:** Start fresh in apps/paul-framework/ for paul.json work, or commit parent + start v2.1 planning in apps/base/.

**Resume:** `/paul:resume` → read this handoff

---

*Handoff created: 2026-03-17T14:00:00-05:00*
