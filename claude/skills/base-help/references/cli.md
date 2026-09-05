# base CLI reference (v0.13.19)

Generated from this release's own command tree: the verbatim `--help` of every subcommand, exactly as the binary prints it. `src/help_docs.rs` in the base repo regenerates this file on every release and fails `cargo test` when it is behind the code, so when the version above matches `base --version`, every flag here is real. Do not edit by hand; regenerate with `BASE_REGEN_DOCS=1 cargo test --bin base help_docs`.

Use it for exact syntax. `commands.md` groups the same surface by what is safe to run, and `qa.md` explains the mechanics.

## Index

- `base`
- `base hook`
- `base hooks`
- `base hooks manifest`
- `base ast` (alias: a)
- `base ast query` (alias: q)
- `base ast list` (alias: l)
- `base ast ensure`
- `base project` (alias: p)
- `base project add` (alias: a)
- `base project list` (alias: l)
- `base project get`
- `base project peer`
- `base project repath`
- `base project update` (alias: u)
- `base project move`
- `base project delete`
- `base milestone` (alias: m)
- `base milestone add` (alias: a)
- `base milestone list` (alias: l)
- `base milestone get`
- `base milestone update` (alias: u)
- `base milestone delete`
- `base task` (alias: t)
- `base task add` (alias: a)
- `base task list` (alias: l)
- `base task get`
- `base task update` (alias: u)
- `base task delete`
- `base task done`
- `base task tag`
- `base decision` (alias: d)
- `base decision log`
- `base decision search`
- `base decision delete`
- `base decision update` (alias: u)
- `base entity` (alias: e)
- `base entity add`
- `base entity list`
- `base entity get`
- `base entity update`
- `base goal` (alias: g)
- `base goal add`
- `base goal list`
- `base goal update`
- `base reminder` (alias: r)
- `base reminder add`
- `base reminder list`
- `base reminder remove`
- `base handoff`
- `base handoff create`
- `base handoff list`
- `base handoff snooze`
- `base handoff archive`
- `base fork`
- `base fork create`
- `base fork list`
- `base fork snooze`
- `base fork archive`
- `base sync`
- `base domain`
- `base domain add-trigger`
- `base domain list`
- `base domain get`
- `base domain sync`
- `base domain create`
- `base domain remove`
- `base domain remove-trigger`
- `base standards` (alias: std)
- `base standards sync`
- `base standards list`
- `base standards get`
- `base standards test`
- `base relay`
- `base relay init`
- `base relay register`
- `base relay send`
- `base relay poll`
- `base relay wait`
- `base relay claim`
- `base relay release`
- `base relay board`
- `base relay export`
- `base relay dispose`
- `base relay task`
- `base relay ping`
- `base relay done`
- `base relay tasks`
- `base relay sessions`
- `base learn`
- `base recall`
- `base changes`
- `base rule`
- `base rule add`
- `base rule list`
- `base rule remove`
- `base install`
- `base activate`
- `base update`
- `base uninstall`
- `base dashboard` (alias: dash)
- `base scaffold`
- `base reconcile`
- `base workspace`
- `base workspace sync`
- `base operator`
- `base operator init`
- `base operator show`
- `base extension` (alias: ext)
- `base extension list`
- `base extension validate`
- `base extension install`
- `base extension add`
- `base extension scaffold`
- `base extension remove`
- `base extension run`
- `base commands` (alias: cmd)
- `base commands list`
- `base commands show`
- `base commands add`
- `base commands remove`
- `base commands import`
- `base memory`
- `base memory list`
- `base memory purge`
- `base config`
- `base config get`
- `base config set`
- `base config list`
- `base context`
- `base doctor`
- `base graph`
- `base graph compact`
- `base graph apply-ops`
- `base graph purge`
- `base graph extract`
- `base graph query`
- `base graph analyze`
- `base graph get-node`
- `base graph neighbors`
- `base graph path`
- `base graph move`
- `base secret`
- `base secret set`
- `base secret list`
- `base secret rm`

## base

```text
BASE — Proactive context-injection engine for Claude Code

Usage: base [COMMAND]

Commands:
  hook       Handle Claude Code hook events (session-start, post-tool-use, user-prompt-submit)
  hooks      Publish base's hook wiring for an external installer (JSON)
  ast        Query AST codebase graph (entities, calls, imports) [aliases: a]
  project    Manage projects [aliases: p]
  milestone  Manage milestones (epics within a project) [aliases: m]
  task       Manage tasks [aliases: t]
  decision   Log and search decisions [aliases: d]
  entity     Manage entities (people, organizations) [aliases: e]
  goal       Manage goals [aliases: g]
  reminder   Manage reminders [aliases: r]
  handoff    Manage session handoffs (resume docs surfaced at session start)
  fork       Manage parallel side-work forks (build-specs surfaced at session start)
  sync       Sync file-owned data into the graph
  domain     Manage domain matching rules
  standards  Manage context-triggered standards (MIDAS protocols injected on edit) [aliases: std]
  relay      Session-to-session message relay (parallel PAUL workers, Cadre firm members)
  learn      Graph-backed structured memory
  recall     Search notes by keyword, domain, or slug
  changes    Read the graph change log — every successful graph write, as JSON
  rule       Manage rules in the graph (add, list, remove)
  install    Install base globally: build, symlink, create ~/.base-gbl, wire hooks, write manifest
  activate   Activate ChrisAI — enter your Skool classroom key to remove attribution
  update     Self-update the base binary from public GitHub releases (or snooze the banner)
  uninstall  Uninstall base: remove hooks from settings.json, remove binary, remove CLAUDE.md section
  dashboard  Launch the Command Center Dashboard (local web UI) [aliases: dash]
  scaffold   Scaffold a new workspace: create .base/, write configs, register globally
  reconcile  Reconcile project active/deferred state from real folder last-touch
  workspace  Registered-workspace registry (sync CLAUDE.md from base.toml)
  operator   Operator identity profile (init, show)
  extension  Manage extensions (list, validate, install, remove) [aliases: ext]
  commands   List and inspect star commands (*BLUNT, *AUDIT, etc.) [aliases: cmd]
  memory     Manage graph-backed memory (migrate flat files, purge)
  config     Read and write base.toml configuration (dot-notation: section.key)
  context    Pull targeted graph context on demand (same engine as hook injection)
  doctor     Diagnose graph health across tiers (parser-independent). Exits nonzero when unhealthy
  graph      First-class graph maintenance (atomic, backs up first — never hand-edit graph.nq)
  secret     Securely manage API keys / secrets in ~/.base-gbl/.env (echo-off, 0600). Plugins read these from their environment — never type secrets into chat
  help       Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help

  -V, --version
          Print version

Drop-in plugin commands (from extensions): run `base ext list`

Built by Chris Kahler · Chris AI Systems
Community & support: https://www.skool.com/claude-code-titans-9203
Tutorials: https://www.youtube.com/@chris-ai-systems
```

## base hook

