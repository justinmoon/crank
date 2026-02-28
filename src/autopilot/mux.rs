use std::process::Command;

use anyhow::{anyhow, Context, Result};

#[derive(Clone, Debug)]
pub enum MuxTarget {
    Tmux { pane: String },
    Zellij { pane_id: u32 },
}

impl MuxTarget {
    pub fn from_env() -> Result<Self> {
        if let Ok(session) = std::env::var("ZELLIJ_SESSION_NAME") {
            if !session.trim().is_empty() {
                let pane = std::env::var("ZELLIJ_PANE_ID").context("ZELLIJ_PANE_ID is not set")?;
                let pane_id = parse_zellij_pane_id(&pane)?;
                return Ok(MuxTarget::Zellij { pane_id });
            }
        }

        if std::env::var("TMUX").unwrap_or_default().is_empty() {
            return Err(anyhow!("crank worker must run inside tmux or zellij"));
        }
        let pane = std::env::var("TMUX_PANE").context("TMUX_PANE is not set")?;
        Ok(MuxTarget::Tmux { pane })
    }

    pub fn from_pane_arg(pane: &str) -> Result<Self> {
        let trimmed = pane.trim();
        if trimmed.is_empty() {
            return Err(anyhow!("pane is required"));
        }
        if let Some(rest) = trimmed.strip_prefix("zellij:") {
            let pane_id = parse_zellij_pane_id(rest)?;
            return Ok(MuxTarget::Zellij { pane_id });
        }
        Ok(MuxTarget::Tmux {
            pane: trimmed.to_string(),
        })
    }

    pub fn to_env_value(&self) -> String {
        match self {
            MuxTarget::Tmux { pane } => format!("tmux:{pane}"),
            MuxTarget::Zellij { pane_id } => format!("zellij:{pane_id}"),
        }
    }
}

pub fn send_nudge(target: &MuxTarget, message: &str) -> Result<()> {
    match target {
        MuxTarget::Tmux { pane } => {
            let status = Command::new("tmux")
                .args(["send-keys", "-t", pane, message, "Enter"])
                .status()
                .context("failed to send tmux nudge")?;
            if !status.success() {
                return Err(anyhow!("tmux send-keys failed"));
            }
        }
        MuxTarget::Zellij { pane_id } => {
            let status = Command::new("zellij")
                .args(["action", "write-chars"])
                .arg(format!("{message}\n"))
                .env("ZELLIJ_PANE_ID", pane_id.to_string())
                .status()
                .context("failed to send zellij nudge")?;
            if !status.success() {
                return Err(anyhow!("zellij action write-chars failed"));
            }
        }
    }
    Ok(())
}

pub fn rename_target(target: &MuxTarget, name: &str) -> Result<()> {
    match target {
        MuxTarget::Tmux { pane } => {
            let status = Command::new("tmux")
                .args(["rename-window", "-t", pane, name])
                .status()
                .context("failed to rename tmux window")?;
            if !status.success() {
                return Err(anyhow!("tmux rename-window failed"));
            }
        }
        MuxTarget::Zellij { pane_id } => {
            let status = Command::new("zellij")
                .args(["action", "rename-pane", name])
                .env("ZELLIJ_PANE_ID", pane_id.to_string())
                .status()
                .context("failed to rename zellij pane")?;
            if !status.success() {
                return Err(anyhow!("zellij rename-pane failed"));
            }
        }
    }
    Ok(())
}

fn parse_zellij_pane_id(raw: &str) -> Result<u32> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("zellij pane id is required"));
    }
    if trimmed.starts_with("plugin_") {
        return Err(anyhow!("zellij plugin pane ids are not supported"));
    }
    let normalized = trimmed.strip_prefix("terminal_").unwrap_or(trimmed);
    let normalized = normalized.trim();
    if normalized.is_empty() {
        return Err(anyhow!("zellij pane id is required"));
    }
    if normalized.starts_with("plugin_") {
        return Err(anyhow!("zellij plugin pane ids are not supported"));
    }
    let pane_id: u32 = normalized.parse().context("invalid zellij pane id")?;
    Ok(pane_id)
}
