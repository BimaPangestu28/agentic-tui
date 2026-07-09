# Goal Refine Step Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Insert an optional one-round goal-refine step between goal entry and planning: a `claude -p` pass rewrites the goal and proposes clarifying questions, the user answers them one at a time, a second pass finalizes the goal, and the user confirms it before the Plan stage runs.

**Architecture:** Reuses the `plan.json` pattern. Each refine pass is a one-shot `engine::run_stage` invocation told to write `.agentic-refine.json`, which we parse. A blocking `refine::run` flow (like `run_picker`/`run_onboarding`) drives the screens; it is `async` because it awaits `run_stage`. `engine.rs` is unchanged. On by default; `--no-refine` or Esc skips it; any failure falls back to the original goal.

**Tech Stack:** Rust 2021, ratatui + crossterm (TUI), tokio (async), serde/serde_json (parse), anyhow (errors).

## Global Constraints

- Edition 2021; keep `Cargo.lock` pinned; do not run `cargo update`.
- No `unwrap()`/`expect()`/`panic!` in production code (tests may use them).
- Comment/prose style: no em dashes, no contractions in English prose.
- Descriptive names; verbs for functions, nouns for types.
- Every task leaves `make verify` (fmt-check, clippy `--all-targets -- -D warnings`, tests) green.
- **Binary-crate dead-code reality:** this is a `bin` crate, so an item referenced only from a `#[cfg(test)]` module is still dead in the normal build and `clippy -D warnings` fails on it. `RefineResult`, `parse_refine`, `RefineOutcome` (Task 1) and the refine consts and prompt functions (Task 2) are therefore each annotated with `#[allow(dead_code)]` as temporary scaffolding. Task 3 wires them all into the binary path and removes every one of these `#[allow(dead_code)]` attributes as an explicit step, verifying the gate stays green without them. No `#[allow(dead_code)]` may remain after Task 3.
- Conventional commits. Commit after every task. Work on branch `feat/goal-refine`.

## File Structure

| File | Change |
|---|---|
| `src/refine.rs` | new: `RefineResult`, `parse_refine`, `RefineOutcome`, `run`, `run_refine_pass`, `teardown`; tests |
| `src/config.rs` | refine knobs (`MODEL_REFINE`, `REFINE_TOOLS`, `REFINE_MAX_TURNS`, `REFINE_BUDGET_USD`, `REFINE_MAX_QUESTIONS`) and `refine_questions_prompt`, `refine_finalize_prompt` |
| `src/ui.rs` | `render_refining`, `render_refine_question`, `render_goal_confirm` |
| `src/main.rs` | `mod refine;`, `--no-refine` arg, call `refine::run` before the pipeline, thread refine cost into `run_pipeline` |
| `.gitignore` | ignore `.agentic-refine.json` |
| `README.md` | document the refine step and `--no-refine` |

---

### Task 1: Refine result parsing

**Files:**
- Create: `src/refine.rs`
- Modify: `src/main.rs` (add `mod refine;`)

**Interfaces:**
- Produces:
  - `pub struct RefineResult { pub refined_goal: String, pub questions: Vec<String> }` (derives `Debug, Clone, Deserialize`; `questions` has `#[serde(default)]`).
  - `pub struct RefineOutcome { pub goal: Option<String>, pub cost: f64 }` (derives `Debug, Clone`).
  - `pub fn parse_refine(json: &str) -> anyhow::Result<RefineResult>`.

- [ ] **Step 1: Create `src/refine.rs` with the parser and a failing test**

Create `src/refine.rs`:

```rust
//! The goal-refine step: a claude pass rewrites the goal and proposes
//! clarifying questions written to `.agentic-refine.json`, the user answers
//! them, a second pass finalizes the goal, and the user confirms it. Reuses the
//! `plan.json` pattern: each pass writes a JSON file we parse here.

use serde::Deserialize;

/// The JSON a refine pass writes to `.agentic-refine.json`.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct RefineResult {
    pub refined_goal: String,
    #[serde(default)]
    pub questions: Vec<String>,
}

/// The result of the whole refine flow. `goal` is `None` only when the user
/// cancelled the run.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RefineOutcome {
    pub goal: Option<String>,
    pub cost: f64,
}

/// Parse the JSON a refine pass wrote. A missing `questions` defaults to empty;
/// an empty `refined_goal` or malformed JSON is an error, which the caller turns
/// into a fall back to the original goal.
#[allow(dead_code)]
pub fn parse_refine(json: &str) -> anyhow::Result<RefineResult> {
    let result: RefineResult = serde_json::from_str(json)?;
    if result.refined_goal.trim().is_empty() {
        anyhow::bail!("refine result has an empty refined_goal");
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_goal_with_questions() {
        let json = r#"{"refined_goal":"Add a health check endpoint at /healthz","questions":["Which port?","Auth required?"]}"#;
        let result = parse_refine(json).unwrap();
        assert_eq!(result.refined_goal, "Add a health check endpoint at /healthz");
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
```

