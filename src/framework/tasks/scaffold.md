<purpose>
Set up BASE in a new or existing workspace. Scan the workspace, ask guided questions, generate the manifest, install hooks, and establish baseline state. Optional --full mode adds operational templates (ACTIVE.md, BACKLOG.md, CLAUDE.md audit).
</purpose>

<user-story>
As an AI builder setting up my workspace, I want a guided scaffolding process that configures workspace management for my specific setup, so that I get maintenance automation without manual configuration.
</user-story>

<when-to-use>
- First-time BASE installation in any workspace
- When user says "base scaffold", "set up base", "initialize workspace management"
- Entry point routes here via /base:scaffold
- Use --full flag for batteries-included mode with operational templates
</when-to-use>

<context>
@templates/workspace-json.md
@templates/state-md.md
@templates/active-md.md
@templates/backlog-md.md
</context>

<steps>

<step name="detect_mode" priority="first">
Determine scaffold mode.

1. Check if user specified `--full` or mentioned wanting templates/full setup
2. If `--full`: operational templates will be offered after data layer setup
3. If standard: data layer only (workspace.json, STATE.md, ROADMAP.md)
4. Announce mode: "Running BASE scaffold ({standard|full} mode)."
</step>

<step name="scan_workspace">
Scan the workspace and detect what exists.

1. List top-level directories and files
2. Detect common patterns:
   - ACTIVE.md, BACKLOG.md → working memory (note if already in .base/)
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
Create .base/ directory and generate all state files.

1. Create `.base/` directory structure:
   ```
   .base/
   ├── workspace.json
   ├── STATE.md
   ├── ROADMAP.md
   ├── grooming/
   └── audits/
   ```
2. Write workspace.json from guided configuration
3. Generate initial STATE.md using `@templates/state-md.md` by checking current filesystem timestamps
4. Initialize ROADMAP.md with scaffold entry
5. Register any detected satellite projects in workspace.json
6. Report: "BASE data layer installed. {N} areas tracked, {N} satellites registered."
</step>

<step name="install_operational_templates">
**Full mode only.** Offer operational templates for working memory files.

**ACTIVE.md:**
1. Check if ACTIVE.md exists at workspace root
2. If exists at root (not in .base/):
   - "You have an existing ACTIVE.md. Want me to move it into .base/ and adopt BASE conventions?"
   - If yes: move to `.base/ACTIVE.md`, audit against `@templates/active-md.md`
   - Check for existing references (CLAUDE.md @mentions, hooks, etc.) — if found, offer symlink at root for backward compatibility
   - If no: skip, user keeps their current setup
3. If doesn't exist anywhere:
   - "Want me to create ACTIVE.md from the BASE template?"
   - If yes: generate from `@templates/active-md.md` directly in `.base/ACTIVE.md`
4. Explain the TASKS section and graduation flow

**BACKLOG.md:**
1. Same pattern — check if exists at root
2. If exists: offer to move into .base/ with optional symlink for existing references
3. If creating fresh: generate from `@templates/backlog-md.md` in `.base/BACKLOG.md`
4. Explain time-based rules (review-by, staleness, graduation)

**CLAUDE.md:**
1. Check if CLAUDE.md exists
2. If exists: "Want me to audit your CLAUDE.md against BASE conventions?"
   - If yes: route to `/base:audit-claude-md` task
3. If doesn't exist: "Want me to generate a CLAUDE.md template for your workspace?"
   - If yes: generate based on detected workspace structure

**Symlinks (migration only — not default):**
Symlinks are ONLY offered when scaffold detects existing files at workspace root that have references pointing to them (CLAUDE.md @mentions, hooks reading from root paths, etc.). BASE operates strictly out of `.base/`. Symlinks exist solely to prevent breaking existing references during migration.
- If existing references detected: "I found references to ACTIVE.md at root in your CLAUDE.md. Want a symlink for backward compatibility?"
- If no existing references: no symlinks offered. Files live in `.base/` only.
</step>

<step name="install_hooks">
Install and register BASE session hooks.

**UserPromptSubmit hooks** (fire every prompt):
- base-pulse-check.py — drift detection + groom reminders
- psmm-injector.py — per-session meta memory injection

**SessionStart hooks** (fire once when session begins):
- satellite-detection.py — PAUL project auto-registration

For each hook:
1. Check if `.claude/hooks/{hook}` exists in workspace
2. If not: copy from `~/.claude/base-framework/hooks/{hook}` (global install source)
   - If `~/.claude/base-framework/hooks/{hook}` doesn't exist either, warn:
     "BASE framework not globally installed. Run `npx base-framework --global` first, then re-run scaffold."
3. Check project `.claude/settings.json` for hook registration:
   - base-pulse-check.py and psmm-injector.py → register in `UserPromptSubmit` array
   - satellite-detection.py → register in `SessionStart` array
4. If not registered: add the hook path to the correct event array in settings.json

Report: "Hooks installed (pulse + PSMM on every prompt, satellite detection on session start)."
</step>

<step name="first_groom">
Offer to establish baseline.

1. "Want to run an initial groom to establish baseline? This reviews each area once."
2. If yes: run /base:groom flow
3. If no: "Baseline set from filesystem timestamps. First groom due: {date}."
</step>

</steps>

<output>
Fully configured BASE installation. Standard mode: data layer with workspace.json, STATE.md, hooks. Full mode: adds operational templates (ACTIVE.md, BACKLOG.md with graduation flow, CLAUDE.md audit), symlinks, and guided first groom.
</output>

<acceptance-criteria>
- [ ] Workspace scanned and areas detected
- [ ] Operator confirmed tracked areas and cadences
- [ ] .base/ directory created with all required files
- [ ] workspace.json generated from guided configuration
- [ ] STATE.md reflects current filesystem state
- [ ] Satellite projects detected and registered
- [ ] Pulse hook installed and registered
- [ ] (Full mode) Operational templates offered and created if accepted
- [ ] (Full mode) Symlinks created for .base/ files
- [ ] (Full mode) Graduation flow and time-based rules explained
- [ ] Operator informed of next groom date
</acceptance-criteria>
