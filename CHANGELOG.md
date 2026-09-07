# Changelog

Every released version of base, newest first. Each entry is generated from the commits
between that release's tag and the one before it, by `scripts/changelog.py`.
`scripts/release.sh` writes the top section when it cuts a release, and `cargo test`
fails when the version in `Cargo.toml` has no entry here.

Releases before 0.13.3 are tagged in the repository but are not written up.

## 0.14.0 (2026-09-07)

Every record in the graph now carries a domain. The first session start on 0.14.0 backfills a link onto every record that lacks one, per tier, after snapshotting the store; on a 14 MB workspace store (60,000 quads) that pass took 1.1 seconds, once, and later session starts add about half a second only when the store changed since the last one. Later session starts file only what was written since, so records that `base sync` and the PAUL ingest create are filed at the next session start, and tasks, milestones and handoffs are filed the moment they are created. `base doctor` reports each tier's `schema:` line and anything still without a domain; `base graph migrate` runs the backfill by hand. Relay pings no longer appear in `graph analyze`, `recall`, prompt-time injection or the dashboard.

**Every install rewrites the BASE CLI section of `~/.claude/CLAUDE.md` once.** The section's text changed in this release, so the first session start on 0.14.0 refreshes it and prints one `[contract]` line; everything outside the section is left byte for byte as it was.

**`base activate` is retired.** It removed an attribution block that no longer exists. The command still parses and exits 0, so a script that runs it does not break. An install that had activated will see the update-available banner again when a newer release exists; `base update --snooze` quiets it for 24 hours.

Nothing base prints or writes names a company any more: the install and scaffold banners, the update banner, the generated `base.toml`, `domains.toml` and operator files, the CLAUDE.md section, `--help`, the release bodies and the docs site carry only the docs link and the author credit. `base install` and `base scaffold` print one short first-run message, and the first session start after an update prints one line saying what version is now running and where the changelog is.

**Prompt-time injection walks the graph from what you named.** Until now everything served at prompt time hung off a domain that a keyword selected, so naming a project surfaced nothing about its client or its files, and a topic with no configured keyword could never fire at all. Naming a thing now resolves it to a record and serves what hangs off it — decisions, rules, tasks in progress, notes, documents and entities — as one `<base-context>` block per named thing, each line carrying the relation that put it there. A project or a domain gets two hops; a single record gets one. Keyword triggers stay as the domain-level layer and this is the record-level layer beside them, so a domain with no keywords is now reachable through anything filed in it that you name. Quoting, backticking, capitalising, a path shape, or two or more words all read as naming something; a bare lowercase word never resolves on its own, so `base` mid-sentence stays a word. An ambiguous name resolves to the same record every time or is skipped, never guessed. Relay pings never appear. The walk reuses the store the hook has already parsed rather than reading it a second time — one graph load per prompt, and the per-app AST map is not read at all — and `injection.walk_budget` caps the block, counting what it drops rather than dropping it silently.

### Added

- **injection**: prompt-time injection walks the graph from what the prompt names ([#56](https://github.com/ChristopherKahler/base/issues/56))
- **domain**: every record carries a domain link, migrated in one idempotent pass ([#50](https://github.com/ChristopherKahler/base/issues/50))
- **install**: brand strip, one first-run message, and a session-start update notice ([#49](https://github.com/ChristopherKahler/base/issues/49))

### Fixed

- **release**: label-fixed-in windows on the previous release and always creates the label ([#58](https://github.com/ChristopherKahler/base/issues/58)) ([#57](https://github.com/ChristopherKahler/base/issues/57))

### Changed

- **release**: the 0.14.0 note and the licence notice without the company ([#51](https://github.com/ChristopherKahler/base/issues/51))

## 0.13.19 (2026-09-05)

A follow-up to 0.13.18's CLAUDE.md contract refresh. 0.13.18 marked the refresh done even when it had installed nothing (no CLAUDE.md, no `## BASE CLI` section, or two of them), so a user who fixed that later never received the current section. This release looks once more at the next session start: a current section is left alone, a missing or duplicated one is reported every session until it is resolved, and the refresh lands as soon as it can.

### Fixed

- **install**: stamp the CLAUDE.md refresh only once the text is on disk ([#48](https://github.com/ChristopherKahler/base/issues/48))

## 0.13.18 (2026-09-05)

This release makes the relational graph commands read the edges base writes for itself, makes `sync --repair` actually write the repairs it prints, and refreshes the installed CLAUDE.md contract once per version at session start, so an agent on an old install stops running an old contract.

**Windows installs older than 0.13.4 cannot receive this or any update.** That version fixed self-update on Windows, and the fix ships inside the update those installs cannot apply; `base update` on them prints "Run: base update" and cannot succeed. Bootstrap once with the release zip or `npx chrisai`, after which base updates itself in place.

### Fixed

- **update**: refresh the installed CLAUDE.md contract once per version ([#47](https://github.com/ChristopherKahler/base/issues/47)) ([#45](https://github.com/ChristopherKahler/base/issues/45))
- **sync**: --repair writes the repairs it prints, one statement at a time ([#46](https://github.com/ChristopherKahler/base/issues/46)) ([#44](https://github.com/ChristopherKahler/base/issues/44))
- **graph**: read the edges base writes for itself, and resolve names the same way every run ([#43](https://github.com/ChristopherKahler/base/issues/43)) ([#42](https://github.com/ChristopherKahler/base/issues/42))

## 0.13.17 (2026-09-04)

### Fixed

- **domain**: count the rules a domain actually injects, not just the file's ([#38](https://github.com/ChristopherKahler/base/issues/38))
- **triage**: read reports with the Claude Code CLI, polled from a machine
- **rule**: the listing counts up too, and a test that fails on the old code ([#30](https://github.com/ChristopherKahler/base/issues/30))
- **rule**: compare rule numbers as integers, not text

## 0.13.16 (2026-09-04)

### Added

- **automap**: a size fuse before any unattended build
- **triage**: vet bug reports and label the release that fixed them ([#24](https://github.com/ChristopherKahler/base/issues/24))
- **release**: write the changelog entry at release time, and refuse a release without one ([#23](https://github.com/ChristopherKahler/base/issues/23))

### Fixed

- **automap**: the sandbox exemption covers the segments the sandbox itself sits under
- **hook**: write the sync time into .domain-sync-ts so the guard closes on NTFS
- **store**: buffer the write_back dump, propagate the flush, and count the quads it wrote
- **automap**: key the temp exemption on the sandbox, never on a feature flag
- **automap**: a shell `cd` never adopts an unmarked folder
- **automap**: never map temp, caches, AppData, node_modules or the other OS's home
- **release**: give the rehearsal clone a git identity
- **release**: write the changelog before regenerating the coach ([#27](https://github.com/ChristopherKahler/base/issues/27))
- **clippy**: the 26 lints rustc 1.98.1 raises, so CI's clippy job is green again
- **changelog**: accept --date and --notes-dir after the subcommand ([#25](https://github.com/ChristopherKahler/base/issues/25))

### Changed

- **changelog**: CHANGELOG.md, generated from the tags by scripts/changelog.py ([#17](https://github.com/ChristopherKahler/base/issues/17))
- CI on every PR, a bug issue form, triage labels, and the process in CONTRIBUTING.md ([#16](https://github.com/ChristopherKahler/base/issues/16))

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
