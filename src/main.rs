use anyhow::Result;
use clap::{Parser, Subcommand};

mod agentsmd;
#[path = "autopilot/mod.rs"]
mod orchestrator;
mod git;
mod opencode;
mod run;
mod workflow;

pub mod task;

#[derive(Parser)]
#[command(name = "crank")]
#[command(about = "Local merge queue CLI - run CI, review, merge and push")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Launch tmux orchestrator session
    Tmux {
        /// Number of workers to run
        #[arg(long, short)]
        concurrency: u16,

        /// Filter tasks by project/app name
        #[arg(long)]
        project: Option<String>,
    },

    /// Run a tmux worker (internal)
    Worker {
        /// Worker ID (1-based)
        #[arg(long)]
        id: u16,

        /// Filter tasks by project/app name
        #[arg(long)]
        project: Option<String>,
    },

    /// Ask for help and keep the session open
    AskForHelp {
        /// Message describing what is blocking
        #[arg(value_name = "message", num_args = 1..)]
        message: Vec<String>,
    },

    /// Nudge an agent in a tmux pane
    Nudge {
        /// tmux pane id (from $TMUX_PANE)
        #[arg(long)]
        pane: String,
    },

    /// Pause or resume nudges
    Pause {
        /// Clear pause marker
        #[arg(long)]
        clear: bool,
    },
    /// Run agentic code review on a worktree
    Review {
        /// Path to the worktree (defaults to current directory)
        worktree: Option<String>,

        /// Skip running tests
        #[arg(long)]
        skip_tests: bool,

        /// OpenCode server port
        #[arg(long, default_value = "19191")]
        port: u16,

        /// Timeout in milliseconds
        #[arg(long, default_value = "600000")]
        timeout: u64,
    },

    /// Print AGENTS.md-style guide for crank
    #[command(name = "agents.md", alias = "agentsmd")]
    AgentsMd,

    /// Build a workflow instance from a template
    Build(workflow::BuildArgs),

    /// Run the next task (or a specific task/workflow)
    Run(run::RunArgs),

    /// Task tracking commands
    #[command(subcommand)]
    Task(task::cli::TaskCommand),
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Review {
            worktree,
            skip_tests,
            port: _,
            timeout,
        } => {
            let path = worktree.unwrap_or_else(|| ".".to_string());
            opencode::review_command(&path, skip_tests, timeout).await?;
        }

        Commands::AgentsMd => {
            agentsmd::print_agentsmd();
        }

        Commands::Build(args) => {
            let git_root = task::git::git_root()?;
            workflow::build_template_at(&git_root, &args)?;
        }

        Commands::Run(args) => {
            run::run_command(args)?;
        }

        Commands::Task(cmd) => {
            task::cli::run_subcommand(cmd)?;
        }

        Commands::Tmux {
            concurrency,
            project,
        } => {
            orchestrator::tmux::run_tmux(concurrency, project)?;
        }

        Commands::Worker { id, project } => {
            orchestrator::worker::run_worker(id, project).await?;
        }

        Commands::AskForHelp { message } => {
            let msg = message.join(" ");
            let repo_root = task::git::git_root()?;
            let path = orchestrator::controls::ask_for_help(&repo_root, &msg)?;
            println!("Wrote help marker: {}", path.display());
        }

        Commands::Nudge { pane } => {
            let repo_root = task::git::git_root()?;
            orchestrator::controls::nudge(&repo_root, &pane)?;
        }

        Commands::Pause { clear } => {
            let repo_root = task::git::git_root()?;
            let path = orchestrator::controls::pause(&repo_root, clear)?;
            if let Some(path) = path {
                println!("Paused nudges: {}", path.display());
            } else {
                println!("Resumed nudges");
            }
        }
    }

    Ok(())
}
