use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::orchestrator::{markers, mux};

const NUDGE_MESSAGE_SUPERVISED: &str = "Continue. If blocked, run crank ask-for-help \"<msg>\". Run tests via just; commit changes (clean git status); run the merge workflow until it passes.";
const NUDGE_MESSAGE_UNSUPERVISED: &str =
    "Continue. Run tests via just; commit changes (clean git status); run the merge workflow until it passes; run crank done when complete.";

pub fn ask_for_help(repo_root: &Path, message: &str) -> Result<PathBuf> {
    if std::env::var("CRANK_SUPERVISION")
        .map(|value| value == "unsupervised")
        .unwrap_or(false)
    {
        return Err(anyhow!(
            "crank ask-for-help is disabled in unsupervised mode"
        ));
    }
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
    let target = mux::MuxTarget::from_pane_arg(pane)?;
    nudge_task(&task_id, &target)
}

pub fn nudge_task(task_id: &str, target: &mux::MuxTarget) -> Result<()> {
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

    let message = if std::env::var("CRANK_SUPERVISION")
        .map(|value| value == "unsupervised")
        .unwrap_or(false)
    {
        NUDGE_MESSAGE_UNSUPERVISED
    } else {
        NUDGE_MESSAGE_SUPERVISED
    };

    mux::send_nudge(target, message)?;

    Ok(())
}
