# Run persistence and resume

## Problem

The run registry (`RUNS` in `crates/server/src/run.rs`) is an in-memory
`static Mutex<Option<HashMap<String, RunHandle>>>`. Every run started in a
session lives only in RAM. When the server process stops, all runs, their
epics, phases, costs, and logs vanish. Only two things survive a restart:

- Workspaces, persisted to `~/.config/agentic-tui/workspaces.toml`.
- The most recent plan per workspace root, written to `.agentic-plan.json`
  at `common_root(workspace)`. This file is overwritten by every subsequent
  run in the same workspace root, so it is not a reliable per-run record.

Users lose their run history and any in-flight work whenever the server
restarts.

## Goal

Persist each run to disk so that, after a restart:

1. Finished runs reappear as read-only history (goal, epics, statuses, cost,
   report, log).
2. Runs interrupted mid-flight can be resumed. Resume re-invokes the
   orchestrator with the run's stored plan, skipping already-merged epics, so
   the DAG scheduler re-runs pending and interrupted epics while honoring
   dependencies and parallelism.

Resume is **manual, per run**: the user clicks a "Resume run" button. Nothing
resumes automatically on startup, so an accidental restart never triggers
unexpected Claude spend.

## Non-goals

- Resuming a run that was interrupted during the Planning phase (before the
  plan existed). Such a run produced no usable artifacts; it is simply not
  persisted. Planning is cheap and short.
- Reattaching to a dead Claude session. Interrupted epics are re-run from the
  plan, not resumed at the token level.
- Changing the existing per-epic retry path for finished runs. It stays as-is.

## Approach

Chosen approach (A): **run-level resume via the orchestrator.** On resume,
call the orchestrator with the persisted plan and seed the already-merged
epics as complete. The existing DAG scheduler then runs everything not yet
merged, in dependency order, up to `MAX_PARALLEL_EPICS` at a time. This reuses
the entire scheduling, verification, and merge machinery and correctly handles
both pending epics (never started) and interrupted epics (cut off mid-run).

Rejected: (B) reuse the per-epic retry path — leaves `Pending` epics stranded
with no retry affordance. (C) full resume including re-planning — over-built
for the low-value Planning-interruption case.

## Data model

One JSON file per run at `~/.config/agentic-tui/runs/<id>.json`, in the same
config directory as `workspaces.toml`.

```rust
struct PersistedRun {
    id: String,
    workspace: String,
    goal: String,
    default_verify: String,
    plan_cwd: PathBuf,
    repos: Vec<PersistedRepo>, // ordered, preserves display order
    plan_json: String,         // this run's own copy of .agentic-plan.json
    app: App,                  // full snapshot: phase, epics, cost, log, error
}

struct PersistedRepo {
    name: String,
    path: PathBuf,
    base_ref: String,
    integration_branch: String,
}
```

`app: shared::App` already derives `Serialize`/`Deserialize`. `plan_json` is a
per-run copy so resume never depends on the shared `.agentic-plan.json`, which
a later run in the same workspace root can overwrite. `repos` rebuilds the
`HashMap<String, orchestrator::RepoRun>`, the ordered `repo_names`, and
`repo_paths` a `RunHandle` needs.

## Components

### New module `crates/server/src/run_store.rs`

Owns all disk I/O for persisted runs, keeping `run.rs` (already large)
focused.

- `runs_dir() -> PathBuf` — `~/.config/agentic-tui/runs/`, mirroring
  `workspace::default_config_path()`.
- `save(run: &PersistedRun) -> anyhow::Result<()>` — atomic write: serialize
  to `<id>.json.tmp`, then rename to `<id>.json`, so a crash mid-write never
  leaves a torn file. Creates `runs_dir()` if absent.
- `load_all() -> Vec<PersistedRun>` — read every `*.json`, skipping (and
  logging a warning for) any file that fails to parse, so one bad file never
  blocks startup.
- `delete(id: &str) -> anyhow::Result<()>` — remove a run's file.

### `plan.rs` and `orchestrator.rs` derives

Add `Serialize`/`Deserialize` to `plan::{Plan, Epic, Task}` and to
`orchestrator::RepoRun`. These are plain data types.

### `orchestrator.rs`: `run_resume`

Add an entry point that pre-seeds a given set of epic ids as `Merged` in the
initial `RunState`, then runs the existing scheduler loop. Merged epics are
terminal-success, so the scheduler skips them and their dependents inherit the
merged work from the integration branch. Signature mirrors `run`:

```rust
pub async fn run_resume(
    plan: &Plan,
    config: RunConfig,
    seed_merged: &[String],
    tx: mpsc::UnboundedSender<StageEvent>,
) -> anyhow::Result<()>
```

`run` and `run_resume` share the scheduler; `run` is `run_resume` with an
empty `seed_merged`.

### `run.rs` changes

