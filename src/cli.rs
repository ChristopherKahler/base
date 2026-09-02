use clap::{Parser, Subcommand};

use base::command;
use base::config::BaseConfig;
use base::crud;
use base::domain;
use base::extension;
use base::hook;
use base::scope;

#[derive(Parser)]
#[command(
    name = "base",
    version,
    about = "BASE — Proactive context-injection engine for Claude Code",
    after_help = "Drop-in plugin commands (from extensions): run `base ext list`\n\n\
                  Built by Chris Kahler · Chris AI Systems\n\
                  Community & support: https://www.skool.com/claude-code-titans-9203\n\
                  Tutorials: https://www.youtube.com/@chris-ai-systems"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum HooksAction {
    /// Print the hook command table as JSON, for an installer outside base
    Manifest,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Handle Claude Code hook events (session-start, post-tool-use, user-prompt-submit)
    Hook {
        /// Hook event type
        event: String,
    },
    /// Publish base's hook wiring for an external installer (JSON)
    Hooks {
        #[command(subcommand)]
        action: HooksAction,
    },
    /// Query AST codebase graph (entities, calls, imports)
    #[command(visible_alias = "a")]
    Ast {
        #[command(subcommand)]
        action: AstAction,
    },
    /// Manage projects
    #[command(visible_alias = "p")]
    Project {
        #[command(subcommand)]
        action: ProjectAction,
    },
    /// Manage milestones (epics within a project)
    #[command(visible_alias = "m")]
    Milestone {
        #[command(subcommand)]
        action: MilestoneAction,
    },
    /// Manage tasks
    #[command(visible_alias = "t")]
    Task {
        #[command(subcommand)]
        action: TaskAction,
    },
    /// Log and search decisions
    #[command(visible_alias = "d")]
    Decision {
        /// Target the global tier (~/.base-gbl/) instead of workspace
        #[arg(long, short)]
        global: bool,
        #[command(subcommand)]
        action: DecisionAction,
    },
    /// Manage entities (people, organizations)
    #[command(visible_alias = "e")]
    Entity {
        #[command(subcommand)]
        action: EntityAction,
    },
    /// Manage goals
    #[command(visible_alias = "g")]
    Goal {
        #[command(subcommand)]
        action: GoalAction,
    },
    /// Manage reminders
    #[command(visible_alias = "r")]
    Reminder {
        #[command(subcommand)]
        action: ReminderAction,
    },
    /// Manage session handoffs (resume docs surfaced at session start)
    Handoff {
        /// Target the global tier (~/.base-gbl/) instead of workspace
        #[arg(long, short)]
        global: bool,
        #[command(subcommand)]
        action: HandoffAction,
    },
    /// Manage parallel side-work forks (build-specs surfaced at session start)
    Fork {
        /// Target the global tier (~/.base-gbl/) instead of workspace
        #[arg(long, short)]
        global: bool,
        #[command(subcommand)]
        action: ForkAction,
    },
    /// Sync file-owned data into the graph
    Sync {
        /// Only re-extract files changed since last sync
        #[arg(long)]
        incremental: bool,
        /// Run AST codebase extraction (tree-sitter, 35+ languages)
        #[arg(long)]
        ast: bool,
        /// Target directory for AST extraction (defaults to cwd)
        #[arg(long)]
        target: Option<String>,
        /// Unattended: proceed past the extractor's file-count safety threshold
        /// without asking (what the hooks pass — nobody is there to answer)
        #[arg(long)]
        yes: bool,
        /// Repair missing edges (backfill decision→domain, milestone→project, task→project links)
        #[arg(long)]
        repair: bool,
    },
    /// Manage domain matching rules
    Domain {
        #[command(subcommand)]
        action: DomainAction,
    },
    /// Manage context-triggered standards (MIDAS protocols injected on edit)
    #[command(visible_alias = "std")]
    Standards {
        #[command(subcommand)]
        action: StandardsAction,
    },
    /// Session-to-session message relay (parallel PAUL workers, Cadre firm members)
    Relay {
        #[command(subcommand)]
        action: RelayAction,
    },
    /// Graph-backed structured memory
    Learn {
        /// Target the global tier (~/.base-gbl/) instead of workspace
        #[arg(long, short)]
        global: bool,
        /// The memory text to store (required unless --mention, --remove, --update, or --list)
        #[arg(long)]
        text: Option<String>,
        /// Note type: insight, correction, decision, commitment, shift
        #[arg(long, default_value = "insight")]
        r#type: String,
        /// Link to a domain (required unless --mention)
        #[arg(long)]
        domain: Option<String>,
        /// Link to a project (optional additional edge)
        #[arg(long)]
        project: Option<String>,
        /// Link to an entity (optional additional edge)
        #[arg(long)]
        entity: Option<String>,
        /// Record a mention of an existing note (pass the slug)
        #[arg(long)]
        mention: Option<String>,
        /// Context for the mention
        #[arg(long)]
        context: Option<String>,
        /// Remove a note by slug
        #[arg(long)]
        remove: Option<String>,
        /// Update a note's text by slug (requires --text)
        #[arg(long)]
        update: Option<String>,
        /// List all notes (optionally filter by --type or --domain)
        #[arg(long)]
        list: bool,
    },
    /// Search notes by keyword, domain, or slug
    Recall {
        /// Search text in note content
        #[arg(long)]
        keyword: Option<String>,
        /// Filter by linked domain
        #[arg(long)]
        domain: Option<String>,
        /// Look up a specific note by slug
        #[arg(long)]
        slug: Option<String>,
    },
    /// Read the graph change log — every successful graph write, as JSON
    ///
    /// Cursor is a BYTE OFFSET into the log, not a sequence number: it needs no
    /// sidecar counter, survives concurrent appenders, and is what a reader
    /// resumes from directly.
    Changes {
        /// Target the global tier (~/.base-gbl/) instead of workspace
        #[arg(long, short)]
        global: bool,
        /// Print entries written after this byte offset
        #[arg(long)]
        since: Option<u64>,
        /// Print only the current end offset and exit
        #[arg(long)]
        cursor: bool,
    },
    /// Manage rules in the graph (add, list, remove)
    Rule {
        /// Target the global tier (~/.base-gbl/) instead of workspace
        #[arg(long, short)]
        global: bool,
        #[command(subcommand)]
        action: RuleAction,
    },
    /// Install base globally: build, symlink, create ~/.base-gbl, wire hooks, write manifest
    Install {
        /// Path to carl.json for decision migration
        #[arg(long)]
        carl: Option<String>,
        /// Skip hook wiring in settings.json
        #[arg(long)]
        skip_hooks: bool,
        /// Register all ChrisAI components (PAUL, SEED, SKILLSMITH) in manifest
        #[arg(long)]
        full: bool,
        /// Install the starter star commands without asking (*handoff, *fork, *base, *end)
        #[arg(long)]
        starter_commands: bool,
        /// Skip the starter star commands without asking
        #[arg(long, conflicts_with = "starter_commands")]
        no_starter_commands: bool,
    },
    /// Activate ChrisAI — enter your Skool classroom key to remove attribution
    Activate {
        /// Activation key from ChrisAI community
        key: String,
    },
    /// Self-update the base binary from public GitHub releases (or snooze the banner)
    Update {
        /// Re-validate + report whether a newer base is available, without installing
        #[arg(long)]
        check: bool,
        /// Install even when already on the latest version
        #[arg(long)]
        force: bool,
        /// Dismiss the update banner for 24 hours
        #[arg(long)]
        snooze: bool,
    },
    /// Uninstall base: remove hooks from settings.json, remove binary, remove CLAUDE.md section
    Uninstall {
        /// Also remove ~/.base-gbl/ global tier (destructive)
        #[arg(long)]
        purge: bool,
    },
    /// Launch the Command Center Dashboard (local web UI)
    #[command(visible_alias = "dash")]
    Dashboard {
        /// Port to serve on (default: 3741)
        #[arg(short, long, default_value = "3741")]
        port: u16,
    },
    /// Scaffold a new workspace: create .base/, write configs, register globally
    Scaffold {
        /// Target directory (defaults to cwd)
        path: Option<String>,
    },
    /// Reconcile project active/deferred state from real folder last-touch
    Reconcile {
        /// Preview what would change — no graph writes. Bypasses the [protocol] enabled gate.
        #[arg(long)]
        dry_run: bool,
    },
    /// Registered-workspace registry (sync CLAUDE.md from base.toml)
    Workspace {
        #[command(subcommand)]
        action: WorkspaceAction,
    },
    /// Operator identity profile (init, show)
    Operator {
        #[command(subcommand)]
        action: OperatorAction,
    },
    /// Manage extensions (list, validate, install, remove)
    #[command(visible_alias = "ext")]
    Extension {
        #[command(subcommand)]
        action: ExtensionAction,
    },
    /// List and inspect star commands (*BLUNT, *AUDIT, etc.)
    #[command(name = "commands", visible_alias = "cmd")]
    Command {
        #[command(subcommand)]
        action: CommandAction,
    },
    /// Manage graph-backed memory (migrate flat files, purge)
    Memory {
        #[command(subcommand)]
        action: MemoryAction,
    },
    /// Read and write base.toml configuration (dot-notation: section.key)
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Pull targeted graph context on demand (same engine as hook injection)
    Context {
        /// Text to match against domain triggers
        text: Option<String>,
        /// List all available context triggers
        #[arg(long)]
        list: bool,
    },
    /// Diagnose graph health across tiers (parser-independent). Exits nonzero when unhealthy.
    Doctor {
        /// Emit machine-readable JSON instead of the human report
        #[arg(long)]
        json: bool,
        /// Self-heal: quarantine malformed lines and atomically rewrite the good set (backs up first)
        #[arg(long)]
        repair: bool,
        /// Restore the workspace graph from a backup snapshot. Bare `--restore` lists snapshots.
        #[arg(long, num_args = 0..=1)]
        restore: Option<Option<String>>,
    },
    /// First-class graph maintenance (atomic, backs up first — never hand-edit graph.nq)
    Graph {
        #[command(subcommand)]
        action: GraphAction,
    },
    /// Securely manage API keys / secrets in ~/.base-gbl/.env (echo-off, 0600).
    /// Plugins read these from their environment — never type secrets into chat.
    Secret {
        #[command(subcommand)]
        action: SecretAction,
    },
    /// Post to, read from, or list the workspace's Slack — one bot token
    /// (`base secret set SLACK_BOT_TOKEN`), no MCP, works from any session
    Slack {
        #[command(subcommand)]
        action: SlackAction,
    },
    /// Drop-in command plugins from extensions (`base <foo>` → handler).
    /// Any unrecognized subcommand is captured here and routed to the plugin
    /// dispatcher (git's `git-foo` external-subcommand model). Core commands
    /// always resolve first — a plugin can never shadow a built-in.
    #[command(external_subcommand)]
    External(Vec<String>),
}

