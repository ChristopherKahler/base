---
type: reference
status: active
tags: [base, translation-chain, positioning, messaging, feature-benefit, copy-source, marketing, identity]
relatedTo: [base, FEATURE-INVENTORY, operator-kit-47, voice]
---

# BASE — Translation Chain (Top 56)

Phase 2 of the extraction. Each of the 56 highest-leverage features from [`FEATURE-INVENTORY.md`](FEATURE-INVENTORY.md) walked up the chain:

```
Feature → Benefit → Outcome → Identity
```

**Who this is for (the target the Identity rung speaks to):** solo operators and small-team builders running Claude Code who are quietly exhausted by the same loop — re-explaining their codebase every session, watching the AI repeat a mistake it already made, prefacing every prompt with "remember, we always…", and feeling like the tool that promised leverage actually added babysitting. They don't want a smarter assistant. They want to *be* the operator whose system remembers everything and compounds. Every Identity line below is written to make that person see themselves.

**How to use this doc:** this is copy source, not finished copy. Each rung is a raw angle. Feature = spec sheet. Benefit = what changes for them. Outcome = the pain removed or capability gained. Identity = who they become. For ads/sales/content, usually open at Outcome or Identity and earn the right to mention the Feature. No fabricated metrics — outcomes are qualitative where a number would be invented.

---

## A. Foundation — what it fundamentally is

> The "one thing, no overhead, knows everything" base story. Differentiator vs CLAUDE.md (static) and MCP servers (standing cost).

### A1 — Single self-contained binary
| Rung | |
|---|---|
| **Feature** | BASE is one Rust binary (~20MB, dashboard included) that serves Claude, the hooks, and you — no server to run, no runtime, no dependency tree. |
| **Benefit** | You install one thing and the whole context system works, with nothing to keep alive or babysit. |
| **Outcome** | You never lose a session to a dead Node process or a flaky local server. It's just there, every time. |
| **Identity** | You're the operator whose tooling disappears into the work — no maintenance tax, no moving parts to think about. |

### A2 — One ontological graph for the whole operation
| Rung | |
|---|---|
| **Feature** | Your code, projects, people, decisions, and docs live in a single ontological graph instead of scattered across files, tickets, and your memory. |
| **Benefit** | Everything your AI needs to know about your business sits in one place it can actually query. |
| **Outcome** | You stop being the human integration layer between your own tools — the connections already exist. |
| **Identity** | You're the operator who runs on one source of truth, not a pile of apps you reconcile by hand. |

### A3 — SPARQL queries, zero inference cost
| Rung | |
|---|---|
| **Feature** | "What calls this," "what depends on this," "what did we decide" are answered by SPARQL against the graph — deterministic, no LLM in the loop. |
| **Benefit** | Lookups are instant and free; no tokens spent and no model guessing. |
| **Outcome** | Your AI returns the same right answer every time, and you're not paying inference just to find where something lives. |
| **Identity** | You're the operator whose AI *knows* instead of guesses — answers come from a graph, not a hallucination. |

### A5 — Two-tier graph (global + workspace)
| Rung | |
|---|---|
| **Feature** | BASE merges a machine-wide global graph with a per-workspace graph on every query, so context spans both what's true everywhere and what's true here. |
| **Benefit** | Your AI carries your universal lessons and the project at hand at once, without you choosing which. |
| **Outcome** | You stop re-teaching cross-project lessons in every new repo; the global tier follows you in. |
| **Identity** | You're the operator whose hard-won lessons compound across every project instead of evaporating when you switch repos. |

### A8 — CLI-over-MCP, zero standing context cost
| Rung | |
|---|---|
| **Feature** | BASE runs as `base hook <event>` over stdin/stdout, not an always-on MCP server, so it adds nothing to the context window until it has something to say. |
| **Benefit** | Full graph-awareness with none of the window bloat that kills long sessions. |
| **Outcome** | You run deep, multi-hour sessions without an always-on tool eating the context you need for the actual work. |
| **Identity** | You're the operator running a smart system that stays out of the way — presence without weight. |

## B. The injection pipeline — the heart

> The cluster that's "too powerful to describe" because there's no consumer analog. Lead the whole pitch here.

