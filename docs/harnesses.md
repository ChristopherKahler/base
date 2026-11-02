# Harness bindings

How base attaches to each agent harness, and what changes for a fork that cannot
assume Claude Code is installed.

Researched against: OpenCode 2.1.4 (local), Cursor hooks docs (2026-10), GitHub
Copilot CLI hooks reference (2026-10). Re-verify the OpenCode and Copilot APIs
before implementing; both are moving.

## Why this is Phase 1.5, not an afterthought

`base install` currently writes `~/.claude/settings.json` and calls that "wired".
Two things follow:

- A user with no Claude Code gets an install that claims success and does
  nothing.
- Cursor reads `~/.claude/settings.json` by default (Settings → Agents →
  Third-Party Imports), so Cursor is *already* half-supported. That is an
  accident of compatibility, not a binding, and it breaks silently when the
  binary is not on PATH — which is the state of this machine today.

The engine is harness-agnostic in the important way: every injection is "read a
JSON event, answer with context". Only the transport differs. So the work is an
adapter layer, not four products.

## Capability matrix

| Capability | Claude Code | OpenCode 2.x | Cursor | Copilot CLI | Antigravity CLI |
|---|---|---|---|---|---|
| Session start | `SessionStart` hook | `ctx.event.subscribe` → `session.created` | `sessionStart` | `sessionStart` | `SessionStart` |
| Prompt submit | `UserPromptSubmit` | `ctx.session.hook("prompt")` | `beforeSubmitPrompt` | `userPromptSubmitted` | `PreInvocation` |
| Pre tool | `PreToolUse` + matcher | `ctx.tool.hook("execute.before")` | `preToolUse` | `preToolUse` | `PreToolUse` |
| Post tool | `PostToolUse` | `ctx.tool.hook("execute.after")` | `postToolUse` | `postToolUse` | `PostToolUse` |
| Stop | `Stop` | `session.idle` | `stop` | `agentStop` | `Stop` |
| Session end | `SessionEnd` | `session.deleted` | `sessionEnd` | `sessionEnd` | `Stop` |
| Context injection | stdout `hookSpecificOutput.additionalContext` | mutate `event.prompt.text`, push `event.system` | `additional_context` / `agent_message` | `{"additionalContext": "..."}` | `injectSteps: [systemMessage/ephemeralMessage]` |
| Blocking | exit 2 / `permissionDecision` | throw in `execute.before` | exit 2 / `permission: deny` | exit 2 / `permissionDecision` | `decision: deny` |
| Config file | `~/.claude/settings.json` | `~/.config/opencode/plugins/base.ts` | `~/.cursor/hooks.json` | `~/.copilot/hooks/base.json` | `~/.gemini/config/plugins/base/hooks.json` |
| Project scope | `.claude/settings.json` | `.opencode/plugins/` | `.cursor/hooks.json` | `.github/hooks/*.json` | `.agents/hooks.json` |
| Custom tools | MCP / skills | `ctx.tool.transform` | MCP | MCP | MCP / skills |
| Skills | `~/.claude/skills` | `ctx.skill.transform` | third-party import | not yet | `~/.gemini/skills` |

## Per-harness detail

### OpenCode (primary — the only one installed here)

OpenCode 2.x plugins are packages with a server part:

```ts
import { Plugin } from "@opencode/plugin"
export default Plugin.define({ id: "base", async setup(ctx) { /* … */ } })
```

The three hooks that matter:

- `ctx.session.hook("prompt", (event) => { event.prompt.text = … })` — fires once
  at admission, before the message is persisted. This is the `UserPromptSubmit`
  equivalent, and it is *stronger*: base can rewrite the prompt rather than
  append to it.
- `ctx.session.hook("context", (event) => { event.system.push({ type: "text", text }) })`
  — fires before every model call, including tool continuations. This is a
  better home for the pre/post-tool "shape of this file" injection than
  `execute.before`, because it survives compaction.
- `ctx.tool.hook("execute.before" | "execute.after", …)` — tool interception.

Also available: `ctx.tool.transform` (register `base ast query`, `base recall`
as native tools), `ctx.command.transform` (the star commands), `ctx.skill.transform`
(base skills), `ctx.event.subscribe` (session lifecycle).

Transport: the plugin `execFile`s `base hook <event> --harness opencode`, writes
the normalized event JSON on stdin, reads a JSON answer, and applies it. Node
`child_process`, not Bun's `$` — the rtk plugin on this machine learned that
lesson (Electron has no `$`).

Constraints to respect:
- The plugin must be a plain object (`Plugin.define`), not a callable: the 2.x
  loader schema-validates it.
- `BASE_HEADLESS=1` must short-circuit the plugin, or base's own
  `opencode run` extraction calls re-enter the hooks.
- Global install path is `~/.config/opencode/plugins/base.ts`; project path is
  `.opencode/plugins/base.ts`.

### Cursor (nearly free, but verify the payload)

Cursor loads Claude Code hooks from `~/.claude/settings.json` when third-party
imports are on (default), maps the event names, maps tool names
(`Bash`→`Shell`, `Edit`→`Write`, `Glob`→unsupported), and accepts Claude's
nested `hookSpecificOutput`. So base's existing wiring already reaches Cursor.

What is missing is honesty and coverage:

- `base install` should detect `~/.cursor` and say "Cursor will pick these up
  through Claude Code compatibility" rather than pretending nothing happened.
- Prefer a native `~/.cursor/hooks.json` entry when Claude Code is absent, so
  the install does not depend on a directory the user does not have.
