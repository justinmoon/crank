use std::path::Path;

use anyhow::{anyhow, Result};
use chrono::Local;

use crate::task::model::{Dependency, SupervisionMode};
use crate::task::prompts;
use crate::task::store;

pub fn parse_deps_flag(deps: &str) -> Result<Vec<Dependency>> {
    let mut result = Vec::new();
    for part in deps.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((dep_type, dep_id)) = part.split_once(':') else {
            return Err(anyhow!(
                "invalid dependency format: {part} (expected type:id)"
            ));
        };
        if dep_type.trim().is_empty() || dep_id.trim().is_empty() {
            return Err(anyhow!(
                "invalid dependency format: {part} (expected type:id)"
            ));
        }
        result.push(Dependency {
            id: dep_id.trim().to_string(),
            dep_type: dep_type.trim().to_string(),
        });
    }

    Ok(result)
}

pub fn create_task_file(
    git_root: &Path,
    title: Option<String>,
    priority: Option<i32>,
    supervision: Option<SupervisionMode>,
    deps: &[Dependency],
) -> Result<()> {
    let (title, priority, supervision) =
        prompts::prompt_task_fields(title, priority, supervision)?;
    store::create_task_file(git_root, &title, priority, supervision, deps)?;
    Ok(())
}

pub fn create_task_interactive(
    git_root: &Path,
    title: Option<String>,
    priority: Option<i32>,
    supervision: Option<SupervisionMode>,
) -> Result<()> {
    let (title, priority, supervision) =
        prompts::prompt_task_fields(title, priority, supervision)?;

    let id = store::generate_id();
    let date = Local::now().format("%Y-%m-%d").to_string();
    let filename = format!("{id}.md");
    let tasks_dir = crate::crank_io::repo_crank_dir(git_root);
    let task_path = tasks_dir.join(&filename);
    let rel_task_path = format!(".crank/{filename}");

    crate::crank_io::ensure_dir(&tasks_dir).map_err(|err| {
        anyhow!(
            "failed to create tasks directory: {} ({})",
            tasks_dir.display(),
            err
        )
    })?;

    let content = store::task_template(&title, priority, supervision, &date, &[]);
    store::write_task_file(&task_path, &content)?;

    store::open_editor(&task_path)?;

    let task = store::parse_task(&task_path)
        .map_err(|err| anyhow!("failed to parse {rel_task_path}: {err}"))?;

    if task.title.trim().is_empty() {
        return Err(anyhow!("title is required; edit {rel_task_path}"));
    }
    if task.priority < 1 || task.priority > 5 {
        return Err(anyhow!("priority must be 1-5; edit {rel_task_path}"));
    }

    println!("Created: {rel_task_path}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_deps_flag;

    #[test]
    fn parse_deps_flag_parses_multiple() {
        let deps = parse_deps_flag("blocks:abcd, parent:ef01").unwrap();
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0].id, "abcd");
        assert_eq!(deps[0].dep_type, "blocks");
        assert_eq!(deps[1].id, "ef01");
        assert_eq!(deps[1].dep_type, "parent");
    }

    #[test]
    fn parse_deps_flag_rejects_invalid() {
        assert!(parse_deps_flag("invalid").is_err());
        assert!(parse_deps_flag("blocks:").is_err());
    }
}
