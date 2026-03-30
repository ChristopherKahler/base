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
   │   ├── projects.json
   │   ├── entities.json
   │   ├── state.json
   │   ├── psmm.json
   │   └── staging.json
   ├── hooks/
   ├── base-mcp/
   ├── grooming/
   ├── schemas/
   └── audits/
   ```
2. Write workspace.json from guided configuration (with surfaces and carl_hygiene sections)
3. Initialize JSON data surfaces with empty starter content (don't overwrite existing):
   - projects.json — unified active work + backlog tracking
   - entities.json — people, organizations, systems
   - state.json — workspace health, drift, groom tracking
   - psmm.json — per-session meta memory
   - staging.json — proposed changes staging
4. Copy operator.json template (don't overwrite existing)
5. Register any detected satellite projects in workspace.json
6. Report: "BASE data layer installed. {N} areas tracked, {N} satellites registered, {N} data surfaces initialized."
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

---

### ENVIRONMENT DETECTION (REQUIRED — do this FIRST)

Hooks are shell commands that Claude Code executes. The python path AND file paths must work in the context where Claude Code is running. Detect the environment before wiring anything.

**Step 1: Identify the platform.**
Run these commands and read the results:
```bash
uname -a          # Linux vs Darwin vs MINGW/MSYS
cat /proc/version 2>/dev/null  # WSL detection (contains "Microsoft" or "WSL")
echo $TERM_PROGRAM  # vscode = VS Code integrated terminal
```

**Step 2: Classify the environment.**

| Environment | Detection | Python Command | File Paths |
|---|---|---|---|
| **Native Linux** | `uname` = Linux, no WSL in /proc/version | `which python3` → use result | Native paths work |
| **Native macOS** | `uname` = Darwin | `which python3` → use result (often /opt/homebrew/bin/python3) | Native paths work |
| **WSL Terminal** (Claude Code CLI in WSL) | Linux + "Microsoft" in /proc/version + NOT in VS Code | `which python3` → use result (typically /usr/bin/python3) | WSL paths work (/home/user/...) |
| **VS Code Extension (WSL Remote)** | Linux + WSL + TERM_PROGRAM=vscode | `which python3` → use result | WSL paths work (VS Code server runs inside WSL) |
| **VS Code Extension (Windows-native)** | platform: win32 in Claude Code, OR `uname` returns MINGW/MSYS | See troubleshooting below | Windows paths required |
| **Native Windows** | No WSL, Windows paths | `where python` or `py -3` | Windows paths (C:\...) |

**Step 3: Handle the tricky cases.**

**VS Code Extension on Windows accessing WSL files (PROBLEMATIC):**
This is the hardest case. The VS Code extension runs on the Windows side but can see WSL files. Hooks execute in a Windows context, so:
- `/usr/bin/python3` does NOT exist
- `/home/user/...` paths are NOT valid
- The Windows Python stub (`WindowsApps/python3.exe`) can't access WSL paths

**Solutions (present to user in order of preference):**

1. **Use VS Code Remote - WSL extension** (RECOMMENDED):
   - Install the "WSL" extension in VS Code (by Microsoft)
   - Open the workspace with "Reopen in WSL" or `code --remote wsl+Ubuntu /path/to/workspace`
   - This runs the VS Code server inside WSL — all hooks fire natively
   - All WSL paths and python work correctly

2. **Use Claude Code CLI in WSL terminal instead of VS Code extension:**
   - Open a WSL terminal, `cd` to workspace, run `claude`
   - All hooks fire natively in WSL context
   - Use VS Code separately for editing if needed

3. **Wrapper script approach** (for advanced users who need both contexts):
   Create a wrapper at a Windows-accessible location that detects context and routes:
   ```bash
   #!/bin/bash
   # Detect if running in WSL or Windows and route accordingly
   if [ -f /proc/version ] && grep -qi microsoft /proc/version; then
     # Running in WSL context — use WSL python directly
     /usr/bin/python3 "$@"
   else
     # Running in Windows context — invoke via wsl
     wsl /usr/bin/python3 "$@"
   fi
   ```
   This is fragile and NOT recommended for most users.

**IMPORTANT: Ask the user which environment they use Claude Code in before proceeding.**
If they use multiple environments (e.g., CLI in WSL + VS Code extension), explain the constraints and recommend option 1 (VS Code Remote WSL).

---

### HOOK REGISTRATION

After environment is classified and python path is determined:

For each auto-fire hook:
1. Check if `.base/hooks/{hook}` exists
2. If not: copy from `~/.claude/base-framework/hooks/{hook}` (global install source)
   - If `~/.claude/base-framework/hooks/{hook}` doesn't exist either, warn:
     "BASE framework not globally installed. Run `npx base-framework --global` first, then re-run scaffold."
3. Check `.claude/settings.json` for hook registration in `UserPromptSubmit` array
4. If not registered: add the hook entry using detected python path + absolute path to `.base/hooks/{hook}`

Hook registration format in settings.json:
```json
{
  "hooks": {
    "UserPromptSubmit": [
      { "type": "command", "command": "{detected_python3_path} {absolute_path_to_workspace}/.base/hooks/{hook}" }
    ]
  }
}
```

---

### HOOK TROUBLESHOOTING

If hooks aren't firing after setup, diagnose with these checks:

**Symptom: "operation blocked by hook" or "No such file"**
- Python path is wrong for the current environment
- Fix: re-detect python path for the environment Claude Code is running in

**Symptom: Zero hooks fire (no CARL, no pulse, no calendar, nothing)**
- Likely a platform mismatch (Windows paths vs WSL paths)
- Check: `echo $PATH | tr ':' '\n' | grep python` — does python3 resolve?
- Check: Can Claude Code's shell access the hook file? Run `cat {hook_path}` to verify

**Symptom: Hooks fire in terminal but not in VS Code (or vice versa)**
- Different Claude Code instances run in different contexts
- VS Code extension (Windows-native) ≠ Claude Code CLI (WSL)
- Fix: Use VS Code Remote WSL extension so both contexts are WSL

**Symptom: "python3: command not found"**
- Python3 is not on PATH in the hook execution context
- Fix: Use absolute path to python3 (detect with `which python3`)

**Diagnostic command (run this to check hook health):**
```bash
# Test each hook manually
for hook in .base/hooks/*.py; do
  echo "--- Testing: $hook ---"
  {detected_python3_path} "$hook" 2>&1 | head -3
  echo "Exit code: $?"
done
```

Report: "Hooks installed ({N} auto-fire hooks registered, 1 on-demand hook available)."
Report environment: "{environment_type} detected — hooks configured for {python_path}"
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
Install and wire the MCP server from the global BASE package.

The MCP server package lives globally at `~/.claude/base-framework/packages/base-mcp/`. Scaffold copies it into the workspace and wires it up.

1. Check if `.base/base-mcp/index.js` exists in the workspace
2. If NOT present:
   a. Check if global source exists at `~/.claude/base-framework/packages/base-mcp/`
   b. If global source missing: warn "BASE framework not globally installed. Run the installer first."
   c. If global source exists: copy the entire `base-mcp/` directory to `.base/base-mcp/`
   d. Run `npm install` in `.base/base-mcp/` to install dependencies
3. If already present: check for `node_modules/`. If missing, run `npm install`.
4. Check `.mcp.json` for base-mcp registration
5. If not registered: add registration to `.mcp.json`:
   ```json
   { "base-mcp": { "type": "stdio", "command": "node", "args": ["./.base/base-mcp/index.js"] } }
   ```
6. Report: "BASE MCP server installed from global package and registered. Claude can now manage your data surfaces through tool calls."
</step>

<step name="full_mode_extras">
**Full mode only.**

**CLAUDE.md audit:**
1. Check if CLAUDE.md exists
2. If exists: "Want me to audit your CLAUDE.md against the CLAUDE.md Strategy?"
   - If yes: route to `/base:audit-claude-md` (interactive, strategy-driven audit with CARL detection)
3. If doesn't exist: "Want me to generate a CLAUDE.md from the strategy template?"
   - If yes: use `@~/.claude/base-framework/templates/claudemd-template.md` as starting point, fill from detected workspace structure

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
- [ ] JSON data surfaces initialized (projects, entities, state, psmm, staging)
- [ ] operator.json created and profile questionnaire offered
- [ ] Satellite projects detected and registered
- [ ] All auto-fire hooks installed and registered in settings.json
- [ ] BASE MCP server wired in .mcp.json
- [ ] (Full mode) CLAUDE.md audit offered
- [ ] (Full mode) First groom offered
- [ ] Operator informed of next groom date
</acceptance-criteria>
