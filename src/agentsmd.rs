pub fn print_agentsmd() {
    println!(
        r#"# CRANK.AGENTS.MD

Tool: project ticketing + agent orchestration. Replace side systems.
Think: single source of truth. Task -> worktree -> agent -> merge. Keep all tickets here.
Two tools in one:
- Ticketing: crank task (create/next/claim/done/dep)
- Orchestration: crank tmux/worker/nudge/pause/ask-for-help/status/attach/merge/review

## Commands
- crank agents.md (this doc)
- crank task create "title" -a app -p 1-5
- crank task (or crank task next)
- crank task claim --project <name> [--json]
- crank task done <id> [--pr <num>]
- crank tmux -c N [--project <name>]
- crank nudge --pane $TMUX_PANE
- crank pause [--clear]
- crank ask-for-help "msg"
- crank status [-w] [-f <id>]
- crank attach <id>
- crank merge [--dry-run] [--notify]
- crank review [--skip-tests]
"#,
    );
}
