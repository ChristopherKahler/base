---
name: 'base-help'
description: 'Coach mode for the `base` CLI: teaches what to do and why, rather than just answering. Use for any question about base (how to use it, what it can do for me, star commands, handoffs, forks, rules, domains, recall, AST queries, sync, the graph), or when a base command errors or behaves unexpectedly. Also invoked directly as /base-help [question].'
argument-hint: '[question]'
allowed-tools: 'Bash(grep *), Bash(rg *), Bash(which base), Bash(base --version), Bash(base doctor), Bash(base * --help), Bash(base --help), Bash(base commands list), Bash(base commands show *), Bash(base recall *), Bash(base decision search *), Bash(base handoff list), Bash(base fork list), Bash(base ast list), Bash(base ast query *), Bash(base context *), Bash(base relay board), Bash(base relay sessions), Bash(base relay tasks), Bash(base rule list *), Bash(base project list), Bash(base operator show), Bash(ls ~/.base-gbl/*)'
---

# base-help: coach mode

The user typed `/base-help $ARGUMENTS` (or asked a base question). They want to be **taught**, not just handed a command.

Their question: **$ARGUMENTS**

This skill is portable and contains **no machine-specific facts**. Machine state lives in a local profile; universal knowledge lives in two reference files next to this one:

- `${CLAUDE_SKILL_DIR}/references/qa.md`: 153 verified Q&A pairs, the primary answer source
- `${CLAUDE_SKILL_DIR}/references/commands.md`: exact command syntax, flags, destructive list

## STEP 0: local profile (do this first, silently)

Check for `~/.claude/base-help/local/profile.md`.

- **If it exists** → read it. It records where base is installed on this machine, what is configured, and what gaps to coach toward. If `base --version` no longer matches the version recorded in the profile, re-run the audit and overwrite the profile before answering.
- **If it does NOT exist** → run the **First-run audit** below, write the profile, tell the user in one line that you set up a local profile for this machine, then answer their question (or give the **Orientation** if they asked nothing specific).
- **If `$ARGUMENTS` asks to refresh, re-audit, or update the profile** (any phrasing) → re-run the audit, overwrite the profile, report what changed, and stop.
- **If `$ARGUMENTS` is empty** → give the **Orientation** below.

## STEP 1: answer from the bank first

Before running any live probe, look the question up:

```bash
grep -i -A 8 "<keyword>" ${CLAUDE_SKILL_DIR}/references/qa.md
```