#[derive(Subcommand)]
pub enum SlackAction {
    /// Post a message. --to takes a channel id (C…), a #name, or a message
    /// permalink (replies in that thread)
    Post {
        #[arg(long)]
        to: String,
        /// The message (Slack mrkdwn)
        #[arg(long)]
        text: String,
        /// Reply in this thread (parent ts) instead of the channel
        #[arg(long)]
        thread: Option<String>,
    },
    /// Read recent messages from a channel, or a whole thread from a permalink
    Read {
        #[arg(long)]
        to: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// List the channels the app can see (id, #name, private, member)
    Channels,
}

#[derive(Subcommand)]
pub enum GraphAction {
    /// Dedup + canonicalize the workspace graph (atomic rewrite, snapshots first)
    Compact,
    /// Apply inbound fact ops (JSON on stdin) into the local graph.
    ///
    /// The pull half of desktop sync. Reads either a bare array of ops or
    /// `{"ops":[…]}`, applies every assert and retire in ONE transaction, and
    /// prints `{applied, skipped_duplicate, skipped_unknown}`. If any op is
    /// invalid it exits non-zero having applied nothing.
    ApplyOps {
        /// Apply into the global tier (~/.base-gbl) instead of the workspace
        #[arg(long)]
        global: bool,
    },
    /// Remove notes unread past --days (recency only). PREVIEW unless --apply.
    Purge {
        /// Required: select the stale-note rule (no other purge rules yet)
        #[arg(long)]
        stale: bool,
        /// Actually delete (default is a dry-run preview; snapshots before deleting)
        #[arg(long)]
        apply: bool,
        /// Unread-age threshold in days (a note's clock resets each time it's recalled)
        #[arg(long, default_value_t = 21)]
        days: i64,
    },
    /// LLM semantic extraction over a doc corpus → concepts + edges in the graph.
    /// Markdown-only by default; PDF/image/audio/video need multimodal enabled
    /// (`base config set multimodal.enabled true`, or one-shot --multimodal).
    Extract {
        /// Directory to extract (defaults to cwd)
        #[arg(short, long)]
        target: Option<String>,
        /// Claude Code model alias for extraction (e.g. haiku, sonnet, opus)
        #[arg(short, long)]
        model: Option<String>,
        /// Force multimodal ingest for this run (overrides config; bootstraps
        /// pdftotext/ffmpeg/whisper once if a non-markdown corpus needs them)
        #[arg(long)]
        multimodal: bool,
    },
    /// GraphRAG: answer a natural-language question over the graph (retrieve + synthesize)
    Query {
        /// The natural-language question
        question: String,
        /// Traversal depth
        #[arg(short, long, default_value_t = 3)]
        depth: usize,
        /// Token budget for the retrieved subgraph
        #[arg(short = 'b', long, default_value_t = 2000)]
        token_budget: usize,
        /// Claude Code model alias for synthesis
        #[arg(short, long)]
        model: Option<String>,
        /// Print the retrieved subgraph instead of a synthesized answer
        #[arg(long)]
        raw: bool,
    },
    /// Analyze emergent structure: god nodes, communities, surprising connections
    Analyze {
        /// How many of each to show
        #[arg(short = 'n', long, default_value_t = 10)]
        top_n: usize,
    },
    /// Agentic retrieval: full detail for one node (label, type, source, summary, edges)
    GetNode {
        /// Node label, concept slug, or unique substring
        node: String,
    },
    /// Agentic retrieval: the n-hop neighborhood of a node as edge lines
    Neighbors {
        /// Node label, concept slug, or unique substring
        node: String,
        /// Hops to expand
        #[arg(short, long, default_value_t = 1)]
        depth: usize,
    },
    /// Agentic retrieval: shortest path between two nodes
    Path {
        /// Start node (label, slug, or unique substring)
        from: String,
        /// End node (label, slug, or unique substring)
        to: String,
    },
    /// Move a subgraph between workspace graphs (rewrites the named-graph stamp,
    /// backs up both tiers, atomic with rollback). PREVIEW unless --yes.
    Move {
        /// What to move: `node:<iri>`, `domain:<name>`, `prefix:<str>`, or a full node IRI
        #[arg(long)]
        select: String,
        /// Destination workspace name (registered in base.toml [[workspace]])
        #[arg(long)]
        to: String,
        /// Source workspace name (defaults to the current workspace)
        #[arg(long)]
        from: Option<String>,
        /// Preview the move plan; write nothing
        #[arg(long)]
        dry_run: bool,
        /// Exclude AST entities (code# namespace + codemap/ pointers); regenerate at destination
        #[arg(long)]
        no_ast: bool,
        /// Apply the move (without it, prints the plan and writes nothing)
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub enum SecretAction {
    /// Set a secret by prompting with echo OFF (masked, paste-friendly). Writes
    /// ~/.base-gbl/.env (0600). Never echoes the value.
    Set {
        /// The key name (e.g. GEMINI_API_KEY)
        key: String,
    },
    /// List stored secret KEY names with masked values (never the full secret).
    List,
    /// Remove a secret by key.
    Rm {
        /// The key name to remove
        key: String,
    },
}

#[derive(Subcommand)]
pub enum WorkspaceAction {
    /// Regenerate the registered-workspaces block in ~/.claude/CLAUDE.md from base.toml
    Sync,
}

#[derive(Subcommand)]
pub enum OperatorAction {
    /// Create operator profile at ~/.base-gbl/operator.toml
    Init {
        #[arg(long)]
        name: String,
    },
    /// Show current operator profile
    Show,
}

#[derive(Subcommand)]
pub enum ExtensionAction {
    /// List all installed extensions
    List,
    /// Validate an extension manifest file
    Validate {
        /// Path to the TOML file to validate
        path: String,
    },
    /// Install an extension (copy validated TOML to extensions/)
    Install {
        /// Path to the TOML file to install
        path: String,
        /// Bundle the handler into ~/.base-gbl/plugins/<name>/ and repoint the
        /// manifest there — a self-contained, repo-independent (shippable) install.
        #[arg(long)]
        bundle: bool,
    },
    /// Fetch a plugin's prebuilt binary for THIS host from its GitHub release
    /// ([dist] block), verify the sha256, unpack + install — cross-platform, no
    /// toolchain. Falls back to a local source build when no host asset exists.
    Add {
        /// Path to the base-extension.toml (with a [dist] block) to fetch + install
        path: String,
    },
    /// Scaffold a new, conformant Bun cross-platform plugin. With --bootstrap, the
    /// one-command kickoff: writes the files, builds, git-inits, and creates+pushes
    /// a private GitHub repo — ready to develop, born cross-platform.
    Scaffold {
        /// Plugin/binary name — the `base <name>` command (lowercase, hyphens ok)
        name: String,
        /// Parent directory to create <name>-cli/ in (default: current dir)
        #[arg(long)]
        path: Option<String>,
        /// Exact target folder (new or empty) — overrides the default <name>-cli
        #[arg(long)]
        into: Option<String>,
        /// GitHub owner/repo for releases (default: ChristopherKahler/<name>-cli)
        #[arg(long)]
        repo: Option<String>,
        /// Run prepare.sh (bun build → bin/<name>) after writing files
        #[arg(long)]
        build: bool,
        /// git init + first commit
        #[arg(long)]
        git: bool,
        /// Create a private GitHub repo, wire origin, and push (implies --git)
        #[arg(long)]
        create_repo: bool,
        /// One-flag full kickoff: build + git + create-repo
        #[arg(long)]
        bootstrap: bool,
    },
    /// Remove an installed extension by name
    Remove {
        /// Extension name to remove
        name: String,
    },
    /// Run a drop-in plugin command explicitly (collision-proof):
    /// `base ext run <name> [args…]`
    Run {
        /// Plugin command name (as declared in an extension's [[commands]])
        name: String,
        /// Arguments forwarded verbatim to the handler
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand)]
pub enum CommandAction {
    /// List all configured star commands
    List,
    /// Show details for a specific star command
    Show {
        /// Command name (case-insensitive, without *)
        name: String,
    },
    /// Add a new star command to commands.toml
    Add {
        #[arg(long)]
        name: String,
        #[arg(long)]
        description: String,
        /// Rules (repeatable)
        #[arg(long)]
        rule: Vec<String>,
    },
    /// Remove a star command from commands.toml
    Remove {
        /// Command name (case-insensitive)
        name: String,
    },
    /// Import star commands from a commands.toml file (append-only; skips names already present, never alters preceding content)
    Import {
        /// Path to a commands.toml file to import (e.g. an Operator Modes pack)
        file: String,
    },
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Get a config value (dot-notation: section.key)
    Get {
        /// Config key (e.g. memory.mode, signal.enabled, flow.resurface)
        key: String,
    },
    /// Set a config value (dot-notation: section.key value)
    Set {
        /// Config key
        key: String,
        /// New value
        value: String,
    },
    /// List all config values
    List,
}

#[derive(Subcommand)]
pub enum MemoryAction {
    /// List Claude's flat-file memories for review (name, type, description, path)
    List,
    /// Remove flat-file memories that have been confirmed in the graph
    Purge,
}

#[derive(Subcommand)]
pub enum AstAction {
    /// Query AST graph for entities, calls, and imports
    #[command(visible_alias = "q")]
    Query {
        /// Find entities by name (case-insensitive substring match)
        #[arg(short, long)]
        contains: Option<String>,
        /// List all entities in a source file with relationships
        #[arg(short, long)]
        file: Option<String>,
        /// Find all callers of a named entity
        #[arg(long)]
        calls: Option<String>,
        /// Find all files that import from a given file
        #[arg(short, long)]
        imports: Option<String>,
        /// Query a specific app's map by path (e.g. apps/foo) instead of the cwd's map
        #[arg(short, long)]
        target: Option<String>,
    },
    /// List registered per-app code maps (name, entities, path, last synced)
    #[command(visible_alias = "l")]
    List,
    /// Make sure the app containing PATH has a code map: build one in the
    /// background if it has none, do nothing if it has (what the hooks do on
    /// first contact; the Windows hooks call this inside WSL for Linux paths)
    Ensure {
        /// A file or folder inside the app
        path: String,
        /// Build in the foreground and return when the map has landed (for a
        /// caller whose process must outlive the build, e.g. `wsl -e sh`)
        #[arg(long)]
        wait: bool,
    },
}

#[derive(Subcommand)]
pub enum ProjectAction {
    /// Add a new project
    #[command(visible_alias = "a")]
    Add {
        #[arg(short, long)]
        name: String,
        #[arg(short, long, default_value = "active")]
        status: String,
        /// Project path (workspace-relative). If omitted and [protocol] is enabled,
        /// the folder is derived from the protocol stage and auto-created.
        #[arg(short, long)]
        path: Option<String>,
        /// Protocol lifecycle stage the project starts in (default: first stage).
        #[arg(long)]
        stage: Option<String>,
    },
    /// List projects (defaults to the current workspace; cross-awareness via flags)
    #[command(visible_alias = "l")]
    List {
        /// Show projects from every registered workspace (today's flat union)
        #[arg(long)]
        all: bool,
        /// Show only projects homed in the named workspace
        #[arg(long)]
        workspace: Option<String>,
        /// Show only projects with no #path / no registered home
        #[arg(long)]
        unscoped: bool,
        /// Emit JSON (stable dashboard contract) instead of a table
        #[arg(long)]
        json: bool,
    },
    /// Show a specific project (accepts slug or display name)
    Get {
        slug: String,
        /// Emit JSON instead of the human field list
        #[arg(long)]
        json: bool,
    },
    /// Make a project also surface in another workspace (additive peerWorkspace edge)
    Peer {
        /// Project slug or display name
        slug: String,
        /// Workspace the project should also surface in
        #[arg(short, long)]
        workspace: String,
        /// Remove the peer edge instead of adding it
        #[arg(long)]
        remove: bool,
    },
    /// Re-point a project's folder path (graph + domain trigger) after it moves
    Repath {
        slug: String,
        /// New folder path (absolute, or relative to the workspace root)
        path: String,
    },
    /// Update a project (accepts slug or display name)
    #[command(visible_alias = "u")]
    Update {
        slug: String,
        #[arg(short, long)]
        status: Option<String>,
        #[arg(short, long)]
        blocked_by: Option<String>,
        #[arg(long)]
        next_action: Option<String>,
    },
    /// Re-home a project to another workspace graph (node + tasks + domain +
    /// decisions/rules/notes). AST regenerates at the destination. PREVIEW unless --yes.
    Move {
        /// Project slug or display name (in the current workspace)
        slug: String,
        /// Destination workspace name (registered in base.toml [[workspace]])
        #[arg(long)]
        to: String,
        /// Preview the move plan; write nothing
        #[arg(long)]
        dry_run: bool,
        /// Skip regenerating the AST map at the destination
        #[arg(long)]
        no_ast: bool,
        /// Apply the move (without it, prints the plan and writes nothing)
        #[arg(long)]
        yes: bool,
    },
    /// Delete a project. Refuses a non-empty project unless --force (which cascade-
    /// deletes tasks/milestones/decisions/rules). PREVIEW unless --yes.
    Delete {
        /// Project slug or display name
        slug: String,
        /// Cascade-delete child tasks/milestones/decisions/rules
        #[arg(long)]
        force: bool,
        /// Apply the delete (without it, prints the plan and writes nothing)
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub enum MilestoneAction {
    /// Add a milestone to a project
    #[command(visible_alias = "a")]
    Add {
        /// Project slug or display name
        #[arg(short, long)]
        project: String,
        #[arg(short, long)]
        name: String,
        #[arg(short, long)]
        description: Option<String>,
    },
    /// List milestones (optionally filtered by project)
    #[command(visible_alias = "l")]
    List {
        /// Project slug or display name
        #[arg(short, long)]
        project: Option<String>,
        /// Emit JSON (stable dashboard contract) instead of a table
        #[arg(long)]
        json: bool,
    },
    /// Show a specific milestone
    Get {
        slug: String,
        /// Emit JSON instead of the human field list
        #[arg(long)]
        json: bool,
    },
    /// Update a milestone
    #[command(visible_alias = "u")]
    Update {
        slug: String,
        #[arg(short, long)]
        status: Option<String>,
        #[arg(short, long)]
        description: Option<String>,
    },
    /// Delete a milestone. Tasks are DETACHED to project-level by default; --force
    /// cascade-deletes them. PREVIEW unless --yes.
    Delete {
        slug: String,
        /// Cascade-delete the milestone's tasks instead of detaching them
        #[arg(long)]
        force: bool,
        /// Apply the delete (without it, prints the plan and writes nothing)
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub enum TaskAction {
    /// Add a task to a project (optionally under a milestone)
    #[command(visible_alias = "a")]
    Add {
        /// Project slug or display name
        #[arg(short, long)]
        project: String,
        #[arg(short, long)]
        name: String,
        #[arg(long)]
        priority: Option<String>,
        /// Milestone slug to group this task under
        #[arg(short, long)]
        milestone: Option<String>,
    },
    /// List tasks (filter by project, milestone, or label)
    #[command(visible_alias = "l")]
    List {
        /// Project slug or display name
        #[arg(short, long)]
        project: Option<String>,
        /// Milestone slug to filter by
        #[arg(short, long)]
        milestone: Option<String>,
        /// Only tasks carrying ALL of these labels (repeatable)
        #[arg(long)]
        label: Vec<String>,
        /// Emit JSON (stable dashboard contract) instead of a table
        #[arg(long)]
        json: bool,
    },
    /// Show a specific task (all fields; accepts slug or display name)
    Get {
        slug: String,
        /// Emit JSON instead of the human field list
        #[arg(long)]
        json: bool,
    },
    /// Update a task's mutable fields (accepts slug or display name)
    #[command(visible_alias = "u")]
    Update {
        slug: String,
        /// New display name
        #[arg(long)]
        name: Option<String>,
        /// Status (canonical vocabulary: active | completed)
        #[arg(short, long)]
        status: Option<String>,
        #[arg(long)]
        priority: Option<String>,
        /// Free-form description / notes
        #[arg(long, visible_alias = "notes")]
        description: Option<String>,
        #[arg(long)]
        assignee: Option<String>,
        /// Due date (free-form or ISO)
        #[arg(long)]
        due: Option<String>,
        /// Reassign to another project (rewrites the project edge only)
        #[arg(short, long)]
        project: Option<String>,
        /// Reassign to another milestone (rewrites the milestone edge only)
        #[arg(short, long)]
        milestone: Option<String>,
    },
    /// Delete a task node + its edges. PREVIEW unless --yes.
    Delete {
        slug: String,
        /// Apply the delete (without it, prints the task and writes nothing)
        #[arg(long)]
        yes: bool,
    },
    /// Mark a task as completed
    Done { slug: String },
    /// Attach/detach free-form labels on a task (the dashboard's tagging facet)
    Tag {
        slug: String,
        /// Label to attach (repeatable, idempotent)
        #[arg(long = "add")]
        add: Vec<String>,
        /// Label to detach (repeatable)
        #[arg(long = "remove")]
        remove: Vec<String>,
    },
}

#[derive(Subcommand)]
pub enum DecisionAction {
    /// Log a new decision
    Log {
        #[arg(long)]
        domain: String,
        #[arg(long)]
        decision: String,
        #[arg(long)]
        rationale: String,
        #[arg(long)]
        recall: Option<String>,
    },
    /// Search decisions by keyword
    Search {
        #[arg(long)]
        keyword: String,
        /// Emit JSON (stable dashboard contract) instead of a table
        #[arg(long)]
        json: bool,
    },
    /// Delete decisions matching a keyword
    Delete {
        /// Keyword to match against decision names
        #[arg(long)]
        keyword: String,
    },
    /// Update a decision in place, addressed by its stable {domain}.{decision} slug
    #[command(visible_alias = "u")]
    Update {
        /// Decision slug ({domain}.{decision}) or exact decision text
        slug: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        rationale: Option<String>,
        #[arg(long)]
        recall: Option<String>,
        #[arg(short, long)]
        status: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum EntityAction {
    /// Add an entity (person or organization) — must link to at least one domain or project
    Add {
        #[arg(long)]
        name: String,
        /// Type: person, organization
        #[arg(long, name = "type", default_value = "person")]
        entity_type: String,
        /// Domain this entity relates to (REQUIRED — prevents orphan entities)
        #[arg(long)]
        domain: String,
        /// Project this entity relates to (optional additional edge)
        #[arg(long)]
        project: Option<String>,
    },
    /// List all entities
    List,
    /// Show a specific entity (accepts slug or display name)
    Get { slug: String },
    /// Update an entity (accepts slug or display name)
    Update {
        slug: String,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        description: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum GoalAction {
    /// Add a goal
    Add {
        #[arg(long)]
        name: String,
        #[arg(long)]
        target: String,
    },
    /// List all goals
    List,
    /// Update a goal (accepts slug or display name)
    Update {
        slug: String,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        target: Option<String>,
    },
}

/// Parse a relative duration like "30s", "3m", "2h", "1d".
fn parse_duration(s: &str) -> anyhow::Result<chrono::Duration> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.len().saturating_sub(1));
    let n: i64 = num
        .parse()
        .map_err(|_| anyhow::anyhow!("bad duration number in '{s}'"))?;
    Ok(match unit {
        "s" => chrono::Duration::seconds(n),
        "m" => chrono::Duration::minutes(n),
        "h" => chrono::Duration::hours(n),
        "d" => chrono::Duration::days(n),
        _ => anyhow::bail!("duration unit must be s/m/h/d (e.g. 3m)"),
    })
}

/// Resolve a reminder's surface time (ISO-8601 string) from --at, --in, or --due.
fn resolve_surface_at(
    due: Option<&str>,
    at: Option<&str>,
    in_dur: Option<&str>,
) -> anyhow::Result<String> {
    if let Some(a) = at {
        let dt = chrono::DateTime::parse_from_rfc3339(a)
            .map_err(|_| anyhow::anyhow!("--at must be ISO-8601 (e.g. 2026-06-23T14:30:00-05:00)"))?;
        Ok(dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, false))
    } else if let Some(d) = in_dur {
        Ok((chrono::Local::now() + parse_duration(d)?)
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, false))
    } else if let Some(due) = due {
        let off = chrono::Local::now().format("%:z").to_string();
        Ok(format!("{due}T00:00:00{off}"))
    } else {
        anyhow::bail!("provide one of --in <3m|2h|1d>, --at <ISO datetime>, or --due <YYYY-MM-DD>")
    }
}

#[derive(Subcommand)]
pub enum ReminderAction {
    /// Add a reminder (provide one of --in, --at, or --due)
    Add {
        #[arg(long)]
        name: String,
        /// Due date (YYYY-MM-DD) — surfaces on/after this date
        #[arg(long)]
        due: Option<String>,
        /// Exact surface time, ISO-8601 (e.g. 2026-06-23T14:30:00-05:00)
        #[arg(long)]
        at: Option<String>,
        /// Relative surface time from now: 30s, 3m, 2h, 1d
        #[arg(long = "in")]
        in_dur: Option<String>,
    },
    /// List all reminders
    List,
    /// Remove a reminder (hard delete)
    Remove { slug: String },
}

#[derive(Subcommand)]
pub enum HandoffAction {
    /// Register a handoff doc (archives any prior open handoff for the project)
    Create {
        #[arg(long)]
        project: String,
        #[arg(long)]
        doc: String,
        /// Graph slug / title to summon it by (default: doc basename)
        #[arg(long)]
        slug: Option<String>,
    },
    /// List handoffs across global + workspace tiers
    List,
    /// Snooze a handoff for N days (hide until then)
    Snooze { slug: String, days: i64 },
    /// Archive a handoff (stop resurfacing)
    Archive { slug: String },
}

#[derive(Subcommand)]
pub enum ForkAction {
    /// Register a fork build-spec (additive — does not archive sibling forks)
    Create {
        #[arg(long)]
        project: String,
        #[arg(long)]
        doc: String,
        /// Graph slug / title to summon it by (default: doc basename)
        #[arg(long)]
        slug: Option<String>,
    },
    /// List forks across global + workspace tiers
    List,
    /// Snooze a fork for N days (hide until then)
    Snooze { slug: String, days: i64 },
    /// Archive a fork (stop resurfacing)
    Archive { slug: String },
}

#[derive(Subcommand)]
pub enum RuleAction {
    /// Add a rule to a domain in the graph
    Add {
        #[arg(long)]
        domain: String,
        #[arg(long)]
        text: String,
        /// Optional rationale — injected as "rule — because rationale" (Phase 26)
        #[arg(long)]
        rationale: Option<String>,
    },
    /// List rules for a domain from the graph
    List {
        #[arg(long)]
        domain: String,
    },
    /// Remove a rule by index from a domain
    Remove {
        #[arg(long)]
        domain: String,
        #[arg(long)]
        index: u32,
    },
}

#[derive(Subcommand)]
pub enum DomainAction {
    /// Add a keyword or path trigger to a domain
    AddTrigger {
        #[arg(long)]
        domain: String,
        #[arg(long)]
        keyword: Option<String>,
        #[arg(long)]
        path: Option<String>,
    },
    /// List all configured domains
    List,
    /// Show a specific domain's full configuration
    Get { name: String },
    /// Sync domains/rules from domains.toml into the graph. Optionally migrate decisions from carl.json.
    Sync {
        /// Path to carl.json for one-time decision migration
        #[arg(long)]
        carl: Option<String>,
    },
    /// Create a new domain in domains.toml
    Create {
        #[arg(long)]
        name: String,
        #[arg(long)]
        keyword: Option<String>,
        #[arg(long)]
        path: Option<String>,
    },
    /// Remove a domain from domains.toml
    Remove {
        /// Domain name (case-insensitive)
        name: String,
    },
    /// Remove a keyword or path trigger from a domain
    RemoveTrigger {
        #[arg(long)]
        domain: String,
        #[arg(long)]
        keyword: Option<String>,
        #[arg(long)]
        path: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum StandardsAction {
    /// Sync MIDAS protocols.md → standards.toml + graph Standard entities
    Sync {
        /// Override the protocols.md source path
        #[arg(long)]
        source: Option<String>,
    },
    /// List all standards with trigger/annotation counts
    List,
    /// Show a standard's full config
    Get { id: String },
    /// Dry-run the matcher against a file — scores + what would inject
    Test {
        file: String,
        /// Extra content included in the haystack (simulates an edit payload)
        #[arg(long)]
        content: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum RelayAction {
    /// Create the ephemeral relay store for a project
    Init {
        #[arg(long)]
        project: String,
    },
    /// Register (or re-bind) this session under a stable title
    Register {
        /// Stable identity: worker-phase-11, quill, orchestrator…
        #[arg(long = "as")]
        title: String,
        /// Session id override (defaults to CLAUDE_CODE_SESSION_ID)
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        phase: Option<String>,
        #[arg(long)]
        project: Option<String>,
    },
    /// Send a message to a session, title, phase, or all
    Send {
        /// Recipient: title | session-id | phase:<n> | all
        #[arg(long)]
        to: String,
        /// claim|release|notify|unblock|contract-change|ready-to-merge|question|answer
        #[arg(long = "type")]
        mtype: String,
        #[arg(long)]
        msg: String,
        /// Sender override (defaults to this session's registered title)
        #[arg(long)]
        from: Option<String>,
        /// File paths / phase ids this message references
        #[arg(long)]
        refs: Vec<String>,
        #[arg(long)]
        project: Option<String>,
    },
    /// Non-blocking read of pending messages (consumes them)
    Poll {
        /// Read for a specific title (defaults to this session's identity)
        #[arg(long = "for")]
        for_title: Option<String>,
        /// Peek without consuming
        #[arg(long)]
        peek: bool,
        #[arg(long)]
        project: Option<String>,
    },
    /// BLOCK until a matching message arrives — burns zero session tokens
    Wait {
        /// Only messages from this sender
        #[arg(long)]
        from: Option<String>,
        /// Only messages of this type
        #[arg(long = "type")]
        mtype: Option<String>,
        /// Timeout in seconds
        #[arg(long, default_value = "300")]
        timeout: u64,
        /// Wait as a specific title (defaults to this session's identity)
        #[arg(long = "for")]
        for_title: Option<String>,
        #[arg(long)]
        project: Option<String>,
    },
    /// Take an advisory claim on a path or phase (TTL-bounded)
    Claim {
        resource: String,
        #[arg(long, default_value = "")]
        note: String,
        /// TTL in seconds
        #[arg(long, default_value = "3600")]
        ttl: i64,
        #[arg(long)]
        project: Option<String>,
    },
    /// Release a claim
    Release {
        resource: String,
        /// Operator force-release of another session's claim
        #[arg(long)]
        force: bool,
        #[arg(long)]
        project: Option<String>,
    },
    /// Operator view: sessions, liveness, claims, pending messages
    Board {
        #[arg(long)]
        project: Option<String>,
    },
    /// Export the spool as inbox.nq (read-only graph snapshot)
    Export {
        #[arg(long)]
        project: Option<String>,
    },
    /// End-of-milestone teardown — the store is disposable by design
    Dispose {
        #[arg(long)]
        project: String,
        /// Actually delete (without this, prints what would be removed)
        #[arg(long)]
        force: bool,
    },
    /// Relay a briefed task to a live titled session. It auto-fires in that
    /// session's hooks (loud) until picked up — cross-workspace via the global tier.
    Task {
        /// Target session's registered title (set with: base relay register --as <title>)
        #[arg(long)]
        to: String,
        /// Task slug — kebab-case, matches the briefing doc basename
        #[arg(long)]
        slug: String,
        /// One-line summary shown in the alert
        #[arg(long)]
        summary: String,
        /// Absolute path to the full briefing doc the receiver should read
        #[arg(long)]
        doc: Option<String>,
        /// Priority: high | medium (default: high)
        #[arg(long)]
        priority: Option<String>,
        /// Origin label shown to the receiver (defaults to this session's title)
        #[arg(long)]
        from: Option<String>,
    },
    /// Instant message to a live titled session — no doc, no done-ceremony.
    /// Screams in the receiver's hooks mid-turn; their reply ping clears it.
    Ping {
        /// Target session's registered title
        #[arg(long)]
        to: String,
        /// The message — carries ALL context inline (a sentence or three; more than that is a task, not a ping)
        #[arg(long)]
        msg: String,
        /// File paths / entity ids this ping references
        #[arg(long)]
        refs: Vec<String>,
        /// Origin label (defaults to this session's registered/auto-assigned title)
        #[arg(long)]
        from: Option<String>,
    },
    /// Mark a relayed task done — clears the inbox alert and closes the graph mirror
    Done {
        /// Task slug
        slug: String,
    },
    /// List inbound relay tasks across all live sessions
    Tasks,
    /// List titled sessions in the global registry (liveness for `*task` targets)
    Sessions,
}

/// Resolve a user identifier (slug, display name, or mixed) to a canonical slug.
/// Prints error and returns None on failure.
fn resolve(cwd: &std::path::Path, ns: &base::config::NamespaceConfig, entity_type: &str, input: &str) -> Option<String> {
    match crud::resolve_slug(cwd, ns, entity_type, input) {
        Ok(slug) => Some(slug),
        Err(e) => {
            eprintln!("{e}");
            None
        }
    }
}

/// Print a CLI error to stderr and exit nonzero.
///
/// Graph-backed commands route their error paths through this so a corrupt or
/// unparseable graph fails LOUD (nonzero exit) instead of the old silent exit-0
/// — the 2026-06-18 incident where `base learn` / `base sync` printed
/// "Failed to parse graph from <path>" but still returned 0, hiding the
/// corruption for hours. Hooks deliberately do NOT use this: they stay
/// fail-open (exit 0) and surface corruption via the session-start warning
/// block instead (see hook::session_start). `{e:#}` prints the full anyhow
/// context chain so the underlying parse error is visible.
fn die(prefix: &str, e: impl std::fmt::Display) -> ! {
    eprintln!("{prefix}: {e:#}");
    std::process::exit(1);
}

/// Which tier a write targets: `-g/--global` swaps cwd for `~/.base-gbl`, so
/// the global tier is something you opt into rather than something you land in
/// (issue #8). Without the flag, tier-bound writes resolve from cwd and fail
/// loudly outside a workspace instead of silently discarding.
fn tier_cwd(cwd: &std::path::Path, global: bool) -> std::path::PathBuf {
    if !global {
        return cwd.to_path_buf();
    }
    match base::home::home_root() {
        Some(h) => h.join(".base-gbl"),
        None => die("Failed", "cannot determine home directory for --global"),
    }
}

pub fn run() {
    let cli = Cli::parse();
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: cannot determine current directory: {e}");
            std::process::exit(1);
        }
    };
    let config = BaseConfig::load(&cwd);

    match cli.command {
        Some(Commands::Hook { event }) => hook::dispatch(&event),

        // ─── Hooks manifest ─────────────────────────────────
        // Machine-readable only: stdout is one JSON object, because the sole
        // consumer is another installer parsing it.
        Some(Commands::Hooks { action }) => match action {
            HooksAction::Manifest => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&base::install::hooks_manifest())
                        .unwrap_or_else(|_| "{}".into())
                );
            }
        },

        // ─── AST Query ──────────────────────────────────
        Some(Commands::Ast { action }) => match action {
            AstAction::Query { contains, file, calls, imports, target } => {
                // --target lets the parent query a specific app's map: resolve to
                // that dir so find_ast_ttl walks up to its .base-ast/ast.ttl.
                let qcwd = match &target {
                    Some(t) => {
                        let p = std::path::Path::new(t);
                        if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) }
                    }
                    None => cwd.clone(),
                };
                if let Some(name) = contains {
                    if let Err(e) = crud::ast_query::contains(&qcwd, &config.namespace, &name) { die("Error", e); }
                } else if let Some(path) = file {
                    if let Err(e) = crud::ast_query::file(&qcwd, &config.namespace, &path) { die("Error", e); }
                } else if let Some(name) = calls {
                    if let Err(e) = crud::ast_query::calls(&qcwd, &config.namespace, &name) { die("Error", e); }
                } else if let Some(path) = imports {
                    if let Err(e) = crud::ast_query::imports(&qcwd, &config.namespace, &path) { die("Error", e); }
                } else {
                    eprintln!("Provide one of: --contains, --file, --calls, --imports");
                }
            }
            AstAction::List => {
                if let Err(e) = crud::ast_query::list(&cwd, &config.namespace) { die("Error", e); }
            }
            AstAction::Ensure { path, wait } => {
                let p = std::path::Path::new(&path);
                let abs = if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) };
                use base::hook::automap::{first_contact, first_contact_wait, MapPlan};
                let outcome = if wait { first_contact_wait(&abs) } else { first_contact(&abs) };
                match outcome {
                    None if base::config::ast_app_root(&abs).is_none() => println!("{}: not inside an app (no .git / .paul / .base-ast / .base above it)", abs.display()),
                    None => println!("{}: already mapped", abs.display()),
                    Some(MapPlan::Build) if wait => println!("{}: built", abs.display()),
                    Some(MapPlan::Build) => println!("{}: no map yet — building in the background", abs.display()),
                    Some(MapPlan::Debounced) => println!("{}: a build is already in flight", abs.display()),
                    Some(MapPlan::SkipHome) => println!("{}: the home directory is never mapped", abs.display()),
                    Some(MapPlan::SkipHub) => println!("{}: a workspace of apps — each app maps itself", abs.display()),
                    Some(MapPlan::Refresh) => println!("{}: refreshing", abs.display()),
                }
            }
        },

        // ─── Project ─────────────────────────────────────
        Some(Commands::Project { action }) => match action {
            ProjectAction::Add { name, status, path, stage } => {
                let slug = crud::slugify(&name);
                // Explicit --path wins; otherwise the protocol provisions the folder.
                let provisioned = if path.is_none() {
                    match crud::project::provision_folder(&cwd, &config.protocol, &name, &slug, stage.as_deref()) {
                        Ok(v) => v,
                        Err(e) => die("Failed", e),
                    }
                } else {
                    None
                };
                let resolved_stage = provisioned.as_ref().map(|(_, s)| s.clone()).or(stage);
                let resolved_path = path.or_else(|| provisioned.as_ref().map(|(f, _)| f.clone()));
                match resolved_path {
                    Some(rp) => match crud::project::add_with_stage(&cwd, &config.namespace, &name, &status, Some(&rp), resolved_stage.as_deref()) {
                        Ok(slug) => {
                            println!("Project '{name}' created (slug: {slug}, path: {rp})");
                            // A registered project is an app: its code map starts
                            // now, not at the next session start.
                            let folder = {
                                let p = std::path::Path::new(&rp);
                                if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) }
                            };
                            if let base::hook::automap::RootPlan::Marked(root) | base::hook::automap::RootPlan::Adopt(root) =
                                base::hook::automap::session_root(&folder)
                                && base::hook::automap::ensure_first_map(&root) == Some(base::hook::automap::MapPlan::Build)
                            {
                                println!("   Code map: building in the background → {}/.base-ast/", root.display());
                            }
                        }
                        Err(e) => die("Failed", e),
                    },
                    None => die("Failed", anyhow::anyhow!("--path is required, or enable [protocol] with a stage in base.toml")),
                }
            }
            ProjectAction::List { all, workspace, unscoped, json } => {
                // Precedence: --all > --workspace > --unscoped > default (current workspace).
                let project_scope = if all {
                    scope::ProjectScope::All
                } else if let Some(w) = workspace {
                    scope::ProjectScope::Workspace(crud::slugify(&w))
                } else if unscoped {
                    scope::ProjectScope::Unscoped
                } else if config.signal.scope == "global" {
                    scope::ProjectScope::All // [signal] scope = "global" restores the flat union
                } else {
                    scope::ProjectScope::Current
                };
                let r = if json {
                    crud::project::list_json(&cwd, &config, project_scope)
                } else {
                    crud::project::list(&cwd, &config, project_scope)
                };
                if let Err(e) = r { die("Error", e); }
            }
            ProjectAction::Get { slug, json } => {
                if let Some(s) = resolve(&cwd, &config.namespace, "project", &slug) {
                    let r = if json {
                        crud::project::get_json(&cwd, &config.namespace, &s)
                    } else {
                        crud::project::get(&cwd, &config.namespace, &s)
                    };
                    if let Err(e) = r { die("Error", e); }
                }
            }
            ProjectAction::Peer { slug, workspace, remove } => {
                if let Some(s) = resolve(&cwd, &config.namespace, "project", &slug)
                    && let Err(e) = crud::project::peer(&cwd, &config, &s, &workspace, remove) { die("Error", e); }
            }
            ProjectAction::Repath { slug, path } => {
                if let Some(s) = resolve(&cwd, &config.namespace, "project", &slug) {
                    match crud::project::repath(&cwd, &config.namespace, &s, &path) {
                        Ok(r) => {
                            let from = r.old_path.as_deref().unwrap_or("(none)");
                            let dom = if r.domain_changed { format!(", domain '{}' trigger updated", r.name) } else { String::new() };
                            println!("Repathed '{s}': {from} → {}{dom}", r.new_path);
                        }
                        Err(e) => die("Failed", e),
                    }
                }
            }
            ProjectAction::Update { slug, status, blocked_by, next_action } => {
                if let Some(s) = resolve(&cwd, &config.namespace, "project", &slug) {
                    match crud::project::update(&cwd, &config.namespace, &s, status.as_deref(), blocked_by.as_deref(), next_action.as_deref()) {
                        Ok(()) => println!("Project '{s}' updated"),
                        Err(e) => die("Failed", e),
                    }
                }
            }
            ProjectAction::Move { slug, to, dry_run, no_ast, yes } => {
                if let Some(s) = resolve(&cwd, &config.namespace, "project", &slug) {
                    // The destructive remove-from-source requires --yes; otherwise preview.
                    let preview_only = dry_run || !yes;
                    match crud::project::move_project(&cwd, &config, &s, &to, preview_only) {
                        Ok(report) => {
                            print!("{}", base::graph_move::format_report(&report));
                            if report.applied && report.moved_lines > 0 {
                                if no_ast {
                                    println!("   AST regeneration skipped (--no-ast).");
                                } else {
                                    println!("   Next: run `base sync --ast` from '{to}' to rebuild {s}'s code map.");
                                }
                            } else if preview_only && !dry_run {
                                println!("   Pass --yes to apply.");
                            }
                        }
                        Err(e) => die("project move failed", e),
                    }
                }
            }
            ProjectAction::Delete { slug, force, yes } => {
                if let Some(s) = resolve(&cwd, &config.namespace, "project", &slug) {
                    match crud::project::delete_plan(&cwd, &config.namespace, &s) {
                        Ok(plan) if !plan.exists => {
                            eprintln!("Project '{s}' not found in this workspace graph.");
                        }
                        Ok(plan) => {
                            println!(
                                "Project '{s}': {} node(s) would be removed ({} child node(s)).",
                                plan.subjects.len(),
                                plan.children
                            );
                            if plan.children > 0 && !force {
                                println!("   Non-empty — pass --force to cascade-delete the children, plus --yes to apply.");
                            } else if !yes {
                                println!("   Pass --yes to apply.");
                            } else {
                                match crud::project::delete(&cwd, &config.namespace, &s, force) {
                                    Ok(n) => println!("Deleted project '{s}' ({n} node(s) removed)."),
                                    Err(e) => die("Failed", e),
                                }
                            }
                        }
                        Err(e) => die("Error", e),
                    }
                }
            }
        },