### B1 — Four hooks, auto-wired
| Rung | |
|---|---|
| **Feature** | BASE wires itself into all four Claude Code hooks — session start, prompt, pre-tool, post-tool — automatically at install. |
| **Benefit** | Context shows up at every decision point without you configuring anything. |
| **Outcome** | You set it up once and every session is context-aware by default, forever. |
| **Identity** | You're the operator whose AI is plugged into the work at every step, not bolted on after. |

### B3 — Prompt keyword → rules injected before Claude acts
| Rung | |
|---|---|
| **Feature** | When you type a prompt, BASE matches its keywords to your domains and injects the relevant rules, decisions, and notes from the graph before Claude responds. |
| **Benefit** | Claude already knows your conventions for whatever you just raised — you didn't remind it. |
| **Outcome** | You stop prefacing every request with "remember, we always do X here." |
| **Identity** | You're the operator whose standards are enforced automatically, not re-typed a hundred times. |

### B4 — AST file map injected before the read
| Rung | |
|---|---|
| **Feature** | Before Claude opens a source file, BASE injects its shape — entities, key symbols with line numbers, what it imports, and what imports it. |
| **Benefit** | Claude understands a file's role and blast radius before it reads a single line. |
| **Outcome** | You stop watching your AI change one thing and silently break three files it didn't know existed. |
| **Identity** | You're the operator whose AI sees the whole dependency picture, like a senior engineer who already knows the codebase. |

### B5 — Grep intercept → one graph query
| Rung | |
|---|---|
| **Feature** | When Claude tries to grep across files, BASE intercepts and points it to a single graph query that already has the answer. |
| **Benefit** | One query replaces a fifteen-file scan. |
| **Outcome** | You stop burning time and tokens watching your AI hunt through files for what the graph already mapped. |
| **Identity** | You're the operator whose AI asks the map instead of wandering the territory. |

### B7 — Markdown extraction contract, taught just-in-time
| Rung | |
|---|---|
| **Feature** | When Claude is about to write a markdown doc, BASE injects the extraction contract so it's authored with proper frontmatter, typed tags, and real links — graph-ready by default. |
| **Benefit** | Every doc your AI writes becomes a connected node, not a dead file. |
| **Outcome** | Your documentation compounds into the graph instead of rotting in a folder no one queries. |
| **Identity** | You're the operator whose knowledge base gets richer every time anyone writes anything. |

### B8 — Post-read call chain for the exact lines
| Rung | |
|---|---|
| **Feature** | After Claude reads a section of code, BASE injects the call chain for exactly those lines — what they call and what calls them. |
| **Benefit** | Claude knows the consequences of the specific code in front of it, scoped to what it actually read. |
| **Outcome** | You stop getting changes that pass locally but quietly break a caller upstream. |
| **Identity** | You're the operator whose AI reasons about ripple effects, not just the lines on screen. |

### B10 — Every agent inherits the same graph
| Rung | |
|---|---|
| **Feature** | Main session, subagents, Explore agents, workflow agents — every agent inherits the same hooks and the same graph. |
| **Benefit** | Your whole fleet sees the identical map; no agent starts blind. |
| **Outcome** | You fan out parallel agents and trust they share context instead of contradicting each other. |
| **Identity** | You're the operator running a coordinated fleet, not a swarm of amnesiacs. |

### B12 — Suppression and session dedup
| Rung | |
|---|---|
| **Feature** | BASE tracks what it already injected this session and refuses to repeat it — same file touched twice, same rules already present, nothing re-fires. |
| **Benefit** | You get context exactly once, when it's new, and silence when it isn't. |
| **Outcome** | Your context window stays clean across a long session instead of filling with the same reminders. |
| **Identity** | You're the operator whose system has the discipline to stay quiet — the rarest thing in AI tooling. |

## C. Session start — the heaviest-weighted hook ⭐

> The flagship cluster. Before you type a single word, the session-start hook boots your AI into your entire operation — every project across every workspace, your working set, your mission, the live state of the graph. CLAUDE.md is a static note you wrote once and forgot; this is a fresh briefing assembled every session that speaks before you do. If one thing carries the whole "it just knows" story, it's this.

