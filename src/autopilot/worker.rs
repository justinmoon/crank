use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant, SystemTime};

use crate::orchestrator::{controls, logging::Logger, markers, opencode};
use crate::task::branch;
use crate::task::git as task_git;
use crate::task::model::{Task, TASK_STATUS_CLOSED, TASK_STATUS_NEEDS_HUMAN};
use crate::task::store;
use crate::task::{claim_next_task, clear_active_claim};
use anyhow::{anyhow, Context, Result};

const MAX_BRANCH_LEN: usize = 20;

const CLAIM_BACKOFF_START: Duration = Duration::from_secs(5);
const CLAIM_BACKOFF_MAX: Duration = Duration::from_secs(60);
const SUPERVISE_INTERVAL: Duration = Duration::from_secs(15);
const OPENCODE_STATUS_INTERVAL: Duration = Duration::from_secs(30);
const OPENCODE_NUDGE_THROTTLE: Duration = Duration::from_secs(60);
const CODEX_IDLE_NUDGE_AFTER: Duration = Duration::from_secs(300);

pub async fn run_worker(id: u16, project: Option<String>) -> Result<()> {
    if id == 0 {
        return Err(anyhow!("worker id must be at least 1"));
    }
    if std::env::var("TMUX").unwrap_or_default().is_empty() {
        return Err(anyhow!("crank worker must run inside tmux"));
    }
    let tmux_pane = std::env::var("TMUX_PANE").context("TMUX_PANE is not set")?;

    let repo_root = task_git::repo_root()?;
    let tasks_root = repo_root.clone();
    let project = project.as_deref().map(str::trim).filter(|p| !p.is_empty());
    let logger = Logger::new(&format!("worker-{id}"))?;
    log_and_print(&logger, "info", "worker started")?;
    log_and_print(
        &logger,
        "info",
        &format!("log path: {}", logger.path().display()),
    )?;
    log_and_print(
        &logger,
        "info",
        &format!("tasks root: {}", tasks_root.display()),
    )?;

    let mut backoff = CLAIM_BACKOFF_START;

    loop {
        logger.log("debug", "claiming next task")?;
        let task = claim_next_task(&tasks_root, &repo_root, project)?;
        let Some(task) = task else {
            logger.log(
                "debug",
                &format!("no claimable tasks; sleeping {}s", backoff.as_secs()),
            )?;
            tokio::time::sleep(backoff).await;
            backoff = std::cmp::min(
                backoff.checked_mul(2).unwrap_or(CLAIM_BACKOFF_MAX),
                CLAIM_BACKOFF_MAX,
            );
            continue;
        };
        backoff = CLAIM_BACKOFF_START;

        log_and_print(
            &logger,
            "info",
            &format!("claimed task {} ({})", task.id, task.title),
        )?;
        markers::clear_task_markers(&task.id)?;

        let (branch, worktree_path) = create_worktree(&repo_root, &task)?;
        log_and_print(
            &logger,
            "info",
            &format!("created worktree {} at {}", branch, worktree_path.display()),
        )?;
        rename_tmux_window(&tmux_pane, &branch)?;

        store::write_current_task_marker(&worktree_path, &task.id)
            .context("failed to write current task marker")?;
        write_task_alias(&task, &worktree_path)?;
        run_direnv_allow(&worktree_path)?;

        let prompt = build_prompt(&task, &worktree_path)?;
        let agent = agent_kind(&task)?;
        log_and_print(&logger, "info", &format!("agent: {:?}", agent))?;

        supervise_task(
            &task,
            &worktree_path,
            &prompt,
            agent,
            id,
            &tmux_pane,
            &logger,
            &repo_root,
        )
        .await?;
    }
}

