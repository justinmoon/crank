use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

const HELP_LONG_ABOUT: &str = include_str!("../prompts/help_long_about.md");
const HELP_AFTER_LONG: &str = include_str!("../prompts/help_after_long.md");
const TURN_PROMPT_TEMPLATE: &str = include_str!("../prompts/turn_prompt.md");

#[derive(Debug, Parser)]
#[command(name = "crank")]
#[command(about = "Agent-first governor for plan-driven tasks")]
#[command(long_about = HELP_LONG_ABOUT, after_long_help = HELP_AFTER_LONG)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[command(about = "Run the unattended governor from a TOML config")]
    Run(RunArgs),
    #[command(about = "Write a starter TOML config template")]
    Init(InitArgs),
    #[command(about = "Inspect or control a running governor state dir")]
    Ctl(CtlArgs),
}

#[derive(Debug, Args)]
struct RunArgs {
    #[arg(long, help = "Path to crank TOML config")]
    config: PathBuf,
}

#[derive(Debug, Args)]
struct InitArgs {
    #[arg(long, help = "Output path for starter TOML config")]
    output: PathBuf,
}

#[derive(Debug, Args)]
struct CtlArgs {
    #[command(subcommand)]
    command: CtlCommand,
}

#[derive(Debug, Subcommand)]
enum CtlCommand {
    #[command(about = "Print current run state JSON")]
    Snapshot {
        #[arg(long, help = "Governor state directory path")]
        state_dir: PathBuf,
    },
    #[command(about = "Exit 0 if run is safe to stop; 1 otherwise")]
    CanExit {
        #[arg(long, help = "Governor state directory path")]
        state_dir: PathBuf,
    },
    #[command(about = "Append an operator note to the run journal")]
    Note {
        #[arg(long, help = "Governor state directory path")]
        state_dir: PathBuf,
        #[arg(long, help = "Note text to append to journal")]
        message: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
struct Config {
    run_id: Option<String>,
    workspace: PathBuf,
    state_dir: PathBuf,
    #[serde(default = "default_unattended")]
    unattended: bool,
    #[serde(default = "default_poll_interval")]
    poll_interval_secs: u64,
    #[serde(default)]
    timeouts: TimeoutsConfig,
    #[serde(default)]
    recovery: RecoveryConfig,
    backend: BackendConfig,
    roles: RolesConfig,
    tasks: Vec<TaskConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct TimeoutsConfig {
    #[serde(default = "default_stall_secs")]
    stall_secs: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RecoveryConfig {
    #[serde(default = "default_max_recovery_attempts_per_task")]
    max_recovery_attempts_per_task: u32,
    #[serde(default = "default_max_failures_before_block")]
    max_failures_before_block: u32,
    #[serde(default = "default_backoff_initial_secs")]
    backoff_initial_secs: u64,
    #[serde(default = "default_backoff_max_secs")]
    backoff_max_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum BackendConfig {
    Codex(CodexBackendConfig),
    Mock(MockBackendConfig),
}

#[derive(Debug, Clone, Deserialize)]
struct CodexBackendConfig {
    #[serde(default = "default_codex_binary")]
    binary: String,
    model: String,
    thinking: String,
    #[serde(default = "default_approval_policy")]
    approval_policy: String,
    #[serde(default = "default_sandbox_mode")]
    sandbox_mode: String,
    #[serde(default)]
    extra_args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct MockBackendConfig {
    #[serde(default = "default_mock_steps_per_task")]
    steps_per_task: u32,
}

#[derive(Debug, Clone, Deserialize)]
struct RolesConfig {
    implementer: RoleConfig,
    reviewer_1: RoleConfig,
    reviewer_2: RoleConfig,
}

#[derive(Debug, Clone, Deserialize)]
struct RoleConfig {
    harness: String,
    model: String,
    thinking: String,
}

#[derive(Debug, Clone, Deserialize)]
struct TaskConfig {
    id: String,
    todo_file: PathBuf,
    #[serde(default)]
    depends_on: Vec<String>,
    coord_dir: Option<PathBuf>,
    completion_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum RunStatus {
    Running,
    Completed,
    FailedTerminal,
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum TaskStatus {
    Pending,
    Running,
    Completed,
    BlockedBestEffort,
}

impl TaskStatus {
    fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::BlockedBestEffort)
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::BlockedBestEffort => "blocked_best_effort",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TaskRuntime {
    id: String,
    todo_file: String,
    depends_on: Vec<String>,
    status: TaskStatus,
    coord_dir: String,
    completion_file: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
    last_progress_epoch: Option<i64>,
    recovery_attempts: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RunState {
    run_id: String,
    workspace: String,
    state_dir: String,
    unattended: bool,
    status: RunStatus,
    started_at: String,
    updated_at: String,
    journal_path: String,
    thread_id: Option<String>,
    cycle: u64,
    last_turn_at: Option<String>,
    tasks: Vec<TaskRuntime>,
}

#[derive(Debug, Clone)]
struct TurnResult {
    thread_id: Option<String>,
    final_response: String,
}

#[derive(Debug, Default, Deserialize)]
struct ControlBlock {
    task_id: Option<String>,
    status: Option<String>,
    needs_user_input: Option<bool>,
    summary: Option<String>,
    next_action: Option<String>,
}

struct LockGuard {
    lock_path: PathBuf,
}

impl LockGuard {
    fn acquire(state_dir: &Path) -> Result<Self> {
        ensure_dir(state_dir)?;
        let lock_path = state_dir.join("run.lock");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .with_context(|| {
                format!(
                    "could not acquire lock {} (another crank run may be active)",
                    lock_path.display()
                )
            })?;
        writeln!(file, "pid={}", std::process::id())?;
        Ok(Self { lock_path })
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock_path);
    }
}

fn default_unattended() -> bool {
    true
}

fn default_poll_interval() -> u64 {
    30
}

fn default_stall_secs() -> u64 {
    900
}

fn default_max_recovery_attempts_per_task() -> u32 {
    4
}

fn default_max_failures_before_block() -> u32 {
    6
}

fn default_backoff_initial_secs() -> u64 {
    5
}

fn default_backoff_max_secs() -> u64 {
    120
}

fn default_codex_binary() -> String {
    "codex".to_string()
}

fn default_approval_policy() -> String {
    "never".to_string()
}

fn default_sandbox_mode() -> String {
    "danger-full-access".to_string()
}

fn default_mock_steps_per_task() -> u32 {
    2
}

fn now_iso() -> String {
    Utc::now().to_rfc3339()
}

fn now_epoch() -> i64 {
    Utc::now().timestamp()
}

fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("failed to create {}", path.display()))
}

fn state_path(state_dir: &Path) -> PathBuf {
    state_dir.join("state.json")
}

fn journal_path(state_dir: &Path) -> PathBuf {
    state_dir.join("JOURNAL.md")
}

fn events_log_path(state_dir: &Path) -> PathBuf {
    state_dir.join("logs").join("orchestrator.events.jsonl")
}

fn turns_log_path(state_dir: &Path) -> PathBuf {
    state_dir.join("logs").join("orchestrator.turns.log")
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let bytes = serde_json::to_vec_pretty(value)?;
    fs::write(&tmp, bytes).with_context(|| format!("failed to write {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("failed to move {} to {}", tmp.display(), path.display()))?;
    Ok(())
}

fn append_journal(journal: &Path, title: &str, body: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(journal)
        .with_context(|| format!("failed to open {}", journal.display()))?;
    writeln!(file, "\n## {}", now_iso())?;
    writeln!(file, "**{}**", title)?;
    writeln!(file, "{}", body)?;
    Ok(())
}

fn append_text(path: &Path, text: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    file.write_all(text.as_bytes())?;
    Ok(())
}

fn mtime_epoch(path: &Path) -> Option<i64> {
    let md = fs::metadata(path).ok()?;
    let modified = md.modified().ok()?;
    let dur = modified.duration_since(UNIX_EPOCH).ok()?;
    Some(dur.as_secs() as i64)
}

fn latest_progress_epoch(coord_dir: &Path) -> Option<i64> {
    let mut latest = mtime_epoch(&coord_dir.join("state.md"));
    for sub in ["requests", "reviews", "decisions", "heartbeats"] {
        let dir = coord_dir.join(sub);
        let entries = match fs::read_dir(&dir) {
            Ok(it) => it,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if let Some(ts) = mtime_epoch(&entry.path()) {
                latest = Some(latest.map_or(ts, |cur| cur.max(ts)));
            }
        }
    }
    latest
}

fn check_coord_done(coord_dir: &Path) -> bool {
    let path = coord_dir.join("state.md");
    let text = match fs::read_to_string(path) {
        Ok(v) => v,
        Err(_) => return false,
    };
    text.trim() == "done"
}

fn load_config(path: &Path) -> Result<Config> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read config {}", path.display()))?;
    let cfg: Config =
        toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))?;

    if cfg.tasks.is_empty() {
        return Err(anyhow!("config.tasks must not be empty"));
    }

    let mut seen = std::collections::BTreeSet::new();
    for task in &cfg.tasks {
        if task.id.trim().is_empty() {
            return Err(anyhow!("task id must not be empty"));
        }
        if !seen.insert(task.id.clone()) {
            return Err(anyhow!("duplicate task id '{}'", task.id));
        }
    }

    Ok(cfg)
}

fn init_state(cfg: &Config) -> Result<RunState> {
    ensure_dir(&cfg.state_dir)?;
    ensure_dir(&cfg.state_dir.join("logs"))?;
    ensure_dir(&cfg.state_dir.join("coord"))?;

    let journal = journal_path(&cfg.state_dir);
    if !journal.exists() {
        let mut file = File::create(&journal)?;
        writeln!(file, "# JOURNAL")?;
        writeln!(file, "")?;
        writeln!(
            file,
            "Run journal for unattended orchestration. Blockers are recorded here instead of stopping the run."
        )?;
    }

    let s_path = state_path(&cfg.state_dir);
    if s_path.exists() {
        let bytes = fs::read(&s_path)?;
        let existing: RunState = serde_json::from_slice(&bytes)
            .with_context(|| format!("failed to parse {}", s_path.display()))?;
        return Ok(existing);
    }

    let run_id = cfg
        .run_id
        .clone()
        .unwrap_or_else(|| format!("run-{}", now_epoch()));

    let mut tasks = Vec::new();
    for task in &cfg.tasks {
        let coord = task
            .coord_dir
            .clone()
            .unwrap_or_else(|| cfg.state_dir.join("coord").join(&task.id));
        let completion_file = task.completion_file.clone();
        tasks.push(TaskRuntime {
            id: task.id.clone(),
            todo_file: task.todo_file.display().to_string(),
            depends_on: task.depends_on.clone(),
            status: TaskStatus::Pending,
            coord_dir: coord.display().to_string(),
            completion_file: completion_file.as_ref().map(|p| p.display().to_string()),
            started_at: None,
            completed_at: None,
            last_progress_epoch: None,
            recovery_attempts: 0,
        });
    }

    let now = now_iso();
    Ok(RunState {
        run_id,
        workspace: cfg.workspace.display().to_string(),
        state_dir: cfg.state_dir.display().to_string(),
        unattended: cfg.unattended,
        status: RunStatus::Running,
        started_at: now.clone(),
        updated_at: now,
        journal_path: journal.display().to_string(),
        thread_id: None,
        cycle: 0,
        last_turn_at: None,
        tasks,
    })
}

fn save_state(state: &mut RunState, state_dir: &Path) -> Result<()> {
    state.updated_at = now_iso();
    write_json_atomic(&state_path(state_dir), state)
}

fn deps_satisfied(state: &RunState, idx: usize) -> bool {
    let Some(task) = state.tasks.get(idx) else {
        return false;
    };

    for dep in &task.depends_on {
        let Some(dep_task) = state.tasks.iter().find(|t| &t.id == dep) else {
            return false;
        };
        if !dep_task.status.is_terminal() {
            return false;
        }
    }

    true
}

fn choose_next_pending_task(state: &RunState) -> Option<usize> {
    for (idx, task) in state.tasks.iter().enumerate() {
        if task.status == TaskStatus::Pending && deps_satisfied(state, idx) {
            return Some(idx);
        }
    }
    None
}

fn all_terminal(state: &RunState) -> bool {
    state.tasks.iter().all(|t| t.status.is_terminal())
}

fn can_exit(state: &RunState) -> bool {
    all_terminal(state)
}

fn task_done_by_artifact(task: &TaskRuntime) -> bool {
    if let Some(completion) = &task.completion_file {
        return Path::new(completion).exists();
    }
    check_coord_done(Path::new(&task.coord_dir))
}

fn sync_completion_and_progress(state: &mut RunState) {
    for task in &mut state.tasks {
        if task.status == TaskStatus::Running {
            if let Some(ts) = latest_progress_epoch(Path::new(&task.coord_dir)) {
                task.last_progress_epoch =
                    Some(task.last_progress_epoch.map_or(ts, |cur| cur.max(ts)));
            }
        }

        if !task.status.is_terminal() && task_done_by_artifact(task) {
            task.status = TaskStatus::Completed;
            if task.completed_at.is_none() {
                task.completed_at = Some(now_iso());
            }
            task.last_progress_epoch = Some(now_epoch());
        }
    }
}

fn mark_task_started(task: &mut TaskRuntime) -> Result<()> {
    task.status = TaskStatus::Running;
    if task.started_at.is_none() {
        task.started_at = Some(now_iso());
    }
    let coord = Path::new(&task.coord_dir);
    ensure_dir(coord)?;
    ensure_dir(&coord.join("heartbeats"))?;
    Ok(())
}

fn mark_task_blocked(task: &mut TaskRuntime) {
    task.status = TaskStatus::BlockedBestEffort;
    task.completed_at = Some(now_iso());
    task.last_progress_epoch = Some(now_epoch());
}

fn status_table(state: &RunState) -> String {
    let mut lines = Vec::new();
    for task in &state.tasks {
        lines.push(format!(
            "- {}: {} (deps: [{}])",
            task.id,
            task.status.as_str(),
            task.depends_on.join(", ")
        ));
    }
    lines.join("\n")
}

fn unresolved_placeholders(input: &str) -> Vec<String> {
    let mut pending = Vec::new();
    let mut rest = input;

    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            break;
        };
        let key = after[..end].trim();
        if !key.is_empty() && !pending.iter().any(|existing| existing == key) {
            pending.push(key.to_string());
        }
        rest = &after[end + 2..];
    }

    pending
}

fn render_template(template: &str, vars: &[(&str, String)]) -> Result<String> {
    let mut rendered = template.to_string();

    for (key, value) in vars {
        let placeholder = format!("{{{{{}}}}}", key);
        rendered = rendered.replace(&placeholder, value);
    }

    let pending = unresolved_placeholders(&rendered);
    if !pending.is_empty() {
        return Err(anyhow!(
            "unresolved template placeholders: {}",
            pending.join(", ")
        ));
    }

    Ok(rendered)
}

fn build_prompt(
    cfg: &Config,
    state: &RunState,
    task: &TaskRuntime,
    recovery_note: Option<&str>,
) -> Result<String> {
    let completion_line = if let Some(completion_file) = &task.completion_file {
        format!("- completion_file: {completion_file}")
    } else {
        "- completion rule: coord_dir/state.md must be exactly 'done'".to_string()
    };

    let recovery_block = recovery_note
        .map(|note| format!("\nRecovery note from governor:\n{note}\n"))
        .unwrap_or_default();

    render_template(
        TURN_PROMPT_TEMPLATE,
        &[
            ("run_id", state.run_id.clone()),
            ("workspace", cfg.workspace.display().to_string()),
            (
                "journal",
                journal_path(&cfg.state_dir).display().to_string(),
            ),
            ("state_dir", cfg.state_dir.display().to_string()),
            (
                "thread_id",
                state.thread_id.as_deref().unwrap_or("(new)").to_string(),
            ),
            ("task_board", status_table(state)),
            ("task_id", task.id.clone()),
            ("todo_file", task.todo_file.clone()),
            ("coord_dir", task.coord_dir.clone()),
            ("completion_line", completion_line),
            ("implementer_harness", cfg.roles.implementer.harness.clone()),
            ("implementer_model", cfg.roles.implementer.model.clone()),
            (
                "implementer_thinking",
                cfg.roles.implementer.thinking.clone(),
            ),
            ("reviewer_1_harness", cfg.roles.reviewer_1.harness.clone()),
            ("reviewer_1_model", cfg.roles.reviewer_1.model.clone()),
            ("reviewer_1_thinking", cfg.roles.reviewer_1.thinking.clone()),
            ("reviewer_2_harness", cfg.roles.reviewer_2.harness.clone()),
            ("reviewer_2_model", cfg.roles.reviewer_2.model.clone()),
            ("reviewer_2_thinking", cfg.roles.reviewer_2.thinking.clone()),
            ("recovery_block", recovery_block),
        ],
    )
}

fn extract_control_block(text: &str) -> Option<ControlBlock> {
    const START: &str = "<CONTROL_JSON>";
    const END: &str = "</CONTROL_JSON>";

    if let (Some(s), Some(e)) = (text.find(START), text.find(END)) {
        if e > s + START.len() {
            let raw = &text[s + START.len()..e];
            if let Ok(control) = serde_json::from_str::<ControlBlock>(raw.trim()) {
                return Some(control);
            }
        }
    }

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('{') && trimmed.ends_with('}') {
            if let Ok(control) = serde_json::from_str::<ControlBlock>(trimmed) {
                return Some(control);
            }
        }
    }

    None
}

fn run_turn_codex(
    cfg: &Config,
    backend: &CodexBackendConfig,
    state: &RunState,
    prompt: &str,
) -> Result<TurnResult> {
    let mut cmd = Command::new(&backend.binary);
    cmd.arg("exec")
        .arg("--experimental-json")
        .arg("--model")
        .arg(&backend.model)
        .arg("--sandbox")
        .arg(&backend.sandbox_mode)
        .arg("--config")
        .arg(format!("model_reasoning_effort=\"{}\"", backend.thinking))
        .arg("--config")
        .arg(format!("approval_policy=\"{}\"", backend.approval_policy))
        .arg("--cd")
        .arg(&cfg.workspace);

    for extra in &backend.extra_args {
        cmd.arg(extra);
    }

    if let Some(thread_id) = &state.thread_id {
        cmd.arg("resume").arg(thread_id);
    }

    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().with_context(|| {
        format!(
            "failed to spawn codex backend executable '{}'",
            backend.binary
        )
    })?;

    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("failed to open codex stdin"))?;
        stdin
            .write_all(prompt.as_bytes())
            .context("failed to write prompt to codex")?;
    }

