use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

const HELP_LONG_ABOUT: &str = include_str!("../prompts/help_long_about.md");
const HELP_AFTER_LONG: &str = include_str!("../prompts/help_after_long.md");
const TURN_PROMPT_TEMPLATE: &str = include_str!("../prompts/turn_prompt.md");
const DEFAULT_TEAMS_DIR: &str = "teams";
const REQUIRED_CODEX_ARG: &str = "--yolo";
const REQUIRED_CLAUDE_ARG: &str = "--dangerously-skip-permissions";

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
    #[command(about = "Manage reusable role/model team definitions")]
    Teams(TeamsArgs),
}

#[derive(Debug, Args)]
struct RunArgs {
    #[arg(long, help = "Path to crank TOML config")]
    config: PathBuf,
    #[arg(long, help = "Apply team by name (e.g. xhigh) to role settings")]
    team: Option<String>,
    #[arg(long, help = "Apply team from explicit TOML file path")]
    team_file: Option<PathBuf>,
    #[arg(long, default_value = DEFAULT_TEAMS_DIR, help = "Teams directory")]
    teams_dir: PathBuf,
}

#[derive(Debug, Args)]
struct InitArgs {
    #[arg(long, help = "Output path for starter TOML config")]
    output: PathBuf,
    #[arg(long, help = "Seed config with team by name (e.g. xhigh)")]
    team: Option<String>,
    #[arg(long, help = "Seed config with team from explicit TOML file path")]
    team_file: Option<PathBuf>,
    #[arg(long, default_value = DEFAULT_TEAMS_DIR, help = "Teams directory")]
    teams_dir: PathBuf,
}

#[derive(Debug, Args)]
struct CtlArgs {
    #[command(subcommand)]
    command: CtlCommand,
}