async fn supervise_task(
    task: &Task,
    worktree_path: &Path,
    prompt: &str,
    agent: AgentKind,
    worker_id: u16,
    tmux_pane: &str,
    logger: &Logger,
    repo_root: &Path,
) -> Result<()> {
    log_and_print(
        logger,
        "info",
        &format!(
            "supervising task {} in {}",
            task.id,
            worktree_path.display()
        ),
    )?;
    let mut agent_session = AgentSession::start(
        task,
        worktree_path,
        prompt,
        agent,
        worker_id,
        tmux_pane,
        logger,
    )
    .await?;

    let mut last_status_check = Instant::now();
    let mut last_opencode_nudge = Instant::now();
    let mut start_time = SystemTime::now();

    loop {
        if markers::merged_marker_exists(&task.id)? {
            log_and_print(logger, "info", "merged marker found; closing task")?;
            clear_active_claim(repo_root, &task.id)?;
            close_task(task)?;
            agent_session.terminate();
            return Ok(());
        }

        let help_requested = markers::help_marker_exists(&task.id)?;
        let pause_requested = markers::pause_marker_exists(&task.id)?;

        if help_requested {
            log_and_print(
                logger,
                "info",
                "help requested; marking needs_human and releasing task",
            )?;
            mark_task_needs_human(task)?;
            clear_active_claim(repo_root, &task.id)?;
            agent_session.terminate();
            return Ok(());
        }

        if agent_session.child_exited()? {
            log_and_print(logger, "info", "agent exited; restarting")?;
            agent_session.restart_child(task, worktree_path, prompt, tmux_pane)?;
        }

        if !help_requested && !pause_requested {
            match agent {
                AgentKind::Opencode => {
                    if let Some((server, id)) = agent_session.opencode_info() {
                        if last_status_check.elapsed() >= OPENCODE_STATUS_INTERVAL {
                            last_status_check = Instant::now();
                            if opencode::is_idle(server, id).await?
                                && last_opencode_nudge.elapsed() >= OPENCODE_NUDGE_THROTTLE
                            {
                                last_opencode_nudge = Instant::now();
                                log_and_print(logger, "info", "opencode idle; nudging")?;
                                opencode::send_prompt(server, id, prompt).await?;
                            }
                        }
                    }
                }
                AgentKind::Codex => {
                    let last_activity =
                        markers::read_activity_time(&task.id)?.unwrap_or(start_time);
                    if SystemTime::now()
                        .duration_since(last_activity)
                        .unwrap_or_default()
                        >= CODEX_IDLE_NUDGE_AFTER
                    {
                        log_and_print(logger, "info", "codex idle; nudging")?;
                        controls::nudge_task(&task.id, tmux_pane)?;
                        start_time = SystemTime::now();
                    }
                }
                AgentKind::Claude => {}
            }
        }

        tokio::time::sleep(SUPERVISE_INTERVAL).await;
    }
}

fn close_task(task: &Task) -> Result<()> {
    store::update_task_status(&task.path, TASK_STATUS_CLOSED)
        .context("failed to mark task closed")?;
    Ok(())
}

fn mark_task_needs_human(task: &Task) -> Result<()> {
    store::update_task_status(&task.path, TASK_STATUS_NEEDS_HUMAN)
        .context("failed to mark task needs_human")?;
    Ok(())
}

fn log_and_print(logger: &Logger, level: &str, message: &str) -> Result<()> {
    logger.log(level, message)?;
    println!("{message}");
    Ok(())
}

struct AgentSession {
    kind: AgentKind,
    child: Option<Child>,
    opencode: Option<opencode::OpencodeServer>,
    session_id: Option<String>,
}

impl AgentSession {
    async fn start(
        task: &Task,
        worktree_path: &Path,
        prompt: &str,
        kind: AgentKind,
        worker_id: u16,
        tmux_pane: &str,
        logger: &Logger,
    ) -> Result<Self> {
        match kind {
            AgentKind::Opencode => {
                log_and_print(logger, "info", "starting opencode server")?;
                let server = opencode::OpencodeServer::start(worker_id, worktree_path).await?;
                log_and_print(logger, "info", &format!("opencode url: {}", server.url))?;
                let session_id = opencode::create_session(&server, worktree_path, task).await?;
                log_and_print(logger, "info", &format!("opencode session: {session_id}"))?;
                opencode::send_prompt(&server, &session_id, prompt).await?;
                let child = opencode::spawn_attach(&server, &session_id, worktree_path)?;
                Ok(Self {
                    kind,
                    child: Some(child),
                    opencode: Some(server),
                    session_id: Some(session_id),
                })
            }
            AgentKind::Codex => Ok(Self {
                kind,
                child: Some(spawn_codex(task, worktree_path, prompt, tmux_pane)?),
                opencode: None,
                session_id: None,
            }),
            AgentKind::Claude => Ok(Self {
                kind,
                child: Some(spawn_claude(task, worktree_path, prompt, tmux_pane)?),
                opencode: None,
                session_id: None,
            }),
        }
    }

    fn opencode_info(&self) -> Option<(&opencode::OpencodeServer, &str)> {
        Some((self.opencode.as_ref()?, self.session_id.as_deref()?))
    }