- **Persistence hook.** Pass a small `PersistCtx` (id, workspace, goal,
  default_verify, plan_cwd, ordered repos) into `spawn_pipeline` and
  `spawn_retry`. In their forward loops, after `apply_stage` + broadcast, when
  the event is non-streaming (not `StageLog`/`StageAssistant`/`StageTool`),
  build a `PersistedRun` from the ctx plus the current `App` and call
  `run_store::save`. `save` reads `plan_cwd/.agentic-plan.json` for
  `plan_json` (valid: the busy guard prevents a concurrent run in the same
  workspace from overwriting it). `save` writes nothing while `app.phase ==
  Planning`, so a planning-time interruption leaves no file.

- **`rehydrate()`.** Called once at startup. `run_store::load_all()`, then for
  each `PersistedRun`:
  - Rebuild a `RunHandle`: `App` from the snapshot, a fresh `broadcast`
    channel, `task: None`, `completed: true`, `repos`/`repo_names`/
    `repo_paths` reconstructed from `PersistedRepo`.
  - Transform interrupted state when the persisted `phase` is `Implementing`:
    epics in `Running`/`Verifying` become `Failed` with reason "interrupted by
    a server restart"; `Pending` epics stay `Pending`; `Merged` stay `Merged`;
    `phase` becomes `Failed` with `error` "Interrupted by a server restart.
    Resume to continue." The dashboard then shows an honest Failed run, not a
    fake "Running" one.
  - Terminal runs (`Done`, or `Failed` from the original run) are kept as-is.
  - Track the max numeric id; set `NEXT_ID` to `max + 1` so new runs never
    collide with rehydrated ids.

- **`resume(run_id)`.** Validates the run exists, is `completed`, and has at
  least one non-`Merged` epic. Cleans stale worktrees first
  (`worktree::cleanup_all` per repo) so `worktree::create` does not fail on a
  leftover branch/worktree. Flips `completed = false`, parses the stored
  `plan_json`, computes `seed_merged` from the epics currently `Merged`, and
  spawns a task that calls `orchestrator::run_resume`, forwarding events into
  the run's existing `App` exactly as `spawn_pipeline` does. On completion,
  `completed = true` and the final snapshot is persisted.

### `http.rs` changes

- Add route `POST /api/runs/{id}/resume` → handler over `run::resume`, mapping
  its errors to status codes the way `retry_epic` does.
- Call `run::rehydrate().await` inside `serve()` before `axum::serve`.

### Web UI changes (`crates/web`)

- Add a "Resume run" button, shown when a run is resumable: `phase == Failed`
  and at least one epic is not `Merged`. Place it on the run view; optionally
  surface it on the dashboard run card.
- Wire the button to `POST /api/runs/{id}/resume` and reload on success.
- Show an "interrupted" indicator on runs recovered from disk (driven by the
  `Failed` phase plus the restart `error` string).

## Data flow

```
Run active:
  StageEvent -> App::apply_stage -> broadcast(App) -> [non-streaming] run_store::save
                                                        (skipped while Planning)

Server restart:
  serve() -> run::rehydrate() -> run_store::load_all()
          -> rebuild RunHandle per run, transform interrupted state, set NEXT_ID
          -> insert into RUNS

Resume:
  POST /api/runs/{id}/resume -> run::resume(id)
    -> validate + cleanup_all worktrees
    -> parse stored plan_json, seed_merged = merged epic ids
    -> spawn orchestrator::run_resume -> events -> App -> broadcast + persist
    -> completed = true, final snapshot saved
```

## Error handling

- **Torn files:** atomic tmp-then-rename write.
- **Corrupt run file:** skipped at load with a warning; never blocks startup.
- **Stale worktrees after a crash:** `worktree::cleanup_all` per repo before a
  resume re-runs epics. Merged work is safe on the integration branch; conflict
  worktrees are re-run from scratch, so removing them is acceptable.
- **Invalid resume** (run active, unknown, or nothing left to resume): a clear
  error returned to the UI, mirroring the `retry` error taxonomy.
- **Id collisions:** `NEXT_ID` advanced past the max rehydrated id.

## Testing

- `run_store` round-trip: `save` then `load_all` returns an equal
  `PersistedRun`; a malformed file is skipped.
- Atomic write leaves no `.tmp` behind on success.
- `rehydrate` transforms an interrupted `Implementing` run: `Running`/
  `Verifying` epics become `Failed`, `phase` becomes `Failed`, `completed` is
  true, and `NEXT_ID` is advanced.
- `rehydrate` keeps a terminal `Done` run unchanged.
- `orchestrator::run_resume` seeds merged epics and runs the rest, following
  the existing orchestrator test patterns.
- Integration test in a temp git repo, mirroring `tests/multi_repo.rs`: start a
  run, persist, clear `RUNS` and `rehydrate` to simulate a restart, assert
  `list()` shows the run, then `resume` re-runs the unfinished epics.
