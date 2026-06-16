use clap::{Parser, Subcommand};

use base::command;
use base::config::BaseConfig;
use base::crud;
use base::domain;
use base::extension;
use base::hook;

#[derive(Parser)]
#[command(
    name = "base",
    version,
    about = "BASE — Proactive context-injection engine for Claude Code",
    after_help = "Built by Chris Kahler · Chris AI Systems\n\
                  Community & support: https://chrisai.cv/skool\n\
                  Tutorials: https://www.youtube.com/@chris-ai-systems"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Handle Claude Code hook events (session-start, post-tool-use, user-prompt-submit)
    Hook {
        /// Hook event type
        event: String,
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
        /// Repair missing edges (backfill decision→domain, milestone→project, task→project links)
        #[arg(long)]
        repair: bool,
    },
    /// Manage domain matching rules
    Domain {
        #[command(subcommand)]
        action: DomainAction,
    },
    /// Graph-backed structured memory
    Learn {
        /// The memory text to store (required unless --mention)
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
    },
    /// Search notes by keyword and/or domain
    Recall {
        /// Search text in note content
        #[arg(long)]
        keyword: Option<String>,
        /// Filter by linked domain
        #[arg(long)]
        domain: Option<String>,
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
    },
    /// Activate ChrisAI — enter your Skool classroom key to remove attribution
    Activate {
        /// Activation key from ChrisAI community
        key: String,
    },
    /// Check for updates, snooze the update banner, or install available updates
    Update {
        /// Check for updates without installing
        #[arg(long)]
        check: bool,
        /// Dismiss update banner for 24 hours
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
    },
    /// Remove an installed extension by name
    Remove {
        /// Extension name to remove
        name: String,
    },
}

