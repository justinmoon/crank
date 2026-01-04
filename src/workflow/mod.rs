use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use clap::{Args, Subcommand};
use serde::Deserialize;

use crate::task::model::Dependency;
use crate::task::store;
use crate::task::{git as task_git, model};

#[derive(Subcommand, Clone)]
pub enum WorkflowCommand {
    /// List available workflow templates
    List,

    /// Apply a workflow template and create tasks
    Apply(WorkflowApplyArgs),

    /// Run a workflow instance by ID
    Run(WorkflowRunArgs),
}

#[derive(Args, Clone)]
pub struct WorkflowApplyArgs {
    /// Template name
    #[arg(value_name = "template")]
    pub template: String,

    /// Workflow instance ID (defaults to template name + random suffix)
    #[arg(long)]
    pub id: Option<String>,

    /// Template variables (format: key=value)
    #[arg(long = "var")]
    pub vars: Vec<String>,

    /// Ignore created tasks in git status
    #[arg(long)]
    pub ephemeral: bool,

    /// Overwrite existing task files
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Clone)]
pub struct WorkflowRunArgs {
    /// Workflow instance ID
    #[arg(value_name = "workflow-id")]
    pub id: String,

    /// Max number of steps to run in parallel
    #[arg(long, default_value = "2")]
    pub concurrency: usize,
}

#[derive(Debug, Deserialize)]
struct WorkflowTemplate {
    workflow: String,
    version: u32,
    vars: Option<HashMap<String, VarDef>>,
    steps: Vec<WorkflowStep>,
}

#[derive(Debug, Deserialize)]
struct VarDef {
    default: Option<String>,
    required: Option<bool>,
}

#[derive(Debug, Deserialize, Clone)]
struct WorkflowStep {
    id: String,
    title: String,
    run: Option<String>,
    needs: Option<Vec<String>>,
}

pub async fn run_command(cmd: WorkflowCommand) -> Result<()> {
    match cmd {
        WorkflowCommand::List => list_templates(),
        WorkflowCommand::Apply(args) => {
            let git_root = task_git::git_root()?;
            apply_template_at(&git_root, &args)
        }
        WorkflowCommand::Run(args) => {
            let git_root = task_git::git_root()?;
            run_workflow_at(&git_root, &args.id, args.concurrency).await
        }
    }
}

fn list_templates() -> Result<()> {
    let git_root = task_git::git_root()?;
    let mut templates = Vec::new();

    let repo_dir = repo_templates_dir(&git_root);
    if repo_dir.exists() {
        templates.extend(read_template_names(&repo_dir)?);
    }

    if let Some(home_dir) = user_templates_dir() {
        if home_dir.exists() {
            templates.extend(read_template_names(&home_dir)?);
        }
    }

    templates.sort();
    templates.dedup();

    if templates.is_empty() {
        println!("No workflow templates found");
        return Ok(());
    }

    for template in templates {
        println!("{template}");
    }

    Ok(())
}

fn read_template_names(dir: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if let Some(stripped) = name.strip_suffix(".workflow.toml") {
                names.push(stripped.to_string());
            }
        }
    }
    Ok(names)
}

pub fn apply_template_at(git_root: &Path, args: &WorkflowApplyArgs) -> Result<()> {
    let template = load_template(git_root, &args.template)?;
    validate_template(&template, &args.template)?;

    let workflow_id = match &args.id {
        Some(id) => id.clone(),
        None => generate_workflow_id(&template.workflow),
    };
    validate_workflow_id(&workflow_id)?;

    let vars = parse_vars(&args.vars)?;
    let resolved_vars = resolve_vars(&template, vars)?;

    let tasks_dir = git_root.join(".crank");
    std::fs::create_dir_all(&tasks_dir)
        .with_context(|| format!("failed to create tasks directory: {}", tasks_dir.display()))?;

    let date = chrono::Local::now().format("%Y-%m-%d").to_string();

    if args.ephemeral {
        let pattern = format!(".crank/{workflow_id}.*.md");
        store::ensure_git_exclude(git_root, &pattern)?;
    }

    for step in &template.steps {
        let task_id = format!("{}.{}", workflow_id, step.id);
        let task_path = tasks_dir.join(format!("{task_id}.md"));
        if task_path.exists() && !args.force {
            return Err(anyhow!("task already exists: {}", task_path.display()));
        }

        let title = render_template(&step.title, &resolved_vars);
        let run = step
            .run
            .as_deref()
            .map(|value| render_template(value, &resolved_vars))
            .filter(|value| !value.trim().is_empty());
        let deps = build_dependencies(&workflow_id, step.needs.as_deref());
        let content =
            build_task_content(&title, &workflow_id, &step.id, run.as_deref(), &date, &deps);

        store::write_task_file(&task_path, &content)?;
    }

    println!(
        "Applied workflow '{}' as '{}'",
        template.workflow, workflow_id
    );
    Ok(())
}

