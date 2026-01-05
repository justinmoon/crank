use anyhow::Result;
use clap::{Parser, Subcommand};

mod agentsmd;
mod crank_io;
mod git;
mod opencode;
mod tutorial;
use task::model::SupervisionMode;

#[path = "autopilot/mod.rs"]
mod orchestrator;
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

        /// Worker mode (supervised or unsupervised)
        #[arg(long, short, value_enum)]
        mode: SupervisionMode,

        /// Filter tasks by project/app name
        #[arg(long, short)]
        project: Option<String>,
    },

    /// Launch zellij orchestrator session
    Zellij {
        /// Number of workers to run
        #[arg(long, short)]
        concurrency: u16,

        /// Worker mode (supervised or unsupervised)
        #[arg(long, short, value_enum)]
        mode: SupervisionMode,

        /// Filter tasks by project/app name
        #[arg(long, short)]
        project: Option<String>,
    },

    /// Run a tmux worker (internal)
    Worker {
        /// Worker ID (1-based)
        #[arg(long, short)]
        id: u16,

        /// Worker mode (supervised or unsupervised)
        #[arg(long, short, value_enum)]
        mode: SupervisionMode,

        /// Filter tasks by project/app name
        #[arg(long, short)]
        project: Option<String>,
    },

    /// Ask for help and keep the session open
    AskForHelp {
        /// Message describing what is blocking
        #[arg(value_name = "message", num_args = 1..)]
        message: Vec<String>,
    },

    /// Nudge an agent in a tmux or zellij pane
    Nudge {
        /// tmux pane id (from $TMUX_PANE) or zellij:<pane_id>
        #[arg(long)]
        pane: String,
    },

    /// Pause or resume nudges
    Pause {
        /// Clear pause marker
        #[arg(long)]
        clear: bool,
    },

    /// Mark the current task done and notify the worker
    Done {
        /// Explicit task id (defaults to $CRANK_TASK_ID or .crank/.current)
        #[arg(long)]
        task: Option<String>,
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

    /// Browse merge tutorials (inbox view)
    Inbox,

    /// Tutorial commands (generate/show)
    #[command(subcommand)]
    Tutorial(tutorial::cli::TutorialCommand),

    /// Show active alerts in a tmux popup
    Alerts {
        /// Watch for new alerts and auto-pop the list
        #[arg(long)]
        watch: bool,
    },

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

        Commands::Inbox => {
            let repo_root = task::git::repo_root()?;
            tutorial::inbox::run_inbox(&repo_root)?;
        }

        Commands::Tutorial(cmd) => {
            tutorial::cli::run_command(cmd)?;
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
            mode,
            project,
        } => {
            orchestrator::tmux::run_tmux(concurrency, mode, project)?;
        }

        Commands::Zellij {
            concurrency,
            mode,
            project,
        } => {
            orchestrator::zellij::run_zellij(concurrency, mode, project)?;
        }

        Commands::Worker { id, mode, project } => {
            orchestrator::worker::run_worker(id, mode, project).await?;
        }

        Commands::AskForHelp { message } => {
            let msg = message.join(" ");
            let repo_root = task::git::git_root()?;
            let path = orchestrator::controls::ask_for_help(&repo_root, &msg)?;
            println!("Wrote help marker: {}", path.display());
        }

        Commands::Alerts { watch } => {
            if watch {
                orchestrator::alerts::run_alerts_watch()?;
            } else {
                orchestrator::alerts::run_alerts_picker()?;
            }
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

        Commands::Done { task } => {
            let repo_root = task::git::git_root()?;
            let task_id =
                if let Some(task_id) = task.or_else(|| std::env::var("CRANK_TASK_ID").ok()) {
                    task_id
                } else {
                    orchestrator::markers::read_current_task_id(&repo_root)?
                };
            let path = orchestrator::markers::write_merged_marker(&task_id)?;
            let task_path = task::git::task_path_for_id(&repo_root, &task_id);
            if task_path.exists() {
                task::store::update_task_status(&task_path, task::model::TASK_STATUS_CLOSED)?;
            }
            println!("Marked task done: {} ({})", task_id, path.display());
        }
    }

    Ok(())
}
