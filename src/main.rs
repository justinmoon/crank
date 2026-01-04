use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod agentsmd;
mod approval;
mod autopilot;
mod git;
mod merge_steps;
mod opencode;
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
    /// Launch tmux autopilot session
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
    /// Run CI + review, merge and push to origin if both pass
    Merge {
        /// Path to the worktree (defaults to current directory)
        worktree: Option<String>,

        /// Don't actually merge, just check
        #[arg(long)]
        dry_run: bool,

        /// Base branch to merge into
        #[arg(long, default_value = "master")]
        base: String,

        /// Target repo for merge/push (defaults to main worktree)
        #[arg(long)]
        target_repo: Option<String>,

        /// Skip running just pre-merge
        #[arg(long)]
        skip_pre_merge: bool,

        /// Skip agentic review
        #[arg(long)]
        skip_review: bool,

        /// OpenCode server port
        #[arg(long, default_value = "19191")]
        port: u16,

        /// Timeout in milliseconds
        #[arg(long, default_value = "600000")]
        timeout: u64,

        /// Wait for human approval before merging
        #[arg(long)]
        notify: bool,

        /// Merge automatically without approval (default)
        #[arg(long)]
        auto: bool,

        /// Interval between notifications in ms
        #[arg(long, default_value = "60000")]
        notify_interval: u64,
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

    /// Approve a pending merge
    Approve {
        /// Branch name or ID to approve
        id: Option<String>,
    },

    /// Reject a pending merge
    Reject {
        /// Branch name or ID to reject
        id: Option<String>,
    },

    /// List pending merges awaiting approval
    Pending,

    /// Print AGENTS.md-style guide for crank
    #[command(name = "agents.md", alias = "agentsmd")]
    AgentsMd,

    /// Show status of active merge operations
    Status {
        /// Watch mode - continuously update
        #[arg(long, short)]
        watch: bool,

        /// Follow output log for a specific merge ID
        #[arg(long, short)]
        follow: Option<String>,
    },

    /// Attach to a running merge's review agent in opencode TUI
    Attach {
        /// Merge ID (optional if only one merge running)
        id: Option<String>,
    },

    /// Workflow commands
    #[command(subcommand)]
    Workflow(workflow::WorkflowCommand),

    /// Merge step commands (internal)
    #[command(subcommand, hide = true)]
    MergeStep(merge_steps::MergeStepCommand),

    /// Task tracking commands
    #[command(subcommand)]
    Task(task::cli::TaskCommand),
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Merge {
            worktree,
            dry_run,
            base,
            target_repo,
            skip_pre_merge,
            skip_review,
            port: _,
            timeout,
            notify,
            auto,
            notify_interval,
        } => {
            let worktree = worktree.unwrap_or_else(|| ".".to_string());
            merge_workflow_command(git::MergeOptions {
                worktree,
                dry_run,
                base,
                target_repo,
                skip_pre_merge,
                skip_review,
                timeout,
                notify: notify && !auto,
                notify_interval,
            })
            .await?;
        }

        Commands::Review {
            worktree,
            skip_tests,
            port: _,
            timeout,
        } => {
            let path = worktree.unwrap_or_else(|| ".".to_string());
            opencode::review_command(&path, skip_tests, timeout).await?;
        }

        Commands::Approve { id } => {
            approval::approve_command(id.as_deref()).await?;
        }

        Commands::Reject { id } => {
            approval::reject_command(id.as_deref()).await?;
        }

        Commands::Pending => {
            approval::pending_command().await?;
        }

        Commands::AgentsMd => {
            agentsmd::print_agentsmd();
        }

        Commands::Status { watch, follow } => {
            status_command(watch, follow).await?;
        }

        Commands::Attach { id } => {
            attach_command(id).await?;
        }

        Commands::Workflow(cmd) => {
            workflow::run_command(cmd).await?;
        }

        Commands::MergeStep(cmd) => {
            merge_steps::run_command(cmd).await?;
        }

        Commands::Task(cmd) => {
            task::cli::run_subcommand(cmd)?;
        }

        Commands::Tmux {
            concurrency,
            project,
        } => {
            autopilot::tmux::run_tmux(concurrency, project)?;
        }

        Commands::Worker { id, project } => {
            autopilot::worker::run_worker(id, project).await?;
        }

        Commands::AskForHelp { message } => {
            let msg = message.join(" ");
            let repo_root = task::git::git_root()?;
            let path = autopilot::controls::ask_for_help(&repo_root, &msg)?;
            println!("Wrote help marker: {}", path.display());
        }

        Commands::Nudge { pane } => {
            let repo_root = task::git::git_root()?;
            autopilot::controls::nudge(&repo_root, &pane)?;
        }

        Commands::Pause { clear } => {
            let repo_root = task::git::git_root()?;
            let path = autopilot::controls::pause(&repo_root, clear)?;
            if let Some(path) = path {
                println!("Paused nudges: {}", path.display());
            } else {
                println!("Resumed nudges");
            }
        }
    }

    Ok(())
}

