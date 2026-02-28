use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use std::os::unix::fs::PermissionsExt;
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

use crate::task::branch;
use crate::task::claim;
use crate::task::creator;
use crate::task::git;
use crate::task::model::SupervisionMode;
use crate::task::model::{
    normalize_task_id, sort_tasks, Task, TASK_STATUS_CLOSED, TASK_STATUS_IN_PROGRESS,
};
use crate::task::prompts::{self, BranchMethod, CodingAgent};
use crate::task::store;
use crate::task::tui;

const COMMIT_MSG_HOOK: &str = r#"#!/bin/sh

MARKER=".crank/.current"
MSG_FILE="$1"

REPO_ROOT=$(git rev-parse --show-toplevel 2>/dev/null || true)
if [ -n "$REPO_ROOT" ]; then
    MARKER="$REPO_ROOT/.crank/.current"
fi

if [ -z "$MSG_FILE" ] || [ ! -f "$MSG_FILE" ]; then
    exit 0
fi

if [ ! -f "$MARKER" ]; then
    exit 0
fi

ids=$(tr '\n' ' ' < "$MARKER" | tr ',' ' ' | tr -s ' ' | sed 's/^ //; s/ $//')
if [ -z "$ids" ]; then
    exit 0
fi

if grep -qi '^Issues:' "$MSG_FILE"; then
    exit 0
fi

printf '\nIssues: %s\n' "$ids" >> "$MSG_FILE"
"#;

