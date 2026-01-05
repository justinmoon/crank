use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use chrono::{Local, NaiveDate};
use rand::random;
use serde::Deserialize;

use crate::task::model::{matches_task_id, Dependency, SupervisionMode, Task};

#[derive(Debug, Default, Deserialize)]
struct TaskFrontmatter {
    priority: Option<i32>,
    status: Option<String>,
    supervision: Option<SupervisionMode>,
    title: Option<String>,
    depends_on: Option<Vec<Dependency>>,
    workflow: Option<String>,
    step_id: Option<String>,
    run: Option<String>,
    coding_agent: Option<String>,
    created: Option<NaiveDate>,
}

pub fn load_tasks(git_root: &Path) -> Result<Vec<Task>> {
    let tasks_dir = crate::crank_io::repo_crank_dir(git_root);
    let entries = match fs::read_dir(&tasks_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };

    let mut tasks = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        if let Ok(task) = parse_task(&path) {
            tasks.push(task);
        }
    }

    Ok(tasks)
}

pub fn parse_task(path: &Path) -> Result<Task> {
    let content = crate::crank_io::read_to_string(path)
        .with_context(|| format!("failed to read task file: {}", path.display()))?;

    let (frontmatter, title_fallback) = parse_frontmatter(&content)?;
    let body_run = extract_run_command(&content);
    let id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_string();

    let title = frontmatter
        .title
        .clone()
        .filter(|title| !title.trim().is_empty())
        .or(title_fallback)
        .unwrap_or_default();

    let supervision = frontmatter
        .supervision
        .ok_or_else(|| anyhow!("supervision is required in frontmatter: {}", path.display()))?;

    Ok(Task {
        priority: frontmatter.priority.unwrap_or_default(),
        status: frontmatter.status.unwrap_or_default(),
        supervision,
        title,
        depends_on: frontmatter.depends_on.unwrap_or_default(),
        workflow: frontmatter
            .workflow
            .filter(|value| !value.trim().is_empty()),
        step_id: frontmatter.step_id.filter(|value| !value.trim().is_empty()),
        run: body_run
            .or(frontmatter.run)
            .filter(|value| !value.trim().is_empty()),
        coding_agent: frontmatter
            .coding_agent
            .unwrap_or_else(|| "opencode".to_string()),
        created: frontmatter.created,
        path: path.to_path_buf(),
        id,
    })
}

fn parse_frontmatter(content: &str) -> Result<(TaskFrontmatter, Option<String>)> {
    let mut in_frontmatter = false;
    let mut frontmatter_lines = Vec::new();
    let mut title_fallback = None;
    let mut frontmatter_done = false;

    for line in content.lines() {
        if line.trim() == "---" {
            if !in_frontmatter {
                in_frontmatter = true;
                continue;
            } else {
                frontmatter_done = true;
                continue;
            }
        }

        if in_frontmatter && !frontmatter_done {
            frontmatter_lines.push(line);
            continue;
        }

        if title_fallback.is_none() {
            if let Some(title) = line.strip_prefix("# ") {
                title_fallback = Some(title.trim().to_string());
            }
        }
    }

    if !in_frontmatter || !frontmatter_done {
        return Err(anyhow!("frontmatter not found"));
    }

    let frontmatter = frontmatter_lines.join("\n");
    let parsed: TaskFrontmatter = serde_yaml::from_str(&frontmatter)?;
    Ok((parsed, title_fallback))
}

fn extract_run_command(content: &str) -> Option<String> {
    let mut in_run_section = false;
    let mut in_code_block = false;
    let mut lines = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "### Run" {
            in_run_section = true;
            continue;
        }

        if in_run_section {
            if trimmed.starts_with("```") {
                if in_code_block {
                    break;
                } else {
                    in_code_block = true;
                    continue;
                }
            }

            if in_code_block {
                lines.push(line);
            }
        }
    }

    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n").trim().to_string())
    }
}

pub fn task_template(
    title: &str,
    priority: i32,
    supervision: SupervisionMode,
    created: &str,
    deps: &[Dependency],
) -> String {
    let priority_line = if priority == 0 {
        "priority:".to_string()
    } else {
        format!("priority: {priority}")
    };

    let title_line = if title.trim().is_empty() {
        "title:".to_string()
    } else {
        format!("title: {title}")
    };

    let mut deps_section = String::new();
    if !deps.is_empty() {
        deps_section.push_str("depends_on:\n");
        for dep in deps {
            deps_section.push_str(&format!("  - id: {}\n    type: {}\n", dep.id, dep.dep_type));
        }
    }

    format!(
        "---\n{title_line}\n{priority_line}\nstatus: open\nsupervision: {}\ncoding_agent: opencode\ncreated: {created}\n{deps_section}---\n\n## Intent\n\n## Spec\n",
        supervision.as_str()
    )
}

