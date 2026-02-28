use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use clap::Args;

use crate::task::claim_next_task;
use crate::task::model::{normalize_task_id, Task, TASK_STATUS_IN_PROGRESS, TASK_STATUS_OPEN};
use crate::task::{git as task_git, store};
use crate::workflow;

#[derive(Args, Clone)]
pub struct RunArgs {
    /// Task ID or path to run
    #[arg(value_name = "task-id")]
    pub task_id: Option<String>,

    /// Force workflow scope (default: loop until waiting or complete)
    #[arg(long, conflicts_with = "task_id")]
    pub workflow: Option<String>,

    /// Run only a single workflow step (no loop)
    #[arg(long, requires = "workflow")]
    pub once: bool,
}

pub fn run_command(args: RunArgs) -> Result<()> {
    let git_root = task_git::git_root()?;
    let repo_root = task_git::repo_root()?;

    let tasks = store::load_tasks(&git_root)?;
    if tasks.is_empty() {
        return Err(anyhow!("no tasks found in {}/.crank", git_root.display()));
    }

    if let Some(task_id) = args.task_id {
        let task =
            find_task(&tasks, &task_id).ok_or_else(|| anyhow!("task not found: {task_id}"))?;
        return run_selected_task(&git_root, &tasks, &task);
    }

    if let Some(workflow_id) = args.workflow {
        if args.once {
            let current = current_task_for_workflow(&git_root, &tasks, &workflow_id);
            return match run_next_workflow_step(&git_root, &tasks, &workflow_id, current.as_ref())?
            {
                WorkflowRunResult::Ran => Ok(()),
                WorkflowRunResult::Waiting(message) => {
                    println!("{message}");
                    Ok(())
                }
                WorkflowRunResult::Complete => {
                    println!("Workflow '{workflow_id}' complete");
                    Ok(())
                }
            };
        }
        return run_workflow_loop(&git_root, &workflow_id);
    }

    if let Some(current_id) = read_current_task_id(&git_root) {
        if let Some(current) = tasks
            .iter()
            .find(|task| task::ids_match(&task.id, &current_id))
        {
            if let Some(workflow_id) = current.workflow.as_deref() {
                match run_next_workflow_step(&git_root, &tasks, workflow_id, Some(current))? {
                    WorkflowRunResult::Ran => return Ok(()),
                    WorkflowRunResult::Waiting(message) => {
                        println!("{message}");
                        return Ok(());
                    }
                    WorkflowRunResult::Complete => {
                        // Fall back to global selection.
                    }
                }
            }
        }
    }

    let task = claim_next_task(&git_root, &repo_root)?;
    let Some(task) = task else {
        println!("No runnable tasks");
        return Ok(());
    };
    run_selected_task(&git_root, &tasks, &task)
}