fn build_task_content(
    title: &str,
    workflow_id: &str,
    step_id: &str,
    run: Option<&str>,
    created: &str,
    deps: &[Dependency],
) -> String {
    let app_line = "app: crank".to_string();
    let title_line = if title.trim().is_empty() {
        "title:".to_string()
    } else {
        format!("title: {}", yaml_quote(title))
    };
    let run_line = run.map(|value| format!("run: {}", yaml_quote(value)));

    let mut deps_section = String::new();
    if !deps.is_empty() {
        deps_section.push_str("depends_on:\n");
        for dep in deps {
            deps_section.push_str(&format!(
                "  - id: {}\n    type: {}\n",
                yaml_quote(&dep.id),
                dep.dep_type
            ));
        }
    }

    let mut frontmatter_lines = vec![
        "---".to_string(),
        app_line,
        title_line,
        "priority: 3".to_string(),
        "status: open".to_string(),
        format!("workflow: {}", yaml_quote(workflow_id)),
        format!("step_id: {}", yaml_quote(step_id)),
    ];

    if let Some(run_line) = run_line {
        frontmatter_lines.push(run_line);
    }

    frontmatter_lines.push(format!("created: {created}"));

    let mut frontmatter = frontmatter_lines.join("\n");
    frontmatter.push('\n');

    if !deps_section.is_empty() {
        frontmatter.push_str(&deps_section);
    }

    frontmatter.push_str("---\n\n## Intent\n\n## Spec\n");
    if let Some(run) = run {
        frontmatter.push_str(&format!("\n- Run: {run}\n"));
    } else {
        frontmatter.push('\n');
    }

    frontmatter
}

fn yaml_quote(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn build_dependencies(workflow_id: &str, needs: Option<&[String]>) -> Vec<Dependency> {
    let Some(needs) = needs else {
        return Vec::new();
    };

    needs
        .iter()
        .map(|need| Dependency {
            id: format!("{}.{}", workflow_id, need),
            dep_type: "blocks".to_string(),
        })
        .collect()
}

fn parse_vars(vars: &[String]) -> Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    for raw in vars {
        let Some((key, value)) = raw.split_once('=') else {
            return Err(anyhow!("invalid var format: {raw} (expected key=value)"));
        };
        let key = key.trim();
        if key.is_empty() {
            return Err(anyhow!("invalid var format: {raw} (empty key)"));
        }
        map.insert(key.to_string(), value.to_string());
    }
    Ok(map)
}

fn resolve_vars(
    template: &WorkflowTemplate,
    mut vars: HashMap<String, String>,
) -> Result<HashMap<String, String>> {
    if let Some(defs) = &template.vars {
        for (name, def) in defs {
            let has_value = vars.contains_key(name);
            let required = def.required.unwrap_or(false);

            if !has_value {
                if let Some(default) = &def.default {
                    vars.insert(name.clone(), default.clone());
                } else if required {
                    return Err(anyhow!("missing required var: {name}"));
                }
            }
        }
    }

    Ok(vars)
}

fn render_template(input: &str, vars: &HashMap<String, String>) -> String {
    let mut output = input.to_string();
    for (key, value) in vars {
        let needle = format!("{{{{{key}}}}}");
        output = output.replace(&needle, value);
    }
    output
}

fn load_template(git_root: &Path, name: &str) -> Result<WorkflowTemplate> {
    let repo_path = repo_templates_dir(git_root).join(format!("{name}.workflow.toml"));
    let user_path = user_templates_dir().map(|dir| dir.join(format!("{name}.workflow.toml")));

    let path = if repo_path.exists() {
        repo_path
    } else if let Some(user_path) = user_path {
        if user_path.exists() {
            user_path
        } else {
            return Err(anyhow!("workflow template not found: {name}"));
        }
    } else {
        return Err(anyhow!("workflow template not found: {name}"));
    };

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read workflow template: {}", path.display()))?;
    let template: WorkflowTemplate = toml::from_str(&content)
        .with_context(|| format!("failed to parse workflow template: {}", path.display()))?;

    Ok(template)
}

fn repo_templates_dir(git_root: &Path) -> PathBuf {
    git_root.join(".crank").join("workflows")
}

fn user_templates_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|dir| dir.join(".crank").join("workflows"))
}

fn validate_template(template: &WorkflowTemplate, expected_name: &str) -> Result<()> {
    if template.workflow != expected_name {
        return Err(anyhow!(
            "workflow name mismatch: expected '{expected_name}', got '{}'",
            template.workflow
        ));
    }

    if template.version == 0 {
        return Err(anyhow!("workflow version must be >= 1"));
    }

    if template.steps.is_empty() {
        return Err(anyhow!("workflow has no steps"));
    }

    let mut ids = HashSet::new();
    for step in &template.steps {
        if step.id.trim().is_empty() {
            return Err(anyhow!("workflow step id cannot be empty"));
        }
        if !ids.insert(step.id.clone()) {
            return Err(anyhow!("duplicate workflow step id: {}", step.id));
        }
    }

    let id_set: HashSet<_> = template.steps.iter().map(|s| s.id.as_str()).collect();
    for step in &template.steps {
        if let Some(needs) = &step.needs {
            for need in needs {
                if !id_set.contains(need.as_str()) {
                    return Err(anyhow!(
                        "workflow step '{}' depends on unknown step '{}'",
                        step.id,
                        need
                    ));
                }
            }
        }
    }

    Ok(())
}