pub fn create_task_file(
    git_root: &Path,
    title: &str,
    priority: i32,
    supervision: SupervisionMode,
    deps: &[Dependency],
) -> Result<PathBuf> {
    let id = generate_id();
    let date = Local::now().format("%Y-%m-%d").to_string();
    let filename = format!("{id}.md");
    let tasks_dir = crate::crank_io::repo_crank_dir(git_root);
    let task_path = tasks_dir.join(&filename);

    crate::crank_io::ensure_dir(&tasks_dir)
        .with_context(|| format!("failed to create tasks directory: {}", tasks_dir.display()))?;

    let content = task_template(title, priority, supervision, &date, deps);
    crate::crank_io::write_string(&task_path, content)
        .with_context(|| format!("failed to write task file: {}", task_path.display()))?;

    println!("Created: .crank/{filename}");
    Ok(task_path)
}

pub fn write_current_task_marker(git_root: &Path, task_id: &str) -> Result<()> {
    let trimmed = task_id.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("task id is required"));
    }

    let tasks_dir = crate::crank_io::repo_crank_dir(git_root);
    crate::crank_io::ensure_dir(&tasks_dir)
        .with_context(|| format!("failed to create tasks directory: {}", tasks_dir.display()))?;

    let marker_path = tasks_dir.join(".current");
    let content = format!("{trimmed}\n");
    crate::crank_io::write_string(&marker_path, content).with_context(|| {
        format!(
            "failed to write current task marker: {}",
            marker_path.display()
        )
    })?;

    ensure_git_exclude(git_root, ".crank/.current")?;

    Ok(())
}

pub fn update_task_status(task_path: &Path, status: &str) -> Result<()> {
    update_frontmatter_field(task_path, "status", status)
}

pub fn update_task_priority(task_path: &Path, priority: i32) -> Result<()> {
    update_frontmatter_field(task_path, "priority", &priority.to_string())
}

fn update_frontmatter_field(task_path: &Path, key: &str, value: &str) -> Result<()> {
    let content = crate::crank_io::read_to_string(task_path)
        .with_context(|| format!("failed to read task file: {}", task_path.display()))?;
    let had_trailing_newline = content.ends_with('\n');

    let mut lines: Vec<String> = content.lines().map(|line| line.to_string()).collect();
    let (start, end) = find_frontmatter_bounds(&lines)
        .ok_or_else(|| anyhow!("frontmatter not found in {}", task_path.display()))?;

    let mut updated = false;
    let field_prefix = format!("{key}:");

    for line in lines.iter_mut().take(end).skip(start + 1) {
        if line.trim_start().starts_with(&field_prefix) {
            *line = format!("{key}: {value}");
            updated = true;
            break;
        }
    }

    if !updated {
        lines.insert(end, format!("{key}: {value}"));
    }

    let mut new_content = lines.join("\n");
    if had_trailing_newline {
        new_content.push('\n');
    }

    crate::crank_io::write_string(task_path, new_content)
        .with_context(|| format!("failed to write task file: {}", task_path.display()))?;

    Ok(())
}

pub(crate) fn ensure_git_exclude(git_root: &Path, pattern: &str) -> Result<()> {
    let git_dir = crate::task::git::git_common_dir_from(git_root)?;
    let exclude_path = git_dir.join("info").join("exclude");
    if let Some(parent) = exclude_path.parent() {
        crate::crank_io::ensure_dir(parent)
            .with_context(|| format!("failed to create exclude dir: {}", parent.display()))?;
    }
    let mut content = crate::crank_io::read_to_string(&exclude_path).unwrap_or_default();
    if content.lines().any(|line| line.trim() == pattern) {
        return Ok(());
    }
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(pattern);
    content.push('\n');
    crate::crank_io::write_string(&exclude_path, content).with_context(|| {
        format!(
            "failed to update git exclude file: {}",
            exclude_path.display()
        )
    })?;
    Ok(())
}

