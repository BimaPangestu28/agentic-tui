//! Run manager: a session registry of pipeline runs, one active run per
//! workspace at a time. Starts a run in a spawned task, applies its
//! `StageEvent`s to an `App`, and broadcasts a snapshot of the `App` after
//! each event so any number of WebSocket subscribers can follow the same run.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::{broadcast, mpsc, Mutex};
use tokio::task::JoinHandle;

use shared::{App, StageEvent};

use crate::{config, orchestrator, resolve_setting, run_pipeline, workspace, worktree};

pub use shared::StartRunRequest;

/// Snapshots buffered per subscriber before an idle one starts missing them
/// (`broadcast::error::RecvError::Lagged`). A subscriber that lags this far
/// behind just resyncs on the next snapshot instead of losing the run.
const SNAPSHOT_CHANNEL_CAPACITY: usize = 256;

/// Why `start` rejected a request: an invalid request (maps to 400) or the
/// request's workspace already has a run in flight (maps to 409).
#[derive(Debug, Clone, PartialEq)]
pub enum StartError {
    Invalid(String),
    WorkspaceBusy,
}

impl StartError {
    pub fn message(&self) -> String {
        match self {
            StartError::Invalid(msg) => msg.clone(),
            StartError::WorkspaceBusy => {
                "this workspace already has a run in flight; only one run per workspace at a time"
                    .to_string()
            }
        }
    }
}

/// One run in the registry, active or finished. Finished runs are kept
/// around (not removed) so they still show up in `list()`.
struct RunHandle {
    id: String,
    workspace: String,
    app: Arc<Mutex<App>>,
    tx: broadcast::Sender<App>,
    task: Option<JoinHandle<()>>,
    repo_paths: Vec<PathBuf>,
    completed: Arc<AtomicBool>,
}

/// Every run started this session, keyed by id. `Mutex::const_new` cannot
/// build a `HashMap` in a const context, so the map is built lazily on first
/// use via `get_or_insert_with` at each lock site.
static RUNS: Mutex<Option<HashMap<String, RunHandle>>> = Mutex::const_new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Start a run from `req`. Validates the resolved base ref and integration
/// branch before touching the registry, so an invalid request never contends
/// with a real run. Rejects with `StartError::WorkspaceBusy` if the request's
/// workspace already has a run active (not yet completed) in the registry.
pub async fn start(req: StartRunRequest) -> Result<String, StartError> {
    let workspace_name = req.workspace.name.clone();
    let repo = workspace::expand_tilde(&req.workspace.path);
    let repo = repo.canonicalize().unwrap_or(repo);

    let base_ref = resolve_setting(req.base.as_deref(), req.workspace.base.as_deref(), "HEAD");
    let integration = resolve_setting(
        req.into.as_deref(),
        req.workspace.integration.as_deref(),
        "agentic-integration",
    );
    let verify_cmd = req
        .verify
        .clone()
        .unwrap_or_else(|| config::DEFAULT_VERIFY_CMD.to_string());

    worktree::verify_ref(&repo, &base_ref)
        .await
        .map_err(|e| StartError::Invalid(e.to_string()))?;
    if integration.trim().is_empty() {
        return Err(StartError::Invalid(
            "into requires a branch name".to_string(),
        ));
    }
    let current_branch = worktree::current_branch(&repo)
        .await
        .map_err(|e| StartError::Invalid(e.to_string()))?;
    if current_branch.as_deref() == Some(integration.as_str()) {
        return Err(StartError::Invalid(format!(
            "cannot merge into '{integration}': it is checked out in the workspace. Check out a different branch first, or choose another integration target."
        )));
    }

    let mut guard = RUNS.lock().await;
    let runs = guard.get_or_insert_with(HashMap::new);
    let workspace_busy = runs.values().any(|handle| {
        handle.workspace == workspace_name && !handle.completed.load(Ordering::SeqCst)
    });
    if workspace_busy {
        return Err(StartError::WorkspaceBusy);
    }

    let id = NEXT_ID.fetch_add(1, Ordering::SeqCst).to_string();
    let app = Arc::new(Mutex::new(App::new(
        req.goal.clone(),
        workspace_name.clone(),
        config::GLOBAL_BUDGET_USD,
    )));
    let (tx, _rx) = broadcast::channel::<App>(SNAPSHOT_CHANNEL_CAPACITY);
    let completed = Arc::new(AtomicBool::new(false));

    // Phase A parity: build a one-repo map keyed by the workspace name, so the
    // multi-repo orchestrator runs a single repo exactly as before.
    let mut repos = HashMap::new();
    repos.insert(
        workspace_name.clone(),
        orchestrator::RepoRun {
            path: repo.clone(),
            base_ref,
            integration_branch: integration,
        },
    );

    let task = spawn_pipeline(
        app.clone(),
        tx.clone(),
        completed.clone(),
        repo.clone(),
        repos,
        req.goal,
        verify_cmd,
        req.refine_cost,
    );

    runs.insert(
        id.clone(),
        RunHandle {
            id: id.clone(),
            workspace: workspace_name,
            app,
            tx,
            task: Some(task),
            repo_paths: vec![repo],
            completed,
        },
    );

    Ok(id)
}