async fn merge_workflow_command(opts: git::MergeOptions) -> Result<()> {
    let worktree_path = std::fs::canonicalize(&opts.worktree)
        .with_context(|| format!("invalid worktree path: {}", opts.worktree))?;
    let git_root = git::get_git_root(&worktree_path).await?;
    let branch = git::get_current_branch(&git_root).await?;
    let short_commit = git::get_head_commit(&git_root).await?;
    let workflow_id = format!(
        "merge-{}-{}",
        sanitize_workflow_component(&branch),
        short_commit
    );

    let target_repo_flag = match opts.target_repo.as_deref() {
        Some(path) => {
            let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
            format!("--target-repo \"{}\"", resolved.display())
        }
        None => String::new(),
    };

    let vars = vec![
        format!("base={}", opts.base),
        format!("worktree={}", worktree_path.display()),
        format!("timeout={}", opts.timeout),
        format!("notify_interval={}", opts.notify_interval),
        format!(
            "skip_pre_merge_flag={}",
            flag_value(opts.skip_pre_merge, "--skip")
        ),
        format!(
            "skip_review_flag={}",
            flag_value(opts.skip_review, "--skip")
        ),
        format!(
            "review_skip_tests_flag={}",
            flag_value(!opts.skip_pre_merge, "--skip-tests")
        ),
        format!("dry_run_flag={}", flag_value(opts.dry_run, "--dry-run")),
        format!("notify_flag={}", flag_value(opts.notify, "--notify")),
        format!("target_repo_flag={}", target_repo_flag),
    ];

    let existing = task::store::load_tasks(&git_root)?;
    let has_workflow = existing
        .iter()
        .any(|task| task.workflow.as_deref() == Some(&workflow_id));

    if !has_workflow {
        let apply_args = workflow::WorkflowApplyArgs {
            template: "merge".to_string(),
            id: Some(workflow_id.clone()),
            vars,
            ephemeral: true,
            force: false,
        };
        workflow::apply_template_at(&git_root, &apply_args)?;
    }

    workflow::run_workflow_at(&git_root, &workflow_id, 2).await
}

fn flag_value(enabled: bool, flag: &str) -> String {
    if enabled {
        flag.to_string()
    } else {
        String::new()
    }
}

