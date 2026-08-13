# base CLI: Merged Q&A Knowledge Bank

**What this file is.** A single deduplicated question-and-answer bank covering the `base` CLI: orientation, star commands, handoffs and forks, the knowledge graph, rules and domains, project scoping, ingestion, GraphRAG, AST code navigation, hooks, session relay, admin surfaces, known bugs, destructive operations, and on-disk storage. It merges four separately-researched Q&A files into one lookup surface.

**Verified against base v0.11.0 on 2026-08-13.** Command syntax was spot-checked against live `--help` across every top-level subcommand, and mechanism claims (hook pipeline, star-command matching, bracket derivation, suppression exemptions, AST path handling, the commands-import corruption) against the source at tag v0.11.0. Pairs still tagged `verified: reference` alone carry the weakest provenance: treat those as leads to re-check rather than settled facts.

**How to look something up.** Grep this file case-insensitively for keywords from the user's question (`grep -i "<keyword>" qa.md`). Each hit is a `### Q:` line; read the pair below it (the `**A:**` block plus its `<!-- ... -->` provenance comment).

**Maintenance rule.** Append new verified pairs to the matching `## ` section rather than creating new sections or a second file. If the installed base version differs from the stamp above, treat every answer here as a lead to re-verify, not a fact.

---

## Getting started

### Q: What is base?
**A:** base is a proactive context-injection engine that plugs into Claude Code's hook pipeline. It keeps a knowledge graph of your projects, decisions, tasks, and rules, and at key moments (session start, every prompt, before/after tool calls) it queries that graph and pushes the relevant slice straight into Claude's context. You never ask for it: the hooks push it, then go quiet again.
<!-- v0.11.0 | verified: reference -->

### Q: How is base different from just writing a CLAUDE.md file?
**A:** CLAUDE.md is static: written once, read every time, never adapts to what you're doing right now. base is dynamic: it matches domains, files, and keywords against a live graph and injects only what's relevant to the current prompt or file, then suppresses itself once you've seen it (unless it's a handoff, reminder, or fork, which always resurface). Think of CLAUDE.md as a constitution and base as an assistant that reads the room.
<!-- v0.11.0 | verified: reference -->

### Q: What actually happens automatically vs. what do I have to type myself?
**A:** Automatic: session-start injection (health check, operator profile, update banner, handoffs/reminders/forks that are due, domain sync), per-prompt domain matching based on keywords/files/paths, AST context on file reads, and a debounced AST refresh after you edit code. What you type yourself: any star command (`*handoff`, `*audit`, etc.), any `base <subcommand>` you run directly (`base task add`, `base decision log`, `base recall`), and anything you want logged into the graph that isn't already inferred from files or prompts.
<!-- v0.11.0 | verified: reference -->

### Q: I just installed base. What should I do first?
**A:** Three things: (1) let it register a project for what you're working on, or just start working and let `*base` catch it later; (2) add any standing behavioral rules with `base rule add --domain <name> --text "..."` so they inject automatically when that domain matches; (3) at the end of your session, type `*end` (or `*close`) so a handoff gets written and registered. Next session, that handoff resurfaces automatically at session-start and tells you exactly where you left off. That loop, work then `*end`, then resume, is the core habit.
<!-- v0.11.0 | verified: reference -->

