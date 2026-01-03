use anyhow::Result;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use crate::git::ReviewStepResult;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

const REVIEW_PROMPT: &str = r#"You are reviewing code changes for merge into master. You are READ-ONLY - do not modify any files.

## Instructions

1. Read AGENTS.md and CLAUDE.md for project context
2. Find the task being worked on:
   - If `.issues/.current` exists, open the referenced `.issues/<id>.md`
   - Otherwise, scan `.issues/*.md` for `status: in_progress` and use the most recent
   - If none found, proceed using only the diff and user request context
3. Review the diff: `git diff master...HEAD`
4. {test_instructions}
5. Evaluate:
   - Does the code fulfill the task requirements?
   - Do tests pass? (if run)
   - Any obvious bugs or security issues?

## Rules
- NO NITS - only flag problems that break functionality or security
- Tests must pass (if run)
- Code must match stated requirements

## Output
Your response MUST start with exactly one of:
- PASS
- FAIL: <reason under 200 chars>

Optional: add context as bullets after the first line."#;

pub struct ReviewResult {
    pub status: String,
    pub reason: Option<String>,
}

/// Send a prompt using the opencode CLI with the review agent.
/// Each review spawns its own opencode process in the target directory.
/// If progress is provided, streams events to the merge log.
async fn send_review_prompt(
    directory: &str,
    prompt: &str,
    timeout_ms: u64,
    progress: Option<Arc<Mutex<MergeProgress>>>,
) -> Result<String> {
    let mut child = Command::new("opencode")
        .arg("run")
        .arg("--format")
        .arg("json")
        .arg("--agent")
        .arg("review")
        .arg(prompt)
        .current_dir(directory)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout).lines();

    let mut all_output = Vec::new();
    let mut session_id_captured = false;

    let read_output = async {
        while let Ok(Some(line)) = reader.next_line().await {
            // Extract session ID from first event
            if !session_id_captured {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) {
                    if let Some(sid) = json.get("sessionID").and_then(|v| v.as_str()) {
                        if let Some(ref p) = progress {
                            let mut guard = p.lock().await;
                            guard.set_session_id("review", sid);
                        }
                        session_id_captured = true;
                    }
                }
            }

            // Log to progress if available
            if let Some(ref p) = progress {
                let summary = summarize_opencode_event(&line);
                if !summary.is_empty() {
                    let mut guard = p.lock().await;
                    guard.append_output("review", &summary);
                }
            }
            all_output.push(line);
        }
        child.wait().await
    };

    let result = tokio::time::timeout(Duration::from_millis(timeout_ms), read_output).await;

    match result {
        Ok(Ok(status)) => {
            if !status.success() {
                return Err(anyhow::anyhow!(
                    "opencode run failed with exit code: {:?}",
                    status.code()
                ));
            }
            let stdout = all_output.join("\n");
            let response = extract_response_text(&stdout);
            if response.trim().is_empty() {
                return Err(anyhow::anyhow!("opencode run returned empty response"));
            }
            Ok(response)
        }
        Ok(Err(e)) => Err(anyhow::anyhow!("opencode run failed: {}", e)),
        Err(_) => {
            let _ = child.kill().await;
            Err(anyhow::anyhow!("opencode run timed out"))
        }
    }
}

/// Summarize an opencode JSON event for logging
fn summarize_opencode_event(json_line: &str) -> String {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(json_line) {
        let event_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match event_type {
            "step_start" => "--- review step started ---".to_string(),
            "step_finish" => {
                let reason = json
                    .pointer("/part/reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("done");
                format!("--- step finished: {} ---", reason)
            }
            "tool_use" => {
                let tool = json
                    .pointer("/part/tool")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let state = json
                    .pointer("/part/state/status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let input = json.pointer("/part/state/input");

                if state == "pending" {
                    if let Some(input) = input {
                        if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
                            return format!(
                                "[{}] $ {}",
                                tool,
                                cmd.chars().take(100).collect::<String>()
                            );
                        }
                        if let Some(pattern) = input.get("pattern").and_then(|v| v.as_str()) {
                            return format!("[{}] searching: {}", tool, pattern);
                        }
                        if let Some(path) = input.get("filePath").and_then(|v| v.as_str()) {
                            return format!("[{}] reading: {}", tool, path);
                        }
                        if let Some(desc) = input.get("description").and_then(|v| v.as_str()) {
                            return format!("[{}] {}", tool, desc);
                        }
                    }
                    format!("[{}] ...", tool)
                } else if state == "completed" {
                    // Show brief output summary for completed tools
                    if let Some(output) =
                        json.pointer("/part/state/output").and_then(|v| v.as_str())
                    {
                        let lines: Vec<&str> = output.lines().collect();
                        if lines.len() > 1 {
                            return format!("[{}] done ({} lines)", tool, lines.len());
                        } else if !output.trim().is_empty() {
                            let short: String = output.trim().chars().take(80).collect();
                            return format!("[{}] -> {}", tool, short);
                        }
                    }
                    String::new()
                } else {
                    String::new()
                }
            }
            "text" => {
                // Extract actual text content - this is the model's response
                if let Some(text) = json.pointer("/part/text").and_then(|v| v.as_str()) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        // Show each line of the response
                        return trimmed
                            .lines()
                            .map(|line| format!(">>> {}", line))
                            .collect::<Vec<_>>()
                            .join("\n");
                    }
                }
                String::new()
            }
            _ => String::new(),
        }
    } else {
        String::new()
    }
}