    let output = child
        .wait_with_output()
        .context("failed waiting for codex process")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    let events_path = events_log_path(&cfg.state_dir);
    let mut parsed_thread_id: Option<String> = None;
    let mut final_response = String::new();

    for line in stdout.lines() {
        let line_trim = line.trim();
        if line_trim.is_empty() {
            continue;
        }

        append_text(&events_path, &format!("{}\n", line_trim))?;

        if let Ok(value) = serde_json::from_str::<Value>(line_trim) {
            if value.get("type").and_then(|v| v.as_str()) == Some("thread.started") {
                if let Some(id) = value.get("thread_id").and_then(|v| v.as_str()) {
                    parsed_thread_id = Some(id.to_string());
                }
            }

            if value.get("type").and_then(|v| v.as_str()) == Some("item.completed") {
                if let Some(item) = value.get("item") {
                    if item.get("type").and_then(|v| v.as_str()) == Some("agent_message") {
                        if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                            final_response = text.to_string();
                        }
                    }
                }
            }
        }
    }

    if !output.status.success() {
        return Err(anyhow!(
            "codex turn failed with status {}\nstderr:\n{}",
            output.status,
            stderr
        ));
    }

    if final_response.is_empty() {
        final_response = "(no agent message captured)".to_string();
    }

    Ok(TurnResult {
        thread_id: parsed_thread_id,
        final_response,
    })
}

