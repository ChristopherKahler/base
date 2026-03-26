<purpose>
Structured weekly maintenance cycle. Walk through each workspace area, review with operator, enforce backlog time-based rules, graduate ready items, and log the groom.
</purpose>

<user-story>
As an AI builder, I want a guided workspace maintenance session, so that my context documents stay current, my backlog items graduate when ready, and my workspace doesn't drift.
</user-story>

<when-to-use>
- Weekly (on configured groom day)
- When pulse reports overdue grooming
- When user says "base groom", "let's groom", "workspace maintenance"
- Entry point routes here via /base:groom
</when-to-use>

<steps>

<step name="assess_scope" priority="first">
Determine what needs grooming.

1. Read `.base/workspace.json` manifest
2. Read `.base/data/state.json` for last groom dates per area
3. Identify which areas are due for grooming (past their cadence)
4. Sort by staleness (most overdue first)
5. Present: "Groom session starting. {N} areas due for review: {list}. Estimated time: {N*5} minutes."

**Wait for operator confirmation before proceeding.**
</step>

<step name="groom_active">
Review active.json — the working memory for projects and tasks.

1. Read `.base/data/active.json` (or use `base_get_surface("active")` MCP tool)
2. Present summary: "{N} projects, {N} tasks, last updated {date}"
3. For each project: "Still active? Status changed? Next action current?"
4. For each task in TASKS section: "Done? Still in progress? Blocked?"
5. Close completed tasks (move to DONE/CLOSED with outcome + date)
6. Ask: "Anything new to add?"
7. Update timestamp

Voice-friendly: walk through one entry at a time, wait for response.
</step>

<step name="groom_backlog">
Review backlog.json — enforce time-based rules AND handle graduation.

**Time-based enforcement:**
1. For each item, check Added date against review-by threshold:
   - High priority: 7 days
   - Medium priority: 14 days
   - Low priority: 30 days
2. Items past review-by → surface: "These items need a decision: {list}"
3. Items past staleness (2x review-by) → "Auto-archiving: {list} (past {N} days without action)"
4. Process operator decisions on each flagged item

**Graduation check:**
5. For each remaining item, ask: "Ready to work on any of these?"
6. If yes — determine destination:
   - **As a TASK:** Item is bounded, standalone work. Move to active.json via `base_add_item` MCP tool.
     Include: name, status, next action, blocked, notes fields.
   - **As a PROJECT:** Item is complex enough to warrant its own project entry.
     Move to active.json via `base_add_item` MCP tool with full project format.
   - **Stay in backlog:** Not ready yet. Keep with updated review-by date.
7. For graduated items: use `base_update_item` / `base_add_item` MCP tools to move between surfaces
8. Update backlog.json via MCP

**The graduation question is explicit every groom.** Items don't graduate silently — the operator decides.
</step>

<step name="groom_directories">
Review directory-type areas (projects/, clients/, tools/).

For each directory area due for grooming:
1. List contents
2. Flag anything that looks orphaned or new since last groom
3. Ask: "Anything to archive, delete, or reclassify?"
4. Execute approved changes
</step>

<step name="groom_satellites">
Review PAUL satellite project health.

1. Read `.base/workspace.json` — collect all satellite entries where `groom_check: true`
2. If no satellites have `groom_check: true` → skip this step silently
3. For each eligible satellite:
   a. Read its STATE.md at the path in `satellite.state` (relative to workspace root)
   b. If STATE.md is missing or unreadable → note as "⚠️ {name}: STATE.md not found"
   c. Get last activity timestamp:
      - PRIMARY: read `satellite.last_activity` from workspace.json entry (ISO timestamp written by session-start hook from paul.json)
      - FALLBACK: if `last_activity` not present in workspace.json, parse "Last activity" line from the satellite's STATE.md
      - If neither available → note as "⚠️ {name}: cannot determine last activity"
   d. Parse "Loop Position" section from STATE.md → extract PLAN/APPLY/UNIFY markers (✓ = done, ○ = pending)
   e. Evaluate health criteria:
      - **STUCK LOOP**: Loop shows PLAN ✓ APPLY ○ or PLAN ✓ APPLY ✓ UNIFY ○, AND last activity > 7 days ago
      - **ABANDONED PHASE**: Last activity > 14 days ago AND milestone status is not COMPLETE
      - **MILESTONE DRIFT**: Milestone marked COMPLETE, loop shows ○ ○ ○ (no new milestone started), AND last activity > 14 days ago
4. Collect all issues across satellites
5. If issues found: surface as:
   ```
   ⚠️ Satellite health issues:
     - {satellite-name}: {issue type} (last active: {date})
   ```
6. If no issues: output single line "Satellites: all healthy ({N} checked)"

**Report only — do NOT auto-fix.** Operator decides what to do with flagged satellites.
</step>

<step name="groom_system">
Review system layer areas (hooks, commands, skills, CARL).

1. Quick scan for obvious dead items
2. Only flag if something clearly wrong
3. Ask: "Any system changes to note?"
4. If CARL hygiene is enabled (workspace.json `carl_hygiene.proactive: true`):
   - Check `.carl/staging/` for pending proposals
   - Check rule timestamps for staleness
   - Surface: "{N} staged proposals, {N} stale rules — run /base:carl-hygiene?"
</step>

<step name="log_groom">
Record the groom session.

1. Update `.base/data/state.json` with new timestamps for all groomed areas
2. Reset drift score
3. Write groom summary to `.base/grooming/{YYYY}-W{NN}.md`:
   ```markdown
   # Groom Summary — Week {NN}, {YYYY}

   **Date:** {YYYY-MM-DD}
   **Areas Reviewed:** {list}
   **Drift Score:** {before} → 0

   ## Changes
   - {what changed}

   ## Graduated from Backlog
   - {item} → active.json TASKS / PROJECT

   ## Archived / Killed
   - {item} (reason)

   ## Next Groom Due
   {YYYY-MM-DD}
   ```
4. Report: "Groom complete. Drift score: 0. Next groom due: {date}."
</step>

</steps>

<output>
Updated workspace state. All due areas reviewed and current. Backlog time-based rules enforced. Ready items graduated. Groom summary logged.
</output>

<acceptance-criteria>
- [ ] All overdue areas reviewed with operator
- [ ] active.json and backlog.json updated
- [ ] Backlog time-based rules enforced (review-by, staleness)
- [ ] Graduation question asked explicitly for backlog items
- [ ] Graduated items moved to active.json (TASKS or PROJECT)
- [ ] state.json updated with new groom date
- [ ] Groom summary written to grooming/ directory
- [ ] Drift score reset to 0
- [ ] Operator confirmed completion of each area
</acceptance-criteria>