fn generate_workflow_id(prefix: &str) -> String {
    let suffix: u16 = rand::random();
    format!("{}-{:04x}", prefix, suffix)
}

fn validate_workflow_id(id: &str) -> Result<()> {
    if id.trim().is_empty() {
        return Err(anyhow!("workflow id cannot be empty"));
    }
    if id.contains('/') || id.contains('\\') {
        return Err(anyhow!("workflow id cannot contain path separators"));
    }
    if id.chars().any(|c| c.is_whitespace()) {
        return Err(anyhow!("workflow id cannot contain whitespace"));
    }
    Ok(())
}

pub async fn run_workflow_at(git_root: &Path, id: &str, concurrency: usize) -> Result<()> {
    if concurrency == 0 {
        return Err(anyhow!("concurrency must be >= 1"));
    }

    loop {
        let tasks = store::load_tasks(git_root)?;
        let workflow_tasks: Vec<model::Task> = tasks
            .iter()
            .filter(|task| task.workflow.as_deref() == Some(id))
            .cloned()
            .collect();

        if workflow_tasks.is_empty() {
            return Err(anyhow!("no tasks found for workflow: {id}"));
        }

        let runnable: Vec<model::Task> = workflow_tasks
            .iter()
            .filter(|task| !task.is_closed())
            .filter(|task| {
                task.run
                    .as_deref()
                    .map(str::trim)
                    .filter(|run| !run.is_empty())
                    .is_some()
            })
            .filter(|task| task.blockers(&tasks).is_empty())
            .cloned()
            .collect();

        let manual_pending: Vec<model::Task> = workflow_tasks
            .iter()
            .filter(|task| !task.is_closed())
            .filter(|task| task.run.as_deref().map(str::trim).unwrap_or("").is_empty())
            .cloned()
            .collect();

        let blocked: Vec<(String, Vec<String>)> = workflow_tasks
            .iter()
            .filter(|task| !task.is_closed())
            .filter(|task| {
                task.run
                    .as_deref()
                    .map(str::trim)
                    .filter(|run| !run.is_empty())
                    .is_some()
            })
            .filter_map(|task| {
                let blockers: Vec<String> = task
                    .blockers(&tasks)
                    .iter()
                    .map(|blocker| blocker.id.clone())
                    .collect();
                if blockers.is_empty() {
                    None
                } else {
                    Some((task.id.clone(), blockers))
                }
            })
            .collect();

        if runnable.is_empty() {
            if manual_pending.is_empty() {
                if !blocked.is_empty() {
                    let details = blocked
                        .iter()
                        .map(|(task_id, blockers)| {
                            format!("{task_id} blocked by {}", blockers.join(", "))
                        })
                        .collect::<Vec<_>>()
                        .join("; ");
                    return Err(anyhow!(
                        "workflow '{id}' has blocked steps with no runnable tasks: {details}"
                    ));
                }

                println!("Workflow '{id}' complete");
                return Ok(());
            }

            let waiting: Vec<String> = manual_pending.iter().map(|task| task.id.clone()).collect();
            println!(
                "Workflow '{id}' waiting on manual steps: {}",
                waiting.join(", ")
            );
            return Ok(());
        }

        let mut join_set = tokio::task::JoinSet::new();
        for task in runnable.into_iter().take(concurrency) {
            store::update_task_status(&task.path, model::TASK_STATUS_IN_PROGRESS)?;
            let workdir = git_root.to_path_buf();
            let run = task.run.clone().unwrap_or_else(|| "".to_string());
            join_set.spawn(async move { run_step(workdir, task, run).await });
        }

        let mut failures = Vec::new();
        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(outcome) => {
                    let path = task_git::task_path_for_id(git_root, &outcome.id);
                    match outcome.result {
                        Ok(()) => {
                            store::update_task_status(&path, model::TASK_STATUS_CLOSED)?;
                        }
                        Err(err) => {
                            store::update_task_status(&path, model::TASK_STATUS_OPEN)?;
                            failures.push(err);
                        }
                    }
                }
                Err(err) => {
                    failures.push(anyhow!("task execution failed: {err}"));
                }
            }
        }

        if !failures.is_empty() {
            let mut message = String::from("workflow step failed");
            for failure in failures {
                message.push_str(&format!("\n- {failure}"));
            }
            return Err(anyhow!(message));
        }
    }
}

struct StepOutcome {
    id: String,
    result: Result<()>,
}

async fn run_step(workdir: PathBuf, task: model::Task, cmd: String) -> StepOutcome {
    let id = task.id.clone();
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return StepOutcome {
            id,
            result: Err(anyhow!("step '{}' has empty run command", task.id)),
        };
    }

    let status = tokio::process::Command::new("bash")
        .arg("-lc")
        .arg(trimmed)
        .current_dir(&workdir)
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .await
        .with_context(|| format!("failed to run step '{}'", task.id));

    let result = match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(anyhow!(
            "step '{}' failed with status {}",
            task.id,
            status.code().unwrap_or(1)
        )),
        Err(err) => Err(err),
    };

    StepOutcome { id, result }
}
