use std::path::PathBuf;

use anyhow::{anyhow, Result};
use clap::{Args, Subcommand};

use crate::task::git;
use crate::tutorial::{self, TutorialGenerateOptions};

#[derive(Subcommand)]
pub enum TutorialCommand {
    /// Generate a tutorial for a merge commit
    Generate(GenerateArgs),
    /// Show a tutorial in markdown or json
    Show(ShowArgs),
}

#[derive(Args, Clone)]
pub struct GenerateArgs {
    /// Worktree path for the merged branch
    #[arg(long, default_value = ".")]
    pub worktree: String,

    /// Base branch name
    #[arg(long, default_value = "master")]
    pub base: String,

    /// Merge commit SHA (defaults to base branch head)
    #[arg(long)]
    pub merge_commit: Option<String>,

    /// Workflow id for metadata
    #[arg(long)]
    pub workflow_id: Option<String>,

    /// Override tutorial output directory
    #[arg(long)]
    pub output_dir: Option<String>,

    /// Replace existing tutorial if it exists
    #[arg(long)]
    pub replace: bool,
}

#[derive(Args, Clone)]
pub struct ShowArgs {
    /// Tutorial id
    pub id: String,

    /// Output format: md or json
    #[arg(long, default_value = "md")]
    pub format: String,

    /// Show a single step (1-based)
    #[arg(long)]
    pub step: Option<usize>,
}

pub fn run_command(cmd: TutorialCommand) -> Result<()> {
    match cmd {
        TutorialCommand::Generate(args) => {
            let options = TutorialGenerateOptions {
                worktree: PathBuf::from(args.worktree),
                base_branch: args.base,
                merge_commit: args.merge_commit,
                workflow_id: args.workflow_id,
                output_dir: args.output_dir.map(PathBuf::from),
                replace: args.replace,
            };
            tutorial::generate_tutorial(&options)?;
        }
        TutorialCommand::Show(args) => {
            let repo_root = git::repo_root()?;
            let format = args.format.to_lowercase();
            if format != "md" && format != "json" {
                return Err(anyhow!("invalid format: {}", args.format));
            }
            tutorial::show_tutorial(&repo_root, &args.id, &format, args.step)?;
        }
    }
    Ok(())
}