```text
Handle Claude Code hook events (session-start, post-tool-use, user-prompt-submit)

Usage: base hook <EVENT>

Arguments:
  <EVENT>
          Hook event type

Options:
  -h, --help
          Print help
```

## base hooks

```text
Publish base's hook wiring for an external installer (JSON)

Usage: base hooks <COMMAND>

Commands:
  manifest  Print the hook command table as JSON, for an installer outside base
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

## base hooks manifest

```text
Print the hook command table as JSON, for an installer outside base

Usage: base hooks manifest

Options:
  -h, --help
          Print help
```

## base ast

```text
Query AST codebase graph (entities, calls, imports)

Usage: base ast <COMMAND>

Commands:
  query   Query AST graph for entities, calls, and imports [aliases: q]
  list    List registered per-app code maps (name, entities, path, last synced) [aliases: l]
  ensure  Make sure the app containing PATH has a code map: build one in the background if it has none, do nothing if it has (what the hooks do on first contact; the Windows hooks call this inside WSL for Linux paths)
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

## base ast query

```text
Query AST graph for entities, calls, and imports

Usage: base ast query [OPTIONS]

Options:
  -c, --contains <CONTAINS>
          Find entities by name (case-insensitive substring match)

  -f, --file <FILE>
          List all entities in a source file with relationships

      --calls <CALLS>
          Find all callers of a named entity

  -i, --imports <IMPORTS>
          Find all files that import from a given file

  -t, --target <TARGET>
          Query a specific app's map by path (e.g. apps/foo) instead of the cwd's map

  -h, --help
          Print help
```

## base ast list

```text
List registered per-app code maps (name, entities, path, last synced)

Usage: base ast list

Options:
  -h, --help
          Print help
```

## base ast ensure

```text
Make sure the app containing PATH has a code map: build one in the background if it has none, do nothing if it has (what the hooks do on first contact; the Windows hooks call this inside WSL for Linux paths)

Usage: base ast ensure [OPTIONS] <PATH>

Arguments:
  <PATH>
          A file or folder inside the app

Options:
      --wait
          Build in the foreground and return when the map has landed (for a caller whose process must outlive the build, e.g. `wsl -e sh`)

  -h, --help
          Print help
```

## base project

```text
Manage projects

Usage: base project <COMMAND>

Commands:
  add     Add a new project [aliases: a]
  list    List projects (defaults to the current workspace; cross-awareness via flags) [aliases: l]
  get     Show a specific project (accepts slug or display name)
  peer    Make a project also surface in another workspace (additive peerWorkspace edge)
  repath  Re-point a project's folder path (graph + domain trigger) after it moves
  update  Update a project (accepts slug or display name) [aliases: u]
  move    Re-home a project to another workspace graph (node + tasks + domain + decisions/rules/notes). AST regenerates at the destination. PREVIEW unless --yes
  delete  Delete a project. Refuses a non-empty project unless --force (which cascade- deletes tasks/milestones/decisions/rules). PREVIEW unless --yes
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

## base project add

```text
Add a new project

Usage: base project add [OPTIONS] --name <NAME>

Options:
  -n, --name <NAME>
          

  -s, --status <STATUS>
          [default: active]

  -p, --path <PATH>
          Project path (workspace-relative). If omitted and [protocol] is enabled, the folder is derived from the protocol stage and auto-created

      --stage <STAGE>
          Protocol lifecycle stage the project starts in (default: first stage)

  -h, --help
          Print help
```

## base project list

```text
List projects (defaults to the current workspace; cross-awareness via flags)

Usage: base project list [OPTIONS]

