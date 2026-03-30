<purpose>
Guided weekly review and planning ritual. Walks the operator through 8 core phases sequentially, executes custom domain phases, and commits the week with calendar events created. Designed as one of two cadence rituals (daily closes the day, weekly closes the week and opens the next).
</purpose>

<user-story>
As an AI builder, I want a structured weekly ritual that reviews my week, plans the next, maintains my workspace, and locks in priorities with real calendar events, so that I start every week with clarity instead of reacting to whatever's loudest.
</user-story>

<when-to-use>
- Weekly (typically Sunday evening)
- When operator says "run my weekly", "weekly review", "time for my weekly"
- Entry point routes here via /base:weekly
</when-to-use>

<steps>

<step name="initialize" priority="first">
Load config and establish session context.

1. Read `.base/weekly.json`
   - If file doesn't exist: create it with empty defaults (see schema below)
   - Parse: domain_phases[], calendar_rules[], daily_logs[], history[]
2. Get last weekly entry from history[] (if any)
   - Note: week_of, priorities set, priorities completed
3. Get current date context (from hook injection or system)
4. Present:
   ```
   Weekly — Week of {Monday date} to {Sunday date}
   Last weekly: {date or "first run"}
   Daily logs this week: {count}
   Domain phases configured: {count}
   ```
5. "Ready to start? (You can skip any phase by saying 'skip')"

**Wait for confirmation.**
</step>

<step name="week_review">
Phase 1: Review the past week.

**If daily logs exist** (daily_logs[] entries from the past 7 days):
1. Summarize: days logged, patterns in wins/misses, energy trends
2. Note any recurring blockers or themes
3. Present the summary to the operator

**If no daily logs:**
1. Note: "No daily logs found this week. Phase 1 is reflective-only."

**Always ask:**
- "What went well this week?"
- "What didn't land?"
- "Anything surprising or worth noting?"

Capture the operator's reflection. This goes into the weekly entry.

**Wait for responses.**
</step>

<step name="calendar_audit">
Phase 2: Audit and plan the calendar.

1. Use `list_events` MCP tool — pull events for the next 7 days
   - If multiple calendars available (personal + family), pull all
   - If calendar MCP unavailable: skip to manual questions, note the gap
2. Apply display rules: show all events with real titles during audit (rules only apply on creation)
3. Present the week's schedule in a clean format:
   ```
   Monday 3/31:
     9:00 AM — Meeting with Charlie
     2:00 PM — Coaching call (Amee)
   Tuesday 4/1:
     (open)
   ...
   ```
4. Ask:
   - "Anything missing from the calendar?"
   - "Where do you want deep work blocks?"
   - "Any family commitments to add?"
   - "Any conflicts to resolve?"
5. Collect requested additions/changes (created in Phase 8)

**Wait for responses.**
</step>

<step name="workspace_groom">
Phase 3: Workspace maintenance.

Run groom logic inline — NOT as a separate /base:groom invocation.

1. Read drift score from `base_get_state` or state.json
2. Read stale areas from base-pulse data
3. Walk through each stale area:
   - Projects: quick status check on active/blocked items
   - Clients: any updates needed?
   - Content: pipeline current?
   - Other flagged areas from pulse
4. For each area reviewed:
   - Update timestamps via `base_update_area`
   - Note changes made
5. Record groom via `base_record_groom`
6. Update drift via `base_update_drift`
7. Report: "Drift score: {before} -> {after}. {N} areas groomed."

**Voice-friendly: walk through one area at a time, wait for response on each.**
</step>

<step name="priority_stack">
Phase 4: Set the week's priorities.

1. Pull context:
   - Operator north star (from operator.json / hook data)
   - Active projects with upcoming deadlines (from active-awareness)
   - Stale urgent/high items
   - Last week's priorities and their status (from previous weekly entry)
2. If previous priorities exist:
   - Report: "Last week's priorities: {list}. Status: {completed/carried/dropped}"
3. Suggest 3-5 outcome-based priorities:
   - Frame as outcomes, not tasks: "Ship X" not "Work on X"
   - Align each to north star or active project
   - Weight toward revenue-generating and deadline-driven work
4. Present: "Here are my suggested priorities for this week: {list}. Adjust?"
5. Finalize the stack after operator input

**Wait for approval or adjustments.**
</step>

<step name="backlog_triage">
Phase 5: Process the backlog.

1. Pull backlog items via `base_list_projects` (status=backlog)
2. Identify items with:
   - review_by date passed (overdue)
   - review_by date within 7 days (upcoming)
   - No review_by date and older than 14 days (stale)
3. Present overdue items first: "These are past their review date: {list}"
4. For each flagged item, ask:
   - **Keep** — set new review_by date
   - **Graduate** — move to active (update status via `base_update_project`)
   - **Kill** — archive via `base_archive_project`
5. After processing flagged items: "Anything new to add to the backlog?"
6. Capture new items via `base_add_project` (status=backlog)

**Walk through one item at a time.**
</step>

<step name="domain_phases">
Phase 6: Execute custom domain phases.

1. Read `weekly.json` -> `domain_phases[]` (only enabled ones)
2. If no domain phases configured:
   - "No domain phases configured. You can add one anytime with /base:weekly-domain."
   - Skip to Phase 7
3. Sort by configured position (after_groom, after_priorities, after_backlog, before_blockers)
4. For each domain phase:
   a. Announce: "Domain phase: {name} — {description}"
   b. Pull data from configured data_sources[]:
      - For each source: call the specified MCP tool with configured params
      - If tool unavailable: note it, continue with remaining sources
   c. Present pulled data
   d. Ask configured questions[] one at a time
   e. Capture responses
   f. Produce configured output (notes, calendar events, project updates)