pub fn toggle_task_status(task: &mut Task) -> Result<()> {
    let content = crate::crank_io::read_to_string(&task.path)
        .with_context(|| format!("failed to read task file: {}", task.path.display()))?;
    let had_trailing_newline = content.ends_with('\n');

    let mut lines: Vec<String> = content.lines().map(|line| line.to_string()).collect();
    let (start, end) = find_frontmatter_bounds(&lines)
        .ok_or_else(|| anyhow!("frontmatter not found in {}", task.path.display()))?;

    let is_closed = task.status == "closed" || task.status.starts_with("closed ");
    let new_status = if is_closed { "open" } else { "closed" };

    let mut updated = false;
    for line in lines.iter_mut().take(end).skip(start + 1) {
        if line.trim_start().starts_with("status:") {
            let mut parts = line.splitn(2, ':');
            let _ = parts.next();
            let rest = parts.next().unwrap_or("").trim_start();
            let mut rest_parts = rest.splitn(2, ' ');
            let _ = rest_parts.next();
            let remainder = rest_parts.next().unwrap_or("").trim_start();
            if remainder.is_empty() {
                *line = format!("status: {new_status}");
            } else {
                *line = format!("status: {new_status} {remainder}");
            }
            updated = true;
            break;
        }
    }

    if !updated {
        lines.insert(end, format!("status: {new_status}"));
    }

    let mut new_content = lines.join("\n");
    if had_trailing_newline {
        new_content.push('\n');
    }

    crate::crank_io::write_string(&task.path, new_content)
        .with_context(|| format!("failed to write task file: {}", task.path.display()))?;

    task.status = new_status.to_string();
    Ok(())
}

pub fn change_task_priority(task: &mut Task, delta: i32) -> Result<()> {
    let mut new_priority = task.priority + delta;
    new_priority = new_priority.clamp(1, 5);
    if new_priority == task.priority {
        return Ok(());
    }
    update_task_priority(&task.path, new_priority)?;
    task.priority = new_priority;
    Ok(())
}

pub fn delete_task(task: &Task) -> Result<()> {
    fs::remove_file(&task.path)
        .with_context(|| format!("failed to delete task file: {}", task.path.display()))
}

pub fn add_dependency_to_file(task_path: &Path, dep: &Dependency) -> Result<()> {
    let content = crate::crank_io::read_to_string(task_path)
        .with_context(|| format!("failed to read task file: {}", task_path.display()))?;
    let had_trailing_newline = content.ends_with('\n');

    let mut lines: Vec<String> = content.lines().map(|line| line.to_string()).collect();
    let (start, end) = find_frontmatter_bounds(&lines)
        .ok_or_else(|| anyhow!("frontmatter not found in {}", task_path.display()))?;

    let mut depends_on_line = None;
    for (i, line) in lines.iter().enumerate().take(end).skip(start + 1) {
        if line.trim_start().starts_with("depends_on:") {
            depends_on_line = Some(i);
            break;
        }
    }

    let new_lines = vec![
        format!("  - id: {}", dep.id),
        format!("    type: {}", dep.dep_type),
    ];

    match depends_on_line {
        None => {
            let mut insert = Vec::new();
            insert.push("depends_on:".to_string());
            insert.extend(new_lines);
            lines.splice(end..end, insert);
        }
        Some(dep_line) => {
            let mut insert_at = dep_line + 1;
            while insert_at < end
                && (lines[insert_at].starts_with("  - ") || lines[insert_at].starts_with("    "))
            {
                insert_at += 1;
            }
            lines.splice(insert_at..insert_at, new_lines);
        }
    }

    let mut new_content = lines.join("\n");
    if had_trailing_newline {
        new_content.push('\n');
    }

    crate::crank_io::write_string(task_path, new_content)
        .with_context(|| format!("failed to write task file: {}", task_path.display()))?;

    Ok(())
}

pub fn remove_dependency_from_file(task_path: &Path, dep_id: &str) -> Result<()> {
    let content = crate::crank_io::read_to_string(task_path)
        .with_context(|| format!("failed to read task file: {}", task_path.display()))?;
    let had_trailing_newline = content.ends_with('\n');

    let lines: Vec<String> = content.lines().map(|line| line.to_string()).collect();
    let (start, end) = find_frontmatter_bounds(&lines)
        .ok_or_else(|| anyhow!("frontmatter not found in {}", task_path.display()))?;

    let mut new_lines = Vec::new();
    let mut skip_next = false;
    let mut in_depends_on = false;
    let mut has_depends_on = false;

    for (i, line) in lines.iter().enumerate() {
        if i > start && i < end {
            let trimmed = line.trim();
            if trimmed.starts_with("depends_on:") {
                in_depends_on = true;
                has_depends_on = true;
                new_lines.push(line.clone());
                continue;
            }

            if in_depends_on {
                if trimmed.starts_with("- id:") {
                    let id_part = trimmed.trim_start_matches("- id:").trim();
                    if matches_task_id(id_part, dep_id) || matches_task_id(dep_id, id_part) {
                        skip_next = true;
                        continue;
                    }
                }

                if skip_next && trimmed.starts_with("type:") {
                    skip_next = false;
                    continue;
                }

                if !line.starts_with("  ") && !line.starts_with("    ") {
                    in_depends_on = false;
                }
            }
        }

        new_lines.push(line.clone());
    }

    if has_depends_on {
        let mut found = false;
        for (i, line) in new_lines.iter().enumerate() {
            if i > start && i < end && line.trim_start().starts_with("- id:") {
                found = true;
                break;
            }
        }

        if !found {
            new_lines.retain(|line| line.trim() != "depends_on:");
        }
    }

    let mut new_content = new_lines.join("\n");
    if had_trailing_newline {
        new_content.push('\n');
    }

    crate::crank_io::write_string(task_path, new_content)
        .with_context(|| format!("failed to write task file: {}", task_path.display()))?;

    Ok(())
}