    fn child_exited(&mut self) -> Result<bool> {
        let Some(child) = self.child.as_mut() else {
            return Ok(false);
        };
        if let Some(_status) = child.try_wait()? {
            self.child = None;
            return Ok(true);
        }
        Ok(false)
    }

    fn restart_child(
        &mut self,
        task: &Task,
        worktree_path: &Path,
        prompt: &str,
        tmux_pane: &str,
    ) -> Result<()> {
        let child = match self.kind {
            AgentKind::Opencode => {
                let (server, session_id) = self
                    .opencode_info()
                    .ok_or_else(|| anyhow!("opencode session missing"))?;
                opencode::spawn_attach(server, session_id, worktree_path)?
            }
            AgentKind::Codex => spawn_codex(task, worktree_path, prompt, tmux_pane)?,
            AgentKind::Claude => spawn_claude(task, worktree_path, prompt, tmux_pane)?,
        };
        self.child = Some(child);
        Ok(())
    }

    fn terminate(&mut self) {
        terminate_child(self.child.take());
    }
}

fn spawn_codex(task: &Task, worktree_path: &Path, prompt: &str, tmux_pane: &str) -> Result<Child> {
    let notify_script = codex_notify_script(worktree_path)?;
    let mut cmd = Command::new("codex");
    cmd.arg("--cd")
        .arg(worktree_path)
        .arg(prompt)
        .env("CODEX_NOTIFY", &notify_script)
        .env("CODEX_NOTIFY_COMMAND", &notify_script)
        .env("CRANK_TASK_ID", &task.id)
        .env("CRANK_TMUX_PANE", tmux_pane)
        .current_dir(worktree_path);
    cmd.spawn().context("failed to launch codex")
}

fn spawn_claude(task: &Task, worktree_path: &Path, prompt: &str, tmux_pane: &str) -> Result<Child> {
    let plugin_dir = claude_plugin_dir(worktree_path)?;
    let mut cmd = Command::new("claude");
    cmd.arg("--plugin-dir")
        .arg(plugin_dir)
        .arg("--permission-mode")
        .arg("bypassPermissions")
        .arg(prompt)
        .env("CRANK_TASK_ID", &task.id)
        .env("CRANK_TMUX_PANE", tmux_pane)
        .current_dir(worktree_path);
    cmd.spawn().context("failed to launch claude")
}

