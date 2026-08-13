# base-help: a Claude Code coaching skill for the base CLI

A self-contained Claude Code skill that turns `/base-help [question]` into a coach for the `base` CLI: it answers from a pre-verified knowledge bank instantly, teaches the underlying mechanic instead of just handing over a command, and audits the local install into a per-machine profile so it can coach toward adoption gaps (zero domain rules, never-used handoffs, and so on).

## What's inside

| File | Role |
| --- | --- |
| `SKILL.md` | The skill logic: local-profile audit, bank-first answer flow, coaching format, beginner orientation |
| `references/qa.md` | 153 question/answer pairs covering orientation, star commands, handoffs/forks, graph and memory, rules and domains, project scoping, ingestion, GraphRAG, AST navigation, hooks, relay, admin surfaces, known v0.11.0 bugs, and destructive operations. Every pair carries a provenance tag |
| `references/commands.md` | Exact command syntax: read-only, mutating, and destructive surfaces, plus flag gotchas |

## Install

```bash
cp -r claude/skills/base-help ~/.claude/skills/
```

Then type `/base-help` in any Claude Code session. On first use the skill audits the local install (read-only commands only) and writes a machine profile to `~/.claude/base-help/local/profile.md`; the skill files themselves contain no machine-specific state, so the same folder works on any machine.

## How answers stay fast and honest

- The skill greps `references/qa.md` first; live probing happens only on a bank miss, a machine-state question, or a version mismatch.
- The bank is stamped **verified against base v0.11.0 (2026-08-13)**. If the installed version differs, the skill treats bank answers as leads to re-verify rather than facts.
- A close-the-loop rule appends any newly researched answer back into `qa.md`, so every miss becomes a future hit.

## Verification provenance

The pairs were drafted against three sources (live `--help` output, the v0.11.0 source tree, and a researched reference document), then merged, deduplicated, and adversarially fact-checked: a reviewer attempted to refute every mechanism claim against the source and corrected the pairs it could refute. Pairs tagged `source` or `cli-help` were confirmed directly; pairs tagged `reference` trace to the researched reference only.

## Maintenance

- New verified pairs go into the matching section of `references/qa.md`, same format, with a provenance comment.
- On a new base release: re-verify the "Known bugs" section first (each entry cites file and line), then re-stamp the header.