### Q: How do I know base is actually working / injecting anything?
**A:** Run `base doctor` for a graph health report (exits nonzero if something's wrong). Day to day, look for the bracketed blocks base prints at session start and in hook output, things like an operator profile block, an update banner, or a `[*AUDIT ACTIVATED]` block after you type a star command. If you never see any of that, check that hooks are wired (an `install` problem) rather than assuming the graph is empty.
<!-- v0.11.0 | verified: cli-help -->

### Q: What's the single most important habit to build with base?
**A:** End every real working session with `*end` (or `*close`). It sweeps any un-registered side-work into forks, syncs decisions/tasks/learnings into the graph, runs a health check, and writes plus registers a handoff titled with your session's codename. That handoff is what resurfaces automatically next time and lets you say "resume the X work" instead of re-explaining everything from scratch.
<!-- v0.11.0 | verified: reference -->

### Q: Does a broken graph ever block my session?
**A:** No. All base hooks fail open: any internal error prints to stderr and exits 0 with empty stdout, so Claude Code just proceeds without the extra context rather than getting stuck. A damaged graph degrades gracefully instead of blocking you, though you should still run `base doctor` to catch and repair it.
<!-- v0.11.0 | verified: reference -->

### Q: Who makes base, and where do I get support or tutorials?
**A:** base is built by Chris Kahler of Chris AI Systems. Community and support live at `https://chrisai.cv/skool`, and tutorials are on YouTube at `@chris-ai-systems`. This is printed at the bottom of `base --help`.
<!-- v0.11.0 | verified: cli-help -->

---

## Star commands

### Q: What are "star commands"?
**A:** Star commands are short `*word` triggers you type inline in a normal prompt (not a separate CLI invocation) that activate a named mode or trigger a scripted flow, things like `*audit`, `*blunt`, or `*handoff`. base scans your prompt for any token starting with `*`, matches it against configured commands, and injects that command's rules directly into context for that turn.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I actually type a star command?
**A:** Just include it as a word in your normal prompt, anywhere: `*audit review this migration plan` or `review this migration plan *audit` both work. No leading slash, no separate command line. Capitalization doesn't matter and trailing punctuation is stripped, so `*Audit,` and `*AUDIT.` both match `*audit`.
<!-- v0.11.0 | verified: source -->

### Q: Are star commands case-sensitive?
**A:** No. Matching is case-insensitive (`*Blunt`, `*BLUNT`, and `*blunt` are the same command), and all trailing non-alphanumeric characters are stripped before matching, so punctuation right after the word doesn't break it.
<!-- v0.11.0 | verified: source -->

### Q: Can I stack multiple star commands in one prompt?
**A:** Yes, that's the point. `*audit *steelman review this` activates both modes, injected in the order they were first seen, deduplicated if you repeat one. This is how the composite commands are meant to be combined with single-purpose ones on the fly.
<!-- v0.11.0 | verified: source -->

### Q: If I type a star command, do my domain rules still fire that prompt?
**A:** No, and this is the single most important gotcha to know. A matched star command short-circuits domain matching entirely for that prompt: bracket rules and the command's own rules print, then base returns, no domain rules, no graph neighborhood, no auto-sync happen that turn. If you always work in `*blunt` mode, any domain rule you configured simply never injects unless that domain's own `commands` list happens to include it, and even that path is skipped once an explicit star is typed. Bracket rules are the exception: they are built before the star-command check, so a star command cannot bypass them.
<!-- v0.11.0 | verified: source -->

### Q: How many star commands are available, and how do I see them all?
**A:** Run `base commands list` for the full table (name, description, rule count), plus the total count at the bottom (e.g. "31 command(s) available. Type *NAME in a prompt to activate."). On a fully-loaded setup there are 31: 14 single-purpose behavior modes (like `*audit`, `*blunt`, `*mentor`), 8 composite "combo lens" commands built by stacking modes (like `*vet` = audit + operator), and 9 session commands that drive base itself (`*handoff`, `*fork`, `*base`, `*docs`, `*close`, `*end`, `*task`, `*ping`, `*inbox`).
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I see the full detail/rules behind one star command?
**A:** `base commands show <name>` (bare name, no asterisk, no quotes needed), e.g. `base commands show audit` or `base commands show handoff`. It prints the full description and every numbered rule exactly as it would be injected when you type that star command, which is useful for understanding a multi-step flow (like `*handoff`'s registration process) before triggering it. There is no `commands show *audit` form, drop the asterisk.
<!-- v0.11.0 | verified: cli-help -->

### Q: What are the single-purpose behavior mode star commands?
**A:** `*discuss` (explore before acting), `*meta` (work ON the thing, not IN it), `*brief` (generate a session report), `*dev` (development mode), `*blunt` (terse, answer-first), `*analytical` (structured, evidence-cited), `*steelman` (strongest argument, then counter), `*audit` (skeptical, find problems first), `*mentor` (teach-first, patient), `*operator` (business/ROI framing), `*editor` (rewrite and tighten only), `*debug` (systematic hypothesis-test loop), `*plainspeak` (no jargon), and `*hold` (defend a position, move only on evidence).
<!-- v0.11.0 | verified: cli-help -->

### Q: What are the composite "combo lens" star commands?
**A:** `*vet` (audit + operator: what breaks and what it costs), `*counsel` (steelman + operator + blunt), `*weigh` (analytical + operator: a trade-off table), `*teach` (mentor + plainspeak), `*retro` (meta + audit: review a build), `*bottomline` (plainspeak + operator: the plain "so what"), `*dissect` (analytical + audit: forensic severity-tagged review), and `*strategy` (discuss + steelman + operator).
<!-- v0.11.0 | verified: cli-help -->

### Q: What do the "combo lens" star commands actually do under the hood?
**A:** They're documentation-only composites. The `CommandDef` struct that drives star commands only has three fields: name, description, and rules, there's no real "composes" or "execution_type" engine feature. Commands like `*vet` (audit + operator) work purely because their rules text tells Claude, in plain language, to run `base commands show audit` and `base commands show operator` and follow both. It's convention, not code enforcement.
<!-- v0.11.0 | verified: source -->

### Q: How does *audit behave, specifically, and how is it different from a normal code-review ask?
**A:** It flips to a skeptical default: assume something is wrong, name every failure mode (tagged critical/moderate/minor) before acknowledging what works, and state problems directly rather than hedging ("might want to consider"). Crucially, findings are designed to survive pushback: a disputed finding is only withdrawn against actual evidence (a code path, a config value, a test result), never just because you disagree. Disagreement alone gets marked DEFERRED and the finding stays on the list, at most downgraded in severity. That is what stops an audit from quietly shrinking under objection.
<!-- v0.11.0 | verified: cli-help -->

### Q: How does *blunt behave, specifically?
**A:** Answer-first, no hedging, no warm-up, no "let me know if you need anything" closers. One sentence beats a paragraph. It does still preserve uncertainty markers though (like a `[N/5]` confidence tag), it strips filler, not the signal that something is genuinely uncertain.
<!-- v0.11.0 | verified: cli-help -->

### Q: What does *base do?
**A:** It's the safety net that sweeps the *current* session into the graph: decisions, tasks/projects changed, milestones reached, insights/corrections, new behavioral rules, and notable entities. It deliberately dedups first (searching the graph before writing) so re-running it never duplicates what's already logged, then reports a one-line tally like "+3 decisions, +2 tasks, 1 done" plus an explicit "Unrouted:" line for anything it wasn't sure where to file.
<!-- v0.11.0 | verified: cli-help -->

### Q: What does *docs do?
**A:** It brings user-facing documentation (READMEs, running/planning docs, specs, CHANGELOGs) back in sync with what the session actually changed, but it's relevance-gated: it only touches a doc if the session's work made something that doc asserts stale or wrong. If nothing in the session invalidated any doc, the correct, expected outcome is "no doc updates warranted," not a forced edit to look productive.
<!-- v0.11.0 | verified: cli-help -->

### Q: What does *close do, and how is it different from *end?
**A:** `*close` chains three phases in order: `*base` (graph sync), then `*docs` (relevance-gated doc updates), then `*handoff` (write and register the resume doc), then tells you to run `/clear`. `*end` is a leaner one-shot variant that instead chains: a fork sweep (catch any scoped build-work discussed but never formally forked), then `*base` plus a `base doctor` drift check, then `*handoff` (which folds in what the sweep and sync found into a "Session close-out" section). Roughly: use `*close` when docs need grooming too; use `*end` when you mainly want the graph synced, loose forks captured, and a clean handoff.
<!-- v0.11.0 | verified: cli-help -->

### Q: What does *fork sweep mean inside *end?
**A:** During `*end`'s first phase, Claude re-reads the session for any scoped build-work you agreed to but never actually ran `*fork` on (features discussed, specs agreed, "we should build X" moments), compares that against `base fork list`, and registers a fork for anything missing. Zero misses is a completely valid, expected outcome ("fork sweep clean"), it's not trying to invent work.
<!-- v0.11.0 | verified: cli-help -->

### Q: After I run *end or *close, what should I actually do?
**A:** Run `/clear` yourself, base deliberately never runs it for you (it's a user-only command). The handoff that `*end`/`*close` just registered is the resume mechanism: once you `/clear` and start a fresh session, that handoff resurfaces automatically at session-start with everything you need to pick back up.
<!-- v0.11.0 | verified: cli-help -->

### Q: Can I add my own star command?
**A:** Yes: `base commands add --name <NAME> --description <DESCRIPTION> --rule <RULE>` (repeat `--rule` for each rule line). This writes into `commands.toml`. To remove one, `base commands remove <name>`.
<!-- v0.11.0 | verified: cli-help -->

### Q: Can I import a whole pack of star commands from a file?
**A:** A `base commands import <FILE>` command exists (append-only, skips names already present), but do not use it: its TOML writer corrupts `commands.toml` for any rule text containing newlines, tabs, or backslashes, and the loader swallows the resulting parse error silently. See the known-bugs entry on `base commands import` for the mechanism and the reliable workaround (`cp` a validated file directly to `~/.base-gbl/commands.toml`, then confirm with `base commands list`).
<!-- v0.11.0 | verified: audit -->

### Q: Where do global vs. workspace star commands live, and what wins if they conflict?
**A:** Global commands live in `~/.base-gbl/commands.toml` and load first; a workspace's `.base/commands.toml` overlays them by case-insensitive name (a workspace command of the same name replaces the global one; new workspace-only names just get appended).
<!-- v0.11.0 | verified: reference -->

### Q: How do I remove a star command I don't want anymore?
**A:** `base commands remove <name>`.
<!-- v0.11.0 | verified: cli-help -->

---

## Handoffs and forks

### Q: What's the difference between a handoff and a fork?
**A:** A handoff is your one continuity thread per project, it's what "resume this project" means. Creating a new handoff for a project **archives the previous one** automatically, so there's only ever one open handoff per project. A fork is a build-spec for parallel side-work, and forks are additive: creating one never touches the project's handoff or any sibling forks. Use handoff for "this is where the main thread left off," use fork for "here's a separate thing to build later."
<!-- v0.11.0 | verified: cli-help -->

### Q: What's the trap people fall into with handoff vs. fork?
**A:** Using `*handoff` for side-work. Because `handoff create` silently archives the project's prior open handoff, if you run it a second time to capture some side-quest instead of your main continuity doc, you just archived your real "resume here" thread and replaced it with the side-quest. If you meant to keep both, the second one should have been a `*fork`, not a second handoff.
<!-- v0.11.0 | verified: reference -->

### Q: How do I create a handoff?
**A:** In conversation, type `*handoff` and let Claude run the flow: it resolves your session's codename, writes a handoff doc to `{workspace}/.base/handoffs/{YYYY-MM-DD-HHMM}-{codename}-{project-slug}.md` (or the global fallback outside a workspace), then registers it with:
```
base handoff create --project "<project>" --doc "<absolute-doc-path>"
```
`--slug` is optional and defaults to the doc's basename, that's the doc==slug protocol. Registering is what makes it resurface; writing the file alone does nothing.
<!-- v0.11.0 | verified: cli-help -->

### Q: I wrote a handoff doc but nothing showed up next session. Why?
**A:** The doc file itself is inert, only the graph node created by `base handoff create` gets scanned at session start. This is a real trap: some flows (like a PAUL handoff) print their own "HANDOFF CREATED" confirmation box that looks like the end of the job, but producing the doc is only half of it. Always verify with `base handoff list` and confirm it shows exactly one open handoff pointing at your doc path before assuming you're covered.
<!-- v0.11.0 | verified: reference -->

### Q: How do I create a fork?
**A:** Type `*fork` (naming the feature or features to fork off). For each one, Claude writes a forward build-spec (what to build, not what was done) to `{workspace}/.base/forks/{slug}.md`, then registers it:
```
base fork create --project "<project>" --doc "<absolute-doc-path>"
```
Forks are additive, registering a new one never archives existing forks or the project's handoff. Re-running `fork create` on the same slug is idempotent, it just re-points that fork at the (possibly updated) doc.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I see my open handoffs or forks?
**A:** `base handoff list` and `base fork list`, both list across the global and workspace tiers and show which tier each entry lives in. There is no `handoff show` or `fork show` command, to read a doc's actual content, `cat` the file path that `list` gives you.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I dismiss a handoff or fork I don't need anymore?
**A:** Two options: `base handoff snooze <slug> <days>` (or `base fork snooze <slug> <days>`) hides it until that many days pass; `base handoff archive <slug>` (or `base fork archive <slug>`) stops it resurfacing for good. Note snooze takes positional arguments, not flags, e.g. `base handoff snooze my-project-handoff 3`, not `--days 3`.
<!-- v0.11.0 | verified: cli-help -->

### Q: What is the "doc==slug protocol"?
**A:** By convention across handoff, fork, and task-relay docs, the markdown filename (minus extension) and the graph slug/title you use to summon it should match exactly. `handoff create` and `fork create` both default `--slug` to the doc's basename specifically to enforce this, so a filename like `2026-08-13-1400-otter-my-project.md` naturally becomes the slug `2026-08-13-1400-otter-my-project`. Keeping filename and slug identical means you can always find the right doc just from the slug shown in `list`, without a separate lookup.
<!-- v0.11.0 | verified: audit -->

### Q: What is the "session codename" and why does it matter for handoffs?
**A:** Each live Claude Code session can register a friendly title (an animal codename, like "otter" or "jackal") via `base relay register --as <codename>`, checked against `$CLAUDE_CODE_SESSION_ID` in `base relay sessions`. The `*handoff` flow requires this codename in both the handoff's filename and its registered title, because it's how you (or another session) can tell which authoring session a resurfaced handoff came from, especially useful once you're running more than one session in parallel.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I resume work from a previous session?
**A:** You mostly don't have to do anything: an open handoff resurfaces automatically in the injected context at the start of your next session, and it keeps resurfacing every session until you pick it up, snooze it, or archive it. If you have several open handoffs or forks across projects, just tell Claude "resume the X work" and it'll go find and read the matching doc from `base handoff list` / `base fork list`.
<!-- v0.11.0 | verified: reference -->

### Q: Does a handoff go away once I've seen it once?
**A:** No. Handoffs (along with reminders and forks) are explicitly exempt from base's usual suppression logic, they surface every single session until you take an action on them (pick up the work and it naturally gets archived/replaced, or explicitly `snooze`/`archive` them). Most other injected content gets suppressed once unchanged, these three categories don't.
<!-- v0.11.0 | verified: reference -->

---

## Graph and memory (learn/recall/decision)

### Q: What is the base knowledge graph, actually?
**A:** It is a plain-text N-Quads (RDF) file called `graph.nq`, read and written through an embedded oxigraph store and queried with SPARQL. It is not SQLite and not JSON. Every fact is a quad: subject, predicate, object, and a named graph that stamps which tier or workspace owns it. Node types include Domain, Rule, Project, Milestone, Task, Decision, Note, Entity, Goal, Reminder, and Handoff (forks are just Handoff nodes with `kind = "fork"`, not a separate type).
<!-- v0.11.0 | verified: reference -->

### Q: Where does the graph physically live on disk?
**A:** Two files: the global graph at `~/.base-gbl/.base/graph.nq` and one workspace graph at `{workspace}/.base/graph.nq` for each registered workspace. Never hand-edit either file directly, use the `base graph` subcommands or `base doctor` instead.
<!-- v0.11.0 | verified: reference -->

### Q: Can I hand-edit graph.nq to fix something?
**A:** No, never. base's own documentation states every graph corruption incident traced back to an interrupted hand-run edit. All of base's own writes are atomic (write to temp file, validate, then rename) specifically so a graph can't be left half-written, and they refuse to run against an already-unhealthy graph. Reads use a lenient parser so one bad line doesn't blank out your whole context. If something looks wrong, use `base doctor` (add `--repair` to self-heal, or `--restore` to roll back to a snapshot) or the `base graph` maintenance commands, never a hand edit.
<!-- v0.11.0 | verified: reference -->

### Q: How do I record a decision I just made?
**A:** `base decision log --domain <D> --decision "<what was decided>" --rationale "<why>"`. Both `--decision` and `--rationale` are required. There's also an optional `--recall` flag. Use this for durable, deliberate choices (architecture, process, tooling) where the "why" matters for future reference.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I search past decisions?
**A:** `base decision search --keyword "<term>"`, add `--json` for machine-readable output. This searches decisions specifically (with their rationale), as opposed to general notes.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I record an insight, correction, or general learning that isn't a formal decision?
**A:** `base learn --text "<the insight>" --domain <D> --type insight` (type defaults to insight if omitted; other values are correction, decision, commitment, shift). Optionally link it to a project with `--project` or a person/org with `--entity`. Use `base learn --list` to see existing notes and their slugs, `--mention <SLUG> --context "<text>"` to log a re-encounter with an existing note, `--remove <SLUG>` to delete one, and `--update <SLUG> --text "<new text>"` to edit one in place.
<!-- v0.11.0 | verified: cli-help -->

### Q: What's the difference between `base learn`, `base decision log`, and `base rule add`, and when do I use each?
**A:** `base decision log` is for a specific choice made with a rationale (architecture pick, tool selection). `base learn` is for general structured memory: insights, corrections, commitments, or shifts in understanding, it's the broadest bucket. `base rule add` is specifically for a standing behavioral instruction you want injected automatically whenever a domain matches in future sessions, it's prescriptive ("always do X"), not a record of something that happened. If you want future Claude sessions to actively follow a rule when working in a given area, use `rule add`; if you're just recording what happened or what you learned, use `learn` or `decision log`.
<!-- v0.11.0 | verified: reference -->

### Q: What's the difference between `base recall`, `base decision search`, and `base graph query`, and which should I use?
**A:** `base recall --keyword <K>` (or `--domain`, `--slug`) does a keyword search over general notes (the ones written by `base learn`), it's fast and literal. `base decision search --keyword <K>` is the same idea but scoped specifically to logged decisions and their rationale. `base graph query "<question>"` is GraphRAG: it retrieves a relevant subgraph and synthesizes a cited natural-language answer to an open-ended question, use it when you want a real answer to "why did we choose X" rather than a list of matching notes. Reach for `recall`/`decision search` when you know roughly what you're looking for and want raw hits; reach for `graph query` when you want synthesis across multiple related facts.
<!-- v0.11.0 | verified: reference -->

### Q: Does recalling a note have any side effect?
**A:** Yes. `base recall` stamps the note's `lastRead` timestamp even though it's a read/search command, which resets that note's purge clock. This means recalling a note protects it from `base graph purge --stale`, reading is not neutral here.
<!-- v0.11.0 | verified: reference -->

### Q: How do I track a person or organization in base?
**A:** `base entity add --name "<name>" --domain <D> [--entity-type person|organization] [--project <P>]`. `--domain` is required, deliberately, to prevent orphan entities with no context anchor. List with `base entity list`, look one up with `base entity get <slug-or-name>`, edit with `base entity update`.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I set a goal, and how do I set a reminder that surfaces later?
**A:** Goal: `base goal add --name "<name>" --target "<target>"`, list with `base goal list`, edit with `base goal update <slug-or-name>` (no delete subcommand documented). Reminder: `base reminder add --name "<name>"` plus exactly one of `--due YYYY-MM-DD` (surfaces on or after that date), `--at <ISO-8601 timestamp>` (exact surface time), or `--in <duration>` (relative, e.g. `30s`, `3m`, `2h`, `1d`). List with `base reminder list`, hard-delete with `base reminder remove`. Reminders are exempt from suppression and the injection character budget, so a due reminder surfaces every session until removed.
<!-- v0.11.0 | verified: cli-help -->

### Q: What does `base memory list` show me?
**A:** It lists Claude Code's flat-file memories (the kind stored as loose files rather than in base's graph) for review: name, type, description, and path for each. This is a read-only inspection step, meant to run before purging.
<!-- v0.11.0 | verified: cli-help -->

### Q: What exactly does `base memory purge` delete, and is it safe?
**A:** It removes flat-file memories that have already been confirmed present in the graph, i.e. ones that were successfully migrated and are now redundant on disk. There is no preview flag for this command, so run `base memory list` first to see what will be affected before purging. Treat it as a one-way operation.
<!-- v0.11.0 | verified: cli-help|reference -->

---

## Rules and domains

### Q: What are "domains" in base?
**A:** A domain is a named context bucket configured in `domains.toml` (keywords, file patterns, and paths that "trigger" it) whose actual rule content lives in the graph, not in the toml file. When a prompt, edited file, or active path matches a domain's triggers, its graph rules (and optionally a SPARQL query result) get injected into that prompt. `base domain list` shows configured domains, `base domain get <NAME>` shows one domain's full configuration.
<!-- v0.11.0 | verified: reference -->

### Q: How do I add a rule to a domain?
**A:** `base rule add --domain <D> --text "<the rule>"`, optionally with `--rationale "<why>"` which gets injected alongside the rule as "rule, because rationale". List existing rules with `base rule list --domain <D>`, remove one with `base rule remove` by index. Rules live in the graph, not in `domains.toml`, that file only holds the triggers that decide when a domain's rules get injected.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I create a new domain and give it triggers?
**A:** `base domain create --name <NAME> --keyword <K> --path <P>` creates it in `domains.toml`. There is no `add-trigger` or `remove-trigger` subcommand: `base domain` has exactly `list`, `get`, `sync`, `create`, and `remove`. To add or change triggers on an existing domain, edit that domain's block in `domains.toml` by hand (the trigger keys are `prompt_keywords`, `file_keywords`, and `paths`), then run `base domain sync` to push triggers and any bootstrap rule content into the graph (this also runs automatically via a timestamp-gated marker on session start). Confirm the subcommand set with `base domain --help`.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do triggers actually decide whether a domain fires?
**A:** Deterministically: a domain in `domains.toml` declares `prompt_keywords`, `file_keywords`, and/or `paths`; a domain fires if your prompt contains a keyword, a touched file contains a keyword, or a recently-active file (pulled from the graph) matches a configured path. Matching is plain substring/path comparison, not semantic or fuzzy.
<!-- v0.11.0 | verified: source -->

### Q: I set up a domain with keywords but nothing gets injected when it matches. Why?
**A:** Almost certainly the domain has zero rules in the graph. `domains.toml` only declares triggers (keywords and paths); if no one has run `base rule add --domain <D> --text "..."` for that domain, a match fires but there's nothing to inject, silently. Check with `base rule list --domain <D>`. Other causes: you typed a `*star command` in the same prompt (star commands short-circuit all domain matching for that prompt), or the same output already fired this session and got deduped.
<!-- v0.11.0 | verified: audit -->

### Q: My prompt should have triggered a domain but nothing was injected. What's the full troubleshooting ladder?
**A:** Work through it in order, ruling out the star-command short-circuit first since it's the most common cause: (0) you typed any `*command` in that prompt, which short-circuits all domain matching for that turn; (1) the domain matched but has zero rules configured (`rules = []` and nothing logged to the graph), so add rules with `base rule add`; (2) the hook itself is broken, so run it manually, e.g. `echo '{"prompt":"..."}' | base hook user-prompt-submit`, and read stderr; (3) you're outside a registered workspace, so there's no workspace graph to match against, so `cd` into a registered workspace or run `base scaffold`.
<!-- v0.11.0 | verified: source|reference -->

### Q: How do I debug why a domain isn't firing the way I expect?
**A:** Turn on devmode: `base config set devmode.enabled true`. With it on, hook output prints a diagnostic block on each response showing which domains fired, why, and what got deduped or suppressed. This is the fastest way to see whether trigger matching and rule injection are actually working while you're tuning a domain. Turn it back off once tuning is done, since it adds noise to every hook event. Off by default.
<!-- v0.11.0 | verified: source|reference -->

### Q: How do I add a rule to the global tier instead of the workspace?
**A:** Pass `-g` (or `--global`) to `base rule` itself, before the subcommand: `base rule -g add --domain X --text "..."` and `base rule -g list --domain X`. Putting `-g` after `add` is invalid, it belongs on the `rule` command, not the subcommand. Note that `rule` is the ONLY command carrying a `-g/--global` flag in v0.11.0: `base entity`, `base learn`, `base decision`, and `base domain` have no such flag, so their tier is decided by where you run them (inside a registered workspace, or falling back to global outside one). Check with `base <sub> --help` before assuming a `-g` exists.
<!-- v0.11.0 | verified: cli-help|source -->

---

## Project scoping and tiers

### Q: What's the difference between the global tier and the workspace tier, and how do they merge?
**A:** GLOBAL (`~/.base-gbl/`) loads in every workspace, every project, everywhere. WORKSPACE (`{workspace}/.base/`) is scoped to one registered workspace. At hook time, global loads first and the workspace graph overlays it by name into one merged in-memory store, so a single query spans both tiers at once and rules from both can appear in the same injection (workspace wins on conflicts). Config files (`base.toml`, `domains.toml`, `commands.toml`) follow the same overlay pattern: workspace values override global by matching name, and workspace `base.toml` inherits everything from global unless explicitly overridden. Use global for things true everywhere (your general coding preferences); use workspace for things true only in that project family.
<!-- v0.11.0 | verified: reference -->

### Q: Will base surface things from a different project I'm working on? I don't want cross-contamination.
**A:** No, by design. Exactly two graphs load into any session: the global tier (`~/.base-gbl/.base/graph.nq`, always loaded) and the one workspace graph found by walking up from your current directory to find a registered `.base/` folder. Project B's graph never loads while you're working in project A's directory, isolation is by path resolution, not by filtering after the fact. The one leak path is the global tier itself: anything written while outside a registered workspace (or certain handoff/fork operations run outside one) falls back to the global tier and will then surface in every workspace. The fix is to run `base scaffold <path>` on every real project before working in it, so writes land in that project's own tier instead of leaking to global.
<!-- v0.11.0 | verified: audit -->

### Q: Do I need a ".base/" workspace to use base at all?
**A:** No, base still works outside a registered workspace, it just falls back to the global tier (`~/.base-gbl/`). Some workspace-scoped commands will error with "no .base/ directory found. Use --global for global rules, or run `base scaffold`" if you're not inside one. That's expected behavior, not a bug, when you're in an unregistered folder. Be aware, though, that other commands fall back silently rather than erroring, see the known-bugs entry on empty results outside a workspace.
<!-- v0.11.0 | verified: reference -->

### Q: How do I see what workspaces are registered, and how do I register a new project folder?
**A:** `base workspace sync` regenerates the registered-workspaces block in `~/.claude/CLAUDE.md` from `base.toml`, that file is the source of truth for what's registered; there's no separate `base workspace list`. To register a new one, `base scaffold [PATH]` (defaults to the current directory) creates the `.base/` folder, writes the initial `base.toml` and `domains.toml`, and registers the workspace globally. Do this for every real project before working in it, both to get base's context injection and to avoid the global-tier leak described above.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I clean up something that keeps resurfacing from the wrong place?
**A:** Run `base handoff list` or `base fork list`, both list across global and workspace tiers together and show which tier each entry lives in. If something is surfacing that shouldn't be, snooze it (`base handoff snooze <slug> <days>`) or permanently stop it with `base handoff archive <slug>` (or the `fork` equivalents). If it's leaking from global because it was written outside a registered workspace, the long-term fix is `base scaffold` on that project so future writes land in its own tier.
<!-- v0.11.0 | verified: reference -->

---

## Projects, milestones, tasks

### Q: How do I create a new project?
**A:** `base project add -n "<name>" [-s active] [-p <path>] [--stage <stage>]`. Status defaults to `active`. If you omit `-p` and the `[protocol]` config is enabled, the folder is derived from the protocol stage and auto-created for you.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I list projects, and does it show projects from other workspaces?
**A:** `base project list` by default shows only projects homed in the current workspace. Add `--all` to see the flat union across every registered workspace, `--workspace <W>` to filter to one named workspace, `--unscoped` to see projects with no path or no registered home, and `--json` for the stable dashboard-contract output.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I add a milestone or task to a project?
**A:** Milestone: `base milestone add -p <PROJECT> -n "<name>" [-d "<description>"]`. Task: `base task add -p <PROJECT> -n "<name>" [--priority <p>] [-m <MILESTONE>]` to optionally group it under a milestone. Both accept the project as a slug or its display name.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I mark a task done, and how do I inspect one?
**A:** `base task done <slug>`. Look up the slug with `base task list` (filterable by `-p` project, `-m` milestone, or repeatable `--label`, which ANDs together). `base task get <slug-or-name>` shows all fields for one task.
<!-- v0.11.0 | verified: cli-help -->

### Q: What happens if I delete a milestone or a project that still has work under it?
**A:** `base milestone delete` by default detaches its tasks to project-level rather than deleting them, it only cascade-deletes the tasks if you pass `--force`. `base project delete` refuses to delete a non-empty project unless you pass `--force`, which cascade-deletes its tasks, milestones, decisions, and rules. Both, like `project move`, are PREVIEW-only unless you also pass `--yes`, treat the two flags as a deliberate two-step confirmation, not a formality.
<!-- v0.11.0 | verified: cli-help -->

### Q: What does `base reconcile` do?
**A:** It reconciles a project's active/deferred status against real folder last-touch time on disk, projects that haven't been touched recently get marked deferred, ones with recent activity get marked active. Run `base reconcile --dry-run` to preview the changes without writing to the graph; note that `--dry-run` bypasses the `[protocol]` enabled gate that would otherwise skip the check entirely.
<!-- v0.11.0 | verified: cli-help -->

---

## Ingesting data (markdown, MOP, the CRM recipe)

### Q: How do I get my own markdown docs into the graph?
**A:** `base sync` walks the workspace, matches files against the `sync.include`/`sync.exclude` globs in `base.toml` (by default `**/*.md` and `**/paul.json`, excluding `node_modules/`, `target/`, `.git/`, `.base/`), and extracts frontmatter plus body content into the graph. Add `--incremental` to only re-extract files changed since the last sync, and `--repair` to backfill missing edges (decision to domain, milestone to project, task to project links) without touching content.
<!-- v0.11.0 | verified: reference -->

### Q: What frontmatter should my markdown files have for sync to work well?
**A:** Follow the MOP (Markdown Ontology Protocol): YAML frontmatter with `type`, `status`, `tags`, and `relatedTo` fields. The pre-tool-use hook actually injects the MOP contract automatically whenever you write or edit a `.md` file inside a registered workspace, so well-formed frontmatter is prompted for as you go. Files without it still get synced, but extraction quality and graph linking are worse.
<!-- v0.11.0 | verified: audit -->

### Q: What's the difference between `base sync` and `base graph extract`?
**A:** `base sync` is a mechanical extraction: it reads frontmatter and body text from markdown files you already have and files them into the graph as-is, no LLM involved. `base graph extract --target <dir>` is a semantic pass: it runs an LLM (Claude Code, via `-m/--model` to pick an alias) over the doc corpus and derives concepts plus relationship edges between them, building an actual knowledge graph structure rather than just filing documents. Use `sync` for routine bookkeeping of docs you maintain; use `graph extract` when you want base to understand the relationships between ideas across a set of documents, which is also the prerequisite for good `base graph query` answers.
<!-- v0.11.0 | verified: reference -->

### Q: Can base ingest PDFs, images, or audio/video?
**A:** Not by default, `base graph extract` is markdown-only out of the box with zero extra dependencies. Multimodal ingest is opt-in: `base config set multimodal.enabled true` turns it on for all future extracts, or pass `--multimodal` on a single `graph extract` call to force it for just that run. PDF parsing runs in-process (no dependency), image analysis goes through the already-present `claude` binary (vision), and audio/video go through Whisper, whose dependencies (`whisper` plus `ffmpeg`) install once via `pip install --user` the first time they're needed. No sudo is ever required.
<!-- v0.11.0 | verified: cli-help|source -->

### Q: Can I load data from my CRM (tags, workflows, forms) and have base help me with it? Does base have a native connector for that?
**A:** No native connector, base has no built-in API integration to any CRM platform. Ingestion is entirely file-based: base only reads markdown (via `base sync`) or runs LLM extraction over a markdown corpus (via `base graph extract`). The path in is to export or generate markdown reports of the data you care about into a docs folder inside a registered workspace, add MOP frontmatter to each file, then run `base sync` for straightforward filing or `base graph extract --target docs/` if you want the LLM to derive relationships between the concepts. The live pull itself, calling the CRM's API to produce those markdown reports, has to happen outside base in whatever tooling talks to the CRM; base only takes over once the data is on disk as markdown. After that, `base recall` or `base graph query` works over it like any other content in the graph.
<!-- v0.11.0 | verified: audit -->

---

## GraphRAG

### Q: What flags does `base graph query` take, and how do I explore the graph node by node instead of asking a question?
**A:** `base graph query "<your question>"` flags: `-d/--depth` controls how far the traversal expands (default 3), `-b/--token-budget` caps the size of the retrieved subgraph fed to synthesis (default 2000), `-m/--model` picks the Claude Code model alias used for synthesis, and `--raw` skips synthesis and prints just the retrieved subgraph, which is actually the highest-quality path if you want to reason over the raw facts yourself rather than trust a summary. For manual node-by-node exploration instead, three agentic-retrieval primitives let you drive your own multi-step traversal: `base graph get-node <NODE>` shows one node's full detail (label, type, source, summary, edges), `base graph neighbors <NODE> -d <N>` lists the N-hop neighborhood as edge lines, and `base graph path <FROM> <TO>` finds the shortest path between two nodes. All three accept a label, slug, or unique substring.
<!-- v0.11.0 | verified: cli-help -->

---

## AST and code navigation

### Q: How do I navigate code with base instead of grep or find?
**A:** Use `base ast query` with a flag for the kind of lookup: `-c/--contains "<name>"` finds entities by case-insensitive substring match, `-f/--file "<path>"` lists all entities in a file plus their relationships, `--calls "<name>"` finds every caller of that entity, and `-i/--imports "<file>"` finds every file that imports it. Query first to understand structure, then `Read` only the specific lines that matter, instead of scanning files blind.
<!-- v0.11.0 | verified: cli-help -->

### Q: What's the exact command to find something by name with base ast?
**A:** `base ast query -c "auth"` (or the long form `--contains "auth"`). It is a case-insensitive substring match against entity names in the mapped app's code graph. Alias: `base ast q -c "auth"`.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I list everything defined in one file using base?
**A:** `base ast query -f "main.rs"` (or `--file`). It returns all entities declared in that source file along with their call/import relationships, not just a flat symbol list.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I find every place that calls a function?
**A:** `base ast query --calls "validate"` returns all callers of the named entity across the mapped codebase. This replaces a manual grep for the function name.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I find every file that imports a given module?
**A:** `base ast query -i "config.rs"` (or `--imports`) lists every file that imports from the target file. Useful before renaming or refactoring a shared module.
<!-- v0.11.0 | verified: cli-help -->

### Q: Can I query another app's code map without cd-ing into it?
**A:** Yes, add `-t/--target <path>` to any `base ast query`, e.g. `base ast query -t apps/web -c "auth"`. It queries that app's own `.base-ast/ast.ttl` map directly from wherever you currently are, so you never have to `cd`.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I map a brand-new app so base ast query works on it?
**A:** Run `base sync --ast --target apps/foo` once from anywhere (or just `base sync --ast` from inside the app). This runs tree-sitter extraction (35+ languages supported) and writes the map to that app's own `.base-ast/ast.ttl` sidecar file. You only need to do this once per app.
<!-- v0.11.0 | verified: cli-help -->

### Q: After I map an app once, does the code map stay up to date automatically?
**A:** Yes. Once an app has been mapped (its `.base-ast/ast.ttl` exists), a Stop hook automatically re-runs `base sync --ast --target <app>` for the current app and any app you edited that turn, after each turn ends. This refresh is opt-in in the sense that it only fires for apps already mapped; an app that has never been synced stays unmapped forever until you run `base sync --ast` on it once.
<!-- v0.11.0 | verified: source -->

### Q: Is the Stop-hook AST refresh instant, or could my map be stale mid-session?
**A:** It's debounced 20 seconds and never awaited (spawned detached), so a burst of rapid turns can skip a refresh. Skipped apps aren't lost though; they're requeued for the next Stop event, so the map catches up shortly after edits settle.
<!-- v0.11.0 | verified: source|reference -->

### Q: Where does base actually store the AST code map?
**A:** In a per-app sidecar at `{app_root}/.base-ast/ast.ttl` (self-gitignored), not inside `.base/`. This matters because a stale hook check (see the AST false-nag entry under Known bugs) still looks in the old `{workspace}/.base/ast.ttl` location, so the map's real location is worth knowing directly.
<!-- v0.11.0 | verified: source -->

### Q: How do I see which apps already have a code map?
**A:** `base ast list` (alias `base ast l`). It prints a table of app name, entity count, map path, and last-synced timestamp for every registered per-app map. Use this instead of trusting the "not yet populated" hint, which has a known bug (see Known bugs).
<!-- v0.11.0 | verified: cli-help -->

### Q: Does the false "AST graph not yet populated" nag mean my queries are broken?
**A:** No. The query path (`find_ast_ttl`) checks the sidecar location correctly and works fine; only the pre-tool-use hint-check uses the stale legacy path. So the hint is wrong but harmless: ignore it, verify with `base ast list`, and query normally. See the Known bugs section for the underlying path mismatch.
<!-- v0.11.0 | verified: source|audit -->

---

## Hooks and context injection

### Q: What hook events does base respond to?
**A:** Five: session-start, user-prompt-submit, pre-tool-use, post-tool-use, and stop. All of them dispatch through `base hook <EVENT>`, which Claude Code calls automatically at each point in a turn.
<!-- v0.11.0 | verified: cli-help|source -->

### Q: What does the session-start hook inject?
**A:** A pipeline including: a graph-health warning if unhealthy, tier auto-compaction if bloated, clearing this session's own dedup state, domain sync, ingesting `paul.toml` projects, active/deferred state reconciliation, an operator profile block, an update banner, all signals (memory, active-awareness, pulse, flow-resurface, handoff, reminder, fork), the flow protocol block, extension status, a context-triggers cheat sheet, and relay inbox/task delivery.
<!-- v0.11.0 | verified: source -->

### Q: What does the user-prompt-submit hook do?
**A:** It increments the prompt count and derives the current bracket (FRESH/MODERATE/DEPLETED/CRITICAL), builds bracket rules, then checks for a star command. If a star command matched, it short-circuits everything else for that prompt. Otherwise it syncs domains, loads the merged graph, gathers active file paths, matches domains, injects rules/neighborhood per matched domain, and finally ticks the relay inbox/tasks.
<!-- v0.11.0 | verified: source -->

### Q: What does the pre-tool-use hook do?
**A:** It injects the AST file map for source files being touched, intercepts grep-style commands (`grep -r`, `rg`, `ag`, `ack`, `fd`, `find -name`, piped `grep`) with an `<ast-hint>` block nudging you toward `base ast query`, injects domain rules for matched paths, applies a markdown-frontmatter contract on Write/Edit of `.md` files, applies MIDAS standards on mutating operations, delivers mid-turn relay tasks/pings, and marks the current app "dirty" on any Edit/Write/MultiEdit/NotebookEdit so the Stop hook knows to refresh its AST map.
<!-- v0.11.0 | verified: source -->

### Q: What does the post-tool-use hook do?
**A:** It updates internal timestamps and injects section-specific AST context for partial file reads, i.e. context about the exact lines you just read, not the whole file.
<!-- v0.11.0 | verified: source -->

### Q: What does the stop hook do?
**A:** It spawns a detached `base sync --ast --target <app>` for the cwd app plus every app that was dirtied (edited) during the turn. This only applies to apps that already have a `.base-ast/ast.ttl` map (opt-in), is debounced 20 seconds, and is never awaited, so it doesn't block your session ending.
<!-- v0.11.0 | verified: source -->

### Q: How does pre-tool-use injection actually reach Claude, mechanically?
**A:** Differently from the other hooks. Session-start, user-prompt-submit, and stop write plain stdout, which Claude Code prepends directly into context. Pre-tool-use must instead use a JSON envelope, `{"hookSpecificOutput":{"hookEventName":"PreToolUse","additionalContext":"..."}}`, because plain stdout on that event is transcript-only and never reaches the model.
<!-- v0.11.0 | verified: source|reference -->

### Q: What actually controls whether something gets injected into my session?
**A:** A layered system: (1) domain matching against prompt keywords, file keywords, and active file paths, deterministic substring/path matching, nothing fuzzy or semantic, and never context-size-aware; (2) suppression, each signal's output is hashed and unchanged output is skipped, so silence is the default steady state; (3) a character budget (`signal.max_chars`, default 2000) capping most signal output, except handoffs, reminders, and forks, which are exempt from both suppression and the budget and surface every session until acted on; (4) the bracket system layering rule-injection intensity on top of session depletion. Layers 1 to 3 are purely trigger- and content-based. Layer 4 is the exception: in percent mode (what `base install` writes by default) the bracket reads real context depletion from the live transcript's usage block, so injection volume does adapt to how full your context window is. See the bracket pair below for the tiers and thresholds.
<!-- v0.11.0 | verified: source -->

### Q: Does base take the size of my current context window into account when deciding what to load?
**A:** Partly yes, through the bracket system. WHAT matches is never context-aware: domains fire on keyword/path matches against the prompt and recently-active files, with no regard for how full your context is. But HOW MUCH gets injected is context-aware when brackets run in percent mode, which is what current installs get: `base install` writes `bracket.mode = "percent"` into `base.toml`, and in that mode base reads the live Claude Code transcript's most recent `usage` block (summing `input_tokens` + `cache_read_input_tokens` + `cache_creation_input_tokens`) and divides by `bracket.context_window` (default 200000) to compute real depletion. That percentage picks the tier: FRESH at or below `fresh_until_pct` (default 20), MODERATE to 45, DEPLETED to 70, CRITICAL above. If `mode` is absent (a `base.toml` predating the feature) or the transcript has no usage yet, it falls back to counting prompts (`fresh_until` 3, `moderate_until` 10, `depleted_until` 20). So the bracket is a genuine live read of context depletion on a modern install, and a turn-count proxy only on legacy config or first prompt. It governs injection volume and dedup frequency, not whether injection happens at all.
<!-- v0.11.0 | verified: source -->

### Q: What are "brackets" and why do I see the same rules every prompt?
**A:** Brackets are a depletion-staging system configured under `[bracket]` in `base.toml` that tracks how deep you are into a session: FRESH (lean injection, e.g. skipping the graph neighborhood), MODERATE (full injection), then DEPLETED and CRITICAL (force-refresh dedup more often, on `refresh_interval`, default every 5 prompts). Which tier you are in depends on `bracket.mode`. In `"percent"` mode (what `base install` writes for new installs) base reads real context depletion from the live transcript and compares it to `fresh_until_pct` / `moderate_until_pct` / `depleted_until_pct` (defaults 20 / 45 / 70 percent of `context_window`, default 200000). When `mode` is absent or the transcript has no usage block yet, it falls back to prompt counts (`fresh_until` 3, `moderate_until` 10, `depleted_until` 20). Brackets govern how much and how often rules get re-injected, not what content matches your prompt. Bracket-tier rules are intentionally never deduplicated, they reinject every single prompt, on purpose, so your most critical standing rules don't quietly erode as the conversation fills up. The `always` bucket fires at every tier and is additive with the tier-specific bucket. They are also built before the star-command check, so they still fire even when a star command is active.
<!-- v0.11.0 | verified: source -->

### Q: How do I preview what base would inject for a given prompt, without actually submitting it?
**A:** `base context "<some prompt text>"` runs the exact same matching engine the session-start and prompt-submit hooks use, matching your text against configured domain triggers and printing whatever would have been injected. It's read-only and doesn't affect session state.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I see every trigger base currently has configured?
**A:** `base context --list` lists all available context triggers across domains without matching against any text, showing which domains exist and what keywords or paths would fire them (empty if a domain has no keywords configured, e.g. a global rules-only domain).
<!-- v0.11.0 | verified: cli-help -->

### Q: Why does base sometimes say nothing at session start? Is it broken?
**A:** Probably not, silence is the intended steady state once your setup is healthy and stable. Each signal's output is hashed and unchanged output is suppressed so you're not re-shown the same thing every session. Things that always surface regardless are open handoffs, due reminders, and open forks. If you have none of those and nothing changed, getting no output is correct behavior, not a failure. Verify with `base doctor` and, if you want to confirm hooks are actually firing, tail `hook-events.jsonl` in the workspace `.base/` folder, which logs `domains_matched` and `rules_injected` per hook event.
<!-- v0.11.0 | verified: source|reference -->

### Q: A hook doesn't seem to be injecting anything. How do I debug it?
**A:** Run the hook manually and read stderr, since hooks are fail-open and print errors only there. For example: `echo '{"prompt":"test message"}' | base hook user-prompt-submit`. If nothing prints to stdout and nothing useful is on stderr, the hook likely ran cleanly and simply had nothing to inject.
<!-- v0.11.0 | verified: source|audit -->

### Q: What example JSON do I feed a hook to test it manually?
**A:** It depends on the event. For user-prompt-submit: `echo '{"prompt":"your test text"}' | base hook user-prompt-submit`. For session-start you typically don't need a payload: `echo '{}' | base hook session-start`. The point is to bypass Claude Code entirely and see the hook's stdout/stderr directly.
<!-- v0.11.0 | verified: source -->

### Q: What does base's "fail-open" hook design actually mean, and why does it matter for debugging?
**A:** Every hook error is caught, printed to stderr only, and the process exits 0 with empty stdout, so a broken hook is indistinguishable from a hook that legitimately had nothing to say. This is deliberate: a broken graph or crashed extraction should never block your session. The consequence is that Claude Code typically doesn't surface hook stderr, so silent breakage can persist for a long time unnoticed. The fix is to run the hook manually via stdin JSON and inspect stderr directly, or check `hook-events.jsonl`, rather than trying to infer failure from missing output.
<!-- v0.11.0 | verified: source|audit -->

### Q: hook-events.jsonl keeps growing. Is that a problem?
**A:** It's an append-only audit log of every hook firing (timestamp, hook name, success, domains matched, rules injected, suppressed, ast_injected, grep_intercepted) written under `~/.base-gbl/` and, for workspaces, also under `{workspace}/.base/`. It has no rotation in v0.11.0, so it grows unbounded; on a machine that's used base for a while it can reach several megabytes. It's useful as a diagnosis tool (grep it for `"success":false` or a specific hook name) but currently needs manual cleanup if disk usage becomes a concern.
<!-- v0.11.0 | verified: source|audit -->

### Q: Even the audit log write itself can fail silently?
**A:** Yes, `log_hook_event` is fire-and-forget: if the write fails, nothing surfaces anywhere. It's one of several places in the codebase where fail-open means a broken write is simply invisible, consistent with the overall philosophy of never letting logging block a session.
<!-- v0.11.0 | verified: source|audit -->

---

## Session relay

### Q: What is base relay for?
**A:** Session-to-session messaging between live Claude Code sessions, letting one session hand work to, or ping, another session that's currently open, without you manually copy-pasting context between terminals. It's built for parallel workers on the same or related projects.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I set up two sessions to talk to each other?
**A:** Both sessions need a registered title first: `base relay register --as <title>` in each one (`base relay init` creates the ephemeral relay store for a project if you need it). Confirm both are visible with `base relay sessions`. Then from session A: `*task <title-of-B> <what to do>` for real work, or `*ping <title-of-B> <message>` for a quick question. Without a registered title, a session can't receive relayed work and can't be addressed by name.
<!-- v0.11.0 | verified: reference|cli-help -->

### Q: What are the relay "codenames" I keep seeing, and why do they matter?
**A:** Sessions register under a friendly title via `base relay register --as <codename>` (often an animal name). That title is how `*task`/`*ping` address a specific live session, and it's how the current session's own identity gets resolved: matching the session's row in `base relay sessions` against `$CLAUDE_CODE_SESSION_ID` answers "who am I" for handoff and relay purposes.
<!-- v0.11.0 | verified: source|reference -->

### Q: What's the difference between *task, *ping, and *inbox?
**A:** All three relay messages between two *live* Claude Code sessions that each have a registered title, and all three are real star commands (confirmed via `base commands show task` / `base commands show ping`). `*task <title> <what to do>` packages real work into a briefing doc and relays it via `base relay task`, dropping it into another session's inbox; that session picks it up and works it autonomously via its own hooks, no further input from you. `*ping <title> <message>` is a same-turn instant message sent via `base relay ping`, no doc, no ceremony, for something like "is the auth guard rebuilt yet?", pausing in-flight work just long enough to send. `*inbox` is what the *receiving* session runs to force-deliver a pending ping (since pings only arrive attached to a tool call) and reply to clear it. Both `*task` and `*ping` resolve an ambiguous or missing target by running `base relay sessions` first.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I send work from one session to another at the CLI level?
**A:** Both sessions need a registered title first (`base relay register --as <title>`). Then from the sending session run `base relay task --to "<title>" --slug "<kebab-case-slug>" --summary "<one-line summary>" --doc "<absolute-path-to-briefing-doc>" --priority high`. This drops the task into the target session's inbox and fires loudly in its hooks until picked up, working cross-workspace via the global tier. Verify with `base relay tasks`, and the receiver clears it with `base relay done <slug>`.
<!-- v0.11.0 | verified: cli-help|source -->

### Q: What's the difference between base relay task and base relay ping?
**A:** `task` is for heavier, briefed work: it requires a written doc, a slug, and an explicit `done` to clear it, and keeps firing until picked up. `ping` is an instant message with no doc and no done-ceremony: `base relay ping --to "<title>" --msg "<short message>"`, meant for quick questions. An unanswered ping keeps re-firing in the receiver's hooks; replying with a ping is what clears it.
<!-- v0.11.0 | verified: cli-help|source -->

### Q: How does a receiving session clear a task or ping?
**A:** For a relayed task, the receiver runs `base relay done <slug>` once finished (and should note completion in its own state docs per the task's Definition of Done); this clears the inbox alert and closes the graph mirror of that task so it stops re-firing. For a ping, the receiver replies with `base relay ping --to <sender> --from <your-title> --msg "..."`, that reply is what clears the alert firing in the sender's hooks; an unanswered ping keeps re-firing.
<!-- v0.11.0 | verified: cli-help -->

### Q: I sent a ping or task but the other session doesn't seem to have gotten it. Why?
**A:** Delivery happens through hooks, and hooks only fire on a tool call, so an idle receiving session sitting there doing nothing never sees a waiting message. The receiving session needs to actively make tool calls (this is exactly what the `*inbox` star command automates, via cheap `echo` polling) for the delivery hook to fire. Also note that `base relay poll` and `base relay board` are workspace-scoped, while `relay task`/`relay ping` route through the *global* session registry, so "No relay stores exist" from `poll`/`board` is not proof the message never arrived. `base relay sessions` showing the sender as live is the more trustworthy signal than an empty board.
<!-- v0.11.0 | verified: source|reference -->

### Q: How do I see what relay sessions and activity exist right now?
**A:** `base relay board` gives the operator's view: sessions, liveness, claims, and pending messages, scoped to the current workspace (add `--project` to scope further). `base relay sessions` lists titled sessions in the global registry, which is what `*task`/`*ping` targets resolve against.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I see inbound relay tasks addressed to any live session?
**A:** `base relay tasks` lists inbound relay tasks across all live sessions. Use it right after sending a `relay task` to confirm the entry shows up with status pending.
<!-- v0.11.0 | verified: cli-help -->

### Q: What message types does base relay send support?
**A:** `base relay send --to <TO> --type <TYPE> --msg <MSG>` supports types `claim`, `release`, `notify`, `unblock`, `contract-change`, `ready-to-merge`, `question`, `answer`. `--to` accepts a title, a session-id, `phase:<n>`, or `all`. This is the lower-level primitive that `task`/`ping` build convenience on top of.
<!-- v0.11.0 | verified: cli-help -->

---

## Config, secrets, admin surfaces

### Q: How do I read and change base's configuration?
**A:** Use `base config get <section.key>` to read a value, `base config set <section.key> <value>` to write one, and `base config list` to see everything. Keys use dot-notation against `base.toml` (e.g. `devmode.enabled`, `multimodal.enabled`). Config is tiered: global `~/.base-gbl/base.toml` loads first, and a workspace's `.base/base.toml` overlays it.
<!-- v0.11.0 | verified: cli-help|source -->

### Q: Are there other notable config toggles besides devmode and multimodal?
**A:** Yes. `grounding.enabled` makes every prompt-time injection carry a block instructing Claude to source-verify factual claims. `graph.auto_compact` (on by default) governs whether tier graphs auto-compact at session start once they exceed `graph.compact_threshold_mb` (default 12MB), gated by `graph.compact_cooldown_hours` (default 24h) to avoid churn.
<!-- v0.11.0 | verified: source -->

### Q: How do I install base?
**A:** Run `base install`. It builds the binary, symlinks it globally, creates `~/.base-gbl`, wires hooks into `settings.json`, and writes the manifest. Options: `--carl <path>` to migrate decisions from a `carl.json` file, `--skip-hooks` to skip wiring hooks into `settings.json`, and `--full` to register all ChrisAI components (PAUL, SEED, SKILLSMITH) in the manifest, not just base itself.
<!-- v0.11.0 | verified: cli-help -->

### Q: What does `base activate` do, and do I need it?
**A:** `base activate <KEY>` takes an activation key from the ChrisAI Skool community and removes attribution branding from base's output. It is optional: base functions fully without activation, this just controls attribution text. The key is validated against a SHA-256 hash baked into the binary; the actual key never appears in source or the binary itself.
<!-- v0.11.0 | verified: cli-help|source -->

### Q: How do I uninstall base?
**A:** Run `base uninstall`. It removes the hooks it wired into `settings.json`, removes the binary, and removes the base section from `CLAUDE.md`. By default this leaves `~/.base-gbl/` (your graph, decisions, secrets, standards) intact. See the Destructive operations section before considering `--purge`.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I store an API key or secret for base/plugins to use?
**A:** Run `base secret set` and it prompts you with echo off (masked, paste-friendly) so the value never appears in your terminal history or chat. It writes to `~/.base-gbl/.env` with file permissions `0600`. Plugins read secrets from their environment, you should never type a real secret directly into a chat session.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I see what secrets I have stored, without exposing them, and how do I remove one?
**A:** `base secret list` shows the stored key names with masked values, never the full secret. `base secret rm <key>` removes one.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I open base's web dashboard?
**A:** Run `base dashboard` (alias `base dash`). It launches the Command Center Dashboard, a local web UI, on port 3741 by default. Use `-p <PORT>` / `--port <PORT>` to choose a different port.
<!-- v0.11.0 | verified: cli-help -->

### Q: What is `base standards` and what does it do?
**A:** `base standards` (alias `std`) manages context-triggered MIDAS protocol standards that get injected automatically when you edit matching files. Subcommands: `sync` (sync a MIDAS protocols.md file into `standards.toml` plus graph Standard entities), `list` (show all standards with trigger/annotation counts), `get <name>` (show a standard's full config), and `test` (dry-run the matcher against a file to see its score and what would inject).
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I add a new standard? Is there a `base standards add`?
**A:** No, there is no `add` subcommand. Standards are synced in from a MIDAS protocols markdown file via `base standards sync`, not created ad hoc through the CLI. To add a standard, edit the source protocols.md file and re-sync.
<!-- v0.11.0 | verified: reference -->

### Q: What is a base extension, and how do I manage them?
**A:** Extensions are drop-in plugins that add commands to base (visible via `base ext list`). Manage them with `base extension` (alias `ext`): `list` (installed extensions), `validate <file>` (check an extension manifest before installing), `install` (copy a validated TOML manifest into `extensions/`), `remove <name>`, and `run <name> [args...]` to invoke a plugin command explicitly (collision-proof).
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I install a prebuilt extension without a Rust/build toolchain?
**A:** Use `base extension add`. It fetches a plugin's prebuilt binary for your host platform from its GitHub release `[dist]` block, verifies the sha256 checksum, and unpacks/installs it, no toolchain required. If no prebuilt asset exists for your host, it falls back to a local source build.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I create a new extension from scratch?
**A:** Use `base extension scaffold` to generate a conformant, cross-platform Bun plugin. Add `--bootstrap` and it's one command: writes the files, builds it, `git init`s, and creates plus pushes a private GitHub repo, ready to develop immediately.
<!-- v0.11.0 | verified: cli-help -->

### Q: How do I check the health of base's graph?
**A:** Run `base doctor`. It diagnoses graph health across both tiers (global and workspace) using a parser-independent check, and exits with a nonzero status code when something is unhealthy, so it's scriptable in a health-check pipeline. Add `--json` for machine-readable output. Follow up with `base commands list` to confirm star commands loaded, `base project list` to confirm you're in a registered workspace and projects are visible, and `base ast list` to check code maps if relevant.
<!-- v0.11.0 | verified: cli-help|reference -->

### Q: What does a healthy `base doctor` report look like?
**A:** Something like: `✓ global tier - HEALTHY`, followed by the graph file path, line count, byte size, and confirmation it ends with a newline, then an overall `Verdict: HEALTHY ✓`. Each registered tier (global, and each workspace) gets its own line.
<!-- v0.11.0 | verified: cli-help|audit -->

### Q: base doctor says my graph is unhealthy. How do I fix it?
**A:** Run `base doctor --repair`. It quarantines malformed lines and atomically rewrites the good set, taking a snapshot first so nothing is lost. If a repair goes wrong, `base doctor --restore` (bare, no argument) lists available snapshots, and `base doctor --restore <snapshot>` restores the workspace graph from one.
<!-- v0.11.0 | verified: cli-help -->

### Q: Where do doctor's backup snapshots go, and how many are kept?
**A:** Every repair, restore, compact, or purge operation snapshots the graph first, named `graph.nq.bak-<op>-<date>`, with the newest 10 kept. `base doctor --restore` with no argument lists them.
<!-- v0.11.0 | verified: reference -->

---

## Updating and versions

### Q: How do I check what version of base I'm running?
**A:** Run `base --version`. It prints the version baked into the running binary at build time (e.g. `base 0.11.0`), which is always accurate for the binary you are actually running, and is the source of truth for what code is running. Do not confuse it with the version recorded in `~/.base-gbl/manifest.toml`, which is a separate, independently-tracked value that can drift out of sync (see the stale-manifest bug under Known bugs).
<!-- v0.11.0 | verified: cli-help|source -->

### Q: How do I check if a newer version of base is available without installing it?
**A:** Run `base update --check`. It re-validates against the remote source and reports whether a newer version exists, without installing anything. For the `base` component specifically, this compares against GitHub releases; other ChrisAI components are checked against npm.
<!-- v0.11.0 | verified: cli-help|source -->

### Q: How do I actually update base to the latest version, and what is the update banner?
**A:** base checks for a newer version and can print an update banner as part of session-start injection. Run `base update` to self-update the base binary. Add `--force` to reinstall even when you are already on the latest version, or `--snooze` to dismiss the banner for 24 hours without updating (a snooze does not fix an underlying version mismatch, it only silences the banner).
<!-- v0.11.0 | verified: cli-help -->

### Q: Where does base actually get its updates from?
**A:** `base update` pulls from a license-gated ChrisAI distribution channel. Specifically: the `base` component itself is checked against GitHub releases (`fetch_github_version`), while other ChrisAI components (PAUL, SEED, SKILLSMITH, etc.) are checked against npm packages. The default source recorded in the manifest is `https://chrisai.cv/skool`.
<!-- v0.11.0 | verified: source -->

### Q: What's in the manifest.toml file and why does it matter for updates?
**A:** `~/.base-gbl/manifest.toml` tracks, per component (base, and optionally PAUL/SEED/SKILLSMITH with `--full`), the installed version, install path, and install timestamp under `[components.<name>]`, plus an `[update_check]` section holding `last_checked`, `ttl_seconds` (default 7 days between checks), `pending_update` (the pending update string, if any), and `dismissed_until` (the `--snooze` expiry timestamp). It's the single source of truth the update banner reads from, which is exactly why a manual binary swap that doesn't touch this file causes the stale-manifest bug described under Known bugs.
<!-- v0.11.0 | verified: source -->

---

## Known bugs (v0.11.0)

### Q: The update banner keeps nagging me even though I'm already on the latest version, or base --version disagrees with what other tooling reports. What's going on and how do I fix it?
**A:** This is a known, confirmed bug. The update banner never compares against your actual running binary; it compares the version recorded in `~/.base-gbl/manifest.toml` under `[components.base]` against the latest remote version. That `version` field is only ever written by `base install` or `base update`. `base --version` reports `env!(CARGO_PKG_VERSION)`, captured at build time and always accurate for the binary you are running, but nothing keeps the manifest in sync if you swapped the binary manually (e.g. `cp` a new build into `~/.local/bin/base`) instead of running an official install/update. The manifest simply goes stale and nags forever, and `base --version` will never explain the discrepancy because it reads the real binary, not the manifest.
**Fix:** hand-edit `~/.base-gbl/manifest.toml` directly. Under `[components.base]`, correct `version` to match what `base --version` actually reports. Under the `[update_check]` section, clear `pending_update` (the snooze timestamp `dismissed_until` also lives in `[update_check]`). There is no dedicated CLI command for this repair. Unlike `graph.nq`, `manifest.toml` is plain config rather than the graph, so hand-editing it is fine and expected here. Note `--snooze` only silences the banner for 24 hours via `dismissed_until`, it does not fix the underlying mismatch.
<!-- v0.11.0 | verified: source|audit -->

### Q: base commands import says it imported my commands, but base commands list then says "No commands configured." What happened?
**A:** This is a known, confirmed bug: `base commands import` corrupts `commands.toml` when any rule text contains a newline, tab, or backslash, because the writer only escapes double quotes when emitting TOML. This produces invalid TOML with unterminated lines. The loader then makes it worse: it parses with an `if-let` chain that has no `else` branch, so the parse error is silently swallowed and an empty command list is returned with no error message at all. A success message ("Imported N command(s)") and a totally empty result are consistent with each other, nothing tells you it failed. Re-running import against the already-corrupt file appends more duplicates rather than fixing anything.
**Fix:** never use `base commands import`. Instead copy a known-good, validated file directly: `cp validated-commands.toml ~/.base-gbl/commands.toml`, then confirm with `base commands list` that the expected count shows up.
<!-- v0.11.0 | verified: source|audit -->

### Q: base keeps telling me "AST graph not yet populated" / to run `base sync --ast` even though my AST map already exists and queries work fine. Why won't it stop?
**A:** Known path bug in the pre-tool-use hook's populated-check (the same check behind the grep-intercept hint). It looks for `{workspace}/.base/ast.ttl` to decide whether the AST graph is "populated," but `base sync --ast` actually writes maps to `{app_root}/.base-ast/ast.ttl`, a per-app sidecar directory that is not under `.base/`. Because the check looks in the wrong place, it never finds the file and the nag never clears, no matter how many times you run `base sync --ast`. The underlying AST data and `base ast query` are unaffected: the query path (`find_ast_ttl`) checks the sidecar location correctly, so only the hint is wrong.
**Fix:** ignore the nag. Trust `base ast list` (it reports the real state, with an entity count per mapped app) and run your `base ast query` command anyway, the results are correct.
<!-- v0.11.0 | verified: source|audit -->

### Q: I ran a command outside a registered workspace and got back an empty result instead of an error. Is my data gone?
**A:** No, but this is a real gotcha. Some commands fail loudly outside a workspace (`base project list`, `base ast list`, `base task list` all error with "no .base/ directory found"). Others silently fall back to the global tier only: `base doctor` reports just the global tier, `base commands list` shows only globally-scoped commands. Worst case: `base recall --keyword <term>` can return "No results found." from the wrong cwd when the same query inside the correct workspace returns real results, so an empty answer here is indistinguishable from a genuinely empty graph.
**Fix:** `cd` into the registered workspace before trusting an empty result, or run `base scaffold` to register one. Sanity-check with `base project list` first; if that errors, treat any "empty" result from another command as suspect.
<!-- v0.11.0 | verified: reference -->

### Q: Why do base's hooks never show an error, even when something clearly went wrong?
**A:** By design, all five hook events (session-start, user-prompt-submit, pre-tool-use, post-tool-use, stop) fail open: any internal error is written to stderr and the process exits 0 with empty stdout, so a broken graph or a bad query never blocks your session. The tradeoff is that failures are easy to miss, Claude Code typically doesn't surface hook stderr, so a broken hook just looks like a hook that silently had nothing to inject that turn.
**Fix:** if you suspect a hook is failing, check `hook-events.jsonl` in the relevant tier (`ts`, `hook`, `success`, `domains_matched`, etc. fields) as the audit trail, or turn on `devmode.enabled` to see per-domain firing decisions.
<!-- v0.11.0 | verified: source|reference -->

### Q: Is there a full list of places where base silently swallows errors instead of telling me?
**A:** Yes, several beyond the named bugs above: the command loader's `read_to_string`/`toml::from_str` chain has no `else`; extension status injection skips silently on malformed extensions, missing query files, or query errors; session-start signal errors print to stderr and just vanish from the injected block; the stop hook's filesystem operations are fire-and-forget (`let _ = ...`), so a failed AST refresh is invisible; hook event logging is fire-and-forget; and active-path gathering returns an empty list rather than erroring when no graph is available. This is a deliberate fail-open philosophy, but it means an empty or missing result is often not distinguishable from "nothing to report" without checking `hook-events.jsonl` or devmode output.
<!-- v0.11.0 | verified: source|reference -->

### Q: None of these bugs are fixed in the latest version?
**A:** Correct, as of v0.11.0 (the repo tip at the time of this audit), none of the bugs above (the stale-manifest update banner, the commands-import corruption, or the AST false-nag) have been fixed upstream.
<!-- v0.11.0 | verified: audit -->

---

## Destructive operations

### Q: What graph commands are safe to run freely, and which need caution?
**A:** Read-only and safe: anything named `list`, `show`, `get`, `query`, `search`, `recall`, `doctor` (without `--repair`), any `--check`, `--dry-run`, `--peek` flag, and a bare `base doctor --restore` (which only lists snapshots). Preview-by-default, meaning they show you the change but need an explicit `--yes`/`--apply`/`--force` to actually execute: `project move`, `project delete`, `task delete`, `milestone delete`, `graph move`, `graph purge --apply`, `relay dispose --force`. The one to actively avoid without real intent: `base uninstall --purge`, which deletes all of `~/.base-gbl/` (graph, commands, secrets) with no preview and no confirmation flag.
<!-- v0.11.0 | verified: reference -->

### Q: Which base commands should I be careful with, and why?
**A:** Ranked roughly by risk:
- `base uninstall --purge`: catastrophic. Deletes `~/.base-gbl/` entirely (graph, `commands.toml`, secrets `.env`, standards). No preview, no documented confirmation flag. Back up `~/.base-gbl/` first if in any doubt.
- `base project delete --force --yes`: cascade-deletes a project's tasks, milestones, and decisions. Previews unless `--yes`; refuses to run on a non-empty project without `--force`.
- `base graph purge --stale --apply`: deletes graph notes unread past a day threshold (default 21 days). Dry-run by default (needs `--apply` to actually delete); snapshots first; re-reading a note resets its clock.
- `base graph move` / `base project move`: rewrites named-graph ownership stamps across tiers. Previews unless `--yes`; backs up both tiers; atomic with rollback.
- `base task delete`: deletes a task node and its edges. Previews unless `--yes`.
- `base memory purge`: removes flat-file memories already confirmed in the graph. No preview flag.
- `base doctor --repair`: quarantines malformed graph lines. Snapshots first, so it's comparatively low-risk, but still a rewrite.
- `base decision delete --keyword <kw>`: deletes decisions matching a keyword. `--keyword` is required (no accidental delete-everything), but there's no dry-run preview shown before the delete happens.
<!-- v0.11.0 | verified: cli-help|reference -->

### Q: What does `base graph compact` / `base graph purge` / `base graph move` actually do?
**A:** `compact` dedups and canonicalizes a workspace graph in an atomic rewrite, snapshotting first. `purge --stale [--days N] [--apply]` removes notes that haven't been read in more than N days (default 21); it's a dry-run preview unless you pass `--apply`. `move` relocates a subgraph between workspace graphs, rewriting its named-graph stamp; it previews by default and requires `--yes` to actually execute, backing up both source and destination tiers with rollback support.
<!-- v0.11.0 | verified: cli-help -->

---

## Where base stores data

### Q: Where does each piece of base's data live on disk?
**A:** base uses two tiers that merge together when queried. The global tier lives at `~/.base-gbl/` and is loaded into every workspace: it holds the global graph (`~/.base-gbl/.base/graph.nq`), global config (`base.toml`), domain triggers (`domains.toml`), star commands (`commands.toml`), MIDAS standards (`standards.toml`), the install/update manifest (`manifest.toml`), an optional operator identity profile (`operator.toml`), and secrets (`.env`, mode 0600). The workspace tier lives inside each registered project at `{workspace}/.base/` and holds that workspace's own `graph.nq` plus config/domain/command overlays that layer on top of the global tier by name, along with handoff and fork docs and that workspace's `hook-events.jsonl`. A third location, `{app}/.base-ast/ast.ttl`, holds per-app AST code maps as a sidecar, separate from both tiers, and is never merged into the main graph.
<!-- v0.11.0 | verified: reference -->

### Q: If I set up base on one machine, is any of that portable to another machine, or is it all personal to this install?
**A:** Everything under `~/.base-gbl/` (the global graph, secrets, manifest with install paths and machine-specific version state, hook-events telemetry) is machine-local and not meant to be copied wholesale to another machine, particularly `.env` (secrets) and `manifest.toml` (which records this machine's own install/update history). Workspace-tier data (`{workspace}/.base/`) travels with the project itself if the project repo is shared, though it still reflects this machine's graph state until synced. Config and domain/standards definitions (the `.toml` rule files) are the most portable piece, since they're structured, human-authored policy rather than machine state, so they can reasonably be copied or version-controlled and reapplied elsewhere. Knowledge graphs (`graph.nq`), telemetry (`hook-events.jsonl`), and secrets should not be treated as portable.
<!-- v0.11.0 | verified: reference -->