### C-SS — Session-start orientation (the flagship hook)
| Rung | |
|---|---|
| **Feature** | The session-start hook fires before your first prompt: it syncs your domains, ingests every project across every registered workspace, and runs the full signal suite — assembling a live briefing straight from the graph. |
| **Benefit** | Your AI opens already oriented to your whole operation instead of blank and waiting to be told. |
| **Outcome** | You skip the cold start entirely — no recap, no "here's the context," no re-uploading your world every time you sit down. |
| **Identity** | You're the operator whose AI wakes up already knowing the business — every session starts at full speed. |

### C1 — Active projects and tasks at session start
| Rung | |
|---|---|
| **Feature** | At session start BASE injects your true working set — active projects and open tasks — derived from what you've actually touched. |
| **Benefit** | You open a session and your AI already knows what you're in the middle of. |
| **Outcome** | You skip the "here's what I'm working on" ramp-up every single time you sit down. |
| **Identity** | You're the operator who picks up exactly where they left off, with an AI that never lost the thread. |

### C8 — Operator profile injected every session
| Rung | |
|---|---|
| **Feature** | BASE injects your operator profile — North Star, deep why, values, the real objective — at the start of every session. |
| **Benefit** | Every agent works toward your actual goal, not a generic "be helpful." |
| **Outcome** | Your AI frames recommendations around your constraints — your time, your money, the life you're building — not abstract best practice. |
| **Identity** | You're the operator whose AI is aligned to your mission, not just your last prompt. |

### C10 — Context brackets scale injection by session depth
| Rung | |
|---|---|
| **Feature** | BASE scales how much it injects based on how deep you are in a session — lean when context is fresh, heavier after it's been compacted. |
| **Benefit** | You get the right amount of context for where the session actually is. |
| **Outcome** | Late in a session, when AI usually starts forgetting, BASE re-injects what matters instead of letting quality slide. |
| **Identity** | You're the operator whose long sessions stay as sharp at hour three as at minute one. |

### C3 — Resurfacing: newly unblocked + quietly overdue
| Rung | |
|---|---|
| **Feature** | At session start BASE surfaces work that just became unblocked (its blocker completed) and deferred items past their resurface date. |
| **Benefit** | The things that are newly ready, or quietly overdue, come find you. |
| **Outcome** | You stop sitting on work that's been ready for days because nothing told you it was unblocked. |
| **Identity** | You're the operator whose next move surfaces itself the moment it's ready. |

### C2 — Workspace pulse at a glance
| Rung | |
|---|---|
| **Feature** | The pulse signal injects a workspace-health readout at session start — what's clean, what's gone stale, what needs grooming. |
| **Benefit** | You see the state of your workspace the moment you open it. |
| **Outcome** | You catch neglected corners before they rot, without auditing anything by hand. |
| **Identity** | You're the operator who always knows the health of their operation at a glance. |

### C5 — Recurring ideas get promoted
| Rung | |
|---|---|
| **Feature** | When the same idea recurs across enough sessions, BASE surfaces it and suggests promoting it to a real project. |
| **Benefit** | Recurring thoughts stop getting lost; the system notices the pattern for you. |
| **Outcome** | You stop losing the idea you keep almost-starting, because it finally gets captured as work. |
| **Identity** | You're the operator whose system catches the threads you'd otherwise drop. |

## D. Code graph — the LSP-and-Graphify killer ⭐

> Easy to underrate because "code search" sounds mundane. It isn't. This replaces grep, LSP, and keyword-matching graph tools with one queryable structural map of your entire codebase — every symbol, every call, every dependency, across 35+ languages, answered instantly with zero inference. For a builder audience this is the most visceral proof that BASE isn't just another note layer — it actually understands the code.

### D2 — A complete structural map of your code
| Rung | |
|---|---|
| **Feature** | BASE maps every function, struct, class, import, and call relationship in your codebase into the graph as typed entities and edges. |
| **Benefit** | Your AI has the full structure of your code, not a guess assembled from whatever files it happened to read. |
| **Outcome** | You stop watching your AI rediscover your architecture from scratch every session. |
| **Identity** | You're the operator whose AI holds the whole codebase in its head — the way you wish you could. |

### D1 — Tree-sitter across 35+ languages
| Rung | |
|---|---|
| **Feature** | BASE extracts your codebase structure with Tree-sitter across 35+ languages into the graph. |
| **Benefit** | Whatever stack you're in, your AI has a structural map of it. |
| **Outcome** | You get the same "knows my codebase" intelligence in Rust, Python, or a polyglot repo. |
| **Identity** | You're the operator whose AI is fluent in the shape of your code, not just the text of it. |