pub fn open_editor(path: &Path) -> Result<()> {
    let mut cmd = editor_command(path)?;
    cmd.status()
        .with_context(|| format!("failed to open editor for {}", path.display()))?;
    Ok(())
}

fn editor_command(path: &Path) -> Result<std::process::Command> {
    let editor = std::env::var("EDITOR").context("$EDITOR is not set")?;
    let editor = editor.trim();
    if editor.is_empty() {
        return Err(anyhow!("$EDITOR is empty"));
    }

    let mut parts = editor.split_whitespace();
    let binary = parts.next().ok_or_else(|| anyhow!("$EDITOR is empty"))?;

    let mut cmd = std::process::Command::new(binary);
    cmd.args(parts);
    cmd.arg(path);
    cmd.stdin(std::process::Stdio::inherit());
    cmd.stdout(std::process::Stdio::inherit());
    cmd.stderr(std::process::Stdio::inherit());

    Ok(cmd)
}

pub fn generate_id() -> String {
    let value: u16 = random();
    format!("{value:04x}")
}

fn find_frontmatter_bounds(lines: &[String]) -> Option<(usize, usize)> {
    let mut start = None;
    for (i, line) in lines.iter().enumerate() {
        if line.trim() == "---" {
            if start.is_none() {
                start = Some(i);
            } else {
                return start.map(|start| (start, i));
            }
        }
    }
    None
}

pub fn write_task_file(path: &Path, content: &str) -> Result<()> {
    let mut file = fs::File::create(path)
        .with_context(|| format!("failed to create task file: {}", path.display()))?;
    file.write_all(content.as_bytes())
        .with_context(|| format!("failed to write task file: {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use tempfile::tempdir;

    #[test]
    fn parse_task_reads_frontmatter() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("abcd.md");
        let content = r#"---
title: Test Task
priority: 3
status: open
supervision: unsupervised
workflow: review-flow
step_id: implement
created: 2024-12-30
---

## Intent

## Spec

### Run
```bash
crank merge
```
"#;
        crate::crank_io::write_string(&path, content).unwrap();

        let task = parse_task(&path).unwrap();
        assert_eq!(task.id, "abcd");
        assert_eq!(task.title, "Test Task");
        assert_eq!(task.priority, 3);
        assert_eq!(task.status, "open");
        assert_eq!(task.supervision, SupervisionMode::Unsupervised);
        assert_eq!(task.workflow.as_deref(), Some("review-flow"));
        assert_eq!(task.step_id.as_deref(), Some("implement"));
        assert_eq!(task.run.as_deref(), Some("crank merge"));
        assert_eq!(task.coding_agent, "opencode");
        assert_eq!(
            task.created,
            Some(NaiveDate::from_ymd_opt(2024, 12, 30).unwrap())
        );
    }

    #[test]
    fn parse_task_falls_back_to_heading() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("abcd.md");
        let content = r#"---
priority: 3
status: open
supervision: supervised
created: 2024-12-30
---

# Heading Title
"#;
        crate::crank_io::write_string(&path, content).unwrap();

        let task = parse_task(&path).unwrap();
        assert_eq!(task.title, "Heading Title");
    }

    #[test]
    fn toggle_task_status_preserves_suffix() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("abcd.md");
        let content = r#"---
title: Test Task
priority: 3
status: closed #33
supervision: unsupervised
created: 2024-12-30
---
"#;
        crate::crank_io::write_string(&path, content).unwrap();

        let mut task = parse_task(&path).unwrap();
        toggle_task_status(&mut task).unwrap();

        let updated = crate::crank_io::read_to_string(&path).unwrap();
        assert!(updated.contains("status: open #33"));
    }

    #[test]
    fn dependency_helpers_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("abcd.md");
        let content = r#"---
title: Test Task
priority: 3
status: open
supervision: unsupervised
created: 2024-12-30
---
"#;
        crate::crank_io::write_string(&path, content).unwrap();

        let dep = Dependency {
            id: "beef".to_string(),
            dep_type: "blocks".to_string(),
        };
        add_dependency_to_file(&path, &dep).unwrap();
        let updated = crate::crank_io::read_to_string(&path).unwrap();
        assert!(updated.contains("depends_on:"));
        assert!(updated.contains("- id: beef"));

        remove_dependency_from_file(&path, "beef").unwrap();
        let updated = crate::crank_io::read_to_string(&path).unwrap();
        assert!(!updated.contains("depends_on:"));
    }
}
