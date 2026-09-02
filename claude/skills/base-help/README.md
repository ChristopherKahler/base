# base-help: a Claude Code coaching skill for the base CLI

A self-contained Claude Code skill that turns `/base-help [question]` into a coach for the `base` CLI: it answers from a pre-verified knowledge bank instantly, teaches the underlying mechanic instead of just handing over a command, and audits the local install into a per-machine profile so it can coach toward adoption gaps (zero domain rules, never-used handoffs, and so on).

## What's inside

| File | Role |
| --- | --- |
| `SKILL.md` | The skill logic: local-profile audit, bank-first answer flow, coaching format, beginner orientation |
| `references/qa.md` | 179 question/answer pairs covering orientation, star commands, handoffs/forks, graph and memory, rules and domains, project scoping, ingestion, GraphRAG, AST navigation, hooks, relay, admin surfaces, known bugs, and destructive operations. Every pair carries a provenance tag |
| `references/commands.md` | The command surface grouped by what is safe to run: read-only, mutating, and destructive, plus alias and flag gotchas |
| `references/cli.md` | The verbatim `--help` of every subcommand, generated from the binary's own command tree at each release |

## Install

`base install` and `base update` install this skill for you. By hand:

```bash
cp -r claude/skills/base-help ~/.claude/skills/
```

Then type `/base-help` in any Claude Code session. On first use the skill audits the local install (read-only commands only) and writes a machine profile to `~/.claude/base-help/local/profile.md`; the skill files themselves contain no machine-specific state, so the same folder works on any machine.

## How answers stay fast and honest

- The skill greps `references/qa.md` first; live probing happens only on a bank miss, a machine-state question, or a version mismatch.
- Every reference file is stamped to the base release it shipped with, and the stamp is enforced rather than typed. `src/help_docs.rs` in the base repo fails `cargo test` when `cli.md` differs from what the binary renders, when the stamps in `qa.md` and `commands.md` lag `Cargo.toml`, when a shipped subcommand is missing from the bank, or when any `base ...` invocation in these files names a subcommand or flag the binary does not have. `scripts/release.sh` regenerates the generated parts on every release, so a release cannot ship a coach that describes the previous one.
- If the installed `base --version` differs from the stamp anyway (an old install that has not run `base update`), the skill treats bank answers as leads to re-verify rather than facts.
- A close-the-loop rule appends any newly researched answer back into `qa.md`, so every miss becomes a future hit.

## Verification provenance

The pairs were drafted against three sources (live `--help` output, the source tree, and a researched reference document), then merged, deduplicated, and adversarially fact-checked: a reviewer attempted to refute every mechanism claim against the source and corrected the pairs it could refute. Pairs tagged `source` or `cli-help` were confirmed directly; pairs tagged `reference` trace to the researched reference only. Each pair's `<!-- vX.Y.Z | verified: ... -->` tag records the release a person last checked its mechanism claims against. The release stamp at the top of each file is mechanical and never moves those tags.

## Maintenance

- New verified pairs go into the matching section of `references/qa.md`, same format, with a provenance comment.
- Never edit `references/cli.md` or the text between `<!-- stamp:begin -->` and `<!-- stamp:end -->` by hand: `BASE_REGEN_DOCS=1 cargo test --bin base help_docs` rewrites them.
- An invocation the bank shows because it does NOT work (`base rule l` is an error) goes in an `<!-- invalid-by-design: ... -->` comment in that file so the resolver skips it.
- On a new base release: re-verify the "Known bugs" section (each entry cites file and line). The version stamp itself moves with the release script.

<!-- invalid-by-design: the Maintenance example above is an error on purpose. `base rule l` -->
