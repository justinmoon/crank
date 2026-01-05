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
   - If `.crank/.current` exists, open the referenced `.crank/<id>.md`
   - Otherwise, scan `.crank/*.md` for `status: in_progress` and use the most recent
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
    pub details: Option<String>,
}

/// Send a prompt using the opencode CLI with the review agent.
/// Each review spawns its own opencode process in the target directory.
async fn send_review_prompt(directory: &str, prompt: &str, timeout_ms: u64) -> Result<String> {
    let mut child = Command::new("opencode")
        .arg("run")
        .arg("--format")
        .arg("json")
        .arg("--agent")
        // Opencode's built-in agent list can vary by install.
        // "review" doesn't exist in some environments, but "general" does.
        .arg("general")
        .arg(prompt)
        .current_dir(directory)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    let read_stdout = async move {
        let mut reader = BufReader::new(stdout).lines();
        let mut lines = Vec::new();
        while let Ok(Some(line)) = reader.next_line().await {
            lines.push(line);
        }
        lines
    };

    let read_stderr = async move {
        let mut reader = BufReader::new(stderr).lines();
        let mut lines = Vec::new();
        while let Ok(Some(line)) = reader.next_line().await {
            lines.push(line);
        }
        lines
    };

    let read_output = async {
        let (stdout_lines, stderr_lines, status) =
            tokio::join!(read_stdout, read_stderr, child.wait());
        (stdout_lines, stderr_lines, status)
    };

    let result = tokio::time::timeout(Duration::from_millis(timeout_ms), read_output).await;

    match result {
        Ok((stdout_lines, stderr_lines, Ok(status))) => {
            let mut combined = Vec::with_capacity(stdout_lines.len() + stderr_lines.len());
            combined.extend(stdout_lines);
            combined.extend(stderr_lines);
            let output = combined.join("\n");

            if !status.success() {
                return Err(anyhow::anyhow!(
                    "opencode run failed with exit code: {:?}\n{}",
                    status.code(),
                    output
                ));
            }

            let response = extract_response_text(&output);
            if response.trim().is_empty() {
                // Sometimes opencode prints non-JSON text despite `--format json`.
                // In that case, fall back to the raw output so PASS/FAIL parsing can still work.
                if output.trim().is_empty() {
                    return Err(anyhow::anyhow!("opencode run returned empty response"));
                }
                return Ok(output);
            }

            Ok(response)
        }
        Ok((_stdout_lines, _stderr_lines, Err(e))) => {
            Err(anyhow::anyhow!("opencode run failed: {}", e))
        }
        Err(_) => {
            let _ = child.kill().await;
            Err(anyhow::anyhow!("opencode run timed out"))
        }
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

        let payload = trimmed
            .strip_prefix("data:")
            .map(str::trim)
            .unwrap_or(trimmed);
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }

        if let Ok(json) = serde_json::from_str::<serde_json::Value>(payload) {
            let text = collect_text_from_value(&json);
            if !text.is_empty() {
                parts.push(text);
            }
        }
    }

    parts.join("")
}

fn collect_text_from_value(value: &serde_json::Value) -> String {
    let mut parts = Vec::new();

    match value {
        serde_json::Value::String(text) => {
            parts.push(text.clone());
        }
        serde_json::Value::Array(items) => {
            for item in items {
                let text = collect_text_from_value(item);
                if !text.is_empty() {
                    parts.push(text);
                }
            }
        }
        serde_json::Value::Object(map) => {
            for (key, item) in map {
                if key == "text" {
                    if let Some(text) = item.as_str() {
                        parts.push(text.to_string());
                        continue;
                    }
                }

                let text = collect_text_from_value(item);
                if !text.is_empty() {
                    parts.push(text);
                }
            }
        }
        _ => {}
    }

    parts.join("")
}

