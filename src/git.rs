use anyhow::{Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;

#[derive(Debug, Serialize)]
pub struct StepResult {
    pub step: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// Execute a git command and return stdout
async fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("Failed to execute git")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git {} failed: {}", args.join(" "), stderr);
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Get current branch name
pub async fn get_current_branch(cwd: &Path) -> Result<String> {
    git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).await
}

/// Get git root directory
pub async fn get_git_root(cwd: &Path) -> Result<PathBuf> {
    let root = git(cwd, &["rev-parse", "--show-toplevel"]).await?;
    Ok(PathBuf::from(root))
}

// Re-export StepResult for opencode module
impl StepResult {
    pub fn new(
        step: &str,
        status: &str,
        tail: Option<String>,
        details: Option<String>,
        duration_ms: Option<u64>,
    ) -> Self {
        Self {
            step: step.to_string(),
            status: status.to_string(),
            exit: if status == "fail" { Some(1) } else { None },
            tail,
            details,
            duration_ms,
        }
    }
}

pub use StepResult as ReviewStepResult;
