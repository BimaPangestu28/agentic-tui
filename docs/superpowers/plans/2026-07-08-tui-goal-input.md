# TUI Goal Input Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the operator type the goal in the TUI when it is not passed on the command line.

**Architecture:** Add a blocking goal input screen (same pattern as the workspace picker) that runs after the workspace is resolved when no goal was given on the CLI. Presentation/entry only; the pipeline is unchanged.

**Tech Stack:** Rust 2021, ratatui + crossterm.

## Global Constraints

- Edition 2021; keep `Cargo.lock` pinned; do not run `cargo update`.
- Comment/prose style: direct, no em dashes, no contractions.
- Presentation-only: change only `src/ui.rs` and `src/main.rs`.
- Goal stays an optional CLI positional argument (fast path); the screen only appears when it is empty.

## File Structure

| File | Change |
|---|---|
| `src/ui.rs` | Add `render_goal_input` |
| `src/main.rs` | Add `run_goal_input`; `parse_args` stops erroring on empty goal; `main` prompts when goal is empty |

---

### Task 1: Goal input screen and wiring

This is one coupled change: the `ui.rs` renderer and the `main.rs` loop that
uses it land together so nothing is left unused (green build). No unit test (the
blocking key loop needs a TTY, like the existing picker); verification is build,
lint, and a manual run.

**Files:**
- Modify: `src/ui.rs`
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: existing `ui`/terminal setup helpers.
- Produces:
  - `pub fn render_goal_input(f: &mut Frame, workspace: &str, buffer: &str)`
  - `fn run_goal_input(workspace: &str) -> anyhow::Result<Option<String>>`

- [ ] **Step 1: Add `render_goal_input` to `src/ui.rs`**

Add this function (all types it uses are already imported in `ui.rs`):

```rust
/// Goal input screen shown when no goal was given on the command line.
pub fn render_goal_input(f: &mut Frame, workspace: &str, buffer: &str) {
    let area = f.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);
    let title = format!(" Goal for {workspace} (Enter to run, Esc to cancel) ");
    let input = Paragraph::new(Line::from(vec![
        Span::raw(buffer.to_string()),
        Span::styled("\u{2588}", Style::default().fg(Color::Cyan)),
    ]))
    .block(Block::default().borders(Borders::ALL).title(title));
    f.render_widget(input, rows[0]);
}
```

- [ ] **Step 2: Add `run_goal_input` to `src/main.rs`**

Add this function next to `run_picker` (it reuses the same imports):

```rust
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
                KeyCode::Enter => {
                    if !buffer.trim().is_empty() {
                        break Some(buffer.trim().to_string());
                    }
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
```

- [ ] **Step 3: Stop `parse_args` from erroring on an empty goal**

In `parse_args`, the final block currently is:

```rust
    let goal = goal_parts.join(" ").trim().to_string();
    if goal.is_empty() {
        None
    } else {
        Some(Args { goal, workspace, verify })
    }
```

Replace it with (an empty goal is now valid; it is prompted later):

```rust
    let goal = goal_parts.join(" ").trim().to_string();
    Some(Args { goal, workspace, verify })
```

- [ ] **Step 4: Prompt for the goal in `main` when it is empty**

In `main`, after `workspace::validate(&selected)?;` and the `repo`/`verify_cmd`
lines, the code currently builds `App` directly from `args.goal`. Insert a goal
resolution step before `App::new`, and use its result. Add:

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
```

Then change the `App::new` call and the pipeline goal to use `goal` instead of
`args.goal`. Concretely:
- `let mut app = App::new(args.goal.clone(), selected.name.clone(), config::GLOBAL_BUDGET_USD);`
  becomes
  `let mut app = App::new(goal.clone(), selected.name.clone(), config::GLOBAL_BUDGET_USD);`
- `let goal_run = args.goal.clone();` becomes `let goal_run = goal.clone();`

Leave the rest of `main` unchanged.

- [ ] **Step 5: Update the usage string**

`parse_args` no longer requires a goal, so the `main` usage line printed when
`parse_args` returns `None` is now unreachable for the empty-goal case, but keep
`parse_args` returning `Option` and the existing `match`. Update the usage text
to show the goal is optional:

```rust
            eprintln!("usage: agentic-tui [\"<goal>\"] [--workspace <name|path>] [--verify \"<cmd>\"]");
```

- [ ] **Step 6: Build and lint**

Run: `make verify 2>&1 | tail -12`
Expected: PASS (fmt-check, clippy `-D warnings`, all 28 tests). Fix any clippy finding inline (for example an unused import) and re-run until clean.

- [ ] **Step 7: Headless arg-parse check**

Run: `cargo run -- "Add X" --workspace /tmp 2>&1 | head -3`
Expected: it does not print the usage error for the goal (the goal is provided). It may fail later on the workspace not being a git repo; that is fine, it proves the goal path is taken without prompting.

- [ ] **Step 8: Manual check (needs a real terminal)**

Run this yourself in a TTY:

```bash
mkdir -p /tmp/goal-smoke && cd /tmp/goal-smoke && git init -b main && printf '# smoke\n' > README.md && git add -A && git commit -m init
cd /Users/bimapangestu/Desktop/Works/personal/claude-agentic-loop
cargo run -- --workspace /tmp/goal-smoke --verify "true"
```

Expected: no goal on the CLI, so after the workspace is set the goal input screen
appears; type a goal, press Enter, and the run starts. Press Esc instead to
confirm it exits with "no goal entered". If `claude` cannot run here, at least
confirm the input screen appears and accepts text.

- [ ] **Step 9: Commit**

```bash
git add src/ui.rs src/main.rs
git commit -m "feat: enter the goal in the TUI when omitted on the command line"
```

---

## Self-Review

**Spec coverage:**
- Goal optional on CLI, prompted when empty — Steps 3, 4. ✓
- Prompt after workspace resolved, names the workspace — Step 4 passes `selected.name` to `run_goal_input`; the title shows it (Step 1). ✓
- Enter submits non-empty, Backspace, Esc/Ctrl-C cancels — Step 2. ✓
- Same raw-mode/alternate-screen pattern as the picker — Step 2 mirrors `run_picker`. ✓
- Presentation-only, only `ui.rs`/`main.rs` — all steps. ✓

**Placeholder scan:** No TODO/TBD; Step 8 is a manual TTY check, stated honestly; all code steps carry complete code.

**Type consistency:** `run_goal_input` returns `anyhow::Result<Option<String>>`, consumed by the `match` in Step 4. `render_goal_input(f, workspace, buffer)` signature matches its call in Step 2. `goal` (String) replaces `args.goal` at both use sites (Step 4). `KeyCode`/`KeyModifiers`/`Event` used in `run_goal_input` are already imported in `main.rs` for the event loop.
