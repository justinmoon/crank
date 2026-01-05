use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use chrono::Local;

pub fn read_current_task_id(repo_root: &Path) -> Result<String> {
    let marker = crate::crank_io::repo_crank_dir(repo_root).join(".current");
    let content = crate::crank_io::read_to_string(&marker)
        .with_context(|| format!("failed to read current task marker: {}", marker.display()))?;
    parse_current_task_id(&content)
        .ok_or_else(|| anyhow!("current task marker is empty: {}", marker.display()))
}

pub fn write_help_marker(task_id: &str, message: &str) -> Result<PathBuf> {
    let path = help_marker_path(task_id)?;
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let content = format!(
        "# Help requested\n\nTime: {timestamp}\nTask: {task_id}\n\n## Message\n\n{message}\n"
    );
    write_marker_with_content(&path, &content)?;
    Ok(path)
}

pub fn write_pause_marker(task_id: &str) -> Result<PathBuf> {
    let path = pause_marker_path(task_id)?;
    write_marker(&path, "pause")?;
    Ok(path)
}

pub fn clear_pause_marker(task_id: &str) -> Result<()> {
    let path = pause_marker_path(task_id)?;
    let _ = fs::remove_file(path);
    Ok(())
}

pub fn clear_task_markers(task_id: &str) -> Result<()> {
    let _ = fs::remove_file(merged_marker_path(task_id)?);
    let _ = fs::remove_file(help_marker_path(task_id)?);
    let _ = fs::remove_file(pause_marker_path(task_id)?);
    let _ = fs::remove_file(activity_marker_path(task_id)?);
    Ok(())
}

pub fn write_merged_marker(task_id: &str) -> Result<PathBuf> {
    let path = merged_marker_path(task_id)?;
    write_marker(&path, "merged")?;
    Ok(path)
}

pub fn merged_marker_exists(task_id: &str) -> Result<bool> {
    Ok(merged_marker_path(task_id)?.exists())
}

pub fn help_marker_exists(task_id: &str) -> Result<bool> {
    Ok(help_marker_path(task_id)?.exists())
}

pub fn pause_marker_exists(task_id: &str) -> Result<bool> {
    Ok(pause_marker_path(task_id)?.exists())
}

pub fn touch_activity_marker(task_id: &str) -> Result<()> {
    let path = activity_marker_path(task_id)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    write_marker_with_content(&path, &format!("{timestamp}\n"))
}

pub fn read_activity_time(task_id: &str) -> Result<Option<SystemTime>> {
    let path = activity_marker_path(task_id)?;
    if !path.exists() {
        return Ok(None);
    }
    let content = crate::crank_io::read_to_string(&path)
        .with_context(|| format!("failed to read activity marker: {}", path.display()))?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let secs: u64 = trimmed.parse().context("invalid activity timestamp")?;
    Ok(Some(UNIX_EPOCH + std::time::Duration::from_secs(secs)))
}

pub fn merged_marker_path(task_id: &str) -> Result<PathBuf> {
    Ok(crank_dir()?.join("merged").join(task_id))
}

pub fn help_marker_path(task_id: &str) -> Result<PathBuf> {
    Ok(crank_dir()?.join("help").join(format!("{task_id}.md")))
}

pub fn pause_marker_path(task_id: &str) -> Result<PathBuf> {
    Ok(crank_dir()?.join("pause").join(task_id))
}

pub fn activity_marker_path(task_id: &str) -> Result<PathBuf> {
    Ok(crank_dir()?.join("activity").join(task_id))
}

fn crank_dir() -> Result<PathBuf> {
    crate::crank_io::user_crank_dir()
}

fn write_marker(path: &Path, label: &str) -> Result<()> {
    write_marker_with_content(path, &format!("{label}\n"))
}

fn write_marker_with_content(path: &Path, content: &str) -> Result<()> {
    crate::crank_io::write_string(path, content)
        .with_context(|| format!("failed to write marker: {}", path.display()))?;
    Ok(())
}

fn parse_current_task_id(content: &str) -> Option<String> {
    let cleaned = content.replace(',', " ");
    cleaned
        .split_whitespace()
        .next()
        .map(|value| value.to_string())
}
