# Graph Durability & Self-Heal — Feature Spec

**Status:** Proposal — pre-`paul:plan`
**Author:** ZERO (COO session 2026-06-18)
**Trigger:** `graph.nq` corruption (3rd recurrence per operator); a session found the workspace graph unparseable, silently blocking all `recall`/`learn`/`sync`.

---

## 1. Incident summary (2026-06-18)

- Workspace `graph.nq` (21.4 MB, 84,118 lines) failed to parse. `learn`/`sync` returned `Failed to parse graph` — **but exit 0**, so failure was silent until a session tripped over empty recalls.
- **Exactly one malformed line**: the last (84118), truncated mid-string-literal, no terminator, no trailing newline.
- **Fix applied:** dropped the orphaned line (`head -n 84117`), backed up the corrupt file, re-ran `sync` → full recovery. `recall`/`learn`/`sync` verified working.

## 2. Root cause (corrected after code audit)

Initial hypothesis — "base writes non-atomically" — is **WRONG**. base-v2 already writes atomically:

- `store::write_back` (`src/store.rs:142–175`): serialize → temp `.nq.tmp` → **re-parse-validate** → `fs::rename`. A crash during base's own write leaves the live graph untouched.
- Every mutation path routes through `write_back`: `crud/mod.rs`, `crud/note.rs`, `domain/sync.rs`, `extract/mod.rs`, `hook/post_tool_use.rs`, `extract/paul_toml.rs`. No production path does a direct in-place write to `graph.nq`.

**Actual cause:** manual graph surgery by agents. The `.bak-pre-stale-purge-2026-06-16` and `.bak-pre-carl-recovery` backups were created by hand (cp), not by any code in base-v2. A prior session hand-edited `graph.nq` (shell/python rewrite during a "stale purge"), the non-atomic rewrite was interrupted, and the file was truncated.

**The disease:** agents bypass the binary and hand-edit the graph during maintenance ops. Plus two amplifiers that turned a one-line defect into a total outage:
1. **Parsing is all-or-nothing** (oxigraph `load_from_reader`, `src/store.rs:65–77`): one bad quad kills all 84k good ones.
2. **Failures exit 0** (`src/hook/mod.rs` `dispatch()`): errors are logged to stderr but `process::exit(1)` is never called, so corruption is silent.

## 3. Design principles

- **Doctor must never load through the strict parser.** It operates on the raw file (line-level/streaming), so it works *because* the graph is broken, not despite it.
- **No new hand-edit surface.** Every repair/maintenance op writes through `write_back` (atomic) or doctor's own atomic raw-file rewrite. Agents should never need to `sed`/`python` the graph again.
- **Fail loud.** Parse failure must be non-zero exit + actionable message pointing at `base doctor`.

## 4. Scope — three layers (approved 2026-06-18: all three)

### Layer 1 — Prevention: kill the need for manual surgery
The reason agents hand-edit is that maintenance ops aren't first-class commands. Make them so, all via `write_back`:
- `base graph compact` — dedup + canonicalize + rewrite (the safe version of "stale purge").
- `base graph purge --stale [--dry-run]` — remove stale entries by rule, atomic, auto-backup first.
- Both auto-snapshot to `graph.nq.bak-<op>-<date>` before mutating (move the cp-backup convention *into* the binary).
- Document in CLAUDE.md/base-section: **never hand-edit `graph.nq` — use `base graph *` / `base doctor`.**

### Layer 2 — Resilience: lenient parse
- Add `store::load_graph_lenient(path) -> (Store, Vec<BadLine>)`: stream line-by-line, skip+collect malformed quads instead of aborting. One bad line costs one triple, not the graph.
- Wire as fallback: strict load first (fast path); on failure, lenient load + warn with count of skipped lines + pointer to `base doctor --repair`.

### Layer 3 — Diagnosis & self-heal: `base doctor`
- `base doctor` (default = check + report): parse-independent health scan.
  - **Signals:** does it parse? malformed line numbers + previews; truncation check (final line terminator + trailing newline); file size & line count vs each available backup; entity-type composition (document/decision/note/rule/project/…) vs latest good backup to flag real data loss; which tier (workspace vs global) is affected; stale `.nq.tmp` present?
  - Exit nonzero if unhealthy.
- `base doctor --repair`: quarantine malformed lines to `graph.nq.quarantine`, keep the good set, atomic rewrite, re-verify. Backup first.
- `base doctor --restore <backup>`: restore from a chosen snapshot (lists them if omitted). Backup current first.
- `base doctor --json`: machine-readable signal for agents/hooks.

### Cross-cutting fixes
- **Exit codes:** `hook::dispatch` (`src/hook/mod.rs`) + `main()` (`src/main.rs:1–5`) propagate `process::exit(1)` on `Err`. Audit other swallow points.
- **SessionStart hook:** run `base doctor --check` (cheap, parse-only) at session start; warn loudly if the graph is sick — catch corruption at boot, not 3 prompts in.

## 5. Code anchors (from audit)

| Concern | Location |
|---|---|
| Strict parse / error string | `src/store.rs:65–77` (oxigraph 0.4, NQuads) |
| Atomic write (reuse for all repair) | `src/store.rs:142–175` `write_back` |
| Tier resolution (ws vs global) | `src/config.rs:6–16`, `src/store.rs:99–121` `load_merged` |
| CLI subcommand pattern (clap derive) | `src/cli.rs` — add `Commands::Doctor` + `DoctorAction`, dispatch in `run()` |
| Exit-0 bug | `src/hook/mod.rs` `dispatch()`, `src/main.rs:1–5` |
| SessionStart hook | `src/hook/session_start.rs` |

## 6. Proposed phasing (for `paul:plan`)

Recommend a **dedicated reliability milestone (v0.5 "Graph Durability")** that **preempts the in-flight v0.4 Steering Layer** — data fragility is existential; feature work shouldn't sit on top of a graph that can vanish.

1. **P1 — Exit codes + SessionStart check** (smallest, highest signal; stops silent failure immediately)
2. **P2 — `base doctor` check + report + `--json`** (parse-independent scan)
3. **P3 — Lenient parse + `base doctor --repair` / `--restore`** (self-heal)
4. **P4 — `base graph compact` / `purge` first-class ops + in-binary backups** (remove the hand-edit surface)
5. **P5 — Docs + `/base:doctor` thin slash wrapper** (front door; logic stays in binary)

## 7. Open decisions for operator

1. **Sequencing:** preempt v0.4 with this (recommended), or slot as v0.4 hotfix after current phase?
2. **Backup retention:** how many `graph.nq.bak-*` to keep before rotating out (disk: each ~20–28 MB)?
3. **Lenient parse default:** auto-fallback to lenient on strict failure (resilient, but masks corruption), or require explicit `base doctor --repair`? (Lean: warn + auto-fallback for *reads*, explicit for *writes*.)
4. **`/base:doctor` slash command:** build the thin wrapper now or after the binary lands?
