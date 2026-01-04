use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::approval::{create_pending, remove_pending, send_notification, wait_for_approval};
use crate::autopilot::markers;
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

#[allow(dead_code)]
fn generate_merge_id() -> String {
    format!("{:08x}", rand::random::<u32>())
}

impl MergeProgress {
    #[allow(dead_code)]
    fn new(branch: &str, base: &str, worktree: &str) -> Self {
        Self {
            id: generate_merge_id(),
            pid: std::process::id(),
            branch: branch.to_string(),
            base: base.to_string(),
            worktree: worktree.to_string(),
            started_at: now_ms(),
            finished_at: None,
            status: "running".to_string(),
            steps: vec![
                StepProgress {
                    name: "preflight".to_string(),
                    status: "pending".to_string(),
                    started_at: None,
                    finished_at: None,
                    output_lines: 0,
                    last_output: None,
                    session_id: None,
                },
                StepProgress {
                    name: "pre-merge".to_string(),
                    status: "pending".to_string(),
                    started_at: None,
                    finished_at: None,
                    output_lines: 0,
                    last_output: None,
                    session_id: None,
                },
                StepProgress {
                    name: "review".to_string(),
                    status: "pending".to_string(),
                    started_at: None,
                    finished_at: None,
                    output_lines: 0,
                    last_output: None,
                    session_id: None,
                },
            ],
        }
    }

    fn write(&self) {
        let dir = get_progress_dir();
        let _ = std::fs::create_dir_all(&dir);
        let path = get_progress_path(&self.id);
        let _ = std::fs::write(&path, serde_json::to_string_pretty(self).unwrap());
    }

    #[allow(dead_code)]
    fn cleanup(&self) {
        let _ = std::fs::remove_file(get_progress_path(&self.id));
        let _ = std::fs::remove_file(get_output_path(&self.id));
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

    #[allow(dead_code)]
    fn finish(&mut self, status: &str) {
        self.status = status.to_string();
        self.finished_at = Some(now_ms());
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

#[derive(Debug, Serialize)]
#[serde(untagged)]
#[allow(dead_code)]
enum MergeOutput {
    Step(StepResult),
    Conflict {
        status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        files: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Success {
        status: String,
        merged: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        pushed: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        base: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        dry_run: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
    },
    AwaitingApproval {
        status: String,
        branch: String,
        base: String,
        id: String,
        approve_cmd: String,
        reject_cmd: String,
    },
    Rejected {
        status: String,
        branch: String,
        message: String,
    },
}

#[allow(dead_code)]
fn output(data: &impl Serialize) {
    println!("{}", serde_json::to_string(data).unwrap());
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

/// Get remote URL
#[allow(dead_code)]
async fn get_remote_url(cwd: &Path) -> Result<String> {
    git(cwd, &["remote", "get-url", "origin"]).await
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

/// Run just pre-merge with streaming output
#[allow(dead_code)]
async fn run_pre_merge(
    cwd: &Path,
    timeout_ms: u64,
    progress: Arc<Mutex<MergeProgress>>,
) -> StepResult {
    let start = std::time::Instant::now();

    {
        let mut p = progress.lock().await;
        p.start_step("pre-merge");
    }

    let mut child = match Command::new("just")
        .arg("pre-merge")
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            let mut p = progress.lock().await;
            p.finish_step("pre-merge", "fail", Some(e.to_string()));
            return StepResult {
                step: "pre-merge".to_string(),
                status: "fail".to_string(),
                exit: Some(1),
                tail: Some(e.to_string()),
                duration_ms: Some(start.elapsed().as_millis() as u64),
            };
        }
    };

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    let mut stdout_reader = BufReader::new(stdout).lines();
    let mut stderr_reader = BufReader::new(stderr).lines();

    let mut tail_lines: Vec<String> = Vec::new();
    let progress_clone = progress.clone();

    let read_output = async {
        loop {
            tokio::select! {
                line = stdout_reader.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            tail_lines.push(line.clone());
                            if tail_lines.len() > 40 {
                                tail_lines.remove(0);
                            }
                            let mut p = progress_clone.lock().await;
                            p.append_output("pre-merge", &line);
                        }
                        Ok(None) => {}
                        Err(_) => {}
                    }
                }
                line = stderr_reader.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            tail_lines.push(line.clone());
                            if tail_lines.len() > 40 {
                                tail_lines.remove(0);
                            }
                            let mut p = progress_clone.lock().await;
                            p.append_output("pre-merge", &line);
                        }
                        Ok(None) => {}
                        Err(_) => {}
                    }
                }
                status = child.wait() => {
                    return status;
                }
            }
        }
    };

    let result = tokio::time::timeout(Duration::from_millis(timeout_ms), read_output).await;

    let duration_ms = start.elapsed().as_millis() as u64;
    let tail = tail_lines.join("\n");

    let step_result = match result {
        Ok(Ok(status)) => {
            let status_str = if status.success() { "pass" } else { "fail" };
            {
                let mut p = progress.lock().await;
                p.finish_step("pre-merge", status_str, tail_lines.last().cloned());
            }
            StepResult {
                step: "pre-merge".to_string(),
                status: status_str.to_string(),
                exit: status.code(),
                tail: Some(tail),
                duration_ms: Some(duration_ms),
            }
        }
        Ok(Err(e)) => {
            {
                let mut p = progress.lock().await;
                p.finish_step("pre-merge", "fail", Some(e.to_string()));
            }
            StepResult {
                step: "pre-merge".to_string(),
                status: "fail".to_string(),
                exit: Some(1),
                tail: Some(e.to_string()),
                duration_ms: Some(duration_ms),
            }
        }
        Err(_) => {
            // Timeout - kill the process
            let _ = child.kill().await;
            {
                let mut p = progress.lock().await;
                p.finish_step("pre-merge", "fail", Some("Timeout".to_string()));
            }
            StepResult {
                step: "pre-merge".to_string(),
                status: "fail".to_string(),
                exit: Some(1),
                tail: Some("Timeout".to_string()),
                duration_ms: Some(duration_ms),
            }
        }
    };

    step_result
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

