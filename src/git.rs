use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

use crate::opencode;

/// Progress tracking for merge operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeProgress {
    pub id: String,
    pub pid: u32,
    pub branch: String,
    pub base: String,
    pub worktree: String,
    pub started_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<u64>,
    pub status: String, // "running", "pass", "fail"
    pub steps: Vec<StepProgress>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepProgress {
    pub name: String,
    pub status: String, // "pending", "running", "pass", "fail"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<u64>,
    pub output_lines: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

fn get_progress_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".crank")
        .join("merges")
}

fn get_progress_path(id: &str) -> PathBuf {
    get_progress_dir().join(format!("{}.json", id))
}

fn get_output_path(id: &str) -> PathBuf {
    get_progress_dir().join(format!("{}.log", id))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

pub fn now_ms_pub() -> u64 {
    now_ms()
}

impl MergeProgress {
    fn write(&self) {
        let dir = get_progress_dir();
        let _ = std::fs::create_dir_all(&dir);
        let path = get_progress_path(&self.id);
        let _ = std::fs::write(&path, serde_json::to_string_pretty(self).unwrap());
    }

    fn step_mut(&mut self, name: &str) -> Option<&mut StepProgress> {
        self.steps.iter_mut().find(|s| s.name == name)
    }

    pub fn start_step(&mut self, name: &str) {
        if let Some(step) = self.step_mut(name) {
            step.status = "running".to_string();
            step.started_at = Some(now_ms());
        }
        self.write();
    }

    pub fn set_session_id(&mut self, name: &str, session_id: &str) {
        if let Some(step) = self.step_mut(name) {
            step.session_id = Some(session_id.to_string());
        }
        self.write();
    }

    pub fn finish_step(&mut self, name: &str, status: &str, last_output: Option<String>) {
        if let Some(step) = self.step_mut(name) {
            step.status = status.to_string();
            step.finished_at = Some(now_ms());
            step.last_output = last_output;
        }
        self.write();
    }

    pub fn append_output(&mut self, name: &str, line: &str) {
        use std::io::Write;
        if let Some(step) = self.step_mut(name) {
            step.output_lines += 1;
            step.last_output = Some(line.chars().take(200).collect());
        }
        let path = get_output_path(&self.id);
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = writeln!(file, "[{}] {}", name, line);
        }
        self.write();
    }
}

/// List active merges (still running)
pub fn list_active_merges() -> Vec<MergeProgress> {
    let dir = get_progress_dir();
    let mut merges = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(progress) = serde_json::from_str::<MergeProgress>(&content) {
                        // Check if process is still running
                        if progress.status == "running" {
                            // Check if PID exists
                            let pid_exists = std::process::Command::new("kill")
                                .args(["-0", &progress.pid.to_string()])
                                .status()
                                .map(|s| s.success())
                                .unwrap_or(false);

                            if pid_exists {
                                merges.push(progress);
                            } else {
                                // Mark as failed if process died
                                let mut updated = progress.clone();
                                updated.status = "fail".to_string();
                                updated.finished_at = Some(now_ms());
                                updated.write();
                            }
                        }
                    }
                }
            }
        }
    }

    merges
}

/// List all merges (active and completed), sorted by most recent first
pub fn list_all_merges() -> Vec<MergeProgress> {
    let dir = get_progress_dir();
    let mut merges = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(progress) = serde_json::from_str::<MergeProgress>(&content) {
                        merges.push(progress);
                    }
                }
            }
        }
    }

    // Sort by started_at descending (most recent first)
    merges.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    merges
}

/// Options for the merge command
pub struct MergeOptions {
    pub worktree: String,
    pub dry_run: bool,
    pub base: String,
    pub target_repo: Option<String>,
    pub skip_pre_merge: bool,
    pub skip_review: bool,
    pub timeout: u64,
    pub notify: bool,
    pub notify_interval: u64,
}

pub async fn merge_preflight(worktree_path: &Path, base_branch: &str) -> Result<()> {
    ensure_merge_ready(worktree_path, base_branch).await
}

