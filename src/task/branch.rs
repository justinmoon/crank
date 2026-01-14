use std::env;
use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, Context, Result};

const MAX_BRANCH_LEN: usize = 20;
const DEFAULT_MODEL: &str = "gpt-5-mini";

pub fn generate_branch_name(task_path: &Path, _title: &str, _id: &str) -> Result<String> {
    let model = env::var("TASK_BRANCH_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
    if model.trim().is_empty() {
        return Err(anyhow!(
            "TASK_BRANCH_MODEL is required for AI branch generation"
        ));
    }

    let content = crate::crank_io::read_to_string(task_path)
        .with_context(|| format!("failed to read task file: {}", task_path.display()))?;
    if content.trim().is_empty() {
        return Err(anyhow!("task file is empty: {}", task_path.display()));
    }

    let prompt = format!(
        "The following is a software development task/spec. Generate a short, pithy kebab-case git branch name (max 20 chars) that captures the essence of this work.\n\nExamples of good branch names:\n- dry-justfiles\n- fix-auth-flow\n- add-dark-mode\n- refactor-db\n- task-cli\n\nOnly output the slug, nothing else.\n\n---\n{content}"
    );

    let output = Command::new("llm")
        .arg("-m")
        .arg(model.trim())
        .arg(prompt)
        .output()
        .context("failed to run llm")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("llm failed: {stderr}"));
    }

    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() {
        return Err(anyhow!("llm returned empty branch name"));
    }
    if branch.len() > MAX_BRANCH_LEN {
        return Err(anyhow!("llm branch name too long (max {MAX_BRANCH_LEN})"));
    }

    Ok(branch)
}
