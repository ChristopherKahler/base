# Changelog

Every released version of base, newest first. Each entry is generated from the commits
between that release's tag and the one before it, by `scripts/changelog.py`.
`scripts/release.sh` writes the top section when it cuts a release, and `cargo test`
fails when the version in `Cargo.toml` has no entry here.

Releases before 0.13.3 are tagged in the repository but are not written up.

## 0.14.1 (2026-09-07)

**Corrections now retire what they correct.** Until now a correction was stored beside the record it corrected, both came back from `recall` together, and nothing said which one was live — the drift that base exists to stop, sitting in base's own store. `base learn`, `base decision log` and `base rule add` take `--supersedes <slug>`, and `base graph supersede <old> <new>` records it for two records that already exist. The write refuses a slug that matches nothing, a slug that matches more than one record (naming every candidate rather than guessing), a record superseding itself, and any link that would close a cycle; every refusal happens before anything is written, so a refused correction leaves the store exactly as it was. The new record and both edges land in one write.

Surfaces that ANSWER a question now answer with the live version: `base recall`, `base context`, the prompt-time injection, `base rule list` and the domain block a session injects all serve the current record and not the one it replaced; `base rule list --include-superseded` shows the rest, each marked. Surfaces that DESCRIBE the graph keep everything: `graph analyze`, `neighbors`, `path`, `get-node`, the dashboard and the LOGOS export still show the superseded record and the edge that retired it, because that record is the evidence the correction happened — `graph get-node` marks it `[superseded]` and names the live head. `base recall --include-superseded` shows the whole chain. `graph purge --stale` never deletes either end of one. `base doctor` reports how many records are superseded, any chain longer than three links, any cycle, any disagreement between the edge and the `status` field, and how many corrections name nothing they correct.

Nothing is superseded until you say so, and a store that never uses the feature prints and serves exactly what it did before.

**A session start no longer rewrites the store to change nothing.** The PAUL ingest refreshed every scanned project's `updatedAt` on every run and wrote the whole graph back. That moved the store's identity, which re-opened the domain backfill's delta gate, which bought a full re-plan that was then discarded because there was nothing to plan. Measured on a 14 MB store: eighteen session starts, eighteen changelog entries carrying the same four timestamp quads, and no delta work to show for any of them. A project is re-stamped now only when something about it actually changed.

The cause was a duplicate scan, and it is fixed where it happened. A `paul.toml` reachable by two paths - a junction or a symlink sitting beside the real directory - was returned twice, so the ingest ran twice for one project and the two passes overwrote each other's recorded path on every run. One project is one entry now, whichever path reaches it first, and a duplicate is reported once when devmode is on.

**Domain and rule commands act on the tier you are standing in, and say which one.** `base domain remove`, `create` and `remove-trigger` wrote to the global `domains.toml` wherever you ran them, so a domain created inside a workspace could not be removed from it, `create` reported success for a domain it had written somewhere else, and `remove-trigger` reported success while editing a same-named global domain and leaving yours in place. One resolver now decides which file every write touches, every write reports what actually changed, and a command that changed nothing names the tier it searched and the one it did not, then exits non-zero instead of printing success. `base rule list` prints both tiers by default, labelled, each numbered the way `base rule remove --index` counts them, because the prompt-time block merges the two and renumbers them into an order no command used to print; `--global` keeps the single-tier view for scripts. `base domain list` and `base domain get` count both tiers as well. A tier whose graph file does not exist yet reads as empty instead of failing, so a fresh tier can take its first rule.

**Prompt-time injection is scoped to this session and driven by the prompt.** Until now a path trigger matched against every path the store had ever seen active — every registered project, task and handoff, 923 of them on one real store, the same list on every prompt — and matched by substring, so a trigger named `Documents` fired on prompt 3 of every session whatever was typed, and seven domain blocks reached a prompt that said `running a demo!`. Two of those blocks held live negotiation terms. A path trigger now matches only the files this session has touched through the tool hooks, plus the session's cwd, and matches by path prefix: a trigger resolves against the tier it came from (home for the global file, the workspace for the workspace file; `~` and absolute paths as written), compares component by component, and on a Windows path compares case-blind, so `Documents` never covers `MyDocuments` or `Documents-old` and `tools` and `Tools` are one directory. Keywords match whole words, so `base` no longer fires on `database`. An `exclude` now vetoes an always-on domain, which it silently failed to do. A context block whose only row is the domain's own project is not emitted. With devmode on, the reason line names the file that satisfied a path trigger.

**A domain can say it is never injected automatically.** `auto_inject = false` in `domains.toml` keeps a domain out of the prompt hook, the tool hook and the session-start cheat-sheet, whatever its mode or triggers; `base context`, `base recall` and star commands still see it, because you asked. The key is ignored by 0.14.0, and the 0.14.0 `add-trigger` round-trip drops keys it does not know, so install this release on every machine that shares the file before you set the flag, not after.