fn sanitize_workflow_component(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

async fn attach_command(id: Option<String>) -> Result<()> {
    // First try active merges
    let active_merges = git::list_active_merges();

    // Also load completed/recent merges from disk
    let all_merges = git::list_all_merges();

    let merge = if let Some(id) = id {
        // Find by ID prefix in all merges
        all_merges
            .into_iter()
            .find(|m| m.id == id || m.id.starts_with(&id))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No merge found with ID: {}. Run 'crank status' to see available merges.",
                    id
                )
            })?
    } else if active_merges.len() == 1 {
        // If exactly one active merge, use it
        active_merges.into_iter().next().unwrap()
    } else if !active_merges.is_empty() {
        // Multiple active merges
        eprintln!("Multiple merges running. Specify an ID:");
        for m in &active_merges {
            eprintln!("  crank attach {}", m.id);
        }
        std::process::exit(1);
    } else if all_merges.len() == 1 {
        // No active, but one recent merge
        all_merges.into_iter().next().unwrap()
    } else if all_merges.is_empty() {
        return Err(anyhow::anyhow!("No merges found. Run 'crank merge' first."));
    } else {
        // Multiple completed merges, show most recent
        eprintln!("No active merge. Recent merges:");
        for m in all_merges.iter().take(5) {
            let status_icon = match m.status.as_str() {
                "pass" => "✓",
                "fail" => "✗",
                "running" => "◐",
                _ => "?",
            };
            eprintln!("  {} {} - {}", status_icon, m.id, m.branch);
        }
        eprintln!("\nSpecify an ID: crank attach <id>");
        std::process::exit(1);
    };

    // Get the review session ID
    let review_step = merge.steps.iter().find(|s| s.name == "review");
    let session_id = review_step
        .and_then(|s| s.session_id.as_ref())
        .ok_or_else(|| anyhow::anyhow!("Review session not found for merge {}", merge.id))?;

    eprintln!("Opening review session in opencode TUI...");
    eprintln!("Directory: {}", merge.worktree);
    eprintln!("Session:   {}", session_id);
    eprintln!();

    // Launch opencode with --session to view/continue the review
    let status = std::process::Command::new("opencode")
        .arg("--session")
        .arg(session_id)
        .current_dir(&merge.worktree)
        .status()?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}

async fn status_command(watch: bool, follow: Option<String>) -> Result<()> {
    use std::io::{Read, Seek, SeekFrom};

    if let Some(id) = follow {
        // Follow a specific merge's output log
        let log_path = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join(".crank")
            .join("merges")
            .join(format!("{}.log", id));

        let progress_path = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join(".crank")
            .join("merges")
            .join(format!("{}.json", id));

        if !progress_path.exists() {
            eprintln!("No merge found with ID: {}", id);
            std::process::exit(1);
        }

        let mut last_pos = 0u64;
        loop {
            // Print new lines from log
            if log_path.exists() {
                if let Ok(mut file) = std::fs::File::open(&log_path) {
                    file.seek(SeekFrom::Start(last_pos))?;
                    let mut buffer = String::new();
                    let bytes_read = file.read_to_string(&mut buffer)?;
                    if bytes_read > 0 {
                        print!("{}", buffer);
                        last_pos += bytes_read as u64;
                    }
                }
            }

            // Check if merge is still running
            if let Ok(content) = std::fs::read_to_string(&progress_path) {
                if let Ok(progress) = serde_json::from_str::<git::MergeProgress>(&content) {
                    if progress.status != "running" {
                        eprintln!("\n--- Merge {} ---", progress.status);
                        break;
                    }
                }
            } else {
                eprintln!("\n--- Merge file removed ---");
                break;
            }

            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        return Ok(());
    }

    loop {
        // Clear screen in watch mode
        if watch {
            print!("\x1B[2J\x1B[1;1H");
        }

        let merges = git::list_active_merges();

        if merges.is_empty() {
            println!("No active merges");
        } else {
            println!("Active merges:\n");
            for merge in &merges {
                let elapsed = (git::now_ms_pub() - merge.started_at) / 1000;
                println!(
                    "  {} [{}] {} -> {}",
                    merge.id, merge.status, merge.branch, merge.base
                );
                println!("    worktree: {}", merge.worktree);
                println!("    elapsed:  {}s", elapsed);
                println!("    steps:");
                for step in &merge.steps {
                    let status_icon = match step.status.as_str() {
                        "pending" => "○",
                        "running" => "◐",
                        "pass" => "✓",
                        "fail" => "✗",
                        _ => "?",
                    };
                    let mut line = format!("      {} {}", status_icon, step.name);
                    if step.status == "running" {
                        line.push_str(&format!(" ({} lines)", step.output_lines));
                        if let Some(ref last) = step.last_output {
                            let truncated: String = last.chars().take(60).collect();
                            line.push_str(&format!(" - {}", truncated));
                        }
                    }
                    println!("{}", line);
                }
                println!();
                println!("    follow: crank status -f {}", merge.id);
                println!();
            }
        }

        if !watch {
            break;
        }

        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }

    Ok(())
}
