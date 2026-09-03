# How base is developed

Small team, mostly one person and Claude Code sessions. The process is the least that keeps a release honest.

## Bugs

A bug is a GitHub issue, filed with the bug form (it asks for the version, the host, what happens, how to reproduce, and a workaround). That is the only bug tracker; a bug noted in a handoff, a fork doc or a chat is not tracked until it is an issue.

Labels the pipeline reads:

| Label | Meaning |
| --- | --- |
| `bug` | applied by the form |
| `confirmed` | reproduced, or established from the source; it belongs on the Known issues page |
| `needs-info` | cannot be reproduced from what was given |
| `not-a-bug` | works as designed; say why in a comment |
| `fixed-in:<version>` | applied at release time to every issue closed since the previous tag |

## Changes

Every change is a branch and a pull request, even a one-line one. The PR title is a conventional commit (`feat(scope): …`, `fix(scope): …`, `chore: …`); the body says what changes for a user and why, and names the issue it closes (`Closes #N`) so the close and the Known issues page follow the merge. CI runs `cargo test` on the PR; merge when it is green, squash, and delete the branch.

`cargo test` includes the base-help coach gate. If the CLI changed, run `BASE_REGEN_DOCS=1 cargo test --bin base help_docs` and, for a new command, add its line to `claude/skills/base-help/references/commands.md` and a pair to `qa.md`. The gate names exactly what is missing.

## Releases

`scripts/release.sh <version> --push` from a clean `main`. It bumps the version, lets cargo rewrite the lock, regenerates the coach, runs the suite, commits, tags and pushes; the Release workflow builds the binaries after its own docs gate passes. Never edit `Cargo.lock` by hand and never tag by hand.

## Where things are discussed

Questions and how-do-I threads go to the community, not the issue tracker. Design decisions that outlive a PR are logged in the graph with `base decision log`.
