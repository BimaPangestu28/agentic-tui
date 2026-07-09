# Multi-run Dashboard Design (session-scoped)

## Context

The web UI (Phase 1) drives one run at a time and lands on the Workspaces
picker. The Claude Design project ships a richer landing experience
(`mockups/runs.html`): a Dashboard that shows every run in the current session
(live and finished), an aggregate overview, a global kanban board across all
runs, and runs grouped by workspace, with an app-bar runs-switcher to hop
between run detail views. This spec implements that dashboard.

## Goal

Turn the app into a multi-run hub, session-scoped and in-memory (no disk
persistence): keep every run started this server session (active and finished),
allow one active run per workspace, and add a Dashboard landing page plus an
app-bar runs-switcher, matching the `runs.html` / `style.css` design.

## Non-goals

- No disk persistence or cross-session history (runs live only while the server
  process runs, matching the mockup's "every run in this session").
- No change to the plan/implement/verify/integrate pipeline or the per-run
  detail view's live WebSocket.

## Routes

- `/` — **Dashboard** (new landing).
- `/workspaces` — the Workspaces picker + onboarding (moved from `/`).
- `/run/new?workspace=<name>` — New run form (unchanged behavior; gains a Cancel).
- `/run/:id` — the live Run detail dashboard (unchanged).

The app bar (on every page) shows: the brand linking to `/`, nav links
(Workspaces, New run), and a runs-switcher.

## Backend

### Run manager (session-scoped, multi-run)

Replace the single-run slot with a session registry:

- `static RUNS: Mutex<HashMap<String, RunHandle>>` — every run started this
  session, keyed by run id. Finished runs are NOT removed (they stay for the
  Dashboard). `RunHandle` keeps `id`, `workspace` name, `path`, `App`
  (`Arc<Mutex<App>>`), the broadcast sender, the task handle, and `completed`.
- `start(req)`: validation as today (base ref, integration not checked out).
  Reject with a new `StartError::WorkspaceBusy` (mapped to 409) if the request's
  workspace already has an **active** run (a run whose phase is not Done/Failed).
  Otherwise insert a new run and return its id.
- `abort(id)`: unchanged (abort the task + cleanup if not completed). Keep the
  handle in the map with `completed = true` so it still lists.
- `subscribe(id)`: unchanged (snapshot + broadcast receiver), now looked up by id
  in the map.
- `list() -> Vec<RunSummary>`: a snapshot of every run for the Dashboard.

### DTOs (in `shared`)

```rust
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

`Phase` and `EpicView` already derive serde in `shared`.

### Endpoints

- `GET /api/runs` -> `Json<Vec<RunSummary>>` (the session registry snapshot).
- `POST /api/runs` -> unchanged, plus the per-workspace-busy 409.
- `POST /api/runs/{id}/abort`, `GET /api/runs/{id}/events` -> unchanged.

The Dashboard polls `GET /api/runs` on a short interval (about 1.5s) for a live
feel; the per-run detail view keeps its dedicated WebSocket. Polling is fine for
a local single-user tool and avoids an all-runs streaming channel.

## Frontend

### Dashboard (`/`)

Fetch `/api/runs` on mount and on an interval. Render, matching `runs.html`:

- **Page head**: "Dashboard" + subtitle, and a "New run" button (to
  `/workspaces`, where a workspace is chosen).
- **Overview card**: Active loops (active run count `/` total runs this session),
  Epics (total across all runs), Total spend (sum of `total_cost`, mono amber),
  and an Epic-status **stacked bar** + legend counting all epics by kanban column
  (Todo, In progress, Review, Done, Blocked).
- **Board** section: a global five-column kanban of every epic across all runs.
  Each card carries a `.kanban-card-run` workspace label (which run it belongs
  to), then title, meta (id + status). Bucketing reuses `shared::kanban_column`.
- **Runs** section: grouped by workspace (`.ws-group` with `.ws-group-head`
  showing the workspace name/path, run count, and a "+ New run" link to
  `/run/new?workspace=<name>`); each run is a `.run-card` linking to `/run/<id>`
  with a phase dot/badge (planning/running/done/failed) and a mini budget bar.
- **Empty state** when there are no runs: a prompt pointing to `/workspaces`.

Phase -> run-card class: `Planning` -> `planning`, `Running` -> `running`,
`Done` -> `done`, `Failed` -> `failed`.

### App bar + runs-switcher

The app bar becomes a shared component used by all pages: brand (to `/`), nav
links (Workspaces -> `/workspaces`, New run -> `/workspaces`), and a
`.runs-switcher`: a trigger showing "N running" with a live dot, and a dropdown
(`.runs-menu`) listing active runs, each linking to `/run/<id>`. The switcher
reads the same `/api/runs` data (its own short poll, or shared state). Hidden or
showing "0 running" when there are no active runs.

### Workspaces / New run adjustments

- Workspaces moves to `/workspaces`; its rows still link to
  `/run/new?workspace=<name>`.
- New run gains a **Cancel** (`btn-ghost`) button that navigates back to `/`.
- The `main.rs` router adds the Dashboard route and the moved Workspaces route,
  and wraps pages with the shared app bar.

## Error handling

- Starting a second run for a busy workspace returns 409 with a clear message;
  the New-run form shows it inline (as it already shows 400/409 messages).
- `GET /api/runs` never fails destructively; on the client, a fetch error leaves
  the last snapshot and retries on the next poll tick.
- Aborting or a finished run stays in the registry so the Dashboard can show it.

## Testing

- `shared`: `RunSummary` serde round-trip.
- `server`: the run manager holds multiple runs; a second run for the same
  workspace while one is active is rejected; a finished run stays listed;
  `list()` reflects all runs. Reuse the fake-`claude`-on-PATH integration
  harness.
- `web`: components compile and `trunk build` succeeds; the dashboard's pure
  helpers (aggregate counts, phase->class, bucketing via shared) are unit-tested
  where extractable.
- End-to-end: a pty/HTTP smoke with a fake `claude` starts two runs in two temp
  workspaces and asserts `GET /api/runs` lists both with the right phases.

## Files

| File | Change |
|---|---|
| `crates/shared` | `RunSummary` DTO |
| `crates/server/src/run.rs` | `HashMap` registry, per-workspace-busy, `list()`, keep finished |
| `crates/server/src/http.rs` | `GET /api/runs`; `WorkspaceBusy` -> 409 |
| `crates/web/src/api.rs` | `list_runs()` |
| `crates/web/src/components.rs` (new) | shared app bar + runs-switcher |
| `crates/web/src/views/dashboard.rs` (new) | the Dashboard view |
| `crates/web/src/views/workspaces.rs` | route move (logic unchanged) |
| `crates/web/src/views/new_run.rs` | Cancel button |
| `crates/web/src/main.rs` | routes (`/` dashboard, `/workspaces`), shared app bar |
| `README.md` | document the dashboard + per-workspace concurrency |