#[derive(Subcommand)]
pub enum CommandAction {
    /// List all configured star commands
    List,
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
        /// Project path (REQUIRED — auto-creates domain trigger for file matching)
        #[arg(short, long)]
        path: String,
    },
    /// List all projects
    #[command(visible_alias = "l")]
    List,
    /// Show a specific project (accepts slug or display name)
    Get { slug: String },
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
    },
    /// Show a specific milestone
    Get { slug: String },
    /// Update a milestone
    #[command(visible_alias = "u")]
    Update {
        slug: String,
        #[arg(short, long)]
        status: Option<String>,
        #[arg(short, long)]
        description: Option<String>,
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
    /// List tasks (filter by project or milestone)
    #[command(visible_alias = "l")]
    List {
        /// Project slug or display name
        #[arg(short, long)]
        project: Option<String>,
        /// Milestone slug to filter by
        #[arg(short, long)]
        milestone: Option<String>,
    },
    /// Mark a task as completed
    Done { slug: String },
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

#[derive(Subcommand)]
pub enum ReminderAction {
    /// Add a reminder
    Add {
        #[arg(long)]
        name: String,
        /// Due date (YYYY-MM-DD)
        #[arg(long)]
        due: String,
    },
    /// List all reminders
    List,
    /// Remove a reminder (hard delete)
    Remove { slug: String },
}

#[derive(Subcommand)]
pub enum RuleAction {
    /// Add a rule to a domain in the graph
    Add {
        #[arg(long)]
        domain: String,
        #[arg(long)]
        text: String,
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

        // ─── AST Query ──────────────────────────────────
        Some(Commands::Ast { action }) => match action {
            AstAction::Query { contains, file, calls, imports } => {
                if let Some(name) = contains {
                    if let Err(e) = crud::ast_query::contains(&cwd, &config.namespace, &name) { eprintln!("Error: {e}"); }
                } else if let Some(path) = file {
                    if let Err(e) = crud::ast_query::file(&cwd, &config.namespace, &path) { eprintln!("Error: {e}"); }
                } else if let Some(name) = calls {
                    if let Err(e) = crud::ast_query::calls(&cwd, &config.namespace, &name) { eprintln!("Error: {e}"); }
                } else if let Some(path) = imports {
                    if let Err(e) = crud::ast_query::imports(&cwd, &config.namespace, &path) { eprintln!("Error: {e}"); }
                } else {
                    eprintln!("Provide one of: --contains, --file, --calls, --imports");
                }
            }
        },

        // ─── Project ─────────────────────────────────────
        Some(Commands::Project { action }) => match action {
            ProjectAction::Add { name, status, path } => {
                match crud::project::add(&cwd, &config.namespace, &name, &status, Some(&path)) {
                    Ok(slug) => println!("Project '{name}' created (slug: {slug})"),
                    Err(e) => eprintln!("Failed: {e}"),
                }
            }
            ProjectAction::List => { if let Err(e) = crud::project::list(&cwd, &config.namespace) { eprintln!("Error: {e}"); } }
            ProjectAction::Get { slug } => {
                if let Some(s) = resolve(&cwd, &config.namespace, "project", &slug)
                    && let Err(e) = crud::project::get(&cwd, &config.namespace, &s) { eprintln!("Error: {e}"); }
            }
            ProjectAction::Update { slug, status, blocked_by, next_action } => {
                if let Some(s) = resolve(&cwd, &config.namespace, "project", &slug) {
                    match crud::project::update(&cwd, &config.namespace, &s, status.as_deref(), blocked_by.as_deref(), next_action.as_deref()) {
                        Ok(()) => println!("Project '{s}' updated"),
                        Err(e) => eprintln!("Failed: {e}"),
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
                    Err(e) => eprintln!("Failed: {e}"),
                }
            }
            MilestoneAction::List { project } => {
                let ps = match project.as_deref() {
                    Some(p) => match resolve(&cwd, &config.namespace, "project", p) {
                        Some(s) => Some(s),
                        None => return,
                    },
                    None => None,
                };
                if let Err(e) = crud::milestone::list(&cwd, &config.namespace, ps.as_deref()) { eprintln!("Error: {e}"); }
            }
            MilestoneAction::Get { slug } => {
                if let Some(s) = resolve(&cwd, &config.namespace, "milestone", &slug)
                    && let Err(e) = crud::milestone::get(&cwd, &config.namespace, &s) { eprintln!("Error: {e}"); }
            }
            MilestoneAction::Update { slug, status, description } => {
                if let Some(s) = resolve(&cwd, &config.namespace, "milestone", &slug) {
                    match crud::milestone::update(&cwd, &config.namespace, &s, status.as_deref(), description.as_deref()) {
                        Ok(()) => println!("Milestone '{s}' updated"),
                        Err(e) => eprintln!("Failed: {e}"),
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
                    Err(e) => eprintln!("Failed: {e}"),
                }
            }
            TaskAction::List { project, milestone } => {
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
                if let Err(e) = crud::task::list(&cwd, &config.namespace, ps.as_deref(), ms.as_deref()) { eprintln!("Error: {e}"); }
            }
            TaskAction::Done { slug } => {
                if let Some(s) = resolve(&cwd, &config.namespace, "task", &slug) {
                    match crud::task::done(&cwd, &config.namespace, &s) {
                        Ok(()) => println!("Task '{s}' completed"),
                        Err(e) => eprintln!("Failed: {e}"),
                    }
                }
            }
        },

        // ─── Decision ────────────────────────────────────
        Some(Commands::Decision { action }) => match action {
            DecisionAction::Log { domain, decision, rationale, recall } => {
                match crud::decision::log(&cwd, &config.namespace, &domain, &decision, &rationale, recall.as_deref()) {
                    Ok(slug) => println!("Decision logged (slug: {slug})"),
                    Err(e) => eprintln!("Failed: {e}"),
                }
            }
            DecisionAction::Search { keyword } => { if let Err(e) = crud::decision::search(&cwd, &config.namespace, &keyword) { eprintln!("Error: {e}"); } }
        },

        // ─── Entity ──────────────────────────────────────
        Some(Commands::Entity { action }) => match action {
            EntityAction::Add { name, entity_type, domain, project } => {
                match crud::entity::add(&cwd, &config.namespace, &name, &entity_type, &domain, project.as_deref()) {
                    Ok(slug) => println!("Entity '{name}' created (slug: {slug}, domain: {domain})"),
                    Err(e) => eprintln!("Failed: {e}"),
                }
            }
            EntityAction::List => { if let Err(e) = crud::entity::list(&cwd, &config.namespace) { eprintln!("Error: {e}"); } }
            EntityAction::Get { slug } => {
                if let Some(s) = resolve(&cwd, &config.namespace, "entity", &slug)
                    && let Err(e) = crud::entity::get(&cwd, &config.namespace, &s) { eprintln!("Error: {e}"); }
            }
            EntityAction::Update { slug, status, description } => {
                if let Some(s) = resolve(&cwd, &config.namespace, "entity", &slug) {
                    match crud::entity::update(&cwd, &config.namespace, &s, status.as_deref(), description.as_deref()) {
                        Ok(()) => println!("Entity '{s}' updated"),
                        Err(e) => eprintln!("Failed: {e}"),
                    }
                }
            }
        },

        // ─── Goal ────────────────────────────────────────
        Some(Commands::Goal { action }) => match action {
            GoalAction::Add { name, target } => {
                match crud::goal::add(&cwd, &config.namespace, &name, &target) {
                    Ok(slug) => println!("Goal '{name}' created (slug: {slug})"),
                    Err(e) => eprintln!("Failed: {e}"),
                }
            }
            GoalAction::List => { if let Err(e) = crud::goal::list(&cwd, &config.namespace) { eprintln!("Error: {e}"); } }
            GoalAction::Update { slug, status, target } => {
                if let Some(s) = resolve(&cwd, &config.namespace, "goal", &slug) {
                    match crud::goal::update(&cwd, &config.namespace, &s, status.as_deref(), target.as_deref()) {
                        Ok(()) => println!("Goal '{s}' updated"),
                        Err(e) => eprintln!("Failed: {e}"),
                    }
                }
            }
        },

        // ─── Reminder ────────────────────────────────────
        Some(Commands::Reminder { action }) => match action {
            ReminderAction::Add { name, due } => {
                match crud::reminder::add(&cwd, &config.namespace, &name, &due) {
                    Ok(slug) => println!("Reminder '{name}' created (slug: {slug})"),
                    Err(e) => eprintln!("Failed: {e}"),
                }
            }
            ReminderAction::List => { if let Err(e) = crud::reminder::list(&cwd, &config.namespace) { eprintln!("Error: {e}"); } }
            ReminderAction::Remove { slug } => {
                match crud::reminder::remove(&cwd, &config.namespace, &slug) {
                    Ok(()) => println!("Reminder '{slug}' removed"),
                    Err(e) => eprintln!("Failed: {e}"),
                }
            }
        },

        // ─── Sync ────────────────────────────────────────
        Some(Commands::Sync { incremental, ast, target, repair }) => {
            if repair {
                match base::crud::repair_edges(&cwd, &config.namespace) {
                    Ok(count) => println!("Repair complete: {count} edges backfilled"),
                    Err(e) => eprintln!("Repair failed: {e}"),
                }
                return;
            }
            if ast {
                // AST extraction via bundled Python scripts
                let target_dir = target.as_deref().unwrap_or(".");
                let home = dirs::home_dir().unwrap_or_default();

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

                let base_dir = base::config::find_workspace_base(&cwd)
                    .unwrap_or_else(|| cwd.join(".base"));
                let ast_ttl = base_dir.join("ast.ttl");

                // AST data lives ONLY in ast.ttl. Never write it into graph.nq —
                // Turtle appended to an NQuads file corrupts the whole graph (AUDIT C10).
                println!("AST extraction: {} → {}", target_dir, ast_ttl.display());
                let status = std::process::Command::new("python3")
                    .arg(&ast_script)
                    .arg(target_dir)
                    .arg("--full")
                    .arg("--out")
                    .arg(&ast_ttl)
                    .status();

                match status {
                    Ok(s) if s.success() => {
                        println!("AST extraction complete → {}", ast_ttl.display());
                    }
                    Ok(s) => eprintln!("AST extraction exited with code {:?}", s.code()),
                    Err(e) => eprintln!("Failed to run AST extractor: {e}"),
                }
            } else {
                match base::extract::sync(&cwd, &config, incremental) {
                    Ok(report) => {
                        println!(
                            "Sync complete: {} scanned, {} extracted, {} skipped",
                            report.scanned, report.extracted, report.skipped
                        );
                    }
                    Err(e) => eprintln!("Sync failed: {e}"),
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
                    Err(e) => eprintln!("Failed: {e}"),
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
                    Err(e) => eprintln!("Domain sync failed: {e}"),
                }
            }
        },

        // ─── Rule ─────────────────────────────────────────
        Some(Commands::Rule { global, action }) => {
            let rule_cwd = if global {
                match dirs::home_dir() {
                    Some(h) => h.join(".base-gbl"),
                    None => {
                        eprintln!("Failed: cannot determine home directory");
                        return;
                    }
                }
            } else {
                cwd.clone()
            };
            match action {
                RuleAction::Add { domain: name, text } => {
                    match crud::rule::add(&rule_cwd, &config.namespace, &name, &text) {
                        Ok(index) => println!("Rule {index} added to domain '{name}'"),
                        Err(e) => eprintln!("Failed: {e}"),
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
                        Err(e) => eprintln!("Failed: {e}"),
                    }
                }
            }
        },

        // ─── Learn ────────────────────────────────────────
        Some(Commands::Learn { text, r#type, domain, project, entity, mention, context }) => {
            if let Some(slug) = mention {
                // Mention mode: increment mention count on existing note
                match crud::note::mention(
                    &cwd,
                    &config.namespace,
                    &slug,
                    context.as_deref(),
                ) {
                    Ok(count) => println!("Mention recorded: {slug} (count: {count})"),
                    Err(e) => eprintln!("Failed: {e}"),
                }
            } else {
                // Standard learn mode: create a new note
                let Some(text) = text else {
                    eprintln!("--text is required (or use --mention <slug> for mention tracking)");
                    std::process::exit(1);
                };
                let Some(domain) = domain else {
                    eprintln!("--domain is required (or use --mention <slug> for mention tracking)");
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
                    Err(e) => eprintln!("Failed: {e}"),
                }
            }
        }

        // ─── Recall ─────────────────────────────────────────
        Some(Commands::Recall { keyword, domain }) => {
            if keyword.is_none() && domain.is_none() {
                eprintln!("Provide --keyword and/or --domain");
                return;
            }
            if let Err(e) = crud::note::recall(&cwd, &config.namespace, keyword.as_deref(), domain.as_deref()) { eprintln!("Error: {e}"); }
        }

        // ─── Install ─────────────────────────────────────────
        Some(Commands::Install { carl, skip_hooks, full }) => {
            let carl_path = carl.as_ref().map(std::path::Path::new);
            if let Err(e) = base::install::run(carl_path, skip_hooks, full) {
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
        Some(Commands::Update { check, snooze }) => {
            if snooze {
                if let Err(e) = base::manifest::snooze() {
                    eprintln!("Snooze failed: {e}");
                }
            } else if check {
                let mut manifest = base::manifest::Manifest::load().unwrap_or_default();
                match base::manifest::check_for_updates(&mut manifest) {
                    Ok(Some(pending)) => {
                        if let Err(e) = manifest.save() {
                            eprintln!("Failed to save manifest: {e}");
                        }
                        println!("Updates available: {pending}");
                        println!("\nRun `base update` to install (coming soon).");
                    }
                    Ok(None) => {
                        if let Err(e) = manifest.save() {
                            eprintln!("Failed to save manifest: {e}");
                        }
                        println!("All components are up to date.");
                    }
                    Err(e) => eprintln!("Update check failed: {e}"),
                }
            } else {
                // Bare `base update` — download not implemented yet (Phase 12)
                let manifest = base::manifest::Manifest::load().unwrap_or_default();
                if !manifest.update_check.pending_update.is_empty() {
                    println!("Pending updates: {}", manifest.update_check.pending_update);
                    println!("\nAutomatic update download coming in a future release.");
                    println!("Use `base update --check` to refresh, or `base update --snooze` to dismiss.");
                } else {
                    println!("No pending updates. Run `base update --check` to check now.");
                }
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
            ExtensionAction::Install { path } => {
                let p = std::path::Path::new(&path);
                match extension::validate_extension(p) {
                    Ok(ext) => {
                        let home = dirs::home_dir().expect("Cannot determine home directory");
                        let ext_dir = home.join(".base-gbl").join("extensions");
                        if let Err(e) = std::fs::create_dir_all(&ext_dir) {
                            eprintln!("Failed to create extensions directory: {e}");
                            std::process::exit(1);
                        }
                        let dest = ext_dir.join(format!("{}.toml", ext.name));

                        // Check for existing version
                        let old_version = if dest.exists() {
                            std::fs::read_to_string(&dest)
                                .ok()
                                .and_then(|c| toml::from_str::<extension::ExtensionFile>(&c).ok())
                                .map(|f| f.extension.version)
                        } else {
                            None
                        };

                        if let Err(e) = std::fs::copy(p, &dest) {
                            eprintln!("Failed to install extension: {e}");
                            std::process::exit(1);
                        }

                        if let Some(old_v) = old_version {
                            println!(
                                "✓ Updated {} (was v{}, now v{})",
                                ext.name, old_v, ext.version
                            );
                        } else {
                            println!("✓ Installed {} v{}", ext.name, ext.version);
                        }
                        println!("  → {}", dest.display());
                    }
                    Err(violations) => {
                        eprintln!("✗ Cannot install — validation failed:");
                        for v in &violations {
                            eprintln!("  - {v}");
                        }
                        std::process::exit(1);
                    }
                }
            }
            ExtensionAction::Remove { name } => {
                let home = dirs::home_dir().expect("Cannot determine home directory");
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
        },

        None => eprintln!("No command provided. Run `base --help` for usage."),
    }
}
