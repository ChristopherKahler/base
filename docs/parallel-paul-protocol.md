# Parallel Session Protocol — relay choreography

How multiple Claude sessions coordinate on one codebase without file
collisions, semantic cross-contamination, or the operator acting as courier.
Applies to PAUL orchestrator/worker fan-outs and Cadre firm Members alike.

## The two layers

**Worktrees are the safety layer; relay is the signal layer.** Every worker
runs in its OWN git worktree on its OWN branch — file-collision safety comes
from git, never from the claim protocol. Relay claims are advisory: they
prevent wasted work, not corruption.

The relay store is EPHEMERAL: `.base/relay/<project>/`, disposable at
milestone end. The workspace graph stays clean. Durable outcomes (decisions,
learnings) get PROMOTED explicitly:

```bash
base decision log --domain X --decision "..." --rationale "..."
base learn --text "..." --domain X
base relay dispose --project <p> --force   # then tear down
```

## Command reference

```bash
base relay init --project <p>                # create the store
base relay register --as <title> [--phase n] # join (binds CLAUDE_CODE_SESSION_ID)
base relay send --to <title|phase:N|all> --type <t> --msg "..." [--refs path]
base relay poll [--peek]                     # non-blocking read (consumes unless --peek)
base relay wait --from <t> --type answer --timeout 600   # BLOCKS in the CLI — zero tokens
base relay claim <path|phase:N> --note "..." [--ttl 3600]
base relay release <resource> [--force]
base relay board                             # operator view: sessions, claims, liveness
base relay export                            # inbox.nq snapshot for inspection
base relay dispose --project <p> --force     # end-of-milestone teardown
```

Message types: `claim` `release` `notify` `unblock` `contract-change`
`ready-to-merge` `question` `answer`.

Delivery is push-first: base hooks inject pending messages addressed to your
session at session-start and every prompt. Poll only inside explicit `wait`
gates — never loop the model on an empty inbox.

## Orchestrator choreography (PAUL fan-out)

1. **Foundation first, alone.** The orchestrator builds shared ground —
   migrations, models, interfaces, contracts — as commit zero. Contracts
   FREEZE at fan-out.
2. **Fan-out brief.** Each worker's plan includes a conventions section
   (existing decisions, idioms from prior phases) — convention drift is the
   quality tax. Include the register line: `base relay register --as
   worker-phase-<n> --phase <n>`.
3. **One worker = one phase.** Workers write ONLY their own phase plan +
   ledger entries. STATE.md is orchestrator-only (unify is the orchestrator's
   job).
4. **Contract changes block.** A worker needing a contract change sends
   `contract-change` to the orchestrator and BLOCKS on the answer
   (`base relay wait --from orchestrator --type answer`) — never changes a
   frozen contract unilaterally.
5. **Integration is serialized.** Workers send `ready-to-merge`; the
   orchestrator merges in defined order with the full suite between each
   merge.
6. **Test isolation.** Per-worktree test DB names: `{db}_{worktree-slug}`.
7. **Dispose is part of milestone close.** Promote durable outcomes, then
   `base relay dispose`.

## Cadre firm integration

Cadre Members are scheduled one-shot sessions (`claude --print` via Pulse, or
desktop-app routines). Two wiring options — both hook-delivered, zero prompt
surgery in the member's task text:

1. **Env binding (preferred).** Set `BASE_RELAY_AS=<member-name>` in the
   Member's Contract env (or the routine's environment). Every hook fire
   resolves identity from the env var: the member's pending messages inject at
   session-start, replies route automatically, heartbeats update the board.
2. **Prompt binding.** The assembled member prompt opens with:
   `base relay register --as <member-name> --project <p>`. Subsequent hook
   fires bind via the session id.

Member-to-member handoffs replace operator courier work: the IG-growth member
finishes a run and sends `notify` to the content member; the dev member
publishes `ready-to-merge`; the orchestrator (or the Board's next interactive
session) sees everything on `base relay board`. Gates stay in Cadre — relay
messages are coordination signals, never approvals. Anything requiring Board
approval goes through a Cadre Gate, not a relay message.

### The four seam conventions (Cadre ↔ relay boundary)

1. **Identity:** the relay TITLE is the Cadre member slug (`quill`,
   `sterling`) — stable across runs. Claude session ids bind per-run and are
   relay-internal. Cadre `member_run` ids never become identities; they travel
   in message `refs` (`--refs member_run:123`) for audit cross-reference.
2. **Claims vs Units:** Cadre Unit checkout decides WHO does the work —
   atomic, DB-backed, the assignment of record. Relay claims signal WHAT a
   live session is touching right now — advisory, TTL'd, ephemeral. A member
   holding a Unit may take relay claims while executing it. Never assign work
   through relay claims; never signal file-touches through Unit state.
3. **Storage:** the relay spool (`.base/relay/<project>/`) never holds
   operational state and firm.db never holds coordination chatter. Disposing
   a relay store must lose nothing Cadre needs — if it would, that data
   belonged in firm.db or the graph.
4. **Completion signaling:** the member's harness (Cadre post-run validation
   pipeline), not the model, sends the completion message
   (`--from <member> --type notify|ready-to-merge`). A Board Proxy or
   orchestrator harvests via `base relay wait --from <member>` — blocking in
   the CLI, zero tokens. This is the async-pulse harvest shape.

## Anti-patterns

| Anti-pattern | Why it fails |
|---|---|
| Model-level sleep/poll loops | Burns tokens to watch an empty inbox — `base relay wait` blocks in the CLI process instead |
| Claims as safety | Claims are advisory; only worktrees prevent corruption |
| Relay as approval channel | Gates are Cadre/Board territory; relay is coordination signal |
| Relay store as archive | It's disposable; promote durable outcomes to the graph, then dispose |
| Unregistered workers | Unrouted messages pile up — check `base relay board` for UNROUTED |
