# Kanban Board View Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the flat epic list in the run TUI with a five-column kanban board (Todo, In Progress, Review, Done, Blocked) that shows every epic from plan time, including waiting/hold epics.

**Architecture:** Presentation-only. The pipeline, scheduler, worktree, and engine are untouched. Epics are seeded into `App` as `Pending` cards when the plan is validated (via an enriched `PlanReady` event), lifecycle events update card status in place, and rendering buckets cards into columns using two pure helpers. Hold vs ready inside Todo is derived at render time from dependency status.

**Tech Stack:** Rust 2021, ratatui + crossterm (TUI), tokio (unchanged).

## Green-build invariant

Every task leaves the whole crate compiling, all tests passing, and `make verify`
(fmt-check, clippy `--all-targets -- -D warnings`, tests) green. The reintroduced
`EpicStatus::Pending` variant is exercised by tests from the task that adds it, so
`--all-targets` clippy never sees it as dead code.

## Global Constraints

- Edition 2021; keep `Cargo.lock` pinned; do not run `cargo update`.
- Comment/prose style: direct, no em dashes, no contractions in English prose.
- Descriptive names; verbs for functions, nouns for types.
- Presentation-only: do not change `engine.rs`, `orchestrator.rs`, `worktree.rs`, `plan.rs`, `workspace.rs`, or `config.rs`.
- Columns and mapping are fixed: Todo=`Pending`, In Progress=`Running`, Review=`Verifying`, Done=`Merged`, Blocked=`Failed`/`Skipped`/`Conflict`.
- Commit after every task.

## File Structure

| File | Change |
|---|---|
| `src/app.rs` | Reintroduce `EpicStatus::Pending`; add `KanbanColumn`, `kanban_column`, `is_on_hold`; `EpicView.depends_on`; seed cards on `PlanReady` |
| `src/event.rs` | `PlanReady` carries `Vec<EpicMeta>`; add `EpicMeta` |
| `src/main.rs` | `run_pipeline` builds and sends the epic list |
| `src/ui.rs` | Replace `render_epics` with `render_board`; add `EpicStatus::Pending` glyph |

---

### Task 1: Pure kanban helpers

**Files:**
- Modify: `src/app.rs`
- Modify: `src/ui.rs`

**Interfaces:**
- Consumes: existing `EpicStatus`.
- Produces:
  - `EpicStatus::Pending` variant (reintroduced).
  - `pub enum KanbanColumn { Todo, InProgress, Review, Done, Blocked }` (derives `Clone, Copy, PartialEq, Debug`).
  - `pub fn kanban_column(status: EpicStatus) -> KanbanColumn`
  - `pub fn is_on_hold(depends_on: &[String], status_by_id: &HashMap<String, EpicStatus>) -> bool`

- [ ] **Step 1: Add the `HashMap` import to `src/app.rs`**

At the top of `src/app.rs`, the imports currently include `use std::collections::VecDeque;`. Change it to:

```rust
use std::collections::{HashMap, VecDeque};
```

- [ ] **Step 2: Reintroduce the `Pending` variant**

In `src/app.rs`, add `Pending` as the first variant of `EpicStatus`:

```rust
#[derive(Clone, Copy, PartialEq)]
pub enum EpicStatus {
    Pending,
    Running,
    Verifying,
    Merged,
    Failed,
    Skipped,
    Conflict,
}
```

- [ ] **Step 3: Write the failing tests**

At the end of `src/app.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kanban_column_maps_each_status() {
        assert_eq!(kanban_column(EpicStatus::Pending), KanbanColumn::Todo);
        assert_eq!(kanban_column(EpicStatus::Running), KanbanColumn::InProgress);
        assert_eq!(kanban_column(EpicStatus::Verifying), KanbanColumn::Review);
        assert_eq!(kanban_column(EpicStatus::Merged), KanbanColumn::Done);
        assert_eq!(kanban_column(EpicStatus::Failed), KanbanColumn::Blocked);
        assert_eq!(kanban_column(EpicStatus::Skipped), KanbanColumn::Blocked);
        assert_eq!(kanban_column(EpicStatus::Conflict), KanbanColumn::Blocked);
    }

    #[test]
    fn is_on_hold_true_when_a_dependency_is_not_merged() {
        let mut status_by_id = HashMap::new();
        status_by_id.insert("a".to_string(), EpicStatus::Running);
        assert!(is_on_hold(&["a".to_string()], &status_by_id));
    }

    #[test]
    fn is_on_hold_false_when_all_dependencies_merged() {
        let mut status_by_id = HashMap::new();
        status_by_id.insert("a".to_string(), EpicStatus::Merged);
        status_by_id.insert("b".to_string(), EpicStatus::Merged);
        assert!(!is_on_hold(&["a".to_string(), "b".to_string()], &status_by_id));
    }

    #[test]
    fn is_on_hold_true_for_an_unknown_dependency() {
        let status_by_id = HashMap::new();
        assert!(is_on_hold(&["ghost".to_string()], &status_by_id));
    }

    #[test]
    fn no_dependencies_is_never_on_hold() {
        let status_by_id = HashMap::new();
        assert!(!is_on_hold(&[], &status_by_id));
    }
}
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test app:: 2>&1 | tail -20`
Expected: FAIL to compile ("cannot find type `KanbanColumn`", "cannot find function `kanban_column`").

