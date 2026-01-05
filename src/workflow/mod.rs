use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use clap::Args;
use serde::{Deserialize, Serialize};

use crate::task::model::Dependency;
use crate::task::store;

#[derive(Args, Clone)]
pub struct BuildArgs {
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

pub fn build_template_at(git_root: &Path, args: &BuildArgs) -> Result<()> {
    let template = load_template(git_root, &args.template)?;
    validate_template(&template, &args.template)?;

    let workflow_id = match &args.id {
        Some(id) => id.clone(),
        None => generate_workflow_id(&template.workflow),
    };
    validate_workflow_id(&workflow_id)?;

    let vars = parse_vars(&args.vars)?;
    let resolved_vars = resolve_vars(&template, vars)?;

    let tasks_dir = crate::crank_io::repo_crank_dir(git_root);
    crate::crank_io::ensure_dir(&tasks_dir)?;

    let date = chrono::Local::now().format("%Y-%m-%d").to_string();

    if args.ephemeral {
        let pattern = format!(".crank/{workflow_id}.*.md");
        store::ensure_git_exclude(git_root, &pattern)?;
        let manifest_pattern = format!(".crank/workflows/{workflow_id}.manifest.toml");
        store::ensure_git_exclude(git_root, &manifest_pattern)?;
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

    write_manifest_at(git_root, &workflow_id, &template.steps)?;

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
        "supervision: unsupervised".to_string(),
        format!("workflow: {}", yaml_quote(workflow_id)),
        format!("step_id: {}", yaml_quote(step_id)),
    ];

    frontmatter_lines.push(format!("created: {created}"));

    let mut frontmatter = frontmatter_lines.join("\n");
    frontmatter.push('\n');

    if !deps_section.is_empty() {
        frontmatter.push_str(&deps_section);
    }

    frontmatter.push_str("---\n\n## Intent\n\n## Spec\n");
    if let Some(run) = run {
        frontmatter.push_str("\n### Run\n```bash\n");
        frontmatter.push_str(run);
        frontmatter
            .push_str("\n```\n\n### Acceptable Output\n- Describe what success looks like.\n");
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

    let content = crate::crank_io::read_to_string(&path)
        .with_context(|| format!("failed to read workflow template: {}", path.display()))?;
    let template: WorkflowTemplate = toml::from_str(&content)
        .with_context(|| format!("failed to parse workflow template: {}", path.display()))?;

    Ok(template)
}

fn repo_templates_dir(git_root: &Path) -> PathBuf {
    crate::crank_io::repo_workflows_dir(git_root)
}

fn user_templates_dir() -> Option<PathBuf> {
    crate::crank_io::user_workflows_dir_opt()
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WorkflowManifest {
    pub workflow: String,
    pub steps: Vec<String>,
}

fn manifest_path(git_root: &Path, workflow_id: &str) -> PathBuf {
    repo_templates_dir(git_root).join(format!("{workflow_id}.manifest.toml"))
}

fn write_manifest_at(git_root: &Path, workflow_id: &str, steps: &[WorkflowStep]) -> Result<()> {
    let manifest = WorkflowManifest {
        workflow: workflow_id.to_string(),
        steps: steps.iter().map(|step| step.id.clone()).collect(),
    };
    let path = manifest_path(git_root, workflow_id);
    if let Some(parent) = path.parent() {
        crate::crank_io::ensure_dir(parent).with_context(|| {
            format!(
                "failed to create workflow manifest directory: {}",
                parent.display()
            )
        })?;
    }
    let content = toml::to_string_pretty(&manifest)?;
    crate::crank_io::write_string(&path, &content)
        .with_context(|| format!("failed to write workflow manifest: {}", path.display()))?;
    Ok(())
}

pub fn load_manifest(git_root: &Path, workflow_id: &str) -> Result<Option<WorkflowManifest>> {
    let path = manifest_path(git_root, workflow_id);
    if !path.exists() {
        return Ok(None);
    }
    let content = crate::crank_io::read_to_string(&path)
        .with_context(|| format!("failed to read workflow manifest: {}", path.display()))?;
    let manifest: WorkflowManifest = toml::from_str(&content)
        .with_context(|| format!("failed to parse workflow manifest: {}", path.display()))?;
    Ok(Some(manifest))
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
