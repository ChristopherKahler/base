---
ontology: true
type: spec
domain: base-v2
status: planned
summary: Build the *fork star command — sibling to *handoff, for spinning up parallel side-work sessions from named features
tags: [base-v2, star-command, fork, handoff, cli, spec]
related: [graph-migration-spec, framework-sop]
---

# Build the `*fork` Star Command

## Pick this up

Build a new `*fork` star command in base — a sibling to `*handoff`. base-v2 source is this repo (`ops-sys/toolbox/frameworks/00-kit-base/`); it has `.paul/`, so run `/paul:plan` off this doc and build autonomously to green. This file is the contract — build to its Definition of Done.

## Concept

- **`*handoff` = CONTINUITY.** When context fills, it writes a resume doc and registers it so you pick the **same** work back up next session. **One open per project; creating one archives the prior.**
- **`*fork` = BRANCHING OFF PARALLEL SIDE-WORK.** Name one or more features, say "*fork those features," and base writes a forward **build-spec** per feature and registers it to surface at session start — so you spin up a separate session, call the doc by its **title**, and autonomously build that feature. Forks are **additive: multiple open at once**, and they never touch the continuity handoff.

**Live references** (both written + registered by hand this session — that manual act is exactly what `*fork` automates): `GRAPH-MIGRATION-SPEC.md` and this very file, `FORK-COMMAND-SPEC.md`.

## Mirror these files

- Star-command defs: `~/.base-gbl/commands.toml` — the `*handoff` entry is `[[command]] name = "handoff"` (its `rules` are the 4-step flow). Add `[[command]] name = "fork"` mirroring it.
- Star injection: `src/hook/user_prompt_submit.rs` ; CommandDef: `src/command.rs`
- Flow-doc CRUD (graph-backed create/list/archive): `src/crud/handoff.rs` ; `src/crud/mod.rs`
- Session-start surfacing: `src/signal/flow_resurface.rs` (resurfaces open handoffs — forks surface here too)
- Ontology node type: `src/ontology/ops.ttl` (the `Handoff` class) ; CLI wiring: `src/cli.rs`

## Build

1. **Data model** — add a `kind` (`handoff` | `fork`) to the flow-doc record in `src/crud/handoff.rs` + `ops.ttl`.
   - `handoff`: unchanged — one open per project; create archives the prior.
   - `fork`: **multiple open allowed**; create does **not** archive siblings; keyed by slug/feature.
2. **CLI** — `base fork create --project <p> --doc <abs>` and `base fork list` (or `base handoff create --kind fork` — pick the cleaner surface). Reuse the handoff plumbing. New/changed surfaces pass clap `--help`/`-h`/error **byte-for-byte** per the base-v2 CLI rule.
3. **Surfacing** — in `flow_resurface.rs`, list open forks at session start in their own **"Forks"** block (by title + doc path), distinct from the single Handoff resume line. Pick-up by naming the title; archive/snooze like handoffs.
4. **The `*fork` flow** (commands.toml `rules`, mirroring `*handoff` STEP 1–4):
   - STEP 1 — Identify the named feature(s) from the prompt/session. Each feature → its own fork doc.
   - STEP 2 — For each, synthesize a forward **build-spec** and Write it to `{workspace}/.base/forks/{slug}.md` (or the repo's `*-SPEC.md` convention). Sections: one-line title, Created, **Pick this up** (cold-session orientation), **Goal / Deliverables**, **Hard requirements**, **Definition of Done (tests green)**, **Key files**, **Open questions**. Forward — what to BUILD, not what was done.
   - STEP 3 — Register each: `base fork create --project <p> --doc <abs>`. **Additive** (does not archive other forks). Resurfaces at session start.
   - STEP 4 — VERIFY: `base fork list` shows each new fork open at its doc; report titles + that each resurfaces and is callable by title. Don't paste doc bodies.
5. Keep `*handoff` behavior identical. Leave `*close` (composes base+docs+handoff) working.

## PROTOCOL — doc name ↔ graph slug are identical

**A fork's doc filename (basename, no extension) and its graph slug MUST be the same string** — that single name is the title you call it by. This is non-negotiable: a mismatch means you can't reliably summon a fork by name.

- `base fork create` derives the slug from the **doc basename**, NOT a timestamp. The node IRI is `handoff/<doc-basename>` (`kind = fork`).
- Contrast: today's `base handoff create` auto-slugs `<project>-<epoch-ms>` (e.g. `base-v2-1782441596521244`) — which does **not** match its doc. The bootstrap workaround used this session was: create, then rewrite the 8 node triples' subject IRI to `handoff/<doc-basename>`. `base fork create` must remove that step by setting the slug correctly at creation.
- Recommended: backport the same derive-slug-from-doc behavior to `base handoff create` (optional `--slug`, default = doc basename) so the two commands are consistent.

## Definition of Done

- Typing `*fork <feature(s)>` writes + registers a fork doc per feature; `base fork list` shows them open; they resurface next session in a **Forks** block; calling one by title loads it to start the side work.
- **Doc basename == graph slug** for every fork (the protocol above), verified by a test.
- Multiple forks coexist (no archive-prior); `*handoff` still one-open-archives-prior and unchanged.
- New CLI surfaces pass clap byte-for-byte parity; add `tests/fork_test.rs` mirroring the handoff tests; `cargo test` green; no regressions.

## Key insight

`*fork` and `*handoff` are the **same plumbing** (`crud/handoff.rs` + `flow_resurface.rs`) split by one `kind` field. Fork just flips two behaviors: multiple-open instead of archive-prior, and forward build-spec instead of backward resume — plus the doc==slug naming protocol. That's why it's cheap.