fn run_turn_mock(task: &TaskRuntime, backend: &MockBackendConfig) -> Result<TurnResult> {
    let coord = Path::new(&task.coord_dir);
    ensure_dir(coord)?;
    ensure_dir(&coord.join("heartbeats"))?;

    let turns_path = coord.join("mock.turns");
    let prev_turns = fs::read_to_string(&turns_path)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);
    let turns = prev_turns.saturating_add(1);
    fs::write(&turns_path, turns.to_string())?;
    fs::write(
        coord.join("heartbeats").join("implementer.epoch"),
        format!("{}\n", now_epoch()),
    )?;

    let done = turns >= backend.steps_per_task.max(1);
    let state_text = if done { "done\n" } else { "active\n" };
    fs::write(coord.join("state.md"), state_text)?;

    let status = if done { "completed" } else { "in_progress" };
    let final_response = format!(
        "Mock backend processed task {} turn {}.\n<CONTROL_JSON>\n{{\"task_id\":\"{}\",\"status\":\"{}\",\"needs_user_input\":false,\"summary\":\"mock progress\",\"next_action\":\"continue\"}}\n</CONTROL_JSON>",
        task.id, turns, task.id, status
    );

    Ok(TurnResult {
        thread_id: None,
        final_response,
    })
}

fn run_turn(
    cfg: &Config,
    state: &RunState,
    task: &TaskRuntime,
    prompt: &str,
) -> Result<TurnResult> {
    match &cfg.backend {
        BackendConfig::Codex(codex) => run_turn_codex(cfg, codex, state, prompt),
        BackendConfig::Mock(mock) => run_turn_mock(task, mock),
    }
}

