use std::io::Write;
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde_json::Value;

use crate::orchestrator::logging;
use crate::task::model::SupervisionMode;
use crate::task::model::Task;

const OPENCODE_HOST: &str = "http://127.0.0.1";
pub struct OpencodeServer {
    pub url: String,
    client: Client,
    child: Child,
}

impl OpencodeServer {
    pub async fn start(id: u16, worktree_path: &Path) -> Result<Self> {
        let port = allocate_port()?;
        let url = format!("{OPENCODE_HOST}:{port}");
        let log_name = format!("opencode-{id}.log");
        let log_file = logging::log_file(&log_name)?;
        let log_file_err = log_file.try_clone()?;

        let mut cmd = Command::new("opencode");
        cmd.args([
            "serve",
            "--port",
            &port.to_string(),
            "--hostname",
            "127.0.0.1",
        ])
        .current_dir(worktree_path)
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_file_err));
        let child = cmd.spawn().context("failed to start opencode server")?;

        let client = Client::new();
        let status_url = format!("{url}/session");
        let start = Instant::now();
        loop {
            if start.elapsed() > Duration::from_secs(10) {
                return Err(anyhow!("opencode server did not start on {url}"));
            }
            if client.get(&status_url).send().await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        Ok(Self { url, client, child })
    }
}

impl Drop for OpencodeServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn spawn_attach(
    server: &OpencodeServer,
    session_id: &str,
    worktree_path: &Path,
    mode: SupervisionMode,
) -> Result<Child> {
    let mut cmd = Command::new("opencode");
    cmd.arg("attach")
        .arg(&server.url)
        .arg("--session")
        .arg(session_id)
        .arg("--dir")
        .arg(worktree_path)
        .env("CRANK_SUPERVISION", mode.as_str())
        .current_dir(worktree_path);
    cmd.spawn().context("failed to launch opencode attach")
}

pub async fn create_session(
    server: &OpencodeServer,
    worktree_path: &Path,
    task: &Task,
) -> Result<String> {
    let url = format!("{}/session", server.url);
    let title = format!("[{}] {}", task.id, task.title);
    let payload = serde_json::json!({
        "directory": worktree_path.to_string_lossy(),
        "title": title,
    });

    let response: Value = server
        .client
        .post(url)
        .json(&payload)
        .send()
        .await?
        .json()
        .await?;
    let id = response
        .get("id")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow!("opencode session id missing"))?;
    Ok(id.to_string())
}

pub async fn send_prompt(server: &OpencodeServer, session_id: &str, prompt: &str) -> Result<()> {
    let mut cmd = Command::new("opencode");
    cmd.args(["run", "--attach", &server.url, "--session", session_id])
        .arg(prompt)
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    if let Ok(model) =
        std::env::var("CRANK_OPENCODE_MODEL").or_else(|_| std::env::var("OPENCODE_MODEL"))
    {
        if !model.trim().is_empty() {
            cmd.args(["--model", model.trim()]);
        }
    }

    let mut child = cmd.spawn().context("failed to send opencode prompt")?;
    let session = session_id.to_string();
    std::thread::spawn(move || {
        if let Ok(status) = child.wait() {
            if !status.success() {
                if let Ok(mut file) = logging::log_file(&format!("opencode-run-{session}.log")) {
                    let _ = writeln!(
                        file,
                        "opencode run failed for {session}: exit {}",
                        status.code().unwrap_or(1)
                    );
                }
            }
        }
    });
    Ok(())
}

pub async fn is_idle(server: &OpencodeServer, session_id: &str) -> Result<bool> {
    let url = format!("{}/session/status", server.url);
    let response = server.client.get(url).send().await?;
    let value: Value = match response.json().await {
        Ok(value) => value,
        Err(_) => return Ok(false),
    };

    if value.as_object().map(|obj| obj.is_empty()).unwrap_or(false) {
        return Ok(true);
    }

    if let Some(status) = value.get("status").and_then(|v| v.as_str()) {
        return Ok(status.eq_ignore_ascii_case("idle"));
    }
    if let Some(idle) = value.get("idle").and_then(|v| v.as_bool()) {
        return Ok(idle);
    }
    if let Some(session) = value.get(session_id) {
        if let Some(status) = session.get("status").and_then(|v| v.as_str()) {
            return Ok(status.eq_ignore_ascii_case("idle"));
        }
    }
    if let Some(sessions) = value.get("sessions").and_then(|v| v.as_object()) {
        if let Some(session) = sessions.get(session_id) {
            if let Some(status) = session.get("status").and_then(|v| v.as_str()) {
                return Ok(status.eq_ignore_ascii_case("idle"));
            }
        }
    }
    Ok(false)
}

fn allocate_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("failed to bind ephemeral port")?;
    let port = listener
        .local_addr()
        .context("failed to read local addr")?
        .port();
    Ok(port)
}