### D3 — Find any entity by name (the grep replacement)
| Rung | |
|---|---|
| **Feature** | `base ast query --contains` finds any entity by name across the whole codebase, instantly, from the graph. |
| **Benefit** | You locate anything by name without scanning files or remembering where it lives. |
| **Outcome** | You and your AI stop opening five files to find the one definition you needed. |
| **Identity** | You're the operator who finds anything in their code in one query. |

### D6 — Who calls this, instantly
| Rung | |
|---|---|
| **Feature** | `base ast query --calls` returns every caller of a function instantly from the graph. |
| **Benefit** | You know who depends on something before you touch it. |
| **Outcome** | You refactor with confidence instead of grepping and praying. |
| **Identity** | You're the operator who changes code knowing exactly what it touches. |

### D8 — Query, don't scan
| Rung | |
|---|---|
| **Feature** | "What depends on this" is a graph query in BASE, not a multi-file text search. |
| **Benefit** | The answer is exact and instant, not a best-effort scan. |
| **Outcome** | You stop paying the time-and-token tax of your AI reading file after file to answer a structural question. |
| **Identity** | You're the operator whose AI consults a map instead of searching blind. |

## E. Business graph — projects, decisions, memory

### E7 — Decisions logged with rationale
| Rung | |
|---|---|
| **Feature** | BASE logs decisions with their rationale as first-class graph entities, searchable forever. |
| **Benefit** | The "why" behind every choice is captured, not lost in a Slack thread. |
| **Outcome** | Six months later you know why you chose Postgres — and so does your AI — so settled calls don't get re-litigated. |
| **Identity** | You're the operator who never re-fights a decision they already won. |

### E12 — Typed memory with mandatory links
| Rung | |
|---|---|
| **Feature** | `base learn` stores typed memory (insight, correction, decision, commitment, shift), each linked to a domain. |
| **Benefit** | Every lesson is filed where it'll resurface in the right context. |
| **Outcome** | A correction you make once shows up automatically the next time you're in that territory. |
| **Identity** | You're the operator who only has to teach a lesson once. |

### E14 — Recall resets the purge clock
| Rung | |
|---|---|
| **Feature** | `base recall` stamps each note's last-read time, and only notes you never reach for age out. |
| **Benefit** | Your useful knowledge stays; dead weight self-prunes. |
| **Outcome** | Your memory layer gets sharper over time instead of bloating into noise. |
| **Identity** | You're the operator whose knowledge base curates itself. |

### E15 — Mandatory edges, no orphans
| Rung | |
|---|---|
| **Feature** | BASE refuses orphans — learn, entity, and project all require a link to something. |
| **Benefit** | Nothing you capture is disconnected; it's always reachable from related context. |
| **Outcome** | You never hit "I know I wrote that down somewhere," because everything is wired to where you'd look. |
| **Identity** | You're the operator whose every note is findable by design. |

## F. Documentation graph

### F2 — The markdown body becomes edges
| Rung | |
|---|---|
| **Feature** | BASE parses the markdown body, not just frontmatter — headings, links, wikilinks, and @-mentions all become graph edges. |
| **Benefit** | Your docs connect to each other and to the code and entities they reference. |
| **Outcome** | A design doc isn't an island; it's linked to the code it describes and the decisions behind it. |
| **Identity** | You're the operator whose documentation is a navigable web, not a folder of dead text. |

## G. Domains — the matching layer

### G2 — Deterministic matching, no fuzzy guessing
| Rung | |
|---|---|
| **Feature** | BASE matching is keywords, paths, and excludes — no embeddings, no semantic guessing in the core loop. |
| **Benefit** | Context fires exactly when your trigger matches, and never when it doesn't. |
| **Outcome** | You can predict and tune what your AI knows; no black-box "maybe it'll surface." |
| **Identity** | You're the operator in control of your context, not hoping a vector search guesses right. |

### G6 — A domain can fire a live query
| Rung | |
|---|---|
| **Feature** | A domain can run a SPARQL query on match and inject live graph results as context. |
| **Benefit** | Mentioning a topic pulls the current state of that topic from your graph. |
| **Outcome** | You say "offer" and your AI has your live pricing, tiers, and objections — not last month's doc. |
| **Identity** | You're the operator whose AI speaks from current reality, automatically. |

