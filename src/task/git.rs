use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};

pub fn git_root() -> Result<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("failed to run git rev-parse")?;
    if !output.status.success() {
        return Err(anyhow!("not in a git repository"));
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(PathBuf::from(root))
}

pub fn repo_root() -> Result<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .context("failed to run git rev-parse for common dir")?;
    if !output.status.success() {
        return Err(anyhow!("not in a git repository"));
    }
    let mut root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.ends_with(".git") {
        root = root.trim_end_matches(".git").to_string();
        root = root.trim_end_matches('/').to_string();
    }
    Ok(PathBuf::from(root))
}

pub fn task_path_for_id(git_root: &Path, task_id: &str) -> PathBuf {
    crate::crank_io::repo_crank_dir(git_root).join(format!("{task_id}.md"))
}

pub fn git_common_dir_from(path: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .context("failed to run git rev-parse for common dir")?;
    if !output.status.success() {
        return Err(anyhow!("not in a git repository"));
    }
    let dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(PathBuf::from(dir))
}
