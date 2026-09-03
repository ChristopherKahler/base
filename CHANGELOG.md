# Changelog

Every released version of base, newest first. Each entry is generated from the commits
between that release's tag and the one before it, by `scripts/changelog.py`.
`scripts/release.sh` writes the top section when it cuts a release, and `cargo test`
fails when the version in `Cargo.toml` has no entry here.

Releases before 0.13.3 are tagged in the repository but are not written up.

## 0.13.15 (2026-09-03)

### Fixed

- **base-help**: hold the coach to the binary — generated cli.md, enforced stamps, release script

### Changed

- src/help_docs.rs is a source file, not an executable

## 0.13.14 (2026-09-02)

*No user-visible changes.*

## 0.13.13 (2026-09-02)

*No user-visible changes.*

## 0.13.12 (2026-09-02)

*No user-visible changes.*

## 0.13.11 (2026-09-02)

### Added

- **automap**: workspace boots — hubs two levels deep, nested apps map themselves, Bash and wsl first contact, one build at a time

## 0.13.10 (2026-09-01)

### Added

- **automap**: no app goes without a code map — bare folders, read-only contact, big trees, missing hooks

## 0.13.9 (2026-09-01)

### Added

- **hooks**: every app gets a code map on first contact — session-start and Stop build it, never by hand

## 0.13.8 (2026-09-01)

### Fixed

- **hooks**: the Stop-hook AST refresh survives a cwd change — dirty marks get a global-tier copy

## 0.13.7 (2026-09-01)

### Fixed

- **ast**: match paths below the app root on Windows — normalise the stored sourceFile in SPARQL

## 0.13.6 (2026-09-01)

### Fixed

- **ast**: `ast query --file` is linear again — sourceFile pattern back inside the first BGP

## 0.13.5 (2026-09-01)

- **Skill backups move out of `~/.claude/skills`.** When an update replaced `base-help`, the previous copy was kept beside it as `base-help.bak-<timestamp>`, and Claude Code loaded that backup as a second, stale skill. Backups now live under `~/.base-gbl/backups/skills/`. If you already have a `*.bak-*` directory in `~/.claude/skills`, move or delete it.

Windows installs older than 0.13.4: see the v0.13.4 notes for the one-time bootstrap.

### Fixed

- **install**: park replaced skills under ~/.base-gbl/backups/skills, not inside ~/.claude/skills

## 0.13.4 (2026-09-01)

- **Self-update works on Windows.** The updater is `base.exe` itself, and Windows refuses to rename a file over a running image, so every hook-spawned update since 0.13.0 downloaded the release and failed the last step in silence. The running binary is now renamed aside (`base.exe.old`) and the new one renamed in; a side-by-side extensionless `base` (Git Bash) is refreshed too.
- **`~/.base-gbl/update.log`**: one line per swap or failure, background or manual. The silent path finally leaves a trace.

**One-time step for Windows installs older than 0.13.4:** the old binary cannot replace itself, so install this release by downloading `base-windows-x86_64.zip` below (or `npx chrisai`). From here on it updates in place at session start.

### Fixed

- **update**: swap a running Windows binary by renaming it aside, and leave a trace

## 0.13.3 (2026-09-01)

- **Relay wake contract** (#11, #13): the hook-injected block explains what the relay is, that everything it touches stays under `~/.base-gbl/.base/relay-inbox/`, where the instruction comes from and how to switch it off. It names the operator from `base operator init` (or says "the operator"). New `[relay]` section: `base config set relay.enabled false` / `base config set relay.wake_nudge false`.
- **Nudge throttle on Windows** (#13): the 180 s cooldown never engaged because a zero-byte write does not move mtime on Windows; it now sets the mtime explicitly.
- **`base scaffold` on Windows** (#13): the workspace path is written in a TOML-safe form instead of the verbatim `\\?\` path that broke `base config`.
- **Skills install** (#12): reports when `~/.claude/skills` is a symlink; `BASE_SKILLS_DIR` overrides the destination.
- **Domain sync** (#14, thanks @PulseCheckAI): `updatedAt` no longer accumulates one quad per sync.

**Windows note:** the silent self-update in this release cannot replace its own running binary; that is fixed in v0.13.4. Install this or later by downloading the zip or `npx chrisai`.

### Added

- **doctor**: record the coach's version and report when it lags
- **update**: refresh the bundled skill after a binary swap
- **sync**: ring a running app after a graph write (R7)
- **sync**: publish the hook command table as JSON (R4)
- **sync**: capture deltas at the SPARQL write sites behind a pairing gate (R1b)
- **sync**: carry the delta on the five fact-producing writes (R1a)
- **sync**: stamp origin on every changes.jsonl record
- **sync**: base graph apply-ops — apply inbound fact ops into the local graph
- **changelog**: append every successful graph write to changes.jsonl

### Fixed

- **relay,install,scaffold**: answer issues [#11](https://github.com/ChristopherKahler/base/issues/11), [#12](https://github.com/ChristopherKahler/base/issues/12) and [#13](https://github.com/ChristopherKahler/base/issues/13)
- **sync**: GC stale domain metadata before re-upsert
- **sync**: stop dying on backslashed relative paths in triple inserts
- **base-help**: import the v0.12.3 bank and correct it against 0.13.2
- **tier**: resolve --global directly instead of walking up into the workspace tier
- **sync**: let a first pull create graph.nq instead of refusing
- **plugin**: resolve manifest paths to one shape on Windows and unix
- **tests**: stop workspace resolution walking out of the sandbox
- **tests**: isolate cargo test from the real global graph
