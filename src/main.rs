//! Agentic orchestrator TUI. Picks a workspace, plans a goal into epics, then
//! drives worktree-isolated `claude -p` sessions that implement and verify each
//! epic, merging passing epics into an integration branch.
//!
//! Usage:
//!   cargo run -- ["<goal>"] [--workspace <name|path>] [--verify "<cmd>"] [--no-refine]
//!
//! The goal is optional; if omitted it is typed in the TUI after the workspace
//! is chosen. Unless `--no-refine` is passed, the goal runs through a
//! clarifying refine pass before planning starts.
//!
//! Prerequisites: the Claude Code CLI on PATH, a subscription login, and git.

mod app;
mod config;
mod engine;
mod event;
mod orchestrator;
mod plan;
mod refine;
mod ui;
mod workspace;
mod worktree;

use std::io::stdout;
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
use workspace::Workspace;

struct Args {
    goal: String,
    workspace: Option<String>,
    verify: Option<String>,
    no_refine: bool,
}

fn parse_args() -> Option<Args> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut goal_parts: Vec<String> = Vec::new();
    let mut workspace = None;
    let mut verify = None;
    let mut no_refine = false;
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--workspace" => {
                i += 1;
                workspace = raw.get(i).cloned();
            }
            "--verify" => {
                i += 1;
                verify = raw.get(i).cloned();
            }
            "--no-refine" => no_refine = true,
            other => goal_parts.push(other.to_string()),
        }
        i += 1;
    }
    let goal = goal_parts.join(" ").trim().to_string();
    Some(Args {
        goal,
        workspace,
        verify,
        no_refine,
    })
}

/// What the workspace picker returned: a chosen workspace, a request to add more
/// via the onboarding wizard, or a quit.
enum PickerOutcome {
    Chosen(Workspace),
    Add,
    Quit,
}

/// Resolve the chosen workspace. `--workspace` matches by name or path. With no
/// flag and an empty config, the onboarding wizard runs first; otherwise the
/// picker shows. The picker's `a` key re-enters the wizard and refreshes the
/// list. Returns None if the user quits.
fn resolve_workspace(args: &Args, workspaces: &[Workspace]) -> anyhow::Result<Option<Workspace>> {
    if let Some(wanted) = &args.workspace {
        if let Some(found) = workspaces.iter().find(|w| &w.name == wanted) {
            return Ok(Some(found.clone()));
        }
        let path = workspace::expand_tilde(wanted);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "workspace".to_string());
        return Ok(Some(Workspace { name, path }));
    }

    let config = workspace::default_config_path();
    let mut list: Vec<Workspace> = if workspaces.is_empty() {
        match run_onboarding(&config)? {
            Some(all) => all,
            None => return Ok(None),
        }
    } else {
        workspaces.to_vec()
    };

    loop {
        match run_picker(&list)? {
            PickerOutcome::Chosen(workspace) => return Ok(Some(workspace)),
            PickerOutcome::Quit => return Ok(None),
            PickerOutcome::Add => {
                if let Some(all) = run_onboarding(&config)? {
                    list = all;
                }
            }
        }
    }
}

/// Blocking picker loop on its own alternate screen.
fn run_picker(workspaces: &[Workspace]) -> anyhow::Result<PickerOutcome> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;
    let mut selected = 0usize;
    let outcome = loop {
        terminal.draw(|f| ui::render_picker(f, workspaces, selected))?;
        if let Event::Key(key) = crossterm::event::read()? {
            match key.code {
                KeyCode::Up => selected = selected.saturating_sub(1),
                KeyCode::Down if selected + 1 < workspaces.len() => selected += 1,
                KeyCode::Enter => break PickerOutcome::Chosen(workspaces[selected].clone()),
                KeyCode::Char('a') => break PickerOutcome::Add,
                KeyCode::Char('q') => break PickerOutcome::Quit,
                _ => {}
            }
        }
    };
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(outcome)
}

