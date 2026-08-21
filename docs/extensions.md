# BASE Extensions

Extensions let a framework or skill wire itself into BASE's hook pipeline **without modifying base's core**. You ship one TOML file; base discovers and runs it.

- **Location:** `~/.base-gbl/extensions/{name}.toml`
- **Registration:** none. base scans `~/.base-gbl/extensions/*.toml` on every hook fire. A file that parses is active; delete or rename it to disable.
- **Template:** `~/.base-gbl/extensions/_template.toml` (written by `base install`).
- **Working example:** the Outpost content framework (`outpost.toml`).

```toml
[extension]
name = "my-extension"      # required, unique slug
version = "0.1.0"          # required, semver
description = "what it does"
# framework_dir = "~/.claude/my-framework/"   # optional
# state_dir = ".my-state/"                     # optional, workspace-relative
```

Only `[extension]` is required. Declare only the `[hooks.*]` sections you need.

---

## Hook bindings

| Section | Fires | Use for |
|---|---|---|
| `[hooks.session_start]` | once per session | status injection, state ingest, summary queries |
| `[hooks.user_prompt.domains]` | on prompt (keyword/file match) | domain rules + graph queries (gets dedup/bracketing free) |
| `[hooks.pre_tool.triggers]` | before a tool runs (path match) | inject context before a file is touched |
| `[hooks.post_tool.handlers]` | after a tool runs (file match) | react to writes: reingest, log, or **nudge Claude** |

---

## Post-tool handlers

Each handler **matches a file**, then **runs an action**.

```toml
[[hooks.post_tool.handlers]]
pattern = "registry.json"   # substring of the file path, OR "designset" (see below)
action  = "reingest"
```

### `pattern`
- A **substring** of the written file's path (`"registry.json"`, `".outpost/"`), OR
- the reserved token **`"designset"`** — base's built-in design/frontend file heuristic. Matches across four signal classes:
  - **Extensions** — `css scss sass less styl · html · vue svelte astro · tsx jsx mdx · liquid hbs ejs pug erb twig blade.php razor cshtml · xaml qml · svg · css.ts`
  - **Path segments** — `components/ ui/ styles/ design(-system)/ theme(s)/ tokens/ layouts/ pages/ views/ sections/ atoms·molecules·organisms/ storybook/ icons/ fonts/ …`
  - **Filenames** — `tailwind/postcss/unocss/panda/stitches` configs, `components.json`, `*.module.css`, `*.styles/*.styled`, `theme./tokens./palette./typography.`
  - **Content markers** — CSS-in-JS (`styled.`, `sx=`, `cva(`, `clsx(`), Tailwind (`@apply`, `@tailwind`), inline styles, raw CSS props — catches design hidden inside `.ts`/`.js`.

### `action`
| Action | Effect |
|---|---|
| `reingest` | Re-pull the matched file into the graph. |
| `log` | Debug line to **stderr** (not visible to Claude; diagnostics only). |
| `inject` | Print `message` to **stdout** so **Claude sees it**. The "verify-reflex". |
| `query` | Reserved (SPARQL-on-match) — not yet implemented. |

### The `inject` action (added in base v0.6.0)

A packaged way to **nudge Claude after it writes a file** — "after you do X, verify Y" — with no core changes.

```toml
[[hooks.post_tool.handlers]]
pattern          = "designset"      # or any path substring
action           = "inject"
once_per_session = true             # default true; re-fires only if `message` changes
on_tools         = ["Write", "Edit", "MultiEdit"]   # default; NEVER fires on Read
message          = """
Design work detected. Verify it isn't AI slop before shipping:
→ /design-humanizer scan <file>
"""
```

| Field | Default | Notes |
|---|---|---|
| `message` | — | Printed to stdout (Claude-visible). Required for `inject`. |
| `once_per_session` | `true` | Deduped via session state. Re-fires only if `message` text changes. New session resets it. |
| `on_tools` | `["Write","Edit","MultiEdit"]` | Which tools trigger it. Excludes `Read` by design — you nudge on *production*, not consumption. |

**Why once-per-session matters:** post-tool fires after *every* tool call. Without the dedup you'd nudge on every edit, and the nudge gets ignored. One nudge the first time the trigger is hit, then quiet.

---

## The verify-reflex pattern

`inject` generalizes to any "after you do X, check Y" reflex. Ship a handler per reflex:

```toml
# design → design-humanizer
[[hooks.post_tool.handlers]]
pattern = "designset"
action  = "inject"
message = "Design work detected — verify with /design-humanizer scan."

# copy/markdown → writing humanizer
[[hooks.post_tool.handlers]]
pattern = ".md"
action  = "inject"
message = "Content written — verify against the humanizer + voice.md."

# migrations → reversibility
[[hooks.post_tool.handlers]]
pattern = "migrations/"
action  = "inject"
message = "Migration written — confirm it's reversible (down migration present)."
```

First consumer: the **design-humanizer** skill ships `design-humanizer.toml` with a `designset` + `inject` handler that nudges verification against its 32 AI-slop tells.

---

## Notes for authors
- Hooks **fail open** — a malformed extension logs to stderr and is skipped; Claude is never blocked.
- `inject` writes to **stdout**; `log` writes to **stderr**. Only stdout reaches Claude.
- Keep `message` short and actionable — it's an interruption; make it earn the space.
- To uninstall a reflex, remove the handler (or the whole `.toml`). No deregistration step.

*Built by Chris Kahler · Chris AI Systems · https://www.skool.com/claude-code-titans-9203*