fn log_turn(state_dir: &Path, cycle: u64, prompt: &str, response: &str) -> Result<()> {
    let turns_log = turns_log_path(state_dir);
    let mut buf = String::new();
    buf.push_str(&format!("\n===== TURN {} @ {} =====\n", cycle, now_iso()));
    buf.push_str("--- PROMPT ---\n");
    buf.push_str(prompt);
    if !prompt.ends_with('\n') {
        buf.push('\n');
    }
    buf.push_str("--- RESPONSE ---\n");
    buf.push_str(response);
    if !response.ends_with('\n') {
        buf.push('\n');
    }
    append_text(&turns_log, &buf)
}

fn compute_backoff_secs(recovery: &RecoveryConfig, failures: u32) -> u64 {
    let shift = failures.saturating_sub(1).min(10);
    let mult = 1u64 << shift;
    let raw = recovery.backoff_initial_secs.saturating_mul(mult);
    raw.clamp(1, recovery.backoff_max_secs.max(1))
}

fn run_governor(cfg: Config) -> Result<()> {
    ensure_dir(&cfg.state_dir)?;
    ensure_dir(&cfg.state_dir.join("logs"))?;
    ensure_dir(&cfg.state_dir.join("coord"))?;

    let _lock = LockGuard::acquire(&cfg.state_dir)?;

    let mut state = init_state(&cfg)?;
    let journal = PathBuf::from(&state.journal_path);

    if state.cycle == 0 {
        append_journal(
            &journal,
            "run boot",
            &format!(
                "Starting run {} in {} with {} tasks.",
                state.run_id,
                cfg.workspace.display(),
                state.tasks.len()
            ),
        )?;
    } else {
        append_journal(
            &journal,
            "run resume",
            &format!("Resuming run {} at cycle {}.", state.run_id, state.cycle),
        )?;
    }

    let mut consecutive_failures = 0u32;
    save_state(&mut state, &cfg.state_dir)?;

    loop {
        sync_completion_and_progress(&mut state);

        if all_terminal(&state) {
            state.status = RunStatus::Completed;
            save_state(&mut state, &cfg.state_dir)?;
            append_journal(
                &journal,
                "run completed",
                "All tasks reached terminal status.",
            )?;
            break;
        }

        let mut active_idx = state
            .tasks
            .iter()
            .position(|t| t.status == TaskStatus::Running);

        if active_idx.is_none() {
            if let Some(next) = choose_next_pending_task(&state) {
                let task_id = state.tasks[next].id.clone();
                mark_task_started(&mut state.tasks[next])?;
                append_journal(
                    &journal,
                    "task started",
                    &format!(
                        "Task {} started with coord dir {}",
                        task_id, state.tasks[next].coord_dir
                    ),
                )?;
                active_idx = Some(next);
            } else {
                state.status = RunStatus::FailedTerminal;
                save_state(&mut state, &cfg.state_dir)?;
                append_journal(
                    &journal,
                    "deadlock",
                    "No runnable pending task found; dependency graph may be invalid.",
                )?;
                break;
            }
        }

        let idx = active_idx.expect("active index must be set");

        let now = now_epoch();
        let mut recovery_note: Option<String> = None;
        {
            let task = &mut state.tasks[idx];
            if task.last_progress_epoch.is_none() {
                task.last_progress_epoch = Some(now);
            }

            if let Some(last) = task.last_progress_epoch {
                let age = now.saturating_sub(last);
                if age > cfg.timeouts.stall_secs as i64 {
                    if task.recovery_attempts >= cfg.recovery.max_recovery_attempts_per_task {
                        mark_task_blocked(task);
                        append_journal(
                            &journal,
                            "task blocked best-effort",
                            &format!(
                                "Task {} exceeded recovery attempts after {}s without progress. Marked blocked_best_effort.",
                                task.id, age
                            ),
                        )?;
                        save_state(&mut state, &cfg.state_dir)?;
                        thread::sleep(Duration::from_secs(cfg.poll_interval_secs.max(1)));
                        continue;
                    }

                    task.recovery_attempts = task.recovery_attempts.saturating_add(1);
                    recovery_note = Some(format!(
                        "Stall detected: no progress for {}s (threshold {}s). Recovery attempt {} of {}.",
                        age,
                        cfg.timeouts.stall_secs,
                        task.recovery_attempts,
                        cfg.recovery.max_recovery_attempts_per_task
                    ));
                }
            }
        }

        let task_snapshot = state.tasks[idx].clone();
        let prompt = build_prompt(&cfg, &state, &task_snapshot, recovery_note.as_deref())?;

        state.cycle = state.cycle.saturating_add(1);

        let turn = run_turn(&cfg, &state, &task_snapshot, &prompt);
        match turn {
            Ok(turn_result) => {
                consecutive_failures = 0;
                if let Some(id) = turn_result.thread_id {
                    state.thread_id = Some(id);
                }
                state.last_turn_at = Some(now_iso());
                log_turn(
                    &cfg.state_dir,
                    state.cycle,
                    &prompt,
                    &turn_result.final_response,
                )?;

                if let Some(control) = extract_control_block(&turn_result.final_response) {
                    let control_status = control.status.as_deref().unwrap_or("(missing)");
                    let summary = control.summary.unwrap_or_default();
                    let next_action = control.next_action.unwrap_or_default();
                    append_journal(
                        &journal,
                        "turn control",
                        &format!(
                            "task={} control_task={} status={} needs_user_input={}\nsummary={}\nnext_action={}",
                            task_snapshot.id,
                            control.task_id.unwrap_or_else(|| "(missing)".to_string()),
                            control_status,
                            control.needs_user_input.unwrap_or(false),
                            summary,
                            next_action
                        ),
                    )?;

                    if cfg.unattended && control.needs_user_input.unwrap_or(false) {
                        append_journal(
                            &journal,
                            "unattended override",
                            "Orchestrator indicated user input was needed. Governor will continue with best-effort without user interaction.",
                        )?;
                    }
                } else {
                    append_journal(
                        &journal,
                        "missing control block",
                        "No CONTROL_JSON block found in orchestrator response. Continuing.",
                    )?;
                }

                sync_completion_and_progress(&mut state);
                save_state(&mut state, &cfg.state_dir)?;
                thread::sleep(Duration::from_secs(cfg.poll_interval_secs.max(1)));
            }
            Err(err) => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                append_journal(
                    &journal,
                    "turn failure",
                    &format!(
                        "Task {} turn failed (consecutive failures={}): {}",
                        task_snapshot.id, consecutive_failures, err
                    ),
                )?;

                if consecutive_failures >= cfg.recovery.max_failures_before_block {
                    let task = &mut state.tasks[idx];
                    mark_task_blocked(task);
                    append_journal(
                        &journal,
                        "task blocked after repeated failures",
                        &format!(
                            "Task {} hit {} consecutive turn failures and was marked blocked_best_effort.",
                            task.id, consecutive_failures
                        ),
                    )?;
                    consecutive_failures = 0;
                }

                save_state(&mut state, &cfg.state_dir)?;
                let backoff = compute_backoff_secs(&cfg.recovery, consecutive_failures.max(1));
                thread::sleep(Duration::from_secs(backoff));
            }
        }
    }

    Ok(())
}