pub async fn merge_pre_merge(worktree_path: &Path, timeout_ms: u64) -> Result<()> {
    let git_root = get_git_root(worktree_path).await?;
    let mut child = Command::new("just")
        .arg("pre-merge")
        .current_dir(&git_root)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .context("failed to run just pre-merge")?;

    let status = tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait()).await;

    match status {
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(status)) => Err(anyhow::anyhow!(
            "pre-merge failed with status {}",
            status.code().unwrap_or(1)
        )),
        Ok(Err(err)) => Err(anyhow::anyhow!("pre-merge failed: {err}")),
        Err(_) => {
            let _ = child.kill().await;
            Err(anyhow::anyhow!("pre-merge timed out"))
        }
    }
}

pub async fn merge_review(worktree_path: &Path, skip_tests: bool, timeout_ms: u64) -> Result<()> {
    let git_root = get_git_root(worktree_path).await?;
    let branch = get_current_branch(&git_root).await?;
    let result = opencode::run_review(&git_root, &branch, skip_tests, timeout_ms, None).await;

    if result.status == "pass" {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "review failed: {}",
            result.tail.unwrap_or_else(|| "unknown".to_string())
        ))
    }
}

pub async fn merge_conflicts(worktree_path: &Path, base_branch: &str) -> Result<Vec<String>> {
    let git_root = get_git_root(worktree_path).await?;
    git(&git_root, &["fetch", "origin", base_branch]).await?;
    let base_ref = format!("origin/{base_branch}");
    check_conflicts(&git_root, &base_ref).await
}

pub async fn merge_apply(
    worktree_path: &Path,
    base_branch: &str,
    target_repo: Option<&Path>,
) -> Result<String> {
    let git_root = get_git_root(worktree_path).await?;
    merge_and_push(&git_root, base_branch, target_repo).await
}

#[derive(Debug, Serialize)]
pub struct StepResult {
    pub step: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tail: Option<String>,
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

/// Execute a git command, returning result instead of error
async fn git_result(cwd: &Path, args: &[&str]) -> (String, String, i32) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .unwrap_or_else(|e| {
            panic!("Failed to execute git: {}", e);
        });

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code().unwrap_or(1);

    (stdout, stderr, code)
}

async fn ensure_merge_ready(worktree_path: &Path, base_branch: &str) -> Result<()> {
    let base_ref = format!("origin/{base_branch}");
    git(worktree_path, &["fetch", "origin", base_branch]).await?;

    let dirty = git(worktree_path, &["status", "--porcelain"]).await?;
    if !dirty.trim().is_empty() {
        anyhow::bail!("worktree has uncommitted changes:\n{dirty}");
    }

    let range = format!("{base_ref}..HEAD");
    let ahead = git(worktree_path, &["rev-list", "--count", &range]).await?;
    if ahead.trim() == "0" {
        anyhow::bail!("no commits to merge; commit changes before running crank merge");
    }

    Ok(())
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

/// Get short commit hash
pub async fn get_head_commit(cwd: &Path) -> Result<String> {
    git(cwd, &["rev-parse", "--short", "HEAD"]).await
}

/// Check for merge conflicts using git merge-tree
async fn check_conflicts(cwd: &Path, base_ref: &str) -> Result<Vec<String>> {
    let (stdout, stderr, code) =
        git_result(cwd, &["merge-tree", "--write-tree", base_ref, "HEAD"]).await;

    if code == 0 {
        return Ok(vec![]);
    }

    // Parse conflict info
    let mut conflicts = vec![];
    for line in stdout.lines().chain(stderr.lines()) {
        if let Some(caps) = line.to_lowercase().find("conflict") {
            if let Some(file) = line.split("Merge conflict in ").nth(1) {
                conflicts.push(file.trim().to_string());
            } else if caps > 0 {
                conflicts.push(line.to_string());
            }
        }
    }

    if conflicts.is_empty() && code != 0 {
        conflicts.push("(conflict detection failed - manual check needed)".to_string());
    }

    Ok(conflicts)
}

/// Get the main worktree path (the original checkout, not a linked worktree)
pub async fn get_main_worktree(cwd: &Path) -> Result<PathBuf> {
    let output = git(cwd, &["worktree", "list", "--porcelain"]).await?;

    // First worktree in the list is the main one
    // Format: "worktree /path/to/main\n..."
    for line in output.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            return Ok(PathBuf::from(path));
        }
    }

    // Fallback: if no worktrees, we're already in main
    get_git_root(cwd).await
}

