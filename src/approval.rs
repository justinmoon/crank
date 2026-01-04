use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingMerge {
    pub id: String,
    pub branch: String,
    pub base_branch: String,
    pub worktree_path: String,
    pub target_repo: String,
    pub created_at: i64,
    pub pid: u32,
    pub status: String, // "pending", "approved", "rejected"
}

fn get_pending_dir() -> Result<PathBuf> {
    let home_dir =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?;
    get_pending_dir_with_home(&home_dir)
}

fn get_pending_dir_with_home(home_dir: &Path) -> Result<PathBuf> {
    let crank_dir = crate::crank_io::user_crank_dir_from(home_dir).join("pending");
    crate::crank_io::ensure_dir(&crank_dir)?;
    Ok(crank_dir)
}

fn generate_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    format!("{:x}", timestamp % 0xFFFFFF)
}

/// Create a pending merge entry
pub async fn create_pending(
    branch: &str,
    base_branch: &str,
    worktree_path: &Path,
    target_repo: &Path,
) -> Result<PendingMerge> {
    let pending_dir = get_pending_dir()?;

    let pending = PendingMerge {
        id: generate_id(),
        branch: branch.to_string(),
        base_branch: base_branch.to_string(),
        worktree_path: worktree_path.to_string_lossy().to_string(),
        target_repo: target_repo.to_string_lossy().to_string(),
        created_at: chrono::Utc::now().timestamp(),
        pid: std::process::id(),
        status: "pending".to_string(),
    };

    let file_path = pending_dir.join(format!("{}.json", pending.id));
    let content = serde_json::to_string_pretty(&pending)?;
    crate::crank_io::write_string(&file_path, &content)?;

    Ok(pending)
}


/// Get a pending merge by ID or branch name
pub async fn get_pending(id_or_branch: &str) -> Result<Option<PendingMerge>> {
    let pending_dir = get_pending_dir()?;

    for entry in std::fs::read_dir(&pending_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().map(|e| e == "json").unwrap_or(false) {
            let content = crate::crank_io::read_to_string(&path)?;
            let pending: PendingMerge = serde_json::from_str(&content)?;

            if pending.id == id_or_branch || pending.branch == id_or_branch {
                return Ok(Some(pending));
            }
        }
    }

    Ok(None)
}

/// List all pending merges
pub async fn list_pending() -> Result<Vec<PendingMerge>> {
    let pending_dir = get_pending_dir()?;
    let mut pending = vec![];

    for entry in std::fs::read_dir(&pending_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().map(|e| e == "json").unwrap_or(false) {
            let content = crate::crank_io::read_to_string(&path)?;
            if let Ok(p) = serde_json::from_str::<PendingMerge>(&content) {
                pending.push(p);
            }
        }
    }

    // Sort by creation time, newest first
    pending.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(pending)
}

/// Update pending status
async fn update_status(id: &str, status: &str) -> Result<Option<PendingMerge>> {
    let pending_dir = get_pending_dir()?;
    let file_path = pending_dir.join(format!("{}.json", id));

    if !file_path.exists() {
        return Ok(None);
    }

    let content = crate::crank_io::read_to_string(&file_path)?;
    let mut pending: PendingMerge = serde_json::from_str(&content)?;
    pending.status = status.to_string();

    crate::crank_io::write_string(&file_path, &serde_json::to_string_pretty(&pending)?)?;
    Ok(Some(pending))
}

/// Remove a pending merge entry
pub async fn remove_pending(id: &str) -> Result<()> {
    let pending_dir = get_pending_dir()?;
    let file_path = pending_dir.join(format!("{}.json", id));

    if file_path.exists() {
        std::fs::remove_file(file_path)?;
    }

    Ok(())
}

/// Check pending status
async fn check_status(id: &str) -> Result<Option<String>> {
    let pending_dir = get_pending_dir()?;
    let file_path = pending_dir.join(format!("{}.json", id));

    if !file_path.exists() {
        return Ok(None);
    }

    let content = crate::crank_io::read_to_string(&file_path)?;
    let pending: PendingMerge = serde_json::from_str(&content)?;
    Ok(Some(pending.status))
}

/// Send a desktop notification (macOS)
pub async fn send_notification(title: &str, message: &str) -> Result<()> {
    let script = format!(
        "display notification \"{}\" with title \"{}\" sound name \"default\"",
        message.replace('"', "\\\""),
        title.replace('"', "\\\""),
    );

    let _ = Command::new("osascript")
        .args(["-e", &script])
        .output()
        .await;

    Ok(())
}

/// Wait for approval with notification loop
pub async fn wait_for_approval(pending: &PendingMerge, interval_ms: u64) -> bool {
    let approve_cmd = format!("crank approve {}", pending.branch);

    loop {
        // Send notification
        let _ = send_notification(
            &format!("Merge ready: {}", pending.branch),
            &format!("Run: {}", approve_cmd),
        )
        .await;

        // Wait
        tokio::time::sleep(Duration::from_millis(interval_ms)).await;

        // Check status
        match check_status(&pending.id).await {
            Ok(Some(status)) => {
                if status == "approved" {
                    return true;
                }
                if status == "rejected" {
                    return false;
                }
            }
            Ok(None) => return false, // File deleted
            Err(_) => continue,
        }
    }
}

