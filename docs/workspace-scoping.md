---
type: spec
status: planned
tags: [base-v2, workspace-scoping, project-list, active-awareness, named-graph, write-routing, peer-workspace]
relatedTo: [base-v2, graph-durability, command-plugins]
---

# Workspace-Scoped Working Set (Workspace Scoping milestone)

**Status:** planned (open questions resolved 2026-06-24, entering PAUL plan) · **Target milestone:** Workspace Scoping (NEW milestone, Phases 45+) — *not* the queued v0.4 Steering Layer (Phases 26–32 = CARL behavioral steering, unrelated; the original label was a misnomer)
**Discovered:** 2026-06-24, in `~/ops-sys/extendly` during a `picking back up` resume.

## Problem

base has no current-workspace default. Open Claude anywhere and `base project list`
plus the session-start `[Active Projects]` signal return the **same flat union of every
registered workspace's projects**. There is no "the work that belongs *here*" view.

Concretely: a resume in `/home/chriskahler/ops-sys/extendly` surfaced `operator-kit-47`
(a chris-ai-systems app) as "where we left off," because it was the most-recent row in
the global union. The operator is in the Extendly workspace; the resume pointed at
another business entirely.

The operator's requirement, verbatim:

> default first to workspace based work so that we can stay targeted by workspaces
> without losing the cross awareness of other workspaces.

## Evidence (gathered 2026-06-24)

1. **Every project carries its true home as `#path`.** In the extendly tier graph,
   `operator-kit-47` is stamped:
   ```
   <#project/operator-kit-47> <#path> "/home/chriskahler/chris-ai-systems/apps/operator-kit-47" <#graph/ws/extendly> .
   ```
   The `#path` says chris-ai-systems; the **named graph** (4th quad) says `ws/extendly`.
   The named-graph stamp is wrong — it reflects the CWD at scan time, not the project's home.

2. **All 176 triples in `~/ops-sys/extendly/.base/graph.nq` sit in one named graph,
   `graph/ws/extendly`** — and every project inside physically lives under
   `~/chris-ai-systems/apps/...`. Extendly has **zero** base projects whose `#path` is
   actually under `~/ops-sys/extendly`.

3. **`base project list` has no scoping flag** (only `-h`). Same nine projects returned
   from `~/ops-sys/extendly` and from `~` (home) — confirming the result is CWD-independent;
   the list is a single global working set.

4. **Contamination vector:** the global `[[workspace]]` registry in `~/.base-gbl/base.toml`
   lists all 8 workspaces. At session-start, `active_awareness` + the `[protocol]`
   active⇄deferred reconcile scan **all** registered workspaces for projects and write the
   union into **whatever tier is current** — stamping foreign projects into the CWD
   workspace's named graph. Cleaning the graph by hand is futile: next session-start
   re-pollutes it.

## Root cause

Two coupled defects:

- **No scope resolution at read time.** `project list` / `active_awareness` have one mode:
  global union. There is no notion of "filter to the current workspace."
- **Write routing ignores the project's home.** The registry scan stamps every discovered
  project into the CWD's named graph instead of the named graph matching the project's
  `#path`. This is what makes the union look local everywhere.

The fix must address both — read-time scoping alone would still read a contaminated graph;
write-routing alone wouldn't give a default-local view.

## Operator context (2026-06-24)

Extendly's graph was **intentionally** never built out. base has been in active development
with features cycling through pipelines, so the operator dogfooded the graph on
chris-ai-systems only. Much workspace context is known-stale and is being cleaned as the new
ecosystem is built; a **full workspace reset/wipe is planned** to redial everything once base
stabilizes.

Implication for this spec: the contaminated current data is largely throwaway. The durable
value is **forward-correct read scoping + write routing** so the post-reset ecosystem stays
clean by construction. A one-time migration of today's data (§E) is therefore low priority —
the reset supersedes it.

## What already exists to build on

- **Named-graph-per-workspace** is already the storage model (`<#graph/ws/{name}>` quads).
- **Env contract** `BASE_WORKSPACE` / `BASE_GRAPH_PATH` / `BASE_GLOBAL_DIR` is already passed
  to plugins (v0.6 sole-writer decision, 2026-06-22). base already computes a current
  workspace; it just isn't used to scope reads or route writes.
- **`#path` literal** on every project is a reliable home-workspace signal — derive home by
  longest-prefix match of `#path` against the `[[workspace]]` registry paths.

## Requirements

1. **Default = current workspace.** From inside a registered workspace, `base project list`
   and session-start `[Active Projects]` return only projects whose home workspace is the
   current one (home derived from `#path`, not the named-graph stamp).
2. **Cross-awareness preserved, one step away.** A flag (`--all`) returns the global union;
   `--workspace <name>` targets another workspace. Session-start may show a compact
   `elsewhere: N active across M workspaces` line so peers stay visible without dominating.
3. **No silent loss.** A project whose `#path` is under no registered workspace, or that has
   no `#path` at all, still appears under an explicit `unscoped` bucket — never dropped.