fn run_selected_task(git_root: &Path, tasks: &[Task], task: &Task) -> Result<()> {
    if task.is_closed() {
        return Err(anyhow!("task '{}' is already closed", task.id));
    }
    let blockers = task.blockers(tasks);
    if !blockers.is_empty() {
        let list = blockers
            .iter()
            .map(|blocker| blocker.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(anyhow!("task '{}' blocked by {}", task.id, list));
    }

    store::write_current_task_marker(git_root, &task.id)?;
    store::update_task_status(&task.path, TASK_STATUS_IN_PROGRESS)?;

    match task
        .run
        .as_deref()
        .map(str::trim)
        .filter(|run| !run.is_empty())
    {
        Some(run) => run_command_step(git_root, task, run),
        None => run_agent_step(git_root, task),
    }
}

enum WorkflowRunResult {
    Ran,
    Waiting(String),
    Complete,
}

fn run_workflow_loop(git_root: &Path, workflow_id: &str) -> Result<()> {
    loop {
        let tasks = store::load_tasks(git_root)?;
        let current = current_task_for_workflow(git_root, &tasks, workflow_id);
        match run_next_workflow_step(git_root, &tasks, workflow_id, current.as_ref())? {
            WorkflowRunResult::Ran => {
                if let Some(current_id) = read_current_task_id(git_root) {
                    let refreshed = store::load_tasks(git_root)?;
                    if let Some(task) = refreshed
                        .iter()
                        .find(|task| task::ids_match(&task.id, &current_id))
                    {
                        if !task.is_closed() {
                            println!(
                                "Workflow '{workflow_id}' paused on step '{}' (status {})",
                                task.id, task.status
                            );
                            return Ok(());
                        }
                    }
                }
            }
            WorkflowRunResult::Waiting(message) => {
                println!("{message}");
                return Ok(());
            }
            WorkflowRunResult::Complete => {
                println!("Workflow '{workflow_id}' complete");
                return Ok(());
            }
        }
    }
}

fn run_next_workflow_step(
    git_root: &Path,
    tasks: &[Task],
    workflow_id: &str,
    current_task: Option<&Task>,
) -> Result<WorkflowRunResult> {
    let workflow_tasks: Vec<Task> = tasks
        .iter()
        .filter(|task| task.workflow.as_deref() == Some(workflow_id))
        .cloned()
        .collect();

    if workflow_tasks.is_empty() {
        return Err(anyhow!("no tasks found for workflow: {workflow_id}"));
    }

    if let Some(current) = current_task {
        if !current.is_closed() {
            let blockers = current.blockers(tasks);
            if !blockers.is_empty() {
                let list = blockers
                    .iter()
                    .map(|blocker| blocker.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Ok(WorkflowRunResult::Waiting(format!(
                    "Workflow '{workflow_id}' waiting on blocked step '{}' (blocked by {})",
                    current.id, list
                )));
            }

            run_selected_task(git_root, tasks, current)?;
            return Ok(WorkflowRunResult::Ran);
        }
    }

    let ordered_steps = workflow_order(git_root, workflow_id, &workflow_tasks)?;
    let start_index = current_task
        .and_then(|task| task.step_id.as_deref())
        .and_then(|step_id| ordered_steps.iter().position(|id| id == step_id))
        .map(|index| index + 1)
        .unwrap_or(0);

    let mut candidate = None;
    for step_id in ordered_steps.iter().skip(start_index) {
        if let Some(task) = workflow_tasks
            .iter()
            .find(|task| task.step_id.as_deref() == Some(step_id.as_str()))
        {
            if task.is_closed() {
                continue;
            }
            candidate = Some(task.clone());
            break;
        }
    }

    let Some(candidate) = candidate else {
        return Ok(WorkflowRunResult::Complete);
    };

    let blockers = candidate.blockers(tasks);
    if !blockers.is_empty() {
        let list = blockers
            .iter()
            .map(|blocker| blocker.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Ok(WorkflowRunResult::Waiting(format!(
            "Workflow '{workflow_id}' waiting on blocked step '{}' (blocked by {})",
            candidate.id, list
        )));
    }

    run_selected_task(git_root, tasks, &candidate)?;
    Ok(WorkflowRunResult::Ran)
}

fn workflow_order(
    git_root: &Path,
    workflow_id: &str,
    workflow_tasks: &[Task],
) -> Result<Vec<String>> {
    if let Some(manifest) = workflow::load_manifest(git_root, workflow_id)? {
        return Ok(manifest.steps);
    }

    let mut steps: Vec<String> = workflow_tasks
        .iter()
        .filter_map(|task| task.step_id.clone())
        .collect();
    steps.sort();
    steps.dedup();
    Ok(steps)
}

fn current_task_for_workflow(git_root: &Path, tasks: &[Task], workflow_id: &str) -> Option<Task> {
    let current_id = read_current_task_id(git_root)?;
    tasks
        .iter()
        .find(|task| {
            task.workflow.as_deref() == Some(workflow_id) && task::ids_match(&task.id, &current_id)
        })
        .filter(|task| !task.is_closed())
        .cloned()
}

fn run_command_step(git_root: &Path, task: &Task, cmd: &str) -> Result<()> {
    let status = Command::new("bash")
        .arg("-lc")
        .arg(cmd)
        .current_dir(git_root)
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .with_context(|| format!("failed to run step '{}'", task.id))?;

    if status.success() {
        store::update_task_status(&task.path, crate::task::model::TASK_STATUS_CLOSED)?;
        return Ok(());
    }

    store::update_task_status(&task.path, TASK_STATUS_OPEN)?;
    Err(anyhow!(
        "step '{}' failed with status {}",
        task.id,
        status.code().unwrap_or(1)
    ))
}

fn run_agent_step(git_root: &Path, task: &Task) -> Result<()> {
    if std::env::var("CRANK_RUN_NO_AGENT").ok().as_deref() == Some("1") {
        println!("Manual step: {}", task.id);
        return Ok(());
    }

    let rel_path = format!(".crank/{}.md", task.id);
    let prompt =
        format!("Read {rel_path} and implement it. Ask clarifying questions first if needed.");

    let agent = task.coding_agent.trim().to_lowercase();
    let status = if agent == "codex" {
        Command::new("codex")
            .arg("--yolo")
            .arg(prompt)
            .current_dir(git_root)
            .status()
            .context("failed to launch codex")?
    } else if agent == "claude" {
        Command::new("claude")
            .arg("--dangerously-skip-permissions")
            .arg(prompt)
            .current_dir(git_root)
            .status()
            .context("failed to launch claude")?
    } else {
        let mut cmd = Command::new("opencode");
        if let Ok(model) = std::env::var("CRANK_MODEL") {
            if !model.trim().is_empty() {
                cmd.arg("--model").arg(model);
            }
        }
        cmd.arg("--prompt")
            .arg(prompt)
            .current_dir(git_root)
            .status()
            .context("failed to launch opencode")?
    };

    if !status.success() {
        store::update_task_status(&task.path, TASK_STATUS_OPEN)?;
        return Err(anyhow!("agent session exited with a failure status"));
    }

    Ok(())
}

fn read_current_task_id(git_root: &Path) -> Option<String> {
    let path = crate::crank_io::repo_crank_dir(git_root).join(".current");
    let content = crate::crank_io::read_to_string(&path).ok()?;
    task::parse_current_task_id(&content)
}

fn find_task(tasks: &[Task], select: &str) -> Option<Task> {
    let trimmed = select.trim();
    if trimmed.is_empty() {
        return None;
    }

    let select_path = PathBuf::from(trimmed);
    if select_path.exists() {
        return tasks.iter().find(|task| task.path == select_path).cloned();
    }

    let task_id = normalize_task_id(trimmed);
    tasks
        .iter()
        .find(|task| task.id == task_id || task.id.starts_with(&task_id))
        .cloned()
}

mod task {
    pub fn parse_current_task_id(content: &str) -> Option<String> {
        let cleaned = content.replace(',', " ");
        cleaned
            .split_whitespace()
            .next()
            .map(|value| value.to_string())
    }

    pub fn ids_match(left: &str, right: &str) -> bool {
        let left = crate::task::model::normalize_task_id(left);
        let right = crate::task::model::normalize_task_id(right);
        !left.is_empty() && left == right
    }
}