        // ─── Milestone ──────────────────────────────────
        Some(Commands::Milestone { action }) => match action {
            MilestoneAction::Add { project, name, description } => {
                let ps = match resolve(&cwd, &config.namespace, "project", &project) {
                    Some(s) => s,
                    None => return,
                };
                match crud::milestone::add(&cwd, &config.namespace, &ps, &name, description.as_deref()) {
                    Ok(slug) => println!("Milestone '{name}' created (slug: {slug})"),
                    Err(e) => die("Failed", e),
                }
            }
            MilestoneAction::List { project, json } => {
                let ps = match project.as_deref() {
                    Some(p) => match resolve(&cwd, &config.namespace, "project", p) {
                        Some(s) => Some(s),
                        None => return,
                    },
                    None => None,
                };
                let r = if json {
                    crud::milestone::list_json(&cwd, &config.namespace, ps.as_deref())
                } else {
                    crud::milestone::list(&cwd, &config.namespace, ps.as_deref())
                };
                if let Err(e) = r { die("Error", e); }
            }
            MilestoneAction::Get { slug, json } => {
                if let Some(s) = resolve(&cwd, &config.namespace, "milestone", &slug) {
                    let r = if json {
                        crud::milestone::get_json(&cwd, &config.namespace, &s)
                    } else {
                        crud::milestone::get(&cwd, &config.namespace, &s)
                    };
                    if let Err(e) = r { die("Error", e); }
                }
            }
            MilestoneAction::Update { slug, status, description } => {
                if let Some(s) = resolve(&cwd, &config.namespace, "milestone", &slug) {
                    match crud::milestone::update(&cwd, &config.namespace, &s, status.as_deref(), description.as_deref()) {
                        Ok(()) => println!("Milestone '{s}' updated"),
                        Err(e) => die("Failed", e),
                    }
                }
            }
            MilestoneAction::Delete { slug, force, yes } => {
                if let Some(s) = resolve(&cwd, &config.namespace, "milestone", &slug) {
                    let n = match crud::milestone::task_count(&cwd, &config.namespace, &s) {
                        Ok(n) => n,
                        Err(e) => die("Error", e),
                    };
                    let fate = if force { "cascade-deleted" } else { "detached to project-level" };
                    println!("Milestone '{s}': {n} task(s) will be {fate}.");
                    if !yes {
                        println!("   Pass --yes to apply.");
                    } else {
                        match crud::milestone::delete(&cwd, &config.namespace, &s, force) {
                            Ok(removed) => println!("Deleted milestone '{s}' ({removed} task(s) cascade-deleted)."),
                            Err(e) => die("Failed", e),
                        }
                    }
                }
            }
        },

