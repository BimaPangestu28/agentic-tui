# Web UI Phase 1 Implementation Plan (TUI Parity)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the ratatui/crossterm TUI with an all-Rust web UI (Leptos CSR SPA + axum backend + WebSocket), at full parity with today's flows: onboarding scan wizard, workspace picker, multi-line goal input, base/into/refine options, the goal-refine question/answer/confirm flow, and the live run view (kanban/log/budget/abort/report).

**Architecture:** A Cargo workspace with three crates: `shared` (serde state + DTOs, compiles to wasm), `server` (all current logic + axum app + run manager + CLI `main`), and `web` (Leptos CSR app built by trunk and embedded in the server). The server owns an `App` per run and streams JSON snapshots over a WebSocket; the browser renders the latest snapshot. Reuses `engine`/`orchestrator`/`worktree`/`workspace`/`config`/refine logic unchanged.

**Tech Stack:** Rust 2021 (modern toolchain, wasm32 target, trunk), axum + tower-http (server + static + ws), rust-embed (embed the SPA), leptos + leptos_router (frontend), serde/serde_json, tokio, anyhow.

## Global Constraints

- **Drop the rustc 1.75 / pinned-`Cargo.lock` constraint.** Leptos/axum/wasm need a modern toolchain. Do not artificially pin; let Cargo resolve. Do not run `cargo update` gratuitously, but a fresh lock for the new deps is expected.
- No `unwrap()`/`expect()`/`panic!` in production code (tests may use them).
- Comment/prose style: no em dashes, no contractions in English prose.
- Descriptive names; verbs for functions, nouns for types.
- **Green-build invariant:** every task leaves the whole workspace compiling and its tests passing. During the build the web UI is reachable behind a `--web` flag while the TUI stays the default, so the tree is always runnable; the final task flips the default and deletes the TUI. `make verify` (fmt-check, clippy `--all-targets -- -D warnings`, test) is green after every task. Where a task adds a wasm-only crate, `cargo test -p shared -p server` covers the native side; the web crate is checked with `cargo check -p web --target wasm32-unknown-unknown` (or `trunk build`).
- Conventional commits. Commit after every task. Work on branch `feat/web-ui-phase1`.
- Prerequisite (install once before Task 3): `cargo install --locked trunk` and `rustup target add wasm32-unknown-unknown` (the target is already present in this environment).

## Workspace layout (end state)

```
Cargo.toml                # [workspace] members = shared, server, web
crates/
  shared/  Cargo.toml  src/lib.rs        # App, AppEvent wire payloads, state enums, DTOs, kanban helpers
  server/  Cargo.toml  src/main.rs ...   # engine, orchestrator, worktree, workspace, config, refine, http, run manager
  web/     Cargo.toml  src/main.rs ...   index.html  Trunk.toml   # Leptos CSR app
```

---

### Task 1: Convert to a Cargo workspace with a `server` crate

Move all existing source under `crates/server` unchanged, so the current TUI binary still builds and passes, now as a workspace member. Pure restructuring, no logic change.

**Files:**
- Create: root `Cargo.toml` (workspace manifest), `crates/server/Cargo.toml`
- Move: `src/*.rs` -> `crates/server/src/*.rs` (git mv)
- Move: `Cargo.lock` stays at root

- [ ] **Step 1: Move the sources**

```bash
mkdir -p crates/server/src
git mv src/*.rs crates/server/src/
rmdir src
```

- [ ] **Step 2: Write `crates/server/Cargo.toml`**

Copy the current `[package]` and `[dependencies]` from the old root `Cargo.toml` into `crates/server/Cargo.toml`, keeping the same dependencies (tokio, ratatui, crossterm, serde, serde_json, toml, dirs, anyhow) and the `[package]` metadata (rename `name` to `agentic-tui` stays; the binary name stays `agentic-tui`). Add `[[bin]] name = "agentic-tui" path = "src/main.rs"`.

- [ ] **Step 3: Replace the root `Cargo.toml` with a workspace manifest**

```toml
[workspace]
members = ["crates/server"]
resolver = "2"
```

(Keep the `crates/shared` and `crates/web` members out until they exist; add them in their tasks.)

- [ ] **Step 4: Build and test**

Run: `cargo build && cargo test 2>&1 | tail -15`
Expected: builds, all existing tests pass (the crate is unchanged, only relocated).

- [ ] **Step 5: Update `make` paths if needed, then verify**

`make verify` should pass unchanged (cargo resolves the workspace). If `make run` referenced `src/`, it does not; it uses `cargo run`, which still works.