#[derive(Debug, Args)]
struct TeamsArgs {
    #[command(subcommand)]
    command: TeamsCommand,
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

#[derive(Debug, Subcommand)]
enum TeamsCommand {
    #[command(about = "List available teams")]
    List {
        #[arg(long, default_value = DEFAULT_TEAMS_DIR, help = "Teams directory")]
        dir: PathBuf,
    },
    #[command(about = "Validate team file(s) and required harness launch args")]
    Validate(TeamsValidateArgs),
}

#[derive(Debug, Args)]
struct TeamsValidateArgs {
    #[arg(long, help = "Validate a specific team by name (file stem)")]
    team: Option<String>,
    #[arg(long, help = "Validate an explicit team file path")]
    file: Option<PathBuf>,
    #[arg(long, default_value = DEFAULT_TEAMS_DIR, help = "Teams directory")]
    dir: PathBuf,
    #[arg(long, help = "Validate all *.toml files in teams directory")]
    all: bool,
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
    #[serde(default)]
    policy: PolicyConfig,
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
struct PolicyConfig {
    #[serde(default)]
    unattended_escalate: UnattendedEscalatePolicy,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            unattended_escalate: default_unattended_escalate_policy(),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum UnattendedEscalatePolicy {
    Strict,
    BestEffortOnce,
}

impl Default for UnattendedEscalatePolicy {
    fn default() -> Self {
        default_unattended_escalate_policy()
    }
}

impl UnattendedEscalatePolicy {
    fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::BestEffortOnce => "best_effort_once",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum BackendConfig {
    Codex(CodexBackendConfig),
    Claude(ClaudeBackendConfig),
    Droid(DroidBackendConfig),
    Pi(PiBackendConfig),
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

#[derive(Debug, Clone, Deserialize)]
struct ClaudeBackendConfig {
    #[serde(default = "default_claude_binary")]
    binary: String,
    model: String,
    thinking: String,
    #[serde(default)]
    extra_args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct DroidBackendConfig {
    #[serde(default = "default_droid_binary")]
    binary: String,
    model: String,
    thinking: String,
    #[serde(default = "default_droid_autonomy")]
    auto: String,
    #[serde(default)]
    extra_args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct PiBackendConfig {
    #[serde(default = "default_pi_binary")]
    binary: String,
    model: String,
    thinking: String,
    #[serde(default)]
    provider: Option<String>,
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
    #[serde(default)]
    launch_args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct TeamFile {
    name: Option<String>,
    description: Option<String>,
    roles: RolesConfig,
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
    #[serde(default)]
    blocked_reason: Option<String>,
    last_progress_epoch: Option<i64>,
    recovery_attempts: u32,
    #[serde(default)]
    unattended_escalate_retries: u32,
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
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(file) => file,
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                if try_break_stale_lock(&lock_path)? {
                    OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&lock_path)
                        .with_context(|| {
                            format!(
                                "could not acquire lock {} after removing stale lock",
                                lock_path.display()
                            )
                        })?
                } else {
                    return Err(anyhow!(
                        "could not acquire lock {} (another crank run may be active)",
                        lock_path.display()
                    ));
                }
            }
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("could not acquire lock {}", lock_path.display()));
            }
        };
        writeln!(file, "pid={}", std::process::id())?;
        Ok(Self { lock_path })
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock_path);
    }
}

fn lock_pid(lock_path: &Path) -> Option<u32> {
    let text = fs::read_to_string(lock_path).ok()?;
    for line in text.lines() {
        if let Some(raw) = line.strip_prefix("pid=") {
            if let Ok(pid) = raw.trim().parse::<u32>() {
                return Some(pid);
            }
        }
    }
    None
}

fn process_is_alive(pid: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn try_break_stale_lock(lock_path: &Path) -> Result<bool> {
    let Some(pid) = lock_pid(lock_path) else {
        return Ok(false);
    };
    if process_is_alive(pid) {
        return Ok(false);
    }
    fs::remove_file(lock_path)
        .with_context(|| format!("failed to remove stale lock {}", lock_path.display()))?;
    Ok(true)
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

fn default_unattended_escalate_policy() -> UnattendedEscalatePolicy {
    UnattendedEscalatePolicy::BestEffortOnce
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

fn default_claude_binary() -> String {
    "claude".to_string()
}

fn default_droid_binary() -> String {
    "droid".to_string()
}

fn default_droid_autonomy() -> String {
    "high".to_string()
}

fn default_pi_binary() -> String {
    "pi".to_string()
}

fn default_mock_steps_per_task() -> u32 {
    2
}

fn default_roles() -> RolesConfig {
    RolesConfig {
        implementer: RoleConfig {
            harness: "codex".to_string(),
            model: "gpt-5.3-codex".to_string(),
            thinking: "xhigh".to_string(),
            launch_args: vec![REQUIRED_CODEX_ARG.to_string()],
        },
        reviewer_1: RoleConfig {
            harness: "codex".to_string(),
            model: "gpt-5.3-codex".to_string(),
            thinking: "xhigh".to_string(),
            launch_args: vec![REQUIRED_CODEX_ARG.to_string()],
        },
        reviewer_2: RoleConfig {
            harness: "claude".to_string(),
            model: "claude-opus-4-6".to_string(),
            thinking: "xhigh".to_string(),
            launch_args: vec![REQUIRED_CLAUDE_ARG.to_string()],
        },
    }
}

fn builtin_team(name: &str) -> Option<TeamFile> {
    match name {
        "xhigh" => Some(TeamFile {
            name: Some("xhigh".to_string()),
            description: Some(
                "Codex implementer + codex reviewer-1 + Claude reviewer-2, all xhigh".to_string(),
            ),
            roles: default_roles(),
        }),
        _ => None,
    }
}

fn builtin_team_names() -> &'static [&'static str] {
    &["xhigh"]
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

fn ensure_log_files(state_dir: &Path) -> Result<()> {
    for path in [events_log_path(state_dir), turns_log_path(state_dir)] {
        if !path.exists() {
            File::create(&path).with_context(|| format!("failed to create {}", path.display()))?;
        }
    }
    Ok(())
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

const MAX_EVENT_OUTPUT_CHARS: usize = 1200;

fn truncate_event_field(map: &mut serde_json::Map<String, Value>, key: &str, max_chars: usize) {
    let Some(Value::String(s)) = map.get_mut(key) else {
        return;
    };
    if s.chars().count() <= max_chars {
        return;
    }
    let original_chars = s.chars().count();
    let truncated: String = s.chars().take(max_chars).collect();
    *s = format!(
        "{truncated}\n...[truncated {} chars]",
        original_chars.saturating_sub(max_chars)
    );
}

fn sanitize_event_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for key in ["aggregated_output", "stdout", "stderr"] {
                truncate_event_field(map, key, MAX_EVENT_OUTPUT_CHARS);
            }
            for nested in map.values_mut() {
                sanitize_event_value(nested);
            }
        }
        Value::Array(items) => {
            for item in items {
                sanitize_event_value(item);
            }
        }
        _ => {}
    }
}

fn append_event_line(path: &Path, raw_line: &str) -> Result<()> {
    let rendered = match serde_json::from_str::<Value>(raw_line) {
        Ok(mut value) => {
            sanitize_event_value(&mut value);
            serde_json::to_string(&value).unwrap_or_else(|_| raw_line.to_string())
        }
        Err(_) => raw_line.to_string(),
    };
    append_text(path, &format!("{rendered}\n"))
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

fn required_launch_arg_for_harness(harness: &str) -> Option<&'static str> {
    match harness {
        "codex" => Some(REQUIRED_CODEX_ARG),
        "claude" => Some(REQUIRED_CLAUDE_ARG),
        _ => None,
    }
}

fn role_launch_args_display(role: &RoleConfig) -> String {
    if role.launch_args.is_empty() {
        "(none)".to_string()
    } else {
        role.launch_args.join(" ")
    }
}

fn validate_role(role_name: &str, role: &RoleConfig) -> Result<()> {
    if role.harness.trim().is_empty() {
        return Err(anyhow!("role '{role_name}' must set harness"));
    }
    if role.model.trim().is_empty() {
        return Err(anyhow!("role '{role_name}' must set model"));
    }
    if role.thinking.trim().is_empty() {
        return Err(anyhow!("role '{role_name}' must set thinking"));
    }

    if let Some(required) = required_launch_arg_for_harness(role.harness.as_str()) {
        let has_required = role.launch_args.iter().any(|arg| arg == required);
        if !has_required {
            return Err(anyhow!(
                "role '{role_name}' (harness={}) must include launch arg '{}'",
                role.harness,
                required
            ));
        }
    }

    Ok(())
}

fn validate_roles(roles: &RolesConfig) -> Result<()> {
    validate_role("implementer", &roles.implementer)?;
    validate_role("reviewer_1", &roles.reviewer_1)?;
    validate_role("reviewer_2", &roles.reviewer_2)?;
    Ok(())
}

fn parse_team_file(path: &Path) -> Result<TeamFile> {
    let text =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let team: TeamFile =
        toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))?;
    validate_roles(&team.roles).with_context(|| format!("invalid team {}", path.display()))?;
    Ok(team)
}

