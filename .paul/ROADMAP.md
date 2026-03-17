# Roadmap: BASE

## Overview

BASE v2 evolves from a workspace state tracker into a full workspace orchestration platform with data surfaces, dual MCP architecture, and extensibility for any Claude Code user. The build path moves from foundational reorganization through new infrastructure to user-facing extensibility.

## Current Milestone

**v2.0 Data Surfaces & Extensibility** (v2.0.0)
Status: In progress
Phases: 1 of 6 complete

## Phases

| Phase | Name | Plans | Status | Completed |
|-------|------|-------|--------|-----------|
| 1 | Data Directory Reorganization | 1/1 | Complete | 2026-03-17 |
| 2 | Hook Template & Contract | TBD | Not started | - |
| 3 | BASE MCP Server | TBD | Not started | - |
| 4 | Active + Backlog Surface Conversion | TBD | Not started | - |
| 5 | Surface Extensibility | TBD | Not started | - |
| 6 | Scaffold v2 & Documentation | TBD | Not started | - |

## Phase Details

### Phase 1: Data Directory Reorganization

**Goal:** Move all JSON data files into .base/data/, update all references, verify nothing breaks.
**Depends on:** Nothing (foundation)
**Research:** Unlikely (internal file moves and path updates)

**Scope:**
- Create .base/data/ and .base/hooks/ directories
- Move psmm.json and staging.json into .base/data/
- Update carl-mcp tool files (psmm.js, staging.js) with new data paths
- Update source packages (.claude/skills/base/packages/carl-mcp/)
- Update psmm-injector.py hook
- Update CARL rule 12 fallback path reference
- Verify all CARL MCP tools still work after move
- Verify PSMM hook injection still works

### Phase 2: Hook Template & Contract

**Goal:** Create the canonical hook template that defines best practices for all surface injection hooks.
**Depends on:** Phase 1 (hooks directory exists)
**Research:** Unlikely (codifying existing patterns)

**Scope:**
- Design and write .base/hooks/_template.py with full contract documentation
- Document DOs and DONTs as inline comments
- Include behavioral directive template (passive awareness, 24h deadline exception)
- Create source package copy at .claude/skills/base/packages/hooks/_template.py
- Test by manually generating a sample hook from the template

### Phase 3: BASE MCP Server

**Goal:** Build the generic surface CRUD MCP server that operates on any registered data surface.
**Depends on:** Phase 1 (data directory structure), Phase 2 (hook template for reference)
**Research:** Unlikely (follows carl-mcp patterns exactly)

**Scope:**
- Build .base/base-mcp/ with surfaces.js tool module
- Implement: base_list_surfaces, base_get_surface, base_get_item, base_add_item, base_update_item, base_archive_item, base_search
- Add surface registration schema to workspace.json
- Register in .mcp.json
- Create source package at .claude/skills/base/packages/base-mcp/
- Test all tools with a dummy surface registration

### Phase 4: Active + Backlog Surface Conversion

**Goal:** Convert ACTIVE.md and BACKLOG.md into structured JSON data surfaces with hooks and MCP tool access.
**Depends on:** Phase 3 (BASE MCP operational)
**Research:** Unlikely (analyzing existing markdown to design schemas)

**Scope:**
- Design active.json schema from ACTIVE.md content analysis
- Design backlog.json schema from BACKLOG.md content analysis
- Migrate ACTIVE.md content into active.json
- Migrate BACKLOG.md content into backlog.json
- Generate active-hook.py and backlog-hook.py from template
- Register hooks in .claude/settings.json
- Register surfaces in workspace.json
- Test: hook injection, MCP CRUD, passive behavioral compliance
- Decide fate of markdown files (keep as generated views, archive, or delete)

### Phase 5: Surface Extensibility

**Goal:** Build /base:surface commands so any user can create custom data surfaces without manual wiring.
**Depends on:** Phase 4 (proven surface pattern from active + backlog)
**Research:** Unlikely (templating existing patterns)

**Scope:**
- Build /base:surface create task file (guided schema builder → JSON + hook + registration)
- Build /base:surface convert task file (markdown analysis → schema proposal → migration)
- Build /base:surface list task file
- Update base.md entry point with new commands
- End-to-end test: create a custom surface from scratch
- End-to-end test: convert a sample @-mentioned markdown file

### Phase 6: Scaffold v2 & Documentation

**Goal:** Update scaffold to install the full v2 structure and document everything for distribution.
**Depends on:** Phase 5 (all surface infrastructure complete)
**Research:** Unlikely (updating existing scaffold)

**Scope:**
- Update scaffold.md task file with v2 steps (data dir, hooks dir, MCP servers, default surfaces)
- Update base-principles.md with Data Surface concepts
- Update workspace-json.md template with surfaces schema
- Update CLAUDE.md references
- Create BASE teaching notes for "From Zero to Dangerous" course
- Document the Data Surface pattern as standalone concept
- Verify scaffold creates full v2 structure end-to-end

---
*Roadmap created: 2026-03-17*
*Last updated: 2026-03-17*
