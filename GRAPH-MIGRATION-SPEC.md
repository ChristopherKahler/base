---
ontology: true
type: spec
domain: base-v2
status: built
summary: Build base graph move (subgraph transfer between named graphs) + base project move (end-to-end project re-home) — autonomous spec→build→tests-green
tags: [base-v2, graph, migration, cross-workspace, cli, spec]
related: [scope, graph-durability]
---

> **BUILT 2026-06-25.** `src/graph_move.rs` (primitive) + `crud::project::move_project` (composition) + CLI (`base graph move`, `base project move`, preview unless `--yes`). Tests green: 9 `graph_move` unit tests in-module, `tests/graph_move_test.rs` (6), `tests/project_move_test.rs` (3), full suite 0 regressions. Dogfooded on a COPY of the live 112k-line `chris-ai-systems` graph: `prefix:base-v2 --no-ast` moved 4049 lines (129 subjects), rewrote every stamp `graph/ws/chris-ai-systems → graph/ws/toolbox` (0 mis-stamped), left only the 5-line `codemap/base-v2` AST pointer (correctly excluded) + 7 dangling-incoming referencers, idempotent on re-run, both tiers backed up.
>
> **Two known limitations (see Open questions):** (1) the LIVE `--from chris-ai-systems` is ambiguous — two registered workspaces slugify identically AND share the `graph/ws/chris-ai-systems` named-graph IRI, so a name can't disambiguate them; resolver safely errors rather than guessing. (2) `--no-ast` *excludes* the source AST pointer from the move (leaves it) rather than *deleting* it; "drop source AST" was left as a deliberate no-delete choice. Neither blocks the primitive; both are flagged for a follow-up.

# Graph Migration — `base graph move` + `base project move`

## Pick this up

Build cross-workspace graph migration into base. base-v2 lives at `ops-sys/toolbox/frameworks/00-kit-base/` (toolbox workspace). The goal: two commands that move a node/subgraph — or a whole project — from one workspace graph to another, **correctly and atomically**. Run this autonomously: implement → `cargo test` green → manual dogfood verify. This spec is the contract; build to its Definition of Done.

## Why (proof of need — already dogfooded)

In the session that produced this spec, base-v2's own knowledge + tasks were hand-migrated from the `chris-ai-systems` graph to the `toolbox` graph (grep-extract → sed-rewrite → append → grep-remove → doctor). It worked but was manual surgery across two `.nq` files. This feature codifies that golden path and removes the foot-guns. The single command `base project move base-v2 --to toolbox` should have replaced an entire session of `repath` + `project add` + hand-editing for skillsmith, seed, and base-v2.

## Deliverables

### 1. Primitive — `base graph move`
```
base graph move --select <node-iri | domain:<name> | prefix:<str>> --to <workspace> [--from <workspace>] [--dry-run] [--no-ast] [--yes]
```
Transfers the selected subgraph from the source workspace graph to the destination workspace graph.

### 2. High-level — `base project move`
```
base project move <slug> --to <workspace> [--dry-run] [--no-ast]
```
Re-homes a project end-to-end: project node + tasks (`hasTask` edges + task nodes) + domain node + its decisions/rules/notes. AST is **regenerated** at the destination, not copied. Built on the `graph move` primitive.

## Hard requirements (non-negotiable — these are the traps)

1. **Named-graph rewrite (THE correctness property).** Every quad's 4th element is `<…#graph/ws/<workspace>>`. On transfer it MUST be rewritten source→dest. Skip it and the lines land present-but-invisible: `base recall` returns nothing, no error — a silent failure. This is the #1 thing tests must lock.
2. **Atomicity.** A move = backup-both → write-to-dest → remove-from-source → doctor-both. Never leave a duplicate (forgot remove) or an orphan (forgot rewrite). On any failure, roll back both graphs from the pre-move backup. Snapshot via the existing `store.rs` rotation.
3. **AST is regenerated, not moved.** `--no-ast` excludes `*.base-ast`-derived entities. `base project move` defaults to dropping source AST and re-syncing at the destination (the entities rebuild from the code). Don't copy thousands of stale AST triples.
4. **Subgraph selection.** `--select` resolves a node IRI, a `domain:<name>`, or a `prefix:<str>`; collects the node's own triples + outgoing edges. Incoming edges (other nodes referencing the moved node) follow the dangling-edge policy below.
5. **Dry-run.** `--dry-run` prints the move plan (line counts by kind, sample IRIs, source→dest graph) and mutates nothing.
6. **clap parity.** New subcommands match the clap-exact help/usage/error engine byte-for-byte (see the base-v2 CLI rule). Diff every `--help`/`-h`/error surface against the reference.