fn list_team_files(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    let entries =
        fs::read_dir(dir).with_context(|| format!("failed to read teams dir {}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("toml") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn resolve_team_path(dir: &Path, team: &str) -> PathBuf {
    let mut file = team.to_string();
    if !file.ends_with(".toml") {
        file.push_str(".toml");
    }
    dir.join(file)
}

fn load_team(dir: &Path, team: &str) -> Result<TeamFile> {
    let path = resolve_team_path(dir, team);
    if path.exists() {
        return parse_team_file(&path);
    }
    if let Some(builtin) = builtin_team(team) {
        return Ok(builtin);
    }
    Err(anyhow!(
        "team '{}' not found in {} and not a builtin team",
        team,
        dir.display()
    ))
}

fn load_team_from_file(path: &Path) -> Result<TeamFile> {
    parse_team_file(path)
}

fn cmd_teams_list(dir: &Path) -> Result<()> {
    let files = list_team_files(dir)?;
    let mut file_team_names = std::collections::BTreeSet::new();
    for path in &files {
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            file_team_names.insert(stem.to_string());
        }
    }

    for name in builtin_team_names() {
        if file_team_names.contains(*name) {
            continue;
        }
        if let Some(team) = builtin_team(name) {
            let desc = team.description.unwrap_or_default();
            if desc.is_empty() {
                println!("{name}");
            } else {
                println!("{name}\t{desc}");
            }
        }
    }

    if files.is_empty() && builtin_team_names().is_empty() {
        println!("(no teams found in {})", dir.display());
        return Ok(());
    }

    let mut file_count = 0usize;
    for path in files {
        let fallback_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("(unknown)")
            .to_string();
        match parse_team_file(&path) {
            Ok(team) => {
                let name = team.name.unwrap_or(fallback_name);
                let desc = team.description.unwrap_or_default();
                if desc.is_empty() {
                    println!("{name}");
                } else {
                    println!("{name}\t{desc}");
                }
            }
            Err(err) => {
                println!("{fallback_name}\tINVALID ({err})");
            }
        }
        file_count += 1;
    }

    if file_count == 0 {
        println!("(no file-based teams in {})", dir.display());
    }
    Ok(())
}

fn cmd_teams_validate(args: &TeamsValidateArgs) -> Result<()> {
    let requested = args.file.is_some() || args.team.is_some() || args.all;
    if !requested {
        return Err(anyhow!(
            "provide one of --all, --team <name>, or --file <path>"
        ));
    }
    if args.all && (args.file.is_some() || args.team.is_some()) {
        return Err(anyhow!("--all cannot be combined with --team/--file"));
    }
    if args.file.is_some() && args.team.is_some() {
        return Err(anyhow!("use either --team or --file, not both"));
    }

    let mut failures = Vec::new();
    if args.all {
        let files = list_team_files(&args.dir)?;
        let mut file_team_names = std::collections::BTreeSet::new();
        for file in &files {
            if let Some(stem) = file.file_stem().and_then(|s| s.to_str()) {
                file_team_names.insert(stem.to_string());
            }
        }
        for name in builtin_team_names() {
            if file_team_names.contains(*name) {
                continue;
            }
            match load_team(&args.dir, name) {
                Ok(_) => println!("ok\tbuiltin:{name}"),
                Err(err) => {
                    println!("err\tbuiltin:{name}\t{err}");
                    failures.push(format!("builtin:{name}: {err}"));
                }
            }
        }
        for file in &files {
            match parse_team_file(file) {
                Ok(_) => println!("ok\t{}", file.display()),
                Err(err) => {
                    println!("err\t{}\t{}", file.display(), err);
                    failures.push(format!("{}: {err}", file.display()));
                }
            }
        }
        if files.is_empty() && builtin_team_names().is_empty() {
            failures.push("no teams available to validate".to_string());
        }
    } else if let Some(path) = &args.file {
        match load_team_from_file(path) {
            Ok(_) => println!("ok\t{}", path.display()),
            Err(err) => {
                println!("err\t{}\t{}", path.display(), err);
                failures.push(format!("{}: {err}", path.display()));
            }
        }
    } else {
        let team_name = args.team.as_deref().expect("checked above");
        match load_team(&args.dir, team_name) {
            Ok(_) => println!("ok\t{}", team_name),
            Err(err) => {
                println!("err\t{}\t{}", team_name, err);
                failures.push(format!("{team_name}: {err}"));
            }
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(anyhow!("team validation failed:\n{}", failures.join("\n")))
    }
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
            blocked_reason: None,
            last_progress_epoch: None,
            recovery_attempts: 0,
            unattended_escalate_retries: 0,
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
            task.blocked_reason = None;
            task.last_progress_epoch = Some(now_epoch());
        }
    }
}

fn mark_task_started(task: &mut TaskRuntime) -> Result<()> {
    task.status = TaskStatus::Running;
    task.blocked_reason = None;
    if task.started_at.is_none() {
        task.started_at = Some(now_iso());
    }
    let coord = Path::new(&task.coord_dir);
    ensure_dir(coord)?;
    ensure_dir(&coord.join("heartbeats"))?;
    Ok(())
}