Run: `make verify`
Expected: green.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor: move the crate into a cargo workspace under crates/server

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Extract shared state into `crates/shared`

Move the pure, serde-friendly state and DTOs into a `shared` crate that both the server and the future web crate depend on. Split `AppEvent` so the terminal-only variants (`Input`, `Tick`) stay in the server while the wire events move to `shared`.

**Files:**
- Create: `crates/shared/Cargo.toml`, `crates/shared/src/lib.rs`
- Modify: `crates/server/src/app.rs`, `event.rs`, `plan.rs` (re-export/adapt), `Cargo.toml` (dep on shared), root `Cargo.toml` (add member)

**Interfaces:**
- `shared` exports: `App`, `EpicView`, `EpicStatus`, `KanbanColumn`, `Phase`, `EpicMeta`, `kanban_column`, `is_on_hold`, `StageEvent` (the wire subset), and DTOs added in later tasks.
- All derive `Serialize, Deserialize, Clone` and, where they already do, `Debug, PartialEq`.

- [ ] **Step 1: Create the shared crate**

`crates/shared/Cargo.toml`:

```toml
[package]
name = "shared"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = { version = "1", features = ["derive"] }
```

Add `"crates/shared"` to the root workspace `members`.

- [ ] **Step 2: Move the state types into `crates/shared/src/lib.rs`**

Move `App`, `EpicView`, `EpicStatus`, `KanbanColumn`, `Phase`, `EpicMeta`, and the pure helpers `kanban_column`/`is_on_hold` (currently in `app.rs`) into `shared/src/lib.rs`. Add `#[derive(Serialize, Deserialize)]` to every one of them (alongside the existing derives). Move the corresponding unit tests with them.

Define a serde-friendly wire-event enum in `shared` for the events the server pushes to the browser:

```rust
use serde::{Deserialize, Serialize};

/// A pipeline event the server applies to `App` and forwards to the browser.
/// This is the terminal-independent subset of the old `AppEvent`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StageEvent {
    StageLog { tag: String, line: String },
    StageAssistant { tag: String, text: String },
    StageTool { tag: String, name: String },
    Cost(f64),
    PlanReady { epics: Vec<EpicMeta> },
    Done,
    Fatal(String),
}
```

Give `App` a method `pub fn apply_stage(&mut self, event: StageEvent)` that performs exactly what the old `App::apply` did for these variants (move that logic here). Keep `App::tick` in `shared` if it is pure; it is (spinner state), so move it too.

- [ ] **Step 3: Point the server at shared**

In `crates/server/Cargo.toml` add `shared = { path = "../shared" }`. In the server, delete the moved types from `app.rs` and re-export from shared: `pub use shared::{App, EpicView, EpicStatus, KanbanColumn, Phase, EpicMeta, kanban_column, is_on_hold};` (so existing `crate::app::App` paths keep working). Keep the server's `AppEvent` as the terminal enum, now defined in terms of `StageEvent`:

```rust
pub enum AppEvent {
    Stage(shared::StageEvent),
    Input(crossterm::event::KeyEvent),
    Tick,
}
```

Update the pipeline/orchestrator/engine sends: where they previously sent `AppEvent::StageLog { .. }` etc., send `AppEvent::Stage(StageEvent::StageLog { .. })`. Update the TUI loop: `AppEvent::Stage(e) => app.apply_stage(e)`, `AppEvent::Tick => app.tick()`, input as before. Update `ui.rs` imports to use the shared types.

- [ ] **Step 4: Build and test**

Run: `cargo test 2>&1 | tail -20`
Expected: the TUI still works and all tests pass (moved tests included).

- [ ] **Step 5: Verify wasm-compatibility of shared**

Run: `cargo check -p shared --target wasm32-unknown-unknown`
Expected: compiles (shared has no OS deps).

- [ ] **Step 6: `make verify` and commit**

```bash
make verify
git add -A
git commit -m "refactor: extract serde state into the shared crate

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Add the Leptos web crate skeleton and embed it behind `--web`

Create a minimal Leptos CSR app ("Agentic Orchestrator" shell that connects to nothing yet), build it with trunk, and serve it from the server behind a `--web` flag so the tree stays runnable while the TUI is still the default.

**Files:**
- Create: `crates/web/Cargo.toml`, `crates/web/src/main.rs`, `crates/web/index.html`, `crates/web/Trunk.toml`
- Modify: `crates/server/Cargo.toml` (axum, tower-http, rust-embed, open), `crates/server/src/main.rs` (add `--web`), new `crates/server/src/http.rs`

- [ ] **Step 1: Web crate**

`crates/web/Cargo.toml`:

```toml
[package]
name = "web"
version = "0.1.0"
edition = "2021"