- [ ] **Step 5: Add the enum and the two helpers**

In `src/app.rs`, after the `EpicStatus` enum, add:

```rust
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum KanbanColumn {
    Todo,
    InProgress,
    Review,
    Done,
    Blocked,
}

/// Map an epic status to its kanban column.
pub fn kanban_column(status: EpicStatus) -> KanbanColumn {
    match status {
        EpicStatus::Pending => KanbanColumn::Todo,
        EpicStatus::Running => KanbanColumn::InProgress,
        EpicStatus::Verifying => KanbanColumn::Review,
        EpicStatus::Merged => KanbanColumn::Done,
        EpicStatus::Failed | EpicStatus::Skipped | EpicStatus::Conflict => KanbanColumn::Blocked,
    }
}

/// True when any dependency has not yet merged, so a pending epic is on hold.
pub fn is_on_hold(depends_on: &[String], status_by_id: &HashMap<String, EpicStatus>) -> bool {
    depends_on
        .iter()
        .any(|dep| status_by_id.get(dep) != Some(&EpicStatus::Merged))
}
```

- [ ] **Step 6: Add the `Pending` glyph in `src/ui.rs`**

The `status_glyph` match in `src/ui.rs` must stay exhaustive now that `Pending` exists. Add this arm as the first arm:

```rust
        EpicStatus::Pending => ("pending  ", Color::DarkGray),
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test app:: 2>&1 | tail -20`
Expected: PASS (5 tests).

- [ ] **Step 8: Verify the whole crate and lints**

Run: `make verify 2>&1 | tail -12`
Expected: PASS (fmt-check, clippy `-D warnings`, all tests). `Pending` is constructed in the tests, so `--all-targets` clippy does not flag it as dead.

- [ ] **Step 9: Commit**

```bash
git add src/app.rs src/ui.rs
git commit -m "feat: add pure kanban column helpers"
```

---

### Task 2: Seed epics on PlanReady

**Files:**
- Modify: `src/event.rs`
- Modify: `src/app.rs`
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `kanban` helpers and `EpicStatus::Pending` (Task 1), `plan::Plan` (existing).
- Produces:
  - `pub struct EpicMeta { pub id: String, pub title: String, pub depends_on: Vec<String> }`
  - `AppEvent::PlanReady { epics: Vec<EpicMeta> }` (replaces `{ epic_count: usize }`).
  - `EpicView.depends_on: Vec<String>`.

- [ ] **Step 1: Change the `PlanReady` event in `src/event.rs`**

Add the `EpicMeta` struct above the enum, and change the `PlanReady` variant:

```rust
#[derive(Debug, Clone)]
pub struct EpicMeta {
    pub id: String,
    pub title: String,
    pub depends_on: Vec<String>,
}
```

Change:
```rust
    PlanReady { epic_count: usize },
```
to:
```rust
    PlanReady { epics: Vec<EpicMeta> },
```

- [ ] **Step 2: Add `depends_on` to `EpicView` in `src/app.rs`**

```rust
#[derive(Clone)]
pub struct EpicView {
    pub id: String,
    pub title: String,
    pub status: EpicStatus,
    pub cost: f64,
    pub depends_on: Vec<String>,
}
```

- [ ] **Step 3: Seed cards in the `PlanReady` handler**

In `App::apply`, replace the existing `AppEvent::PlanReady { epic_count }` arm with:

```rust
            AppEvent::PlanReady { epics } => {
                self.phase = Phase::Implementing;
                self.epics = epics
                    .into_iter()
                    .map(|meta| EpicView {
                        id: meta.id,
                        title: meta.title,
                        status: EpicStatus::Pending,
                        cost: 0.0,
                        depends_on: meta.depends_on,
                    })
                    .collect();
                self.push_log(format!("plan ready: {} epics", self.epics.len()));
            }
```

- [ ] **Step 4: Update the two `EpicView` construction fallbacks**

The `EpicStarted` and `EpicSkipped` arms currently push a new `EpicView` when the id is not found. Cards now exist from `PlanReady`, but keep the fallback for safety and add the new field. In the `EpicStarted` arm, the fallback push becomes:

