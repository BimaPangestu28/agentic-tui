# Multi-run Dashboard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a session-scoped multi-run Dashboard (landing page), an app-bar runs-switcher, and the backend to keep every run of the session (active + finished) with one active run per workspace, matching the Claude Design `runs.html` mockup.

**Architecture:** The run manager becomes a `HashMap` registry keyed by run id; finished runs stay listed. A new `GET /api/runs` returns `RunSummary` snapshots, which the Dashboard polls. The frontend adds a Dashboard view and a shared app bar with a runs-switcher, and moves Workspaces to `/workspaces`.

**Tech Stack:** Rust 2021, axum + tokio (server), Leptos 0.8 CSR + leptos_router (web), serde (shared), trunk.

## Global Constraints

- No `unwrap()`/`expect()`/`panic!` in production code (tests may).
- Comment/prose style: no em dashes, no contractions.
- `make verify` green each task (fmt-check, clippy `--all-targets -- -D warnings`, tests, plus `cargo check -p web --target wasm32-unknown-unknown`). The `web` crate is excluded from `default-members`, so run its wasm check and `trunk build` explicitly.
- No `#[allow(dead_code)]` in `src`/`crates` after each task.
- Session-scoped only: runs live in memory for the server process; no disk persistence.
- Conventional commits; commit after every task. Work on branch `feat/web-ui-phase1`.

## File Structure

| File | Change |
|---|---|
| `crates/shared/src/lib.rs` | `RunSummary` DTO + test |
| `crates/server/src/run.rs` | `HashMap` registry, per-workspace-busy, `list()`, mark-finished-on-abort |
| `crates/server/src/http.rs` | `GET /api/runs`; `WorkspaceBusy` -> 409 |
| `crates/web/src/api.rs` | `list_runs()` |
| `crates/web/src/components.rs` (new) | shared `AppBar` + runs-switcher |
| `crates/web/src/views/dashboard.rs` (new) | Dashboard view |
| `crates/web/src/views/mod.rs`, `main.rs` | routes + app bar wiring |
| `crates/web/src/views/new_run.rs` | Cancel button |
| `README.md` | dashboard + per-workspace concurrency |

---

### Task 1: `RunSummary` DTO in shared

**Files:** Modify `crates/shared/src/lib.rs`.

- [ ] **Step 1: Add the DTO and a round-trip test**

Add near the other DTOs in `crates/shared/src/lib.rs`:

```rust
/// A snapshot of one run for the multi-run dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    pub id: String,
    pub workspace: String,
    pub path: String,
    pub goal: String,
    pub phase: Phase,
    pub total_cost: f64,
    pub budget: f64,
    pub epics: Vec<EpicView>,
}
```

Add a test in the `#[cfg(test)] mod tests` block:

```rust
#[test]
fn run_summary_round_trips_through_json() {
    let summary = RunSummary {
        id: "1".to_string(),
        workspace: "greentic".to_string(),
        path: "/tmp/greentic".to_string(),
        goal: "Add a health check".to_string(),
        phase: Phase::Running,
        total_cost: 0.42,
        budget: 10.0,
        epics: Vec::new(),
    };
    let json = serde_json::to_string(&summary).expect("serialize");
    let back: RunSummary = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.workspace, "greentic");
    assert_eq!(back.phase, summary.phase);
}
```

If `Phase` does not derive `PartialEq`, compare `format!("{:?}", ...)` instead.

- [ ] **Step 2: Verify and commit**

Run: `cargo test -p shared && cargo check -p shared --target wasm32-unknown-unknown`
Expected: pass; shared still wasm-safe.

```bash
git add crates/shared/src/lib.rs
git commit -m "feat: add the RunSummary dashboard DTO

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Session run registry + `GET /api/runs`

Turn the single-run slot into a session registry and expose the list. This is one task because `run.rs` and `http.rs` are compile-coupled (the new `StartError` variant, `list()`, and the route land together).

**Files:** Modify `crates/server/src/run.rs`, `crates/server/src/http.rs`.

**Interfaces:**
- `run::start` may return `StartError::WorkspaceBusy`.
- `run::list() -> Vec<shared::RunSummary>`.
- `GET /api/runs -> Json<Vec<RunSummary>>`.

- [ ] **Step 1: Registry state**

In `crates/server/src/run.rs`, replace the single-run statics with a map, and extend `RunHandle` with the workspace name (the repo path is already stored):

```rust
use std::collections::HashMap;
// ... existing imports ...

struct RunHandle {
    id: String,
    workspace: String,
    app: Arc<Mutex<App>>,
    tx: broadcast::Sender<App>,
    task: JoinHandle<()>,
    repo: PathBuf,
    completed: Arc<AtomicBool>,
}

