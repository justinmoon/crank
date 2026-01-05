use std::path::PathBuf;

use chrono::NaiveDate;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

pub const TASK_STATUS_OPEN: &str = "open";
pub const TASK_STATUS_IN_PROGRESS: &str = "in_progress";
#[allow(dead_code)]
pub const TASK_STATUS_NEEDS_HUMAN: &str = "needs_human";
pub const TASK_STATUS_CLOSED: &str = "closed";

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum SupervisionMode {
    Supervised,
    Unsupervised,
}

impl SupervisionMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            SupervisionMode::Supervised => "supervised",
            SupervisionMode::Unsupervised => "unsupervised",
        }
    }
}

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
    pub priority: i32,
    pub status: String,
    pub supervision: SupervisionMode,
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
    let task = normalize_task_id(task_id);
    let dep = normalize_task_id(dep_id);
    !task.is_empty() && task == dep
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