/// Spawn one task that runs the pipeline and forwards its events, so
/// `RunHandle` holds a single `JoinHandle` whose abort tears down both: the
/// pipeline future owns the `claude`/`git` child processes (`kill_on_drop`),
/// so cancelling this task kills them the same way the TUI's abort path does.
#[allow(clippy::too_many_arguments)]
fn spawn_pipeline(
    app: Arc<Mutex<App>>,
    tx: broadcast::Sender<App>,
    completed: Arc<AtomicBool>,
    plan_cwd: PathBuf,
    repos: HashMap<String, orchestrator::RepoRun>,
    goal: String,
    default_verify: String,
    refine_cost: f64,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let (pipeline_tx, mut rx) = mpsc::unbounded_channel::<StageEvent>();

        let pipeline_fut = async move {
            if let Err(e) = run_pipeline(
                &plan_cwd,
                repos,
                &goal,
                &default_verify,
                refine_cost,
                &pipeline_tx,
            )
            .await
            {
                let _ = pipeline_tx.send(StageEvent::Fatal {
                    reason: e.to_string(),
                });
            }
            // `pipeline_tx` drops here, closing the channel so the forwarder
            // below sees `None` and returns once the pipeline is done.
        };

        let forward_fut = async {
            while let Some(stage) = rx.recv().await {
                let done = matches!(stage, StageEvent::Done | StageEvent::Fatal { .. });
                let mut guard = app.lock().await;
                guard.apply_stage(stage);
                let _ = tx.send(guard.clone());
                if done {
                    completed.store(true, Ordering::SeqCst);
                }
            }
        };

        tokio::join!(pipeline_fut, forward_fut);
    })
}

/// Abort the run identified by `id` if it exists and has not completed:
/// abort its task and await its unwind (killing any in-flight `claude`/`git`
/// child processes via `kill_on_drop` as the pipeline future drops), mark it
/// `Fatal` so subscribers and `list()` see a terminal state, and clean up
/// epic worktrees, mirroring the TUI's abort path. The handle stays in the
/// registry (never removed, only its `task` becomes `None`) so the run still
/// appears in `list()`. A no-op for an unknown or already-finished id.
pub async fn abort(id: &str) {
    // Phase 1: find the handle, mark it completed, and take its JoinHandle
    // and clone what we need, all under the RUNS lock. Release the lock
    // before the awaits below, so a slow task teardown or worktree cleanup
    // never blocks other start, abort, list, or subscribe requests.
    let target = {
        let mut guard = RUNS.lock().await;
        let runs = guard.get_or_insert_with(HashMap::new);
        match runs.get_mut(id) {
            Some(handle) if !handle.completed.load(Ordering::SeqCst) => {
                handle.completed.store(true, Ordering::SeqCst);
                Some((
                    handle.task.take(),
                    handle.app.clone(),
                    handle.tx.clone(),
                    handle.repo_paths.clone(),
                ))
            }
            _ => None,
        }
    };
    let Some((task, app, tx, repo_paths)) = target else {
        return;
    };

    // Phase 2: abort the task and await its unwind. This drops the pipeline
    // future (and its `kill_on_drop` children) before we touch the app or
    // the worktrees, so cleanup never races a still-running child process.
    if let Some(task) = task {
        task.abort();
        let _ = task.await;
    }

    // Phase 3: now that the task has actually finished, deterministically
    // mark the run Failed.
    {
        let mut app = app.lock().await;
        app.apply_stage(StageEvent::Fatal {
            reason: "run aborted".to_string(),
        });
        let _ = tx.send(app.clone());
    }

    // Phase 4: clean up epic worktrees in every repo the run targeted, now
    // that no child process can still be touching them.
    for repo in repo_paths {
        if let Err(e) = worktree::cleanup_all(&repo).await {
            eprintln!(
                "warning: could not clean up worktrees for {}: {e}",
                repo.display()
            );
        }
    }
}

/// The current `App` snapshot and a live receiver of further snapshots for
/// the run identified by `id`, or `None` if no such run exists.
pub async fn subscribe(id: &str) -> Option<(App, broadcast::Receiver<App>)> {
    // Phase 1: grab the app handle and sender under the RUNS lock.
    let (app, tx) = {
        let mut guard = RUNS.lock().await;
        let runs = guard.get_or_insert_with(HashMap::new);
        let handle = runs.get(id)?;
        (handle.app.clone(), handle.tx.clone())
    };
    // Phase 2: snapshot the app and subscribe, RUNS already released.
    let snapshot = app.lock().await.clone();
    Some((snapshot, tx.subscribe()))
}

/// One run's metadata snapshotted under the RUNS lock: id, workspace name,
/// the repo paths it targets, and a handle to its live `App`.
type RunSnapshot = (String, String, Vec<PathBuf>, Arc<Mutex<App>>);

/// A snapshot of every run started this session (active and finished),
/// sorted by id, for the multi-run dashboard.
pub async fn list() -> Vec<shared::RunSummary> {
    // Phase 1: snapshot the per-run metadata + app handles under the RUNS
    // lock.
    let handles: Vec<RunSnapshot> = {
        let mut guard = RUNS.lock().await;
        let runs = guard.get_or_insert_with(HashMap::new);
        runs.values()
            .map(|h| {
                (
                    h.id.clone(),
                    h.workspace.clone(),
                    h.repo_paths.clone(),
                    h.app.clone(),
                )
            })
            .collect()
    };
    // Phase 2: build summaries by locking each app briefly, RUNS already
    // released.
    let mut out = Vec::with_capacity(handles.len());
    for (id, workspace, repo_paths, app) in handles {
        let app = app.lock().await;
        // The run's repos come from its config, not the epics. In Phase A a
        // run always targets the single workspace repo.
        let repos = vec![workspace.clone()];
        let path = repo_paths
            .first()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        out.push(shared::RunSummary {
            id,
            workspace,
            path,
            goal: app.goal.clone(),
            phase: app.phase,
            total_cost: app.total_cost,
            budget: app.budget,
            epics: app.epics.clone(),
            repos,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}