static RUNS: Mutex<Option<HashMap<String, RunHandle>>> = Mutex::const_new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
```

(`Mutex::const_new` cannot build a `HashMap` in a const, so store `Option<HashMap>` and lazily initialize to `Some(HashMap::new())` on first use via a small helper, or use `HashMap::new()` behind a `tokio::sync::Mutex` created in a `once` — simplest is the `Option` with a `get_or_insert_with(HashMap::new)` at each lock.)

- [ ] **Step 2: `StartError::WorkspaceBusy`**

Extend the enum and its message:

```rust
pub enum StartError {
    Invalid(String),
    WorkspaceBusy,
}
```

```rust
impl StartError {
    pub fn message(&self) -> String {
        match self {
            StartError::Invalid(msg) => msg.clone(),
            StartError::WorkspaceBusy => {
                "this workspace already has a run in flight; only one run per workspace at a time".to_string()
            }
        }
    }
}
```

(Rename the old `Busy` variant to `WorkspaceBusy`; update the `http.rs` match arm in Step 6.)

- [ ] **Step 3: `start` inserts by id, rejects a busy workspace**

Keep all the existing validation (base ref via `verify_ref`, empty/checked-out integration guard). Replace the active-slot check and insertion:

```rust
    let mut guard = RUNS.lock().await;
    let runs = guard.get_or_insert_with(HashMap::new);
    let workspace_busy = runs.values().any(|handle| {
        handle.workspace == req.workspace.name && !handle.completed.load(Ordering::SeqCst)
    });
    if workspace_busy {
        return Err(StartError::WorkspaceBusy);
    }

    let id = NEXT_ID.fetch_add(1, Ordering::SeqCst).to_string();
    // ... build app, tx, completed, task exactly as before ...
    runs.insert(
        id.clone(),
        RunHandle {
            id: id.clone(),
            workspace: req.workspace.name.clone(),
            app,
            tx,
            task,
            repo,
            completed,
        },
    );
    Ok(id)
```

Note `req.workspace.name` is read before it is moved into the pipeline; clone it early if needed.

- [ ] **Step 4: `abort` marks the run finished but keeps it listed**

```rust
pub async fn abort(id: &str) {
    // Extract what we need under the lock, then release it before awaiting.
    let target = {
        let mut guard = RUNS.lock().await;
        let runs = guard.get_or_insert_with(HashMap::new);
        match runs.get(id) {
            Some(handle) if !handle.completed.load(Ordering::SeqCst) => Some((
                handle.task.abort_handle(),
                handle.app.clone(),
                handle.tx.clone(),
                handle.completed.clone(),
                handle.repo.clone(),
            )),
            _ => None,
        }
    };
    if let Some((abort_handle, app, tx, completed, repo)) = target {
        abort_handle.abort();
        // Mark it failed so the dashboard shows a terminal state, and keep the
        // handle in the registry so it still lists.
        {
            let mut app = app.lock().await;
            app.apply_stage(StageEvent::Fatal { reason: "run aborted".to_string() });
            let _ = tx.send(app.clone());
        }
        completed.store(true, Ordering::SeqCst);
        if let Err(e) = worktree::cleanup_all(&repo).await {
            eprintln!("warning: could not clean up worktrees after abort: {e}");
        }
    }
}
```

(`JoinHandle::abort_handle()` gives an `AbortHandle` that is `Send` and lets us abort without holding the `JoinHandle` across the lock. Import `StageEvent` from `shared`.)

- [ ] **Step 5: `subscribe` and `list`**

```rust
pub async fn subscribe(id: &str) -> Option<(App, broadcast::Receiver<App>)> {
    let mut guard = RUNS.lock().await;
    let runs = guard.get_or_insert_with(HashMap::new);
    let handle = runs.get(id)?;
    let snapshot = handle.app.lock().await.clone();
    Some((snapshot, handle.tx.subscribe()))
}