/// Parse review output for PASS/FAIL
fn parse_review_output(output: &str) -> ReviewResult {
    if let Some(parsed) = parse_review_json(output) {
        return parsed;
    }

    let mut lines = output.lines();
    let first_line = lines.next().unwrap_or("").trim();
    let rest = lines.collect::<Vec<_>>().join("\n");
    let rest_trimmed = rest.trim();
    let rest_details = if rest_trimmed.is_empty() {
        None
    } else {
        Some(rest_trimmed.to_string())
    };
    let full_trimmed = output.trim();
    let full_details = if full_trimmed.is_empty() {
        None
    } else {
        Some(full_trimmed.to_string())
    };
    let first_line_upper = first_line.to_ascii_uppercase();

    if first_line_upper == "PASS" || first_line_upper.starts_with("PASS") {
        return ReviewResult {
            status: "pass".to_string(),
            reason: None,
            details: rest_details,
        };
    }

    if first_line_upper.starts_with("FAIL:") {
        let reason = first_line
            .split_once(':')
            .map(|(_, rest)| rest)
            .unwrap_or("")
            .trim();
        return ReviewResult {
            status: "fail".to_string(),
            reason: Some(reason.to_string()),
            details: rest_details,
        };
    }

    let output_upper = output.to_ascii_uppercase();

    // Try to find PASS/FAIL anywhere
    if output_upper.contains("PASS") {
        return ReviewResult {
            status: "pass".to_string(),
            reason: None,
            details: full_details.clone(),
        };
    }

    if let Some(start) = output_upper.find("FAIL:") {
        let rest = &output[start + 5..];
        let reason = rest.lines().next().unwrap_or("").trim();
        return ReviewResult {
            status: "fail".to_string(),
            reason: Some(reason.to_string()),
            details: full_details.clone(),
        };
    }

    ReviewResult {
        status: "fail".to_string(),
        reason: Some("Could not parse review output".to_string()),
        details: full_details,
    }
}

fn parse_review_json(output: &str) -> Option<ReviewResult> {
    let mut candidates = Vec::new();
    candidates.extend(
        output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty()),
    );
    candidates.push(output.trim());

    for candidate in candidates {
        if candidate.is_empty() {
            continue;
        }
        let Ok(json) = serde_json::from_str::<serde_json::Value>(candidate) else {
            continue;
        };
        let Some(status_value) = json.get("status").and_then(|value| value.as_str()) else {
            continue;
        };
        let status = status_value.trim().to_ascii_lowercase();
        let details = json
            .get("details")
            .and_then(|value| value.as_str())
            .or_else(|| json.get("output").and_then(|value| value.as_str()))
            .map(|value| value.to_string());
        if status == "pass" {
            return Some(ReviewResult {
                status: "pass".to_string(),
                reason: None,
                details,
            });
        }
        if status == "fail" {
            let reason = json
                .get("reason")
                .and_then(|value| value.as_str())
                .or_else(|| json.get("message").and_then(|value| value.as_str()))
                .unwrap_or("review failed");
            return Some(ReviewResult {
                status: "fail".to_string(),
                reason: Some(reason.to_string()),
                details,
            });
        }
    }
    None
}

/// Run a review using opencode's review agent.
/// Each review spawns its own opencode process - no shared server needed.
pub async fn run_review(
    git_root: &Path,
    _branch: &str,
    skip_tests: bool,
    timeout_ms: u64,
) -> ReviewStepResult {
    let start = std::time::Instant::now();
    let directory = git_root.to_string_lossy();

    let test_instructions = if skip_tests {
        "Tests have already been run by pre-merge, skip running tests."
    } else {
        "Run tests with `just test` or appropriate test command."
    };

    let prompt = REVIEW_PROMPT.replace("{test_instructions}", test_instructions);

    let result = send_review_prompt(&directory, &prompt, timeout_ms).await;

    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(response) => {
            let review = parse_review_output(&response);
            ReviewStepResult::new(
                "review",
                &review.status,
                review.reason,
                review.details,
                Some(duration_ms),
            )
        }
        Err(e) => ReviewStepResult::new(
            "review",
            "fail",
            Some(e.to_string()),
            None,
            Some(duration_ms),
        ),
    }
}

/// Review command (standalone)
pub async fn review_command(worktree: &str, skip_tests: bool, timeout_ms: u64) -> Result<()> {
    let worktree_path = std::fs::canonicalize(worktree)?;
    let git_root = crate::git::get_git_root(&worktree_path).await?;
    let branch = crate::git::get_current_branch(&git_root).await?;

    let result = run_review(&git_root, &branch, skip_tests, timeout_ms).await;

    println!(
        "{}",
        serde_json::json!({
            "status": result.status,
            "reason": result.tail,
            "details": result.details,
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