        // ─── Task ────────────────────────────────────────
        Some(Commands::Task { action }) => match action {
            TaskAction::Add { project, name, priority, milestone } => {
                let ps = match resolve(&cwd, &config.namespace, "project", &project) {
                    Some(s) => s,
                    None => return,
                };
                let ms = match milestone.as_deref() {
                    Some(m) => match resolve(&cwd, &config.namespace, "milestone", m) {
                        Some(s) => Some(s),
                        None => return,
                    },
                    None => None,
                };
                match crud::task::add(&cwd, &config.namespace, &ps, &name, priority.as_deref(), ms.as_deref()) {
                    Ok(slug) => println!("Task '{name}' created (slug: {slug})"),
                    Err(e) => die("Failed", e),
                }
            }
            TaskAction::List { project, milestone, label, json } => {
                let ps = match project.as_deref() {
                    Some(p) => match resolve(&cwd, &config.namespace, "project", p) {
                        Some(s) => Some(s),
                        None => return,
                    },
                    None => None,
                };
                let ms = match milestone.as_deref() {
                    Some(m) => match resolve(&cwd, &config.namespace, "milestone", m) {
                        Some(s) => Some(s),
                        None => return,
                    },
                    None => None,
                };
                let r = if json {
                    crud::task::list_json(&cwd, &config.namespace, ps.as_deref(), ms.as_deref(), &label)
                } else {
                    crud::task::list(&cwd, &config.namespace, ps.as_deref(), ms.as_deref(), &label)
                };
                if let Err(e) = r { die("Error", e); }
            }
            TaskAction::Get { slug, json } => {
                if let Some(s) = resolve(&cwd, &config.namespace, "task", &slug) {
                    let r = if json {
                        crud::task::get_json(&cwd, &config.namespace, &s)
                    } else {
                        crud::task::get(&cwd, &config.namespace, &s)
                    };
                    if let Err(e) = r { die("Error", e); }
                }
            }
            TaskAction::Update { slug, name, status, priority, description, assignee, due, project, milestone } => {
                if let Some(s) = resolve(&cwd, &config.namespace, "task", &slug) {
                    // Resolve reassignment targets (slug or display name) up front.
                    let proj = match project.as_deref() {
                        Some(p) => match resolve(&cwd, &config.namespace, "project", p) {
                            Some(x) => Some(x),
                            None => return,
                        },
                        None => None,
                    };
                    let ms = match milestone.as_deref() {
                        Some(m) => match resolve(&cwd, &config.namespace, "milestone", m) {
                            Some(x) => Some(x),
                            None => return,
                        },
                        None => None,
                    };
                    match crud::task::update(
                        &cwd, &config.namespace, &s,
                        name.as_deref(), status.as_deref(), priority.as_deref(),
                        description.as_deref(), assignee.as_deref(), due.as_deref(),
                        proj.as_deref(), ms.as_deref(),
                    ) {
                        Ok(()) => println!("Task '{s}' updated"),
                        Err(e) => die("Failed", e),
                    }
                }
            }
            TaskAction::Delete { slug, yes } => {
                if let Some(s) = resolve(&cwd, &config.namespace, "task", &slug) {
                    if yes {
                        match crud::task::delete(&cwd, &config.namespace, &s) {
                            Ok(()) => println!("Deleted task '{s}'."),
                            Err(e) => die("Failed", e),
                        }
                    } else {
                        match crud::task::get_data(&cwd, &config.namespace, &s) {
                            Ok(Some(t)) => {
                                println!("Task '{}' (status: {}) will be deleted.", t.id, t.status);
                                println!("   Pass --yes to apply.");
                            }
                            Ok(None) => eprintln!("Task '{s}' not found."),
                            Err(e) => die("Error", e),
                        }
                    }
                }
            }
            TaskAction::Done { slug } => {
                if let Some(s) = resolve(&cwd, &config.namespace, "task", &slug) {
                    match crud::task::done(&cwd, &config.namespace, &s) {
                        Ok(()) => println!("Task '{s}' completed"),
                        Err(e) => die("Failed", e),
                    }
                }
            }
            TaskAction::Tag { slug, add, remove } => {
                if let Some(s) = resolve(&cwd, &config.namespace, "task", &slug) {
                    match crud::task::tag(&cwd, &config.namespace, &s, &add, &remove) {
                        Ok(()) => println!("Task '{s}' labels updated (+{} -{})", add.len(), remove.len()),
                        Err(e) => die("Failed", e),
                    }
                }
            }
        },

        // ─── Decision ────────────────────────────────────
        Some(Commands::Decision { global, action }) => {
            let cwd = tier_cwd(&cwd, global);
            match action {
                DecisionAction::Log { domain, decision, rationale, recall } => {
                    match crud::decision::log(&cwd, &config.namespace, &domain, &decision, &rationale, recall.as_deref()) {
                        Ok(slug) => println!("Decision logged (slug: {slug})"),
                        Err(e) => die("Failed", e),
                    }
                }
                DecisionAction::Search { keyword, json } => {
                    let r = if json {
                        crud::decision::search_json(&cwd, &config.namespace, &keyword)
                    } else {
                        crud::decision::search(&cwd, &config.namespace, &keyword)
                    };
                    if let Err(e) = r { die("Error", e); }
                }
                DecisionAction::Delete { keyword } => {
                    // Show what will be deleted first
                    if let Err(e) = crud::decision::search(&cwd, &config.namespace, &keyword) {
                        die("Error", e);
                    } else {
                        match crud::decision::delete(&cwd, &config.namespace, &keyword) {
                            Ok(0) => println!("No decisions matching '{keyword}'."),
                            Ok(n) => println!("Deleted {n} decision(s) matching '{keyword}'."),
                            Err(e) => die("Failed", e),
                        }
                    }
                }
                DecisionAction::Update { slug, name, rationale, recall, status } => {
                    if let Some(s) = resolve(&cwd, &config.namespace, "decision", &slug) {
                        match crud::decision::update(
                            &cwd, &config.namespace, &s,
                            name.as_deref(), rationale.as_deref(), recall.as_deref(), status.as_deref(),
                        ) {
                            Ok(()) => println!("Decision '{s}' updated"),
                            Err(e) => die("Failed", e),
                        }
                    }
                }
            }
        }

        // ─── Entity ──────────────────────────────────────
        Some(Commands::Entity { action }) => match action {
            EntityAction::Add { name, entity_type, domain, project } => {
                match crud::entity::add(&cwd, &config.namespace, &name, &entity_type, &domain, project.as_deref()) {
                    Ok(slug) => println!("Entity '{name}' created (slug: {slug}, domain: {domain})"),
                    Err(e) => die("Failed", e),
                }
            }
            EntityAction::List => { if let Err(e) = crud::entity::list(&cwd, &config.namespace) { die("Error", e); } }
            EntityAction::Get { slug } => {
                if let Some(s) = resolve(&cwd, &config.namespace, "entity", &slug)
                    && let Err(e) = crud::entity::get(&cwd, &config.namespace, &s) { die("Error", e); }
            }
            EntityAction::Update { slug, status, description } => {
                if let Some(s) = resolve(&cwd, &config.namespace, "entity", &slug) {
                    match crud::entity::update(&cwd, &config.namespace, &s, status.as_deref(), description.as_deref()) {
                        Ok(()) => println!("Entity '{s}' updated"),
                        Err(e) => die("Failed", e),
                    }
                }
            }
        },

        // ─── Goal ────────────────────────────────────────
        Some(Commands::Goal { action }) => match action {
            GoalAction::Add { name, target } => {
                match crud::goal::add(&cwd, &config.namespace, &name, &target) {
                    Ok(slug) => println!("Goal '{name}' created (slug: {slug})"),
                    Err(e) => die("Failed", e),
                }
            }
            GoalAction::List => { if let Err(e) = crud::goal::list(&cwd, &config.namespace) { die("Error", e); } }
            GoalAction::Update { slug, status, target } => {
                if let Some(s) = resolve(&cwd, &config.namespace, "goal", &slug) {
                    match crud::goal::update(&cwd, &config.namespace, &s, status.as_deref(), target.as_deref()) {
                        Ok(()) => println!("Goal '{s}' updated"),
                        Err(e) => die("Failed", e),
                    }
                }
            }
        },

        // ─── Handoff ─────────────────────────────────────
        Some(Commands::Handoff { global, action }) => {
            let cwd = tier_cwd(&cwd, global);
            match action {
                HandoffAction::Create { project, doc, slug } => {
                    match crud::handoff::create(&cwd, &config.namespace, &project, &doc, slug.as_deref()) {
                        Ok(slug) => println!("Handoff for '{project}' registered (slug: {slug})"),
                        Err(e) => die("Failed", e),
                    }
                }
                HandoffAction::List => { if let Err(e) = crud::handoff::list(&cwd, &config.namespace) { die("Error", e); } }
                HandoffAction::Snooze { slug, days } => {
                    match crud::handoff::snooze(
                        base::home::home_root().as_deref(),
                        &cwd,
                        &config.namespace,
                        &slug,
                        days,
                    ) {
                        Ok(()) => println!("Handoff '{slug}' snoozed {days}d"),
                        Err(e) => die("Failed", e),
                    }
                }
                HandoffAction::Archive { slug } => {
                    match crud::handoff::archive(
                        base::home::home_root().as_deref(),
                        &cwd,
                        &config.namespace,
                        &slug,
                    ) {
                        Ok(()) => println!("Handoff '{slug}' archived"),
                        Err(e) => die("Failed", e),
                    }
                }
            }
        }

        // ─── Fork ────────────────────────────────────────
        Some(Commands::Fork { global, action }) => {
            let cwd = tier_cwd(&cwd, global);
            match action {
                ForkAction::Create { project, doc, slug } => {
                    match crud::handoff::create_fork(&cwd, &config.namespace, &project, &doc, slug.as_deref()) {
                        Ok(slug) => println!("Fork '{slug}' registered for '{project}'"),
                        Err(e) => die("Failed", e),
                    }
                }
                ForkAction::List => { if let Err(e) = crud::handoff::list_forks(&cwd, &config.namespace) { die("Error", e); } }
                ForkAction::Snooze { slug, days } => {
                    match crud::handoff::snooze(
                        base::home::home_root().as_deref(),
                        &cwd,
                        &config.namespace,
                        &slug,
                        days,
                    ) {
                        Ok(()) => println!("Fork '{slug}' snoozed {days}d"),
                        Err(e) => die("Failed", e),
                    }
                }
                ForkAction::Archive { slug } => {
                    match crud::handoff::archive(
                        base::home::home_root().as_deref(),
                        &cwd,
                        &config.namespace,
                        &slug,
                    ) {
                        Ok(()) => println!("Fork '{slug}' archived"),
                        Err(e) => die("Failed", e),
                    }
                }
            }
        }
        // ─── Reminder ────────────────────────────────────
        Some(Commands::Reminder { action }) => match action {
            ReminderAction::Add { name, due, at, in_dur } => {
                match resolve_surface_at(due.as_deref(), at.as_deref(), in_dur.as_deref()) {
                    Ok(surface_at) => match crud::reminder::add(&cwd, &config.namespace, &name, &surface_at, due.as_deref()) {
                        Ok(slug) => println!("Reminder '{name}' set — surfaces at session start on/after {surface_at} (slug: {slug})"),
                        Err(e) => die("Failed", e),
                    },
                    Err(e) => die("Invalid time", e),
                }
            }
            ReminderAction::List => { if let Err(e) = crud::reminder::list(&cwd, &config.namespace) { die("Error", e); } }
            ReminderAction::Remove { slug } => {
                match crud::reminder::remove(&cwd, &config.namespace, &slug) {
                    Ok(()) => println!("Reminder '{slug}' removed"),
                    Err(e) => die("Failed", e),
                }
            }
        },