## H. On-demand context

### H1 — `base context` on demand, same engine
| Rung | |
|---|---|
| **Feature** | `base context --keyword` pulls targeted graph context on demand using the exact engine the hooks use. |
| **Benefit** | You can summon the same intelligence manually whenever you want it. |
| **Outcome** | You're never stuck waiting for a trigger; the context is one command away. |
| **Identity** | You're the operator who can query their whole operation in a breath. |

### H6 — Wired offer/ICP query pack
| Rung | |
|---|---|
| **Feature** | BASE ships a query pack (ICP, offer brief, tiers, objections, transformation) that fires on the right keywords. |
| **Benefit** | Your offer and audience knowledge inject themselves into copy, sales, and content work. |
| **Outcome** | You write an ad and your AI already has the transformation, the objections, and the proof points in front of it. |
| **Identity** | You're the operator whose marketing AI knows your business cold, not a generic copywriter. |

## I. Durability — built like infrastructure

### I2 — Lenient reads, strict writes
| Rung | |
|---|---|
| **Feature** | BASE reads leniently (skips a bad line, warns) and writes strictly (refuses on an unhealthy graph), so corruption is contained, not propagated. |
| **Benefit** | One bad line never blanks your context, and a broken graph never gets silently overwritten. |
| **Outcome** | You never lose a session's worth of context to a single parse error. |
| **Identity** | You're the operator whose memory layer is built like infrastructure, not a toy. |

### I4 — Self-healing graph
| Rung | |
|---|---|
| **Feature** | `base doctor --repair` quarantines malformed lines and atomically rewrites the clean set, snapshotting first. |
| **Benefit** | A corrupt graph self-heals with one command instead of a manual recovery. |
| **Outcome** | You recover from a bad state in seconds, not an afternoon of hand-editing. |
| **Identity** | You're the operator whose system fixes itself. |

### I7 — Stale notes prune themselves
| Rung | |
|---|---|
| **Feature** | `base graph purge --stale` removes notes unread past a threshold, and recall resets the clock on anything you use. |
| **Benefit** | Your knowledge base prunes what you never touch while protecting what you do. |
| **Outcome** | The graph stays lean and fast without you manually gardening it. |
| **Identity** | You're the operator whose system stays clean on its own. |

## K. Operator identity

### K2 — Your mission, injected for every agent
| Rung | |
|---|---|
| **Feature** | Your operator profile injects on every session start, so every agent knows who it works for and what the objective is. |
| **Benefit** | You never get generic AI; you get an AI pointed at your specific mission. |
| **Outcome** | Recommendations come framed around your goals and constraints every time, without you restating them. |
| **Identity** | You're the operator whose AI works for *you* specifically, not for "a user." |

## L. Handoffs

### L3 — Pick up where you left off
| Rung | |
|---|---|
| **Feature** | BASE resurfaces registered handoff docs as "Pick up where you left off" at session start until you handle them. |
| **Benefit** | A stopped task greets you next session with everything needed to resume. |
| **Outcome** | You lose zero ramp-up after a break, a weekend, or a context switch. |
| **Identity** | You're the operator who never loses momentum across sessions. |

## M. Config and the active/deferred protocol

### M3 — DEVMODE telemetry
| Rung | |
|---|---|
| **Feature** | DEVMODE shows, per prompt, which domains fired, why, what was deduped, and what's available — a live readout of your context engine. |
| **Benefit** | You can see exactly what your AI knows and why. |
| **Outcome** | You tune your setup with evidence instead of guessing why context did or didn't show up. |
| **Identity** | You're the operator who can see inside their own system and dial it in. |

### M4 — Honest active list, auto-reconciled
| Rung | |
|---|---|
| **Feature** | BASE auto-defers projects gone cold past a threshold and revives ones you've touched, from real folder activity. |
| **Benefit** | Your active list reflects what you're actually working on, not what you declared weeks ago. |
| **Outcome** | You open your workspace and the working set is honest — no stale "active" projects cluttering the view. |
| **Identity** | You're the operator whose project list is always true, without grooming it. |

## N. Workspace registry

