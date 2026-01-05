use anyhow::{anyhow, Result};

use crate::task::git;
use crate::task::model::{matches_task_id, normalize_task_id, Dependency, Task};
use crate::task::store;

pub fn run_add(from_id: &str, to_id: &str, dep_type: &str) -> Result<()> {
    let from_id = normalize_task_id(from_id);
    let to_id = normalize_task_id(to_id);

    let git_root = git::git_root()?;
    let tasks = store::load_tasks(&git_root)?;

    let from_task =
        find_task_by_id(&tasks, &from_id).ok_or_else(|| anyhow!("task not found: {from_id}"))?;
    let to_task =
        find_task_by_id(&tasks, &to_id).ok_or_else(|| anyhow!("task not found: {to_id}"))?;

    if from_task.id == to_task.id {
        return Err(anyhow!("task cannot depend on itself"));
    }

    for dep in &from_task.depends_on {
        if matches_task_id(&to_task.id, &dep.id) {
            return Err(anyhow!(
                "dependency already exists: {} -> {}",
                from_task.id,
                to_task.id
            ));
        }
    }

    if would_create_cycle(&tasks, &from_task.id, &to_task.id) {
        return Err(anyhow!("adding this dependency would create a cycle"));
    }

    let new_dep = Dependency {
        id: to_task.id.clone(),
        dep_type: dep_type.to_string(),
    };
    store::add_dependency_to_file(&from_task.path, &new_dep)?;

    println!(
        "Added dependency: {} -> {} ({})",
        from_task.id, to_task.id, dep_type
    );
    Ok(())
}

pub fn run_rm(from_id: &str, to_id: &str) -> Result<()> {
    let from_id = normalize_task_id(from_id);
    let to_id = normalize_task_id(to_id);

    let git_root = git::git_root()?;
    let tasks = store::load_tasks(&git_root)?;

    let from_task =
        find_task_by_id(&tasks, &from_id).ok_or_else(|| anyhow!("task not found: {from_id}"))?;
    let to_task =
        find_task_by_id(&tasks, &to_id).ok_or_else(|| anyhow!("task not found: {to_id}"))?;

    let mut found = false;
    for dep in &from_task.depends_on {
        if matches_task_id(&to_task.id, &dep.id) {
            found = true;
            break;
        }
    }
    if !found {
        return Err(anyhow!(
            "dependency not found: {} -> {}",
            from_task.id,
            to_task.id
        ));
    }

    store::remove_dependency_from_file(&from_task.path, &to_task.id)?;

    println!("Removed dependency: {} -> {}", from_task.id, to_task.id);
    Ok(())
}

pub fn run_tree(id: Option<&str>, reverse: bool) -> Result<()> {
    let git_root = git::git_root()?;
    let tasks = store::load_tasks(&git_root)?;

    if let Some(id) = id {
        let id = normalize_task_id(id);
        let task = find_task_by_id(&tasks, &id).ok_or_else(|| anyhow!("task not found: {id}"))?;
        print_task_tree(task, &tasks, reverse, 0, &mut Vec::new());
        return Ok(());
    }

    for task in &tasks {
        if !task.depends_on.is_empty() || has_reverse_deps(task, &tasks) {
            print_task_tree(task, &tasks, reverse, 0, &mut Vec::new());
            println!();
        }
    }

    Ok(())
}

pub fn run_cycles() -> Result<()> {
    let git_root = git::git_root()?;
    let tasks = store::load_tasks(&git_root)?;

    let cycles = detect_cycles(&tasks);
    if cycles.is_empty() {
        println!("No cycles detected");
        return Ok(());
    }

    println!("Found {} cycle(s):", cycles.len());
    for (index, cycle) in cycles.iter().enumerate() {
        println!("  {}: {}", index + 1, cycle.join(" -> "));
    }

    Ok(())
}

fn has_reverse_deps(task: &Task, all_tasks: &[Task]) -> bool {
    for other in all_tasks {
        for dep in &other.depends_on {
            if matches_task_id(&task.id, &dep.id) {
                return true;
            }
        }
    }
    false
}

fn print_task_tree(
    task: &Task,
    all_tasks: &[Task],
    reverse: bool,
    depth: usize,
    visited: &mut Vec<String>,
) {
    let indent = "  ".repeat(depth);
    let prefix = if depth > 0 { "├── " } else { "" };
    let status_mark = if task.is_closed() { "✓" } else { " " };

    println!(
        "{indent}{prefix}[{status_mark}] {}: {}",
        task.id, task.title
    );

    if visited.contains(&task.id) {
        println!("{indent}  └── (cycle detected)");
        return;
    }
    visited.push(task.id.clone());

    if reverse {
        for other in all_tasks {
            for dep in &other.depends_on {
                if matches_task_id(&task.id, &dep.id) {
                    print_task_tree(other, all_tasks, reverse, depth + 1, visited);
                }
            }
        }
    } else {
        for dep in &task.depends_on {
            if let Some(dep_task) = find_task_by_id(all_tasks, &dep.id) {
                let type_label = if dep.dep_type == "blocks" {
                    String::new()
                } else {
                    format!(" ({})", dep.dep_type)
                };
                println!(
                    "{indent}  ├── {}: {}{type_label}",
                    dep_task.id, dep_task.title
                );
            } else {
                println!("{indent}  ├── {}: (not found)", dep.id);
            }
        }
    }
}