        // ─── Sync ────────────────────────────────────────
        Some(Commands::Sync { incremental, ast, target, yes, repair }) => {
            if repair {
                match base::crud::repair_edges(&cwd, &config.namespace) {
                    Ok(count) => println!("Repair complete: {count} edges backfilled"),
                    Err(e) => die("Repair failed", e),
                }
                return;
            }
            if ast {
                // AST extraction via bundled Python scripts
                let target_dir = target.as_deref().unwrap_or(".");
                let home = base::home::home_root().unwrap_or_default();

                // Search order: ~/.base-gbl/scripts/ast/ → cwd/scripts/ast/ → source relative
                let search_paths = [
                    home.join(".base-gbl").join("scripts").join("ast").join("onto_ast.py"),
                    cwd.join("scripts/ast/onto_ast.py"),
                ];

                let ast_script = search_paths
                    .iter()
                    .find(|p| p.exists())
                    .cloned();

                let Some(ast_script) = ast_script else {
                    eprintln!("AST extractor not found. Searched:");
                    for p in &search_paths {
                        eprintln!("  {}", p.display());
                    }
                    eprintln!("\nRun `base install` to copy scripts to ~/.base-gbl/scripts/");
                    return;
                };

                // Per-app self-contained map: resolve output to the TARGET's app
                // root (`<app_root>/.base/ast.ttl`), not cwd — so parsing app B
                // never clobbers app A's map.
                let target_path = {
                    let t = std::path::Path::new(target_dir);
                    if t.is_absolute() { t.to_path_buf() } else { cwd.join(t) }
                };
                let ast_ttl = base::config::resolve_ast_ttl(&target_path);
                if let Some(parent) = ast_ttl.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }

                // AST data lives ONLY in ast.ttl. Never write it into graph.nq —
                // Turtle appended to an NQuads file corrupts the whole graph (AUDIT C10).
                println!("AST extraction: {} → {}", target_dir, ast_ttl.display());
                let mut extractor = std::process::Command::new(base::multimodal::python_bin());
                extractor
                    .arg(&ast_script)
                    .arg(target_dir)
                    .arg("--full")
                    .arg("--out")
                    .arg(&ast_ttl);
                if yes {
                    // Unattended (a hook, a git hook, `--yes` by hand): nobody is
                    // there to answer the file-count prompt, so answer it.
                    extractor.arg("--confirm");
                }
                // A failed build explains itself at the next session start
                // (`.base-ast/.last-error`) instead of vanishing with a detached
                // process; a successful one clears the record.
                let last_error = ast_ttl.with_file_name(".last-error");
                let status = if yes {
                    extractor
                        .stdin(std::process::Stdio::null())
                        .stderr(std::process::Stdio::piped());
                    extractor.output().map(|o| {
                        if !o.status.success() {
                            let _ = std::fs::write(&last_error, base::hook::automap::stderr_tail(&o.stderr));
                        }
                        o.status
                    })
                } else {
                    extractor.status()
                };

                // Whatever happened, this build is over: the hooks may start the next.
                let _ = std::fs::remove_file(ast_ttl.with_file_name(".building"));
                match status {
                    Ok(s) if s.success() => {
                        let _ = std::fs::remove_file(&last_error);
                        println!("AST extraction complete → {}", ast_ttl.display());
                        // Register a pointer to this map in the workspace graph so
                        // it's discoverable outside a dev session. Foreground syncs
                        // only — the Stop hook sets BASE_AST_SKIP_REGISTER to keep
                        // frequent background refreshes off graph.nq.
                        if std::env::var("BASE_AST_SKIP_REGISTER").is_err() {
                            if let Some(app_root) = ast_ttl.parent().and_then(|p| p.parent()) {
                                base::ast_repo::ensure_repo_wiring(app_root);
                                if let Err(e) = crud::ast_map::register(
                                    &cwd, &config.namespace, app_root, &ast_ttl,
                                ) {
                                    eprintln!("(ast map registration skipped: {e})");
                                }
                            }
                        }
                    }
                    Ok(s) => {
                        eprintln!("AST extraction exited with code {:?}", s.code());
                        if !yes {
                            let _ = std::fs::write(&last_error, format!("extractor exited with code {:?}\n", s.code()));
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to run AST extractor: {e}");
                        let _ = std::fs::write(
                            &last_error,
                            format!("failed to run the AST extractor ({}): {e}\n", base::multimodal::python_bin()),
                        );
                    }
                }
            } else {
                match base::extract::sync(&cwd, &config, incremental) {
                    Ok(report) => {
                        println!(
                            "Sync complete: {} scanned, {} extracted, {} skipped",
                            report.scanned, report.extracted, report.skipped
                        );
                    }
                    Err(e) => die("Sync failed", e),
                }
            }
        }

        // ─── Domain ──────────────────────────────────────
        Some(Commands::Domain { action }) => match action {
            DomainAction::AddTrigger { domain: name, keyword, path } => {
                if keyword.is_none() && path.is_none() {
                    eprintln!("Provide --keyword and/or --path");
                    return;
                }
                match domain::add_trigger(&cwd, &name, keyword.as_deref(), path.as_deref()) {
                    Ok(()) => println!("Trigger added to domain '{name}'"),
                    Err(e) => die("Failed", e),
                }
            }
            DomainAction::List => domain::list_domains(&cwd),
            DomainAction::Get { name } => domain::get_domain(&cwd, &name),
            DomainAction::Sync { carl } => {
                let carl_path = carl.as_ref().map(std::path::Path::new);
                match domain::sync::sync_domains_to_graph(&config, &cwd, carl_path) {
                    Ok(stats) => println!(
                        "Domain sync complete: {} domains, {} rules, {} decisions",
                        stats.domains, stats.rules, stats.decisions
                    ),
                    Err(e) => die("Domain sync failed", e),
                }
            }
            DomainAction::Create { name, keyword, path } => {
                match domain::create_domain(&cwd, &name, keyword.as_deref(), path.as_deref()) {
                    Ok(()) => println!("Domain '{name}' created"),
                    Err(e) => die("Failed", e),
                }
            }
            DomainAction::Remove { name } => {
                match domain::remove_domain(&cwd, &name) {
                    Ok(true) => println!("Domain '{name}' removed"),
                    Ok(false) => eprintln!("Domain '{name}' not found"),
                    Err(e) => die("Failed", e),
                }
            }
            DomainAction::RemoveTrigger { domain: name, keyword, path } => {
                if keyword.is_none() && path.is_none() {
                    eprintln!("Provide --keyword and/or --path to remove");
                    return;
                }
                match domain::remove_trigger(&cwd, &name, keyword.as_deref(), path.as_deref()) {
                    Ok(()) => println!("Trigger removed from domain '{name}'"),
                    Err(e) => die("Failed", e),
                }
            }
        },

        // ─── Standards ────────────────────────────────────
        Some(Commands::Standards { action }) => match action {
            StandardsAction::Sync { source } => {
                match base::standards::sync::sync_standards(&config, source.as_deref()) {
                    Ok(stats) => {
                        println!(
                            "Standards sync complete: {} parsed from protocols.md, {} updated, {} created → {}",
                            stats.parsed,
                            stats.updated,
                            stats.created,
                            stats.toml_path.display()
                        );
                        if stats.graph_standards > 0 {
                            println!("Graph: {} Standard entities synced (global tier)", stats.graph_standards);
                        }
                        if !stats.unannotated.is_empty() {
                            println!(
                                "UNANNOTATED (inert until triggers added in standards.toml): {}",
                                stats.unannotated.join(", ")
                            );
                        }
                    }
                    Err(e) => die("Standards sync failed", e),
                }
            }
            StandardsAction::List => base::standards::sync::list_standards(&cwd),
            StandardsAction::Get { id } => base::standards::sync::get_standard(&cwd, &id),
            StandardsAction::Test { file, content } => {
                base::standards::sync::test_standard_match(&config, &cwd, &file, content.as_deref())
            }
        },

