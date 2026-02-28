use std::fs;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, Context, Result};

use crate::orchestrator::session::SessionSpec;
use crate::task::model::SupervisionMode;

pub fn run_zellij(concurrency: u16, mode: SupervisionMode) -> Result<()> {
    if !std::env::var("ZELLIJ_SESSION_NAME")
        .unwrap_or_default()
        .is_empty()
    {
        return Err(anyhow!("crank zellij must be run outside zellij"));
    }
    if concurrency == 0 {
        return Err(anyhow!("concurrency must be at least 1"));
    }

    let spec = SessionSpec::new(concurrency, mode)?;

    if session_exists(&spec.session_name)? {
        return Err(anyhow!(
            "zellij session already exists: {}",
            spec.session_name
        ));
    }

    let layout = build_layout(&spec)?;
    let layout_path = write_layout(&layout)?;

    let status = Command::new("zellij")
        .arg("--layout")
        .arg(&layout_path)
        .arg("attach")
        .arg("--create-background")
        .arg(&spec.session_name)
        .status()
        .context("failed to create zellij session")?;
    let _ = fs::remove_file(&layout_path);

    if !status.success() {
        return Err(anyhow!("failed to create zellij session"));
    }

    println!("Created zellij session: {}", spec.session_name);
    println!("Attach with: zellij attach {}", spec.session_name);
    Ok(())
}

fn session_exists(name: &str) -> Result<bool> {
    let output = Command::new("zellij")
        .arg("list-sessions")
        .output()
        .context("failed to list zellij sessions")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("No active zellij sessions found") {
            return Ok(false);
        }
        return Err(anyhow!("zellij list-sessions failed: {stderr}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if line.contains(name) && !line.contains("(EXITED") {
            return Ok(true);
        }
    }
    Ok(false)
}

fn build_layout(spec: &SessionSpec) -> Result<String> {
    let mut layout = String::from("layout {\n");
    layout.push_str("    tab name=\"workers\" {\n");

    for id in 1..=spec.concurrency {
        let cmd = spec.worker_command(id);
        let command = cmd
            .first()
            .ok_or_else(|| anyhow!("worker command missing"))?;
        let args = &cmd[1..];

        layout.push_str(&format!(
            "        pane name=\"worker-{id}\" command=\"{}\" cwd=\"{}\" {{\n",
            escape_kdl_string(command),
            escape_kdl_string(&spec.git_root.to_string_lossy()),
        ));
        layout.push_str("            args");
        for arg in args {
            layout.push(' ');
            layout.push('"');
            layout.push_str(&escape_kdl_string(arg));
            layout.push('"');
        }
        layout.push_str("\n        }\n");
    }

    layout.push_str("    }\n");
    layout.push_str("    tab name=\"logs\" {\n");

    let log_args = spec.log_tail_args();
    let log_command = log_args
        .first()
        .ok_or_else(|| anyhow!("log command missing"))?;
    let log_args = &log_args[1..];
    layout.push_str(&format!(
        "        pane name=\"logs\" command=\"{}\" cwd=\"{}\" {{\n",
        escape_kdl_string(log_command),
        escape_kdl_string(&spec.git_root.to_string_lossy()),
    ));
    layout.push_str("            args");
    for arg in log_args {
        layout.push(' ');
        layout.push('"');
        layout.push_str(&escape_kdl_string(arg));
        layout.push('"');
    }
    layout.push_str("\n        }\n");
    layout.push_str("    }\n");
    layout.push_str("}\n");

    Ok(layout)
}

fn write_layout(content: &str) -> Result<PathBuf> {
    let filename = format!("crank-zellij-{}.kdl", rand::random::<u64>());
    let path = std::env::temp_dir().join(filename);
    fs::write(&path, content)
        .with_context(|| format!("failed to write zellij layout: {}", path.display()))?;
    Ok(path)
}

fn escape_kdl_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