fn write_default_config(output: &Path) -> Result<()> {
    let content = r#"run_id = "pika-call-plans"
workspace = "/Users/justin/code/pika"
state_dir = "/Users/justin/code/crank/runs/pika-call-plans"
unattended = true
poll_interval_secs = 30

[timeouts]
stall_secs = 900

[recovery]
max_recovery_attempts_per_task = 4
max_failures_before_block = 6
backoff_initial_secs = 5
backoff_max_secs = 120

[backend]
kind = "codex"
binary = "codex"
model = "gpt-5.3-codex"
thinking = "xhigh"
approval_policy = "never"
sandbox_mode = "danger-full-access"
extra_args = []

[roles.implementer]
harness = "codex"
model = "gpt-5.3-codex"
thinking = "xhigh"

[roles.reviewer_1]
harness = "codex"
model = "gpt-5.3-codex"
thinking = "xhigh"

[roles.reviewer_2]
harness = "claude"
model = "claude-opus-4-6"
thinking = "xhigh"

[[tasks]]
id = "call-audio"
todo_file = "/Users/justin/code/pika/todos/call-audio-plan.md"
depends_on = []

[[tasks]]
id = "call-transport"
todo_file = "/Users/justin/code/pika/todos/call-transport-plan.md"
depends_on = ["call-audio"]

[[tasks]]
id = "call-video"
todo_file = "/Users/justin/code/pika/todos/call-video-plan.md"
depends_on = ["call-audio", "call-transport"]

[[tasks]]
id = "call-native-audio"
todo_file = "/Users/justin/code/pika/todos/call-native-audio-plan.md"
depends_on = ["call-audio", "call-transport", "call-video"]
"#;

    if let Some(parent) = output.parent() {
        ensure_dir(parent)?;
    }
    fs::write(output, content).with_context(|| format!("failed to write {}", output.display()))?;
    Ok(())
}