        // ─── Relay ────────────────────────────────────────
        Some(Commands::Relay { action }) => {
            use base::relay::{self, RelayStore};

            // Identity: explicit flag → BASE_RELAY_AS / registry binding.
            fn who(store: &RelayStore, explicit: Option<String>) -> Option<String> {
                explicit.or_else(|| store.identity(relay::env_session_id().as_deref()))
            }
            fn need_identity(store: &RelayStore, explicit: Option<String>) -> Option<String> {
                let id = who(store, explicit);
                if id.is_none() {
                    eprintln!(
                        "Cannot resolve your relay identity. Register first \
                         (base relay register --as <title>) or pass the identity flag."
                    );
                }
                id
            }

            match action {
                RelayAction::Init { project } => {
                    let base_dir = base::config::find_workspace_base(&cwd)
                        .unwrap_or_else(|| cwd.join(".base"));
                    let store = RelayStore {
                        root: base_dir.join("relay").join(&project),
                        project: project.clone(),
                    };
                    match store.init() {
                        Ok(()) => println!(
                            "Relay store '{project}' ready: {}\nSessions join with: base relay register --as <title> --project {project}",
                            store.root.display()
                        ),
                        Err(e) => die("Relay init failed", e),
                    }
                }
                RelayAction::Register { title, session, phase, project } => {
                    let sid = session.or_else(relay::env_session_id);
                    // Global session registry: the cross-workspace title binding
                    // that makes `*task <title>` deliverable. Always attempted.
                    match &sid {
                        Some(s) => {
                            if let Err(e) = relay::session_registry::register(&title, s, &cwd, project.as_deref()) {
                                eprintln!("Warning: global session registry update failed: {e:#}");
                            }
                        }
                        None => eprintln!(
                            "No session id (set CLAUDE_CODE_SESSION_ID or pass --session) — \
                             '{title}' not globally bound; *task delivery needs a session binding."
                        ),
                    }
                    // Project relay store binding (Cadre/PAUL fan-outs) — only when a store exists.
                    match relay::resolve_store(&cwd, project.as_deref()) {
                        Ok(store) if store.exists() => {
                            let wt = cwd.to_string_lossy().to_string();
                            match store.register(&title, sid.as_deref(), &wt, phase.as_deref()) {
                                Ok(()) => println!(
                                    "Registered '{title}' in relay '{}' + global registry{}",
                                    store.project,
                                    sid.as_deref().map(|s| format!(" (session {s})")).unwrap_or_default()
                                ),
                                Err(e) => die("Register failed", e),
                            }
                        }
                        _ => println!(
                            "Registered '{title}' globally{}. Other sessions can now relay to you: *task {title} …",
                            sid.as_deref()
                                .map(|s| format!(" (session {s})"))
                                .unwrap_or_else(|| " (no session binding — hook delivery needs CLAUDE_CODE_SESSION_ID)".into())
                        ),
                    }
                    // Wake contract, in-band: registration is often a boot
                    // sequence's last tool call, so the arming block must ride
                    // the register output itself — a hook nudge on the NEXT
                    // tool call never fires if the session goes idle here.
                    if !base::relay::wake::is_watching(&title)
                        && let Some(block) = base::relay::wake::arm_block(&title)
                    {
                        println!("\n{block}");
                    }
                }
                RelayAction::Send { to, mtype, msg, from, refs, project } => {
                    let store = match relay::resolve_store(&cwd, project.as_deref()) {
                        Ok(s) => s,
                        Err(e) => die("Relay", e),
                    };
                    let Some(sender) = need_identity(&store, from) else { return };
                    match store.send(&sender, &to, &mtype, &msg, &refs) {
                        Ok(m) => {
                            println!("Sent {} → {} ({})", m.mtype, m.to, m.id);
                            // The spool holds the message; the wake monitor
                            // watches the global relay-inbox. Drop a consumed-
                            // on-announce notify there per recipient so a
                            // watching session actually wakes (issue #9).
                            let mut woken = 0usize;
                            for t in store.wake_targets(&to, &sender) {
                                let Some(entry) = relay::session_registry::resolve(&t) else {
                                    continue;
                                };
                                let notify = base::relay::task_inbox::InboxTask {
                                    slug: format!("notify-{}-{}", m.id, t),
                                    summary: format!("[{}] {}", m.mtype, m.msg),
                                    doc: String::new(),
                                    from: sender.clone(),
                                    to_title: t.clone(),
                                    to_session: entry.session_id.clone(),
                                    priority: "high".into(),
                                    created: base::relay::now_iso(),
                                    status: "pending".into(),
                                    last_loud_session: String::new(),
                                    last_alert_ts: String::new(),
                                    kind: "notify".into(),
                                    refs: refs.clone(),
                                };
                                match base::relay::task_inbox::enqueue(&config.namespace, &notify) {
                                    Ok(_) => woken += 1,
                                    Err(e) => eprintln!("Warning: wake notify for '{t}' failed: {e:#}"),
                                }
                            }
                            if woken > 0 {
                                println!("Wake notify → {woken} inbox(es); fires on the receiver's monitor or next tool call.");
                            }
                        }
                        Err(e) => die("Send failed", e),
                    }
                }
                RelayAction::Poll { for_title, peek, project } => {
                    let store = match relay::resolve_store(&cwd, project.as_deref()) {
                        Ok(s) => s,
                        Err(e) => die("Relay", e),
                    };
                    let Some(title) = need_identity(&store, for_title) else { return };
                    let pending = store.pending_for(&title);
                    if pending.is_empty() {
                        println!("No pending messages for '{title}'.");
                        return;
                    }
                    for m in &pending {
                        let refs = if m.refs.is_empty() { String::new() } else { format!(" [refs: {}]", m.refs.join(", ")) };
                        println!("[{} · {} · {} ago] {}{refs}", m.from, m.mtype, base::relay::age_str(&m.ts), m.msg);
                    }
                    if !peek {
                        let ids: Vec<String> = pending.iter().map(|m| m.id.clone()).collect();
                        if let Err(e) = store.mark_seen(&title, &ids) {
                            eprintln!("Warning: failed to mark seen: {e}");
                        }
                    }
                }
                RelayAction::Wait { from, mtype, timeout, for_title, project } => {
                    let store = match relay::resolve_store(&cwd, project.as_deref()) {
                        Ok(s) => s,
                        Err(e) => die("Relay", e),
                    };
                    let Some(title) = need_identity(&store, for_title) else { return };
                    match store.wait(&title, from.as_deref(), mtype.as_deref(), timeout) {
                        Some(m) => {
                            println!("[{} · {} · {}] {}", m.from, m.mtype, m.ts, m.msg);
                            if !m.refs.is_empty() {
                                println!("refs: {}", m.refs.join(", "));
                            }
                        }
                        None => {
                            eprintln!("Timeout after {timeout}s — no matching message.");
                            std::process::exit(1);
                        }
                    }
                }
                RelayAction::Claim { resource, note, ttl, project } => {
                    let store = match relay::resolve_store(&cwd, project.as_deref()) {
                        Ok(s) => s,
                        Err(e) => die("Relay", e),
                    };
                    let Some(title) = need_identity(&store, None) else { return };
                    match store.claim(&resource, &title, &note, ttl) {
                        Ok(()) => println!("Claimed '{resource}' for '{title}' (ttl {ttl}s)"),
                        Err(e) => {
                            eprintln!("{e}");
                            std::process::exit(1);
                        }
                    }
                }
                RelayAction::Release { resource, force, project } => {
                    let store = match relay::resolve_store(&cwd, project.as_deref()) {
                        Ok(s) => s,
                        Err(e) => die("Relay", e),
                    };
                    let title = who(&store, None).unwrap_or_else(|| "operator".into());
                    match store.release(&resource, &title, force) {
                        Ok(()) => println!("Released '{resource}'"),
                        Err(e) => die("Release failed", e),
                    }
                }
                RelayAction::Board { project } => {
                    base::relay::board::print_board(&cwd, project.as_deref());
                }
                RelayAction::Export { project } => {
                    let store = match relay::resolve_store(&cwd, project.as_deref()) {
                        Ok(s) => s,
                        Err(e) => die("Relay", e),
                    };
                    match store.export_nq(config.namespace.uri.trim_end_matches(['#', '/'])) {
                        Ok(path) => println!("Exported: {}", path.display()),
                        Err(e) => die("Export failed", e),
                    }
                }
                RelayAction::Dispose { project, force } => {
                    let store = match relay::resolve_store(&cwd, Some(&project)) {
                        Ok(s) => s,
                        Err(e) => die("Relay", e),
                    };
                    if !store.exists() {
                        eprintln!("Store '{project}' does not exist.");
                        return;
                    }
                    let msgs = store.all_messages().len();
                    let sessions = store.load_registry().sessions.len();
                    if !force {
                        println!(
                            "Would remove relay '{project}': {msgs} messages, {sessions} sessions at {}.\n\
                             Promote anything durable first (base decision log / base learn), then re-run with --force.",
                            store.root.display()
                        );
                        return;
                    }
                    match store.dispose() {
                        Ok(()) => println!("Relay '{project}' disposed ({msgs} messages gone). Durable outcomes belong in the graph."),
                        Err(e) => die("Dispose failed", e),
                    }
                }
                RelayAction::Task { to, slug, summary, doc, priority, from } => {
                    let Some(entry) = relay::session_registry::resolve(&to) else {
                        die("Relay task", anyhow::anyhow!(
                            "no session registered as '{to}'. The target session must first run: base relay register --as {to}"
                        ));
                    };
                    if !entry.alive() {
                        eprintln!(
                            "Warning: session '{to}' last seen {} ago — it may be dead. Relaying anyway.",
                            base::relay::age_str(&entry.last_heartbeat)
                        );
                    }
                    // Origin label: explicit --from, else this session's own title.
                    let origin = from.or_else(|| {
                        relay::env_session_id().and_then(|sid| {
                            relay::session_registry::list()
                                .into_iter()
                                .find(|e| e.session_id == sid)
                                .map(|e| e.title)
                        })
                    });
                    let slug = crud::slugify(&slug);
                    let task = base::relay::task_inbox::InboxTask {
                        slug: slug.clone(),
                        summary,
                        doc: doc.unwrap_or_default(),
                        from: origin.unwrap_or_default(),
                        to_title: to.clone(),
                        to_session: entry.session_id.clone(),
                        priority: priority.unwrap_or_else(|| "high".into()),
                        created: base::relay::now_iso(),
                        status: "pending".into(),
                        last_loud_session: String::new(),
                        last_alert_ts: String::new(),
                        kind: "task".into(),
                        refs: Vec::new(),
                    };
                    match base::relay::task_inbox::enqueue(&config.namespace, &task) {
                        Ok(path) => println!(
                            "Relayed '{slug}' → {to} (session {}). Fires in that session on its next hook.\n  inbox: {}",
                            entry.session_id,
                            path.display()
                        ),
                        Err(e) => die("Relay task failed", e),
                    }
                }
                RelayAction::Ping { to, msg, refs, from } => {
                    let Some(entry) = relay::session_registry::resolve(&to) else {
                        die("Relay ping", anyhow::anyhow!(
                            "no session registered as '{to}'. The target session must first run: base relay register --as {to}"
                        ));
                    };
                    if !entry.alive() {
                        eprintln!(
                            "Warning: session '{to}' last seen {} ago — it may be dead. Pinging anyway.",
                            base::relay::age_str(&entry.last_heartbeat)
                        );
                    } else if !base::relay::wake::is_watching(&to) {
                        eprintln!(
                            "Note: '{to}' has no live wake monitor ({}) — if idle it will NOT wake; \
                             the ping lands on its next tool call or prompt.",
                            base::relay::wake::watch_cell(&to)
                        );
                    }
                    // Origin: explicit --from, else every title this session holds
                    // (auto-codenames included — hooks touch() one on each boundary).
                    let my_titles: Vec<String> = match &from {
                        Some(f) => vec![f.clone()],
                        None => relay::env_session_id()
                            .map(|sid| relay::session_registry::titles_for(&sid))
                            .unwrap_or_default(),
                    };
                    let origin = my_titles.first().cloned().unwrap_or_default();
                    if origin.is_empty() {
                        eprintln!(
                            "Warning: this session has no relay title — the receiver cannot ping back. \
                             Register one: base relay register --as <title>"
                        );
                    }
                    // Chris's output-style contract applies to his pipe
                    // (2026-08-17): pings to chris are scored by his
                    // style-guard (--text mode). Hard violation = the send is
                    // REFUSED so the sender rewrites — same loop his Stop hook
                    // runs on replies. Fail-open when the guard is absent.
                    if to == "chris"
                        && let Some(guard) = base::home::home_root()
                            .map(|h| h.join(".claude").join("hooks").join("style-guard.py"))
                            .filter(|p| p.exists())
                    {
                        use std::io::Write as _;
                        use std::process::{Command, Stdio};
                        let py = if cfg!(windows) { "python" } else { "python3" };
                        let run = Command::new(py)
                            .arg(&guard)
                            .arg("--text")
                            .stdin(Stdio::piped())
                            .stdout(Stdio::piped())
                            .stderr(Stdio::null())
                            .spawn()
                            .and_then(|mut ch| {
                                if let Some(mut si) = ch.stdin.take() {
                                    let _ = si.write_all(msg.as_bytes());
                                }
                                ch.wait_with_output()
                            });
                        if let Ok(out) = run {
                            let verdict =
                                String::from_utf8_lossy(&out.stdout).trim().to_string();
                            match out.status.code() {
                                Some(2) => die(
                                    "Ping to chris REFUSED by style guard",
                                    anyhow::anyhow!(
                                        "{verdict}. Rewrite in Chris's style — plain words, \
                                         within budget, no banned phrases — and resend."
                                    ),
                                ),
                                Some(1) => eprintln!("{verdict} — sent anyway; tighten next time."),
                                _ => {}
                            }
                        }
                    }
                    // Replying? Any pending inbound ping FROM the target in OUR
                    // inbox means this send is the answer — clear those now, and
                    // mark this ping a reply so it can't demand its own ack.
                    let answered =
                        base::relay::task_inbox::clear_pings_from(&config.namespace, &to, &my_titles);
                    let kind = if answered > 0 { "reply" } else { "ping" };
                    let id = format!(
                        "ping-{}",
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis())
                            .unwrap_or(0)
                    );
                    let ping = base::relay::task_inbox::InboxTask {
                        slug: id.clone(),
                        summary: msg.clone(),
                        doc: String::new(),
                        from: origin,
                        to_title: to.clone(),
                        to_session: entry.session_id.clone(),
                        priority: "high".into(),
                        created: base::relay::now_iso(),
                        status: "pending".into(),
                        last_loud_session: String::new(),
                        last_alert_ts: String::new(),
                        kind: kind.into(),
                        refs,
                    };
                    match base::relay::task_inbox::enqueue(&config.namespace, &ping) {
                        Ok(_) if answered > 0 => println!(
                            "Ping (reply) → {to}: \"{msg}\" — cleared {answered} inbound ping(s); fires on their next tool call."
                        ),
                        Ok(_) => println!(
                            "Ping → {to}: \"{msg}\" — fires on their next tool call; clears when they ping back."
                        ),
                        Err(e) => die("Relay ping failed", e),
                    }
                }
                RelayAction::Done { slug } => {
                    let slug = crud::slugify(&slug);
                    match base::relay::task_inbox::done(&config.namespace, &slug) {
                        Ok(0) => println!("No inbound relay task '{slug}' found (already done, or wrong slug)."),
                        Ok(n) => println!(
                            "Relay task '{slug}' done — cleared {n} inbox entr{}.",
                            if n == 1 { "y" } else { "ies" }
                        ),
                        Err(e) => die("Relay done failed", e),
                    }
                }
                RelayAction::Tasks => {
                    let tasks = base::relay::task_inbox::list_all();
                    if tasks.is_empty() {
                        println!("No inbound relay tasks.");
                    } else {
                        println!("Inbound relay tasks ({}):", tasks.len());
                        for t in &tasks {
                            println!("{}", base::relay::task_inbox::format_row(t));
                        }
                    }
                }
                RelayAction::Sessions => {
                    let sessions = relay::session_registry::list();
                    if sessions.is_empty() {
                        println!("No titled sessions. A session claims a title with: base relay register --as <title>");
                    } else {
                        println!("Titled sessions ({}):", sessions.len());
                        for e in &sessions {
                            let live = if e.alive() { "live" } else { "DEAD" };
                            let ws = if e.workspace.is_empty() { "-" } else { e.workspace.as_str() };
                            println!(
                                "  {title}  [{live} · {age}]  ws:{ws}  session:{sid}",
                                title = e.title,
                                age = base::relay::age_str(&e.last_heartbeat),
                                sid = e.session_id,
                            );
                        }
                    }
                }
            }
        }

        // ─── Rule ─────────────────────────────────────────
        Some(Commands::Rule { global, action }) => {
            let rule_cwd = tier_cwd(&cwd, global);
            match action {
                RuleAction::Add { domain: name, text, rationale } => {
                    match crud::rule::add(&rule_cwd, &config.namespace, &name, &text, rationale.as_deref()) {
                        Ok(index) => println!("Rule {index} added to domain '{name}'"),
                        Err(e) => die("Failed", e),
                    }
                }
                RuleAction::List { domain: name } => {
                    if let Err(e) = crud::rule::list(&rule_cwd, &config.namespace, &name) {
                        eprintln!("Failed: {e}");
                    }
                }
                RuleAction::Remove { domain: name, index } => {
                    match crud::rule::remove(&rule_cwd, &config.namespace, &name, index) {
                        Ok(()) => println!("Rule {index} removed from domain '{name}'"),
                        Err(e) => die("Failed", e),
                    }
                }
            }
        },

