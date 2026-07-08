# Agentic Orchestrator + Workspace — Design

Date: 2026-07-08
Status: Approved for planning
Project: `agentic-tui`

## Overview

Extend the existing single-stage PRD generator into a full agentic orchestration
loop. One orchestrator receives a goal, breaks it down into epics and tasks, and
drives many `claude -p` subagent sessions that actually write code until the goal
is done. The user selects a workspace (a single project root) at start; every
session runs there and can read across the whole project tree for context.

This replaces the current flow:

```
goal -> 1x claude -p (read repo, write PRD) -> docs/prd/<slug>.md -> manual handoff
```

with a multi-stage orchestrated flow (below).

## Goals

- Turn a single goal into working code through parallel subagent sessions.
- Keep one epic to one `claude -p` session as the unit of work.
- Isolate parallel epics from each other so they do not clobber shared files.
- Verify each epic objectively before it is merged.
- Let the user pick a workspace from a registered list via a TUI picker.
- Keep the run autonomous, with the ability to pause or abort from the TUI.

## Non-goals

- Auto-resolving merge conflicts between epics (v1 flags them for manual merge).
- Multi-project or cross-location knowledge gathering (workspace is one root).
- A plan approval gate (the run is autonomous once started).
- Splitting each task inside an epic into its own OS process (tasks stay inside
  the epic session).

## Top-level flow

```
start
 |
 |- Load ~/.config/agentic-tui/workspaces.toml
 |- TUI WORKSPACE PICKER  -> select workspace (arrows + Enter)
 |     (skipped when --workspace <name|path> is passed)
 |  -> workspace root becomes cwd for ALL sessions and the base for worktrees
 |
 |- Stage PLAN     (1x claude -p at workspace root, read-only + Write)
 |     -> writes plan.json: epics[] { id, title, depends_on[], acceptance[], tasks[] }
 |
 |- Stage IMPLEMENT (Rust scheduler, respects deps, up to N in parallel)
 |     each epic -> git worktree (from workspace root) -> 1x claude -p
 |       -> Rust runs VERIFY_CMD inside the worktree
 |          pass -> ready-to-merge | fail -> 1 retry -> FAILED (worktree discarded)
 |
 |- Stage INTEGRATE (Rust)
       merge successful epics into an integration branch (in dependency order)
       conflict -> epic marked NEEDS-MANUAL-MERGE (no auto-resolve in v1)
       final report: succeeded / failed / skipped / conflict
```

## Locked decisions