/// Approve command
pub async fn approve_command(id_or_branch: Option<&str>) -> Result<()> {
    let id = match id_or_branch {
        Some(id) => id.to_string(),
        None => {
            let pending = list_pending().await?;
            let actually_pending: Vec<_> = pending
                .into_iter()
                .filter(|p| p.status == "pending")
                .collect();

            if actually_pending.is_empty() {
                println!(r#"{{"status":"error","message":"No pending merges"}}"#);
                std::process::exit(1);
            }

            if actually_pending.len() > 1 {
                let pending_info: Vec<_> = actually_pending
                    .iter()
                    .map(|p| {
                        serde_json::json!({
                            "branch": p.branch,
                            "id": p.id,
                            "age_min": (chrono::Utc::now().timestamp() - p.created_at) / 60
                        })
                    })
                    .collect();

                println!(
                    "{}",
                    serde_json::json!({
                        "status": "error",
                        "message": "Multiple pending merges. Specify which one:",
                        "pending": pending_info
                    })
                );
                std::process::exit(1);
            }

            actually_pending[0].id.clone()
        }
    };

    let pending = get_pending(&id).await?;

    match pending {
        None => {
            println!(
                r#"{{"status":"error","message":"No pending merge found: {}"}}"#,
                id
            );
            std::process::exit(1);
        }
        Some(p) if p.status != "pending" => {
            println!(
                r#"{{"status":"error","message":"Merge already {}"}}"#,
                p.status
            );
            std::process::exit(1);
        }
        Some(p) => {
            update_status(&p.id, "approved").await?;
            println!(
                "{}",
                serde_json::json!({
                    "status": "approved",
                    "branch": p.branch,
                    "id": p.id
                })
            );
        }
    }

    Ok(())
}

/// Reject command
pub async fn reject_command(id_or_branch: Option<&str>) -> Result<()> {
    let id = match id_or_branch {
        Some(id) => id.to_string(),
        None => {
            let pending = list_pending().await?;
            let actually_pending: Vec<_> = pending
                .into_iter()
                .filter(|p| p.status == "pending")
                .collect();

            if actually_pending.is_empty() {
                println!(r#"{{"status":"error","message":"No pending merges"}}"#);
                std::process::exit(1);
            }

            if actually_pending.len() > 1 {
                let pending_info: Vec<_> = actually_pending
                    .iter()
                    .map(|p| {
                        serde_json::json!({
                            "branch": p.branch,
                            "id": p.id,
                            "age_min": (chrono::Utc::now().timestamp() - p.created_at) / 60
                        })
                    })
                    .collect();

                println!(
                    "{}",
                    serde_json::json!({
                        "status": "error",
                        "message": "Multiple pending merges. Specify which one:",
                        "pending": pending_info
                    })
                );
                std::process::exit(1);
            }

            actually_pending[0].id.clone()
        }
    };

    let pending = get_pending(&id).await?;

    match pending {
        None => {
            println!(
                r#"{{"status":"error","message":"No pending merge found: {}"}}"#,
                id
            );
            std::process::exit(1);
        }
        Some(p) if p.status != "pending" => {
            println!(
                r#"{{"status":"error","message":"Merge already {}"}}"#,
                p.status
            );
            std::process::exit(1);
        }
        Some(p) => {
            update_status(&p.id, "rejected").await?;
            println!(
                "{}",
                serde_json::json!({
                    "status": "rejected",
                    "branch": p.branch,
                    "id": p.id
                })
            );
        }
    }

    Ok(())
}

/// Pending command
pub async fn pending_command() -> Result<()> {
    let pending = list_pending().await?;
    let actually_pending: Vec<_> = pending
        .into_iter()
        .filter(|p| p.status == "pending")
        .collect();

    if actually_pending.is_empty() {
        println!(r#"{"status":"ok","message":"No pending merges","pending":[]}"#);
        return Ok(());
    }

    let pending_info: Vec<_> = actually_pending
        .iter()
        .map(|p| {
            serde_json::json!({
                "branch": p.branch,
                "id": p.id,
                "base_branch": p.base_branch,
                "age_min": (chrono::Utc::now().timestamp() - p.created_at) / 60,
                "approve_cmd": format!("crank approve {}", p.branch),
                "reject_cmd": format!("crank reject {}", p.branch)
            })
        })
        .collect();

    println!(
        "{}",
        serde_json::json!({
            "status": "ok",
            "pending": pending_info
        })
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::tempdir;

    #[test]
    fn pending_dir_is_created_under_home_crank_dir() {
        let dir = tempdir().unwrap();
        let home = dir.path();

        let pending = get_pending_dir_with_home(home).unwrap();
        assert_eq!(pending, home.join(".crank").join("pending"));
        assert!(pending.exists());
    }
}
