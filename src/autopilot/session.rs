use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::orchestrator::logging;
use crate::task::git;
use crate::task::model::SupervisionMode;

pub struct SessionSpec {
    pub concurrency: u16,
    pub git_root: PathBuf,
    pub session_name: String,
    mode: SupervisionMode,
    worker_bin: PathBuf,
    log_dir: PathBuf,
}

impl SessionSpec {
    pub fn new(concurrency: u16, mode: SupervisionMode) -> Result<Self> {
        let git_root = git::git_root()?;
        let session_name = "crank".to_string();
        let worker_bin = std::env::current_exe().context("failed to locate crank binary")?;
        let log_dir = logging::log_dir()?;

        Ok(Self {
            concurrency,
            git_root,
            session_name,
            mode,
            worker_bin,
            log_dir,
        })
    }

    pub fn worker_command(&self, id: u16) -> Vec<String> {
        vec![
            self.worker_bin.to_string_lossy().to_string(),
            "worker".to_string(),
            "--id".to_string(),
            id.to_string(),
            "--mode".to_string(),
            self.mode.as_str().to_string(),
        ]
    }

    pub fn log_tail_args(&self) -> Vec<String> {
        let mut args = vec![
            "tail".to_string(),
            "-n".to_string(),
            "200".to_string(),
            "-F".to_string(),
        ];
        for id in 1..=self.concurrency {
            args.push(
                self.log_dir
                    .join(format!("worker-{id}.log"))
                    .to_string_lossy()
                    .to_string(),
            );
            args.push(
                self.log_dir
                    .join(format!("opencode-{id}.log"))
                    .to_string_lossy()
                    .to_string(),
            );
        }
        args
    }
}