[dependencies]
leptos = { version = "0.7", features = ["csr"] }
leptos_router = "0.7"
shared = { path = "../shared" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
gloo-net = "0.6"
wasm-bindgen = "0.2"
web-sys = { version = "0.3", features = ["WebSocket", "MessageEvent"] }
```

`crates/web/index.html`:

```html
<!DOCTYPE html>
<html>
  <head><meta charset="utf-8"/><title>Agentic Orchestrator</title></head>
  <body></body>
</html>
```

`crates/web/Trunk.toml`:

```toml
[build]
target = "index.html"
dist = "dist"
```

`crates/web/src/main.rs` (a minimal shell):

```rust
use leptos::prelude::*;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(|| view! { <h1>"Agentic Orchestrator"</h1> });
}
```

Add `console_error_panic_hook = "0.1"` to the web deps. Add `"crates/web"` to the workspace members.

- [ ] **Step 2: Build the web crate**

```bash
cd crates/web && trunk build && cd ../..
ls crates/web/dist   # index.html + hashed wasm/js
```

Expected: `dist/` produced.

- [ ] **Step 3: Serve it from the server behind `--web`**

Add to `crates/server/Cargo.toml`:

```toml
axum = { version = "0.8", features = ["ws"] }
tower-http = { version = "0.6", features = ["fs"] }
rust-embed = "8"
open = "5"
```

Create `crates/server/src/http.rs` with an embedded-assets handler:

```rust
use axum::{
    body::Body,
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    Router,
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../web/dist"]
struct Assets;

async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match Assets::get(path).or_else(|| Assets::get("index.html")) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref())], content.data).into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

pub fn router() -> Router {
    Router::new().fallback(static_handler)
}

/// Bind loopback, print the URL, optionally open the browser, and serve.
pub async fn serve(open_browser: bool) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let url = format!("http://{addr}");
    println!("agentic-tui web UI at {url}");
    if open_browser {
        let _ = open::that(&url);
    }
    axum::serve(listener, router()).await?;
    Ok(())
}
```

Add `mime_guess = "2"` to the server deps. In `crates/server/src/main.rs`, add `mod http;`, parse a `--web` flag (and `--no-open`), and when `--web` is set, call `http::serve(!no_open).await` and return instead of running the TUI.

- [ ] **Step 4: Build, run the smoke check**

```bash
cargo build
cargo run -p agentic-tui -- --web --no-open &   # prints the URL
sleep 1; curl -s http://127.0.0.1:PORT/ | grep -q "Agentic Orchestrator" && echo OK
kill %1
```

(Use the printed port. The page body is filled by wasm at runtime, but `index.html` and the wasm asset must be served; assert the HTML loads and a `.wasm` asset is reachable.)

- [ ] **Step 5: `make verify` (native side) and commit**

`make verify` covers the native crates. Also run `cargo check -p web --target wasm32-unknown-unknown`.

```bash
git add -A
git commit -m "feat: embed a Leptos web shell served behind --web

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Workspace API (list, scan, save) with DTOs

Add the workspace endpoints and their DTOs in `shared`, backed by the existing `workspace` functions.

**Files:**
- Modify: `crates/shared/src/lib.rs` (DTOs), `crates/server/src/http.rs` (routes + handlers)