/// Main merge command
#[allow(dead_code)]
pub async fn merge_command(opts: MergeOptions) -> Result<()> {
    let start = std::time::Instant::now();
    let worktree_path = std::fs::canonicalize(&opts.worktree)?;
    let git_root = get_git_root(&worktree_path).await?;
    let branch = get_current_branch(&git_root).await?;
    let target_repo = opts
        .target_repo
        .as_ref()
        .map(std::fs::canonicalize)
        .transpose()?;

    // Initialize progress tracking
    let progress = Arc::new(Mutex::new(MergeProgress::new(
        &branch,
        &opts.base,
        &worktree_path.to_string_lossy(),
    )));

    // Write initial progress
    {
        let p = progress.lock().await;
        p.write();
        eprintln!("Merge progress: ~/.crank/merges/{}.json", p.id);
        eprintln!("Merge output:   ~/.crank/merges/{}.log", p.id);
    }

    {
        let mut p = progress.lock().await;
        p.start_step("preflight");
    }
    if let Err(err) = ensure_merge_ready(&worktree_path, &opts.base).await {
        let message = err.to_string();
        {
            let mut p = progress.lock().await;
            p.finish_step("preflight", "fail", Some(message.clone()));
            p.finish("fail");
        }
        output(&MergeOutput::Step(StepResult::new(
            "preflight",
            "fail",
            Some(message),
            None,
        )));
        std::process::exit(1);
    } else {
        let mut p = progress.lock().await;
        p.finish_step("preflight", "pass", None);
    }

    // Run pre-merge and review concurrently
    let mut has_failure = false;
    let mut results = vec![];

    let progress_clone = progress.clone();
    let (pre_merge_result, review_result) = tokio::join!(
        async {
            if opts.skip_pre_merge {
                None
            } else {
                Some(run_pre_merge(&git_root, opts.timeout, progress_clone.clone()).await)
            }
        },
        async {
            if opts.skip_review {
                None
            } else {
                Some(
                    opencode::run_review(
                        &git_root,
                        &branch,
                        !opts.skip_pre_merge, // skip_tests if pre-merge already ran them
                        opts.timeout,
                        Some(progress_clone.clone()),
                    )
                    .await,
                )
            }
        }
    );

    if let Some(result) = pre_merge_result {
        if result.status == "fail" {
            has_failure = true;
        }
        results.push(result);
    }

    if let Some(result) = review_result {
        if result.status == "fail" {
            has_failure = true;
        }
        results.push(result);
    }

    // Output failures
    for result in &results {
        if result.status == "fail" {
            output(&MergeOutput::Step(result.clone()));
        }
    }

    if has_failure {
        {
            let mut p = progress.lock().await;
            p.finish("fail");
        }
        std::process::exit(1);
    }

    // Check conflicts
    let base_ref = format!("origin/{}", opts.base);
    git(&git_root, &["fetch", "origin", &opts.base]).await?;
    let conflicts = check_conflicts(&git_root, &base_ref).await?;
    if !conflicts.is_empty() {
        output(&MergeOutput::Conflict {
            status: "conflict".to_string(),
            files: Some(conflicts),
            message: None,
            error: None,
        });
        std::process::exit(1);
    }

    // Dry run
    if opts.dry_run {
        {
            let mut p = progress.lock().await;
            p.finish("pass");
        }
        output(&MergeOutput::Success {
            status: "pass".to_string(),
            merged: false,
            pushed: None,
            commit: None,
            branch: Some(branch),
            base: Some(opts.base.clone()),
            dry_run: Some(true),
            duration_ms: Some(start.elapsed().as_millis() as u64),
        });
        return Ok(());
    }

    // Wait for approval if --notify
    if opts.notify {
        let main_worktree = match target_repo.as_deref() {
            Some(path) => path.to_path_buf(),
            None => get_main_worktree(&git_root).await?,
        };
        let pending = create_pending(&branch, &opts.base, &git_root, &main_worktree).await?;

        output(&MergeOutput::AwaitingApproval {
            status: "awaiting_approval".to_string(),
            branch: branch.clone(),
            base: opts.base.clone(),
            id: pending.id.clone(),
            approve_cmd: format!("crank approve {}", branch),
            reject_cmd: format!("crank reject {}", branch),
        });

        let approved = wait_for_approval(&pending, opts.notify_interval).await;
        remove_pending(&pending.id).await?;

        if !approved {
            output(&MergeOutput::Rejected {
                status: "rejected".to_string(),
                branch,
                message: "Merge rejected by user".to_string(),
            });
            std::process::exit(1);
        }
    }

    // Merge and push
    match merge_and_push(&git_root, &opts.base, target_repo.as_deref()).await {
        Ok(commit) => {
            {
                let mut p = progress.lock().await;
                p.finish("pass");
            }

            if let Ok(task_id) = markers::read_current_task_id(&worktree_path) {
                if let Err(err) = markers::write_merged_marker(&task_id) {
                    eprintln!("failed to write merged marker: {err}");
                }
            }

            output(&MergeOutput::Success {
                status: "pass".to_string(),
                merged: true,
                pushed: Some(true),
                commit: Some(commit.clone()),
                branch: Some(branch.clone()),
                base: Some(opts.base.clone()),
                dry_run: None,
                duration_ms: Some(start.elapsed().as_millis() as u64),
            });

            if opts.notify {
                let _ = send_notification(
                    "Merge complete",
                    &format!("{} merged to {} ({})", branch, opts.base, commit),
                )
                .await;
            }
        }
        Err(e) => {
            {
                let mut p = progress.lock().await;
                p.finish("fail");
            }
            let msg = e.to_string();
            if msg.contains("Push failed") || msg.contains("conflict") {
                output(&MergeOutput::Conflict {
                    status: "conflict".to_string(),
                    files: None,
                    message: Some(
                        "Merge conflict (race condition - base branch changed)".to_string(),
                    ),
                    error: Some(msg),
                });
            } else {
                output(&MergeOutput::Step(StepResult {
                    step: "merge".to_string(),
                    status: "fail".to_string(),
                    exit: Some(1),
                    tail: Some(msg),
                    duration_ms: None,
                }));
            }
            std::process::exit(1);
        }
    }

    Ok(())
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

impl Clone for StepResult {
    fn clone(&self) -> Self {
        Self {
            step: self.step.clone(),
            status: self.status.clone(),
            exit: self.exit,
            tail: self.tail.clone(),
            duration_ms: self.duration_ms,
        }
    }
}

pub use StepResult as ReviewStepResult;