pub async fn list() -> Vec<shared::RunSummary> {
    let mut guard = RUNS.lock().await;
    let runs = guard.get_or_insert_with(HashMap::new);
    let mut out = Vec::with_capacity(runs.len());
    for handle in runs.values() {
        let app = handle.app.lock().await;
        out.push(shared::RunSummary {
            id: handle.id.clone(),
            workspace: handle.workspace.clone(),
            path: handle.repo.to_string_lossy().to_string(),
            goal: app.goal.clone(),
            phase: app.phase,
            total_cost: app.total_cost,
            budget: app.budget,
            epics: app.epics.clone(),
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}
```

(Confirm the `App` field names against `crates/shared/src/lib.rs`: `goal`, `phase`, `total_cost`, `budget`, `epics`.)

- [ ] **Step 6: Route + handler in `http.rs`**

Add the list handler and route, and update the start-run 409 arm for the renamed variant:

```rust
async fn list_runs() -> Json<Vec<shared::RunSummary>> {
    Json(run::list().await)
}
```

In `start_run`, change the busy arm:

```rust
        Err(e @ StartError::WorkspaceBusy) => (StatusCode::CONFLICT, e.message()).into_response(),
        Err(e @ StartError::Invalid(_)) => (StatusCode::BAD_REQUEST, e.message()).into_response(),
```

Mount `GET /api/runs`:

```rust
        .route("/api/runs", get(list_runs).post(start_run))
```

(Replace the existing `.route("/api/runs", post(start_run))` with the combined `get(...).post(...)`.)

- [ ] **Step 7: Tests**

In `crates/server/tests/`, extend the fake-`claude` harness (a `claude` on `PATH` that writes an empty `.agentic-plan.json` and emits a `result` line):

```rust
// After starting a run in workspace A, starting a second run in workspace A
// (while the first is active) is rejected; a run in workspace B is allowed;
// list() returns both; a finished/aborted run stays in list().
```

Assert: `run::start` for the same workspace twice (second while first active) returns `Err(StartError::WorkspaceBusy)`; a different temp workspace starts fine; `run::list()` includes both; after `run::abort(id)`, that run still appears in `list()` with a terminal phase. Serialize `PATH`/`HOME`/cwd mutation with the crate's existing `#[cfg(test)]` locks.

- [ ] **Step 8: Verify and commit**

Run: `make verify`
Expected: green (native tests + wasm check).

```bash
git add crates/server/src/run.rs crates/server/src/http.rs
git commit -m "feat: keep every run in a session registry, one active per workspace

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Web API, shared app bar + runs-switcher, and the route move

Add the `list_runs` fetch, a shared app bar component with the runs-switcher, and reshuffle the routes so `/` is the Dashboard (a stub until Task 4) and `/workspaces` is the picker. Compile-coupled via `main.rs`.

**Files:** Modify `crates/web/src/api.rs`, `crates/web/src/main.rs`, `crates/web/src/views/mod.rs`; create `crates/web/src/components.rs` and a stub `crates/web/src/views/dashboard.rs`.

- [ ] **Step 1: `list_runs` in `api.rs`**

```rust
pub async fn list_runs() -> Result<Vec<RunSummary>, String> {
    let response = Request::get("/api/runs")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    response.json::<Vec<RunSummary>>().await.map_err(|e| e.to_string())
}
```

Import `shared::RunSummary`.

- [ ] **Step 2: Shared app bar with the runs-switcher (`components.rs`)**

Create `crates/web/src/components.rs` with `#[component] pub fn AppBar()`:
- brand `<A href="/"><span class="hex">"\u{2b21}"</span>" Agentic Orchestrator"</A>`.
- a `<nav>` with `<A class="btn btn-ghost btn-sm" href="/workspaces">"Workspaces"</A>` and `<A class="btn btn-ghost btn-sm" href="/workspaces">"New run"</A>`.
- a `.runs-switcher` reading `api::list_runs()` on a short interval (leptos `set_interval` or a resource refetched on a timer): the trigger shows the active-run count ("N running") with a `.live-dot` when > 0, and a `.runs-menu` dropdown listing active runs (each `<A class="runs-menu-item" href=format!("/run/{id}")>` with workspace, a goal snippet, and a mini budget). Use `:focus-within` (CSS already opens the menu on focus) so no JS toggle is needed.

Match the `runs.html` markup (`.runs-switcher > .trigger`, `.live-dot`, `.caret`, `.runs-menu`, `.runs-menu-head`, `.runs-menu-item`).

- [ ] **Step 3: Dashboard stub + route move in `main.rs`**

Create `crates/web/src/views/dashboard.rs` with a stub `#[component] pub fn Dashboard() -> impl IntoView { view! { <div class="dashboard-view"><h1>"Dashboard"</h1></div> } }` (filled in Task 4). Export it from `views/mod.rs`.

Rewrite `main.rs`'s `App` to use the shared `AppBar` and the new routes:

```rust
view! {
    <Router>
        <AppBar />
        <main class="app-main">
            <Routes fallback=|| view! { <h1>"Not found"</h1> }>
                <Route path=path!("/") view=Dashboard />
                <Route path=path!("/workspaces") view=Workspaces />
                <Route path=path!("/run/new") view=NewRun />
                <Route path=path!("/run/:id") view=Run />
            </Routes>
        </main>
    </Router>
}
```

Add `mod components;` and the imports. Remove the old inline `<header class="app-bar">` from `main.rs` (now in `AppBar`).

- [ ] **Step 4: Build checks**

Run: `cargo check -p web --target wasm32-unknown-unknown` and `cd crates/web && trunk build && cd ../..` and `make verify`.
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add crates/web/src/api.rs crates/web/src/components.rs crates/web/src/main.rs crates/web/src/views/mod.rs crates/web/src/views/dashboard.rs
git commit -m "feat: web runs list api, shared app bar with runs-switcher, dashboard route

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: The Dashboard view

Fill in `dashboard.rs` to match `runs.html`.

**Files:** Modify `crates/web/src/views/dashboard.rs`.

- [ ] **Step 1: Fetch + poll**

On mount, load `api::list_runs()` into an `RwSignal<Vec<RunSummary>>` and refetch on a ~1.5s interval (leptos `set_interval_with_handle`; keep the handle for the component's lifetime). On a fetch error keep the last value.

- [ ] **Step 2: Render the sections (match `runs.html`)**

Compute from the runs list:
- active runs = runs whose `phase` is `Planning` or `Running`; total runs = list length.
- all epics = every run's epics, paired with the run's workspace for the board label.
- status counts = epics bucketed by `shared::kanban_column` into Todo / In progress / Review / Done / Blocked; total epics; total spend = sum of `total_cost`.

Render:
- `.page-head` (flex, space-between): "Dashboard" + `.sub`, and a `<A class="btn btn-primary" href="/workspaces">"+ New run"</A>`.
- `.dashboard-overview > .overview-card`: `.overview-stats` (Active loops `{active} / {total}`, Epics `{total_epics}`, Total spend `.ov-value.mono` `${spend:.2}`), `.overview-divider`, `.overview-dist` (`.dist-title` "Epic status", `.stacked-bar` with `.seg.todo/prog/review/done/block` widths = percentage of total, and `.chart-legend` with counts).
- `.section-head` "Board" + sub, then `.kanban-board` with the five `.kanban-column` (header `<h3>{label} <span class="count">{n}</span></h3>`), each card `.kanban-card` carrying a `.kanban-card-run` workspace label, `.kanban-card-title`, and `.kanban-card-meta` (id + status). Reuse the status/label helpers from `run.rs` (extract them to a shared `views` helper module or duplicate the small `status_label`).
- `.section-head` "Runs" + "Grouped by workspace", then `.ws-groups`: group the runs by workspace; each `.ws-group` with `.ws-group-head` (`.ws-hex`, `.ws-name`, `.ws-path`, spacer, `.ws-count` "N runs", `<A class="btn btn-ghost btn-sm" href=/run/new?workspace=name>"+ New run"</A>`) and a `.runs-list` of `<A class="run-card {phase-class}" href=/run/{id}>` (phase dot, `.col` with `.rc-goal` + `.rc-meta`, `.rc-right` with `.run-phase {phase}` badge and `.rc-budget` mini bar + `.mini-budget`).
- Empty state when the list is empty: a `.empty-state` with a hexagon and a link to `/workspaces`.

Phase -> class: `Planning`->"planning", `Running`->"running", `Done`->"done", `Failed`->"failed". Phase -> label: "Planning"/"Running"/"Done"/"Failed".

- [ ] **Step 3: Build checks and commit**

Run: `cargo check -p web --target wasm32-unknown-unknown`, `trunk build`, `make verify`.

```bash
git add crates/web/src/views/dashboard.rs crates/web/src/views/*.rs
git commit -m "feat: the multi-run dashboard view

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: New-run Cancel, and docs

**Files:** Modify `crates/web/src/views/new_run.rs`, `README.md`.

- [ ] **Step 1: Cancel button**

In `new_run.rs`, add a `.new-run-actions` Cancel button before the primary action that navigates to `/` (leptos_router `use_navigate`): `<button type="button" class="btn-ghost" on:click=... >"Cancel"</button>`. The primary "Start run" / "Refine & plan" button stays.

- [ ] **Step 2: README**

Document that the app opens on a Dashboard listing every run of the session, one active run per workspace, workspaces at `/workspaces`, and that history is session-scoped (not persisted).

- [ ] **Step 3: Verify and commit**

Run: `make verify` and `trunk build`.

```bash
git add crates/web/src/views/new_run.rs README.md
git commit -m "feat: new-run cancel and dashboard docs

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Notes for the implementer

- The registry uses `Mutex<Option<HashMap<..>>>` only because `Mutex::const_new` cannot construct a `HashMap` in a `const`; treat it as always-`Some` via `get_or_insert_with(HashMap::new)` at each lock. Do not hold the `RUNS` lock across the per-run `app.lock().await` longer than needed; `list()` locks each `app` briefly in turn, which is fine for a local tool.
- Browser end-to-end (clicking through the dashboard) is a manual step for the controller/user; the automated gate covers the native API/registry, the DTO round-trip, and that the web crate compiles to wasm and `trunk build` succeeds.
- Keep the per-run `/run/:id` WebSocket unchanged; only the Dashboard polls `/api/runs`.
