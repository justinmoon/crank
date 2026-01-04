use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use clap::{Args, Subcommand};

use crate::approval;
use crate::autopilot::markers;
use crate::git;

#[derive(Subcommand, Clone)]
pub enum MergeStepCommand {
    /// Run merge preflight checks
    Preflight(MergeStepArgs),

    /// Run pre-merge checks
    PreMerge(PreMergeArgs),

    /// Run agentic review
    Review(ReviewArgs),

    /// Check for merge conflicts
    Conflicts(MergeStepArgs),

    /// Wait for merge approval
    Approval(ApprovalArgs),

    /// Apply merge and push
    Apply(ApplyArgs),
}

#[derive(Args, Clone)]
pub struct MergeStepArgs {
    /// Worktree path (defaults to current directory)
    #[arg(long)]
    pub worktree: Option<String>,

    /// Base branch to merge into
    #[arg(long, default_value = "master")]
    pub base: String,
}

#[derive(Args, Clone)]
pub struct PreMergeArgs {
    /// Worktree path (defaults to current directory)
    #[arg(long)]
    pub worktree: Option<String>,

    /// Timeout in milliseconds
    #[arg(long, default_value = "600000")]
    pub timeout: u64,

    /// Skip this step
    #[arg(long)]
    pub skip: bool,
}

#[derive(Args, Clone)]
pub struct ReviewArgs {
    /// Worktree path (defaults to current directory)
    #[arg(long)]
    pub worktree: Option<String>,

    /// Timeout in milliseconds
    #[arg(long, default_value = "600000")]
    pub timeout: u64,

    /// Skip running tests during review
    #[arg(long)]
    pub skip_tests: bool,

    /// Skip this step
    #[arg(long)]
    pub skip: bool,
}

#[derive(Args, Clone)]
pub struct ApprovalArgs {
    /// Worktree path (defaults to current directory)
    #[arg(long)]
    pub worktree: Option<String>,

    /// Base branch to merge into
    #[arg(long, default_value = "master")]
    pub base: String,

    /// Wait for approval
    #[arg(long)]
    pub notify: bool,

    /// Interval between notifications in ms
    #[arg(long, default_value = "60000")]
    pub notify_interval: u64,

    /// Target repo for merge/push (defaults to main worktree)
    #[arg(long)]
    pub target_repo: Option<String>,
}

#[derive(Args, Clone)]
pub struct ApplyArgs {
    /// Worktree path (defaults to current directory)
    #[arg(long)]
    pub worktree: Option<String>,

    /// Base branch to merge into
    #[arg(long, default_value = "master")]
    pub base: String,

    /// Don't actually merge, just check
    #[arg(long)]
    pub dry_run: bool,

    /// Target repo for merge/push (defaults to main worktree)
    #[arg(long)]
    pub target_repo: Option<String>,
}

pub async fn run_command(cmd: MergeStepCommand) -> Result<()> {
    match cmd {
        MergeStepCommand::Preflight(args) => preflight(args).await,
        MergeStepCommand::PreMerge(args) => pre_merge(args).await,
        MergeStepCommand::Review(args) => review(args).await,
        MergeStepCommand::Conflicts(args) => conflicts(args).await,
        MergeStepCommand::Approval(args) => approval_step(args).await,
        MergeStepCommand::Apply(args) => apply(args).await,
    }
}

async fn preflight(args: MergeStepArgs) -> Result<()> {
    let worktree_path = resolve_worktree(args.worktree)?;
    git::merge_preflight(&worktree_path, &args.base).await
}

async fn pre_merge(args: PreMergeArgs) -> Result<()> {
    if args.skip {
        println!("Skipping pre-merge checks");
        return Ok(());
    }
    let worktree_path = resolve_worktree(args.worktree)?;
    git::merge_pre_merge(&worktree_path, args.timeout).await
}

async fn review(args: ReviewArgs) -> Result<()> {
    if args.skip {
        println!("Skipping review step");
        return Ok(());
    }
    let worktree_path = resolve_worktree(args.worktree)?;
    git::merge_review(&worktree_path, args.skip_tests, args.timeout).await
}

async fn conflicts(args: MergeStepArgs) -> Result<()> {
    let worktree_path = resolve_worktree(args.worktree)?;
    let conflicts = git::merge_conflicts(&worktree_path, &args.base).await?;
    if conflicts.is_empty() {
        return Ok(());
    }

    let list = conflicts.join(", ");
    Err(anyhow!("merge conflicts detected: {list}"))
}

async fn approval_step(args: ApprovalArgs) -> Result<()> {
    if !args.notify {
        return Ok(());
    }

    let worktree_path = resolve_worktree(args.worktree)?;
    let git_root = git::get_git_root(&worktree_path).await?;
    let branch = git::get_current_branch(&git_root).await?;

    let target_repo = match args.target_repo {
        Some(path) => std::fs::canonicalize(&path)
            .with_context(|| format!("failed to read target repo: {path}"))?,
        None => git::get_main_worktree(&git_root).await?,
    };

    let pending =
        approval::create_pending(&branch, &args.base, &git_root, &target_repo).await?;

    println!("Merge ready for approval: crank approve {}", branch);

    let approved = approval::wait_for_approval(&pending, args.notify_interval).await;
    approval::remove_pending(&pending.id).await?;

    if !approved {
        return Err(anyhow!("merge rejected"));
    }

    Ok(())
}

async fn apply(args: ApplyArgs) -> Result<()> {
    let worktree_path = resolve_worktree(args.worktree)?;

    if args.dry_run {
        println!("Dry run: merge skipped");
        return Ok(());
    }

    let target_repo = match args.target_repo {
        Some(path) => Some(
            std::fs::canonicalize(&path)
                .with_context(|| format!("failed to read target repo: {path}"))?,
        ),
        None => None,
    };

    let commit = git::merge_apply(&worktree_path, &args.base, target_repo.as_deref()).await?;

    if let Ok(task_id) = markers::read_current_task_id(&worktree_path) {
        if let Err(err) = markers::write_merged_marker(&task_id) {
            eprintln!("failed to write merged marker: {err}");
        }
    }

    println!("Merged {commit}");
    Ok(())
}

fn resolve_worktree(worktree: Option<String>) -> Result<PathBuf> {
    let worktree = worktree.unwrap_or_else(|| ".".to_string());
    std::fs::canonicalize(&worktree)
        .with_context(|| format!("failed to resolve worktree path: {worktree}"))
}