### N1 — One coherent view across every workspace
| Rung | |
|---|---|
| **Feature** | BASE scans every registered workspace for projects at session start, so resolution spans all of them. |
| **Benefit** | Your AI knows about all your workspaces, not just the one you opened. |
| **Outcome** | You stop losing a project because it lives in a different repo; the registry finds it. |
| **Identity** | You're the operator with one coherent view across their entire operation. |

## O. Star commands — operator working modes

### O1 — Switch the AI's whole mode with one token
| Rung | |
|---|---|
| **Feature** | Typing `*NAME` injects a packaged behavioral ruleset from commands.toml, managed by `base commands`. |
| **Benefit** | You change your AI's whole working mode with a single token. |
| **Outcome** | You shift from brainstorming to auditing to shipping without re-explaining how you want it to behave. |
| **Identity** | You're the operator with a command palette for how your AI thinks. |

### O2 — A stance for every job
| Rung | |
|---|---|
| **Feature** | BASE ships modes like *BLUNT (answer-first), *AUDIT (find problems first), *STEELMAN, *OPERATOR (ROI framing), *EDITOR (tighten-only). |
| **Benefit** | You summon a specific cognitive stance instead of fighting the default helpful-and-verbose AI. |
| **Outcome** | You get a skeptical auditor when you need flaws found and a blunt advisor when you need the answer, instantly. |
| **Identity** | You're the operator who directs the mind, not just the task. |

## P. Extensions — frameworks plug into the engine

### P1 — Wire in a whole framework with one TOML
| Rung | |
|---|---|
| **Feature** | A framework wires into BASE's hooks by dropping one TOML file in the extensions folder — auto-discovered, no registration. |
| **Benefit** | Any tool you build plugs into the context engine without touching BASE's core. |
| **Outcome** | You extend your whole AI operating system by writing a config file, not a plugin. |
| **Identity** | You're the operator whose system is endlessly extensible — including by future-you. |

### P7 — The verify-reflex
| Rung | |
|---|---|
| **Feature** | An extension's post-tool inject nudges Claude to verify after it writes a matching file, once per session (e.g. "design work detected → run the humanizer"). |
| **Benefit** | Your quality gates fire automatically right after the work that triggers them. |
| **Outcome** | You stop shipping the thing you forgot to check, because the system reminds the AI at the exact moment. |
| **Identity** | You're the operator whose standards enforce themselves at the point of action. |

## Q. Command plugins

### Q1 — Grow your CLI, keep one safe surface
| Rung | |
|---|---|
| **Feature** | An extension can add brand-new `base <name>` commands routed to a handler in any language, with BASE as the only graph writer. |
| **Benefit** | You grow your CLI with new powers while keeping one safe, consistent surface. |
| **Outcome** | Your image generator, your content pipeline, your custom tools all live under one command, sharing one graph. |
| **Identity** | You're the operator building a personal operating system, not collecting disconnected scripts. |

## S. Command Center dashboard

### S1 — Your whole operation, one command
| Rung | |
|---|---|
| **Feature** | `base dashboard` starts an embedded server and SPA compiled into the binary — no npm, no setup — and opens your operation in the browser. |
| **Benefit** | Your entire graph and operations board is one command away. |
| **Outcome** | You get a Notion- or Linear-grade view of your business without maintaining a separate app or database. |
| **Identity** | You're the operator with a command center, not a stack of spreadsheets. |

### S2 — See your business as a system
| Rung | |
|---|---|
| **Feature** | The dashboard renders your live graph as an interactive network — color-coded, clickable, searchable — with notes that persist back into the graph. |
| **Benefit** | You can see and navigate everything your AI knows, visually. |
| **Outcome** | You spot connections and gaps in your operation at a glance instead of holding them in your head. |
| **Identity** | You're the operator who can actually see their business as a system. |

### S4 — Watch the engine think, live
| Rung | |
|---|---|
| **Feature** | The dashboard streams every hook event across all your sessions — prompts, tool calls, domains matched, rules injected, deduped — in real time. |
| **Benefit** | You watch your context engine work as it happens. |
| **Outcome** | You know exactly what your AI is being told and when, across every session you're running. |
| **Identity** | You're the operator with full visibility into how your AI thinks, not a black box. |

## T. Install and distribution