| Aspect | Decision |
|---|---|
| Final goal | Full agentic loop: subagents write code until the goal is done |
| Orchestration shape | Hybrid 3-stage: Plan -> Implement (per-epic) -> Integrate |
| Session unit | 1 epic = 1 `claude -p` session |
| Tasks within an epic | Handled inside that epic session (sequential; Claude may use its own internal Task tool at its discretion) |
| Parallelism | Independent epics run in parallel, capped at `MAX_PARALLEL_EPICS` (default 3), isolated with git worktrees |
| Epic failure | Failed epic is not merged; epics that depend on it are skipped; independent epics continue; all outcomes reported at the end |
| Control | Autonomous, no approval gate; user can pause/abort from the TUI (kills child processes and cleans worktrees) |
| Verification | Rust runs `VERIFY_CMD` inside the epic worktree; pass = mergeable (does not trust the session's self-report) |
| Retry | 1 retry per epic before it is marked failed |
| Workspace | A single project root; sessions run there and read across the whole tree |
| Workspace source | `~/.config/agentic-tui/workspaces.toml` |
| Workspace selection | TUI picker at start (plus recents); fast-path `--workspace <name\|path>` skips it |
| Goal input | CLI positional argument; if omitted, the TUI prompts for it |

## Components (modules)

Existing `engine.rs` / `event.rs` / `app.rs` / `ui.rs` structure is kept and
extended. New modules are added with single, clear responsibilities.

- **`workspace.rs`** (new) — load and validate `workspaces.toml`, the `Workspace`
  struct, and picker state. Depends on: `toml`, `dirs`, `serde`.
- **`plan.rs`** (new) — `Plan`, `Epic`, `Task` structs and parsing of `plan.json`.
  Depends on: `serde`, `serde_json`.
- **`orchestrator.rs`** (new) — epic scheduler: topological ordering by
  `depends_on`, a bounded parallel pool, retry, and per-epic state transitions.
  Depends on: `plan.rs`, `worktree.rs`, `engine.rs`, `tokio`.
- **`worktree.rs`** (new) — create and remove a git worktree per epic, and merge
  a successful epic branch into the integration branch. Depends on: `git` CLI via
  `tokio::process`.
- **`engine.rs`** (extended) — a generic `run_stage` that spawns `claude -p` with
  a given cwd and tool allowlist; used by both the Plan stage and each epic.
- **`event.rs` / `app.rs` / `ui.rs`** (extended) — a picker phase, per-epic status
  (pending / running / verifying / merged / failed / skipped / conflict), and a
  multi-epic progress view.

## Data flow

1. `workspace.rs` loads the workspace list; the picker (or `--workspace`) resolves
   one `Workspace { name, path }`. `path` becomes the shared cwd.
2. `engine::run_stage` runs the Plan session at the workspace root. The session
   writes `plan.json` to a known path with the `Write` tool (a file, not parsed
   from the stream, so parsing is reliable). Rust reads and validates it via
   `plan.rs`.
3. `orchestrator.rs` builds the epic dependency graph and schedules epics:
   - Ready epic (all deps merged) -> `worktree.rs` creates a worktree ->
     `engine::run_stage` runs the epic session there.
   - On session finish, Rust runs `VERIFY_CMD` in the worktree.
   - Pass -> queued for integration. Fail -> 1 retry -> `FAILED`; dependents are
     marked `SKIPPED`.
4. `INTEGRATE` merges ready epics into the integration branch in dependency order.
   A merge conflict marks that epic `NEEDS-MANUAL-MERGE`.
5. Throughout, the orchestrator emits `AppEvent`s to the UI; the UI renders the
   picker, then per-epic progress, then the final report.

## Configuration (`config.rs`)

- `VERIFY_CMD` — repo verification command (for example `"make verify"`),
  overridable with `--verify`.
- `MAX_PARALLEL_EPICS` — default `3`.
- `GLOBAL_BUDGET_USD` — global circuit breaker across all sessions.
- `EPIC_BUDGET_USD` — per-epic budget.
- Tool allowlists:
  - PLAN: `Read,Glob,Grep,Write,WebSearch,WebFetch,Skill`
  - EPIC: `Read,Glob,Grep,Edit,Write,Bash,WebSearch,WebFetch,Skill`

### `workspaces.toml` format

```toml
[[workspace]]
name = "greentic"
path = "~/Works/personal/greentic"

[[workspace]]
name = "portfolio"
path = "~/Works/personal/portfolio-tracker"
```

### `plan.json` schema

```json
{
  "epics": [
    {
      "id": "epic-1",
      "title": "string",
      "depends_on": ["epic-id", "..."],
      "acceptance": ["verifiable item", "..."],
      "tasks": [
        { "id": "task-1", "title": "string", "detail": "string" }
      ]
    }
  ]
}
```

## New dependencies (`Cargo.toml`)

- `serde` (derive) — struct parsing for `plan.json` and `workspaces.toml`.
- `toml` — parse `workspaces.toml`.
- `dirs` — resolve `~/.config` across platforms.
- (`serde_json` is already present.)

## Error handling

- Missing or invalid `workspaces.toml` -> show a clear message in the picker and
  allow `--workspace <path>` as a direct fallback.
- Selected workspace path missing or not a git repo -> refuse to start with a
  clear error (worktrees require git).
- Plan session fails or `plan.json` is missing/invalid -> abort before any epic
  runs; nothing has been changed yet.
- Epic session error or `VERIFY_CMD` failure -> 1 retry, then `FAILED`; the
  worktree is discarded so the workspace is never left dirty by a failed epic.
- Merge conflict at integration -> `NEEDS-MANUAL-MERGE`; the epic branch is kept
  so the user can resolve it by hand.
- Pause/abort -> kill running child processes and remove epic worktrees before
  exit; the integration branch keeps whatever merged cleanly.

## Testing strategy

- `workspace.rs`: parse valid and malformed `workspaces.toml`; `~` expansion;
  missing-path handling.
- `plan.rs`: parse valid `plan.json`; reject missing fields; detect dependency
  cycles.
- `orchestrator.rs`: topological scheduling with a fake stage runner (no real
  `claude`); verify parallel cap, dependent-skip on failure, and independent
  continuation. This is the correctness core and gets the most coverage.
- `worktree.rs`: create/merge/cleanup against a temporary git repo; assert a
  conflict is detected rather than silently resolved.
- `engine.rs`: parse representative `stream-json` lines into events.

## Open questions

None blocking. Deferred to a later iteration:

- Auto-resolve or auto-retry of merge conflicts.
- Cross-project / external knowledge references for a workspace.
- Recording and reusing recents beyond the current `workspaces.toml`.

## Implementation task breakdown (ordered)

1. Add `serde`, `toml`, `dirs` to `Cargo.toml`.
2. `workspace.rs`: config loading, `~` expansion, validation, unit tests.
3. TUI workspace picker phase + `--workspace` fast-path (UI + app state + events).
4. Optional goal input in the TUI when the goal argument is omitted.
5. `plan.rs`: structs, `plan.json` parsing, cycle detection, unit tests.
6. Refactor `engine.rs` `run_stage` to take cwd + tool allowlist generically.
7. Plan stage: prompt that emits `plan.json`; Rust reads and validates it.
8. `worktree.rs`: per-epic worktree create/remove + integration-branch merge.
9. `orchestrator.rs`: dependency graph, parallel pool, retry, per-epic state.
10. Verification step: run `VERIFY_CMD` in each worktree; gate merges on pass.
11. Integrate stage: ordered merge + conflict detection + final report.
12. Extend `event.rs` / `app.rs` / `ui.rs` for multi-epic progress + report view.
13. Config knobs: `VERIFY_CMD`, `MAX_PARALLEL_EPICS`, budgets, tool allowlists.
14. Pause/abort handling: kill children + clean worktrees on exit.
15. Update `README.md` and the `Makefile` for the new flow.
```