```rust
                if self.epic_mut(&id).is_none() {
                    self.epics.push(EpicView {
                        id: id.clone(),
                        title: title.clone(),
                        status: EpicStatus::Running,
                        cost: 0.0,
                        depends_on: Vec::new(),
                    });
                } else {
                    self.set_status(&id, EpicStatus::Running);
                }
```

In the `EpicSkipped` arm, the fallback push becomes:

```rust
                if self.epic_mut(&id).is_none() {
                    self.epics.push(EpicView {
                        id: id.clone(),
                        title: String::new(),
                        status: EpicStatus::Skipped,
                        cost: 0.0,
                        depends_on: Vec::new(),
                    });
                } else {
                    self.set_status(&id, EpicStatus::Skipped);
                }
```

- [ ] **Step 5: Write the failing seeding test**

Add this test to the `#[cfg(test)] mod tests` block in `src/app.rs`:

```rust
    #[test]
    fn plan_ready_seeds_a_pending_card_per_epic() {
        use crate::event::{AppEvent, EpicMeta};
        let mut app = App::new("goal".to_string(), "ws".to_string(), 10.0);
        app.apply(AppEvent::PlanReady {
            epics: vec![
                EpicMeta { id: "a".to_string(), title: "A".to_string(), depends_on: vec![] },
                EpicMeta {
                    id: "b".to_string(),
                    title: "B".to_string(),
                    depends_on: vec!["a".to_string()],
                },
            ],
        });
        assert_eq!(app.epics.len(), 2);
        assert!(app.epics.iter().all(|e| e.status == EpicStatus::Pending));
        let b = app.epics.iter().find(|e| e.id == "b").unwrap();
        assert_eq!(b.depends_on, vec!["a".to_string()]);
    }
```

- [ ] **Step 6: Send the epic list from `src/main.rs`**

In `run_pipeline`, replace:
```rust
    let _ = tx.send(AppEvent::PlanReady { epic_count: parsed.epics.len() });
```
with:
```rust
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
```

- [ ] **Step 7: Run the seeding test**

Run: `cargo test app::tests::plan_ready 2>&1 | tail -20`
Expected: PASS. (The test constructs `Pending` in the bin path and reads `depends_on`, so neither is dead code.)

- [ ] **Step 8: Verify the whole crate and lints**

Run: `make verify 2>&1 | tail -12`
Expected: PASS. The flat list still renders (all epics, now including pending); the board comes in Task 3.

- [ ] **Step 9: Commit**

```bash
git add src/event.rs src/app.rs src/main.rs
git commit -m "feat: seed all epics as pending cards when the plan is ready"
```

---

### Task 3: Render the board

**Files:**
- Modify: `src/ui.rs`

**Interfaces:**
- Consumes: `App`, `EpicView`, `EpicStatus`, `KanbanColumn`, `kanban_column`, `is_on_hold` (Tasks 1-2).
- Produces: `render_board` replacing `render_epics`; updated `render` layout.

- [ ] **Step 1: Update the `crate::app` import in `src/ui.rs`**

Change the existing `use crate::app::{App, EpicStatus, EpicView, Phase};` to:

```rust
use crate::app::{is_on_hold, kanban_column, App, EpicStatus, EpicView, KanbanColumn, Phase};
```

- [ ] **Step 2: Update `render` to use a board and a fixed-height log**

Replace the `chunks` layout and the epic-list call in `render` with:

```rust
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4), // header
            Constraint::Min(8),    // board
            Constraint::Length(10), // log
            Constraint::Length(1), // footer
        ])
        .split(area);

    render_header(f, app, chunks[0]);
    render_board(f, app, chunks[1]);
    render_log(f, app, chunks[2]);
    render_footer(f, app, chunks[3]);
```

- [ ] **Step 3: Replace `render_epics` with `render_board` and a card helper**

Delete the entire `render_epics` function and add:

```rust
fn render_board(f: &mut Frame, app: &App, area: Rect) {
    use std::collections::HashMap;

    let status_by_id: HashMap<String, EpicStatus> =
        app.epics.iter().map(|epic| (epic.id.clone(), epic.status)).collect();

    let columns = [
        (KanbanColumn::Todo, " Todo "),
        (KanbanColumn::InProgress, " In Progress "),
        (KanbanColumn::Review, " Review "),
        (KanbanColumn::Done, " Done "),
        (KanbanColumn::Blocked, " Blocked "),
    ];

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(20); 5])
        .split(area);

    let card_width = cols[0].width.saturating_sub(2) as usize;
    let rows = area.height.saturating_sub(2) as usize;

    for (index, (column, title)) in columns.iter().enumerate() {
        let cards: Vec<&EpicView> = app
            .epics
            .iter()
            .filter(|epic| kanban_column(epic.status) == *column)
            .collect();

        // Reserve one row for the overflow line when needed.
        let visible = if cards.len() > rows {
            rows.saturating_sub(1)
        } else {
            cards.len()
        };

        let mut items: Vec<ListItem> = Vec::new();
        for epic in cards.iter().take(visible) {
            let (marker, color) = card_marker(epic, *column, &status_by_id);
            let text = truncate(&format!("{marker} {}", epic.id), card_width);
            items.push(ListItem::new(Line::from(Span::styled(
                text,
                Style::default().fg(color),
            ))));
        }
        if cards.len() > visible {
            items.push(ListItem::new(Line::from(Span::styled(
                format!("+{} more", cards.len() - visible),
                Style::default().fg(Color::DarkGray),
            ))));
        }

        let list =
            List::new(items).block(Block::default().borders(Borders::ALL).title(*title));
        f.render_widget(list, cols[index]);
    }
}

/// The short marker and color for an epic card, by column.
fn card_marker(
    epic: &EpicView,
    column: KanbanColumn,
    status_by_id: &std::collections::HashMap<String, EpicStatus>,
) -> (&'static str, Color) {
    match column {
        KanbanColumn::Todo => {
            if is_on_hold(&epic.depends_on, status_by_id) {
                ("hold", Color::DarkGray)
            } else {
                ("redy", Color::Cyan)
            }
        }
        KanbanColumn::InProgress => ("run ", Color::Yellow),
        KanbanColumn::Review => ("chk ", Color::Yellow),
        KanbanColumn::Done => ("ok  ", Color::Green),
        KanbanColumn::Blocked => match epic.status {
            EpicStatus::Failed => ("x   ", Color::Red),
            EpicStatus::Skipped => ("skip", Color::DarkGray),
            EpicStatus::Conflict => ("!   ", Color::Magenta),
            _ => ("    ", Color::Gray),
        },
    }
}
```

- [ ] **Step 4: Build and lint**

Run: `make verify 2>&1 | tail -12`
Expected: PASS (fmt-check, clippy `-D warnings`, all tests). Fix any clippy finding inline (for example, an unused import if `render_epics` referenced something now gone) and re-run until clean.

- [ ] **Step 5: Manual visual check against a throwaway repo**

The TUI needs an interactive terminal, so run this yourself in a real terminal (not headless):

```bash
mkdir -p /tmp/kanban-smoke && cd /tmp/kanban-smoke && git init -b main && printf '# smoke\n' > README.md && git add -A && git commit -m init
cd /Users/bimapangestu/Desktop/Works/personal/claude-agentic-loop
cargo run -- "Add a CONTRIBUTING.md and a LICENSE file" --workspace /tmp/kanban-smoke --verify "true"
```

Expected: after the plan is produced, the board shows five columns; epics start in Todo (hold or ready), move through In Progress and Review, and land in Done. If `claude` cannot run in your environment, record that the visual check was not performed rather than marking it passed.

- [ ] **Step 6: Commit**

```bash
git add src/ui.rs
git commit -m "feat: render epics as a five-column kanban board"
```

---

## Self-Review

**Spec coverage:**
- Five columns with the fixed mapping — Tasks 1 (`kanban_column`) and 3 (`render_board`). ✓
- Show every epic from plan time, including hold/ready — Task 2 (seed on `PlanReady`), Task 3 (Todo hold/ready via `is_on_hold`). ✓
- Review = Verifying, no pipeline change — mapping in Task 1; no engine/orchestrator/worktree edits. ✓
- Hold vs ready derived from dependency status — `is_on_hold` (Task 1), applied in `card_marker` (Task 3). ✓
- Log/header/footer intact — Task 3 keeps `render_header`/`render_log`/`render_footer`. ✓
- Empty-plan and unknown-dependency edge cases — `is_on_hold` returns true for unknown deps (Task 1 test); empty columns render without panic (Task 3 builds `items` from possibly-empty `cards`). ✓
- Narrow terminal / overflow — `truncate` on card text and `+K more` overflow line (Task 3). ✓

**Placeholder scan:** No TODO/TBD. Task 3 Step 5 is a manual visual check (the TUI needs a TTY), stated honestly; all code steps carry complete code.

**Type consistency:** `KanbanColumn` variants and `kanban_column`/`is_on_hold` signatures are identical across Tasks 1 and 3. `EpicMeta` fields (`id`, `title`, `depends_on`) match between `event.rs` (Task 2 Step 1), the `PlanReady` handler (Task 2 Step 3), and `main.rs` (Task 2 Step 6). `EpicView` gains `depends_on` in Task 2 Step 2 and every construction site is updated in the same task (Steps 3 and 4).

**Green-build check:** Task 1 adds `Pending` and the helpers, all exercised by tests (so `--all-targets` clippy stays clean) while the flat list still renders. Task 2 changes the `PlanReady` shape across `event.rs`/`app.rs`/`main.rs` together and adds `depends_on`, read by the new seeding test. Task 3 swaps the renderer. Each task ends green.
