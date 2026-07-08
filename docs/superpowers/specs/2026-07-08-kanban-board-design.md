# Kanban Board View — Design

Date: 2026-07-08
Status: Approved for planning
Project: `agentic-tui`

## Overview

Replace the flat epic list in the run TUI with a kanban board that groups epics
into columns by their state, so the operator can see at a glance which epics are
waiting, running, being verified, done, or blocked. This is a presentation-only
change. The pipeline, scheduler, worktree, and engine are not touched.

## Goals

- Show every epic from the moment the plan is ready, including epics that have
  not started yet (so waiting/hold epics are visible).
- Group epics into five columns by state.
- Keep the streaming log and header/footer intact below/around the board.

## Non-goals

- No pipeline change. "Review" is the existing verify step (running `VERIFY_CMD`),
  not a new review stage or an approval gate.
- No goal input in the TUI (goal stays a CLI argument).
- No horizontal scrolling of columns; on a narrow terminal, card text truncates
  and overflow rows collapse to a "+N more" line.

## Columns and state mapping

Five columns, left to right following the epic flow:

| Column | Epic state |
|---|---|
| **Todo** | `Pending` (not started). Card marked `hold` if any dependency is not yet `Merged`, `ready` if all dependencies are `Merged`. |
| **In Progress** | `Running` |
| **Review** | `Verifying` |
| **Done** | `Merged` |
| **Blocked** | `Failed`, `Skipped`, or `Conflict` (card shows which) |

## Why the plumbing changes

Today the `App` only learns about an epic when it starts (`EpicStarted`) or is
skipped (`EpicSkipped`), so pending/waiting epics are never shown. A kanban with
a Todo column that distinguishes hold from ready needs the full epic set, with
dependencies, from the moment the plan is validated.

## Components (changes)

- **`src/event.rs`** — `PlanReady` carries the epic set instead of just a count:
  `PlanReady { epics: Vec<EpicMeta> }` where `EpicMeta { id: String, title: String, depends_on: Vec<String> }`.
- **`src/app.rs`**
  - Reintroduce `EpicStatus::Pending`.
  - `EpicView` gains `depends_on: Vec<String>`.
  - On `PlanReady`, seed one `EpicView` per epic with status `Pending` and its
    `depends_on`; set the phase to `Implementing`.
  - Existing lifecycle events (`EpicStarted`, `EpicVerifying`, `EpicSucceeded`,
    `EpicMerged`, `EpicFailed`, `EpicSkipped`, `EpicConflict`) continue to update
    the matching card's status in place. `EpicStarted` no longer needs to create
    a card, since the card already exists from `PlanReady`; it updates status to
    `Running`.
  - Add two pure helpers (unit-testable, no rendering):
    - `pub fn kanban_column(status: EpicStatus) -> KanbanColumn` mapping status
      to one of `Todo | InProgress | Review | Done | Blocked`.
    - `pub fn is_on_hold(depends_on: &[String], status_by_id: &HashMap<String, EpicStatus>) -> bool`
      returning true when any dependency is not `Merged`.
  - Add `pub enum KanbanColumn { Todo, InProgress, Review, Done, Blocked }`.
- **`src/ui.rs`** — replace `render_epics` (flat list) with `render_board`:
  five side-by-side columns (equal-width `Layout`), each a bordered block with
  the column title and a list of epic cards. Each card shows `id  title`
  (truncated) plus a marker: `hold`/`ready` in Todo, `x`/`skip`/`!` for
  Failed/Skipped/Conflict in Blocked. Colors reuse the existing status palette.
  If a column has more cards than rows, show the first N and a final `+K more`
  line.
- **`src/main.rs`** — `run_pipeline` builds `Vec<EpicMeta>` from the validated
  `plan.epics` and sends it in `PlanReady`.

## Data flow

1. Plan validated in `run_pipeline` → `PlanReady { epics }` with every epic's id,
   title, and `depends_on`.
2. `App` seeds one Pending card per epic.
3. As the driver emits lifecycle events, each card's status updates in place.
4. Each render, `render_board` buckets cards into the five columns via
   `kanban_column`, and within Todo labels each card hold or ready via
   `is_on_hold` against the current status of its dependencies.

## Layout

Vertical split: header (4 lines, unchanged), board (fills the middle), log
(fixed height, roughly 8 lines, unchanged content), footer (1 line, unchanged).
The board is one horizontal split of five equal columns.

## Error handling and edge cases

- Zero epics (empty plan): the board renders five empty columns; the run still
  completes. No panic on empty columns.
- An epic whose `depends_on` references an id not present in the status map is
  treated as on hold (dependency not `Merged`). Plan validation already rejects
  unknown dependencies, so this is a defensive default.
- Narrow terminal: card text truncates to the column width; the board never
  causes a horizontal overflow.

## Testing strategy

- `kanban_column`: one assertion per status mapping to its column.
- `is_on_hold`: true when a dependency is Pending/Running/etc, false when all
  dependencies are Merged, and true for an empty status map with a named
  dependency.
- Rendering (`render_board`) is verified by build plus manual run, consistent
  with the rest of the TUI.

## Open questions

None. Deferred (not part of this change): typing the goal in the TUI; a real
per-epic review stage; git submodule init inside epic worktrees.
