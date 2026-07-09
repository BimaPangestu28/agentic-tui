//! Agentic orchestrator: plans a goal into epics, then drives worktree-isolated
//! `claude -p` sessions that implement and verify each epic, merging passing
//! epics into an integration branch. The browser-based UI drives the whole
//! flow (workspace picking, goal entry, refine, and the run itself); this
//! binary just serves it.
//!
//! Usage:
//!   cargo run -- [--web] [--no-open]
//!
//! `--web` is accepted for backward compatibility but is now the only mode;
//! `--no-open` skips launching the default browser.
//!
//! Prerequisites: the Claude Code CLI on PATH, a subscription login, and git.

use agentic_tui::http;

struct Args {
    no_open: bool,
}

fn parse_args() -> Args {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut no_open = false;
    for arg in raw {
        match arg.as_str() {
            "--no-open" => no_open = true,
            // Kept for backward compatibility: the web UI is now the only mode.
            "--web" => {}
            other => {
                eprintln!("warning: ignoring unrecognized argument '{other}'");
            }
        }
    }
    Args { no_open }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = parse_args();
    http::serve(!args.no_open).await
}
