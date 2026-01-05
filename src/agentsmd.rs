pub fn print_agentsmd() {
    println!(
        r#"# CRANK.AGENTS.MD

Tool: project ticketing + agent orchestration. Replace side systems.
Think: single source of truth. Task -> worktree -> agent -> workflow. Keep all tickets here.
Two tools in one:
- Ticketing: crank task (create/next/claim/done/dep)
- Orchestration: crank tmux/worker/nudge/pause/ask-for-help/build/run/review

## Supervision tutorial
- Tasks must declare `supervision: supervised|unsupervised` in frontmatter.
- Unsupervised tasks are auto-claimed by unsupervised workers.
- Supervised tasks require manual selection in supervised mode.
- In supervised mode, if you need user input, run `crank ask-for-help "<msg>"`.
- The orchestrator will mark the task `needs_human` and release it when help is requested.
- Once clarified, resume by flipping status back to `open` and re-run/claim.

## Commands
- crank agents.md (this doc)
- crank task create "title" -a app -p 1-5
- crank task (or crank task next)
- crank task claim --project <name> [--json]
- crank task done <id> [--pr <num>]
- crank tmux -c N --mode <supervised|unsupervised> [--project <name>]
- crank nudge --pane $TMUX_PANE
- crank pause [--clear]
- crank done [--task <id>]
- crank ask-for-help "msg"
- crank alerts
- crank build <template> --id <workflow-id> --var key=val
- crank run [--workflow <workflow-id>] [--once] [<task-id>]
- crank review [--skip-tests]
"#,
    );
}
