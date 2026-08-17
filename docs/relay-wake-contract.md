---
type: doc
status: active
tags: [relay, wake, monitor, sentinel, hooks, idle-wake]
relatedTo: [relay-auto-wake-monitor, ping-chat-hub]
---

# Relay wake contract

Every relay-registered session keeps a **persistent harness Monitor** watching
its ping inbox, and proves it with a sentinel file. This is what makes an
*idle* session pingable: hooks only fire on activity, and the Monitor tool is
the one primitive that wakes a session mid-idle. There is no external daemon —
the contract is that every title arms its monitor, and that compliance is
observable from outside the session.

## The loop

1. **Title exists** — `base relay register --as <title>`, or the auto-codename
   assigned by `session_registry::touch()` on the first boundary hook.
2. **Arming block emitted** — `src/relay/wake.rs::arm_block()` renders the
   canonical watch script (single source of truth; never hand-edit a copy).
   It reaches the model three ways:
   - in-band in `base relay register` output (a boot sequence's last tool call
     is often `register` — a next-tool-call nudge would never fire),
   - forced at session-start via `hook/mod.rs::relay_task_tick`,
   - throttled (180s per title) on tool/prompt ticks whenever the sentinel is
     stale — this is the self-healing re-arm after a monitor dies or `/clear`.
3. **Session arms** — one Monitor call, `persistent: true`, script verbatim.
4. **Sentinel** — the loop touches `relay-inbox/<title>/.watching` every 5s
   poll. Freshness threshold: `WATCH_STALE_SECS = 15` (3× the poll; one slow
   loop can't flap it, a dead monitor shows within ~15s). Dotfiles are
   invisible to both the loop's `ls -1` and base's `*.json` inbox scan.

## Observability

- `base relay board` — `Watching` column: `✓`, `✗ stale <age>`, or `✗ never`.
- `base relay ping` — warns the sender when the target's sentinel is stale:
  the ping will land on the target's next tool call or prompt, not mid-idle.

## Rules

- One monitor per title per session; the arming block says "never arm a
  duplicate" and a fresh sentinel suppresses re-emission.
- A session holding several titles (auto-codename + explicit register) arms
  one monitor per title.
- Windows and WSL relay stores are separate; the sentinel lives beside each
  side's own `relay-inbox/<title>/`, so watching-state never crosses sides.
- Known edge: two live sessions bound to one title share a sentinel — the
  newer session skips arming while the older one's monitor keeps the sentinel
  fresh. Pings still deliver to the newer session via hooks.

## Files

| Piece | Where |
|---|---|
| Contract, script, sentinel, nudge throttle | `src/relay/wake.rs` |
| Hook emission (forced/throttled) | `src/hook/mod.rs::relay_task_tick` |
| In-band arm on register + ping warning | `src/cli.rs` (RelayAction::Register / Ping) |
| Board column | `src/relay/board.rs` |
