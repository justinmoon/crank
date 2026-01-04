use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Local;

pub struct Logger {
    path: PathBuf,
}

impl Logger {
    pub fn new(name: &str) -> Result<Self> {
        let dir = log_dir()?;
        let filename = format!("{name}.log");
        Ok(Self {
            path: dir.join(filename),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn log(&self, level: &str, message: &str) -> Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("failed to open log file: {}", self.path.display()))?;
        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
        writeln!(file, "{} [{}] {}", timestamp, level, message)?;
        Ok(())
    }
}

pub fn log_file(name: &str) -> Result<std::fs::File> {
    let dir = log_dir()?;
    let path = dir.join(name);
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("failed to open log file: {}", path.display()))
}

pub fn log_dir() -> Result<PathBuf> {
    let dir = crate::crank_io::user_crank_dir()?.join("logs");
    crate::crank_io::ensure_dir(&dir)
        .with_context(|| format!("failed to create log dir: {}", dir.display()))?;
    Ok(dir)
}