/// Acquire merge lock (using directory-based lock)
async fn acquire_lock() -> Result<PathBuf> {
    let crank_dir = dirs::home_dir()
        .context("Could not find home directory")?
        .join(".crank");

    std::fs::create_dir_all(&crank_dir)?;

    let lock_dir = crank_dir.join("merge.lock.d");
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(300);

    loop {
        match std::fs::create_dir(&lock_dir) {
            Ok(()) => return Ok(lock_dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if start.elapsed() > timeout {
                    anyhow::bail!("Timeout waiting for merge lock");
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// Release merge lock
fn release_lock(lock_dir: &Path) {
    let _ = std::fs::remove_dir(lock_dir);
}

/// Merge and push (under lock)
/// Uses the main worktree to perform the merge to base branch
async fn merge_and_push(
    source_worktree: &Path,
    base_branch: &str,
    target_repo: Option<&Path>,
) -> Result<String> {
    let lock_dir = acquire_lock().await?;

    let result = async {
        let source_branch = get_current_branch(source_worktree).await?;
        let source_commit = get_head_commit(source_worktree).await?;

        // Get the main worktree (where we'll do the merge)
        let main_worktree = match target_repo {
            Some(path) => path.to_path_buf(),
            None => get_main_worktree(source_worktree).await?,
        };

        // Fetch latest and checkout base branch in main worktree
        git(&main_worktree, &["fetch", "origin", base_branch]).await?;
        git(&main_worktree, &["checkout", base_branch]).await?;
        git(
            &main_worktree,
            &["reset", "--hard", &format!("origin/{}", base_branch)],
        )
        .await?;

        // Merge the source branch (it's already available since it's in the same repo)
        let merge_msg = format!("Merge {} ({})", source_branch, source_commit);
        let merge_target = if target_repo.is_some() {
            let source_path = source_worktree.to_string_lossy().to_string();
            let fetch_args = ["fetch", source_path.as_str(), source_branch.as_str()];
            git(&main_worktree, &fetch_args).await?;
            "FETCH_HEAD".to_string()
        } else {
            source_branch.clone()
        };

        let merge_args = vec![
            "merge",
            "--no-ff",
            "-m",
            merge_msg.as_str(),
            merge_target.as_str(),
        ];
        let (_, stderr, code) = git_result(&main_worktree, &merge_args).await;

        if code != 0 {
            let _ = git(&main_worktree, &["merge", "--abort"]).await;
            anyhow::bail!("Merge conflict: {}", stderr);
        }

        // Push
        let (_, stderr, code) = git_result(&main_worktree, &["push", "origin", base_branch]).await;

        if code != 0 {
            let _ = git(
                &main_worktree,
                &["reset", "--hard", &format!("origin/{}", base_branch)],
            )
            .await;
            anyhow::bail!("Push failed (likely race condition): {}", stderr);
        }

        get_head_commit(&main_worktree).await
    }
    .await;

    release_lock(&lock_dir);
    result
}

// Re-export StepResult for opencode module
impl StepResult {
    pub fn new(step: &str, status: &str, tail: Option<String>, duration_ms: Option<u64>) -> Self {
        Self {
            step: step.to_string(),
            status: status.to_string(),
            exit: if status == "fail" { Some(1) } else { None },
            tail,
            duration_ms,
        }
    }
}

pub use StepResult as ReviewStepResult;