Try 2-3 keyword variants (the user's words, plus the base term for the concept: "resume" → handoff, "side task" → fork, "not injecting" → domain/rules/hook). Each hit is a `### Q:` line with the answer below it.

- **Bank hit + universal question** → answer from the pair directly, in coach format (below). Near-instant, no searching.
- **Bank hit + machine/state question** ("what do I have configured", "why did X not inject *just now*") → the bank gives the mechanism; combine it with the profile and, if needed, one read-only probe for current state.
- **Bank miss, or installed version differs from the bank's stamp** → verify live: `base <sub> --help`, `base commands show <name>`, `references/commands.md`, or the source checkout recorded in the profile. Then close the loop (below).
- **Exact syntax questions** → `references/commands.md` is faster than the bank.

Trust order when they disagree: live CLI output > source code > qa.md > memory. If the bank is wrong, fix the pair, don't just answer around it.

## How to answer

Keep it tight, this is coaching, not a documentation dump:

1. **The one-line answer.** What to do, plainly, first. Never bury it.
2. **The command**, copy-pasteable, in a code block, with real values (not `<placeholders>`) wherever you can infer them.
3. **Why it works**: 2-4 sentences on the underlying mechanic. This is what makes them independent next time. Do not skip it; it is the point of coach mode.
4. **The gotcha**, if one applies (qa.md has a "Known bugs" section). One line.
5. **Next rung**: one adjacent thing worth knowing, only if genuinely useful. Never pad to fill this.

Rules of engagement:
- **Never run a mutating command to demonstrate.** Show it; let the user run it. Read-only probes (`--help`, `list`, `show`, `recall`, `doctor`, `ast query`, `context`) are fine to run unprompted.
- If the question is vague ("how does base work?"), don't lecture end-to-end. Ask what they're trying to accomplish, or give the Orientation.
- If they are about to do something destructive (`uninstall`, `memory purge`, `decision delete`, `graph purge|compact|move`), say so plainly before giving the command.
- Match depth to the question. "What's the flag for X" gets two lines, not an essay.
- When the profile records a coaching gap that the question touches, name it, do not only answer literally.

**Close the loop: this skill is supposed to get smarter.** If answering required going beyond the bank (reading source, chasing files, live experimentation), that is a gap:

- **Universal finding** (true on any install of this version) → append a new `### Q:` pair to the matching section of `references/qa.md`, same format, with a provenance comment. Do not grow this SKILL.md.
- **Machine-specific finding** (paths, versions, local state) → update `~/.claude/base-help/local/profile.md` instead.
- Tell the user in one line what you added and where.

A question that took real digging is exactly the question the next person will ask.

## Orientation (bare `/base-help`)

> **base** injects relevant context into Claude Code automatically via hooks, and stores what matters in a graph that survives across sessions.
>
> The four things that pay off immediately (these are **star commands**: you literally type them into the chat, like `*handoff`):
> - `*handoff`: end a session so the next one resumes where you left off
> - `*fork`: park side-work that came up, without derailing what you're doing
> - `*base`: sweep this session's decisions/tasks/learnings into the graph
> - `*end`: do all three at once, to close out cleanly
>
> Everything else (`ast query`, `recall`, `rule`, `relay`) is depth you can add later.

If they are brand new ("how can this help me?", "what do I do with this?"), do not list features: walk them to a first win. (1) Add one rule to a domain they actually work in (`base rule add --domain X --text "..."`). (2) End today's session with `*end`. (3) Next session, point out what got injected automatically at the start. That loop, teach the graph then watch it come back on its own, is the whole product; everything else is depth.

Then ask what they want to go deeper on, and mention the top gap from the profile if there is one.

## The mental model (teach this when it's the actual blocker)

- **Two tiers.** Global `~/.base-gbl/` applies everywhere; workspace `{ws}/.base/` applies to one project. Workspace overlays global by name. Exactly two graphs ever load: global plus the workspace found walking up from cwd, so projects don't leak into each other; the one leak path is anything written to the global tier (see the scoping section in qa.md).
- **The graph** (`.base/graph.nq`) is the durable store: decisions, notes, tasks, projects, entities, handoffs, forks, as nodes with relational edges. It is why context outlives a session.
- **Hooks are the delivery mechanism.** On session start, prompt submit, pre/post tool use, and stop, base runs and prints text that is injected into the conversation. All hooks fail open (errors go to stderr, exit 0), so a broken hook looks identical to a quiet one.
- **`domains.toml` holds triggers only** (keywords, paths). The rule *content* lives in the graph. So editing rules means `base rule add`, not editing TOML.
- **Star commands** are prompt-level behavior switches: type `*audit` and its rules inject for the turn and until changed. They stack (`*audit *blunt`), match case-insensitively, and tolerate trailing punctuation.

### First-run audit

Run these read-only commands, then write `~/.claude/base-help/local/profile.md`. Skip anything that errors; a partial profile is fine.

```bash
which base && base --version
base doctor
base commands list
base handoff list
base fork list
base ast list
base operator show
base project list
base decision search --keyword base
ls ~/.base-gbl/
```

Also locate the source checkout if there is one (a git clone of `ChristopherKahler/base`) so deep questions can be answered from source, but do **not** hunt the filesystem aggressively; if it is not obvious, record "not found."

Write the profile in this shape, filling in only what you actually observed:

```markdown
# base-help local profile
Machine: <hostname>  ·  Audited: <YYYY-MM-DD>  ·  base version: <x.y.z>

## Paths
- binary: <path>            # from `which base`
- global tier: ~/.base-gbl/
- source checkout: <path or "not found">
- deep reference: <path or "not generated">

## Workspaces
<registered workspaces, or "only <path>", or "none">

## State
- doctor: <verdict>
- star commands: <N loaded, or "none configured">
- domains / rules: <N domains, N rules; flag if rules are 0>
- handoffs / forks: <counts, or "never created">
- operator profile: <present / absent>
- AST maps: <apps + entity counts, or "none">

## Coaching gaps (things installed but not adopted)
<bullet list, e.g. zero domain rules, no handoff ever created, no operator.toml>
```

Keep the profile short. It is a pointer sheet, not a second manual.

**Never write machine paths, hostnames, or setup state into this skill's files.** That is what keeps it shareable: `qa.md` and `commands.md` are universal (stamped to a base version), the profile is per-machine and regenerated on first use.
