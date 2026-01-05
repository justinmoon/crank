use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, Context, Result};

const MAX_BRANCH_LEN: usize = 20;
const DEFAULT_BRANCH_MODEL: &str = "gpt-4.1-mini";

pub fn generate_branch_name(task_path: &Path, title: &str, id: &str) -> Result<String> {
    let model = std::env::var("TASK_BRANCH_MODEL").unwrap_or_else(|_| DEFAULT_BRANCH_MODEL.into());
    let model = model.trim();
    let model = if model.is_empty() {
        DEFAULT_BRANCH_MODEL
    } else {
        model
    };

    let content = fs::read_to_string(task_path)
        .with_context(|| format!("failed to read task file: {}", task_path.display()))?;
    if content.trim().is_empty() {
        return Err(anyhow!("task file is empty: {}", task_path.display()));
    }

    let prompt = format!(
        "The following is a software development task/spec. Generate a short, pithy kebab-case git branch name (max 20 chars) that captures the essence of this work.\n\nExamples of good branch names:\n- dry-justfiles\n- fix-auth-flow\n- add-dark-mode\n- refactor-db\n- task-cli\n\nOnly output the slug, nothing else.\n\n---\n{content}"
    );

    let output = Command::new("llm")
        .arg("-m")
        .arg(model)
        .arg(prompt)
        .output()
        .context("failed to run llm")?;

    let llm_branch = if output.status.success() {
        sanitize_branch(&String::from_utf8_lossy(&output.stdout))
    } else {
        String::new()
    };

    let fallback = fallback_branch(title, id);
    let mut branch = if llm_branch.is_empty() || llm_branch.len() > MAX_BRANCH_LEN {
        fallback
    } else {
        llm_branch
    };

    if branch.is_empty() {
        return Err(anyhow!("unable to generate branch name"));
    }

    if branch.len() > MAX_BRANCH_LEN {
        branch = truncate_branch(&branch, MAX_BRANCH_LEN);
    }

    Ok(branch)
}

fn fallback_branch(title: &str, id: &str) -> String {
    let mut branch = sanitize_branch(title);
    if branch.is_empty() {
        branch = sanitize_branch(&format!("task-{id}"));
    }
    if branch.is_empty() {
        branch = sanitize_branch(id);
    }
    if branch.len() > MAX_BRANCH_LEN {
        branch = truncate_branch(&branch, MAX_BRANCH_LEN);
    }
    branch
}

fn sanitize_branch(value: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in value.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
            continue;
        }
        if (ch == '-' || ch == '_' || ch == ' ' || ch == '.' || ch == '/')
            && !out.is_empty()
            && !last_dash
        {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn truncate_branch(value: &str, max_len: usize) -> String {
    if value.len() <= max_len {
        return value.to_string();
    }
    value.chars().take(max_len).collect()
}
