use anyhow::Result;
use clap::{Args, Parser, Subcommand};

use crate::task::model::SupervisionMode;
use crate::task::{deps, workflow};

#[derive(Parser)]
#[command(name = "task")]
#[command(about = "Task tracking tool")]
#[command(
    long_about = "Task tracking tool. Run without subcommands to pick a task to work on.\n\nThis shows a full-screen TUI picker of tasks sorted by priority.\nOn selection, creates a worktree and launches opencode in a new tmux window."
)]
#[command(subcommand_help_heading = "Task commands")]
pub struct Cli {
    #[command(subcommand)]
    command: Option<TaskCommand>,

    /// Just select task and print path (no worktree/tmux)
    #[arg(long)]
    no_worktree: bool,

    /// Select task by id or path (skip TUI)
    #[arg(long)]
    select: Option<String>,
}

/// Task tracking subcommands (used by both standalone `task` and `crank task`)
#[derive(Subcommand, Clone)]
pub enum TaskCommand {
    /// Create a new task
    Create(CreateArgs),

    /// Pick next task to work on (alias for running task with no subcommand)
    Next(NextArgs),

    /// Claim the next available task (non-interactive)
    Claim(ClaimArgs),

    /// Mark a task as closed
    Done(DoneArgs),

    /// Manage git hooks for tasks
    Hooks(HooksArgs),

    /// Manage task dependencies
    Dep(DepArgs),
}

#[derive(Args, Clone)]
pub struct CreateArgs {
    /// Task title
    #[arg(value_name = "title", num_args = 0..)]
    title: Vec<String>,

    /// Priority 1-5 (5=urgent)
    #[arg(short, long)]
    priority: Option<i32>,

    /// Task supervision mode (supervised or unsupervised)
    #[arg(long, value_enum)]
    supervision: Option<SupervisionMode>,

    /// Open $EDITOR with the task template
    #[arg(short, long, conflicts_with = "oc")]
    edit: bool,

    /// Launch opencode task creator
    #[arg(long, conflicts_with = "edit")]
    oc: bool,

    /// Dependencies (format: type:id,type:id e.g. blocks:21a9)
    #[arg(short, long)]
    deps: Option<String>,
}

#[derive(Args, Clone)]
pub struct NextArgs {
    /// Just select task and print path (no worktree/tmux)
    #[arg(long)]
    no_worktree: bool,

    /// Select task by id or path (skip TUI)
    #[arg(long)]
    select: Option<String>,
}

#[derive(Args, Clone)]
pub struct ClaimArgs {
    /// Output JSON
    #[arg(long)]
    json: bool,
}

#[derive(Args, Clone)]
pub struct DoneArgs {
    /// Task id
    #[arg(value_name = "task-id")]
    task_id: String,

    /// PR number to include in closed status
    #[arg(long)]
    pr: Option<i32>,
}

#[derive(Args, Clone)]
pub struct HooksArgs {
    #[command(subcommand)]
    command: HooksCommand,
}

#[derive(Subcommand, Clone)]
pub enum HooksCommand {
    /// Install git hooks into .githooks
    Install,
}

#[derive(Args, Clone)]
pub struct DepArgs {
    #[command(subcommand)]
    command: DepCommand,
}

#[derive(Subcommand, Clone)]
pub enum DepCommand {
    /// Add a dependency (from depends on to)
    Add(DepAddArgs),

    /// Remove a dependency
    Rm(DepRmArgs),

    /// Show dependency tree
    Tree(DepTreeArgs),

    /// Detect dependency cycles
    Cycles,
}

#[derive(Args, Clone)]
pub struct DepAddArgs {
    /// From task id
    #[arg(value_name = "from-id")]
    from_id: String,

    /// To task id
    #[arg(value_name = "to-id")]
    to_id: String,

    /// Dependency type (blocks)
    #[arg(short, long, default_value = "blocks")]
    dep_type: String,
}

#[derive(Args, Clone)]
pub struct DepRmArgs {
    /// From task id
    #[arg(value_name = "from-id")]
    from_id: String,

    /// To task id
    #[arg(value_name = "to-id")]
    to_id: String,
}

#[derive(Args, Clone)]
pub struct DepTreeArgs {
    /// Task id
    #[arg(value_name = "id")]
    id: Option<String>,

    /// Show what depends on this task
    #[arg(short, long)]
    reverse: bool,
}

/// Run a single TaskCommand (used by `crank task <subcommand>`)
pub fn run_subcommand(cmd: TaskCommand) -> Result<()> {
    match cmd {
        TaskCommand::Create(args) => {
            let title = if args.title.is_empty() {
                None
            } else {
                Some(args.title.join(" "))
            };
            workflow::run_create(
                title,
                args.priority,
                args.supervision,
                args.edit,
                args.oc,
                args.deps,
            )
        }
        TaskCommand::Next(args) => workflow::run_next(args.no_worktree, args.select),
        TaskCommand::Claim(args) => workflow::run_claim(args.json),
        TaskCommand::Done(args) => workflow::run_done(&args.task_id, args.pr),
        TaskCommand::Hooks(args) => match args.command {
            HooksCommand::Install => workflow::run_hooks_install(),
        },
        TaskCommand::Dep(args) => match args.command {
            DepCommand::Add(args) => deps::run_add(&args.from_id, &args.to_id, &args.dep_type),
            DepCommand::Rm(args) => deps::run_rm(&args.from_id, &args.to_id),
            DepCommand::Tree(args) => deps::run_tree(args.id.as_deref(), args.reverse),
            DepCommand::Cycles => deps::run_cycles(),
        },
    }
}

/// Run the standalone `task` CLI (parses args itself)
pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(cmd) => run_subcommand(cmd),
        None => workflow::run_next(cli.no_worktree, cli.select),
    }
}