Options:
      --all
          Show projects from every registered workspace (today's flat union)

      --workspace <WORKSPACE>
          Show only projects homed in the named workspace

      --unscoped
          Show only projects with no #path / no registered home

      --json
          Emit JSON (stable dashboard contract) instead of a table

  -h, --help
          Print help
```

## base project get

```text
Show a specific project (accepts slug or display name)

Usage: base project get [OPTIONS] <SLUG>

Arguments:
  <SLUG>
          

Options:
      --json
          Emit JSON instead of the human field list

  -h, --help
          Print help
```

## base project peer

```text
Make a project also surface in another workspace (additive peerWorkspace edge)

Usage: base project peer [OPTIONS] --workspace <WORKSPACE> <SLUG>

Arguments:
  <SLUG>
          Project slug or display name

Options:
  -w, --workspace <WORKSPACE>
          Workspace the project should also surface in

      --remove
          Remove the peer edge instead of adding it

  -h, --help
          Print help
```

## base project repath

```text
Re-point a project's folder path (graph + domain trigger) after it moves

Usage: base project repath <SLUG> <PATH>

Arguments:
  <SLUG>
          

  <PATH>
          New folder path (absolute, or relative to the workspace root)

Options:
  -h, --help
          Print help
```

## base project update

```text
Update a project (accepts slug or display name)

Usage: base project update [OPTIONS] <SLUG>

Arguments:
  <SLUG>
          

Options:
  -s, --status <STATUS>
          

  -b, --blocked-by <BLOCKED_BY>
          

      --next-action <NEXT_ACTION>
          

  -h, --help
          Print help
```

## base project move

```text
Re-home a project to another workspace graph (node + tasks + domain + decisions/rules/notes). AST regenerates at the destination. PREVIEW unless --yes

Usage: base project move [OPTIONS] --to <TO> <SLUG>

Arguments:
  <SLUG>
          Project slug or display name (in the current workspace)

Options:
      --to <TO>
          Destination workspace name (registered in base.toml [[workspace]])

      --dry-run
          Preview the move plan; write nothing

      --no-ast
          Skip regenerating the AST map at the destination

      --yes
          Apply the move (without it, prints the plan and writes nothing)

  -h, --help
          Print help
```

## base project delete

```text
Delete a project. Refuses a non-empty project unless --force (which cascade- deletes tasks/milestones/decisions/rules). PREVIEW unless --yes

Usage: base project delete [OPTIONS] <SLUG>

Arguments:
  <SLUG>
          Project slug or display name

Options:
      --force
          Cascade-delete child tasks/milestones/decisions/rules

      --yes
          Apply the delete (without it, prints the plan and writes nothing)

  -h, --help
          Print help
```

## base milestone

```text
Manage milestones (epics within a project)

Usage: base milestone <COMMAND>

Commands:
  add     Add a milestone to a project [aliases: a]
  list    List milestones (optionally filtered by project) [aliases: l]
  get     Show a specific milestone
  update  Update a milestone [aliases: u]
  delete  Delete a milestone. Tasks are DETACHED to project-level by default; --force cascade-deletes them. PREVIEW unless --yes
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

## base milestone add

```text
Add a milestone to a project

Usage: base milestone add [OPTIONS] --project <PROJECT> --name <NAME>

Options:
  -p, --project <PROJECT>
          Project slug or display name

  -n, --name <NAME>
          

  -d, --description <DESCRIPTION>
          

  -h, --help
          Print help
```

## base milestone list

```text
List milestones (optionally filtered by project)

Usage: base milestone list [OPTIONS]

Options:
  -p, --project <PROJECT>
          Project slug or display name

      --json
          Emit JSON (stable dashboard contract) instead of a table

  -h, --help
          Print help
```

## base milestone get

```text
Show a specific milestone

Usage: base milestone get [OPTIONS] <SLUG>

Arguments:
  <SLUG>
          

Options:
      --json
          Emit JSON instead of the human field list

  -h, --help
          Print help
```

## base milestone update

```text
Update a milestone

Usage: base milestone update [OPTIONS] <SLUG>

Arguments:
  <SLUG>
          

Options:
  -s, --status <STATUS>
          

  -d, --description <DESCRIPTION>
          

  -h, --help
          Print help
```

## base milestone delete

```text
Delete a milestone. Tasks are DETACHED to project-level by default; --force cascade-deletes them. PREVIEW unless --yes

Usage: base milestone delete [OPTIONS] <SLUG>

Arguments:
  <SLUG>
          

Options:
      --force
          Cascade-delete the milestone's tasks instead of detaching them

      --yes
          Apply the delete (without it, prints the plan and writes nothing)

  -h, --help
          Print help
```

## base task

```text
Manage tasks

Usage: base task <COMMAND>

Commands:
  add     Add a task to a project (optionally under a milestone) [aliases: a]
  list    List tasks (filter by project, milestone, or label) [aliases: l]
  get     Show a specific task (all fields; accepts slug or display name)
  update  Update a task's mutable fields (accepts slug or display name) [aliases: u]
  delete  Delete a task node + its edges. PREVIEW unless --yes
  done    Mark a task as completed
  tag     Attach/detach free-form labels on a task (the dashboard's tagging facet)
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

## base task add

```text
Add a task to a project (optionally under a milestone)

Usage: base task add [OPTIONS] --project <PROJECT> --name <NAME>

Options:
  -p, --project <PROJECT>
          Project slug or display name

  -n, --name <NAME>
          

      --priority <PRIORITY>
          

  -m, --milestone <MILESTONE>
          Milestone slug to group this task under

  -h, --help
          Print help
```

## base task list

```text
List tasks (filter by project, milestone, or label)

Usage: base task list [OPTIONS]

Options:
  -p, --project <PROJECT>
          Project slug or display name

  -m, --milestone <MILESTONE>
          Milestone slug to filter by

      --label <LABEL>
          Only tasks carrying ALL of these labels (repeatable)

      --json
          Emit JSON (stable dashboard contract) instead of a table

  -h, --help
          Print help
```

## base task get

```text
Show a specific task (all fields; accepts slug or display name)

Usage: base task get [OPTIONS] <SLUG>

Arguments:
  <SLUG>
          

Options:
      --json
          Emit JSON instead of the human field list

  -h, --help
          Print help
```

## base task update

```text
Update a task's mutable fields (accepts slug or display name)

Usage: base task update [OPTIONS] <SLUG>

Arguments:
  <SLUG>
          

Options:
      --name <NAME>
          New display name

  -s, --status <STATUS>
          Status (canonical vocabulary: active | completed)

      --priority <PRIORITY>
          

      --description <DESCRIPTION>
          Free-form description / notes
          
          [aliases: --notes]

      --assignee <ASSIGNEE>
          

      --due <DUE>
          Due date (free-form or ISO)

  -p, --project <PROJECT>
          Reassign to another project (rewrites the project edge only)

  -m, --milestone <MILESTONE>
          Reassign to another milestone (rewrites the milestone edge only)

  -h, --help
          Print help
```

## base task delete

```text
Delete a task node + its edges. PREVIEW unless --yes

Usage: base task delete [OPTIONS] <SLUG>

Arguments:
  <SLUG>
          

Options:
      --yes
          Apply the delete (without it, prints the task and writes nothing)

  -h, --help
          Print help
```

## base task done

```text
Mark a task as completed

Usage: base task done <SLUG>

Arguments:
  <SLUG>
          

Options:
  -h, --help
          Print help
```

## base task tag

```text
Attach/detach free-form labels on a task (the dashboard's tagging facet)

Usage: base task tag [OPTIONS] <SLUG>

Arguments:
  <SLUG>
          

Options:
      --add <ADD>
          Label to attach (repeatable, idempotent)

      --remove <REMOVE>
          Label to detach (repeatable)

  -h, --help
          Print help
```

## base decision

```text
Log and search decisions

Usage: base decision [OPTIONS] <COMMAND>

Commands:
  log     Log a new decision
  search  Search decisions by keyword
  delete  Delete decisions matching a keyword
  update  Update a decision in place, addressed by its stable {domain}.{decision} slug [aliases: u]
  help    Print this message or the help of the given subcommand(s)

Options:
  -g, --global
          Target the global tier (~/.base-gbl/) instead of workspace

  -h, --help
          Print help
```

## base decision log

```text
Log a new decision

Usage: base decision log [OPTIONS] --domain <DOMAIN> --decision <DECISION> --rationale <RATIONALE>

Options:
      --domain <DOMAIN>
          

      --decision <DECISION>
          

      --rationale <RATIONALE>
          

      --recall <RECALL>
          

  -h, --help
          Print help
```

## base decision search

```text
Search decisions by keyword

Usage: base decision search [OPTIONS] --keyword <KEYWORD>

Options:
      --keyword <KEYWORD>
          

      --json
          Emit JSON (stable dashboard contract) instead of a table

  -h, --help
          Print help
```

## base decision delete

```text
Delete decisions matching a keyword

Usage: base decision delete --keyword <KEYWORD>

Options:
      --keyword <KEYWORD>
          Keyword to match against decision names

  -h, --help
          Print help
```

## base decision update

```text
Update a decision in place, addressed by its stable {domain}.{decision} slug

Usage: base decision update [OPTIONS] <SLUG>

Arguments:
  <SLUG>
          Decision slug ({domain}.{decision}) or exact decision text

Options:
      --name <NAME>
          

      --rationale <RATIONALE>
          

      --recall <RECALL>
          

  -s, --status <STATUS>
          

  -h, --help
          Print help
```

## base entity

```text
Manage entities (people, organizations)

Usage: base entity <COMMAND>

Commands:
  add     Add an entity (person or organization) — must link to at least one domain or project
  list    List all entities
  get     Show a specific entity (accepts slug or display name)
  update  Update an entity (accepts slug or display name)
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

## base entity add

```text
Add an entity (person or organization) — must link to at least one domain or project

Usage: base entity add [OPTIONS] --name <NAME> --domain <DOMAIN>

Options:
      --name <NAME>
          

      --entity-type <type>
          Type: person, organization
          
          [default: person]

      --domain <DOMAIN>
          Domain this entity relates to (REQUIRED — prevents orphan entities)

      --project <PROJECT>
          Project this entity relates to (optional additional edge)

  -h, --help
          Print help
```

## base entity list

```text
List all entities

Usage: base entity list

Options:
  -h, --help
          Print help
```

## base entity get

```text
Show a specific entity (accepts slug or display name)

Usage: base entity get <SLUG>

Arguments:
  <SLUG>
          

Options:
  -h, --help
          Print help
```

## base entity update

```text
Update an entity (accepts slug or display name)

Usage: base entity update [OPTIONS] <SLUG>

Arguments:
  <SLUG>
          

Options:
      --status <STATUS>
          

      --description <DESCRIPTION>
          

  -h, --help
          Print help
```

## base goal

```text
Manage goals

Usage: base goal <COMMAND>

Commands:
  add     Add a goal
  list    List all goals
  update  Update a goal (accepts slug or display name)
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

## base goal add

```text
Add a goal

Usage: base goal add --name <NAME> --target <TARGET>

Options:
      --name <NAME>
          

      --target <TARGET>
          

  -h, --help
          Print help
```

## base goal list

```text
List all goals

Usage: base goal list

Options:
  -h, --help
          Print help
```

## base goal update

```text
Update a goal (accepts slug or display name)

Usage: base goal update [OPTIONS] <SLUG>

Arguments:
  <SLUG>
          

Options:
      --status <STATUS>
          

      --target <TARGET>
          

  -h, --help
          Print help
```

## base reminder

```text
Manage reminders

Usage: base reminder <COMMAND>

Commands:
  add     Add a reminder (provide one of --in, --at, or --due)
  list    List all reminders
  remove  Remove a reminder (hard delete)
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

## base reminder add

```text
Add a reminder (provide one of --in, --at, or --due)

Usage: base reminder add [OPTIONS] --name <NAME>

Options:
      --name <NAME>
          

      --due <DUE>
          Due date (YYYY-MM-DD) — surfaces on/after this date

      --at <AT>
          Exact surface time, ISO-8601 (e.g. 2026-06-23T14:30:00-05:00)

      --in <IN_DUR>
          Relative surface time from now: 30s, 3m, 2h, 1d

  -h, --help
          Print help
```

## base reminder list

```text
List all reminders

Usage: base reminder list

Options:
  -h, --help
          Print help
```

## base reminder remove

```text
Remove a reminder (hard delete)

Usage: base reminder remove <SLUG>

Arguments:
  <SLUG>
          

Options:
  -h, --help
          Print help
```

## base handoff

```text
Manage session handoffs (resume docs surfaced at session start)

Usage: base handoff [OPTIONS] <COMMAND>

Commands:
  create   Register a handoff doc (archives any prior open handoff for the project)
  list     List handoffs across global + workspace tiers
  snooze   Snooze a handoff for N days (hide until then)
  archive  Archive a handoff (stop resurfacing)
  help     Print this message or the help of the given subcommand(s)

Options:
  -g, --global
          Target the global tier (~/.base-gbl/) instead of workspace

  -h, --help
          Print help
```

## base handoff create

```text
Register a handoff doc (archives any prior open handoff for the project)

Usage: base handoff create [OPTIONS] --project <PROJECT> --doc <DOC>

Options:
      --project <PROJECT>
          

      --doc <DOC>
          

      --slug <SLUG>
          Graph slug / title to summon it by (default: doc basename)

  -h, --help
          Print help
```

## base handoff list

```text
List handoffs across global + workspace tiers

Usage: base handoff list

Options:
  -h, --help
          Print help
```

## base handoff snooze

```text
Snooze a handoff for N days (hide until then)

Usage: base handoff snooze <SLUG> <DAYS>

Arguments:
  <SLUG>
          

  <DAYS>
          

Options:
  -h, --help
          Print help
```

## base handoff archive

```text
Archive a handoff (stop resurfacing)

Usage: base handoff archive <SLUG>

Arguments:
  <SLUG>
          

Options:
  -h, --help
          Print help
```

## base fork

```text
Manage parallel side-work forks (build-specs surfaced at session start)

Usage: base fork [OPTIONS] <COMMAND>

Commands:
  create   Register a fork build-spec (additive — does not archive sibling forks)
  list     List forks across global + workspace tiers
  snooze   Snooze a fork for N days (hide until then)
  archive  Archive a fork (stop resurfacing)
  help     Print this message or the help of the given subcommand(s)

Options:
  -g, --global
          Target the global tier (~/.base-gbl/) instead of workspace

  -h, --help
          Print help
```

## base fork create

```text
Register a fork build-spec (additive — does not archive sibling forks)

Usage: base fork create [OPTIONS] --project <PROJECT> --doc <DOC>

Options:
      --project <PROJECT>
          

      --doc <DOC>
          

      --slug <SLUG>
          Graph slug / title to summon it by (default: doc basename)

  -h, --help
          Print help
```

## base fork list

```text
List forks across global + workspace tiers

Usage: base fork list

Options:
  -h, --help
          Print help
```

## base fork snooze

```text
Snooze a fork for N days (hide until then)

Usage: base fork snooze <SLUG> <DAYS>

Arguments:
  <SLUG>
          

  <DAYS>
          

Options:
  -h, --help
          Print help
```

## base fork archive

```text
Archive a fork (stop resurfacing)

Usage: base fork archive <SLUG>

Arguments:
  <SLUG>
          

Options:
  -h, --help
          Print help
```

## base sync

```text
Sync file-owned data into the graph

Usage: base sync [OPTIONS]

Options:
      --incremental
          Only re-extract files changed since last sync

      --ast
          Run AST codebase extraction (tree-sitter, 35+ languages)

      --target <TARGET>
          Target directory for AST extraction (defaults to cwd)

      --yes
          Unattended: proceed past the extractor's file-count safety threshold without asking (what the hooks pass — nobody is there to answer)

      --repair
          Repair missing edges (backfill decision→domain, milestone→project, task→project links)

  -h, --help
          Print help
```

## base domain

```text
Manage domain matching rules

Usage: base domain <COMMAND>

Commands:
  add-trigger     Add a keyword or path trigger to a domain
  list            List all configured domains
  get             Show a specific domain's full configuration
  sync            Sync domains/rules from domains.toml into the graph. Optionally migrate decisions from carl.json
  create          Create a new domain in domains.toml
  remove          Remove a domain from domains.toml
  remove-trigger  Remove a keyword or path trigger from a domain
  help            Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

## base domain add-trigger

```text
Add a keyword or path trigger to a domain

Usage: base domain add-trigger [OPTIONS] --domain <DOMAIN>

Options:
      --domain <DOMAIN>
          

      --keyword <KEYWORD>
          

      --path <PATH>
          

  -h, --help
          Print help
```

## base domain list

```text
List all configured domains

Usage: base domain list

Options:
  -h, --help
          Print help
```

## base domain get

```text
Show a specific domain's full configuration

Usage: base domain get <NAME>

Arguments:
  <NAME>
          

Options:
  -h, --help
          Print help
```

## base domain sync

```text
Sync domains/rules from domains.toml into the graph. Optionally migrate decisions from carl.json

Usage: base domain sync [OPTIONS]

Options:
      --carl <CARL>
          Path to carl.json for one-time decision migration

  -h, --help
          Print help
```

## base domain create

```text
Create a new domain in domains.toml

Usage: base domain create [OPTIONS] --name <NAME>

Options:
      --name <NAME>
          

      --keyword <KEYWORD>
          

      --path <PATH>
          

  -h, --help
          Print help
```

## base domain remove

```text
Remove a domain from domains.toml

Usage: base domain remove <NAME>

Arguments:
  <NAME>
          Domain name (case-insensitive)

Options:
  -h, --help
          Print help
```

## base domain remove-trigger

```text
Remove a keyword or path trigger from a domain

Usage: base domain remove-trigger [OPTIONS] --domain <DOMAIN>

Options:
      --domain <DOMAIN>
          

      --keyword <KEYWORD>
          

      --path <PATH>
          

  -h, --help
          Print help
```

## base standards

```text
Manage context-triggered standards (MIDAS protocols injected on edit)

Usage: base standards <COMMAND>

Commands:
  sync  Sync MIDAS protocols.md → standards.toml + graph Standard entities
  list  List all standards with trigger/annotation counts
  get   Show a standard's full config
  test  Dry-run the matcher against a file — scores + what would inject
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

## base standards sync

```text
Sync MIDAS protocols.md → standards.toml + graph Standard entities

Usage: base standards sync [OPTIONS]

Options:
      --source <SOURCE>
          Override the protocols.md source path

  -h, --help
          Print help
```

## base standards list

```text
List all standards with trigger/annotation counts

Usage: base standards list

Options:
  -h, --help
          Print help
```

## base standards get

```text
Show a standard's full config

Usage: base standards get <ID>

Arguments:
  <ID>
          

Options:
  -h, --help
          Print help
```

## base standards test

```text
Dry-run the matcher against a file — scores + what would inject

Usage: base standards test [OPTIONS] <FILE>

Arguments:
  <FILE>
          

Options:
      --content <CONTENT>
          Extra content included in the haystack (simulates an edit payload)

  -h, --help
          Print help
```

## base relay

```text
Session-to-session message relay (parallel PAUL workers, Cadre firm members)

Usage: base relay <COMMAND>

Commands:
  init      Create the ephemeral relay store for a project
  register  Register (or re-bind) this session under a stable title
  send      Send a message to a session, title, phase, or all
  poll      Non-blocking read of pending messages (consumes them)
  wait      BLOCK until a matching message arrives — burns zero session tokens
  claim     Take an advisory claim on a path or phase (TTL-bounded)
  release   Release a claim
  board     Operator view: sessions, liveness, claims, pending messages
  export    Export the spool as inbox.nq (read-only graph snapshot)
  dispose   End-of-milestone teardown — the store is disposable by design
  task      Relay a briefed task to a live titled session. It auto-fires in that session's hooks (loud) until picked up — cross-workspace via the global tier
  ping      Instant message to a live titled session — no doc, no done-ceremony. Screams in the receiver's hooks mid-turn; their reply ping clears it
  done      Mark a relayed task done — clears the inbox alert and closes the graph mirror
  tasks     List inbound relay tasks across all live sessions
  sessions  List titled sessions in the global registry (liveness for `*task` targets)
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

## base relay init

```text
Create the ephemeral relay store for a project

Usage: base relay init --project <PROJECT>

Options:
      --project <PROJECT>
          

  -h, --help
          Print help
```

## base relay register

```text
Register (or re-bind) this session under a stable title

Usage: base relay register [OPTIONS] --as <TITLE>

Options:
      --as <TITLE>
          Stable identity: worker-phase-11, quill, orchestrator…

      --session <SESSION>
          Session id override (defaults to CLAUDE_CODE_SESSION_ID)

      --phase <PHASE>
          

      --project <PROJECT>
          

  -h, --help
          Print help
```

## base relay send

```text
Send a message to a session, title, phase, or all

Usage: base relay send [OPTIONS] --to <TO> --type <MTYPE> --msg <MSG>

Options:
      --to <TO>
          Recipient: title | session-id | phase:<n> | all

      --type <MTYPE>
          claim|release|notify|unblock|contract-change|ready-to-merge|question|answer

      --msg <MSG>
          

      --from <FROM>
          Sender override (defaults to this session's registered title)

      --refs <REFS>
          File paths / phase ids this message references

      --project <PROJECT>
          

  -h, --help
          Print help
```

## base relay poll

```text
Non-blocking read of pending messages (consumes them)

Usage: base relay poll [OPTIONS]

Options:
      --for <FOR_TITLE>
          Read for a specific title (defaults to this session's identity)

      --peek
          Peek without consuming

      --project <PROJECT>
          

  -h, --help
          Print help
```

## base relay wait

```text
BLOCK until a matching message arrives — burns zero session tokens

Usage: base relay wait [OPTIONS]

Options:
      --from <FROM>
          Only messages from this sender

      --type <MTYPE>
          Only messages of this type

      --timeout <TIMEOUT>
          Timeout in seconds
          
          [default: 300]

      --for <FOR_TITLE>
          Wait as a specific title (defaults to this session's identity)

      --project <PROJECT>
          

  -h, --help
          Print help
```

## base relay claim

```text
Take an advisory claim on a path or phase (TTL-bounded)

Usage: base relay claim [OPTIONS] <RESOURCE>

Arguments:
  <RESOURCE>
          

Options:
      --note <NOTE>
          [default: ""]

      --ttl <TTL>
          TTL in seconds
          
          [default: 3600]

      --project <PROJECT>
          

  -h, --help
          Print help
```

## base relay release

```text
Release a claim

Usage: base relay release [OPTIONS] <RESOURCE>

Arguments:
  <RESOURCE>
          

Options:
      --force
          Operator force-release of another session's claim

      --project <PROJECT>
          

  -h, --help
          Print help
```

## base relay board

```text
Operator view: sessions, liveness, claims, pending messages

Usage: base relay board [OPTIONS]

Options:
      --project <PROJECT>
          

  -h, --help
          Print help
```

## base relay export

```text
Export the spool as inbox.nq (read-only graph snapshot)

Usage: base relay export [OPTIONS]

Options:
      --project <PROJECT>
          

  -h, --help
          Print help
```

## base relay dispose

```text
End-of-milestone teardown — the store is disposable by design

Usage: base relay dispose [OPTIONS] --project <PROJECT>

Options:
      --project <PROJECT>
          

      --force
          Actually delete (without this, prints what would be removed)

  -h, --help
          Print help
```

## base relay task

```text
Relay a briefed task to a live titled session. It auto-fires in that session's hooks (loud) until picked up — cross-workspace via the global tier

Usage: base relay task [OPTIONS] --to <TO> --slug <SLUG> --summary <SUMMARY>

Options:
      --to <TO>
          Target session's registered title (set with: base relay register --as <title>)

      --slug <SLUG>
          Task slug — kebab-case, matches the briefing doc basename

      --summary <SUMMARY>
          One-line summary shown in the alert

      --doc <DOC>
          Absolute path to the full briefing doc the receiver should read

      --priority <PRIORITY>
          Priority: high | medium (default: high)

      --from <FROM>
          Origin label shown to the receiver (defaults to this session's title)

  -h, --help
          Print help
```

## base relay ping

```text
Instant message to a live titled session — no doc, no done-ceremony. Screams in the receiver's hooks mid-turn; their reply ping clears it

Usage: base relay ping [OPTIONS] --to <TO> --msg <MSG>

Options:
      --to <TO>
          Target session's registered title

      --msg <MSG>
          The message — carries ALL context inline (a sentence or three; more than that is a task, not a ping)

      --refs <REFS>
          File paths / entity ids this ping references

      --from <FROM>
          Origin label (defaults to this session's registered/auto-assigned title)

  -h, --help
          Print help
```

## base relay done

```text
Mark a relayed task done — clears the inbox alert and closes the graph mirror

Usage: base relay done <SLUG>

Arguments:
  <SLUG>
          Task slug

Options:
  -h, --help
          Print help
```

## base relay tasks

```text
List inbound relay tasks across all live sessions

Usage: base relay tasks

Options:
  -h, --help
          Print help
```

## base relay sessions

```text
List titled sessions in the global registry (liveness for `*task` targets)

Usage: base relay sessions

Options:
  -h, --help
          Print help
```

## base learn

```text
Graph-backed structured memory

Usage: base learn [OPTIONS]

Options:
  -g, --global
          Target the global tier (~/.base-gbl/) instead of workspace

      --text <TEXT>
          The memory text to store (required unless --mention, --remove, --update, or --list)

      --type <TYPE>
          Note type: insight, correction, decision, commitment, shift
          
          [default: insight]

      --domain <DOMAIN>
          Link to a domain (required unless --mention)

      --project <PROJECT>
          Link to a project (optional additional edge)

      --entity <ENTITY>
          Link to an entity (optional additional edge)

      --mention <MENTION>
          Record a mention of an existing note (pass the slug)

      --context <CONTEXT>
          Context for the mention

      --remove <REMOVE>
          Remove a note by slug

      --update <UPDATE>
          Update a note's text by slug (requires --text)

      --list
          List all notes (optionally filter by --type or --domain)

  -h, --help
          Print help
```

## base recall

```text
Search notes by keyword, domain, or slug

Usage: base recall [OPTIONS]

Options:
      --keyword <KEYWORD>
          Search text in note content

      --domain <DOMAIN>
          Filter by linked domain

      --slug <SLUG>
          Look up a specific note by slug

  -h, --help
          Print help
```

## base changes

```text
Read the graph change log — every successful graph write, as JSON

Cursor is a BYTE OFFSET into the log, not a sequence number: it needs no sidecar counter, survives concurrent appenders, and is what a reader resumes from directly.

Usage: base changes [OPTIONS]

Options:
  -g, --global
          Target the global tier (~/.base-gbl/) instead of workspace

      --since <SINCE>
          Print entries written after this byte offset

      --cursor
          Print only the current end offset and exit

  -h, --help
          Print help (see a summary with '-h')
```

## base rule

```text
Manage rules in the graph (add, list, remove)

Usage: base rule [OPTIONS] <COMMAND>

Commands:
  add     Add a rule to a domain in the graph
  list    List rules for a domain from the graph
  remove  Remove a rule by index from a domain
  help    Print this message or the help of the given subcommand(s)

Options:
  -g, --global
          Target the global tier (~/.base-gbl/) instead of workspace

  -h, --help
          Print help
```

## base rule add

```text
Add a rule to a domain in the graph

Usage: base rule add [OPTIONS] --domain <DOMAIN> --text <TEXT>

Options:
      --domain <DOMAIN>
          

      --text <TEXT>
          

      --rationale <RATIONALE>
          Optional rationale — injected as "rule — because rationale" (Phase 26)

  -h, --help
          Print help
```

## base rule list

```text
List rules for a domain from the graph

Usage: base rule list --domain <DOMAIN>

Options:
      --domain <DOMAIN>
          

  -h, --help
          Print help
```

## base rule remove

```text
Remove a rule by index from a domain

Usage: base rule remove --domain <DOMAIN> --index <INDEX>

Options:
      --domain <DOMAIN>
          

      --index <INDEX>
          

  -h, --help
          Print help
```

## base install

```text
Install base globally: build, symlink, create ~/.base-gbl, wire hooks, write manifest

Usage: base install [OPTIONS]

Options:
      --carl <CARL>
          Path to carl.json for decision migration

      --skip-hooks
          Skip hook wiring in settings.json

      --full
          Register all ChrisAI components (PAUL, SEED, SKILLSMITH) in manifest

      --starter-commands
          Install the starter star commands without asking (*handoff, *fork, *base, *end)

      --no-starter-commands
          Skip the starter star commands without asking

  -h, --help
          Print help
```

## base activate

```text
Activate ChrisAI — enter your Skool classroom key to remove attribution

Usage: base activate <KEY>

Arguments:
  <KEY>
          Activation key from ChrisAI community

Options:
  -h, --help
          Print help
```

## base update

```text
Self-update the base binary from public GitHub releases (or snooze the banner)

Usage: base update [OPTIONS]

Options:
      --check
          Re-validate + report whether a newer base is available, without installing

      --force
          Install even when already on the latest version

      --snooze
          Dismiss the update banner for 24 hours

  -h, --help
          Print help
```

## base uninstall

```text
Uninstall base: remove hooks from settings.json, remove binary, remove CLAUDE.md section

Usage: base uninstall [OPTIONS]

Options:
      --purge
          Also remove ~/.base-gbl/ global tier (destructive)

  -h, --help
          Print help
```

## base dashboard

```text
Launch the Command Center Dashboard (local web UI)

Usage: base dashboard [OPTIONS]

Options:
  -p, --port <PORT>
          Port to serve on (default: 3741)
          
          [default: 3741]

  -h, --help
          Print help
```

## base scaffold

```text
Scaffold a new workspace: create .base/, write configs, register globally

Usage: base scaffold [PATH]

Arguments:
  [PATH]
          Target directory (defaults to cwd)

Options:
  -h, --help
          Print help
```

## base reconcile

```text
Reconcile project active/deferred state from real folder last-touch

Usage: base reconcile [OPTIONS]

Options:
      --dry-run
          Preview what would change — no graph writes. Bypasses the [protocol] enabled gate

  -h, --help
          Print help
```

## base workspace

```text
Registered-workspace registry (sync CLAUDE.md from base.toml)

Usage: base workspace <COMMAND>

Commands:
  sync  Regenerate the registered-workspaces block in ~/.claude/CLAUDE.md from base.toml
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

## base workspace sync

```text
Regenerate the registered-workspaces block in ~/.claude/CLAUDE.md from base.toml

Usage: base workspace sync

Options:
  -h, --help
          Print help
```

## base operator

```text
Operator identity profile (init, show)

Usage: base operator <COMMAND>

Commands:
  init  Create operator profile at ~/.base-gbl/operator.toml
  show  Show current operator profile
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

## base operator init

```text
Create operator profile at ~/.base-gbl/operator.toml

Usage: base operator init --name <NAME>

Options:
      --name <NAME>
          

  -h, --help
          Print help
```

## base operator show

```text
Show current operator profile

Usage: base operator show

Options:
  -h, --help
          Print help
```

## base extension

```text
Manage extensions (list, validate, install, remove)

Usage: base extension <COMMAND>

Commands:
  list      List all installed extensions
  validate  Validate an extension manifest file
  install   Install an extension (copy validated TOML to extensions/)
  add       Fetch a plugin's prebuilt binary for THIS host from its GitHub release ([dist] block), verify the sha256, unpack + install — cross-platform, no toolchain. Falls back to a local source build when no host asset exists
  scaffold  Scaffold a new, conformant Bun cross-platform plugin. With --bootstrap, the one-command kickoff: writes the files, builds, git-inits, and creates+pushes a private GitHub repo — ready to develop, born cross-platform
  remove    Remove an installed extension by name
  run       Run a drop-in plugin command explicitly (collision-proof): `base ext run <name> [args…]`
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

## base extension list

```text
List all installed extensions

Usage: base extension list

Options:
  -h, --help
          Print help
```

## base extension validate

```text
Validate an extension manifest file

Usage: base extension validate <PATH>

Arguments:
  <PATH>
          Path to the TOML file to validate

Options:
  -h, --help
          Print help
```

## base extension install

```text
Install an extension (copy validated TOML to extensions/)

Usage: base extension install [OPTIONS] <PATH>

Arguments:
  <PATH>
          Path to the TOML file to install

Options:
      --bundle
          Bundle the handler into ~/.base-gbl/plugins/<name>/ and repoint the manifest there — a self-contained, repo-independent (shippable) install

  -h, --help
          Print help
```

## base extension add

```text
Fetch a plugin's prebuilt binary for THIS host from its GitHub release ([dist] block), verify the sha256, unpack + install — cross-platform, no toolchain. Falls back to a local source build when no host asset exists

Usage: base extension add <PATH>

Arguments:
  <PATH>
          Path to the base-extension.toml (with a [dist] block) to fetch + install

Options:
  -h, --help
          Print help
```

## base extension scaffold

```text
Scaffold a new, conformant Bun cross-platform plugin. With --bootstrap, the one-command kickoff: writes the files, builds, git-inits, and creates+pushes a private GitHub repo — ready to develop, born cross-platform

Usage: base extension scaffold [OPTIONS] <NAME>

Arguments:
  <NAME>
          Plugin/binary name — the `base <name>` command (lowercase, hyphens ok)

Options:
      --path <PATH>
          Parent directory to create <name>-cli/ in (default: current dir)

      --into <INTO>
          Exact target folder (new or empty) — overrides the default <name>-cli

      --repo <REPO>
          GitHub owner/repo for releases (default: ChristopherKahler/<name>-cli)

      --build
          Run prepare.sh (bun build → bin/<name>) after writing files

      --git
          git init + first commit

      --create-repo
          Create a private GitHub repo, wire origin, and push (implies --git)

      --bootstrap
          One-flag full kickoff: build + git + create-repo

  -h, --help
          Print help
```

## base extension remove

```text
Remove an installed extension by name

Usage: base extension remove <NAME>

Arguments:
  <NAME>
          Extension name to remove

Options:
  -h, --help
          Print help
```

## base extension run

```text
Run a drop-in plugin command explicitly (collision-proof): `base ext run <name> [args…]`

Usage: base extension run <NAME> [ARGS]...

Arguments:
  <NAME>
          Plugin command name (as declared in an extension's [[commands]])

  [ARGS]...
          Arguments forwarded verbatim to the handler

Options:
  -h, --help
          Print help
```

## base commands

```text
List and inspect star commands (*BLUNT, *AUDIT, etc.)

Usage: base commands <COMMAND>

Commands:
  list    List all configured star commands
  show    Show details for a specific star command
  add     Add a new star command to commands.toml
  remove  Remove a star command from commands.toml
  import  Import star commands from a commands.toml file (append-only; skips names already present, never alters preceding content)
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

## base commands list

```text
List all configured star commands

Usage: base commands list

Options:
  -h, --help
          Print help
```

## base commands show

```text
Show details for a specific star command

Usage: base commands show <NAME>

Arguments:
  <NAME>
          Command name (case-insensitive, without *)

Options:
  -h, --help
          Print help
```

## base commands add

```text
Add a new star command to commands.toml

Usage: base commands add [OPTIONS] --name <NAME> --description <DESCRIPTION>

Options:
      --name <NAME>
          

      --description <DESCRIPTION>
          

      --rule <RULE>
          Rules (repeatable)

  -h, --help
          Print help
```

## base commands remove

```text
Remove a star command from commands.toml

Usage: base commands remove <NAME>

Arguments:
  <NAME>
          Command name (case-insensitive)

Options:
  -h, --help
          Print help
```

## base commands import

```text
Import star commands from a commands.toml file (append-only; skips names already present, never alters preceding content)

Usage: base commands import <FILE>

Arguments:
  <FILE>
          Path to a commands.toml file to import (e.g. an Operator Modes pack)

Options:
  -h, --help
          Print help
```

## base memory

```text
Manage graph-backed memory (migrate flat files, purge)

Usage: base memory <COMMAND>

Commands:
  list   List Claude's flat-file memories for review (name, type, description, path)
  purge  Remove flat-file memories that have been confirmed in the graph
  help   Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

## base memory list

```text
List Claude's flat-file memories for review (name, type, description, path)

Usage: base memory list

Options:
  -h, --help
          Print help
```

## base memory purge

```text
Remove flat-file memories that have been confirmed in the graph

Usage: base memory purge

Options:
  -h, --help
          Print help
```

## base config

```text
Read and write base.toml configuration (dot-notation: section.key)

Usage: base config <COMMAND>

Commands:
  get   Get a config value (dot-notation: section.key)
  set   Set a config value (dot-notation: section.key value)
  list  List all config values
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

## base config get

```text
Get a config value (dot-notation: section.key)

Usage: base config get <KEY>

Arguments:
  <KEY>
          Config key (e.g. memory.mode, signal.enabled, flow.resurface)

Options:
  -h, --help
          Print help
```

## base config set

```text
Set a config value (dot-notation: section.key value)

Usage: base config set <KEY> <VALUE>

Arguments:
  <KEY>
          Config key

  <VALUE>
          New value

Options:
  -h, --help
          Print help
```

## base config list

```text
List all config values

Usage: base config list

Options:
  -h, --help
          Print help
```

## base context

```text
Pull targeted graph context on demand (same engine as hook injection)

Usage: base context [OPTIONS] [TEXT]

Arguments:
  [TEXT]
          Text to match against domain triggers

Options:
      --list
          List all available context triggers

  -h, --help
          Print help
```

## base doctor

```text
Diagnose graph health across tiers (parser-independent). Exits nonzero when unhealthy

Usage: base doctor [OPTIONS]

Options:
      --json
          Emit machine-readable JSON instead of the human report

      --repair
          Self-heal: quarantine malformed lines and atomically rewrite the good set (backs up first)

      --restore [<RESTORE>]
          Restore the workspace graph from a backup snapshot. Bare `--restore` lists snapshots

  -h, --help
          Print help
```

## base graph

```text
First-class graph maintenance (atomic, backs up first — never hand-edit graph.nq)

Usage: base graph <COMMAND>

Commands:
  compact    Dedup + canonicalize the workspace graph (atomic rewrite, snapshots first)
  apply-ops  Apply inbound fact ops (JSON on stdin) into the local graph
  purge      Remove notes unread past --days (recency only). PREVIEW unless --apply
  extract    LLM semantic extraction over a doc corpus → concepts + edges in the graph. Markdown-only by default; PDF/image/audio/video need multimodal enabled (`base config set multimodal.enabled true`, or one-shot --multimodal)
  query      GraphRAG: answer a natural-language question over the graph (retrieve + synthesize)
  analyze    Analyze emergent structure: god nodes, communities, surprising connections
  get-node   Agentic retrieval: full detail for one node (label, type, source, summary, edges)
  neighbors  Agentic retrieval: the n-hop neighborhood of a node as edge lines
  path       Agentic retrieval: shortest path between two nodes
  move       Move a subgraph between workspace graphs (rewrites the named-graph stamp, backs up both tiers, atomic with rollback). PREVIEW unless --yes
  help       Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

## base graph compact

```text
Dedup + canonicalize the workspace graph (atomic rewrite, snapshots first)

Usage: base graph compact

Options:
  -h, --help
          Print help
```

## base graph apply-ops

```text
Apply inbound fact ops (JSON on stdin) into the local graph.

The pull half of desktop sync. Reads either a bare array of ops or `{"ops":[…]}`, applies every assert and retire in ONE transaction, and prints `{applied, skipped_duplicate, skipped_unknown}`. If any op is invalid it exits non-zero having applied nothing.

Usage: base graph apply-ops [OPTIONS]

Options:
      --global
          Apply into the global tier (~/.base-gbl) instead of the workspace

  -h, --help
          Print help (see a summary with '-h')
```

## base graph purge

```text
Remove notes unread past --days (recency only). PREVIEW unless --apply

Usage: base graph purge [OPTIONS]

Options:
      --stale
          Required: select the stale-note rule (no other purge rules yet)

      --apply
          Actually delete (default is a dry-run preview; snapshots before deleting)

      --days <DAYS>
          Unread-age threshold in days (a note's clock resets each time it's recalled)
          
          [default: 21]

  -h, --help
          Print help
```

## base graph extract

```text
LLM semantic extraction over a doc corpus → concepts + edges in the graph. Markdown-only by default; PDF/image/audio/video need multimodal enabled (`base config set multimodal.enabled true`, or one-shot --multimodal)

Usage: base graph extract [OPTIONS]

Options:
  -t, --target <TARGET>
          Directory to extract (defaults to cwd)

  -m, --model <MODEL>
          Claude Code model alias for extraction (e.g. haiku, sonnet, opus)

      --multimodal
          Force multimodal ingest for this run (overrides config; bootstraps pdftotext/ffmpeg/whisper once if a non-markdown corpus needs them)

  -h, --help
          Print help
```

## base graph query

```text
GraphRAG: answer a natural-language question over the graph (retrieve + synthesize)

Usage: base graph query [OPTIONS] <QUESTION>

Arguments:
  <QUESTION>
          The natural-language question

Options:
  -d, --depth <DEPTH>
          Traversal depth
          
          [default: 3]

  -b, --token-budget <TOKEN_BUDGET>
          Token budget for the retrieved subgraph
          
          [default: 2000]

  -m, --model <MODEL>
          Claude Code model alias for synthesis

      --raw
          Print the retrieved subgraph instead of a synthesized answer

  -h, --help
          Print help
```

## base graph analyze

```text
Analyze emergent structure: god nodes, communities, surprising connections

Usage: base graph analyze [OPTIONS]

Options:
  -n, --top-n <TOP_N>
          How many of each to show
          
          [default: 10]

  -h, --help
          Print help
```

## base graph get-node

```text
Agentic retrieval: full detail for one node (label, type, source, summary, edges)

Usage: base graph get-node <NODE>

Arguments:
  <NODE>
          Node label, concept slug, or unique substring

Options:
  -h, --help
          Print help
```

## base graph neighbors

```text
Agentic retrieval: the n-hop neighborhood of a node as edge lines

Usage: base graph neighbors [OPTIONS] <NODE>

Arguments:
  <NODE>
          Node label, concept slug, or unique substring

Options:
  -d, --depth <DEPTH>
          Hops to expand
          
          [default: 1]

  -h, --help
          Print help
```

## base graph path

```text
Agentic retrieval: shortest path between two nodes

Usage: base graph path <FROM> <TO>

Arguments:
  <FROM>
          Start node (label, slug, or unique substring)

  <TO>
          End node (label, slug, or unique substring)

Options:
  -h, --help
          Print help
```

## base graph move

```text
Move a subgraph between workspace graphs (rewrites the named-graph stamp, backs up both tiers, atomic with rollback). PREVIEW unless --yes

Usage: base graph move [OPTIONS] --select <SELECT> --to <TO>

Options:
      --select <SELECT>
          What to move: `node:<iri>`, `domain:<name>`, `prefix:<str>`, or a full node IRI

      --to <TO>
          Destination workspace name (registered in base.toml [[workspace]])

      --from <FROM>
          Source workspace name (defaults to the current workspace)

      --dry-run
          Preview the move plan; write nothing

      --no-ast
          Exclude AST entities (code# namespace + codemap/ pointers); regenerate at destination

      --yes
          Apply the move (without it, prints the plan and writes nothing)

  -h, --help
          Print help
```

## base secret

```text
Securely manage API keys / secrets in ~/.base-gbl/.env (echo-off, 0600). Plugins read these from their environment — never type secrets into chat

Usage: base secret <COMMAND>

Commands:
  set   Set a secret by prompting with echo OFF (masked, paste-friendly). Writes ~/.base-gbl/.env (0600). Never echoes the value
  list  List stored secret KEY names with masked values (never the full secret)
  rm    Remove a secret by key
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

## base secret set

```text
Set a secret by prompting with echo OFF (masked, paste-friendly). Writes ~/.base-gbl/.env (0600). Never echoes the value

Usage: base secret set <KEY>

Arguments:
  <KEY>
          The key name (e.g. GEMINI_API_KEY)

Options:
  -h, --help
          Print help
```

## base secret list

```text
List stored secret KEY names with masked values (never the full secret)

Usage: base secret list

Options:
  -h, --help
          Print help
```

## base secret rm

```text
Remove a secret by key

Usage: base secret rm <KEY>

Arguments:
  <KEY>
          The key name to remove

Options:
  -h, --help
          Print help
```