**A path trigger that covers two or more registered projects is a broadcast, and a broadcast cannot fire.** `Documents` on a home where nine projects live under Documents is not a trigger for any of them. Such a trigger is inert in both hooks; `base doctor` reports it per tier with the domain, the trigger and the projects it covers, and counts it against the verdict, because an inert trigger is a domain that silently stopped loading and the fix is one line in `domains.toml`; `base domain add-trigger --path` refuses one with the same sentence, exits non-zero and writes nothing; `base project add` under such a path registers the project, creates no domain, and says why. A project registered in both tiers counts once. Devmode prints one `inert:` line per faulty trigger on every prompt. An unrooted trigger — a glob, or a relative path in a file with no tier root — is inert for the same reason and named the same way.

**The dashboard no longer overwrites what you wrote from the CLI.** It loaded the graph once at startup and serialised that snapshot back over `graph.nq` on every write, so anything `base learn`, `base decision log` or `base rule add` had written since the server started was erased by the next click. Six of seven writes were lost in the session that reported it. Every dashboard read and write now compares each graph file's identity on disk with the one it was loaded from and reloads first when they differ, so a CLI write made while the dashboard is open survives. `base graph reload` stays as the manual escape hatch.

**A map that is too big to refresh unattended is not refreshed unattended.** The size fuse only ran when a map was built for the first time, so an existing map licensed a full refresh on every Stop hook however large the tree was, and the fuse counted source files by code extension, which meant a markdown-only workspace was never protected even on first contact. A 25,166-file docs tree re-indexed for ten minutes after every turn. Files the extractor turns into entities now count toward the fuse, markdown included; the fuse applies to refreshes as well as first builds, reading the counts recorded at the last measure rather than walking the tree again; and a refusal is written down and named once at session start with the command to run by hand.

**`hook-events.jsonl` is bounded by the thing that writes it.** The only trim lived in the dashboard's startup path, so an install that never opened the dashboard grew the file forever. The writer now trims to its last 5,000 lines when the file is over 10 MiB, before it appends, and the dashboard's rotation calls the same code.

**A hook that failed leaves a trace you can find.** Hooks fail open on purpose, so a broken one is silent by design and looks exactly like a quiet one. Failure lines now carry the error text, `base doctor` reports how many hooks failed in the recent window and what the last error was, and a session whose hooks are failing prints one line saying so. A hook whose most recent run failed is failing now and makes `doctor` exit non-zero; a failure with successes after it is reported as history and does not hold `doctor` red. A healthy install prints nothing new.

**Outside a workspace, every reader says so.** `recall` printed "No results found", `project list` exited 1 with an error, and `doctor` and `commands list` quietly answered from the global tier alone: four behaviours for one situation, none of which said the workspace was missing rather than empty. All four now print one line naming the tier they searched and how to make a workspace.

### Added

- **supersede**: drift has a mechanism, ops:supersedes gets a writer and every reader honours it ([#59](https://github.com/ChristopherKahler/base/issues/59))

### Fixed

- **hooks,dashboard**: [#41](https://github.com/ChristopherKahler/base/issues/41) [#40](https://github.com/ChristopherKahler/base/issues/40) [#22](https://github.com/ChristopherKahler/base/issues/22) [#20](https://github.com/ChristopherKahler/base/issues/20) [#19](https://github.com/ChristopherKahler/base/issues/19), the dashboard keeps CLI writes, the fuse guards refreshes, the hook log caps itself, doctor names a broken hook, readers name the tier ([#69](https://github.com/ChristopherKahler/base/issues/69))
- **injection**: the prompt hook injects what this session touched and what the prompt names, never a broadcast; auto_inject = false keeps a domain out of every automatic surface (F29, [#67](https://github.com/ChristopherKahler/base/issues/67))
- **tiers**: the tier cluster, [#52](https://github.com/ChristopherKahler/base/issues/52) [#53](https://github.com/ChristopherKahler/base/issues/53) [#55](https://github.com/ChristopherKahler/base/issues/55) [#18](https://github.com/ChristopherKahler/base/issues/18) [#60](https://github.com/ChristopherKahler/base/issues/60) are one seam, writes act on the tier you stand in and say what changed ([#61](https://github.com/ChristopherKahler/base/issues/61))
- **ingest**: the paul scan dedupes a project reachable by two paths, so an unchanged project is not rewritten every session ([#63](https://github.com/ChristopherKahler/base/issues/63))

### Changed

- **release**: the 0.14.1 note carries F29 ([#68](https://github.com/ChristopherKahler/base/issues/68))
- **release**: the 0.14.1 note carries F25 and names every serving surface ([#64](https://github.com/ChristopherKahler/base/issues/64))

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
