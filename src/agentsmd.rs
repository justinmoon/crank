pub fn print_agentsmd() {
    println!(
        r#"# CRANK.AGENTS.MD

Tool: project ticketing + agent orchestration. Replace side systems.
Think: single source of truth. Task -> worktree -> agent -> workflow. Keep all tickets here.
Two tools in one:
- Ticketing: crank task (create/next/claim/done/dep)
- Orchestration: crank tmux/worker/nudge/pause/ask-for-help/build/run/review

## Autopilot tutorial
- Tasks default to `autopilot: true` in frontmatter.
- Set `autopilot: false` to prevent auto-claiming; those tasks require manual run/claim.
- If you need user input, run `crank ask-for-help "<msg>"`.
- The orchestrator will mark the task `needs_human` and release it when help is requested.
- Once clarified, resume by flipping status back to `open` and re-run/claim.

## Commands
- crank agents.md (this doc)
- crank task create "title" -a app -p 1-5
- crank task (or crank task next)
- crank task claim --project <name> [--json]
- crank task done <id> [--pr <num>]
- crank tmux -c N [--project <name>]
- crank nudge --pane $TMUX_PANE
- crank pause [--clear]
- crank done [--task <id>]
- crank ask-for-help "msg"
- crank build <template> --id <workflow-id> --var key=val
- crank run [--workflow <workflow-id>] [--loop] [<task-id>]
- crank review [--skip-tests]
"#,
    );
}