- [ ] **Step 2: Declare the module in `src/main.rs`**

In `src/main.rs`, the module declarations are:

```rust
mod app;
mod config;
mod engine;
mod event;
mod orchestrator;
mod plan;
mod ui;
mod workspace;
mod worktree;
```

Add `mod refine;` in alphabetical position (after `mod plan;`):

```rust
mod plan;
mod refine;
mod ui;
```

- [ ] **Step 3: Run the tests to verify they fail, then pass**

Run: `cargo test refine:: 2>&1 | tail -20`
Expected first: the file compiles and the four tests pass (this task is transcription of a known-good parser, so RED is trivial; if any test fails, fix the parser to match the asserted behavior).

- [ ] **Step 4: Verify the gate is green**

Run: `make verify`
Expected: fmt-check, clippy (the `#[allow(dead_code)]` attributes keep the new items from failing the dead-code check), and all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/refine.rs src/main.rs
git commit -m "feat: parse the goal-refine result file

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Refine config knobs and prompts

**Files:**
- Modify: `src/config.rs`

**Interfaces:**
- Produces: `MODEL_REFINE`, `REFINE_TOOLS`, `REFINE_MAX_TURNS`, `REFINE_BUDGET_USD`, `REFINE_MAX_QUESTIONS`, `refine_questions_prompt(goal, out_path)`, `refine_finalize_prompt(goal, answers, out_path)`.

- [ ] **Step 1: Add the refine knobs**

In `src/config.rs`, after the `EPIC_MAX_TURNS` line (`pub const EPIC_MAX_TURNS: u32 = 40;`), add:

```rust
// Refine stage. Sharpening the goal does not need opus, so it defaults to
// sonnet. Read-only plus Write (for the result file), no Edit or Bash.
#[allow(dead_code)]
pub const MODEL_REFINE: &str = "sonnet";
#[allow(dead_code)]
pub const REFINE_TOOLS: &str = "Read,Glob,Grep,Write,WebSearch,WebFetch,Skill";
#[allow(dead_code)]
pub const REFINE_MAX_TURNS: u32 = 12;
#[allow(dead_code)]
pub const REFINE_BUDGET_USD: f64 = 0.20;
#[allow(dead_code)]
pub const REFINE_MAX_QUESTIONS: usize = 5;
```

- [ ] **Step 2: Add the two refine prompts**

In `src/config.rs`, after `plan_prompt` (before `epic_prompt`), add:

```rust
/// Prompt for the first refine pass. Claude reads the repo, rewrites the goal to
/// be specific, and lists clarifying questions, writing them to a JSON file.
#[allow(dead_code)]
pub fn refine_questions_prompt(goal: &str, out_path: &str) -> String {
    format!(
        "You are a Tech Lead sharpening a goal before planning work on a \
repository. {style}\n\n\
GOAL:\n{goal}\n\n\
Step 1. Understand this repository with Glob and Grep so your rewrite and \
questions fit the real code.\n\
Step 2. Rewrite the goal so it is specific and actionable.\n\
Step 3. List at most {max} clarifying questions whose answers would materially \
change the plan. Ask only genuinely useful questions. If the goal is already \
clear, use an empty list.\n\
Step 4. Write ONLY a JSON file to {out} with this exact shape and nothing else:\n\
{{\"refined_goal\":\"...\",\"questions\":[\"...\"]}}\n\
Do not write any other file.",
        style = STYLE,
        goal = goal,
        max = REFINE_MAX_QUESTIONS,
        out = out_path,
    )
}

/// Prompt for the second refine pass. Given the original goal and the user's
/// answers, produce one final goal, writing it to the same JSON file.
#[allow(dead_code)]
pub fn refine_finalize_prompt(goal: &str, answers: &[(String, String)], out_path: &str) -> String {
    let qa: String = answers
        .iter()
        .map(|(question, answer)| {
            let answer = if answer.is_empty() { "(no answer)" } else { answer };
            format!("Q: {question}\nA: {answer}\n")
        })
        .collect();
    format!(
        "You are a Tech Lead finalizing a goal before planning. {style}\n\n\
ORIGINAL GOAL:\n{goal}\n\n\
CLARIFICATIONS:\n{qa}\n\
Produce one specific, actionable goal statement that folds in the answers \
above. Write ONLY a JSON file to {out} with this exact shape and nothing \
else:\n\
{{\"refined_goal\":\"...\",\"questions\":[]}}\n\
Do not write any other file.",
        style = STYLE,
        goal = goal,
        qa = qa,
        out = out_path,
    )
}
```