## Behavior spec (the golden path the feature codifies)

For `base graph move --select domain:X --to B --from A`:
1. Resolve source graph (`A/.base/graph.nq`) + dest graph (`B/.base/graph.nq`) via `scope.rs`.
2. Backup both (`store.rs` snapshot).
3. Select the subgraph lines from A (node + outgoing edges; honor `--no-ast`).
4. Rewrite each line's 4th quad: `graph/ws/A` → `graph/ws/B`.
5. Append rewritten lines to B; remove the selected lines from A.
6. `base doctor` both tiers → must be HEALTHY; else roll back.
7. Optionally `graph compact` the destination.

## Definition of Done (validated = all green)

**Unit**
- `quad_rewrite`: rewrites only the 4th element, preserves S/P/O exactly, idempotent on re-run.
- `subgraph_select`: node / `domain:` / `prefix:` selection returns the correct line set; `--no-ast` filters AST entities.

**Integration** (`tests/graph_move_test.rs`, `tests/project_move_test.rs`, on `tests/fixtures/`)
- Move `domain:X` from A→B: `recall` in B finds it, A returns 0, `doctor` both HEALTHY.
- **Idempotent**: re-running the move is a no-op (nothing left in A to move).
- **dry-run**: zero mutation; preview counts match the real move.
- **--no-ast**: AST entities excluded; `project move` regenerates the dest AST map.
- **Atomic rollback**: inject a failure after write-to-dest; both graphs restore to pre-move state from backup.
- **project move**: node + tasks + domain + decisions/rules/notes all land in B and attach to B's project node; A is clean of them.
- **clap parity**: `--help`/`-h`/error diff byte-for-byte vs reference (reuse the existing clap-exact test pattern).
- **No regressions**: full `cargo test` stays green.

## Key files

| File | Role |
|------|------|
| `src/graph_tools.rs` | add the `move` subcommand next to compact/purge/get-node/… |
| `src/graph.rs` | named-graph read/write + the quad-rewrite primitive |
| `src/store.rs` | backup/snapshot (reuse rotation; note the mtime-tie bug task) |
| `src/crud/project.rs` | `base project move` — compose graph-move + project re-home |
| `src/scope.rs` | resolve source/dest workspace graph paths (read-side companion to this write side) |
| `src/doctor.rs` | post-move health gate |
| `src/cli.rs`, `src/command.rs` | clap wiring + help parity |
| `tests/graph_move_test.rs`, `tests/project_move_test.rs` | new; mirror `crud_project_test.rs` style |

## Manual dogfood verify (after tests green)

Re-run the real case this spec came from: the `chris-ai-systems` graph still holds base-v2's **stale AST residual** (~4,121 lines) + a duplicate `project/base-v2` node. `base graph move --select prefix:base-v2 --from chris-ai-systems --to toolbox --no-ast` (or a dedicated cleanup) should sweep it, leaving `chris-ai-systems` free of base-v2 and `doctor` HEALTHY — the live acceptance test.

## Open questions (decide during build)

- **Dangling incoming edges** — leave dangling (default, log count), drop, or pull the referencing node? Recommend: leave + log.
- **project move** — auto-create the dest project node if absent, or require it exists first?
- **Registry** — should `project move` update `base.toml` workspace registry / peerWorkspace edges?
- **Confirm** — `--yes` to skip the interactive confirm on the destructive remove-from-source.

## Reference — manual procedure used (the executable golden path)
```
cp A/.base/graph.nq A/...bak ; cp B/.base/graph.nq B/...bak          # backup both
grep -E '<select-pattern>' A/.base/graph.nq > sub.nq                  # select subgraph
sed 's|#graph/ws/A> \.$|#graph/ws/B> .|' sub.nq > sub-B.nq            # rewrite 4th quad
cat sub-B.nq >> B/.base/graph.nq                                      # append to dest
grep -vE '<select-pattern>' A/.base/graph.nq > A.tmp && mv A.tmp …    # remove from source
base doctor   # both tiers HEALTHY ; base recall in B finds it
```