### T1 — One command turns Claude Code into an OS
| Rung | |
|---|---|
| **Feature** | `base install` copies the binary, creates the global config, wires all four hooks into settings.json, and adds a CLI reference to CLAUDE.md. |
| **Benefit** | One command turns Claude Code into a context-aware operating system. |
| **Outcome** | You go from blank Claude Code to fully wired in under a minute, nothing configured by hand. |
| **Identity** | You're the operator who ships a working system, not a weekend of setup. |

## U. Ecosystem — how BASE ties the whole OS together

> The "this is bigger than a dev tool" cluster. Where the meta-OS / productized-business story lives.

### U1 — The memory layer under PAUL
| Rung | |
|---|---|
| **Feature** | BASE ingests PAUL projects automatically and the handoff flow defers to PAUL when present — BASE is the memory layer beneath the planning engine. |
| **Benefit** | Your planning framework and your context graph are already integrated. |
| **Outcome** | You plan in PAUL and your AI carries that plan's full context into every session without you bridging them. |
| **Identity** | You're the operator whose planning and memory are one connected system. |

### U3 — Shipped as the $47 Operator Kit
| Rung | |
|---|---|
| **Feature** | BASE ships as the $47 Operator Kit — an installer plus an operator chain (business context → OS config → CLAUDE.md → voice → brand) that stands up a complete Claude Code operating system in about 90 minutes. |
| **Benefit** | Someone with zero setup can have a full system running in an afternoon. |
| **Outcome** | You hand a buyer a guided path from blank machine to configured operator, not a pile of docs to decipher. |
| **Identity** | You're the operator who packaged their edge into something anyone can install. |

### U6 — os-config wires the AI into the business
| Rung | |
|---|---|
| **Feature** | The os-config operator scaffolds BASE, writes domains and rules, classifies your profile, and wires external connections (MCP, OAuth, Railway) from each pillar's connections.toml. |
| **Benefit** | Your AI gets connected to your business's actual tools, not just configured in the abstract. |
| **Outcome** | You move from "Claude knows my code" to "Claude is wired into my CRM, my docs, my ad account" through one guided operator. |
| **Identity** | You're the operator whose AI is plugged into the whole business — brain and hands. |

### U8 — Leverage Score measures connection completeness
| Rung | |
|---|---|
| **Feature** | The Leverage Score judges not just whether frameworks are installed but whether Claude is actually connected to your business's external tools. |
| **Benefit** | You get a real measure of how much leverage your setup gives you. |
| **Outcome** | You see the exact gap between "AI installed" and "AI running my business," and close it. |
| **Identity** | You're the operator who measures and raises their own leverage on purpose. |

### U11 — The ship-map sees your whole portfolio
| Rung | |
|---|---|
| **Feature** | The ship-map generator audits 13-pillar shippability mechanically — what's package-ready versus stranded in apps/ — from your asset tags. |
| **Benefit** | You always know what's ready to sell or ship. |
| **Outcome** | You stop guessing which of your tools is productizable; the map tells you, computed from the filesystem. |
| **Identity** | You're the operator who can see their whole portfolio's shippability at a glance. |

---

## Patterns worth noticing (for whoever writes the copy)

- **Three recurring Identity territories** the features collapse into, each a different buyer: *(1) the operator whose AI remembers and compounds* (A2, A5, B3, B12, **C-SS**, C1, **D2**, E7, E12, E14, L3), *(2) the operator who sees and controls the system* (D6, G2, M3, S2, S4, U8), *(3) the operator who packaged their edge into a product* (T1, U3, U6, U11). Pick one per asset; don't blend.
- **The strongest single Outcome line** for cold traffic is **C-SS / B3 / C1** territory: "stop re-explaining yourself to your AI every session — it already knows." That's the felt pain.
- **The two flagships — lead with these.** The single heaviest-weighted feature is the **session-start hook (C-SS)**: your AI boots into your entire operation before you type a word — CLAUDE.md is static, this is a live briefing that speaks first. The **code graph (D2 / D8)** is the builder-audience flagship: it replaces grep + LSP + keyword graph tools with one instant structural map. The pure no-analog differentiator underneath both is **B12 (the silence)** + **A8 (zero standing cost)**.
- **Phase 3 candidates:** turn the top ~10 Identity lines into hooks, then back-fill the Outcome/Benefit as the body. The Feature rung is the proof you earn the right to show last.
