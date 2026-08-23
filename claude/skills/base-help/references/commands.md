# base command reference (v0.11.0)

Verified against base v0.11.0 on 2026-08-13. If the installed version differs, re-check flags with `base help <sub>` before asserting.

## Read-only, safe to run anytime

```bash
base doctor                              # graph health across tiers; nonzero exit if unhealthy
base commands list                       # all star commands
base commands show <name>                # full rules text for one
base recall --keyword "<term>"           # search notes  (--domain, --slug)
base decision search --keyword "<term>"  # search past decisions (--json for machine output)
base handoff list                        # open handoffs (shows tier per entry)
base fork list                           # open forks
base ast list                            # which apps have a code map
base ast query -c "<name>"               # find entities by name
base ast query -f "<file>"               # entities in a file
base ast query --calls "<fn>"            # callers of a function
base ast query -i "<file>"               # importers of a file
base ast query -t apps/X -c "<name>"     # query another app's map
base context "<text>"                    # preview what would inject for that text
base context --list                      # all context triggers
base relay board                         # sessions, liveness, pending messages
base relay sessions                      # titled sessions (targets for *task/*ping)
base relay tasks                         # inbound relay tasks
base rule list --domain X                # rules for a domain (global tier: base rule -g list ...)
base project list                        # registered projects
base operator show                       # operator identity block
base changes --cursor                    # graph change log: current end offset (byte offset, not a seq)
base changes --since <offset>            # every graph write after that offset, as JSON (-g for global tier)
```

## Mutating: show, don't run

```bash
base handoff create --project "<p>" --doc "<abs-path>" [--slug "<title>"]
base fork    create --project "<p>" --doc "<abs-path>" [--slug "<title>"]
base handoff archive <slug> | snooze <slug> <days>     # fork has the same verbs
base learn --text "..." --domain X --type insight|correction|decision|commitment|shift
base decision log --domain X --decision "..." --rationale "..." [--recall]
base rule add --domain X --text "..."    # global tier: -g goes on `rule`, BEFORE the verb (base rule -g add ...)
base project add -n "..." -p "src/x" [-s <status>] [--stage <stage>]
base milestone add -p <project> -n "..." [-d "..."]
base task add -p <project> -n "..." [--priority <p>] [-m <milestone>]
base task done <slug>
base operator init --name "<name>"       # identity block at session start
base sync [--incremental] [--repair]     # markdown -> graph
base sync --ast [--target apps/X]        # code structure -> graph (per-app .base-ast/ sidecar)
base scaffold [path]                     # new workspace
base relay register --as <codename>      # title this session for relay
base config get|set <key> [value]        # notable keys: devmode.enabled, multimodal.enabled
base graph extract --target docs/        # LLM pass: markdown -> concepts + edges
base graph query "<question>" [--raw]    # GraphRAG retrieve + synthesize (or raw subgraph)
base graph analyze                       # god nodes, communities, bridges
base graph get-node "<label>" | neighbors "<node>" -d N | path "<from>" "<to>"
```

## Destructive: warn first, never run to demonstrate

```bash
base uninstall [--purge]
base memory purge
base decision delete --keyword "<k>"
base graph purge | compact | move
```

## Other surfaces (exist in v0.11.0; check --help before teaching)

`goal`, `reminder`, `entity`, `domain`, `standards` (std), `dashboard` (dash), `secret`, `workspace`, `reconcile`, `extension` (ext), `update`, `hook`, `install`, `activate`, `memory list`.

Never tell the user a capability does not exist without checking `base --help` first.
