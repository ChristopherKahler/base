<purpose>
Guided creation of a custom domain phase for /base:weekly. Walks the user through defining what to check, what tools to pull data from, what questions to ask weekly, where the phase fits in the flow, and what output it produces. Every domain phase goes through this same workflow — no presets, no shortcuts.
</purpose>

<user-story>
As an AI builder, I want to add custom check-in areas to my weekly ritual, so that the weekly covers everything that matters to me without being locked into someone else's priorities.
</user-story>

<when-to-use>
- When operator wants to add a domain to their weekly
- After first /base:weekly run (offered when no domain phases exist)
- When operator says "add a domain to my weekly", "I want to track X weekly"
- Entry point routes here via /base:weekly-domain
</when-to-use>

<steps>

<step name="describe_domain" priority="first">
Understand what the operator wants to track weekly.

1. Ask: "What area of your week needs its own check-in?"
2. Offer starter ideas to help them think clearly:
   - "Some common ones people create: revenue tracking, content pipeline, client follow-ups, team sync, health/fitness check, learning log, community engagement."
   - "Or describe something entirely your own."
3. Wait for response
4. Clarify if needed: "So this phase would check on {paraphrase}. What would make this useful to you every week?"

**Wait for clear description before proceeding.**
</step>

<step name="tool_discovery">
Identify data sources and tools for this domain.

1. Ask: "What data should this phase pull? What tools or sources are relevant?"
   - Give examples: "Calendar events in a category, Slack channel messages, MCP server data, project statuses, external dashboard"
2. If the operator is unsure:
   a. Search available MCP tools (use tool listing / grep for relevant keywords)
   b. Present relevant ones: "I found these tools that might be useful: {list}"
   c. Let operator pick or decline
3. If a needed tool doesn't exist:
   - Note it: "That tool doesn't exist yet. I'll note it as a future backlog item."
   - The domain phase still works — it just skips that data source at runtime
4. For each selected data source, capture:
   - Tool name (MCP tool ID)
   - Parameters to pass
   - Human-readable label

**Build the data_sources[] array from this conversation.**
</step>

<step name="define_questions">
Define the weekly questions this phase asks.

1. Ask: "What questions should this phase ask you every week?"
   - Give examples based on their domain:
     - Revenue: "Did I hit my revenue target? What's the pipeline value? Any invoices pending?"
     - Content: "How many pieces published? What's queued? Am I on cadence?"
     - Custom: derive from their description
2. Capture each question as a string
3. Confirm the list: "These are the questions for your {name} phase: {list}. Adjust?"

**Wait for confirmation.**
</step>

<step name="choose_position">
Determine where this phase runs in the weekly flow.

Present the weekly structure:
```
1. Week Review
2. Calendar Audit
3. Workspace Groom
4. Priority Stack
5. Backlog Triage
   --- domain phases run here ---
6. Blockers & Delegation
7. Week Commit
```

Ask: "Where should this phase run? Most domain phases run after backlog triage (position 5) and before blockers (position 6). Sound right, or do you want it somewhere else?"

Options:
- `after_groom` — runs after Phase 3
- `after_priorities` — runs after Phase 4
- `after_backlog` — runs after Phase 5 (default, recommended)
- `before_blockers` — same as after_backlog unless other domains exist (controls ordering among domains)

**Wait for response.**
</step>

<step name="define_output">
Determine what this phase produces.

Ask: "What should this phase output?"
Options:
- **Notes** — just captures your responses as part of the weekly record
- **Calendar events** — creates specific calendar blocks (e.g., "content recording session")
- **Project updates** — updates project statuses or adds tasks
- **Mixed** — any combination

For each output type, capture specifics:
- Calendar: what kind of events, default duration, which calendar
- Project: which project ID to update, what fields
- Notes: just stored in weekly entry (default, always happens)

**Wait for response.**
</step>

<step name="generate_and_save">
Generate the domain phase config and save to weekly.json.

1. Compose the domain phase object:
   ```json
   {
     "id": "{kebab-case-id}",
     "name": "{Display Name}",
     "description": "{one-line description}",
     "position": "{chosen position}",
     "data_sources": [
       {
         "type": "mcp",
         "tool": "{tool_name}",
         "params": {},
         "label": "{human label}"
       }
     ],
     "questions": [
       "{question 1}",
       "{question 2}"
     ],
     "outputs": {
       "type": "notes|calendar|project_update|mixed",
       "details": {}
     },
     "enabled": true,
     "created": "{ISO date}"
   }
   ```
2. Read current `.base/weekly.json`
3. Append to `domain_phases[]`
4. Write updated weekly.json
5. Present:
   ```
   Domain phase created: {name}
   Position: {where it runs}
   Data sources: {list}
   Questions: {count}
   Output: {type}

   This will run during your next /base:weekly.
   Want to create another domain phase, or are you done?
   ```

**Wait for response.**
</step>

</steps>

<output>
New domain phase config added to `.base/weekly.json` -> `domain_phases[]`. Ready to execute on next /base:weekly run.
</output>

<acceptance-criteria>
- [ ] Operator described the domain area clearly
- [ ] Tool discovery performed — available MCP tools searched if operator was unsure
- [ ] Data sources identified and captured with tool names + params
- [ ] Weekly questions defined and confirmed
- [ ] Position in weekly flow chosen
- [ ] Output type specified
- [ ] Domain phase config written to weekly.json
- [ ] Config is valid JSON matching the schema
- [ ] Operator informed of next steps
</acceptance-criteria>