fn ctl_snapshot(state_dir: &Path) -> Result<()> {
    let bytes = fs::read(state_path(state_dir))
        .with_context(|| format!("failed to read state under {}", state_dir.display()))?;
    let state: RunState = serde_json::from_slice(&bytes)?;
    println!("{}", serde_json::to_string_pretty(&state)?);
    Ok(())
}

fn ctl_can_exit(state_dir: &Path) -> Result<bool> {
    let bytes = fs::read(state_path(state_dir))
        .with_context(|| format!("failed to read state under {}", state_dir.display()))?;
    let state: RunState = serde_json::from_slice(&bytes)?;
    Ok(can_exit(&state))
}

fn ctl_note(state_dir: &Path, message: &str) -> Result<()> {
    append_journal(&journal_path(state_dir), "operator note", message)
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run(args) => {
            let cfg = load_config(&args.config)?;
            run_governor(cfg)
        }
        Commands::Init(args) => {
            write_default_config(&args.output)?;
            println!("wrote {}", args.output.display());
            Ok(())
        }
        Commands::Ctl(args) => match args.command {
            CtlCommand::Snapshot { state_dir } => ctl_snapshot(&state_dir),
            CtlCommand::CanExit { state_dir } => {
                let ok = ctl_can_exit(&state_dir)?;
                println!("{}", if ok { "true" } else { "false" });
                if ok {
                    Ok(())
                } else {
                    std::process::exit(1);
                }
            }
            CtlCommand::Note { state_dir, message } => ctl_note(&state_dir, &message),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_template_replaces_placeholders() {
        let rendered = render_template("hello {{name}}", &[("name", "crank".to_string())]).unwrap();
        assert_eq!(rendered, "hello crank");
    }

    #[test]
    fn render_template_fails_with_unresolved_placeholders() {
        let err = render_template(
            "hello {{name}} {{missing}}",
            &[("name", "crank".to_string())],
        )
        .expect_err("template should fail when placeholders are unresolved");
        assert!(err.to_string().contains("missing"));
    }
}