fn mark_task_blocked(task: &mut TaskRuntime, reason: &str) {
    task.status = TaskStatus::BlockedBestEffort;
    task.completed_at = Some(now_iso());
    task.blocked_reason = Some(reason.to_string());
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

fn configured_reviewer_quorum(roles: &RolesConfig) -> u32 {
    let mut count = 0u32;
    if !roles.reviewer_1.harness.trim().is_empty() {
        count = count.saturating_add(1);
    }
    if !roles.reviewer_2.harness.trim().is_empty() {
        count = count.saturating_add(1);
    }
    count.max(1)
}

fn coord_reviewer_count(coord_dir: &Path) -> Option<u32> {
    let meta_path = coord_dir.join("meta.env");
    let text = fs::read_to_string(meta_path).ok()?;
    for line in text.lines() {
        if let Some(raw) = line.strip_prefix("REVIEWER_COUNT=") {
            let cleaned = raw.trim().trim_matches('\'').trim_matches('"');
            if let Ok(value) = cleaned.parse::<u32>() {
                return Some(value);
            }
            let digits: String = cleaned.chars().filter(|c| c.is_ascii_digit()).collect();
            if let Ok(value) = digits.parse::<u32>() {
                return Some(value);
            }
        }
    }
    None
}

fn run_summary_path(state_dir: &Path) -> PathBuf {
    state_dir.join("run-summary.json")
}

#[derive(Serialize)]
struct RunSummary {
    run_id: String,
    status: RunStatus,
    cycle: u64,
    started_at: String,
    finished_at: String,
    thread_id: Option<String>,
    unattended: bool,
    unattended_escalate_policy: String,
    tasks_total: usize,
    tasks_completed: usize,
    tasks_blocked: usize,
    blocked_tasks: Vec<BlockedTaskSummary>,
}

#[derive(Serialize)]
struct BlockedTaskSummary {
    id: String,
    reason: Option<String>,
}

fn write_run_summary(state: &RunState, cfg: &Config) -> Result<()> {
    let mut tasks_completed = 0usize;
    let mut tasks_blocked = 0usize;
    let mut blocked_tasks = Vec::new();

    for task in &state.tasks {
        match task.status {
            TaskStatus::Completed => tasks_completed = tasks_completed.saturating_add(1),
            TaskStatus::BlockedBestEffort => {
                tasks_blocked = tasks_blocked.saturating_add(1);
                blocked_tasks.push(BlockedTaskSummary {
                    id: task.id.clone(),
                    reason: task.blocked_reason.clone(),
                });
            }
            _ => {}
        }
    }

    let summary = RunSummary {
        run_id: state.run_id.clone(),
        status: state.status.clone(),
        cycle: state.cycle,
        started_at: state.started_at.clone(),
        finished_at: state.updated_at.clone(),
        thread_id: state.thread_id.clone(),
        unattended: state.unattended,
        unattended_escalate_policy: cfg.policy.unattended_escalate.as_str().to_string(),
        tasks_total: state.tasks.len(),
        tasks_completed,
        tasks_blocked,
        blocked_tasks,
    };

    write_json_atomic(&run_summary_path(&cfg.state_dir), &summary)
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum EscalateHandling {
    Ignore,
    Retry,
    Block,
}

fn decide_unattended_escalate(
    unattended: bool,
    policy: UnattendedEscalatePolicy,
    task: &mut TaskRuntime,
    control_status: Option<&str>,
    next_action: Option<&str>,
) -> EscalateHandling {
    if !unattended {
        return EscalateHandling::Ignore;
    }
    let action_escalate = next_action
        .map(|v| v.eq_ignore_ascii_case("ESCALATE"))
        .unwrap_or(false);
    let status_escalate = control_status
        .map(|v| {
            let s = v.trim();
            s.eq_ignore_ascii_case("blocked") || s.eq_ignore_ascii_case("blocked_best_effort")
        })
        .unwrap_or(false);
    let should_escalate = action_escalate || status_escalate;
    if !should_escalate {
        return EscalateHandling::Ignore;
    }

    match policy {
        UnattendedEscalatePolicy::Strict => EscalateHandling::Block,
        UnattendedEscalatePolicy::BestEffortOnce => {
            if task.unattended_escalate_retries == 0 {
                task.unattended_escalate_retries = 1;
                EscalateHandling::Retry
            } else {
                EscalateHandling::Block
            }
        }
    }
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
    let reviewer_quorum = configured_reviewer_quorum(&cfg.roles);
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
            (
                "implementer_args",
                role_launch_args_display(&cfg.roles.implementer),
            ),
            ("reviewer_1_harness", cfg.roles.reviewer_1.harness.clone()),
            ("reviewer_1_model", cfg.roles.reviewer_1.model.clone()),
            ("reviewer_1_thinking", cfg.roles.reviewer_1.thinking.clone()),
            (
                "reviewer_1_args",
                role_launch_args_display(&cfg.roles.reviewer_1),
            ),
            ("reviewer_2_harness", cfg.roles.reviewer_2.harness.clone()),
            ("reviewer_2_model", cfg.roles.reviewer_2.model.clone()),
            ("reviewer_2_thinking", cfg.roles.reviewer_2.thinking.clone()),
            (
                "reviewer_2_args",
                role_launch_args_display(&cfg.roles.reviewer_2),
            ),
            ("reviewer_quorum", reviewer_quorum.to_string()),
            (
                "unattended_escalate_policy",
                cfg.policy.unattended_escalate.as_str().to_string(),
            ),
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

fn run_backend_command_streaming<F>(
    mut cmd: Command,
    prompt: &str,
    backend_name: &str,
    mut on_stdout_line: F,
) -> Result<()>
where
    F: FnMut(&str) -> Result<()>,
{
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn {backend_name} backend executable"))?;

    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("failed to open {backend_name} stdin"))?;
        if !prompt.is_empty() {
            stdin
                .write_all(prompt.as_bytes())
                .with_context(|| format!("failed to write prompt to {backend_name}"))?;
            if !prompt.ends_with('\n') {
                stdin
                    .write_all(b"\n")
                    .with_context(|| format!("failed to finalize prompt for {backend_name}"))?;
            }
        }
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("failed to open {backend_name} stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("failed to open {backend_name} stderr"))?;

    let stderr_handle = thread::spawn(move || {
        let mut stderr_text = String::new();
        let mut reader = BufReader::new(stderr);
        let _ = reader.read_to_string(&mut stderr_text);
        stderr_text
    });

    let mut stdout_reader = BufReader::new(stdout);
    let mut line_buf = String::new();
    loop {
        line_buf.clear();
        let n = stdout_reader
            .read_line(&mut line_buf)
            .with_context(|| format!("failed reading {backend_name} stdout"))?;
        if n == 0 {
            break;
        }
        let line_trim = line_buf.trim();
        if line_trim.is_empty() {
            continue;
        }
        on_stdout_line(line_trim)?;
    }

    let status = child
        .wait()
        .with_context(|| format!("failed waiting for {backend_name} process"))?;
    let stderr_text = stderr_handle.join().unwrap_or_default();

    if !status.success() {
        return Err(anyhow!(
            "{backend_name} turn failed with status {}\nstderr:\n{}",
            status,
            stderr_text
        ));
    }

    Ok(())
}

fn parse_assistant_text_from_content(content: &Value) -> Option<String> {
    let blocks = content.as_array()?;
    let mut text = String::new();
    for block in blocks {
        if block.get("type").and_then(|v| v.as_str()) == Some("text") {
            if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                text.push_str(t);
            }
        }
    }
    if text.is_empty() { None } else { Some(text) }
}

fn run_turn_codex(
    cfg: &Config,
    backend: &CodexBackendConfig,
    state: &RunState,
    prompt: &str,
    on_activity: &mut dyn FnMut() -> Result<()>,
) -> Result<TurnResult> {
    let mut cmd = Command::new(&backend.binary);
    cmd.current_dir(&cfg.workspace);
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

    let events_path = events_log_path(&cfg.state_dir);
    let mut parsed_thread_id: Option<String> = None;
    let mut final_response = String::new();

    run_backend_command_streaming(cmd, prompt, "codex", |line_trim| {
        append_event_line(&events_path, line_trim)?;
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
        on_activity()?;
        Ok(())
    })?;

    if final_response.is_empty() {
        final_response = "(no agent message captured)".to_string();
    }

    Ok(TurnResult {
        thread_id: parsed_thread_id,
        final_response,
    })
}

fn run_turn_claude(
    cfg: &Config,
    backend: &ClaudeBackendConfig,
    state: &RunState,
    prompt: &str,
    on_activity: &mut dyn FnMut() -> Result<()>,
) -> Result<TurnResult> {
    let effort = match backend.thinking.as_str() {
        "xhigh" => "high",
        other => other,
    };

    let mut cmd = Command::new(&backend.binary);
    cmd.current_dir(&cfg.workspace);
    cmd.arg("-p")
        .arg("--verbose")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--input-format")
        .arg("text")
        .arg("--model")
        .arg(&backend.model)
        .arg("--effort")
        .arg(effort)
        .arg("--dangerously-skip-permissions")
        .arg("--permission-mode")
        .arg("bypassPermissions")
        .arg("--add-dir")
        .arg(&cfg.workspace);

    for extra in &backend.extra_args {
        cmd.arg(extra);
    }

    if let Some(session_id) = &state.thread_id {
        cmd.arg("--resume").arg(session_id);
    }

    let events_path = events_log_path(&cfg.state_dir);
    let mut parsed_thread_id: Option<String> = None;
    let mut final_response = String::new();

    run_backend_command_streaming(cmd, prompt, "claude", |line_trim| {
        append_event_line(&events_path, line_trim)?;
        if let Ok(value) = serde_json::from_str::<Value>(line_trim) {
            if let Some(id) = value.get("session_id").and_then(|v| v.as_str()) {
                parsed_thread_id = Some(id.to_string());
            }

            match value.get("type").and_then(|v| v.as_str()) {
                Some("assistant") => {
                    if let Some(msg) = value.get("message") {
                        if let Some(content) = msg.get("content") {
                            if let Some(text) = parse_assistant_text_from_content(content) {
                                final_response = text;
                            }
                        }
                    }
                }
                Some("result") => {
                    if let Some(text) = value.get("result").and_then(|v| v.as_str()) {
                        final_response = text.to_string();
                    }
                }
                _ => {}
            }
        }
        on_activity()?;
        Ok(())
    })?;

    if final_response.is_empty() {
        final_response = "(no agent message captured)".to_string();
    }

    Ok(TurnResult {
        thread_id: parsed_thread_id,
        final_response,
    })
}

fn run_turn_droid(
    cfg: &Config,
    backend: &DroidBackendConfig,
    state: &RunState,
    prompt: &str,
    on_activity: &mut dyn FnMut() -> Result<()>,
) -> Result<TurnResult> {
    let effort = match backend.thinking.as_str() {
        "xhigh" => "max",
        other => other,
    };

    let mut cmd = Command::new(&backend.binary);
    cmd.current_dir(&cfg.workspace);
    cmd.arg("exec")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--input-format")
        .arg("text")
        .arg("--model")
        .arg(&backend.model)
        .arg("--reasoning-effort")
        .arg(effort)
        .arg("--auto")
        .arg(&backend.auto)
        .arg("--cwd")
        .arg(&cfg.workspace);

    for extra in &backend.extra_args {
        cmd.arg(extra);
    }

    if let Some(session_id) = &state.thread_id {
        cmd.arg("--session-id").arg(session_id);
    }

    let events_path = events_log_path(&cfg.state_dir);
    let mut parsed_thread_id: Option<String> = None;
    let mut final_response = String::new();

    run_backend_command_streaming(cmd, prompt, "droid", |line_trim| {
        append_event_line(&events_path, line_trim)?;
        if let Ok(value) = serde_json::from_str::<Value>(line_trim) {
            if let Some(id) = value.get("session_id").and_then(|v| v.as_str()) {
                parsed_thread_id = Some(id.to_string());
            }

            match value.get("type").and_then(|v| v.as_str()) {
                Some("message") => {
                    if value.get("role").and_then(|v| v.as_str()) == Some("assistant") {
                        if let Some(text) = value.get("text").and_then(|v| v.as_str()) {
                            final_response = text.to_string();
                        }
                    }
                }
                Some("completion") => {
                    if let Some(text) = value.get("finalText").and_then(|v| v.as_str()) {
                        final_response = text.to_string();
                    }
                }
                Some("result") => {
                    if let Some(text) = value.get("result").and_then(|v| v.as_str()) {
                        final_response = text.to_string();
                    }
                }
                _ => {}
            }
        }
        on_activity()?;
        Ok(())
    })?;

    if final_response.is_empty() {
        final_response = "(no agent message captured)".to_string();
    }

    Ok(TurnResult {
        thread_id: parsed_thread_id,
        final_response,
    })
}

fn run_turn_pi(
    cfg: &Config,
    backend: &PiBackendConfig,
    state: &RunState,
    prompt: &str,
    on_activity: &mut dyn FnMut() -> Result<()>,
) -> Result<TurnResult> {
    let mut cmd = Command::new(&backend.binary);
    cmd.current_dir(&cfg.workspace);
    cmd.arg("--print")
        .arg("--mode")
        .arg("json")
        .arg("--model")
        .arg(&backend.model)
        .arg("--thinking")
        .arg(&backend.thinking)
        .arg("--session-dir")
        .arg(cfg.state_dir.join("pi-sessions"))
        .arg("--no-extensions")
        .arg("--no-skills")
        .arg("--no-prompt-templates")
        .arg("--no-themes")
        .arg(prompt);

    if let Some(session_id) = &state.thread_id {
        cmd.arg("--session").arg(session_id);
    }

    if let Some(provider) = &backend.provider {
        cmd.arg("--provider").arg(provider);
    }

    for extra in &backend.extra_args {
        cmd.arg(extra);
    }

    let events_path = events_log_path(&cfg.state_dir);
    let mut parsed_thread_id: Option<String> = None;
    let mut final_response = String::new();

    run_backend_command_streaming(cmd, "", "pi", |line_trim| {
        append_event_line(&events_path, line_trim)?;
        if let Ok(value) = serde_json::from_str::<Value>(line_trim) {
            if value.get("type").and_then(|v| v.as_str()) == Some("session") {
                if let Some(id) = value.get("id").and_then(|v| v.as_str()) {
                    parsed_thread_id = Some(id.to_string());
                }
            }

            if value.get("type").and_then(|v| v.as_str()) == Some("message_end") {
                if let Some(msg) = value.get("message") {
                    if msg.get("role").and_then(|v| v.as_str()) == Some("assistant") {
                        if let Some(content) = msg.get("content") {
                            if let Some(text) = parse_assistant_text_from_content(content) {
                                final_response = text;
                            }
                        }
                    }
                }
            }
        }
        on_activity()?;
        Ok(())
    })?;

    if final_response.is_empty() {
        final_response = "(no agent message captured)".to_string();
    }

    Ok(TurnResult {
        thread_id: parsed_thread_id.or_else(|| state.thread_id.clone()),
        final_response,
    })
}

fn run_turn_mock(
    task: &TaskRuntime,
    backend: &MockBackendConfig,
    on_activity: &mut dyn FnMut() -> Result<()>,
) -> Result<TurnResult> {
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
    on_activity()?;

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
    on_activity: &mut dyn FnMut() -> Result<()>,
) -> Result<TurnResult> {
    match &cfg.backend {
        BackendConfig::Codex(codex) => run_turn_codex(cfg, codex, state, prompt, on_activity),
        BackendConfig::Claude(claude) => run_turn_claude(cfg, claude, state, prompt, on_activity),
        BackendConfig::Droid(droid) => run_turn_droid(cfg, droid, state, prompt, on_activity),
        BackendConfig::Pi(pi) => run_turn_pi(cfg, pi, state, prompt, on_activity),
        BackendConfig::Mock(mock) => run_turn_mock(task, mock, on_activity),
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
    ensure_log_files(&cfg.state_dir)?;
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
    let expected_reviewer_quorum = configured_reviewer_quorum(&cfg.roles);
    save_state(&mut state, &cfg.state_dir)?;

    loop {
        sync_completion_and_progress(&mut state);

        if all_terminal(&state) {
            state.status = RunStatus::Completed;
            save_state(&mut state, &cfg.state_dir)?;
            write_run_summary(&state, &cfg)?;
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
                write_run_summary(&state, &cfg)?;
                append_journal(
                    &journal,
                    "deadlock",
                    "No runnable pending task found; dependency graph may be invalid.",
                )?;
                break;
            }
        }

        let idx = active_idx.expect("active index must be set");
        if let Some(actual) = coord_reviewer_count(Path::new(&state.tasks[idx].coord_dir)) {
            if actual != expected_reviewer_quorum {
                let reason = format!(
                    "reviewer quorum mismatch: expected {} from configured team roles, but coord meta.env has REVIEWER_COUNT={}",
                    expected_reviewer_quorum, actual
                );
                append_journal(&journal, "task blocked reviewer quorum", &reason)?;
                let task = &mut state.tasks[idx];
                mark_task_blocked(task, &reason);
                save_state(&mut state, &cfg.state_dir)?;
                thread::sleep(Duration::from_secs(cfg.poll_interval_secs.max(1)));
                continue;
            }
        }

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
                        let reason =
                            format!("exceeded recovery attempts after {}s without progress", age);
                        mark_task_blocked(task, &reason);
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
        let state_snapshot = state.clone();
        let prompt = build_prompt(&cfg, &state, &task_snapshot, recovery_note.as_deref())?;

        state.cycle = state.cycle.saturating_add(1);
        state.last_turn_at = Some(now_iso());
        save_state(&mut state, &cfg.state_dir)?;

        let mut last_activity_state_save_epoch = 0i64;
        let mut on_activity = || -> Result<()> {
            let now = now_epoch();
            if let Some(task) = state.tasks.get_mut(idx) {
                task.last_progress_epoch = Some(now);
            }
            state.last_turn_at = Some(now_iso());
            if now.saturating_sub(last_activity_state_save_epoch) >= 5 {
                save_state(&mut state, &cfg.state_dir)?;
                last_activity_state_save_epoch = now;
            }
            Ok(())
        };

        let turn = run_turn(
            &cfg,
            &state_snapshot,
            &task_snapshot,
            &prompt,
            &mut on_activity,
        );
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

                let mut escalated_block_reason: Option<String> = None;
                if let Some(control) = extract_control_block(&turn_result.final_response) {
                    let control_status_raw = control.status.clone();
                    let control_status = control_status_raw.as_deref().unwrap_or("(missing)");
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

                    let handling = {
                        let task = &mut state.tasks[idx];
                        decide_unattended_escalate(
                            cfg.unattended,
                            cfg.policy.unattended_escalate,
                            task,
                            control_status_raw.as_deref(),
                            Some(&next_action),
                        )
                    };
                    match handling {
                        EscalateHandling::Ignore => {}
                        EscalateHandling::Retry => {
                            append_journal(
                                &journal,
                                "unattended escalate retry",
                                &format!(
                                    "Task {} requested ESCALATE. Applying best_effort_once retry path (attempt {}).",
                                    task_snapshot.id, state.tasks[idx].unattended_escalate_retries
                                ),
                            )?;
                        }
                        EscalateHandling::Block => {
                            escalated_block_reason = Some(format!(
                                "orchestrator requested ESCALATE in unattended mode (policy={})",
                                cfg.policy.unattended_escalate.as_str()
                            ));
                        }
                    }
                } else {
                    append_journal(
                        &journal,
                        "missing control block",
                        "No CONTROL_JSON block found in orchestrator response. Continuing.",
                    )?;
                }

                sync_completion_and_progress(&mut state);
                if let Some(reason) = escalated_block_reason {
                    let task = &mut state.tasks[idx];
                    if task.status != TaskStatus::Completed {
                        mark_task_blocked(task, &reason);
                        append_journal(&journal, "task blocked escalate policy", &reason)?;
                    }
                }
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
                    let reason = format!("hit {} consecutive turn failures", consecutive_failures);
                    mark_task_blocked(task, &reason);
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

fn toml_string(value: &str) -> String {
    format!("{value:?}")
}

fn toml_array(values: &[String]) -> String {
    let quoted: Vec<String> = values.iter().map(|v| toml_string(v)).collect();
    format!("[{}]", quoted.join(", "))
}

fn render_role_block(name: &str, role: &RoleConfig) -> String {
    format!(
        r#"[roles.{name}]
harness = {harness}
model = {model}
thinking = {thinking}
launch_args = {launch_args}
"#,
        harness = toml_string(&role.harness),
        model = toml_string(&role.model),
        thinking = toml_string(&role.thinking),
        launch_args = toml_array(&role.launch_args),
    )
}

fn write_default_config(output: &Path, roles: &RolesConfig) -> Result<()> {
    let content = format!(
        r#"run_id = "pika-call-plans"
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

[policy]
unattended_escalate = "best_effort_once"

[backend]
kind = "codex"
binary = "codex"
model = "gpt-5.3-codex"
thinking = "xhigh"
approval_policy = "never"
sandbox_mode = "danger-full-access"
extra_args = []

{implementer_role}
{reviewer_1_role}
{reviewer_2_role}

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
"#,
        implementer_role = render_role_block("implementer", &roles.implementer),
        reviewer_1_role = render_role_block("reviewer_1", &roles.reviewer_1),
        reviewer_2_role = render_role_block("reviewer_2", &roles.reviewer_2),
    );

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

fn resolve_team_roles(
    team: Option<&str>,
    team_file: Option<&Path>,
    teams_dir: &Path,
) -> Result<Option<RolesConfig>> {
    if team.is_some() && team_file.is_some() {
        return Err(anyhow!("use either --team or --team-file, not both"));
    }

    if let Some(path) = team_file {
        let loaded = load_team_from_file(path)?;
        return Ok(Some(loaded.roles));
    }

    if let Some(name) = team {
        let loaded = load_team(teams_dir, name)?;
        return Ok(Some(loaded.roles));
    }

    Ok(None)
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run(args) => {
            let mut cfg = load_config(&args.config)?;
            if let Some(team_roles) = resolve_team_roles(
                args.team.as_deref(),
                args.team_file.as_deref(),
                &args.teams_dir,
            )? {
                cfg.roles = team_roles;
            }
            validate_roles(&cfg.roles).with_context(|| {
                format!(
                    "invalid roles for run config {} (codex requires '{}' and claude requires '{}')",
                    args.config.display(),
                    REQUIRED_CODEX_ARG,
                    REQUIRED_CLAUDE_ARG
                )
            })?;
            run_governor(cfg)
        }
        Commands::Init(args) => {
            let roles = resolve_team_roles(
                args.team.as_deref(),
                args.team_file.as_deref(),
                &args.teams_dir,
            )?
            .unwrap_or_else(default_roles);
            validate_roles(&roles).with_context(|| {
                format!(
                    "invalid team roles for init output {} (codex requires '{}' and claude requires '{}')",
                    args.output.display(),
                    REQUIRED_CODEX_ARG,
                    REQUIRED_CLAUDE_ARG
                )
            })?;
            write_default_config(&args.output, &roles)?;
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
        Commands::Teams(args) => match args.command {
            TeamsCommand::List { dir } => cmd_teams_list(&dir),
            TeamsCommand::Validate(validate) => cmd_teams_validate(&validate),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::time::{SystemTime, UNIX_EPOCH};

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

    #[test]
    fn codex_role_requires_yolo() {
        let role = RoleConfig {
            harness: "codex".to_string(),
            model: "gpt-5.3-codex".to_string(),
            thinking: "xhigh".to_string(),
            launch_args: vec![],
        };
        let err = validate_role("implementer", &role).expect_err("should require --yolo");
        assert!(err.to_string().contains(REQUIRED_CODEX_ARG));
    }

    #[test]
    fn builtin_team_xhigh_is_valid() {
        let team = builtin_team("xhigh").expect("xhigh should exist");
        validate_roles(&team.roles).expect("xhigh roles must validate");
    }

    #[test]
    fn lock_guard_breaks_stale_lock() {
        let state_dir = make_temp_dir("lock-stale");
        let lock_path = state_dir.join("run.lock");
        fs::write(&lock_path, "pid=999999\n").expect("write stale lock");

        let guard = LockGuard::acquire(&state_dir).expect("should recover stale lock");
        let lock_text = fs::read_to_string(&lock_path).expect("read recovered lock");
        assert!(lock_text.contains("pid="));
        drop(guard);
        assert!(!lock_path.exists(), "lock should be removed on drop");
    }

    #[test]
    fn lock_guard_keeps_live_lock() {
        let state_dir = make_temp_dir("lock-live");
        let lock_path = state_dir.join("run.lock");
        fs::write(&lock_path, format!("pid={}\n", std::process::id())).expect("write live lock");

        match LockGuard::acquire(&state_dir) {
            Ok(_guard) => panic!("live lock should fail acquire"),
            Err(err) => assert!(err.to_string().contains("could not acquire lock")),
        }
    }

    #[test]
    fn reviewer_quorum_derived_from_roles() {
        let roles = default_roles();
        assert_eq!(configured_reviewer_quorum(&roles), 2);
    }

    #[test]
    fn coord_reviewer_count_parses_meta_env() {
        let coord_dir = make_temp_dir("coord-meta");
        fs::write(coord_dir.join("meta.env"), "REVIEWER_COUNT=2\n").expect("write meta.env");
        assert_eq!(coord_reviewer_count(&coord_dir), Some(2));
    }

    #[test]
    fn escalate_policy_strict_blocks_immediately() {
        let mut task = TaskRuntime {
            id: "t1".to_string(),
            todo_file: "todo.md".to_string(),
            depends_on: Vec::new(),
            status: TaskStatus::Running,
            coord_dir: "/tmp/coord".to_string(),
            completion_file: None,
            started_at: None,
            completed_at: None,
            blocked_reason: None,
            last_progress_epoch: None,
            recovery_attempts: 0,
            unattended_escalate_retries: 0,
        };

        let decision = decide_unattended_escalate(
            true,
            UnattendedEscalatePolicy::Strict,
            &mut task,
            None,
            Some("ESCALATE"),
        );
        assert_eq!(decision, EscalateHandling::Block);
        assert_eq!(task.unattended_escalate_retries, 0);
    }

    #[test]
    fn escalate_policy_best_effort_once_then_blocks() {
        let mut task = TaskRuntime {
            id: "t2".to_string(),
            todo_file: "todo.md".to_string(),
            depends_on: Vec::new(),
            status: TaskStatus::Running,
            coord_dir: "/tmp/coord".to_string(),
            completion_file: None,
            started_at: None,
            completed_at: None,
            blocked_reason: None,
            last_progress_epoch: None,
            recovery_attempts: 0,
            unattended_escalate_retries: 0,
        };

        let first = decide_unattended_escalate(
            true,
            UnattendedEscalatePolicy::BestEffortOnce,
            &mut task,
            None,
            Some("ESCALATE"),
        );
        assert_eq!(first, EscalateHandling::Retry);
        assert_eq!(task.unattended_escalate_retries, 1);

        let second = decide_unattended_escalate(
            true,
            UnattendedEscalatePolicy::BestEffortOnce,
            &mut task,
            None,
            Some("ESCALATE"),
        );
        assert_eq!(second, EscalateHandling::Block);
    }

    #[test]
    fn escalate_policy_best_effort_once_uses_blocked_status() {
        let mut task = TaskRuntime {
            id: "t3".to_string(),
            todo_file: "todo.md".to_string(),
            depends_on: Vec::new(),
            status: TaskStatus::Running,
            coord_dir: "/tmp/coord".to_string(),
            completion_file: None,
            started_at: None,
            completed_at: None,
            blocked_reason: None,
            last_progress_epoch: None,
            recovery_attempts: 0,
            unattended_escalate_retries: 0,
        };

        let first = decide_unattended_escalate(
            true,
            UnattendedEscalatePolicy::BestEffortOnce,
            &mut task,
            Some("blocked"),
            Some("wait for user sign-off"),
        );
        assert_eq!(first, EscalateHandling::Retry);
        assert_eq!(task.unattended_escalate_retries, 1);

        let second = decide_unattended_escalate(
            true,
            UnattendedEscalatePolicy::BestEffortOnce,
            &mut task,
            Some("blocked"),
            Some("wait for user sign-off"),
        );
        assert_eq!(second, EscalateHandling::Block);
    }

    #[test]
    fn non_escalate_control_is_ignored() {
        let mut task = TaskRuntime {
            id: "t4".to_string(),
            todo_file: "todo.md".to_string(),
            depends_on: Vec::new(),
            status: TaskStatus::Running,
            coord_dir: "/tmp/coord".to_string(),
            completion_file: None,
            started_at: None,
            completed_at: None,
            blocked_reason: None,
            last_progress_epoch: None,
            recovery_attempts: 0,
            unattended_escalate_retries: 0,
        };

        let decision = decide_unattended_escalate(
            true,
            UnattendedEscalatePolicy::BestEffortOnce,
            &mut task,
            Some("in_progress"),
            Some("continue"),
        );
        assert_eq!(decision, EscalateHandling::Ignore);
        assert_eq!(task.unattended_escalate_retries, 0);
    }

    fn make_temp_dir(prefix: &str) -> PathBuf {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock must be after epoch")
            .as_millis();
        let pid = std::process::id();
        let dir = env::temp_dir().join(format!("crank-{prefix}-{pid}-{ts}"));
        fs::create_dir_all(&dir).expect("failed to create temp dir");
        dir
    }

    fn local_smoke_run(backend: BackendConfig) -> Result<TurnResult> {
        let state_dir = make_temp_dir("local-e2e");
        let workspace = env::current_dir().context("failed to get current dir")?;
        fs::create_dir_all(state_dir.join("logs")).context("failed to create logs dir")?;
        fs::create_dir_all(state_dir.join("coord")).context("failed to create coord dir")?;

        let cfg = Config {
            run_id: Some("local-e2e".to_string()),
            workspace: workspace.clone(),
            state_dir: state_dir.clone(),
            unattended: true,
            poll_interval_secs: 1,
            timeouts: TimeoutsConfig { stall_secs: 900 },
            recovery: RecoveryConfig::default(),
            policy: PolicyConfig::default(),
            backend,
            roles: default_roles(),
            tasks: Vec::new(),
        };

        let state = RunState {
            run_id: "local-e2e".to_string(),
            workspace: workspace.display().to_string(),
            state_dir: state_dir.display().to_string(),
            unattended: true,
            status: RunStatus::Running,
            started_at: now_iso(),
            updated_at: now_iso(),
            journal_path: journal_path(&state_dir).display().to_string(),
            thread_id: None,
            cycle: 0,
            last_turn_at: None,
            tasks: Vec::new(),
        };

        let task = TaskRuntime {
            id: "smoke".to_string(),
            todo_file: "N/A".to_string(),
            depends_on: Vec::new(),
            status: TaskStatus::Running,
            coord_dir: state_dir.join("coord").join("smoke").display().to_string(),
            completion_file: None,
            started_at: None,
            completed_at: None,
            blocked_reason: None,
            last_progress_epoch: None,
            recovery_attempts: 0,
            unattended_escalate_retries: 0,
        };

        let mut on_activity = || -> Result<()> { Ok(()) };
        run_turn(
            &cfg,
            &state,
            &task,
            "Respond with a one-line greeting and include the token CRANK_LOCAL_SMOKE.",
            &mut on_activity,
        )
    }

    #[test]
    #[ignore = "local e2e; requires authenticated claude CLI"]
    fn local_e2e_claude_backend_smoke() {
        let result = local_smoke_run(BackendConfig::Claude(ClaudeBackendConfig {
            binary: "claude".to_string(),
            model: "claude-opus-4-6".to_string(),
            thinking: "high".to_string(),
            extra_args: Vec::new(),
        }))
        .expect("claude local smoke should succeed");
        assert!(!result.final_response.trim().is_empty());
    }

    #[test]
    #[ignore = "local e2e; requires authenticated droid CLI"]
    fn local_e2e_droid_backend_smoke() {
        let result = local_smoke_run(BackendConfig::Droid(DroidBackendConfig {
            binary: "droid".to_string(),
            model: "claude-opus-4-6".to_string(),
            thinking: "high".to_string(),
            auto: "high".to_string(),
            extra_args: Vec::new(),
        }))
        .expect("droid local smoke should succeed");
        assert!(!result.final_response.trim().is_empty());
    }

    #[test]
    #[ignore = "local e2e; requires authenticated pi CLI"]
    fn local_e2e_pi_backend_smoke() {
        let result = local_smoke_run(BackendConfig::Pi(PiBackendConfig {
            binary: "pi".to_string(),
            model: "claude-opus-4-6".to_string(),
            thinking: "high".to_string(),
            provider: Some("anthropic".to_string()),
            extra_args: Vec::new(),
        }))
        .expect("pi local smoke should succeed");
        assert!(!result.final_response.trim().is_empty());
    }
}
