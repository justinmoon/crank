use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};

use crate::autopilot::markers;

const NUDGE_MESSAGE: &str = "Continue. If blocked, run crank ask-for-help \"<msg>\". Run tests via just; commit changes (clean git status); run the merge workflow until it passes.";

pub fn ask_for_help(repo_root: &Path, message: &str) -> Result<PathBuf> {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("help message is required"));
    }
    let task_id = markers::read_current_task_id(repo_root)?;
    let help_path = markers::write_help_marker(&task_id, trimmed)?;
    let _ = markers::write_pause_marker(&task_id)?;
    Ok(help_path)
}

pub fn pause(repo_root: &Path, clear: bool) -> Result<Option<PathBuf>> {
    let task_id = markers::read_current_task_id(repo_root)?;
    if clear {
        markers::clear_pause_marker(&task_id)?;
        Ok(None)
    } else {
        Ok(Some(markers::write_pause_marker(&task_id)?))
    }
}

pub fn nudge(repo_root: &Path, pane: &str) -> Result<()> {
    let task_id = markers::read_current_task_id(repo_root)?;
    nudge_task(&task_id, pane)
}

pub fn nudge_task(task_id: &str, pane: &str) -> Result<()> {
    let pane = pane.trim();
    if pane.is_empty() {
        return Err(anyhow!("tmux pane is required"));
    }
    if markers::merged_marker_exists(task_id)? {
        return Ok(());
    }
    if markers::help_marker_exists(task_id)? {
        return Ok(());
    }
    if markers::pause_marker_exists(task_id)? {
        return Ok(());
    }

    markers::touch_activity_marker(task_id)?;

    let status = Command::new("tmux")
        .args(["send-keys", "-t", pane, NUDGE_MESSAGE, "Enter"])
        .status()
        .context("failed to send tmux nudge")?;
    if !status.success() {
        return Err(anyhow!("tmux send-keys failed"));
    }

    Ok(())
}