pub fn run_next(no_worktree: bool, select: Option<String>) -> Result<()> {
    let git_root = git::git_root()?;
    let repo_root = git::repo_root()?;

    let mut tasks = store::load_tasks(&git_root)?;
    if tasks.is_empty() {
        return Err(anyhow!("no tasks found in {}/.crank", git_root.display()));
    }

    sort_tasks(&mut tasks);

    let selected = if let Some(select) = select {
        find_selected_task(&tasks, &select)
            .ok_or_else(|| anyhow!("task not found: {select}"))?
            .clone()
    } else {
        let selected_path = tui::run_picker(&tasks, &git_root, tui::PickerOptions::default())?;
        if selected_path.is_none() {
            return Ok(());
        }
        let selected_path = selected_path.unwrap();
        tasks
            .iter()
            .find(|task| task.path == selected_path)
            .cloned()
            .ok_or_else(|| anyhow!("task not found"))?
    };

    if no_worktree {
        println!("{}", selected.path.display());
        return Ok(());
    }

    if std::env::var("TMUX").unwrap_or_default().is_empty() {
        return Err(anyhow!(
            "must be running inside tmux (or use --no-worktree)"
        ));
    }

    store::update_task_status(&selected.path, TASK_STATUS_IN_PROGRESS)
        .context("failed to mark task in progress")?;

    let coding_agent = prompts::prompt_coding_agent()?;
    let model = if matches!(coding_agent, CodingAgent::Opencode) {
        Some(prompts::prompt_model()?)
    } else {
        None
    };
    let branch_method = prompts::prompt_branch_method()?;

    let branch = match branch_method {
        BranchMethod::Manual => prompts::prompt_branch_name()?,
        BranchMethod::Ai => {
            print!("Generating branch name... ");
            let branch =
                branch::generate_branch_name(&selected.path, &selected.title, &selected.id)?;
            println!("{branch}");
            branch
        }
    };

    let worktree_path = repo_root.join("worktrees").join(&branch);

    if !worktree_path.exists() {
        crate::crank_io::ensure_dir(worktree_path.parent().unwrap())
            .context("failed to create worktrees dir")?;

        let output = Command::new("git")
            .arg("worktree")
            .arg("add")
            .arg("-b")
            .arg(&branch)
            .arg(&worktree_path)
            .current_dir(&git_root)
            .output()
            .context("failed to create worktree")?;

        if !output.status.success() {
            return Err(anyhow!(
                "failed to create worktree: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        println!("Created worktree: {}", worktree_path.display());
    } else {
        println!("Using existing worktree: {}", worktree_path.display());
    }

    store::write_current_task_marker(&worktree_path, &selected.id)
        .context("failed to write current task marker")?;

    let rel_issue_path = format!(".crank/{}.md", selected.id);

    run_tmux_flow(
        &branch,
        &worktree_path,
        coding_agent,
        model.as_deref(),
        &rel_issue_path,
    )
}

fn run_tmux_flow(
    branch: &str,
    worktree_path: &Path,
    coding_agent: CodingAgent,
    model: Option<&str>,
    rel_issue_path: &str,
) -> Result<()> {
    let status = Command::new("tmux")
        .args(["new-window", "-n", branch, "-c"])
        .arg(worktree_path)
        .status()
        .context("failed to create tmux window")?;
    if !status.success() {
        return Err(anyhow!("failed to create tmux window"));
    }

    thread::sleep(Duration::from_millis(300));

    let status = Command::new("tmux")
        .args(["send-keys", "-t", branch, "direnv allow", "Enter"])
        .status()
        .context("failed to send direnv allow")?;
    if !status.success() {
        return Err(anyhow!("failed to send direnv allow"));
    }

    thread::sleep(Duration::from_millis(200));

    let prompt = format!(
        "Read {rel_issue_path} and implement it. Ask clarifying questions first if needed."
    );

    let (cmd, agent_name) = match coding_agent {
        CodingAgent::Opencode => {
            let model = model.expect("model required for opencode");
            (
                format!("opencode --model '{model}' --prompt '{prompt}'"),
                "opencode",
            )
        }
        CodingAgent::Claude => (
            format!("claude --dangerously-skip-permissions '{prompt}'"),
            "claude",
        ),
        CodingAgent::Codex => (format!("codex --yolo '{prompt}'"), "codex"),
    };

    let status = Command::new("tmux")
        .args(["send-keys", "-t", branch])
        .arg(&cmd)
        .arg("Enter")
        .status()
        .context(format!("failed to launch {agent_name}"))?;
    if !status.success() {
        return Err(anyhow!("failed to launch {agent_name}"));
    }

    println!("Launched {agent_name} in tmux window '{branch}'");
    Ok(())
}

fn find_selected_task<'a>(tasks: &'a [Task], select: &str) -> Option<&'a Task> {
    let trimmed = select.trim();
    if trimmed.is_empty() {
        return None;
    }

    let select_path = PathBuf::from(trimmed);
    if select_path.exists() {
        return tasks.iter().find(|task| task.path == select_path);
    }

    let task_id = normalize_task_id(trimmed);
    tasks
        .iter()
        .find(|task| task.id == task_id || task.id.starts_with(&task_id))
}

pub fn run_create(
    title: Option<String>,
    priority: Option<i32>,
    supervision: Option<SupervisionMode>,
    use_editor: bool,
    use_opencode: bool,
    deps: Option<String>,
) -> Result<()> {
    if use_opencode {
        return launch_opencode();
    }

    let git_root = git::git_root()?;

    let dependencies = if let Some(deps) = deps {
        creator::parse_deps_flag(&deps)?
    } else {
        Vec::new()
    };

    if use_editor {
        return creator::create_task_interactive(&git_root, title, priority, supervision);
    }

    creator::create_task_file(&git_root, title, priority, supervision, &dependencies)
}

pub fn run_done(task_id: &str, pr_number: Option<i32>) -> Result<()> {
    let git_root = git::git_root()?;

    let task_id = normalize_task_id(task_id);
    if task_id.is_empty() {
        return Err(anyhow!("task id is required"));
    }

    let task_path = git::task_path_for_id(&git_root, &task_id);
    if !task_path.exists() {
        return Err(anyhow!("task not found: .crank/{task_id}.md"));
    }

    let status = if let Some(pr) = pr_number {
        format!("{TASK_STATUS_CLOSED} #{pr}")
    } else {
        TASK_STATUS_CLOSED.to_string()
    };

    store::update_task_status(&task_path, &status)?;

    println!("Marked .crank/{task_id}.md as {status}");
    Ok(())
}

pub fn run_claim(json: bool) -> Result<()> {
    let git_root = git::git_root()?;
    let repo_root = git::repo_root()?;

    let task = claim::claim_next_task(&git_root, &repo_root)?;
    let Some(task) = task else {
        return Err(anyhow!("no claimable tasks"));
    };

    if json {
        let output = serde_json::to_string_pretty(&task)?;
        println!("{output}");
    } else {
        println!("{}", task.path.display());
    }

    Ok(())
}

pub fn run_hooks_install() -> Result<()> {
    let git_root = git::git_root()?;
    let hooks_dir = git_root.join(".githooks");
    crate::crank_io::ensure_dir(&hooks_dir)
        .with_context(|| format!("failed to create hooks directory: {}", hooks_dir.display()))?;

    let hook_path = hooks_dir.join("commit-msg");
    crate::crank_io::write_string(&hook_path, COMMIT_MSG_HOOK)
        .with_context(|| format!("failed to write commit-msg hook: {}", hook_path.display()))?;

    let mut perms = fs::metadata(&hook_path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&hook_path, perms)?;

    println!("Installed .githooks/commit-msg");
    println!("Enable with: git config core.hooksPath .githooks");
    Ok(())
}

fn launch_opencode() -> Result<()> {
    let status = Command::new("opencode")
        .args(["--agent", "task-creator"])
        .status()
        .context("failed to launch opencode")?;
    if !status.success() {
        return Err(anyhow!("opencode task creator failed"));
    }
    Ok(())
}
