use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use chrono::Local;
use serde::{Deserialize, Serialize};

use crate::crank_io;
use crate::task::store;

pub mod inbox;
pub mod cli;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TutorialIndexEntry {
    pub id: String,
    pub title: String,
    pub issue_ids: Vec<String>,
    pub created_at: String,
    pub merge_commit: String,
    pub base_branch: String,
    pub source_branch: String,
    pub status: String,
    pub steps: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TutorialManifest {
    pub id: String,
    pub title: String,
    pub issue_ids: Vec<String>,
    pub created_at: String,
    pub merge_commit: String,
    pub base_branch: String,
    pub source_branch: String,
    pub status: String,
    pub workflow_id: Option<String>,
    pub steps: Vec<TutorialStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TutorialStep {
    pub index: u32,
    pub commit: String,
    pub subject: String,
    pub files: Vec<String>,
    pub note: String,
    pub diff: String,
}

#[derive(Debug, Clone)]
pub struct TutorialGenerateOptions {
    pub worktree: PathBuf,
    pub base_branch: String,
    pub merge_commit: Option<String>,
    pub workflow_id: Option<String>,
    pub output_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct TutorialFull {
    pub manifest: TutorialManifest,
    pub issue: String,
    pub summary: String,
    pub steps: Vec<TutorialStepContent>,
}

#[derive(Debug, Clone)]
pub struct TutorialStepContent {
    pub step: TutorialStep,
    pub note: String,
    pub diff: String,
}

pub fn generate_tutorial(options: &TutorialGenerateOptions) -> Result<String> {
    let worktree = canonicalize_dir(&options.worktree)?;
    let repo_root = repo_root_from(&worktree)?;
    let output_root = options
        .output_dir
        .clone()
        .map(|path| resolve_path(&repo_root, &path))
        .unwrap_or_else(|| tutorials_dir(&repo_root));

    store::ensure_git_exclude(&repo_root, ".crank/tutorials/")?;
    crank_io::ensure_dir(&output_root)?;

    let base = options.base_branch.trim().to_string();
    let source_branch = git_output(&worktree, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let merge_commit = match &options.merge_commit {
        Some(commit) => commit.clone(),
        None => git_output(&repo_root, &["rev-parse", &base])?,
    };
    let merge_commit_short = git_output(&repo_root, &["rev-parse", "--short", &merge_commit])?;

    let tutorial_id = format!(
        "merge-{}-{}",
        sanitize_id(&source_branch),
        merge_commit_short
    );
    let tutorial_dir = output_root.join(&tutorial_id);
    if tutorial_dir.exists() {
        return Err(anyhow!(
            "tutorial already exists: {}",
            tutorial_dir.display()
        ));
    }

    crank_io::ensure_dir(&tutorial_dir)?;
    crank_io::ensure_dir(&tutorial_dir.join("steps"))?;

    let issue_ids = load_issue_ids(&worktree);
    let (mut title, issue_content) = load_issue_content(&worktree, &repo_root, &issue_ids);

    let range = resolve_commit_range(&repo_root, &merge_commit, &base)?;
    let commits = load_commits(&repo_root, &range)?;

    let summary = build_summary(
        &repo_root,
        &range,
        &merge_commit_short,
        &source_branch,
        &base,
        &commits,
    )?;
    if title.trim().is_empty() {
        title = derive_title(&repo_root, &commits, &source_branch, &base)?;
    }
    let summary_path = tutorial_dir.join("summary.md");
    crank_io::write_string(&summary_path, &summary)?;

    let issue_path = tutorial_dir.join("issue.md");
    crank_io::write_string(&issue_path, &issue_content)?;

    let steps = write_steps(&repo_root, &tutorial_dir, &commits)?;

    let created_at = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let manifest = TutorialManifest {
        id: tutorial_id.clone(),
        title: title.clone(),
        issue_ids: issue_ids.clone(),
        created_at: created_at.clone(),
        merge_commit: merge_commit.clone(),
        base_branch: base.clone(),
        source_branch: source_branch.clone(),
        status: "unread".to_string(),
        workflow_id: options.workflow_id.clone(),
        steps: steps.clone(),
    };

    let manifest_path = tutorial_dir.join("tutorial.json");
    write_manifest(&manifest_path, &manifest)?;

    let mut index = load_index_at(&output_root)?;
    index.retain(|entry| entry.id != tutorial_id);
    index.push(TutorialIndexEntry {
        id: tutorial_id.clone(),
        title,
        issue_ids,
        created_at,
        merge_commit,
        base_branch: base,
        source_branch,
        status: "unread".to_string(),
        steps: steps.len(),
    });
    index.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    save_index_at(&output_root, &index)?;

    println!("Generated tutorial: {tutorial_id}");
    Ok(tutorial_id)
}

pub fn show_tutorial(
    repo_root: &Path,
    id: &str,
    format: &str,
    step: Option<usize>,
) -> Result<()> {
    let full = load_full_tutorial(repo_root, id)?;
    match format {
        "json" => {
            let output = TutorialOutput::from_full(&full, step)?;
            let json = serde_json::to_string_pretty(&output)?;
            println!("{json}");
        }
        _ => {
            let text = render_markdown(&full, step)?;
            print!("{text}");
        }
    }
    Ok(())
}

pub fn load_full_tutorial(repo_root: &Path, id: &str) -> Result<TutorialFull> {
    let tutorial_dir = tutorials_dir(repo_root).join(id);
    if !tutorial_dir.exists() {
        return Err(anyhow!("tutorial not found: {}", tutorial_dir.display()));
    }

    let manifest_path = tutorial_dir.join("tutorial.json");
    let manifest = read_manifest(&manifest_path)?;
    let issue = read_optional_file(tutorial_dir.join("issue.md"))?;
    let summary = read_optional_file(tutorial_dir.join("summary.md"))?;

    let mut steps = Vec::new();
    for step in &manifest.steps {
        let note = read_optional_file(tutorial_dir.join(&step.note))?;
        let diff = read_optional_file(tutorial_dir.join(&step.diff))?;
        steps.push(TutorialStepContent {
            step: step.clone(),
            note,
            diff,
        });
    }

    Ok(TutorialFull {
        manifest,
        issue,
        summary,
        steps,
    })
}

pub fn load_index(repo_root: &Path) -> Result<Vec<TutorialIndexEntry>> {
    let root = tutorials_dir(repo_root);
    load_index_at(&root)
}

#[allow(dead_code)]
pub fn save_index(repo_root: &Path, entries: &[TutorialIndexEntry]) -> Result<()> {
    let root = tutorials_dir(repo_root);
    save_index_at(&root, entries)
}

pub fn set_tutorial_status(repo_root: &Path, id: &str, status: &str) -> Result<()> {
    let root = tutorials_dir(repo_root);
    let tutorial_dir = root.join(id);
    if !tutorial_dir.exists() {
        return Err(anyhow!("tutorial not found: {}", tutorial_dir.display()));
    }

    let manifest_path = tutorial_dir.join("tutorial.json");
    let mut manifest = read_manifest(&manifest_path)?;
    manifest.status = status.to_string();
    write_manifest(&manifest_path, &manifest)?;

    let mut index = load_index_at(&root)?;
    if let Some(entry) = index.iter_mut().find(|entry| entry.id == id) {
        entry.status = status.to_string();
    }
    save_index_at(&root, &index)?;
    Ok(())
}

pub fn load_index_at(root: &Path) -> Result<Vec<TutorialIndexEntry>> {
    let index_path = root.join("index.json");
    if !index_path.exists() {
        return rebuild_index(root);
    }
    let raw = crank_io::read_to_string(&index_path)
        .with_context(|| format!("failed to read {}", index_path.display()))?;
    let entries: Vec<TutorialIndexEntry> = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", index_path.display()))?;
    Ok(entries)
}

pub fn rebuild_index(root: &Path) -> Result<Vec<TutorialIndexEntry>> {
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let manifest_path = entry.path().join("tutorial.json");
            if !manifest_path.exists() {
                continue;
            }
            if let Ok(manifest) = read_manifest(&manifest_path) {
                entries.push(TutorialIndexEntry {
                    id: manifest.id.clone(),
                    title: manifest.title.clone(),
                    issue_ids: manifest.issue_ids.clone(),
                    created_at: manifest.created_at.clone(),
                    merge_commit: manifest.merge_commit.clone(),
                    base_branch: manifest.base_branch.clone(),
                    source_branch: manifest.source_branch.clone(),
                    status: manifest.status.clone(),
                    steps: manifest.steps.len(),
                });
            }
        }
    }

    entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    save_index_at(root, &entries)?;
    Ok(entries)
}

fn save_index_at(root: &Path, entries: &[TutorialIndexEntry]) -> Result<()> {
    crank_io::ensure_dir(root)?;
    let index_path = root.join("index.json");
    let json = serde_json::to_string_pretty(entries)?;
    crank_io::write_string(&index_path, json)?;
    Ok(())
}

fn tutorials_dir(repo_root: &Path) -> PathBuf {
    crank_io::repo_crank_dir(repo_root).join("tutorials")
}

fn read_manifest(path: &Path) -> Result<TutorialManifest> {
    let raw = crank_io::read_to_string(path)?;
    let manifest: TutorialManifest = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(manifest)
}

fn write_manifest(path: &Path, manifest: &TutorialManifest) -> Result<()> {
    let json = serde_json::to_string_pretty(manifest)?;
    crank_io::write_string(path, json)?;
    Ok(())
}

fn read_optional_file(path: PathBuf) -> Result<String> {
    if !path.exists() {
        return Ok(String::new());
    }
    crank_io::read_to_string(&path)
}

fn resolve_commit_range(
    repo_root: &Path,
    merge_commit: &str,
    base: &str,
) -> Result<(String, String)> {
    let parents = git_output(repo_root, &["rev-list", "--parents", "-n", "1", merge_commit])?;
    let parts: Vec<&str> = parents.split_whitespace().collect();
    if parts.len() >= 3 {
        return Ok((parts[1].to_string(), parts[2].to_string()));
    }

    let base_ref = git_output(repo_root, &["rev-parse", base])?;
    Ok((base_ref, merge_commit.to_string()))
}

fn load_commits(repo_root: &Path, range: &(String, String)) -> Result<Vec<String>> {
    let range_expr = format!("{}..{}", range.0, range.1);
    let output = git_output(repo_root, &["log", "--reverse", "--format=%H", &range_expr])?;
    let commits: Vec<String> = output
        .lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();

    if commits.is_empty() {
        return Ok(vec![range.1.clone()]);
    }

    Ok(commits)
}

fn write_steps(
    repo_root: &Path,
    tutorial_dir: &Path,
    commits: &[String],
) -> Result<Vec<TutorialStep>> {
    let mut steps = Vec::new();
    for (idx, commit) in commits.iter().enumerate() {
        let subject = git_output(repo_root, &["show", "-s", "--format=%s", commit])?;
        let files = git_list_files(repo_root, commit)?;
        let note_text = build_step_note(&subject, commit, &files, idx + 1);
        let diff_text = git_output(repo_root, &["show", "--no-color", "--format=", commit])?;

        let index_num = idx + 1;
        let note_name = format!("steps/{index_num:02}.md");
        let diff_name = format!("steps/{index_num:02}.diff");

        crank_io::write_string(&tutorial_dir.join(&note_name), &note_text)?;
        crank_io::write_string(&tutorial_dir.join(&diff_name), &diff_text)?;

        steps.push(TutorialStep {
            index: index_num as u32,
            commit: commit.clone(),
            subject,
            files,
            note: note_name,
            diff: diff_name,
        });
    }
    Ok(steps)
}

fn build_step_note(subject: &str, commit: &str, files: &[String], index: usize) -> String {
    let short = short_commit(commit);
    let mut lines = Vec::new();
    lines.push(format!("# Step {index}: {subject}"));
    lines.push(String::new());
    lines.push(format!("- Commit: {short}"));

    if files.is_empty() {
        lines.push("- Files: (none)".to_string());
    } else if files.len() <= 6 {
        lines.push(format!("- Files: {}", files.join(", ")));
    } else {
        let head = files[..6].join(", ");
        lines.push(format!("- Files: {head} (+{} more)", files.len() - 6));
    }

    lines.push(String::new());
    lines.join("\n")
}

fn git_list_files(repo_root: &Path, commit: &str) -> Result<Vec<String>> {
    let output = git_output(repo_root, &["show", "--name-only", "--format=", commit])?;
    Ok(output
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .map(|line| line.to_string())
        .collect())
}

fn build_summary(
    repo_root: &Path,
    range: &(String, String),
    merge_commit_short: &str,
    source_branch: &str,
    base_branch: &str,
    commits: &[String],
) -> Result<String> {
    let mut lines = Vec::new();
    lines.push("# Summary".to_string());
    lines.push(format!(
        "- Merged {source_branch} into {base_branch} at {merge_commit_short}."
    ));

    let mut subjects = Vec::new();
    for commit in commits {
        let subject = git_output(repo_root, &["show", "-s", "--format=%s", commit])?;
        if !subject.trim().is_empty() {
            subjects.push(subject);
        }
    }

    if !subjects.is_empty() {
        if subjects.len() <= 5 {
            lines.push(format!("- Changes: {}", subjects.join("; ")));
        } else {
            let head = subjects[..5].join("; ");
            lines.push(format!(
                "- Changes: {head}; (+{} more)",
                subjects.len() - 5
            ));
        }
    }

    let files = git_output(
        repo_root,
        &[
            "diff",
            "--name-only",
            &format!("{}..{}", range.0, range.1),
        ],
    )?;
    let file_count = files.lines().filter(|line| !line.trim().is_empty()).count();
    if file_count > 0 {
        lines.push(format!("- Files touched: {file_count}"));
    }
    lines.push("- Tests: not recorded".to_string());
    lines.push(String::new());
    Ok(lines.join("\n"))
}

fn load_issue_ids(worktree: &Path) -> Vec<String> {
    let marker = worktree.join(".crank").join(".current");
    let content = match crank_io::read_to_string(&marker) {
        Ok(content) => content,
        Err(_) => return Vec::new(),
    };
    parse_issue_ids(&content)
}

fn parse_issue_ids(content: &str) -> Vec<String> {
    let cleaned = content.replace(',', " ");
    cleaned
        .split_whitespace()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn load_issue_content(worktree: &Path, repo_root: &Path, ids: &[String]) -> (String, String) {
    if ids.is_empty() {
        return (
            String::new(),
            "# Issue\n\n(No issue linked.)\n".to_string(),
        );
    }

    let mut title = None;
    let mut sections = Vec::new();

    for id in ids {
        let task_path = find_task_file(worktree, repo_root, id);
        if let Some(task_path) = task_path {
            let raw = match crank_io::read_to_string(&task_path) {
                Ok(raw) => raw,
                Err(_) => {
                    sections.push(format!("## Issue {id}\n\n(Missing task file)\n"));
                    continue;
                }
            };
            let parsed = store::parse_task(&task_path).ok();
            if title.is_none() {
                if let Some(task) = parsed.as_ref() {
                    if !task.title.trim().is_empty() {
                        title = Some(task.title.clone());
                    }
                }
            }
            let body = strip_frontmatter(&raw);
            let header = parsed
                .map(|task| task.title)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| format!("Issue {id}"));

            sections.push(format!("## {header}\n\n{body}\n"));
        } else {
            sections.push(format!("## Issue {id}\n\n(Missing task file)\n"));
        }
    }

    let issue_text = format!("# Issue\n\n{}\n", sections.join("\n"));
    (title.unwrap_or_default(), issue_text)
}

fn strip_frontmatter(content: &str) -> String {
    let mut in_frontmatter = false;
    let mut done = false;
    let mut lines = Vec::new();

    for line in content.lines() {
        if line.trim() == "---" {
            if !in_frontmatter {
                in_frontmatter = true;
                continue;
            }
            if in_frontmatter && !done {
                done = true;
                continue;
            }
        }
        if in_frontmatter && !done {
            continue;
        }
        lines.push(line);
    }

    let mut text = lines.join("\n");
    while text.starts_with('\n') {
        text = text[1..].to_string();
    }
    if text.trim().is_empty() {
        "(No issue content)".to_string()
    } else {
        text
    }
}

fn find_task_file(worktree: &Path, repo_root: &Path, id: &str) -> Option<PathBuf> {
    let worktree_path = worktree.join(".crank").join(format!("{id}.md"));
    if worktree_path.exists() {
        return Some(worktree_path);
    }
    let repo_path = crate::task::git::task_path_for_id(repo_root, id);
    if repo_path.exists() {
        return Some(repo_path);
    }
    None
}

fn derive_title(
    repo_root: &Path,
    commits: &[String],
    source_branch: &str,
    base_branch: &str,
) -> Result<String> {
    if let Some(first) = commits.first() {
        let subject = git_output(repo_root, &["show", "-s", "--format=%s", first])?;
        if !subject.trim().is_empty() {
            return Ok(subject);
        }
    }
    Ok(format!("Merge {source_branch} into {base_branch}"))
}

fn git_output(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;
    if !output.status.success() {
        return Err(anyhow!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn repo_root_from(worktree: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .context("failed to run git rev-parse for common dir")?;
    if !output.status.success() {
        return Err(anyhow!("not in a git repository"));
    }
    let mut root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.ends_with(".git") {
        root = root.trim_end_matches(".git").to_string();
        root = root.trim_end_matches('/').to_string();
    }
    Ok(PathBuf::from(root))
}

fn sanitize_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect()
}

fn short_commit(commit: &str) -> String {
    if commit.len() <= 8 {
        commit.to_string()
    } else {
        commit[..8].to_string()
    }
}

fn canonicalize_dir(path: &Path) -> Result<PathBuf> {
    let path = if path.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        path.to_path_buf()
    };
    let resolved = fs::canonicalize(&path)
        .with_context(|| format!("failed to resolve {}", path.display()))?;
    Ok(resolved)
}

fn resolve_path(root: &Path, value: &Path) -> PathBuf {
    if value.is_absolute() {
        value.to_path_buf()
    } else {
        root.join(value)
    }
}

#[derive(Serialize)]
struct TutorialOutput {
    manifest: TutorialManifest,
    issue: String,
    summary: String,
    steps: Vec<TutorialStepOutput>,
}

#[derive(Serialize)]
struct TutorialStepOutput {
    index: u32,
    commit: String,
    subject: String,
    files: Vec<String>,
    note: String,
    diff: String,
}

impl TutorialOutput {
    fn from_full(full: &TutorialFull, step: Option<usize>) -> Result<Self> {
        let steps = select_steps(&full.steps, step)?;
        let step_outputs = steps
            .iter()
            .map(|step| TutorialStepOutput {
                index: step.step.index,
                commit: step.step.commit.clone(),
                subject: step.step.subject.clone(),
                files: step.step.files.clone(),
                note: step.note.clone(),
                diff: step.diff.clone(),
            })
            .collect();
        Ok(Self {
            manifest: full.manifest.clone(),
            issue: full.issue.clone(),
            summary: full.summary.clone(),
            steps: step_outputs,
        })
    }
}

fn render_markdown(full: &TutorialFull, step: Option<usize>) -> Result<String> {
    let mut output = String::new();
    output.push_str(full.issue.trim_end());
    output.push_str("\n\n");
    output.push_str(full.summary.trim_end());
    output.push_str("\n\n");

    let steps = select_steps(&full.steps, step)?;
    for step in steps {
        output.push_str(step.note.trim_end());
        output.push_str("\n\n```diff\n");
        output.push_str(step.diff.trim_end());
        output.push_str("\n```\n\n");
    }

    Ok(output)
}

fn select_steps<'a>(
    steps: &'a [TutorialStepContent],
    step: Option<usize>,
) -> Result<Vec<&'a TutorialStepContent>> {
    if let Some(step) = step {
        if step == 0 || step > steps.len() {
            return Err(anyhow!("step out of range"));
        }
        return Ok(vec![&steps[step - 1]]);
    }
    Ok(steps.iter().collect())
}