fn terminate_child(child: Option<Child>) {
    if let Some(mut child) = child {
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn build_prompt(task: &Task, worktree_path: &Path) -> Result<String> {
    let mut prompt = format!(
        "Read AGENTS.md and any project CLAUDE.md. Task: TASK.md (copy of .crank/{}.md).\n\nRules:\n- Implement the task.\n- Run tests via just when relevant.\n- If blocked, run crank ask-for-help \"<msg>\".\n- When complete, run crank done to mark the task finished.\n- Do not stop until crank done or crank ask-for-help is called.\n- Commands already run in the task worktree; do not use cd, -C, or absolute paths.",
        task.id
    );

    let extra_path = worktree_path.join(".crank").join("worker-prompt.txt");
    if extra_path.exists() {
        if let Ok(extra) = std::fs::read_to_string(&extra_path) {
            let trimmed = extra.trim();
            if !trimmed.is_empty() {
                prompt.push_str("\n\nRepo instructions:\n");
                prompt.push_str(trimmed);
            }
        }
    }

    Ok(prompt)
}

fn write_task_alias(task: &Task, worktree_path: &Path) -> Result<()> {
    let source = worktree_path.join(".crank").join(format!("{}.md", task.id));
    if !source.exists() {
        return Ok(());
    }
    let target = worktree_path.join("TASK.md");
    std::fs::copy(&source, &target)
        .with_context(|| format!("failed to write task alias at {}", target.display()))?;
    store::ensure_git_exclude(worktree_path, "TASK.md")?;
    let task_id_path = worktree_path.join(".crank").join("TASK_ID");
    std::fs::write(&task_id_path, format!("{}\n", task.id))
        .with_context(|| format!("failed to write task id at {}", task_id_path.display()))?;
    store::ensure_git_exclude(worktree_path, ".crank/TASK_ID")?;
    Ok(())
}

fn agent_kind(task: &Task) -> Result<AgentKind> {
    match task.coding_agent.trim().to_lowercase().as_str() {
        "" | "opencode" => Ok(AgentKind::Opencode),
        "codex" => Ok(AgentKind::Codex),
        "claude" => Ok(AgentKind::Claude),
        other => Err(anyhow!("unknown coding_agent: {other}")),
    }
}

fn rename_tmux_window(pane: &str, name: &str) -> Result<()> {
    let status = Command::new("tmux")
        .args(["rename-window", "-t", pane, name])
        .status()
        .context("failed to rename tmux window")?;
    if !status.success() {
        return Err(anyhow!("tmux rename-window failed"));
    }
    Ok(())
}

fn run_direnv_allow(worktree_path: &Path) -> Result<()> {
    if !worktree_path.join(".envrc").exists() {
        return Ok(());
    }
    let status = Command::new("direnv")
        .arg("allow")
        .current_dir(worktree_path)
        .status()
        .context("failed to run direnv allow")?;
    if !status.success() {
        return Err(anyhow!("direnv allow failed"));
    }
    Ok(())
}

fn repo_root_from(path: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
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

fn codex_notify_script(worktree_path: &Path) -> Result<PathBuf> {
    let repo_root = repo_root_from(worktree_path)?;
    let path = repo_root
        .join("projects")
        .join("crank")
        .join("scripts")
        .join("codex-notify");
    if !path.exists() {
        return Err(anyhow!("codex notify script not found: {}", path.display()));
    }
    Ok(path)
}

fn claude_plugin_dir(worktree_path: &Path) -> Result<PathBuf> {
    let repo_root = repo_root_from(worktree_path)?;
    let path = repo_root
        .join("projects")
        .join("crank")
        .join("claude-hooks");
    if !path.exists() {
        return Err(anyhow!("claude hook plugin not found: {}", path.display()));
    }
    Ok(path)
}

fn create_worktree(repo_root: &Path, task: &Task) -> Result<(String, PathBuf)> {
    let status = Command::new("git")
        .args(["fetch", "origin", "master"])
        .current_dir(repo_root)
        .status()
        .context("failed to fetch origin/master")?;
    if !status.success() {
        return Err(anyhow!("git fetch origin master failed"));
    }

    let worktrees_dir = repo_root.join("worktrees");
    crate::crank_io::ensure_dir(&worktrees_dir)
        .with_context(|| format!("failed to create {}", worktrees_dir.display()))?;

    let base = branch::generate_branch_name(&task.path, &task.title, &task.id)?;
    let mut attempts = 0;

    loop {
        let candidate = if attempts == 0 {
            base.clone()
        } else {
            let suffix = format!("-{:03x}", rand::random::<u16>() & 0xfff);
            with_suffix(&base, &suffix)
        };

        let worktree_path = repo_root.join("worktrees").join(&candidate);
        if branch_or_worktree_exists(repo_root, &candidate)? {
            attempts += 1;
            if attempts > 25 {
                return Err(anyhow!("failed to generate unique branch name"));
            }
            continue;
        }
        if worktree_path.exists() {
            attempts += 1;
            if attempts > 25 {
                return Err(anyhow!("failed to generate unique branch name"));
            }
            continue;
        }

        let output = Command::new("git")
            .args(["worktree", "add", "-b", &candidate])
            .arg(&worktree_path)
            .arg("origin/master")
            .current_dir(repo_root)
            .output()
            .context("failed to create worktree")?;

        if output.status.success() {
            return Ok((candidate, worktree_path));
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("already exists") || stderr.contains("already checked out") {
            attempts += 1;
            if attempts > 25 {
                return Err(anyhow!("failed to generate unique branch name"));
            }
            continue;
        }

        return Err(anyhow!("failed to create worktree: {stderr}"));
    }
}

fn branch_or_worktree_exists(repo_root: &Path, branch: &str) -> Result<bool> {
    let status = Command::new("git")
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .current_dir(repo_root)
        .status()
        .context("failed to check branch")?;
    Ok(status.success())
}

fn with_suffix(base: &str, suffix: &str) -> String {
    let max_len = MAX_BRANCH_LEN.saturating_sub(suffix.len());
    let trimmed: String = base.chars().take(max_len).collect();
    format!("{trimmed}{suffix}")
}

#[derive(Clone, Copy)]
enum AgentKind {
    Opencode,
    Codex,
    Claude,
}

impl std::fmt::Debug for AgentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentKind::Opencode => write!(f, "opencode"),
            AgentKind::Codex => write!(f, "codex"),
            AgentKind::Claude => write!(f, "claude"),
        }
    }
}