/// Extract text from opencode json events
fn extract_response_text(json_events: &str) -> String {
    let mut parts = vec![];

    for line in json_events.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) {
            let text = collect_text_from_value(&json);
            if !text.is_empty() {
                parts.push(text);
            }
        }
    }

    parts.join("")
}

fn collect_text_from_value(value: &serde_json::Value) -> String {
    let mut parts = vec![];

    if let Some(text) = value.get("text").and_then(|v| v.as_str()) {
        parts.push(text.to_string());
    }

    if let Some(text) = value
        .get("part")
        .and_then(|p| p.get("text"))
        .and_then(|v| v.as_str())
    {
        parts.push(text.to_string());
    }

    if let Some(items) = value.get("parts").and_then(|v| v.as_array()) {
        for item in items {
            let text = collect_text_from_value(item);
            if !text.is_empty() {
                parts.push(text);
            }
        }
    }

    if let Some(items) = value.get("data").and_then(|v| v.as_array()) {
        for item in items {
            let text = collect_text_from_value(item);
            if !text.is_empty() {
                parts.push(text);
            }
        }
    }

    parts.join("")
}

/// Parse review output for PASS/FAIL
fn parse_review_output(output: &str) -> ReviewResult {
    let first_line = output.lines().next().unwrap_or("").trim();

    if first_line == "PASS" {
        return ReviewResult {
            status: "pass".to_string(),
            reason: None,
        };
    }

    if let Some(reason) = first_line.strip_prefix("FAIL:") {
        return ReviewResult {
            status: "fail".to_string(),
            reason: Some(reason.trim().to_string()),
        };
    }

    // Try to find PASS/FAIL anywhere
    if output.contains("PASS") {
        return ReviewResult {
            status: "pass".to_string(),
            reason: None,
        };
    }

    if output.contains("FAIL:") {
        if let Some(start) = output.find("FAIL:") {
            let rest = &output[start + 5..];
            let reason = rest.lines().next().unwrap_or("").trim();
            return ReviewResult {
                status: "fail".to_string(),
                reason: Some(reason.to_string()),
            };
        }
    }

    ReviewResult {
        status: "fail".to_string(),
        reason: Some("Could not parse review output".to_string()),
    }
}

use crate::git::MergeProgress;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Run a review using opencode's review agent.
/// Each review spawns its own opencode process - no shared server needed.
pub async fn run_review(
    git_root: &Path,
    _branch: &str,
    skip_tests: bool,
    timeout_ms: u64,
    progress: Option<Arc<Mutex<MergeProgress>>>,
) -> ReviewStepResult {
    let start = std::time::Instant::now();
    let directory = git_root.to_string_lossy();

    if let Some(ref p) = progress {
        let mut guard = p.lock().await;
        guard.start_step("review");
    }

    let test_instructions = if skip_tests {
        "Tests have already been run by pre-merge, skip running tests."
    } else {
        "Run tests with `just test` or appropriate test command."
    };

    let prompt = REVIEW_PROMPT.replace("{test_instructions}", test_instructions);

    let result = send_review_prompt(&directory, &prompt, timeout_ms, progress.clone()).await;

    let duration_ms = start.elapsed().as_millis() as u64;

    let step_result = match result {
        Ok(response) => {
            let review = parse_review_output(&response);
            if let Some(ref p) = progress {
                let mut guard = p.lock().await;
                guard.finish_step("review", &review.status, review.reason.clone());
            }
            ReviewStepResult::new("review", &review.status, review.reason, Some(duration_ms))
        }
        Err(e) => {
            if let Some(ref p) = progress {
                let mut guard = p.lock().await;
                guard.finish_step("review", "fail", Some(e.to_string()));
            }
            ReviewStepResult::new("review", "fail", Some(e.to_string()), Some(duration_ms))
        }
    };

    step_result
}

/// Review command (standalone)
pub async fn review_command(worktree: &str, skip_tests: bool, timeout_ms: u64) -> Result<()> {
    let worktree_path = std::fs::canonicalize(worktree)?;
    let git_root = crate::git::get_git_root(&worktree_path).await?;
    let branch = crate::git::get_current_branch(&git_root).await?;

    let result = run_review(&git_root, &branch, skip_tests, timeout_ms, None).await;

    println!(
        "{}",
        serde_json::json!({
            "status": result.status,
            "reason": result.tail,
        })
    );

    if result.status == "fail" {
        std::process::exit(1);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_review_output_pass() {
        let result = parse_review_output("PASS\n- looks good");
        assert_eq!(result.status, "pass");
        assert!(result.reason.is_none());
    }

    #[test]
    fn test_parse_review_output_fail() {
        let result = parse_review_output("FAIL: missing tests for new function");
        assert_eq!(result.status, "fail");
        assert_eq!(
            result.reason,
            Some("missing tests for new function".to_string())
        );
    }

    #[test]
    fn test_extract_response_text() {
        let json = r#"{"type":"text","part":{"text":"PASS"}}
{"type":"text","part":{"text":"\n- looks good"}}"#;
        let result = extract_response_text(json);
        assert!(result.contains("PASS"));
    }
}
