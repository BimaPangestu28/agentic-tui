//! The goal-refine step: a claude pass rewrites the goal and proposes
//! clarifying questions written to `.agentic-refine.json`, the user answers
//! them, a second pass finalizes the goal, and the user confirms it. Reuses the
//! `plan.json` pattern: each pass writes a JSON file we parse here.

use serde::Deserialize;

use std::io::{self, Stdout};
use std::path::Path;

use crossterm::{
    event::{Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::mpsc;

use crate::config;
use crate::engine;
use crate::event::AppEvent;
use crate::ui;

/// The JSON a refine pass writes to `.agentic-refine.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct RefineResult {
    pub refined_goal: String,
    #[serde(default)]
    pub questions: Vec<String>,
}

/// The result of the whole refine flow. `goal` is `None` only when the user
/// cancelled the run.
#[derive(Debug, Clone)]
pub struct RefineOutcome {
    pub goal: Option<String>,
    pub cost: f64,
}

/// Parse the JSON a refine pass wrote. A missing `questions` defaults to empty;
/// an empty `refined_goal` or malformed JSON is an error, which the caller turns
/// into a fall back to the original goal.
pub fn parse_refine(json: &str) -> anyhow::Result<RefineResult> {
    let result: RefineResult = serde_json::from_str(json)?;
    if result.refined_goal.trim().is_empty() {
        anyhow::bail!("refine result has an empty refined_goal");
    }
    Ok(result)
}

/// How the user left a single question screen.
enum Answered {
    Entered,
    SkipRefine,
    Cancel,
}

/// Restore the terminal from the refine flow's alternate screen.
fn teardown(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> anyhow::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}

/// Run one refine pass. Returns the cost incurred (which is real money even if
/// the session failed to write a usable file) together with the parsed result
/// or the read/parse error. The cost is 0.0 only when the session did not run.
async fn run_refine_pass(
    repo: &Path,
    prompt: &str,
    out_path: &Path,
    tx: &mpsc::UnboundedSender<AppEvent>,
) -> (f64, anyhow::Result<RefineResult>) {
    let _ = std::fs::remove_file(out_path);
    let spec = engine::StageSpec {
        tag: "refine",
        cwd: repo,
        model: config::MODEL_REFINE,
        tools: config::REFINE_TOOLS,
        max_turns: config::REFINE_MAX_TURNS,
        budget_usd: config::REFINE_BUDGET_USD,
        prompt,
    };
    let outcome = match engine::run_stage(&spec, tx).await {
        Ok(outcome) => outcome,
        Err(e) => return (0.0, Err(e)),
    };
    let result = std::fs::read_to_string(out_path)
        .map_err(|e| anyhow::anyhow!(".agentic-refine.json was not written: {e}"))
        .and_then(|text| parse_refine(&text));
    (outcome.cost, result)
}

/// Run refine pass 1 for the HTTP API: rewrite the goal and gather
/// clarifying questions, truncated to `REFINE_MAX_QUESTIONS`. On failure
/// (the pass errored, or `.agentic-refine.json` was unreadable or
/// unparseable), falls back to the original goal with no questions. The cost
/// is real either way, since it is billed once the session runs.
pub async fn questions(repo: &Path, goal: &str) -> (String, Vec<String>, f64) {
    let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();
    let out_path = repo.join(".agentic-refine.json");
    let (cost, result) = run_refine_pass(
        repo,
        &config::refine_questions_prompt(goal, &out_path.to_string_lossy()),
        &out_path,
        &tx,
    )
    .await;
    match result {
        Ok(result) => {
            let mut questions = result.questions;
            questions.truncate(config::REFINE_MAX_QUESTIONS);
            (result.refined_goal, questions, cost)
        }
        Err(_) => (goal.to_string(), Vec::new(), cost),
    }
}

/// Run refine pass 2 for the HTTP API: fold the user's answers into a final
/// goal. On failure, falls back to the original goal; the cost is still
/// reported.
pub async fn finalize(repo: &Path, goal: &str, answers: &[(String, String)]) -> (String, f64) {
    let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();
    let out_path = repo.join(".agentic-refine.json");
    let (cost, result) = run_refine_pass(
        repo,
        &config::refine_finalize_prompt(goal, answers, &out_path.to_string_lossy()),
        &out_path,
        &tx,
    )
    .await;
    match result {
        Ok(result) => (result.refined_goal, cost),
        Err(_) => (goal.to_string(), cost),
    }
}

/// Run the goal-refine flow on its own alternate screen. Returns the confirmed
/// goal (or the original goal if refining is skipped or fails) and the total
/// refine cost. A `None` goal means the user cancelled the run.
pub async fn run(repo: &Path, goal: &str) -> anyhow::Result<RefineOutcome> {
    // The refine passes stream events; the run UI does not exist yet, so we
    // drain them into a channel we hold but never read.
    let (tx, _rx) = mpsc::unbounded_channel::<AppEvent>();
    let out_path = repo.join(".agentic-refine.json");

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut total_cost = 0.0f64;

    // Pass 1: rewrite the goal and gather questions. On any failure, fall back
    // to the original goal.
    terminal.draw(|f| ui::render_refining(f, "reading the repository and sharpening the goal"))?;
    let (cost1, result1) = run_refine_pass(
        repo,
        &config::refine_questions_prompt(goal, &out_path.to_string_lossy()),
        &out_path,
        &tx,
    )
    .await;
    total_cost += cost1;
    let result1 = match result1 {
        Ok(result) => result,
        Err(_) => {
            teardown(&mut terminal)?;
            return Ok(RefineOutcome {
                goal: Some(goal.to_string()),
                cost: total_cost,
            });
        }
    };

    let mut questions = result1.questions;
    questions.truncate(config::REFINE_MAX_QUESTIONS);
    let mut final_goal = result1.refined_goal;

    if !questions.is_empty() {
        let total = questions.len();
        let mut answers: Vec<(String, String)> = Vec::new();
        for (index, question) in questions.iter().enumerate() {
            let mut buffer = String::new();
            let outcome = loop {
                terminal
                    .draw(|f| ui::render_refine_question(f, question, index + 1, total, &buffer))?;
                if let Event::Key(key) = crossterm::event::read()? {
                    match (key.code, key.modifiers) {
                        (KeyCode::Char('c'), KeyModifiers::CONTROL) => break Answered::Cancel,
                        (KeyCode::Esc, _) => break Answered::SkipRefine,
                        (KeyCode::Enter, _) => break Answered::Entered,
                        (KeyCode::Backspace, _) => {
                            buffer.pop();
                        }
                        (KeyCode::Char(c), _) => buffer.push(c),
                        _ => {}
                    }
                }
            };
            match outcome {
                Answered::Cancel => {
                    teardown(&mut terminal)?;
                    return Ok(RefineOutcome {
                        goal: None,
                        cost: total_cost,
                    });
                }
                Answered::SkipRefine => {
                    teardown(&mut terminal)?;
                    return Ok(RefineOutcome {
                        goal: Some(goal.to_string()),
                        cost: total_cost,
                    });
                }
                Answered::Entered => {
                    answers.push((question.clone(), buffer.trim().to_string()));
                }
            }
        }

        // Pass 2: fold the answers into a final goal. Keep pass 1's rewrite if
        // this pass fails.
        terminal.draw(|f| ui::render_refining(f, "folding your answers into a final goal"))?;
        let (cost2, result2) = run_refine_pass(
            repo,
            &config::refine_finalize_prompt(goal, &answers, &out_path.to_string_lossy()),
            &out_path,
            &tx,
        )
        .await;
        total_cost += cost2;
        if let Ok(result2) = result2 {
            final_goal = result2.refined_goal;
        }
    }

    // Confirm screen: the user has the last word on the goal.
    let mut buffer = final_goal;
    let confirmed = loop {
        terminal.draw(|f| ui::render_goal_confirm(f, &buffer))?;
        if let Event::Key(key) = crossterm::event::read()? {
            match (key.code, key.modifiers) {
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => break None,
                (KeyCode::Esc, _) => break Some(goal.to_string()),
                (KeyCode::Enter, _) => {
                    // A line ending in a backslash continues onto a new line;
                    // otherwise Enter accepts the goal.
                    if buffer.ends_with('\\') {
                        buffer.pop();
                        buffer.push('\n');
                    } else if !buffer.trim().is_empty() {
                        break Some(buffer.trim().to_string());
                    }
                }
                (KeyCode::Backspace, _) => {
                    buffer.pop();
                }
                (KeyCode::Char(c), _) => buffer.push(c),
                _ => {}
            }
        }
    };

    teardown(&mut terminal)?;
    Ok(RefineOutcome {
        goal: confirmed,
        cost: total_cost,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_goal_with_questions() {
        let json = r#"{"refined_goal":"Add a health check endpoint at /healthz","questions":["Which port?","Auth required?"]}"#;
        let result = parse_refine(json).unwrap();
        assert_eq!(
            result.refined_goal,
            "Add a health check endpoint at /healthz"
        );
        assert_eq!(result.questions, vec!["Which port?", "Auth required?"]);
    }

    #[test]
    fn a_missing_questions_field_defaults_to_empty() {
        let result = parse_refine(r#"{"refined_goal":"Do the thing"}"#).unwrap();
        assert!(result.questions.is_empty());
    }

    #[test]
    fn an_empty_refined_goal_is_an_error() {
        assert!(parse_refine(r#"{"refined_goal":"   ","questions":[]}"#).is_err());
    }

    #[test]
    fn malformed_json_is_an_error() {
        assert!(parse_refine("not json at all").is_err());
    }
}
