use std::cmp::Ordering;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use chrono::NaiveDate;

use crate::task::model::{Task, TASK_STATUS_IN_PROGRESS, TASK_STATUS_OPEN};
use crate::task::store;

const CLAIM_LOCK_TIMEOUT: Duration = Duration::from_secs(30);
const CLAIM_LOCK_BACKOFF: Duration = Duration::from_millis(200);

pub fn claim_next_task(
    git_root: &Path,
    repo_root: &Path,
    project: Option<&str>,
) -> Result<Option<Task>> {
    let _lock = acquire_claim_lock(repo_root)?;

    let tasks = store::load_tasks(git_root)?;
    if tasks.is_empty() {
        return Ok(None);
    }

    let mut claimable = Vec::new();
    for task in &tasks {
        if task.status != TASK_STATUS_OPEN {
            continue;
        }
        if let Some(project) = project {
            if task.app != project {
                continue;
            }
        }
        if !task.blockers(&tasks).is_empty() {
            continue;
        }
        claimable.push(task.clone());
    }

    if claimable.is_empty() {
        return Ok(None);
    }

    claimable.sort_by(|a, b| {
        let priority_cmp = b.priority.cmp(&a.priority);
        if priority_cmp != Ordering::Equal {
            return priority_cmp;
        }
        let a_created = a.created.unwrap_or_else(max_date);
        let b_created = b.created.unwrap_or_else(max_date);
        let created_cmp = a_created.cmp(&b_created);
        if created_cmp != Ordering::Equal {
            return created_cmp;
        }
        a.id.cmp(&b.id)
    });

    let mut selected = claimable
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no claimable tasks"))?;

    store::update_task_status(&selected.path, TASK_STATUS_IN_PROGRESS)
        .context("failed to mark task in progress")?;
    selected.status = TASK_STATUS_IN_PROGRESS.to_string();

    Ok(Some(selected))
}

fn max_date() -> NaiveDate {
    NaiveDate::from_ymd_opt(9999, 12, 31).unwrap()
}

fn acquire_claim_lock(repo_root: &Path) -> Result<ClaimLock> {
    let crank_dir = dirs::home_dir()
        .context("Could not find home directory")?
        .join(".crank")
        .join("locks")
        .join(repo_id(repo_root));

    std::fs::create_dir_all(&crank_dir)
        .with_context(|| format!("failed to create crank lock dir: {}", crank_dir.display()))?;

    let lock_dir = crank_dir.join("task-claim.lock.d");
    let start = Instant::now();

    loop {
        match std::fs::create_dir(&lock_dir) {
            Ok(()) => return Ok(ClaimLock { path: lock_dir }),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if start.elapsed() > CLAIM_LOCK_TIMEOUT {
                    return Err(anyhow!("timeout waiting for task claim lock"));
                }
                thread::sleep(CLAIM_LOCK_BACKOFF);
            }
            Err(err) => return Err(err.into()),
        }
    }
}

fn repo_id(repo_root: &Path) -> String {
    let name = repo_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("repo");
    let mut hasher = DefaultHasher::new();
    repo_root.to_string_lossy().hash(&mut hasher);
    let hash = hasher.finish();
    format!("{name}-{hash:016x}")
}

struct ClaimLock {
    path: PathBuf,
}

impl Drop for ClaimLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use tempfile::tempdir;

    fn write_task(
        dir: &Path,
        id: &str,
        priority: i32,
        status: &str,
        created: &str,
        depends_on: &str,
    ) -> PathBuf {
        let path = dir.join(format!("{id}.md"));
        let content = format!(
            "---\napp: crank\ntitle: Task {id}\npriority: {priority}\nstatus: {status}\ncoding_agent: opencode\ncreated: {created}\n{depends_on}---\n\n## Intent\n"
        );
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn claim_next_task_respects_priority_and_blockers() {
        let dir = tempdir().unwrap();
        let git_root = dir.path();
        let repo_root = dir.path();
        let issues = git_root.join(".issues");
        fs::create_dir_all(&issues).unwrap();

        write_task(
            &issues,
            "a111",
            3,
            "open",
            "2024-12-30",
            "depends_on:\n  - id: b222\n    type: blocks\n",
        );
        write_task(&issues, "b222", 5, "open", "2024-12-29", "");
        write_task(&issues, "c333", 4, "open", "2024-12-28", "");

        let claimed = claim_next_task(git_root, repo_root, None).unwrap().unwrap();
        assert_eq!(claimed.id, "b222");

        let claimed = claim_next_task(git_root, repo_root, None).unwrap().unwrap();
        assert_eq!(claimed.id, "c333");
    }

    #[test]
    fn claim_next_task_honors_project_filter() {
        let dir = tempdir().unwrap();
        let git_root = dir.path();
        let repo_root = dir.path();
        let issues = git_root.join(".issues");
        fs::create_dir_all(&issues).unwrap();

        write_task(&issues, "a111", 3, "open", "2024-12-30", "");
        let other = issues.join("b222.md");
        let content = "---\napp: other\ntitle: Task b222\npriority: 4\nstatus: open\ncoding_agent: opencode\ncreated: 2024-12-29\n---\n";
        fs::write(&other, content).unwrap();

        let claimed = claim_next_task(git_root, repo_root, Some("crank"))
            .unwrap()
            .unwrap();
        assert_eq!(claimed.id, "a111");
    }

    #[test]
    fn claim_next_task_uses_fifo_for_same_priority() {
        let dir = tempdir().unwrap();
        let git_root = dir.path();
        let repo_root = dir.path();
        let issues = git_root.join(".issues");
        fs::create_dir_all(&issues).unwrap();

        write_task(&issues, "a111", 3, "open", "2024-12-30", "");
        write_task(&issues, "b222", 3, "open", "2024-12-29", "");

        let claimed = claim_next_task(git_root, repo_root, None).unwrap().unwrap();
        assert_eq!(claimed.id, "b222");
    }
}
