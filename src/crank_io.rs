use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub fn repo_crank_dir(git_root: &Path) -> PathBuf {
    git_root.join(".crank")
}

#[allow(dead_code)]
pub fn repo_workflows_dir(git_root: &Path) -> PathBuf {
    repo_crank_dir(git_root).join("workflows")
}

pub fn user_crank_dir_from(home_dir: &Path) -> PathBuf {
    home_dir.join(".crank")
}

#[allow(dead_code)]
pub fn user_crank_dir() -> Result<PathBuf> {
    let home_dir = dirs::home_dir().context("Could not find home directory")?;
    Ok(user_crank_dir_from(&home_dir))
}

#[allow(dead_code)]
pub fn user_workflows_dir_opt() -> Option<PathBuf> {
    dirs::home_dir().map(|dir| user_crank_dir_from(&dir).join("workflows"))
}

pub fn ensure_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("failed to create directory: {}", path.display()))
}

pub fn read_to_string(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .with_context(|| format!("failed to read file: {}", path.display()))
}

#[allow(dead_code)]
pub fn write_string(path: &Path, content: impl AsRef<str>) -> Result<()> {
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    std::fs::write(path, content.as_ref().as_bytes())
        .with_context(|| format!("failed to write file: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::tempdir;

    #[test]
    fn repo_dirs_are_under_git_root() {
        let dir = tempdir().unwrap();
        let git_root = dir.path();

        assert_eq!(repo_crank_dir(git_root), git_root.join(".crank"));
        assert_eq!(
            repo_workflows_dir(git_root),
            git_root.join(".crank").join("workflows")
        );
    }

    #[test]
    fn user_dirs_are_under_home_dir() {
        let dir = tempdir().unwrap();
        let home = dir.path();

        assert_eq!(user_crank_dir_from(home), home.join(".crank"));
    }

    #[test]
    fn ensure_dir_creates_missing_directories() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("a").join("b");

        ensure_dir(&nested).unwrap();
        assert!(nested.exists());
    }

    #[test]
    fn ensure_dir_is_idempotent() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("a").join("b");

        ensure_dir(&nested).unwrap();
        ensure_dir(&nested).unwrap();
        assert!(nested.exists());
    }

    #[test]
    fn write_and_read_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("file.txt");

        write_string(&path, "hello\n").unwrap();
        let content = read_to_string(&path).unwrap();
        assert_eq!(content, "hello\n");
    }
}
