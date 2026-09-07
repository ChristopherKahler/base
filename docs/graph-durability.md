---
type: Reference
status: active
tags: [base, graph, durability, recovery, doctor, backup, operations]
relatedTo: ["GRAPH-DURABILITY.md", "README.md"]
---

# Graph Durability & Recovery

How BASE keeps `graph.nq` durable, how to recover when it isn't, and the one rule
that prevents the failure mode this whole subsystem exists for.

> **THE RULE: never hand-edit `graph.nq`.** Use `base graph` and `base doctor`.
> Every corruption incident to date came from a hand-run edit (a shell/python
> "stale purge" that was interrupted mid-write). BASE's own writes are atomic and
> validated; manual edits are neither. If you think you need to edit the graph by
> hand, there is a command for it — use that.

---

## Durability model

- **Atomic writes.** Every mutation goes through `store::write_back`: serialize to a
  temp file → **re-parse-validate** the temp → atomic `rename` into place. A crash
  mid-write leaves the live graph untouched; a serializer bug is caught before it can
  overwrite good data.
- **Fail loud.** A parse failure exits non-zero (CLI) and prints a warning at session
  start — corruption surfaces immediately, not three prompts later when a recall comes
  back empty.
- **Lenient reads, strict writes.** Reads (recall, hook injection) fall back to a
  lenient parse that skips malformed lines and warns, so a single bad line never blanks
  your context. Writes stay strict and refuse to run on an unhealthy graph — so a corrupt
  graph is never silently rewritten with lines dropped. Repair is always explicit.
- **Snapshots + rotation.** Every repair / restore / compact / purge snapshots the graph
  first to `graph.nq.bak-<op>-<date>` and keeps the newest 10, rotating older ones out.

---

## Recovery runbook

When a session warns the graph is unhealthy, or `recall`/`learn`/`sync` misbehave:

### 1. Diagnose — `base doctor`
```
base doctor            # human report, both tiers (workspace + global)
base doctor --json     # machine-readable, for agents/hooks
```
Parser-independent — it works *because* the graph is broken. Reports per tier: parse
status, the malformed line number, line/byte counts, trailing-newline (truncation)
check, stale `graph.nq.tmp` (an interrupted write), entity composition, and a line-delta
vs the newest backup (a data-loss smell). Exits non-zero if any tier is unhealthy.

### 2. Repair — `base doctor --repair`
```
base doctor --repair
```
Snapshots first, then lenient-loads the good quads, quarantines the malformed lines to
`graph.nq.quarantine-<date>`, atomically rewrites the good set, and re-verifies. One bad
line costs one triple, not the whole graph.

### 3. Restore — `base doctor --restore`
```
base doctor --restore                 # list available snapshots
base doctor --restore <backup>        # restore a chosen snapshot (workspace tier)
```
Backs up the current file first, then swaps the chosen snapshot into place atomically.
Use when repair can't recover enough — roll back to a known-good snapshot.

---

## Maintenance (instead of hand-editing)

### Compact — `base graph compact`
```
base graph compact
```
Dedups + canonicalizes the workspace graph via an atomic rewrite (snapshots first).
Idempotent — running it twice changes nothing. The safe replacement for a manual cleanup.
Refuses an unhealthy graph (run `base doctor --repair` first).

### Domain migration — `base graph migrate`
```
base graph migrate --dry-run          # what would be linked, and by which source
base graph migrate                    # snapshot, link, stamp, atomic rewrite
```
Gives every covered record a domain link, once per tier. **Normally you never run
this**: it runs itself at session start, and says nothing when there is nothing to
do. Run it by hand after `base doctor --repair`, because a migration skips an
unhealthy graph and `base doctor` will keep reporting `schema: not migrated`
until it succeeds.

Idempotent, and idempotent the hard way: the work is recomputed from the store
every time, and the schema stamp is written **inside the same atomic rewrite as
the links**. A stamp in a file beside the store would survive a `.bak` restore and
claim a migration that is no longer in the data — so restoring a pre-migration
snapshot correctly re-migrates, and restoring a post-migration one correctly does
nothing.

Where a record's domain comes from is reported per source, so the catchall
(`unfiled`) is one line among the others rather than a silent remainder:
```
base: domain migration — 2871 record(s) now carry a domain.
  by source: fixed 1434, parent 1241, path-trigger 510, unfiled 862, ...
  by kind:   LoreKnowledge 929, AcceptanceCriteria 375, Document 999, ...
```
`base doctor` reports each tier's `schema:` line and `without a domain: N` so a
migration that never ran is never invisible.

### Purge stale notes — `base graph purge --stale`
```
base graph purge --stale              # PREVIEW: list notes unread > 21 days (writes nothing)
base graph purge --stale --apply      # snapshot, then delete them
base graph purge --stale --days 30    # override the window
```
Usage-based note GC: a note is stale when it hasn't been recalled within the window
(default 21 days). `base recall` stamps a `lastRead` timestamp, so every recall renews a
note's clock — only notes you never reach for age out. **Dry-run by default**; `--apply`
is required to delete, and it always snapshots first. Removed notes are re-addable later
if they become relevant again.

---

## Backups

- **Naming:** `graph.nq.bak-<op>-<date>` (e.g. `graph.nq.bak-pre-repair-2026-06-19-140817`).
- **Rotation:** the newest 10 snapshots are kept; older ones are rotated out automatically.
- **Who snapshots:** repair, restore, compact, purge and migrate all snapshot before
  mutating. They share one pool of 10, so a migration snapshot evicts the oldest
  compact snapshot. A migration that finds nothing to do takes no snapshot, for
  exactly that reason.

---

*Part of v0.5 Graph Durability & Self-Heal. Full root-cause analysis and design rationale:
`GRAPH-DURABILITY.md`.*