**Interfaces (shared DTOs):**

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceDto {
    pub name: String,
    pub path: String,
    pub base: Option<String>,
    pub integration: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanRequest { pub root: String }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanResponse { pub repos: Vec<WorkspaceDto> }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveRequest { pub workspaces: Vec<WorkspaceDto> }
```

Add `From<&workspace::Workspace> for WorkspaceDto` and a `to_workspace()` in the server (path via `to_string_lossy`).

- [ ] **Step 1: Handlers**

In `http.rs` add (using `axum::Json`):
- `GET /api/workspaces` -> `Json<Vec<WorkspaceDto>>` from `workspace::load_workspaces(default_config_path())` (empty on error).
- `POST /api/workspaces/scan` `Json<ScanRequest>` -> `Json<ScanResponse>` via `workspace::scan_for_repos(expand_tilde(&root), DEFAULT_SCAN_DEPTH)`.
- `POST /api/workspaces` `Json<SaveRequest>` -> `save_workspaces(default_config_path(), &workspaces)`; 200 or 500 with the error text.

Mount them on the router before the static fallback.

- [ ] **Step 2: Tests**

In `crates/server`, add handler tests that call the functions directly (not over HTTP) with a temp `HOME` and a temp repo, asserting: scan finds a `.git` repo, save then load round-trips, list reflects saved entries. (These are the same assertions as the existing workspace tests, at the DTO boundary.)

- [ ] **Step 3: `make verify` and commit**

```bash
git add -A
git commit -m "feat: workspace list, scan, and save HTTP endpoints

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Run manager, `POST /api/runs`, WebSocket snapshots, abort

The core: start a run, stream `App` snapshots over a WebSocket, and abort. One run at a time.

**Files:**
- Create: `crates/server/src/run.rs` (run manager)
- Modify: `crates/server/src/http.rs` (routes), `crates/shared/src/lib.rs` (run DTOs)

**Interfaces (shared):**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartRunRequest {
    pub workspace: WorkspaceDto,
    pub goal: String,
    pub base: Option<String>,
    pub into: Option<String>,
    pub verify: Option<String>,
    pub refine_cost: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartRunResponse { pub run_id: String }
```

**Run manager design (`run.rs`):**
- A process-global `Mutex<Option<RunHandle>>` (one run at a time). `RunHandle { id, app: Arc<Mutex<App>>, tx: broadcast::Sender<App>, task: JoinHandle<()>, repo, integration }`.
- `start(req) -> Result<String>`: resolve `base_ref`/`integration` (reuse `resolve_setting` moved to a shared-free helper in server), validate with `verify_ref` and the checked-out-branch guard (reuse), build `App`, spawn a task that runs `run_pipeline` with an `mpsc` sender; a forwarding loop applies each `StageEvent` to the `App` and broadcasts a clone of the `App`. Return the run id (a monotonic counter rendered as a string; do not use randomness).
- `abort(id)`: abort the task and run `worktree::cleanup_all` if the run had not completed, mirroring `main`'s abort path.
- `subscribe(id) -> Option<(App, broadcast::Receiver<App>)>`: current snapshot plus a live receiver.

- [ ] **Step 1: Implement `run.rs`** with the design above. Reuse `run_pipeline` unchanged; it already takes `base_ref`/`integration`/`refine_cost`. The forwarding loop replaces `main`'s terminal event loop: `while let Some(ev) = rx.recv().await { if let AppEvent::Stage(e) = ev { let mut app = app.lock(); app.apply_stage(e); let _ = tx.send(app.clone()); } }`.

- [ ] **Step 2: Routes in `http.rs`**
- `POST /api/runs` `Json<StartRunRequest>` -> `Json<StartRunResponse>`; 400 with a message if validation fails, 409 if a run is already active.
- `POST /api/runs/:id/abort` -> 200.
- `GET /api/runs/:id/events` -> `WebSocketUpgrade`; on upgrade, send the current `App` snapshot as JSON text, then forward every broadcast `App` as JSON until the channel closes.

- [ ] **Step 3: End-to-end test with a fake `claude`**

Add an integration test (in `crates/server/tests/`) that puts a fake `claude` on `PATH` (writes an empty `.agentic-plan.json`, emits a `result` line with a cost), starts a run against a temp git repo via the run manager, subscribes, and asserts the snapshot stream reaches `Phase::Done` with the expected cost. Also assert `POST /api/runs` rejects an invalid base and a checked-out integration target.

- [ ] **Step 4: `make verify` and commit**

```bash
git add -A
git commit -m "feat: run manager, start-run endpoint, and websocket app snapshots

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Refine API (questions, finalize)

Expose the two refine passes so the frontend can run the clarification flow. Reuse `refine::run_refine_pass` and `parse_refine`; drop only the terminal parts.

**Files:**
- Modify: `crates/server/src/refine.rs` (make `run_refine_pass` callable without the TUI; it already is, plus a public wrapper), `crates/server/src/http.rs`, `crates/shared/src/lib.rs` (DTOs)

**Interfaces (shared):**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefineQuestionsRequest { pub repo: String, pub goal: String }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefineQuestionsResponse { pub refined_goal: String, pub questions: Vec<String>, pub cost: f64 }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefineFinalizeRequest { pub repo: String, pub goal: String, pub answers: Vec<(String, String)> }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefineFinalizeResponse { pub refined_goal: String, pub cost: f64 }
```

- [ ] **Step 1: Server wrappers + routes**
- `POST /api/refine/questions`: run pass 1 (`run_refine_pass` with `refine_questions_prompt`), truncate to `REFINE_MAX_QUESTIONS`, return the parsed result and cost; on failure return the original goal, empty questions, and the incurred cost (same fallback semantics as the TUI).
- `POST /api/refine/finalize`: run pass 2 with `refine_finalize_prompt`; on failure return the original goal.

- [ ] **Step 2: Test** each endpoint with a fake `claude` writing `.agentic-refine.json` (questions case and finalize case), asserting the parsed goal/questions and that a billed-but-unparseable pass still reports its cost.

- [ ] **Step 3: `make verify` and commit**

```bash
git add -A
git commit -m "feat: refine questions and finalize HTTP endpoints

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: Web Workspaces view (picker + onboarding)

Leptos view listing configured workspaces and an add panel (the onboarding wizard as a form).

**Files:**
- Create: `crates/web/src/api.rs` (fetch helpers via gloo-net), `crates/web/src/views/workspaces.rs`
- Modify: `crates/web/src/main.rs` (router)

**Structure:**
- `api.rs`: async fns `list_workspaces()`, `scan(root)`, `save(workspaces)`, `start_run(req)`, `refine_questions(..)`, `refine_finalize(..)` using `gloo_net::http::Request` and the `shared` DTOs.
- `views/workspaces.rs`: `#[component] fn Workspaces()`. On mount, `list_workspaces()` into a signal; render each as a row linking to `/run/new?workspace=<name>`. An "Add workspace" panel: a path input, a "Scan" button calling `scan`, a checklist of results (each a checkbox), and a "Save" button calling `save` then refreshing the list. If the list is empty on load, expand the add panel by default.

- [ ] **Step 1: Implement `api.rs` and `views/workspaces.rs`** per the structure. Keep component logic (which repos are checked, the scan-result signal) pure and small.

- [ ] **Step 2: Wire the router** in `main.rs` with `leptos_router`: routes `/` -> `Workspaces`, `/run/new` -> `NewRun` (Task 8), `/run/:id` -> `Run` (Task 9). Stub `NewRun`/`Run` as empty components for now so it compiles.

- [ ] **Step 3: Build and manual check**

```bash
cd crates/web && trunk build && cd ../..
cargo check -p web --target wasm32-unknown-unknown
```

Manual browser check is deferred to the controller (Task 10 flip), but confirm it compiles and `trunk build` succeeds.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat: web workspaces view with onboarding scan and save

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: Web New-run view (goal + options + refine flow)

**Files:**
- Create: `crates/web/src/views/new_run.rs`
- Modify: `crates/web/src/main.rs`

**Structure:** `#[component] fn NewRun()` reads the `workspace` query param, loads it (from the list) for its `base`/`integration` defaults. Form fields: a multi-line `<textarea>` goal (native newlines), `base` input, `into` input, `verify` input, and a "refine before planning" checkbox. On submit:
- If refine is on: `POST /refine/questions`; render each returned question with an answer `<input>`; a "Continue" button collects answers and `POST /refine/finalize`; show the returned goal in an editable field to confirm; accumulate `cost`.
- Then `POST /api/runs` with the (possibly refined) goal, options, and accumulated `refine_cost`; on success navigate to `/run/<id>`; on 400 show the validation error inline.

- [ ] **Step 1: Implement `new_run.rs`** per the structure, with the refine sub-flow as local reactive state (a small enum: `Editing | Answering(questions) | Confirming(goal) | Submitting`).

- [ ] **Step 2: Build/check** (`trunk build`, `cargo check -p web --target wasm32-unknown-unknown`).

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat: web new-run form with the goal-refine flow

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 9: Web Run view (live kanban/log/budget/abort/report)

**Files:**
- Create: `crates/web/src/views/run.rs`, `crates/web/src/ws.rs` (WebSocket client)
- Modify: `crates/web/src/main.rs`

**Structure:**
- `ws.rs`: open `ws://<host>/api/runs/<id>/events` via `web_sys::WebSocket`; on each message, `serde_json::from_str::<App>` and push into an `RwSignal<Option<App>>`.
- `views/run.rs`: `#[component] fn Run()` reads `:id`, opens the WS into an `App` signal, and renders from the latest snapshot:
  - header: goal, workspace, and a budget bar (`total_cost` / `budget_usd`).
  - a five-column kanban (`KanbanColumn` order Todo, In Progress, Review, Done, Blocked); bucket `app.epics` with `shared::kanban_column`, show the on-hold hint in Todo via `shared::is_on_hold`.
  - a scrolling log pane from the app's log lines.
  - an Abort button -> `POST /api/runs/<id>/abort`.
  - when `app.phase` is `Done`/`Failed`, render the final report (per-epic status, total cost, and the integration-branch reminder using the run's integration branch).

- [ ] **Step 1: Implement `ws.rs` and `run.rs`.** Reuse `shared::kanban_column`/`is_on_hold` so the bucketing matches the old TUI exactly.

- [ ] **Step 2: Build/check** (`trunk build`, `cargo check -p web --target wasm32-unknown-unknown`).

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat: web live run view with kanban, log, budget, and abort

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 10: Make the web UI the default and delete the TUI

Flip `main` to launch the server by default, and remove the ratatui/crossterm TUI entirely.

**Files:**
- Modify: `crates/server/src/main.rs`
- Delete: `crates/server/src/ui.rs`; the TUI loops (`run_picker`, `run_goal_input`, `run_onboarding`, the interactive parts of `refine.rs`, the terminal event loop)
- Modify: `crates/server/Cargo.toml` (drop `ratatui`, `crossterm`); `event.rs` (drop `Input`/`Tick`; `AppEvent` becomes just `Stage(StageEvent)` or is replaced by `StageEvent` directly in the pipeline sender)

- [ ] **Step 1: Simplify the pipeline channel** to carry `StageEvent` directly (the server no longer needs `AppEvent::Input`/`Tick`). Update `engine`/`orchestrator`/`run_pipeline` senders to `mpsc::UnboundedSender<StageEvent>` and adjust the run manager forwarder accordingly.

- [ ] **Step 2: Rewrite `main`** to: parse args (`--web` becomes the default; keep `--no-open` and an optional `--port`; `--workspace`/`--goal`/etc. are no longer needed because the browser drives the flow, so remove them), then `http::serve(open_browser).await`. Delete `run_picker`, `run_goal_input`, `run_onboarding`, `print_report` (the report now renders in the browser), and the terminal setup/loop.

- [ ] **Step 3: Delete `ui.rs`** and remove `mod ui;`. Remove `ratatui` and `crossterm` from `Cargo.toml`. Reduce `refine.rs` to its non-terminal logic (`run_refine_pass`, `parse_refine`, `RefineResult`); delete its `run`/screens.

- [ ] **Step 4: Build, test, and confirm no dead code**

```bash
cargo build && cargo test 2>&1 | tail -20
grep -rn "ratatui\|crossterm" crates/server/src   # expect: none
```

- [ ] **Step 5: `make verify` and commit**

```bash
git add -A
git commit -m "feat: serve the web UI by default and remove the ratatui TUI

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 11: Build, CI, and docs

**Files:**
- Modify: `Makefile`, `.github/workflows/ci.yml`, `README.md`

- [ ] **Step 1: Makefile** — `build` runs `cd crates/web && trunk build --release` then `cargo build`. `run` runs the server (`cargo run -p agentic-tui`). `verify` keeps fmt-check + clippy + test on the native crates and adds `cargo check -p web --target wasm32-unknown-unknown`.

- [ ] **Step 2: CI** — install the wasm target and `trunk` (`cargo install --locked trunk`), build the web crate, then run `make verify`. Drop any 1.75-specific note.

- [ ] **Step 3: README** — replace the TUI description with the web UI: `agentic-tui` starts a local server and opens the browser; document `--no-open`/`--port`; note the new prerequisites (modern rustc, `wasm32-unknown-unknown`, `trunk`); remove the 1.75 pin note and the TUI key hints.

- [ ] **Step 4: `make verify` and commit**

```bash
git add -A
git commit -m "docs: build, CI, and README for the web UI

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Notes for the implementer

- Leptos 0.7 API: use `leptos::prelude::*`, signals via `RwSignal`, `#[component]`, and `view!`. If a pinned version differs, follow the installed version's docs; keep the component structure and the shared DTO boundary regardless of minor API drift.
- The server reuses `run_pipeline`, `orchestrator`, `engine`, `worktree`, `workspace`, `config`, and the refine passes without logic changes; only the presentation and the event channel type change.
- Do not use `Date::now`/randomness for run ids; use a monotonic `AtomicU64` counter.
- Browser end-to-end verification (clicking through the three views against a live run) is a manual step for the controller/user after Task 10; the automated gate covers the native API + logic and that the web crate compiles to wasm.
- Keep one run at a time in Phase 1; concurrent runs and history arrive in the next phase.