- Accept both payload shapes: Cursor sends `tool_name`/`tool_input` like Claude
  but with its own tool names. Normalize `Shell`→`Bash`, `Write`→`Edit` before
  the matcher runs.
- `Glob` has no Cursor equivalent; the pre-tool injection for Glob is dead
  there and should be skipped, not errored.

### Copilot CLI (last; no install here to test against)

User-level hooks live in `~/.copilot/hooks/*.json` (honours `COPILOT_HOME`);
repo-level in `.github/hooks/*.json` and must be committed. Shape:

```json
{ "version": 1, "hooks": {
  "sessionStart": [{ "type": "command",
    "bash": "base hook session-start --harness copilot",
    "powershell": "base hook session-start --harness copilot",
    "timeoutSec": 10 }] } }
```

Events: `sessionStart`, `sessionEnd`, `userPromptSubmitted`, `preToolUse`,
`postToolUse`, `errorOccurred`, `agentStop`. Injection is
`{"additionalContext": "..."}` on one line.

Known upstream bugs to design around (all open as of 2026-10): `sessionStart`
`additionalContext` is ignored on some versions (#2142), `sessionStart` fires per
prompt rather than once per session (#2491), `preToolUse` ignores
`updatedInput` (#2013). So: treat Copilot as injection-only, never as a
guarantee, and make every hook fail-open.

### Antigravity CLI (native plugin)

Antigravity CLI (`agy`) discovers customizations from `~/.gemini/config/` and
workspace `.agents/`. Hooks live in plugins (`~/.gemini/config/plugins/base/`):
- `plugin.json`: manifest declaring the plugin name and description.
- `hooks.json`: declares hooks for `SessionStart`, `PreInvocation` (prompt),
  `PreToolUse`, `PostToolUse`, and `Stop`.

Antigravity expects protojson responses:
- `SessionStart`: `{"injectSteps": [{"systemMessage": {"systemMessage": "..."}}]}`
- `PreInvocation`: `{"injectSteps": [{"ephemeralMessage": "..."}]}`
- `PreToolUse`: `{"decision": "allow"}`
- `PostToolUse` / `Stop`: `{}`

Tool names map cleanly: `run_command` -> `Bash`, `read_file` -> `Read`,
`edit_file` -> `Edit`, `write_file` -> `Write`. Argument keys `CommandLine` and
`Path` normalize to `command` and `file_path`.

## The part nobody mentions: the LLM backend

`src/llm.rs` shells out to `claude -p`. Without Claude Code, these are dead:

- `base graph extract` (GraphRAG concept extraction)
- `base graph query` (retrieve + synthesize)
- `base tune` and the corrections judge
- `base rule test` where a rule needs a model

So the fork needs a backend abstraction, not just hook adapters:

| Backend | Command | Notes |
|---|---|---|
| `claude` | `claude -p --output-format text` | current default |
| `opencode` | `opencode run --agent base-extract` | needs a tool-less agent; `run` otherwise edits files |
| `cursor` | `cursor-agent -p` | Cursor CLI |
| `copilot` | `copilot -p` | Copilot CLI |

Resolution: `[llm] backend = "auto" | "claude" | "opencode" | …` in `base.toml`;
`auto` probes PATH in that order. Every backend must receive `BASE_HEADLESS=1`
so its hooks no-op during the call. The `opencode` backend must use a dedicated
agent with all tools disabled — `opencode run` with default settings is a full
coding agent and would be both slow and dangerous for a background extraction.

## Install and uninstall

`base install --harness auto|claude|opencode|cursor|copilot|all`:

1. Probe: `command -v opencode`, `~/.cursor`, `~/.copilot`, `~/.claude`.
2. Wire every detected harness; print exactly what was written.
3. `--harness claude` still works, but Claude Code is no longer required and no
   longer the default assumption.
4. `base doctor` gains a harness section: wired / detected / binary on PATH /
   last hook error.

`base uninstall` must remove only entries base wrote, per harness, and leave the
rest of each file byte-identical (the `settings_json.rs` approach already does
this for Claude; the same contract applies to `hooks.json`).

## Implementation order

1. **Extract, don't rewrite.** `src/harness/mod.rs` with a `Harness` enum and
   `normalize(event, harness) -> HookEvent`. Move the Claude payload parsing
   behind it. No behavior change; the existing 1,771 tests stay green.
2. **OpenCode.** Plugin file, install/uninstall, LLM backend. This is the only
   binding that can be tested end to end on this machine.
3. **Cursor.** Detection, native `hooks.json` when Claude is absent, tool-name
   normalization, Glob skip.
4. **Copilot.** User-level hooks, injection-only, fail-open.
5. **Shared conformance test.** One fixture payload per harness, asserting the
   same normalized event and the same injection text. This is the test that
   keeps four transports honest.

## Test plan

- Fixture per harness under `tests/fixtures/harness/`; the conformance test
  asserts identical normalized output.
- A `base hook <event> --harness <name>` round-trip test per harness: feed the
  fixture on stdin, assert the JSON on stdout.
- Install/uninstall idempotence: install twice, uninstall once, assert the
  config is byte-identical to the original.
- Manual: run each harness with the binary on PATH and a scratch workspace,
  confirm a session start injects and a prompt injects.

## Open questions

- OpenCode `session.idle` fires after every turn, not at session end. Decide
  whether `Stop` maps to it (and throttles) or whether `Stop` is skipped for
  OpenCode.
- Copilot's `sessionStart` is unreliable upstream. Decide whether to wire it at
  all, or rely on `userPromptSubmitted` alone.
- Cursor's Claude-compat path may change. Keep the native path as the tested
  one; treat compat as a bonus.
