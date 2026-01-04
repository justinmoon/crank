use std::path::PathBuf;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

pub const TASK_STATUS_OPEN: &str = "open";
pub const TASK_STATUS_IN_PROGRESS: &str = "in_progress";
pub const TASK_STATUS_CLOSED: &str = "closed";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Dependency {
    pub id: String,
    #[serde(rename = "type")]
    pub dep_type: String,
}

impl Dependency {
    pub fn is_blocking(&self) -> bool {
        self.dep_type == "blocks"
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Task {
    pub app: String,
    pub priority: i32,
    pub status: String,
    pub title: String,
    pub depends_on: Vec<Dependency>,
    pub workflow: Option<String>,
    pub step_id: Option<String>,
    pub run: Option<String>,
    pub coding_agent: String,
    pub created: Option<NaiveDate>,
    pub path: PathBuf,
    pub id: String,
}

impl Task {
    pub fn is_closed(&self) -> bool {
        self.status == TASK_STATUS_CLOSED || self.status.starts_with("closed ")
    }

    pub fn blockers<'a>(&self, tasks: &'a [Task]) -> Vec<&'a Task> {
        let mut blockers = Vec::new();
        for dep in &self.depends_on {
            if !dep.is_blocking() {
                continue;
            }
            for other in tasks {
                if matches_task_id(&other.id, &dep.id) {
                    if !other.is_closed() {
                        blockers.push(other);
                    }
                    break;
                }
            }
        }
        blockers
    }
}

pub fn matches_task_id(task_id: &str, dep_id: &str) -> bool {
    task_id == dep_id || task_id.starts_with(dep_id) || dep_id.starts_with(task_id)
}

pub fn normalize_task_id(arg: &str) -> String {
    let trimmed = arg.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let base = std::path::Path::new(trimmed)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(trimmed);
    base.strip_suffix(".md").unwrap_or(base).to_string()
}

pub fn sort_tasks(tasks: &mut [Task]) {
    tasks.sort_by(|a, b| {
        if a.status != b.status {
            if a.status == TASK_STATUS_OPEN {
                return std::cmp::Ordering::Less;
            }
            if b.status == TASK_STATUS_OPEN {
                return std::cmp::Ordering::Greater;
            }
        }
        b.priority.cmp(&a.priority)
    });
}