- [ ] **Step 3: Verify the gate is green**

Run: `make verify`
Expected: all green (the `#[allow(dead_code)]` attributes keep the new consts and functions from failing the dead-code check).

- [ ] **Step 4: Commit**

```bash
git add src/config.rs
git commit -m "feat: add refine stage knobs and prompts

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Wizard screens, refine flow, and wiring

This task lands the interactive layer in one commit; each new piece is used only by the next, so splitting would leave an unused item and fail clippy. It also removes all the `#[allow(dead_code)]` scaffolding from Tasks 1 and 2. No unit tests for the TUI, consistent with `ui.rs`; verification is `make verify` plus a manual/pty smoke test noted below.

**Files:**
- Modify: `src/ui.rs`
- Modify: `src/refine.rs`
- Modify: `src/config.rs` (remove allows)
- Modify: `src/main.rs`
- Modify: `.gitignore`

**Interfaces:**
- Consumes: `engine::run_stage`, `engine::StageSpec`, `config::MODEL_REFINE`/`REFINE_TOOLS`/`REFINE_MAX_TURNS`/`REFINE_BUDGET_USD`/`REFINE_MAX_QUESTIONS`/`refine_questions_prompt`/`refine_finalize_prompt`, `parse_refine`, `RefineResult`, `RefineOutcome`, `crate::event::AppEvent`, `ui` render functions.
- Produces:
  - `ui`: `render_refining`, `render_refine_question`, `render_goal_confirm`.
  - `refine`: `pub async fn run(repo: &Path, goal: &str) -> anyhow::Result<RefineOutcome>`.
  - `main`: `Args.no_refine: bool`; `run_pipeline` gains a `refine_cost: f64` parameter.

- [ ] **Step 1: Add the three refine render functions to `src/ui.rs`**

`src/ui.rs` already imports `Layout`, `Constraint`, `Direction`, `Color`, `Style`, `Line`, `Span`, `Block`, `Borders`, `Paragraph`, `Wrap`, `Frame`. Add after `render_goal_input`:

```rust
/// Status frame shown while a refine pass runs.
pub fn render_refining(f: &mut Frame, note: &str) {
    let area = f.area();
    let message = Paragraph::new(format!("Refining the goal: {note}..."))
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title(" Refine "));
    f.render_widget(message, area);
}

/// One clarifying question with an editable answer field.
pub fn render_refine_question(
    f: &mut Frame,
    question: &str,
    index: usize,
    total: usize,
    answer: &str,
) {
    let area = f.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(area);
    let prompt = Paragraph::new(question.to_string())
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Question {index} of {total} ")),
        );
    f.render_widget(prompt, rows[0]);
    let input = Paragraph::new(Line::from(vec![
        Span::raw(answer.to_string()),
        Span::styled("\u{2588}", Style::default().fg(Color::Cyan)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Answer (Enter next, empty to skip, Esc to skip refining) "),
    );
    f.render_widget(input, rows[1]);
}

/// The final refined goal in an editable field, confirmed before planning.
pub fn render_goal_confirm(f: &mut Frame, goal: &str) {
    let area = f.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);
    let input = Paragraph::new(Line::from(vec![
        Span::raw(goal.to_string()),
        Span::styled("\u{2588}", Style::default().fg(Color::Cyan)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Final goal (edit if needed, Enter to plan, Esc to use original) "),
    );
    f.render_widget(input, rows[0]);
}
```

- [ ] **Step 2: Add the refine flow to `src/refine.rs`**

Add these imports at the top of `src/refine.rs`, below the existing `use serde::Deserialize;`:

```rust
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
```

Then add, after `parse_refine`:

```rust
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

/// Run one refine pass: a one-shot claude session that writes the result JSON,
/// which we read and parse. Returns the parsed result and the pass cost.
async fn run_refine_pass(
    repo: &Path,
    prompt: &str,
    out_path: &Path,
    tx: &mpsc::UnboundedSender<AppEvent>,
) -> anyhow::Result<(RefineResult, f64)> {
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
    let outcome = engine::run_stage(&spec, tx).await?;
    let text = std::fs::read_to_string(out_path)
        .map_err(|e| anyhow::anyhow!(".agentic-refine.json was not written: {e}"))?;
    let result = parse_refine(&text)?;
    Ok((result, outcome.cost))
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
    let (result1, cost1) = match run_refine_pass(
        repo,
        &config::refine_questions_prompt(goal, &out_path.to_string_lossy()),
        &out_path,
        &tx,
    )
    .await
    {
        Ok(pass) => pass,
        Err(_) => {
            teardown(&mut terminal)?;
            return Ok(RefineOutcome {
                goal: Some(goal.to_string()),
                cost: total_cost,
            });
        }
    };
    total_cost += cost1;

    let mut questions = result1.questions;
    questions.truncate(config::REFINE_MAX_QUESTIONS);
    let mut final_goal = result1.refined_goal;

    if !questions.is_empty() {
        let total = questions.len();
        let mut answers: Vec<(String, String)> = Vec::new();
        for (index, question) in questions.iter().enumerate() {
            let mut buffer = String::new();
            let outcome = loop {
                terminal.draw(|f| {
                    ui::render_refine_question(f, question, index + 1, total, &buffer)
                })?;
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
        if let Ok((result2, cost2)) = run_refine_pass(
            repo,
            &config::refine_finalize_prompt(goal, &answers, &out_path.to_string_lossy()),
            &out_path,
            &tx,
        )
        .await
        {
            total_cost += cost2;
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
                (KeyCode::Enter, _) if !buffer.trim().is_empty() => {
                    break Some(buffer.trim().to_string());
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
```

- [ ] **Step 3: Remove the `#[allow(dead_code)]` scaffolding**

Every scaffolded item is now reached from the binary path (`parse_refine`, `RefineResult`, `RefineOutcome` via `refine::run`; the config consts and prompts via `run`/`run_refine_pass`). Delete every `#[allow(dead_code)]` attribute added in Tasks 1 and 2:

- In `src/refine.rs`: on `RefineResult`, `RefineOutcome`, `parse_refine`.
- In `src/config.rs`: on `MODEL_REFINE`, `REFINE_TOOLS`, `REFINE_MAX_TURNS`, `REFINE_BUDGET_USD`, `REFINE_MAX_QUESTIONS`, `refine_questions_prompt`, `refine_finalize_prompt`.

Verify none remain:

```bash
grep -rn "allow(dead_code)" src/
# expect: no output
```

- [ ] **Step 4: Add `--no-refine` to `Args` and `parse_args` in `src/main.rs`**

Change the `Args` struct:

```rust
struct Args {
    goal: String,
    workspace: Option<String>,
    verify: Option<String>,
}
```

to:

```rust
struct Args {
    goal: String,
    workspace: Option<String>,
    verify: Option<String>,
    no_refine: bool,
}
```

In `parse_args`, add the flag. The loop currently is:

```rust
    let mut workspace = None;
    let mut verify = None;
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
            other => goal_parts.push(other.to_string()),
        }
        i += 1;
    }
    let goal = goal_parts.join(" ").trim().to_string();
    Some(Args {
        goal,
        workspace,
        verify,
    })
```

Change it to add `no_refine`:

```rust
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
```

Update the usage string in `main` to mention the flag:

```rust
            eprintln!(
                "usage: agentic-tui [\"<goal>\"] [--workspace <name|path>] [--verify \"<cmd>\"]"
            );
```

becomes:

```rust
            eprintln!(
                "usage: agentic-tui [\"<goal>\"] [--workspace <name|path>] [--verify \"<cmd>\"] [--no-refine]"
            );
```

- [ ] **Step 5: Call the refine flow in `main` and thread its cost**

In `src/main.rs`, the goal block and `App::new` are:

```rust
    let goal = if args.goal.is_empty() {
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

    let mut app = App::new(
        goal.clone(),
        selected.name.clone(),
        config::GLOBAL_BUDGET_USD,
    );
```

Replace with (make `goal` mutable, run refine before building the app):

```rust
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
```

- [ ] **Step 6: Pass the refine cost into the pipeline**

The pipeline spawn is:

```rust
    let pipeline_tx = tx.clone();
    let repo_run = repo.clone();
    let goal_run = goal.clone();
    let verify_run = verify_cmd.clone();
    let pipeline_handle = tokio::spawn(async move {
        if let Err(e) = run_pipeline(&repo_run, &goal_run, &verify_run, &pipeline_tx).await {
            let _ = pipeline_tx.send(AppEvent::Fatal(e.to_string()));
        }
    });
```

Change it to capture and pass `refine_cost`:

```rust
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
```

- [ ] **Step 7: Update `run_pipeline` to account for the refine cost**

The signature and top of `run_pipeline` are:

```rust
async fn run_pipeline(
    repo: &std::path::Path,
    goal: &str,
    verify_cmd: &str,
    tx: &mpsc::UnboundedSender<AppEvent>,
) -> anyhow::Result<()> {
    let plan_path = repo.join(".agentic-plan.json");
```

Change the signature to add `refine_cost` and report it as spent immediately:

```rust
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
```

Find the `RunConfig` construction near the end of `run_pipeline`. It sets `initial_cost: outcome.cost`. Change that single field to include the refine cost:

```rust
        initial_cost: refine_cost + outcome.cost,
```

(Leave every other `RunConfig` field unchanged.)

- [ ] **Step 8: Gitignore the refine result file**

`.gitignore` already ignores `/.agentic-plan.json` under an "Orchestrator scratch state" comment. Add the refine file beside it:

```
# Orchestrator scratch state
/.agentic-plan.json
/.agentic-refine.json
/.agentic-worktrees/
```

- [ ] **Step 9: Verify the gate is green**

Run: `make verify`
Expected: fmt-check, clippy (no dead-code, no remaining allows), and all tests pass. If clippy reports any refine item as dead, it is not wired in yet; wire it rather than re-adding an allow.

- [ ] **Step 10: Manual verification (controller performs the interactive smoke test)**

The refine flow launches `claude`, which costs money, and any bare `cargo run` launches the interactive TUI (there is no `--help` flag; unknown args become the goal), so do NOT run the binary here. Confirm instead:

```bash
cargo build                        # compiles
grep -rn "allow(dead_code)" src/   # expect: no output
```

Note in your report that the interactive refine flow (which spends budget on a real `claude` call) is deferred to the controller for a manual or pty smoke test, exactly as the onboarding wizard was.

- [ ] **Step 11: Commit**

```bash
git add src/ui.rs src/refine.rs src/config.rs src/main.rs .gitignore
git commit -m "feat: refine the goal with a clarifying pass before planning

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Documentation

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Document the refine step in the Run section**

In `README.md`, the Run section explains the goal flow. After the paragraph that introduces `cargo run -- "<goal>"`, add a paragraph describing refine. Find:

```markdown
Override the verify command per run with `--verify`:
```

Insert this block immediately before it:

```markdown
Before planning, the tool runs a goal-refine step: a short `claude` pass reads
the repository, rewrites your goal to be more specific, and may ask a few
clarifying questions. You answer them one at a time, a second pass folds the
answers into a final goal, and you confirm (and can edit) that goal before
planning starts. Skip the whole step with `--no-refine`, or press Esc during it
to plan with your original goal. The refine cost counts toward the run budget.

```

- [ ] **Step 2: Document `--no-refine` where the other flags live**

In `README.md`, the "Config knobs" section lists tunables. Add a line about the refine models and budget after the `MODEL_PLAN`/`MODEL_EPIC` bullet:

```markdown
- `MODEL_REFINE`, `REFINE_BUDGET_USD`, and `REFINE_MAX_QUESTIONS` tune the
  goal-refine step (model, per-pass budget, and how many clarifying questions it
  may ask). Pass `--no-refine` on the command line to skip refining entirely.
```

- [ ] **Step 3: Verify the gate is green**

Run: `make verify`
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: document the goal-refine step

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Notes for the implementer

- `refine::run` awaits `engine::run_stage` (async) but uses a blocking
  `crossterm::event::read()` for the answer and confirm screens, exactly like
  `run_onboarding`. This is fine: the tick task and input thread are not spawned
  until after refining returns.
- The throwaway `mpsc` channel (`_rx` held, never read) absorbs the refine
  session's stream events; `run_stage` ignores send results, so nothing breaks.
- Refine writes `.agentic-refine.json` at the repo root and removes it before
  each pass to avoid reading a stale file from a failed prior pass.
- Do not add cycle/symlink handling or new abstractions here; keep the change
  scoped to the refine step.
