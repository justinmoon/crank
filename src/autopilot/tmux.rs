use std::os::unix::process::CommandExt;
use std::process::Command;

use anyhow::{anyhow, Context, Result};

use crate::orchestrator::logging;
use crate::task::git;
use crate::task::model::SupervisionMode;

pub fn run_tmux(concurrency: u16, mode: SupervisionMode) -> Result<()> {
    if !std::env::var("TMUX").unwrap_or_default().is_empty() {
        return Err(anyhow!("crank tmux must be run outside tmux"));
    }
    if concurrency == 0 {
        return Err(anyhow!("concurrency must be at least 1"));
    }

    let git_root = git::git_root()?;
    let session = "crank".to_string();

    if session_exists(&session)? {
        return Err(anyhow!("tmux session already exists: {session}"));
    }

    let crank_bin = std::env::current_exe().context("failed to resolve crank binary path")?;
    let crank_bin = crank_bin
        .to_str()
        .ok_or_else(|| anyhow!("crank binary path is not valid UTF-8"))?;

    for id in 1..=concurrency {
        let window = format!("worker-{id}");
        let worker_args = vec![
            "worker".to_string(),
            "--id".to_string(),
            id.to_string(),
            "--mode".to_string(),
            mode.as_str().to_string(),
        ];
        if id == 1 {
            let status = Command::new("tmux")
                .args(["new-session", "-d", "-s", &session, "-n", &window, "-c"])
                .arg(&git_root)
                .arg(crank_bin)
                .args(&worker_args)
                .status()
                .context("failed to create tmux session")?;
            if !status.success() {
                return Err(anyhow!("failed to create tmux session"));
            }
        } else {
            let status = Command::new("tmux")
                .args(["new-window", "-t", &session, "-n", &window, "-c"])
                .arg(&git_root)
                .arg(crank_bin)
                .args(&worker_args)
                .status()
                .context("failed to create tmux window")?;
            if !status.success() {
                return Err(anyhow!("failed to create tmux window"));
            }
        }
    }

    let log_dir = logging::log_dir()?;
    let mut tail_cmd = String::from("tail -n 200 -F");
    for id in 1..=concurrency {
        tail_cmd.push_str(&format!(
            " {}/worker-{}.log {}/opencode-{}.log",
            log_dir.display(),
            id,
            log_dir.display(),
            id
        ));
    }
    let status = Command::new("tmux")
        .args(["new-window", "-d", "-t", &session, "-n", "logs", "-c"])
        .arg(&git_root)
        .arg(&tail_cmd)
        .status()
        .context("failed to create tmux logs window")?;
    if !status.success() {
        return Err(anyhow!("failed to create tmux logs window"));
    }

    let status = Command::new("tmux")
        .args(["new-window", "-d", "-t", &session, "-n", "alerts", "-c"])
        .arg(&git_root)
        .arg(crank_bin)
        .args(["alerts", "--watch"])
        .status()
        .context("failed to create tmux alerts window")?;
    if !status.success() {
        return Err(anyhow!("failed to create tmux alerts window"));
    }

    if std::env::var("CRANK_TMUX_NO_ATTACH")
        .map(|value| !value.trim().is_empty() && value != "0")
        .unwrap_or(false)
    {
        println!("Created tmux session: {session}");
        println!("Attach with: tmux attach -t {session}");
        return Ok(());
    }

    // Attach to the session (replaces current process)
    let err = Command::new("tmux").args(["attach", "-t", &session]).exec();
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
