# Web UI Phase 1 Design (TUI Parity)

## Context

`agentic-tui` is a Rust orchestrator that plans a goal into epics and drives
worktree-isolated `claude -p` sessions, today rendered in a ratatui/crossterm
TUI. This is Phase 1 of replacing that TUI with an all-Rust web UI. Phase 1
delivers **full parity** with the current TUI (every existing flow, in the
browser) and removes the TUI entirely. Later phases add net-new features (run
history, per-epic diffs, cost charts).

## Goal

Ship a local web app that reproduces every current interactive flow:
onboarding scan wizard, workspace picker, goal input (multi-line), base/into/
refine run options, the goal-refine question/answer/confirm flow, the live run
view (kanban board, log, budget), abort, and the final report. `agentic-tui`
launches a local server bound to loopback and opens the browser to it. The
ratatui/crossterm TUI and `src/ui.rs` are deleted.

## Non-goals (deferred to later phases)

- Run history and persistence (Phase: history + SQLite).
- Per-epic diff viewer.
- Cost charts / analytics.
- Any non-loopback binding, authentication, or multi-user support.
- Multiple concurrent runs (one run at a time in Phase 1).

## Architecture

All Rust. The repo becomes a Cargo **workspace** with three crates so the
frontend (WASM) and backend (native) share types:

```
crates/
  shared/   # pure state + DTOs used by BOTH server and web (compiles to wasm)
  server/   # axum + engine + orchestrator + worktree + refine/workspace/config; the CLI binary
  web/      # Leptos CSR app, built by trunk to static wasm+js+html
```

- **shared**: `App` and its sub-types (`Phase`, `EpicView`, `EpicStatus`,
  `KanbanColumn`, `EpicMeta`), the `AppEvent` payloads that cross the wire, the
  plan DTOs, and request/response DTOs for the API. All `serde`-serializable, no
  tokio/std::process/git dependencies (so it compiles to `wasm32`).
- **server**: everything that touches the OS or network. It keeps `engine`,
  `orchestrator`, `worktree`, `workspace`, `config`, and the refine passes
  (`refine::run_refine_pass` plus parsing), reused unchanged in logic. It adds an
  axum app that serves the embedded web build and the API, owns the run, and
  streams state to the browser. `main` parses args, starts the server, and opens
  the browser.
- **web**: a Leptos client-side-rendered SPA. Views mirror the TUI screens.
  Live run state arrives over a WebSocket and drives a reactive `App` render.

### State and streaming

Reuse the existing `App` + `AppEvent`. The server owns an `App` per run and
applies incoming `AppEvent`s to it exactly as the TUI loop did. On every change
it serializes the whole `App` to JSON and pushes the snapshot over the run's
WebSocket. The browser renders the latest snapshot (no delta logic on the
client). `App` and its sub-types gain `#[derive(Serialize, Deserialize)]` and
move to `shared`.

### Run lifecycle

The server reuses `run_pipeline` / `orchestrator` / `engine` unchanged. Starting
a run spawns the pipeline task (as `main` did), forwarding `AppEvent`s to a
`tokio::sync::broadcast` channel that WebSocket handlers subscribe to. Abort
reuses the existing task-abort + `worktree::cleanup_all` logic. Only one run may
be active at a time in Phase 1; starting a new run while one is active returns a
409.

### HTTP API (loopback only)

- `GET  /` and static assets — the embedded SPA (via `rust-embed`).
- `GET  /api/workspaces` — list configured workspaces (`load_workspaces`).
- `POST /api/workspaces/scan` `{ root }` -> `{ repos: [Workspace] }`
  (`scan_for_repos`).
- `POST /api/workspaces` `{ workspaces: [Workspace] }` — persist
  (`save_workspaces`).
- `POST /api/refine/questions` `{ repo, goal }` ->
  `{ refined_goal, questions }` (refine pass 1).
- `POST /api/refine/finalize` `{ repo, goal, answers }` -> `{ refined_goal }`
  (refine pass 2).
- `POST /api/runs` `{ workspace, goal, base?, into?, verify?, refine_cost }` ->
  `{ run_id }`. Validates the base ref and the integration target (reusing
  `verify_ref` and the checked-out-branch guard) and returns 400 on failure
  before starting.
- `POST /api/runs/:id/abort` — abort the active run.
- `GET  /api/runs/:id/events` — WebSocket; emits `App` JSON snapshots until the
  run ends, then the final snapshot and closes.

All endpoints bind `127.0.0.1`. No auth (loopback, single user). CORS is not
needed (same origin).

### Frontend views (Leptos CSR)

- **Workspaces** (`/`): the configured workspace list; select one to start a
  run. An "Add workspace" panel takes a folder path, calls `/scan`, shows the
  found repos as a checklist, and saves the checked ones (the onboarding wizard,
  as a web form). If the config is empty, this view opens on the add panel.
