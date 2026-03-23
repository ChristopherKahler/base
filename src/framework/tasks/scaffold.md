<purpose>
Set up BASE in a new or existing workspace. Scan the workspace, ask guided questions, generate the manifest, install hooks, initialize JSON data surfaces, and run operator profile setup. Optional --full mode adds CLAUDE.md audit and guided first groom.
</purpose>

<user-story>
As an AI builder setting up my workspace, I want a guided scaffolding process that configures workspace management for my specific setup, so that I get maintenance automation without manual configuration.
</user-story>

<when-to-use>
- First-time BASE installation in any workspace
- When user says "base scaffold", "set up base", "initialize workspace management"
- Entry point routes here via /base:scaffold
- Use --full flag for batteries-included mode with CLAUDE.md audit + first groom
</when-to-use>

<context>
@templates/workspace-json.md
</context>

<steps>

<step name="detect_mode" priority="first">
Determine scaffold mode.

1. Check if user specified `--full` or mentioned wanting full setup
2. If `--full`: CLAUDE.md audit + first groom will be offered after data layer setup
3. If standard: data layer + hooks + operator profile
4. Announce mode: "Running BASE scaffold ({standard|full} mode)."
</step>

<step name="scan_workspace">
Scan the workspace and detect what exists.

1. List top-level directories and files
2. Detect common patterns:
   - .base/data/ → existing JSON surfaces (v2 data model)
   - ACTIVE.md, BACKLOG.md → legacy working memory (offer migration)
   - projects/ → project tracking
   - apps/ → satellite projects
   - tools/ → tool management
   - .claude/ → system layer
   - .mcp.json → MCP configuration
   - content/ → content pipeline
   - clients/ → client work
   - obsidian/ → knowledge graph
   - .carl/ → CARL dynamic rules
3. Detect satellite projects (directories with .paul/ inside apps/)
4. Present findings: "I found: {list of detected areas}"

**Wait for confirmation before proceeding.**
</step>

<step name="guided_configuration">
Walk through each detected area and configure tracking.

For each detected area:
1. "I found {area}. Want BASE to track this?"
2. If yes: "What grooming cadence? (weekly/bi-weekly/monthly)"
3. Auto-select audit strategy based on area type
4. Allow override of defaults

Also ask:
- "What day do you prefer for weekly grooming?" (default: Friday)
- "Any directories I should scan for satellite projects?" (default: apps/)
- "Anything else you want tracked that I didn't detect?"

Build workspace.json from responses using `@templates/workspace-json.md` schema.
</step>

<step name="install_data_layer">
Create .base/ directory and generate JSON data surfaces.

1. Create `.base/` directory structure:
   ```
   .base/
   ├── workspace.json
   ├── operator.json
   ├── data/
   │   ├── active.json
   │   ├── backlog.json
   │   ├── projects.json
   │   ├── entities.json
   │   └── state.json
   ├── hooks/
   ├── base-mcp/
   ├── grooming/
   └── audits/
   ```