fn find_task_by_id<'a>(tasks: &'a [Task], id: &str) -> Option<&'a Task> {
    tasks.iter().find(|task| matches_task_id(&task.id, id))
}

fn detect_cycles(tasks: &[Task]) -> Vec<Vec<String>> {
    let mut deps = std::collections::HashMap::new();
    for task in tasks {
        for dep in &task.depends_on {
            if dep.is_blocking() {
                deps.entry(task.id.clone())
                    .or_insert_with(Vec::new)
                    .push(dep.id.clone());
            }
        }
    }

    let mut cycles = Vec::new();
    let mut visited = std::collections::HashSet::new();
    let mut stack = std::collections::HashSet::new();
    let mut path = Vec::new();

    fn dfs(
        id: &str,
        deps: &std::collections::HashMap<String, Vec<String>>,
        tasks: &[Task],
        visited: &mut std::collections::HashSet<String>,
        stack: &mut std::collections::HashSet<String>,
        path: &mut Vec<String>,
        cycles: &mut Vec<Vec<String>>,
    ) {
        visited.insert(id.to_string());
        stack.insert(id.to_string());
        path.push(id.to_string());

        if let Some(children) = deps.get(id) {
            for dep_id in children {
                let mut actual_id = dep_id.clone();
                for task in tasks {
                    if matches_task_id(&task.id, dep_id) {
                        actual_id = task.id.clone();
                        break;
                    }
                }

                if !visited.contains(&actual_id) {
                    dfs(&actual_id, deps, tasks, visited, stack, path, cycles);
                } else if stack.contains(&actual_id) {
                    if let Some(pos) = path.iter().position(|p| p == &actual_id) {
                        let mut cycle = path[pos..].to_vec();
                        cycle.push(actual_id.clone());
                        cycles.push(cycle);
                    }
                }
            }
        }

        path.pop();
        stack.remove(id);
    }

    for task in tasks {
        if !visited.contains(&task.id) {
            dfs(
                &task.id,
                &deps,
                tasks,
                &mut visited,
                &mut stack,
                &mut path,
                &mut cycles,
            );
        }
    }

    cycles
}

fn would_create_cycle(tasks: &[Task], from_id: &str, to_id: &str) -> bool {
    let mut visited = std::collections::HashSet::new();

    fn has_path(
        current: &str,
        target: &str,
        tasks: &[Task],
        visited: &mut std::collections::HashSet<String>,
    ) -> bool {
        if matches_task_id(current, target) || matches_task_id(target, current) {
            return true;
        }
        if visited.contains(current) {
            return false;
        }
        visited.insert(current.to_string());

        let task = tasks.iter().find(|task| matches_task_id(&task.id, current));
        let Some(task) = task else {
            return false;
        };

        for dep in &task.depends_on {
            if dep.is_blocking() && has_path(&dep.id, target, tasks, visited) {
                return true;
            }
        }

        false
    }

    has_path(to_id, from_id, tasks, &mut visited)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::model::SupervisionMode;

    fn task(id: &str, deps: Vec<Dependency>) -> Task {
        Task {
            priority: 3,
            status: "open".to_string(),
            supervision: SupervisionMode::Unsupervised,
            title: format!("Task {id}"),
            depends_on: deps,
            workflow: None,
            step_id: None,
            run: None,
            coding_agent: "opencode".to_string(),
            created: None,
            path: std::path::PathBuf::from(format!("{id}.md")),
            id: id.to_string(),
        }
    }

    #[test]
    fn detects_simple_cycle() {
        let tasks = vec![
            task(
                "a",
                vec![Dependency {
                    id: "b".to_string(),
                    dep_type: "blocks".to_string(),
                }],
            ),
            task(
                "b",
                vec![Dependency {
                    id: "a".to_string(),
                    dep_type: "blocks".to_string(),
                }],
            ),
        ];

        let cycles = detect_cycles(&tasks);
        assert!(!cycles.is_empty());
    }

    #[test]
    fn would_create_cycle_flags_existing_path() {
        let tasks = vec![
            task(
                "a",
                vec![Dependency {
                    id: "b".to_string(),
                    dep_type: "blocks".to_string(),
                }],
            ),
            task(
                "b",
                vec![Dependency {
                    id: "c".to_string(),
                    dep_type: "blocks".to_string(),
                }],
            ),
            task("c", Vec::new()),
        ];

        assert!(would_create_cycle(&tasks, "c", "a"));
    }
}
