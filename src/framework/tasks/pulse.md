<purpose>
Daily workspace activation. Read workspace state, calculate drift, present health dashboard, prime the operator for their session.
</purpose>

<user-story>
As an AI builder, I want a quick workspace health briefing at session start, so that I know what needs attention before I start working.
</user-story>

<when-to-use>
- Start of every work session
- When user says "base pulse", "what's the state of things", "workspace status"
- When the pulse hook detects overdue grooming and injects a prompt
- Entry point routes here via /base:pulse
</when-to-use>

<steps>

<step name="read_state" priority="first">
Read workspace state from `.base/workspace.json` and `.base/data/state.json`.

1. Read `.base/workspace.json` — the manifest
2. Read `.base/data/state.json` — the last known state
3. If either file is missing, suggest running `/base:scaffold` first
4. Extract: last groom date, groom cadence, area list, satellite list
</step>

<step name="calculate_drift">
Check each tracked area against filesystem reality.

For each area in the manifest:
1. Check filesystem timestamps on tracked paths (stat modification dates)
2. Compare against last groom date and area-specific cadence
3. Calculate days overdue (0 if within cadence)
4. Classify: Current (within cadence), Stale (1-2x overdue), Critical (2x+ overdue)

For each registered satellite:
1. Check if state file exists and is readable
2. Extract last modification date
3. Report current phase if parseable

Calculate total drift score: sum of days-overdue across all areas, with Critical areas weighted 2x.
</step>

<step name="present_dashboard">
Present the health dashboard to the operator.

Format:
```
BASE Pulse — {workspace-name}
Last Groom: {date} ({N} days ago)
Drift Score: {score}

| Area | Status | Age | Due |
|------|--------|-----|-----|
...

Satellites:
| Project | Phase | Last Active |
...

{Recommendation based on drift score}
```

Recommendations:
- Drift 0: "Workspace is clean. Proceed normally."
- Drift 1-7: "Minor drift in {areas}. Consider grooming this week."
- Drift 8-14: "Moderate drift. Run /base:groom soon."
- Drift 15+: "Critical drift. Workspace context is stale. Run /base:groom now."
</step>

</steps>

<output>
Health dashboard with drift score, area statuses, satellite health, and recommended next action.
</output>

<acceptance-criteria>
- [ ] All manifest areas checked against filesystem reality
- [ ] Drift score calculated correctly
- [ ] Satellites checked for health
- [ ] Clear recommendation provided based on drift level
- [ ] Dashboard is concise and scannable (not a wall of text)
</acceptance-criteria>