        // ─── Learn ────────────────────────────────────────
        Some(Commands::Learn { global, text, r#type, domain, project, entity, mention, context, remove, update, list }) => {
            let cwd = tier_cwd(&cwd, global);
            if list {
                if let Err(e) = crud::note::list_notes(&cwd, &config.namespace, if r#type != "insight" { Some(&r#type) } else { None }, domain.as_deref()) {
                    die("Error", e);
                }
            } else if let Some(slug) = remove {
                match crud::note::remove(&cwd, &config.namespace, &slug) {
                    Ok(true) => println!("Removed note/{slug}"),
                    Ok(false) => eprintln!("Not found: note/{slug}"),
                    Err(e) => die("Failed", e),
                }
            } else if let Some(slug) = update {
                let Some(new_text) = text else {
                    eprintln!("--text is required with --update");
                    std::process::exit(1);
                };
                match crud::note::update_text(&cwd, &config.namespace, &slug, &new_text) {
                    Ok(true) => println!("Updated note/{slug}"),
                    Ok(false) => eprintln!("Not found: note/{slug}"),
                    Err(e) => die("Failed", e),
                }
            } else if let Some(slug) = mention {
                match crud::note::mention(
                    &cwd,
                    &config.namespace,
                    &slug,
                    context.as_deref(),
                ) {
                    Ok(count) => println!("Mention recorded: {slug} (count: {count})"),
                    Err(e) => die("Failed", e),
                }
            } else {
                let Some(text) = text else {
                    eprintln!("--text is required (or use --mention, --remove, --update, --list)");
                    std::process::exit(1);
                };
                let Some(domain) = domain else {
                    eprintln!("--domain is required (or use --mention, --remove, --update, --list)");
                    std::process::exit(1);
                };
                match crud::note::learn(
                    &cwd,
                    &config.namespace,
                    &text,
                    &r#type,
                    Some(&domain),
                    project.as_deref(),
                    entity.as_deref(),
                ) {
                    Ok(slug) => println!("Learned: '{text}' (slug: {slug}, type: {}, domain: {domain})", r#type),
                    Err(e) => die("Failed", e),
                }
            }
        }

        // ─── Recall ─────────────────────────────────────────
        Some(Commands::Recall { keyword, domain, slug }) => {
            // Note IRIs to stamp lastRead on (usage signal for `base graph purge --stale`).
            // Resolved BEFORE printing so an explicit recall marks what it surfaced.
            let mut surfaced: Vec<String> = Vec::new();
            if let Some(slug) = slug {
                if let Err(e) = crud::note::recall_by_slug(&cwd, &config.namespace, &slug) {
                    die("Error", e);
                }
                surfaced.push(crud::build_iri(&config.namespace, "note", &slug));
            } else {
                if keyword.is_none() && domain.is_none() {
                    eprintln!("Provide --keyword, --domain, or --slug");
                    return;
                }
                surfaced = crud::note::recalled_note_iris(
                    &cwd,
                    &config.namespace,
                    keyword.as_deref(),
                    domain.as_deref(),
                );
                if let Err(e) = crud::note::recall(&cwd, &config.namespace, keyword.as_deref(), domain.as_deref()) { die("Error", e); }
            }
            // Best-effort lastRead stamping (strict write). On a corrupt graph this
            // errs — warn and skip rather than silently lenient-writing (Phase 35).
            if let Err(e) = crud::note::stamp_last_read(&cwd, &config.namespace, &surfaced) {
                eprintln!("recall: skipped lastRead stamping (graph unhealthy?) — run `base doctor --repair`: {e}");
            }
        }

        // ─── Changes (graph change log) ─────────────────────
        // Machine-readable only: stdout is always one JSON object, including on
        // error, because this is the surface an external app polls.
        Some(Commands::Changes { global, since, cursor }) => {
            let tier = tier_cwd(&cwd, global);
            let Some(base_dir) = base::config::find_workspace_base(&tier) else {
                println!(
                    "{}",
                    serde_json::json!({
                        "error": "no .base/ directory found",
                        "cwd": tier.display().to_string(),
                    })
                );
                std::process::exit(1);
            };
            let log = base::changelog::log_path_for(&base_dir.join("graph.nq"));

            if cursor {
                println!("{}", serde_json::json!({ "offset": base::changelog::cursor(&log) }));
                return;
            }

            match base::changelog::read_since(&log, since.unwrap_or(0)) {
                Ok(page) => {
                    let mut parsed = Vec::with_capacity(page.lines.len());
                    let mut skipped = 0usize;
                    for line in &page.lines {
                        match serde_json::from_str::<serde_json::Value>(line) {
                            Ok(v) => parsed.push(v),
                            Err(_) => skipped += 1,
                        }
                    }
                    // A record either carries its delta or it does not, and the
                    // count of the ones that do not is reported rather than left
                    // for the reader to discover: a write this version of base
                    // cannot express as ops is still in the local graph and NOT in
                    // the team's, and a client that cannot see that number would
                    // render a confident, wrong "everything is synced".
                    let mut delta_free = 0usize;
                    for v in &mut parsed {
                        let has_ops = v.get("ops").is_some_and(serde_json::Value::is_array);
                        if !has_ops {
                            delta_free += 1;
                        }
                        if let Some(obj) = v.as_object_mut() {
                            obj.insert("has_ops".into(), has_ops.into());
                        }
                    }
                    println!(
                        "{}",
                        serde_json::json!({
                            "offset": page.offset,
                            "reset": page.reset,
                            "count": parsed.len(),
                            "skipped": skipped,
                            "delta_free_count": delta_free,
                            "changes": parsed,
                        })
                    );
                }
                Err(e) => {
                    println!("{}", serde_json::json!({ "error": e.to_string() }));
                    std::process::exit(1);
                }
            }
        }

        // ─── Install ─────────────────────────────────────────
        Some(Commands::Install { carl, skip_hooks, full, starter_commands, no_starter_commands }) => {
            let carl_path = carl.as_ref().map(std::path::Path::new);
            let starter = match (starter_commands, no_starter_commands) {
                (true, _) => base::install::StarterCommands::Yes,
                (_, true) => base::install::StarterCommands::No,
                _ => base::install::StarterCommands::Ask,
            };
            if let Err(e) = base::install::run(carl_path, skip_hooks, full, starter) {
                eprintln!("Install failed: {e}");
            }
        }

        // ─── Activate ────────────────────────────────────────
        Some(Commands::Activate { key }) => {
            if let Err(e) = base::manifest::activate(&key) {
                eprintln!("{e}");
            }
        }

        // ─── Update ───────────────────────────────────────────
        Some(Commands::Update { check, force, snooze }) => {
            if snooze {
                if let Err(e) = base::manifest::snooze() {
                    eprintln!("Snooze failed: {e}");
                }
            } else if let Err(e) = base::update::run(check, force) {
                die("Update failed", e);
            }
        }

        // ─── Uninstall ────────────────────────────────────────
        Some(Commands::Uninstall { purge }) => {
            if let Err(e) = base::install::uninstall(purge) {
                eprintln!("Uninstall failed: {e}");
            }
        }

        // ─── Dashboard ────────────────────────────────────────
        Some(Commands::Dashboard { port }) => {
            let rt = tokio::runtime::Runtime::new().expect("Failed to start async runtime");
            rt.block_on(base::dashboard::server::start(port, cwd));
        }

        // ─── Scaffold ─────────────────────────────────────────
        Some(Commands::Scaffold { path }) => {
            let target = path
                .as_ref()
                .map(std::path::PathBuf::from)
                .unwrap_or(cwd.clone());
            if let Err(e) = base::scaffold::run(&target) {
                eprintln!("Scaffold failed: {e}");
            }
        }

        // ─── Reconcile ────────────────────────────────────────
        Some(Commands::Reconcile { dry_run }) => {
            let config = base::config::BaseConfig::load(&cwd);
            match base::protocol::reconcile::open_workspace(&cwd) {
                None => eprintln!("base: no workspace graph found (run from inside a base workspace)"),
                Some((store, trig_path, ws_root)) => {
                    let stale = config.protocol.stale_days as i64;
                    let roots = base::protocol::reconcile::registered_roots(&config);
                    match base::protocol::reconcile::plan(&store, &config.namespace, &ws_root, &roots, stale) {
                        Err(e) => eprintln!("base: reconcile plan failed: {e}"),
                        Ok(decisions) => {
                            if dry_run {
                                print!("{}", base::protocol::reconcile::format_report(&decisions, &ws_root, stale));
                            } else if !config.protocol.enabled {
                                eprintln!("base: [protocol] not enabled — refusing to apply. Preview with `base reconcile --dry-run`.");
                            } else {
                                match base::protocol::reconcile::apply(&store, &config.namespace, &trig_path, &decisions) {
                                    Ok(s) => println!(
                                        "base: reconcile — {} deferred, {} revived, {} refreshed ({} scanned)",
                                        s.deferred, s.revived, s.refreshed, s.scanned
                                    ),
                                    Err(e) => eprintln!("base: reconcile apply failed: {e}"),
                                }
                            }
                        }
                    }
                }
            }
        }

        // ─── Workspace registry ───────────────────────────────
        Some(Commands::Workspace { action }) => match action {
            WorkspaceAction::Sync => match base::scaffold::sync_claude_md_registry() {
                Ok(n) => println!("✓ synced {n} workspace(s) into ~/.claude/CLAUDE.md"),
                Err(e) => die("Failed", e),
            },
        },

        // ─── Operator ─────────────────────────────────────────
        Some(Commands::Operator { action }) => match action {
            OperatorAction::Init { name } => {
                if let Err(e) = base::operator::init(&name) {
                    eprintln!("Failed: {e}");
                }
            }
            OperatorAction::Show => base::operator::show(),
        },

        // ─── Extension ────────────────────────────────────────
        Some(Commands::Extension { action }) => match action {
            ExtensionAction::List => {
                let extensions = extension::load_extensions();
                if extensions.is_empty() {
                    println!("No extensions installed.");
                    println!("  Directory: ~/.base-gbl/extensions/");
                    println!("  Template:  ~/.base-gbl/extensions/_template.toml");
                } else {
                    println!("{:<20} {:<10} {:<6} DESCRIPTION", "NAME", "VERSION", "HOOKS");
                    println!("{}", "─".repeat(70));
                    for ext in &extensions {
                        println!(
                            "{:<20} {:<10} {:<6} {}",
                            ext.name,
                            ext.version,
                            ext.hook_summary(),
                            ext.description
                        );
                    }
                    println!("\n{} extension(s) installed.", extensions.len());
                }

                // Drop-in plugin commands contributed by those extensions (v0.6).
                let registry = base::plugin::build_registry(&extensions);
                if !registry.is_empty() {
                    println!();
                    println!("{:<16} {:<16} DESCRIPTION", "PLUGIN COMMAND", "FROM");
                    println!("{}", "─".repeat(70));
                    for c in registry.list() {
                        println!("base {:<11} ext:{:<12} {}", c.name, c.ext_name, c.description);
                    }
                    println!(
                        "\n{} plugin command(s). Invoke: base <name> …  (or: base ext run <name> …)",
                        registry.list().len()
                    );
                }
            }
            ExtensionAction::Validate { path } => {
                let p = std::path::Path::new(&path);
                match extension::validate_extension(p) {
                    Ok(ext) => {
                        println!("✓ Valid extension: {} v{}", ext.name, ext.version);
                        println!("  Description: {}", ext.description);
                        println!("  Hooks: {}", ext.hook_summary());
                    }
                    Err(violations) => {
                        eprintln!("✗ Validation failed for {path}:");
                        for v in &violations {
                            eprintln!("  - {v}");
                        }
                        std::process::exit(1);
                    }
                }
            }
            ExtensionAction::Install { path, bundle } => {
                let p = std::path::Path::new(&path);
                if bundle {
                    match base::plugin::bundle_install(p) {
                        Ok(outcome) => println!("{}", base::plugin::format_bundle_human(&outcome)),
                        Err(e) => {
                            eprintln!("Bundle install failed: {e}");
                            std::process::exit(1);
                        }
                    }
                    return;
                }
                // Linked install — resolves a relative/"." framework_dir to an
                // absolute path so a shipped base-extension.toml installs correctly.
                match base::plugin::install_linked(p) {
                    Ok(o) => {
                        match o.prev_version {
                            Some(old) => println!("✓ Updated {} (was v{}, now v{})", o.name, old, o.version),
                            None => println!("✓ Installed {} v{}", o.name, o.version),
                        }
                        println!("  → {}", o.dest.display());
                    }
                    Err(e) => {
                        eprintln!("✗ Cannot install: {e}");
                        std::process::exit(1);
                    }
                }
            }
            ExtensionAction::Add { path } => {
                let p = std::path::Path::new(&path);
                match base::plugin::dist::dist_install(p) {
                    Ok(outcome) => println!("{}", base::plugin::dist::format_dist_human(&outcome)),
                    Err(e) => {
                        eprintln!("✗ Cannot add: {e}");
                        std::process::exit(1);
                    }
                }
            }
            ExtensionAction::Scaffold { name, path, into, repo, build, git, create_repo, bootstrap } => {
                let parent = match path {
                    Some(p) => std::path::PathBuf::from(p),
                    None => std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
                };
                let opts = base::plugin::scaffold::ScaffoldOpts {
                    into: into.map(std::path::PathBuf::from),
                    repo,
                    build: build || bootstrap,
                    git: git || bootstrap,
                    create_repo: create_repo || bootstrap,
                };
                match base::plugin::scaffold::scaffold_plugin(&name, &parent, &opts) {
                    Ok(outcome) => println!("{}", base::plugin::scaffold::format_scaffold_human(&outcome)),
                    Err(e) => {
                        eprintln!("✗ Cannot scaffold: {e}");
                        std::process::exit(1);
                    }
                }
            }
            ExtensionAction::Remove { name } => {
                let home = base::home::home_root().expect("Cannot determine home directory");
                let ext_dir = home.join(".base-gbl").join("extensions");

                if !ext_dir.is_dir() {
                    eprintln!("Extension '{name}' not found (no extensions directory).");
                    std::process::exit(1);
                }

                // Scan for matching extension by parsed name
                let mut found = None;
                if let Ok(entries) = std::fs::read_dir(&ext_dir) {
                    for entry in entries.filter_map(|e| e.ok()) {
                        let path = entry.path();
                        if path.extension().is_some_and(|e| e == "toml")
                            && path.file_name().is_some_and(|n| n != "_template.toml")
                            && let Ok(content) = std::fs::read_to_string(&path)
                            && let Ok(file) =
                                toml::from_str::<extension::ExtensionFile>(&content)
                            && file.extension.name == name
                        {
                            found = Some((path, file.extension.version));
                            break;
                        }
                    }
                }

                if let Some((path, version)) = found {
                    if let Err(e) = std::fs::remove_file(&path) {
                        eprintln!("Failed to remove extension: {e}");
                        std::process::exit(1);
                    }
                    println!("✓ Removed {name} v{version}");
                    println!("  ← {}", path.display());
                } else {
                    eprintln!("Extension '{name}' not found.");
                    std::process::exit(1);
                }
            }
            ExtensionAction::Run { name, args } => base::plugin::run_named(&name, &args, &cwd),
        },

        // ─── Commands (star commands) ─────────────────────────
        Some(Commands::Command { action }) => match action {
            CommandAction::List => {
                let commands = command::load_commands(&cwd);
                if commands.is_empty() {
                    println!("No commands configured.");
                    println!("  Add entries to ~/.base-gbl/commands.toml");
                } else {
                    println!("{:<16} {:<52} RULES", "COMMAND", "DESCRIPTION");
                    println!("{}", "─".repeat(74));
                    for cmd in &commands {
                        let desc = if cmd.description.chars().count() > 50 {
                            let truncated: String = cmd.description.chars().take(49).collect();
                            format!("{truncated}…")
                        } else {
                            cmd.description.clone()
                        };
                        println!("*{:<15} {:<52} {}", cmd.name, desc, cmd.rules.len());
                    }
                    println!("\n{} command(s) available. Type *NAME in a prompt to activate.", commands.len());
                }
            }
            CommandAction::Show { name } => {
                let commands = command::load_commands(&cwd);
                match commands.iter().find(|c| c.name.eq_ignore_ascii_case(&name)) {
                    Some(cmd) => {
                        println!("*{}", cmd.name);
                        if !cmd.description.is_empty() {
                            println!("  {}", cmd.description);
                        }
                        println!();
                        for (i, rule) in cmd.rules.iter().enumerate() {
                            println!("  {i}. {rule}");
                        }
                    }
                    None => eprintln!("Command '{name}' not found. Run `base commands list` to see available."),
                }
            }
            CommandAction::Add { name, description, rule } => {
                let home = base::home::home_root().expect("Cannot determine home directory");
                let path = home.join(".base-gbl").join("commands.toml");
                let mut content = std::fs::read_to_string(&path).unwrap_or_default();
                let rules_toml: String = rule.iter()
                    .map(|r| format!("  \"{}\",", r.replace('"', "\\\"")))
                    .collect::<Vec<_>>()
                    .join("\n");
                let block = format!(
                    "\n[[command]]\nname = \"{}\"\ndescription = \"{}\"\nrules = [\n{}\n]\n",
                    name.replace('"', "\\\""),
                    description.replace('"', "\\\""),
                    rules_toml,
                );
                content.push_str(&block);
                if let Err(e) = std::fs::write(&path, content) {
                    eprintln!("Failed to write commands.toml: {e}");
                } else {
                    println!("Added *{name} ({} rules)", rule.len());
                }
            }
            CommandAction::Remove { name } => {
                let home = base::home::home_root().expect("Cannot determine home directory");
                let path = home.join(".base-gbl").join("commands.toml");
                let Ok(content) = std::fs::read_to_string(&path) else {
                    eprintln!("Cannot read commands.toml");
                    return;
                };

                #[derive(serde::Deserialize, serde::Serialize)]
                struct CmdFile { #[serde(default)] command: Vec<command::CommandDef> }

                let Ok(mut file) = toml::from_str::<CmdFile>(&content) else {
                    eprintln!("Failed to parse commands.toml");
                    return;
                };
                let before = file.command.len();
                file.command.retain(|c| !c.name.eq_ignore_ascii_case(&name));
                if file.command.len() == before {
                    eprintln!("Command '{name}' not found");
                    return;
                }
                match toml::to_string_pretty(&file) {
                    Ok(out) => {
                        if let Err(e) = std::fs::write(&path, out) {
                            eprintln!("Failed to write: {e}");
                        } else {
                            println!("Removed *{name}");
                        }
                    }
                    Err(e) => eprintln!("Failed to serialize: {e}"),
                }
            }
            CommandAction::Import { file } => {
                let src = match std::fs::read_to_string(&file) {
                    Ok(c) => c,
                    Err(e) => { eprintln!("Cannot read {file}: {e}"); return; }
                };
                #[derive(serde::Deserialize, serde::Serialize)]
                struct CmdFile { #[serde(default)] command: Vec<command::CommandDef> }
                let incoming = match toml::from_str::<CmdFile>(&src) {
                    Ok(f) => f.command,
                    Err(e) => { eprintln!("Failed to parse {file}: {e}"); return; }
                };
                if incoming.is_empty() {
                    println!("No [[command]] entries found in {file}.");
                    return;
                }
                let home = base::home::home_root().expect("Cannot determine home directory");
                let path = home.join(".base-gbl").join("commands.toml");
                if let Some(parent) = path.parent()
                    && let Err(e) = std::fs::create_dir_all(parent)
                {
                    eprintln!("Failed to create {}: {e}", parent.display());
                    std::process::exit(1);
                }
                // Parse what is already there before touching it. The previous
                // implementation appended raw text and treated an unparseable file as
                // empty, so importing onto a corrupt file silently duplicated every
                // command on top of the damage. Refuse instead, and point at the tool
                // that explains why.
                let existing_text = std::fs::read_to_string(&path).unwrap_or_default();
                let mut merged: Vec<command::CommandDef> = if existing_text.trim().is_empty() {
                    Vec::new()
                } else {
                    match toml::from_str::<CmdFile>(&existing_text) {
                        Ok(f) => f.command,
                        Err(e) => {
                            eprintln!("Refusing to import: {} is not valid TOML: {e}", path.display());
                            eprintln!("Run `base doctor` for the diagnosis, then repair or move that file and retry.");
                            std::process::exit(1);
                        }
                    }
                };

                let (mut added, mut skipped): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
                for cmd in incoming {
                    if merged.iter().any(|c| c.name.eq_ignore_ascii_case(&cmd.name)) {
                        skipped.push(cmd.name);
                        continue;
                    }
                    added.push(cmd.name.clone());
                    merged.push(cmd);
                }

                // Serialize through the toml crate. The hand-rolled emitter this
                // replaces escaped only double quotes, so any rule containing a
                // newline, tab, or backslash — which multi-line rules routinely do —
                // produced an invalid basic string and corrupted the file.
                let out = match toml::to_string_pretty(&CmdFile { command: merged }) {
                    Ok(o) => o,
                    Err(e) => {
                        eprintln!("Failed to serialize commands: {e}");
                        std::process::exit(1);
                    }
                };
                // Never persist bytes we cannot read back.
                if let Err(e) = toml::from_str::<CmdFile>(&out) {
                    eprintln!("Refusing to write {}: serialized output does not round-trip: {e}", path.display());
                    std::process::exit(1);
                }
                if let Err(e) = std::fs::write(&path, out) {
                    eprintln!("Failed to write commands.toml: {e}");
                    std::process::exit(1);
                }
                println!("Imported {} command(s) from {file}", added.len());
                if !added.is_empty() {
                    println!("  added: {}", added.iter().map(|n| format!("*{}", n.to_uppercase())).collect::<Vec<_>>().join(" "));
                }
                if !skipped.is_empty() {
                    println!("  skipped (already present): {}", skipped.iter().map(|n| format!("*{}", n.to_uppercase())).collect::<Vec<_>>().join(" "));
                }
            }
        },

        // ─── Memory ────────────────────────────────────────
        Some(Commands::Memory { action }) => match action {
            MemoryAction::List => {
                let home = base::home::home_root().expect("Cannot determine home directory");
                let claude_projects = home.join(".claude").join("projects");
                if !claude_projects.is_dir() {
                    println!("No Claude projects directory found.");
                    return;
                }

                let mut count = 0u32;
                let Ok(project_dirs) = std::fs::read_dir(&claude_projects) else {
                    eprintln!("Failed to read {}", claude_projects.display());
                    return;
                };

                let mut entries: Vec<(String, String, String, String, String)> = Vec::new();

                for entry in project_dirs.filter_map(|e| e.ok()) {
                    let memory_dir = entry.path().join("memory");
                    if !memory_dir.is_dir() {
                        continue;
                    }
                    let project = hook::memory::infer_project_from_memory_path(
                        &memory_dir.to_string_lossy(),
                    ).unwrap_or_else(|| "unknown".into());

                    let Ok(files) = std::fs::read_dir(&memory_dir) else { continue };
                    for file_entry in files.filter_map(|e| e.ok()) {
                        let path = file_entry.path();
                        if path.extension().is_none_or(|e| e != "md") {
                            continue;
                        }
                        if hook::memory::is_memory_index(&path) {
                            continue;
                        }

                        let Ok(content) = std::fs::read_to_string(&path) else { continue };
                        let (name, desc, note_type, _body) =
                            hook::memory::parse_memory_content(&content);
                        let base_type = hook::memory::map_memory_type(&note_type);

                        let display_name = if !name.is_empty() {
                            name
                        } else {
                            path.file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("unnamed")
                                .to_string()
                        };

                        let short_desc: String = if !desc.is_empty() {
                            desc.chars().take(60).collect()
                        } else {
                            "(no description)".into()
                        };

                        entries.push((
                            project.clone(),
                            base_type.to_string(),
                            display_name,
                            short_desc,
                            path.to_string_lossy().to_string(),
                        ));
                        count += 1;
                    }
                }

                entries.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

                let mut current_project = String::new();
                for (project, note_type, name, desc, path) in &entries {
                    if *project != current_project {
                        if !current_project.is_empty() {
                            println!();
                        }
                        println!("── {project} ──");
                        current_project = project.clone();
                    }
                    println!("  [{note_type:<10}] {name}");
                    println!("             {desc}");
                    println!("             {path}");
                }

                println!("\n{count} memory file(s) across Claude's flat-file system.");
                println!("To convert: base learn --text \"...\" --domain <domain> --type <type>");
            }
            MemoryAction::Purge => {
                let home = base::home::home_root().expect("Cannot determine home directory");
                let claude_projects = home.join(".claude").join("projects");
                if !claude_projects.is_dir() {
                    println!("No Claude projects directory found.");
                    return;
                }

                let mut purged = 0u32;
                let mut kept = 0u32;

                let Ok(project_dirs) = std::fs::read_dir(&claude_projects) else {
                    eprintln!("Failed to read {}", claude_projects.display());
                    return;
                };

                for entry in project_dirs.filter_map(|e| e.ok()) {
                    let memory_dir = entry.path().join("memory");
                    if !memory_dir.is_dir() {
                        continue;
                    }
                    let Ok(files) = std::fs::read_dir(&memory_dir) else { continue };
                    for file_entry in files.filter_map(|e| e.ok()) {
                        let path = file_entry.path();
                        if path.extension().is_none_or(|e| e != "md") {
                            continue;
                        }
                        if hook::memory::is_memory_index(&path) {
                            continue;
                        }

                        let slug = path.file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("")
                            .replace('-', " ");

                        let graph_result = crud::note::recall_to_string(
                            &cwd,
                            &config.namespace,
                            Some(&slug),
                            None,
                        );

                        if !graph_result.is_empty() {
                            if let Err(e) = std::fs::remove_file(&path) {
                                eprintln!("  ✗ Failed to delete {}: {e}", path.display());
                                kept += 1;
                            } else {
                                purged += 1;
                            }
                        } else {
                            kept += 1;
                        }
                    }
                }

                println!("Memory purge complete: {purged} deleted, {kept} kept (no graph entry — run migrate first)");
            }
        },

        // ─── Config ────────────────────────────────────────
        Some(Commands::Config { action }) => {
            let home = base::home::home_root().expect("Cannot determine home directory");
            let path = home.join(".base-gbl").join("base.toml");

            match action {
                ConfigAction::List => {
                    let Ok(content) = std::fs::read_to_string(&path) else {
                        eprintln!("Cannot read base.toml at {}", path.display());
                        return;
                    };
                    let Ok(val) = content.parse::<toml::Value>() else {
                        eprintln!("Failed to parse base.toml");
                        return;
                    };
                    if let Some(table) = val.as_table() {
                        for (section, v) in table {
                            if let Some(inner) = v.as_table() {
                                for (key, val) in inner {
                                    println!("{section}.{key} = {val}");
                                }
                            }
                        }
                    }
                }
                ConfigAction::Get { key } => {
                    let Ok(content) = std::fs::read_to_string(&path) else {
                        eprintln!("Cannot read base.toml");
                        return;
                    };
                    let Ok(val) = content.parse::<toml::Value>() else {
                        eprintln!("Failed to parse base.toml");
                        return;
                    };
                    let parts: Vec<&str> = key.splitn(2, '.').collect();
                    if parts.len() != 2 {
                        eprintln!("Key must be section.field (e.g. memory.mode)");
                        return;
                    }
                    match val.get(parts[0]).and_then(|s| s.get(parts[1])) {
                        Some(v) => println!("{v}"),
                        None => eprintln!("Key '{key}' not found"),
                    }
                }
                ConfigAction::Set { key, value } => {
                    let Ok(content) = std::fs::read_to_string(&path) else {
                        eprintln!("Cannot read base.toml");
                        return;
                    };
                    let Ok(mut doc) = content.parse::<toml::Value>() else {
                        eprintln!("Failed to parse base.toml");
                        return;
                    };
                    let parts: Vec<&str> = key.splitn(2, '.').collect();
                    if parts.len() != 2 {
                        eprintln!("Key must be section.field (e.g. memory.mode)");
                        return;
                    }
                    let section = parts[0];
                    let field = parts[1];

                    let new_val: toml::Value = if value == "true" {
                        toml::Value::Boolean(true)
                    } else if value == "false" {
                        toml::Value::Boolean(false)
                    } else if let Ok(n) = value.parse::<i64>() {
                        toml::Value::Integer(n)
                    } else {
                        toml::Value::String(value.clone())
                    };

                    let table = doc.as_table_mut().unwrap();
                    let sec = table.entry(section).or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
                    if let Some(sec_table) = sec.as_table_mut() {
                        sec_table.insert(field.to_string(), new_val.clone());
                    } else {
                        eprintln!("Section '{section}' is not a table");
                        return;
                    }

                    match toml::to_string_pretty(&doc) {
                        Ok(out) => {
                            if let Err(e) = std::fs::write(&path, out) {
                                eprintln!("Failed to write base.toml: {e}");
                            } else {
                                println!("Updated {key} = {new_val}");
                            }
                        }
                        Err(e) => eprintln!("Failed to serialize: {e}"),
                    }
                }
            }
        },

        Some(Commands::Context { text, list }) => {
            if list {
                domain::query::context_list(&cwd);
            } else if let Some(text) = text {
                domain::query::context_pull(&config, &cwd, &text);
            } else {
                eprintln!("Usage: base context <text> or base context --list");
            }
        },

        // ─── Doctor ───────────────────────────────────────────
        Some(Commands::Doctor { json, repair, restore }) => {
            // Parser-independent: every branch must run BECAUSE the graph is broken.
            if let Some(which) = restore {
                // --restore: workspace tier only (operator's corruptible graph).
                let Some(base_dir) = base::config::find_workspace_base(&cwd) else {
                    eprintln!("doctor --restore: no workspace .base/ found from {}", cwd.display());
                    std::process::exit(1);
                };
                let ws = base_dir.join("graph.nq");
                match which {
                    // Bare `--restore` → list available snapshots, mutate nothing.
                    None => {
                        let baks = base::doctor::list_backups(&ws);
                        if json {
                            let rows: Vec<_> = baks
                                .iter()
                                .map(|(p, n)| serde_json::json!({ "path": p.display().to_string(), "lines": n }))
                                .collect();
                            println!("{}", serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".into()));
                        } else if baks.is_empty() {
                            println!("No backups found next to {}", ws.display());
                        } else {
                            println!("Backups for {} (newest first):", ws.display());
                            for (p, n) in &baks {
                                println!("  {n:>8} lines  {}", p.display());
                            }
                            println!("\nRestore with: base doctor --restore <path>");
                        }
                    }
                    // `--restore <name|path>` → resolve, snapshot current, swap in.
                    Some(arg) => {
                        let candidate = std::path::Path::new(&arg);
                        let backup = if candidate.is_absolute() {
                            candidate.to_path_buf()
                        } else {
                            base_dir.join(&arg)
                        };
                        match base::doctor::restore_tier(&ws, &backup) {
                            Ok(()) => {
                                let after = matches!(
                                    base::store::graph_health(&ws),
                                    base::store::GraphHealth::Healthy
                                );
                                if json {
                                    println!(
                                        "{}",
                                        serde_json::json!({
                                            "restored": ws.display().to_string(),
                                            "from": backup.display().to_string(),
                                            "healthy_after": after
                                        })
                                    );
                                } else {
                                    println!("Restored {} from {}", ws.display(), backup.display());
                                    println!(
                                        "  graph is now {}",
                                        if after { "HEALTHY ✓" } else { "still UNHEALTHY ⚠ (bad snapshot?)" }
                                    );
                                }
                                if !after {
                                    std::process::exit(1);
                                }
                            }
                            Err(e) => {
                                eprintln!("doctor --restore failed: {e}");
                                std::process::exit(1);
                            }
                        }
                    }
                }
            } else if repair {
                // --repair: quarantine bad lines + atomic rewrite of the good set.
                let outcomes = base::doctor::repair(&cwd);
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&outcomes).unwrap_or_else(|_| "[]".into())
                    );
                } else {
                    println!("{}", base::doctor::format_repair_human(&outcomes));
                }
                if outcomes.iter().any(|o| !o.healthy_after) {
                    std::process::exit(1);
                }
            } else {
                // Default: check + report. Print the FULL report, THEN exit nonzero
                // if unhealthy (unlike `die`, which bails on the first error).
                let report = base::doctor::diagnose(&cwd);
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".into())
                    );
                } else {
                    println!("{}", base::doctor::format_human(&report));
                }
                if !report.healthy {
                    std::process::exit(1);
                }
            }
        }

