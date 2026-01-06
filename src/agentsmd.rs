pub fn print_agentsmd() {
    println!(
        r#"# CRANK.AGENTS.MD

Tool: project ticketing + agent orchestration. Replace side systems.
Think: single source of truth. Task -> worktree -> agent -> workflow. Keep all tickets here.
Two tools in one:
- Ticketing: crank task (create/next/claim/done/dep)
- Orchestration: crank tmux/zellij/worker/nudge/pause/ask-for-help/build/run/review

## Supervision tutorial
- Tasks must declare `supervision: supervised|unsupervised` in frontmatter.
- Unsupervised tasks are auto-claimed by unsupervised workers.
- Supervised tasks require manual selection in supervised mode.
- In supervised mode, if you need user input, run `crank ask-for-help "<msg>"`.
- The orchestrator will mark the task `needs_human` and release it when help is requested.
- Once clarified, resume by flipping status back to `open` and re-run/claim.

## Tutorials
- Generate from base -> branch tip: `crank tutorial generate --worktree . --base master --merge-commit HEAD [--replace]`
- Include unstaged tracked changes: `git stash create` (captures staged+unstaged), then pass the SHA to `--merge-commit`
- Include untracked files: `git stash push -u`, then `git rev-parse stash@{{0}}` for the SHA

## Commands
- crank agents.md (this doc)
- crank task create "title" -a app -p 1-5
- crank task (or crank task next)
- crank task claim --project <name> [--json]
- crank task done <id> [--pr <num>]
- crank tmux -c N --mode <supervised|unsupervised> [--project <name>]
- crank zellij -c N --mode <supervised|unsupervised> [--project <name>]
- crank nudge --pane $TMUX_PANE (or zellij:<pane_id>)
- crank pause [--clear]
- crank done [--task <id>]
- crank ask-for-help "msg"
- crank alerts
- crank build <template> --id <workflow-id> --var key=val
- crank run [--workflow <workflow-id>] [--once] [<task-id>]
- crank review [--skip-tests]
- crank inbox
- crank tutorial generate [--worktree <path>] [--merge-commit <sha>] [--replace]
- crank tutorial show <id> [--format md|json]
- crank tutorial delete <id> [--all]
"#,
    );
}