/// Blocking onboarding wizard on its own alternate screen. Scans a folder for
/// git repositories, lets the user pick some, saves them, and returns the full
/// saved workspace list. Returns None if the user cancels.
fn run_onboarding(config_path: &std::path::Path) -> anyhow::Result<Option<Vec<Workspace>>> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    enum Screen {
        Root,
        List,
    }
    let mut screen = Screen::Root;
    let mut root = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let mut repos: Vec<Workspace> = Vec::new();
    let mut checked: Vec<bool> = Vec::new();
    let mut selected = 0usize;

    let result: Option<Vec<Workspace>> = loop {
        match screen {
            Screen::Root => {
                terminal.draw(|f| ui::render_scan_root_input(f, &root))?;
                if let Event::Key(key) = crossterm::event::read()? {
                    match (key.code, key.modifiers) {
                        (KeyCode::Char('c'), KeyModifiers::CONTROL) => break None,
                        (KeyCode::Esc, _) => break None,
                        (KeyCode::Enter, _) => {
                            let dir = workspace::expand_tilde(root.trim());
                            terminal.draw(|f| ui::render_scanning(f, &root))?;
                            repos = workspace::scan_for_repos(&dir, workspace::DEFAULT_SCAN_DEPTH);
                            checked = vec![false; repos.len()];
                            selected = 0;
                            screen = Screen::List;
                        }
                        (KeyCode::Backspace, _) => {
                            root.pop();
                        }
                        (KeyCode::Char(c), _) => root.push(c),
                        _ => {}
                    }
                }
            }
            Screen::List => {
                terminal
                    .draw(|f| ui::render_repo_checklist(f, &repos, selected, &checked, &root))?;
                if let Event::Key(key) = crossterm::event::read()? {
                    match (key.code, key.modifiers) {
                        (KeyCode::Char('c'), KeyModifiers::CONTROL) => break None,
                        (KeyCode::Esc, _) => break None,
                        (KeyCode::Char('r'), _) => screen = Screen::Root,
                        (KeyCode::Up, _) => selected = selected.saturating_sub(1),
                        (KeyCode::Down, _) if selected + 1 < repos.len() => selected += 1,
                        (KeyCode::Char(' '), _) if !repos.is_empty() => {
                            checked[selected] = !checked[selected];
                        }
                        (KeyCode::Enter, _) => {
                            let picked: Vec<Workspace> = repos
                                .iter()
                                .zip(checked.iter())
                                .filter(|(_, &is_checked)| is_checked)
                                .map(|(workspace, _)| workspace.clone())
                                .collect();
                            if picked.is_empty() {
                                continue;
                            }
                            match workspace::save_workspaces(config_path, &picked) {
                                Ok(()) => {
                                    let all =
                                        workspace::load_workspaces(config_path).unwrap_or(picked);
                                    break Some(all);
                                }
                                // Persist failed (for example a read-only disk):
                                // still proceed with the picks for this session.
                                Err(_) => break Some(picked),
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(result)
}

/// Blocking goal input screen on its own alternate screen. Returns None on cancel.
fn run_goal_input(workspace: &str) -> anyhow::Result<Option<String>> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;
    let mut buffer = String::new();
    let result = loop {
        terminal.draw(|f| ui::render_goal_input(f, workspace, &buffer))?;
        if let Event::Key(key) = crossterm::event::read()? {
            match key.code {
                KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => break None,
                KeyCode::Esc => break None,
                KeyCode::Enter if !buffer.trim().is_empty() => {
                    break Some(buffer.trim().to_string());
                }
                KeyCode::Backspace => {
                    buffer.pop();
                }
                KeyCode::Char(c) => buffer.push(c),
                _ => {}
            }
        }
    };
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(result)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = match parse_args() {
        Some(a) => a,
        None => {
            eprintln!(
                "usage: agentic-tui [\"<goal>\"] [--workspace <name|path>] [--verify \"<cmd>\"] [--no-refine]"
            );
            std::process::exit(1);
        }
    };

    let workspaces =
        workspace::load_workspaces(&workspace::default_config_path()).unwrap_or_default();
    let selected = match resolve_workspace(&args, &workspaces)? {
        Some(w) => w,
        None => {
            println!("no workspace selected");
            return Ok(());
        }
    };
    workspace::validate(&selected)?;
    let repo = selected
        .path
        .canonicalize()
        .unwrap_or(selected.path.clone());
    let verify_cmd = args
        .verify
        .clone()
        .unwrap_or_else(|| config::DEFAULT_VERIFY_CMD.to_string());

    let mut goal = if args.goal.is_empty() {
        match run_goal_input(&selected.name)? {
            Some(entered) => entered,
            None => {
                println!("no goal entered");
                return Ok(());
            }
        }
    } else {
        args.goal.clone()
    };

    let refine_cost = if args.no_refine {
        0.0
    } else {
        match refine::run(&repo, &goal).await? {
            refine::RefineOutcome {
                goal: Some(refined),
                cost,
            } => {
                goal = refined;
                cost
            }
            refine::RefineOutcome { goal: None, .. } => {
                println!("run cancelled");
                return Ok(());
            }
        }
    };

    let mut app = App::new(
        goal.clone(),
        selected.name.clone(),
        config::GLOBAL_BUDGET_USD,
    );

    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();

    let input_tx = tx.clone();
    std::thread::spawn(move || loop {
        match crossterm::event::read() {
            Ok(Event::Key(key)) => {
                if input_tx.send(AppEvent::Input(key)).is_err() {
                    break;
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    });

    let tick_tx = tx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(200));
        loop {
            interval.tick().await;
            if tick_tx.send(AppEvent::Tick).is_err() {
                break;
            }
        }
    });

    let pipeline_tx = tx.clone();
    let repo_run = repo.clone();
    let goal_run = goal.clone();
    let verify_run = verify_cmd.clone();
    let pipeline_handle = tokio::spawn(async move {
        if let Err(e) =
            run_pipeline(&repo_run, &goal_run, &verify_run, refine_cost, &pipeline_tx).await
        {
            let _ = pipeline_tx.send(AppEvent::Fatal(e.to_string()));
        }
    });

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    // Whether the pipeline finished (Done/Fatal) before the user quit. A quit
    // while the pipeline is still running is a mid-flight abort, which cleans up
    // the epic worktrees; a quit after completion leaves them in place so any
    // conflict worktree kept for a manual merge survives.
    let mut completed = false;
    loop {
        terminal.draw(|f| ui::render(f, &app))?;
        match rx.recv().await {
            Some(AppEvent::Input(key)) => match (key.code, key.modifiers) {
                (KeyCode::Char('q'), _) => break,
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => break,
                _ => {}
            },
            Some(AppEvent::Tick) => app.tick(),
            Some(other) => {
                let done = matches!(other, AppEvent::Done | AppEvent::Fatal(_));
                app.apply(other);
                if done {
                    completed = true;
                    terminal.draw(|f| ui::render(f, &app))?;
                }
            }
            None => {
                completed = true;
                break;
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    if !completed {
        // Cancel the pipeline task so its child `claude`/`git` processes are
        // killed on drop (they run with `kill_on_drop`), then remove the epic
        // worktrees left behind. Awaiting the aborted task guarantees the child
        // handles are dropped before cleanup touches their directories.
        pipeline_handle.abort();
        let _ = pipeline_handle.await;
        if let Err(e) = worktree::cleanup_all(&repo).await {
            eprintln!("warning: could not clean up worktrees: {e}");
        }
    }

    print_report(&app, &repo);
    Ok(())
}

/// Plan the goal, then run the orchestrator.
async fn run_pipeline(
    repo: &std::path::Path,
    goal: &str,
    verify_cmd: &str,
    refine_cost: f64,
    tx: &mpsc::UnboundedSender<AppEvent>,
) -> anyhow::Result<()> {
    // Count refine spending toward the run total before planning starts.
    if refine_cost > 0.0 {
        let _ = tx.send(AppEvent::Cost(refine_cost));
    }
    let plan_path = repo.join(".agentic-plan.json");
    let plan_path_str = plan_path.to_string_lossy().to_string();
    let prompt = config::plan_prompt(goal, &plan_path_str);
    let spec = engine::StageSpec {
        tag: "plan",
        cwd: repo,
        model: config::MODEL_PLAN,
        tools: config::PLAN_TOOLS,
        max_turns: config::PLAN_MAX_TURNS,
        budget_usd: config::EPIC_BUDGET_USD,
        prompt: &prompt,
    };
    let outcome = engine::run_stage(&spec, tx).await?;
    let _ = tx.send(AppEvent::Cost(outcome.cost));

    let plan_text = std::fs::read_to_string(&plan_path)
        .map_err(|e| anyhow::anyhow!("plan.json was not written: {e}"))?;
    let parsed = plan::parse_plan(&plan_text)?;
    parsed.validate()?;
    let epic_metas: Vec<event::EpicMeta> = parsed
        .epics
        .iter()
        .map(|epic| event::EpicMeta {
            id: epic.id.clone(),
            title: epic.title.clone(),
            depends_on: epic.depends_on.clone(),
        })
        .collect();
    let _ = tx.send(AppEvent::PlanReady { epics: epic_metas });

    let run_config = orchestrator::RunConfig {
        repo: repo.to_path_buf(),
        goal: goal.to_string(),
        verify_cmd: verify_cmd.to_string(),
        integration_branch: "agentic-integration".to_string(),
        budget_usd: config::GLOBAL_BUDGET_USD,
        initial_cost: refine_cost + outcome.cost,
    };
    orchestrator::run(&parsed, run_config, tx.clone()).await?;
    Ok(())
}

fn print_report(app: &App, repo: &std::path::Path) {
    println!("\n=== Run report ===");
    println!("Workspace: {}", app.workspace);
    println!("Goal: {}", app.goal);
    for epic in &app.epics {
        let status = match epic.status {
            app::EpicStatus::Merged => "merged",
            app::EpicStatus::Failed => "failed",
            app::EpicStatus::Skipped => "skipped",
            app::EpicStatus::Conflict => "conflict (manual merge)",
            _ => "incomplete",
        };
        println!("  [{status}] {} {}", epic.id, epic.title);
    }
    println!("Total cost ~${:.4}", app.total_cost);
    match app.phase {
        Phase::Done => {
            println!(
                "Merged work is on branch 'agentic-integration' in {}. Review and merge to your main branch.",
                repo.display()
            );
        }
        Phase::Failed => {
            if let Some(e) = &app.error {
                eprintln!("Run failed: {e}");
            }
        }
        _ => {}
    }
}