2. Write workspace.json from guided configuration (with surfaces and carl_hygiene sections)
3. Initialize JSON data surfaces with empty starter content (don't overwrite existing)
4. Copy operator.json template (don't overwrite existing)
5. Register any detected satellite projects in workspace.json
6. Report: "BASE data layer installed. {N} areas tracked, {N} satellites registered, {N} data surfaces initialized."

**Legacy migration:** If ACTIVE.md or BACKLOG.md exist at workspace root, offer to convert them to JSON surfaces using `/base:surface convert`. Do not delete originals — let the user do that after verifying the conversion.
</step>

<step name="install_hooks">
Install and register BASE hooks.

All hooks live in `.base/hooks/`. Session hooks are registered in `.claude/settings.json`.

**UserPromptSubmit hooks** (fire every prompt):
- active-hook.py — active work surface injection
- backlog-hook.py — backlog surface injection
- base-pulse-check.py — drift detection + groom reminders
- psmm-injector.py — per-session meta memory injection
- operator.py — operator identity context injection
- satellite-detection.py — PAUL project auto-registration

**On-demand hooks** (invoked by commands, not auto-registered):
- apex-insights.py — workspace analytics (invoked by /apex:insights)

For each auto-fire hook:
1. Check if `.base/hooks/{hook}` exists
2. If not: copy from `~/.claude/base-framework/hooks/{hook}` (global install source)
   - If `~/.claude/base-framework/hooks/{hook}` doesn't exist either, warn:
     "BASE framework not globally installed. Run `npx base-framework --global` first, then re-run scaffold."
3. Check `.claude/settings.json` for hook registration in `UserPromptSubmit` array
4. If not registered: add the hook entry with absolute path to `.base/hooks/{hook}`

Hook registration format in settings.json:
```json
{
  "hooks": {
    "UserPromptSubmit": [
      { "type": "command", "command": "python3 /absolute/path/.base/hooks/{hook}" }
    ]
  }
}
```

Report: "Hooks installed ({N} auto-fire hooks registered, 1 on-demand hook available)."
</step>

<step name="operator_profile">
Guide the operator through their profile setup.

1. Check if `.base/operator.json` has completed sections (check `completed_at` fields)
2. If all sections completed: "Operator profile already configured. Want to update any section?"
3. If incomplete or new:
   - Walk through each section of operator.json:
     a. **Deep Why** — 5 progressively deeper questions about motivation
     b. **North Star** — One measurable metric with timeframe
     c. **Key Values** — Rank-ordered values with concrete meanings (max 5)
     d. **Elevator Pitch** — Layered pitch (1-4 floors)
     e. **Surface Vision** — Concrete scenes of what success looks like
   - Each section can be skipped: "Skip for now? You can complete it later."
   - Write responses to operator.json after each section
4. Report: "Operator profile {complete|partially complete}. The operator hook will inject your identity context every session."
</step>

<step name="install_mcp">
Verify MCP server is wired.

1. Check `.mcp.json` for base-mcp registration
2. If not registered:
   - Check if `.base/base-mcp/` exists with node_modules
   - If no node_modules: run `npm install` in `.base/base-mcp/`
   - Add registration to `.mcp.json`:
     ```json
     { "base-mcp": { "type": "stdio", "command": "node", "args": ["./.base/base-mcp/index.js"] } }
     ```
3. Report: "BASE MCP server registered. Claude can now manage your data surfaces through tool calls."
</step>

<step name="full_mode_extras">
**Full mode only.**

**CLAUDE.md audit:**
1. Check if CLAUDE.md exists
2. If exists: "Want me to audit your CLAUDE.md against BASE conventions?"
   - If yes: route to `/base:audit-claude-md` task
3. If doesn't exist: "Want me to generate a CLAUDE.md template for your workspace?"
   - If yes: generate based on detected workspace structure

**First groom:**
1. "Want to run an initial groom to establish baseline? This reviews each area once."
2. If yes: run /base:groom flow
3. If no: "Baseline set from filesystem timestamps. First groom due: {date}."
</step>

</steps>

<output>
Fully configured BASE installation. Standard mode: data layer with JSON surfaces, hooks wired, operator profile setup, MCP registered. Full mode: adds CLAUDE.md audit and guided first groom.
</output>

<acceptance-criteria>
- [ ] Workspace scanned and areas detected
- [ ] Operator confirmed tracked areas and cadences
- [ ] .base/ directory created with all required files
- [ ] workspace.json generated from guided configuration
- [ ] JSON data surfaces initialized (active, backlog, projects, entities, state)
- [ ] operator.json created and profile questionnaire offered
- [ ] Satellite projects detected and registered
- [ ] All auto-fire hooks installed and registered in settings.json
- [ ] BASE MCP server wired in .mcp.json
- [ ] (Full mode) CLAUDE.md audit offered
- [ ] (Full mode) First groom offered
- [ ] Operator informed of next groom date
</acceptance-criteria>