- **New run** (`/run/new?workspace=...`): a form with a multi-line `<textarea>`
  goal (Enter inserts newlines natively), optional `--base`/`--into` inputs, a
  verify-command input, and a "refine before planning" toggle. Submitting with
  refine on runs the refine flow inline: it posts to `/refine/questions`, renders
  each question with an answer field, posts `/refine/finalize`, and shows the
  final goal in an editable field to confirm; then it posts `/api/runs`.
- **Run** (`/run/:id`): the live view. A header shows goal, workspace, and a
  budget bar (spent / total). A five-column kanban board (Todo, In Progress,
  Review, Done, Blocked) buckets epic cards by status, with the on-hold hint in
  Todo, mirroring `kanban_column`/`is_on_hold`. A scrolling log shows stage
  output. An Abort button posts `/abort`. When the run ends, the final report
  (per-epic status, total cost, integration-branch reminder) renders in place.

The kanban bucketing and on-hold logic move to `shared` so both the (removed)
render path and the web render the same way; the pure helpers
`kanban_column`/`is_on_hold` are reused verbatim.

## Removed

- `src/ui.rs` (all ratatui rendering).
- `crossterm` and `ratatui` dependencies.
- The TUI loops in `main` (`run_picker`, `run_goal_input`, `run_onboarding`, and
  the interactive parts of `refine::run` and the main event loop). The refine
  **logic** (`run_refine_pass`, `parse_refine`, prompts) is kept and called by
  the API; only its terminal drawing is dropped.

## Build and toolchain

- The web crate builds with `trunk` to `crates/web/dist`, which the server
  embeds at compile time. `make build` runs `trunk build --release` then
  `cargo build`. A new `wasm32-unknown-unknown` target and `trunk` are required.
- This **drops the rustc 1.75 / pinned-`Cargo.lock` constraint**: Leptos, axum,
  and the wasm toolchain need a modern toolchain. The README prerequisite and
  CI are updated accordingly (CI installs `trunk` and the wasm target and builds
  both crates).
- New dependencies: `axum`, `tower`/`tower-http` (static + ws), `rust-embed`,
  `leptos`, `leptos_router`, `serde` (already present), `open` (launch browser),
  `gloo-net`/`web-sys` (web WS client). Version pinning is chosen fresh; the old
  1.75 pin is retired.

## Config knobs

- `SERVER_PORT` (default 0 = ephemeral; the chosen port is printed and used to
  open the browser).
- `OPEN_BROWSER` (default true; `--no-open` to skip, useful for remote/dev).

## Error handling

- Base-ref / integration-target validation happens in `POST /api/runs` and
  returns 400 with the same clear messages the CLI produced, before any session
  starts.
- A refine pass failure returns the original goal to the client (same fallback
  as the TUI), and the API reports the incurred cost.
- WebSocket disconnects do not kill the run; reconnecting to
  `/api/runs/:id/events` resumes streaming from the current snapshot.
- If the browser cannot be opened, the server prints the URL to stdout and keeps
  running.

## Testing

- `shared`: `App` apply/serialize round-trips; `kanban_column`/`is_on_hold`
  (moved, tests move with them); plan and DTO (de)serialization.
- `server`: API handler tests with a stub run (workspace list, scan, save,
  refine endpoints against a temp repo with a fake `claude` on PATH, run
  validation rejects a bad base / checked-out integration target). Reused engine/
  worktree/workspace tests stay green.
- `web`: component logic that is pure (form state, kanban bucketing via shared)
  is unit-tested; full browser rendering is verified by a manual smoke test
  (documented steps), since headless WASM UI testing is out of scope for Phase 1.
- End-to-end: a scripted smoke test drives the API with a fake `claude` on PATH
  (no billing) through scan -> save -> refine -> run -> abort, asserting the
  event stream and the final report, mirroring the pty smoke tests used for the
  TUI features.

## Migration / structure notes

- Moving to a workspace is a mechanical but broad change: `Cargo.toml` becomes a
  workspace manifest; `src/*` splits into `crates/shared` (state/DTOs) and
  `crates/server` (logic + CLI); a new `crates/web` is added. Module paths and
  imports update accordingly. This is done in one early task so the tree compiles
  before any web code lands.
- The default `make run` behavior is preserved conceptually: it launches the app
  (now the server + browser) rather than the TUI.

## Files (high level; the plan enumerates exact tasks)

| Area | Change |
|---|---|
| workspace manifest | root `Cargo.toml` becomes `[workspace]`; three member crates |
| `crates/shared` | `App`, `AppEvent` payloads, `Phase`/`EpicView`/`EpicStatus`/`KanbanColumn`/`EpicMeta`, plan + API DTOs, `kanban_column`/`is_on_hold`; all `serde` |
| `crates/server` | `engine`, `orchestrator`, `worktree`, `workspace`, `config`, refine passes; axum app (static + API + WS); run manager; `main` (args -> server -> browser) |
| `crates/web` | Leptos CSR app: Workspaces, New run (+ refine flow), Run (kanban/log/budget/abort/report); WS client |
| removed | `src/ui.rs`, `crossterm`, `ratatui`, TUI loops |
| `Makefile`, CI, README | trunk + wasm build; drop 1.75 pin; document `--no-open`/port |
