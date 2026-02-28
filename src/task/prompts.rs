use anyhow::{anyhow, Result};
use dialoguer::{Input, Select};

use crate::task::model::SupervisionMode;

#[derive(Debug, Clone, Copy)]
pub enum BranchMethod {
    Ai,
    Manual,
}

#[derive(Debug, Clone, Copy)]
pub enum CodingAgent {
    Opencode,
    Claude,
    Codex,
}

pub fn prompt_task_fields(
    title: Option<String>,
    priority: Option<i32>,
    supervision: Option<SupervisionMode>,
) -> Result<(String, i32, SupervisionMode)> {
    let mut title = title.unwrap_or_default();
    let mut priority = priority.unwrap_or(0);
    let mut supervision = supervision;

    if title.trim().is_empty() {
        title = Input::new()
            .with_prompt("Task title")
            .with_initial_text("")
            .interact_text()?;
    }

    if priority == 0 {
        let priorities = [
            "5 - Urgent",
            "4 - High",
            "3 - Normal",
            "2 - Low",
            "1 - Backlog",
        ];
        let selection = Select::new()
            .with_prompt("Priority")
            .items(&priorities)
            .default(2)
            .interact()?;
        priority = match selection {
            0 => 5,
            1 => 4,
            2 => 3,
            3 => 2,
            4 => 1,
            _ => 0,
        };
    }

    if supervision.is_none() {
        supervision = Some(prompt_supervision_mode()?);
    }

    if title.trim().is_empty() {
        return Err(anyhow!("title is required"));
    }
    if priority == 0 {
        return Err(anyhow!("priority is required"));
    }

    Ok((
        title.trim().to_string(),
        priority,
        supervision.ok_or_else(|| anyhow!("supervision is required"))?,
    ))
}

pub fn prompt_supervision_mode() -> Result<SupervisionMode> {
    let options = ["supervised (manual selection)", "unsupervised (auto-claim)"];
    let selection = Select::new()
        .with_prompt("Supervision")
        .items(&options)
        .default(0)
        .interact()?;
    Ok(match selection {
        0 => SupervisionMode::Supervised,
        1 => SupervisionMode::Unsupervised,
        _ => return Err(anyhow!("supervision selection required")),
    })
}

pub fn prompt_model() -> Result<String> {
    let options = [
        ("Claude Opus 4.5 (latest)", "claude-opus-4-5-20250514"),
        ("GPT 5.2 Codex High (OAuth)", "openai/gpt-5.2-codex-high"),
    ];
    let labels: Vec<&str> = options.iter().map(|(label, _)| *label).collect();
    let selection = Select::new()
        .with_prompt("Select model")
        .items(&labels)
        .default(0)
        .interact()?;
    Ok(options
        .get(selection)
        .ok_or_else(|| anyhow!("model selection required"))?
        .1
        .to_string())
}

pub fn prompt_branch_method() -> Result<BranchMethod> {
    let options = ["AI-generate", "Enter manually"];
    let selection = Select::new()
        .with_prompt("Branch name")
        .items(&options)
        .default(0)
        .interact()?;
    Ok(if selection == 0 {
        BranchMethod::Ai
    } else {
        BranchMethod::Manual
    })
}

pub fn prompt_branch_name() -> Result<String> {
    let branch = Input::<String>::new()
        .with_prompt("Branch name")
        .with_initial_text("")
        .interact_text()?;
    let trimmed = branch.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("branch name is required"));
    }
    Ok(trimmed.to_string())
}

pub fn prompt_coding_agent() -> Result<CodingAgent> {
    let options = ["Opencode", "Claude Code", "Codex"];
    let selection = Select::new()
        .with_prompt("Select coding agent")
        .items(&options)
        .default(0)
        .interact()?;
    Ok(match selection {
        0 => CodingAgent::Opencode,
        1 => CodingAgent::Claude,
        _ => CodingAgent::Codex,
    })
}
