use std::os::unix::process::CommandExt;
use std::process::Command;

use anyhow::{anyhow, Context, Result};

use crate::orchestrator::session::SessionSpec;

pub fn run_tmux(concurrency: u16, project: Option<String>) -> Result<()> {
    if !std::env::var("TMUX").unwrap_or_default().is_empty() {
        return Err(anyhow!("crank tmux must be run outside tmux"));
    }
    if concurrency == 0 {
        return Err(anyhow!("concurrency must be at least 1"));
    }

    let spec = SessionSpec::new(concurrency, project)?;

    if session_exists(&spec.session_name)? {
        return Err(anyhow!(
            "tmux session already exists: {}",
            spec.session_name
        ));
    }

    for id in 1..=spec.concurrency {
        let window = spec.worker_name(id);
        let cmd = spec.worker_command_string(id);
        if id == 1 {
            let status = Command::new("tmux")
                .args([
                    "new-session",
                    "-d",
                    "-s",
                    &spec.session_name,
                    "-n",
                    &window,
                    "-c",
                ])
                .arg(&spec.git_root)
                .arg(&cmd)
                .status()
                .context("failed to create tmux session")?;
            if !status.success() {
                return Err(anyhow!("failed to create tmux session"));
            }
        } else {
            let status = Command::new("tmux")
                .args(["new-window", "-t", &spec.session_name, "-n", &window, "-c"])
                .arg(&spec.git_root)
                .arg(&cmd)
                .status()
                .context("failed to create tmux window")?;
            if !status.success() {
                return Err(anyhow!("failed to create tmux window"));
            }
        }
    }

    let tail_cmd = spec.log_tail_command();
    let status = Command::new("tmux")
        .args([
            "new-window",
            "-d",
            "-t",
            &spec.session_name,
            "-n",
            "logs",
            "-c",
        ])
        .arg(&spec.git_root)
        .arg(&tail_cmd)
        .status()
        .context("failed to create tmux logs window")?;
    if !status.success() {
        return Err(anyhow!("failed to create tmux logs window"));
    }

    // Attach to the session (replaces current process)
    let err = Command::new("tmux")
        .args(["attach", "-t", &spec.session_name])
        .exec();
    Err(anyhow!("failed to attach to tmux session: {}", err))
}

fn session_exists(name: &str) -> Result<bool> {
    let status = Command::new("tmux")
        .args(["has-session", "-t", name])
        .status();
    match status {
        Ok(status) => Ok(status.success()),
        Err(err) => Err(err.into()),
    }
}
