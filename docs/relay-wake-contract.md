---
type: doc
status: active
tags: [relay, wake, monitor, sentinel, hooks, idle-wake]
relatedTo: [relay-auto-wake-monitor, ping-chat-hub]
---

# Relay inbox watcher

Every relay-registered session keeps a harness Monitor watching its ping inbox,
and proves it with a sentinel file. This is what makes an *idle* session
pingable: hooks only fire on activity, and the Monitor tool is the one primitive
that wakes a session mid-idle. There is no external daemon. Every title runs its
watcher, and whether it does is observable from outside the session.

## The loop

1. **Title exists**: `base relay register --as <title>`, or the auto-codename
   assigned by `session_registry::touch()` on the first boundary hook.
2. **Setup printed on request**: `base relay arm` prints what to start the
   watcher with, rendered by `src/relay/wake.rs::arm_text()` from the canonical
   watch script (single source of truth; never hand-edit a copy): the Monitor
   tool's `description`, `command` and `timeout_ms`, the re-arm instruction, the
   status line and the register step. `base relay register` prints the same text
   when the title has no watcher running today's script (a boot sequence's last
   tool call is often `register`, so it rides that output).
3. **One line in the hooks** (BO-04, F4b): with no current watcher, session
   start and prompt-submit carry
   `relay: <title> has no inbox watcher · run base relay arm and start the Monitor it prints`.
   Once per session, and once more each time a watcher dies (the sentinel was
   written after the line was given, and is stale now). Never the script, and
   never on a tool call.
   **Only for a title in use** (BO-27, V2): one set by hand (`base relay
   register --as`, or pinned at launch by `BASE_RELAY_AS`), or one this session
   has sent a ping or task from or been sent one to (`used_by` on its row in
   `sessions.json`, `SessionEntry::in_use`). An auto-codename nobody has used
   gets no line, so a user who never uses relay is never told to start a
   Monitor. A ping to such a session still reaches it at its next prompt, and
   from then on its title is in use. `[relay] wake_nudge = false` turns the line
   off for every title.
4. **Session arms**: one Monitor call with those fields. Claude Code's Monitor
   tool has no `persistent` field and caps `timeout_ms` at 1,800,000 (30
   minutes), so the watcher always expires; when it does, run `base relay arm`
   again and start it again.
5. **Sentinel**: the loop writes `relay-inbox/<title>/.watching` every 5s poll.
   Freshness threshold: `WATCH_STALE_SECS = 15` (3× the poll; one slow loop
   can't flap it, a dead monitor shows within ~15s). Dotfiles are invisible to
   both the loop's `ls -1` and base's `*.json` inbox scan.

## What the hooks show of a message (BO-04, F13)

- Relay text never claims priority over the user's prompt. A ping is
  information: who, when, the message on its own line, and the reply command.
- Relay content appears at session start and on a prompt, never on a tool call.
  The one exception: a run that sets `BASE_NO_WAKE_NUDGE` (it cannot keep a
  watcher) gets each new item once on its next tool call, in the same plain
  form, with no watcher line. `BASE_RELAY_AS` does not count: the operator's
  launchers set it on interactive sessions.
- A ping, task or notify carries the session id that held its title when it
  was sent, and only that session is shown it (BO-05, F12). A title that passes
  to another session (a restart, a /clear, a codename handed out again, a
  `relay register` from another session) does not pass its inbox: what was sent
  to the previous holder moves to `relay-inbox/.archive/<title>-<session>/`, the
  new holder starts with an inbox holding only what was sent to it, and each
  sender of something no session was shown gets one `relay: not delivered`
  notice, addressed to the session that sent it. A ping no session was shown in
  a day is archived the same way, and its sender told.
- A new ping is shown once. While it stays unanswered, session start lists it in
  one line; prompts do not repeat it.
- Of one sender's unshown messages, only the newest is shown; the older ones are
  kept, marked `superseded`, and the shown one names the command that lists
  them (`base relay tasks --from <sender>`, or for the spool
  `base relay poll --project <p> --peek --all --from <sender>`). A hidden ping
  is still listed as unanswered. A sender with no title is never one thread.
- A reply to a sender (a ping to someone whose ping is waiting) clears that
  sender's pings and hidden messages and marks seen what it sent up to its
  newest ping, so nothing it sent before is shown again. A ping that answers
  nothing marks nothing.
- A `relay send` message is written to the spool and dropped as a wake notify;
  the notify shows it, and marks the spool copy seen, so it is shown once.

## Observability

- `base relay board`: `Watching` column: `✓`, `✓ old script`, `✗ stale <age>`, or `✗ never`.
- `base relay ping`: warns the sender when the target's sentinel is stale:
  the ping lands on the target's next prompt, not mid-idle.

## Opt-out

`BASE_NO_WAKE_NUDGE=1` suppresses the hook line for a session whose harness has
no Monitor tool (Agent SDK runs, brain.js NPC sessions). The text on
`relay register` and `relay arm` still prints; it is output, not context.

## Rules

- One watcher per title per session; the setup text says not to start a second
  one, and a current sentinel suppresses the line.
- A session holding several titles (auto-codename + explicit register) arms
  one watcher per title.
- Windows and WSL relay stores are separate; the sentinel lives beside each
  side's own `relay-inbox/<title>/`, so watching-state never crosses sides.
- Known edge: two live sessions bound to one title share a sentinel. The newer
  session sees a fresh sentinel while the older one's monitor runs; pings still
  reach the newer session through its prompts.

## Files

| Piece | Where |
|---|---|
| Setup text, script, sentinel, nudge line and its once-per-session rule | `src/relay/wake.rs` |
| Hook emission (session start, prompt) | `src/hook/mod.rs::relay_task_parts_deferred` |
| Inbox delivery (shown once, superseded, session-start listing) | `src/relay/task_inbox.rs` |
| Spool delivery | `src/relay/deliver.rs` |
| `relay arm`, setup on register, ping warning | `src/cli.rs` (RelayAction::Arm / Register / Ping) |
| Board column | `src/relay/board.rs` |

## Switching it off

The watcher line is on by default; `[relay]` in `~/.base-gbl/base.toml` governs it:

| Key | Default | Effect when `false` |
|---|---|---|
| `relay.enabled` | `true` | No auto-codename, no watcher line. A session that runs `base relay register` itself still takes part. |
| `relay.wake_nudge` | `true` | Titles and pings stay; the watcher line is never injected. |

`base config set relay.enabled false` / `base config set relay.wake_nudge false`.
`BASE_NO_AUTONAME` and `BASE_NO_WAKE_NUDGE` are the per-process equivalents.

Everything the watcher touches is local: the inbox folder, the `.watching`
sentinel, the `.watch-nudge` record (which session was told, and when) and the
`.status` line all live under `~/.base-gbl/.base/relay-inbox/<title>/`. The
setup text names the operator from `base operator init`'s profile (or says "the
operator") and says how to turn the line off.
