//! PRD generator TUI. Drives `claude -p` to generate a PRD from a goal, then
//! shows streaming progress in the terminal.
//!
//! Usage:
//!   cargo run -- "Add per-tenant rate limiting in the API gateway" --repo /path/to/repo
//!
//! Prerequisites: the Claude Code CLI installed on PATH, and a subscription
//! login (do not set ANTHROPIC_API_KEY if you want to use the subscription limit).

mod app;
mod config;
mod engine;
mod event;
mod orchestrator;
mod plan;
mod ui;
mod workspace;
mod worktree;

use std::io::stdout;
use std::path::PathBuf;
use std::time::Duration;

use crossterm::{
    event::{Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::mpsc;

use app::{App, Phase};
use event::AppEvent;

fn slugify(goal: &str) -> String {
    let words: Vec<String> = goal
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .take(6)
        .map(|w| w.to_string())
        .collect();
    if words.is_empty() {
        "prd".to_string()
    } else {
        words.join("-")
    }
}

fn parse_args() -> Option<(String, PathBuf)> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut goal_parts: Vec<String> = Vec::new();
    let mut repo = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--repo" => {
                i += 1;
                if i < args.len() {
                    repo = PathBuf::from(&args[i]);
                }
            }
            other => goal_parts.push(other.to_string()),
        }
        i += 1;
    }
    let goal = goal_parts.join(" ").trim().to_string();
    if goal.is_empty() {
        None
    } else {
        Some((goal, repo))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (goal, repo) = match parse_args() {
        Some(v) => v,
        None => {
            eprintln!("usage: agentic-tui \"<goal>\" [--repo <path>]");
            std::process::exit(1);
        }
    };
    let repo = repo.canonicalize().unwrap_or(repo);
    if !repo.is_dir() {
        eprintln!("repo not found: {}", repo.display());
        std::process::exit(1);
    }

    let out_path = format!("docs/prd/{}.md", slugify(&goal));
    // Ensure the output folder exists so Write succeeds immediately.
    let _ = std::fs::create_dir_all(repo.join("docs/prd"));

    let mut app = App::new(goal.clone(), config::BUDGET_USD);

    // --- Event channel ---
    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();

    // Input thread (crossterm::read is blocking, runs on a std thread).
    let itx = tx.clone();
    std::thread::spawn(move || loop {
        match crossterm::event::read() {
            Ok(Event::Key(k)) => {
                if itx.send(AppEvent::Input(k)).is_err() {
                    break;
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    });

    // Tick task for the spinner and elapsed time.
    let ttx = tx.clone();
    tokio::spawn(async move {
        let mut iv = tokio::time::interval(Duration::from_millis(200));
        loop {
            iv.tick().await;
            if ttx.send(AppEvent::Tick).is_err() {
                break;
            }
        }
    });

    // Pipeline task: run the PRD stage.
    let rtx = tx.clone();
    let repo_run = repo.clone();
    let out_run = out_path.clone();
    tokio::spawn(async move {
        let stage = config::prd_stage();
        let prompt = config::prd_prompt(&goal, &out_run);
        let _ = rtx.send(AppEvent::Started {
            model: stage.model.to_string(),
            out_path: out_run.clone(),
        });
        match engine::run_stage(&repo_run, &stage, &prompt, &rtx).await {
            Ok(o) => {
                let _ = rtx.send(AppEvent::Cost(o.cost));
                let _ = rtx.send(AppEvent::Finished { cost: o.cost, ok: o.ok });
            }
            Err(e) => {
                let _ = rtx.send(AppEvent::Fatal(e.to_string()));
            }
        }
    });

    // --- Terminal setup ---
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    // --- Event loop ---
    loop {
        terminal.draw(|f| ui::render(f, &app))?;
        match rx.recv().await {
            Some(AppEvent::Input(k)) => match (k.code, k.modifiers) {
                (KeyCode::Char('q'), _) => break,
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => break,
                _ => {}
            },
            Some(AppEvent::Tick) => app.tick(),
            Some(other) => app.apply(other),
            None => break,
        }
    }

    // --- Restore ---
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    // Summary after leaving the TUI.
    match app.phase {
        Phase::Done => {
            let full = repo.join(&out_path);
            if full.exists() {
                println!("PRD written to: {}", full.display());
            } else {
                println!("Done, check {}", out_path);
            }
            println!("Cost ~${:.4}", app.total_cost);
            println!(
                "Handoff to Claude Code: /clear then 'Implement {}. Work through the task breakdown one by one, and treat the acceptance criteria as a contract.'",
                out_path
            );
        }
        Phase::Failed => {
            if let Some(e) = &app.error {
                eprintln!("Failed: {e}");
            } else {
                eprintln!("PRD generation failed. Check the log.");
            }
        }
        Phase::Running => {}
    }

    Ok(())
}