4. **Idempotent hygiene.** Re-running session-start must not re-contaminate; write routing
   sends each project's triples to its home named graph regardless of CWD.
5. **Backward compatible.** Existing global behavior remains reachable; `scope = "global"`
   restores today's flat-union default for operators who prefer it.

## Proposed design

### A. Scope resolution
Resolve `current_workspace` from CWD by longest-prefix match against `[[workspace]]` paths
(reuse whatever already sets `BASE_WORKSPACE`). When CWD is under no registered workspace,
`current_workspace = none` → behavior falls back to global.

### B. Read-time scoping
`base project list` (and the `active_awareness` signal) filter by **home workspace derived
from `#path`**, not by named graph:
- `home(project) = longest-prefix match of project.#path against registry paths`
- default view = `home(project) == current_workspace` **OR** a `peerWorkspace` edge on the project includes `current_workspace`
- `--all` = no filter (today's union); `--workspace <name>` = `home == name OR peer-includes name`
- `--unscoped` = projects with no `#path` or no registry match

**Scope surface = full working set, not just projects (decision 2026-06-24).** The
`active_awareness` signal and `[Pick up where you left off]` scope handoffs + recent
memory/decisions by tier/home the same way, so a project-less workspace (e.g. Extendly)
still resumes from its tier-local handoffs and notes instead of an empty list. Nested repos
become visible only via a deliberate `base project add` — no auto-discovery.

Deriving home from `#path` (not the named-graph stamp) means scoping is correct even before
the data is re-slotted — it tolerates the existing contamination.

### C. Write routing (stop the re-pollution)
The registry scan and reconcile must write each project's triples into
`<#graph/ws/{home(project)}>`, where home is derived from `#path` — not `<#graph/ws/{cwd}>`.
A project discovered while CWD=extendly but living in chris-ai-systems lands in
`ws/chris-ai-systems`.

### D. Config knob
```toml
[signal]
scope = "workspace"   # "workspace" (default) | "global"
```
Optionally `[project] default_scope` to control the `project list` default independently of
the session-start signal.

### E. One-time migration (LOW PRIORITY — superseded by planned reset)
`base graph reslot` (atomic, snapshot-first per GRAPH-DURABILITY): walk every project triple,
recompute home from `#path`, move triples whose named graph ≠ home into the correct
`ws/*` graph. Report moved/kept/unscoped counts.
Deprioritized: the operator plans a full workspace reset, so cleaning *today's* contamination
has little payoff. Build C (write-routing) and B (read-scoping) so the reset ecosystem is
clean from the start; `reslot` is only worth it if the reset slips and the contamination
keeps causing misreads in the meantime.

## Resolved decisions (2026-06-24)

All three open questions were resolved with the operator before planning. Logged to the graph
(domain `development`).

- **Multi-home / shared projects → WIRE peerWorkspace now.** Single canonical home per project
  (longest-prefix `#path`), PLUS an additive `peerWorkspace` edge and a `base project peer
  <slug> --workspace <name>` command, shipped this phase. Read filter becomes `home ==
  current OR peer-includes current`. Operator chose to build the seam now (over the lazy-defer
  option) so shared projects expected post-reset surface in both workspaces immediately. The
  home graph stays the single source of truth; the edge is a visibility pointer, not a duplicate.
- **Sub-repo workspaces → scope the FULL working set, no auto-register.** Scoping operates on
  projects + handoffs + memory/decisions by home/tier — not Projects-only — so a project-less
  workspace resumes meaningfully from tier-local handoffs/notes. Nested repos (`rev-ops/`,
  `decks/`) register only via a deliberate `base project add`; no auto-discovery (rejected as a
  discovery explosion).
- **`unscoped` ergonomics → bucket + warn + reslot count.** Keep the explicit `unscoped` bucket
  (Req 3, no silent loss), emit a one-line backfill nudge when a project has no `#path`
  (`base project update <slug> --path …`), and have any `reslot` run report moved/kept/unscoped
  counts. A missing `#path` is an anomaly (`project add` mandates `--path`, E2) worth surfacing.

## Acceptance

- From `~/ops-sys/extendly`, `base project list` shows only true Extendly-homed projects
  (today: none → an explicit empty/`unscoped` state, not chris-ai-systems apps).
- The session-start resume from `~/ops-sys/extendly` is workspace-true: it surfaces Extendly's
  tier-local handoffs + recent memory (full-working-set scope), not chris-ai-systems apps, and
  not an empty list.
- `base project list --all` reproduces today's union; `--workspace <name>` targets another;
  a project with a `peerWorkspace` edge to the current workspace appears in the default view.
- `base project peer <slug> --workspace <name>` adds the edge; the project then surfaces in
  both its home workspace and the named peer.
- Session-start `[Active Projects]` is workspace-scoped, with a one-line cross-workspace
  pointer.
- Write-routing: a session-start in any CWD lands each project's triples in its `#path`-derived
  home graph; re-running session-start does not re-contaminate (idempotent). (`reslot` migration
  is optional — deprioritized in favor of the planned reset.)