5. Store domain phase results in weekly entry -> domains{}

**Each domain phase is self-contained — failure in one doesn't block others.**
</step>

<step name="blockers_delegation">
Phase 7: Identify blockers and queue follow-ups.

**If Slack MCP available:**
1. Pull recent messages from relevant channels/DMs (last 7 days)
2. Surface threads with pending action or unanswered questions
3. Present: "Here are open threads that may need follow-up: {list}"
4. For each: "Send a nudge this week? (becomes a calendar reminder or note)"

**If Slack MCP unavailable:**
1. Ask: "What's currently blocked?"
2. Ask: "Who do you need to follow up with this week?"

**Always:**
3. Cross-reference with active projects that have `blocked` status
4. Generate follow-up list: person, action, urgency
5. Ask which follow-ups should become calendar reminders

**Wait for responses.**
</step>

<step name="week_commit">
Phase 8: Lock in the week.

1. Summarize everything from this session:
   ```
   WEEK COMMIT — {date range}

   Priorities:
   1. {outcome}
   2. {outcome}
   ...

   Calendar changes:
   - {new event/block} on {day}
   ...

   Groom: Drift {before} -> {after}
   Backlog: {N} reviewed, {N} graduated, {N} killed, {N} new
   Follow-ups: {list}
   ```
2. Ask: "Confirm? I'll create the calendar events and log the weekly."

**On confirmation:**
3. Create calendar events via `create_event` MCP tool:
   - Deep work blocks
   - Follow-up reminders
   - Any additions from Phase 2
   - Apply calendar_rules[] to event titles on creation:
     - For each rule where type=title_transform and enabled=true:
       - If event title matches rule.match regex: replace with rule.replace
       - If rule.calendars specified: only apply to those calendars
4. Write weekly entry to `weekly.json` -> `history[]`:
   - Include all phase outputs (review, calendar, groom, priorities, backlog, domains, blockers)
   - Compute changeover metrics vs previous entry (priorities carried/completed/dropped, drift delta, backlog net)
5. Report:
   ```
   Weekly complete.
   {N} calendar events created.
   Drift score: {X}.
   Next weekly: {suggested date}.
   ```

**Wait for confirmation before creating events.**
</step>

</steps>

<schemas>

## weekly.json — Initial Empty Config

When weekly.json doesn't exist, create with:

```json
{
  "version": "1.0",
  "created": "{ISO date}",
  "calendar_rules": [],
  "domain_phases": [],
  "daily_logs": [],
  "history": []
}
```

## Daily Log Entry Schema (forward-looking — consumed by Phase 1, written by /base:daily)

```json
{
  "date": "YYYY-MM-DD",
  "logged_at": "ISO datetime",
  "reflection": "string",
  "wins": ["string"],
  "misses": ["string"],
  "energy": "high|medium|low",
  "domains": {
    "domain_id": {
      "activities": ["string"],
      "metrics": {}
    }
  },
  "blockers_surfaced": ["string"],
  "tomorrow_intent": "string"
}
```

## Weekly History Entry Schema (written by Phase 8)

```json
{
  "id": "unique-id",
  "week_of": "YYYY-MM-DD (Monday)",
  "run_date": "YYYY-MM-DD",
  "run_at": "ISO datetime",
  "review": {
    "reflection": "string",
    "daily_logs_count": 0,
    "patterns": ["string"]
  },
  "calendar": {
    "events_existing": 0,
    "events_created": 0,
    "conflicts_resolved": 0,
    "deep_work_blocks": 0
  },
  "groom": {
    "drift_score_before": 0,
    "drift_score_after": 0,
    "stale_areas_resolved": [],
    "projects_touched": 0
  },
  "priorities": [
    {
      "outcome": "string",
      "aligned_to": "project_id or north_star",
      "status": "pending"
    }
  ],
  "backlog": {
    "items_reviewed": 0,
    "graduated": 0,
    "deferred": 0,
    "killed": 0,
    "new_captured": 0
  },
  "domains": {},
  "blockers": {
    "identified": 0,
    "follow_ups_queued": 0,
    "resolved_since_last": 0
  },
  "changeover": {
    "priorities_carried_over": 0,
    "priorities_completed": 0,
    "priorities_dropped": 0,
    "drift_delta": 0,
    "backlog_net": 0
  }
}
```

</schemas>

<output>
Weekly entry logged to weekly.json. Calendar events created. Workspace groomed. Priorities locked. Backlog current. Operator walks away with a clear week ahead.
</output>

<acceptance-criteria>
- [ ] Weekly.json loaded or created on first run
- [ ] Phase 1: Daily logs consumed (if available), reflection captured
- [ ] Phase 2: Calendar pulled and reviewed, additions collected
- [ ] Phase 3: Groom executed inline, drift score updated
- [ ] Phase 4: 3-5 outcome-based priorities set, aligned to north star
- [ ] Phase 5: Overdue backlog items surfaced and processed
- [ ] Phase 6: Domain phases executed (if configured), results captured
- [ ] Phase 7: Blockers identified, follow-ups queued
- [ ] Phase 8: Summary confirmed, calendar events created with rules applied, weekly entry logged
- [ ] Each phase skippable without breaking the flow
- [ ] Operator confirmed at each decision point (voice-friendly pacing)
</acceptance-criteria>