        // ─── Graph ────────────────────────────────────────────
        Some(Commands::Graph { action }) => match action {
            GraphAction::ApplyOps { global } => {
                let tier = tier_cwd(&cwd, global);
                let Some(base_dir) = base::config::find_workspace_base(&tier) else {
                    println!(
                        "{}",
                        serde_json::json!({"error": {
                            "code": "no_workspace",
                            "message": format!("no .base/ directory found from {}", tier.display()),
                        }})
                    );
                    std::process::exit(1);
                };
                let mut input = String::new();
                if let Err(e) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input) {
                    println!(
                        "{}",
                        serde_json::json!({"error": {
                            "code": "stdin_read_failed",
                            "message": e.to_string(),
                        }})
                    );
                    std::process::exit(1);
                }
                let (out, code) = base::apply_ops::run(&base_dir.join("graph.nq"), &input);
                println!("{out}");
                if code != 0 {
                    std::process::exit(code);
                }
            }
            GraphAction::Compact => match base::graph::compact(&cwd) {
                Ok(outcome) => print!("{}", base::graph::format_compact_human(&outcome)),
                Err(e) => {
                    eprintln!("base graph compact failed: {e}");
                    std::process::exit(1);
                }
            },
            GraphAction::Purge { stale, apply, days } => {
                if !stale {
                    eprintln!("Usage: base graph purge --stale [--apply] [--days N]");
                    std::process::exit(2);
                }
                match base::graph::purge(&cwd, &config.namespace, days, apply) {
                    Ok(outcome) => print!("{}", base::graph::format_purge_human(&outcome)),
                    Err(e) => {
                        eprintln!("base graph purge failed: {e}");
                        std::process::exit(1);
                    }
                }
            }
            GraphAction::Extract { target, model, multimodal } => {
                let tp = {
                    let t = target.as_deref().unwrap_or(".");
                    let p = std::path::Path::new(t);
                    if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) }
                };
                let mm_enabled = multimodal || config.multimodal.enabled;
                match base::graph_extract::run(&cwd, &config.namespace, &tp, model.as_deref(), mm_enabled) {
                    Ok(report) => print!("{}", base::graph_extract::format_report(&report)),
                    Err(e) => die("graph extract failed", e),
                }
            }
            GraphAction::Query { question, depth, token_budget, model, raw } => {
                let opts = base::graph_query::Options {
                    depth,
                    token_budget,
                    model: model.as_deref(),
                    raw,
                };
                if let Err(e) = base::graph_query::run(&cwd, &config.namespace, &question, &opts) {
                    die("graph query failed", e);
                }
            }
            GraphAction::Analyze { top_n } => {
                if let Err(e) = base::graph_analyze::run(&cwd, &config.namespace, top_n) {
                    die("graph analyze failed", e);
                }
            }
            GraphAction::GetNode { node } => {
                if let Err(e) = base::graph_tools::get_node(&cwd, &config.namespace, &node) {
                    die("graph get-node failed", e);
                }
            }
            GraphAction::Neighbors { node, depth } => {
                if let Err(e) = base::graph_tools::neighbors(&cwd, &config.namespace, &node, depth) {
                    die("graph neighbors failed", e);
                }
            }
            GraphAction::Path { from, to } => {
                if let Err(e) = base::graph_tools::shortest_path(&cwd, &config.namespace, &from, &to) {
                    die("graph path failed", e);
                }
            }
            GraphAction::Move { select, to, from, dry_run, no_ast, yes } => {
                let selector = match base::graph_move::Selector::parse(&select) {
                    Ok(s) => s,
                    Err(e) => die("graph move", e),
                };
                let from_name = from.unwrap_or_else(|| crud::workspace_slug(&cwd));
                let spec = match base::graph_move::spec_from_names(
                    &from_name, &to, &config.workspace, &config.namespace, no_ast,
                ) {
                    Ok(s) => s,
                    Err(e) => die("graph move", e),
                };
                // The destructive remove-from-source requires --yes; without it (and
                // not an explicit --dry-run) we still show the full plan, write nothing.
                let preview_only = dry_run || !yes;
                match base::graph_move::graph_move(&spec, &selector, &config.namespace, preview_only) {
                    Ok(report) => {
                        print!("{}", base::graph_move::format_report(&report));
                        if preview_only && !dry_run {
                            println!("   Pass --yes to apply.");
                        }
                    }
                    Err(e) => die("graph move failed", e),
                }
            }
        },

        // ─── Secret ───────────────────────────────────────────
        Some(Commands::Slack { action }) => {
            let token = match base::slack::token() {
                Ok(t) => t,
                Err(e) => die("Slack", e),
            };
            match action {
                SlackAction::Post { to, text, thread } => {
                    match base::slack::resolve(&token, &to, thread.as_deref()).and_then(|t| base::slack::post(&token, &t, &text)) {
                        Ok(link) => println!("{link}"),
                        Err(e) => die("Slack", e),
                    }
                }
                SlackAction::Read { to, limit } => {
                    match base::slack::resolve(&token, &to, None).and_then(|t| base::slack::read(&token, &t, limit)) {
                        Ok(lines) => { for l in lines { println!("{l}"); } }
                        Err(e) => die("Slack", e),
                    }
                }
                SlackAction::Channels => match base::slack::channels(&token) {
                    Ok(lines) => { for l in lines { println!("{l}"); } }
                    Err(e) => die("Slack", e),
                },
            }
        }
        Some(Commands::Secret { action }) => match action {
            SecretAction::Set { key } => {
                if let Err(e) = base::secret::set_interactive(&key) {
                    die("Failed", e);
                }
            }
            SecretAction::List => {
                if let Err(e) = base::secret::list() {
                    die("Failed", e);
                }
            }
            SecretAction::Rm { key } => match base::secret::remove(&key) {
                Ok(true) => println!("✓ Removed {key} from ~/.base-gbl/.env"),
                Ok(false) => eprintln!("Secret '{key}' not found"),
                Err(e) => die("Failed", e),
            },
        },

        // ─── Plugin commands (drop-in from extensions) ────────
        // Any unrecognized subcommand lands here (clap external_subcommand).
        // Core commands above always match first, so a plugin can never shadow
        // a built-in. dispatch() never returns (resolves+execs or exits loud).
        Some(Commands::External(args)) => base::plugin::dispatch(&args, &cwd),

        None => eprintln!("No command provided. Run `base --help` for usage."),
    }
}
