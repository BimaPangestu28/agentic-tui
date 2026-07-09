//! Run manager: one pipeline run active at a time. Starts a run in a spawned
//! task, applies its `StageEvent`s to an `App`, and broadcasts a snapshot of
//! the `App` after each event so any number of WebSocket subscribers can
//! follow the same run.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::{broadcast, mpsc, Mutex};
use tokio::task::JoinHandle;

use shared::{App, StageEvent};

use crate::event::AppEvent;
use crate::{config, resolve_setting, run_pipeline, workspace, worktree};

pub use shared::StartRunRequest;

/// Snapshots buffered per subscriber before an idle one starts missing them
/// (`broadcast::error::RecvError::Lagged`). A subscriber that lags this far
/// behind just resyncs on the next snapshot instead of losing the run.
const SNAPSHOT_CHANNEL_CAPACITY: usize = 256;

/// Why `start` rejected a request: an invalid request (maps to 400) or a run
/// already active (maps to 409).
#[derive(Debug, Clone, PartialEq)]
pub enum StartError {
    Invalid(String),
    Busy,
}

impl StartError {
    pub fn message(&self) -> String {
        match self {
            StartError::Invalid(msg) => msg.clone(),
            StartError::Busy => "a run is already active".to_string(),
        }
    }
}

/// The one active (or most recently finished) run. `None` before the first
/// run and briefly after an abort clears it.
struct RunHandle {
    id: String,
    app: Arc<Mutex<App>>,
    tx: broadcast::Sender<App>,
    task: JoinHandle<()>,
    repo: PathBuf,
    completed: Arc<AtomicBool>,
}

static ACTIVE: Mutex<Option<RunHandle>> = Mutex::const_new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Start a run from `req`. Validates the resolved base ref and integration
/// branch before touching the active-run slot, so an invalid request never
/// contends with a real run. Rejects with `StartError::Busy` if a run is
/// already active and not yet completed.
pub async fn start(req: StartRunRequest) -> Result<String, StartError> {
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

    let mut active = ACTIVE.lock().await;
    if let Some(existing) = active.as_ref() {
        if !existing.completed.load(Ordering::SeqCst) {
            return Err(StartError::Busy);
        }
    }

    let id = NEXT_ID.fetch_add(1, Ordering::SeqCst).to_string();
    let app = Arc::new(Mutex::new(App::new(
        req.goal.clone(),
        req.workspace.name.clone(),
        config::GLOBAL_BUDGET_USD,
    )));
    let (tx, _rx) = broadcast::channel::<App>(SNAPSHOT_CHANNEL_CAPACITY);
    let completed = Arc::new(AtomicBool::new(false));

    let task = spawn_pipeline(
        app.clone(),
        tx.clone(),
        completed.clone(),
        repo.clone(),
        req.goal,
        verify_cmd,
        base_ref,
        integration,
        req.refine_cost,
    );

    *active = Some(RunHandle {
        id: id.clone(),
        app,
        tx,
        task,
        repo,
        completed,
    });

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
    repo: PathBuf,
    goal: String,
    verify_cmd: String,
    base_ref: String,
    integration: String,
    refine_cost: f64,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let (pipeline_tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();

        let pipeline_fut = async move {
            if let Err(e) = run_pipeline(
                &repo,
                &goal,
                &verify_cmd,
                &base_ref,
                &integration,
                refine_cost,
                &pipeline_tx,
            )
            .await
            {
                let _ = pipeline_tx.send(AppEvent::Stage(StageEvent::Fatal {
                    reason: e.to_string(),
                }));
            }
            // `pipeline_tx` drops here, closing the channel so the forwarder
            // below sees `None` and returns once the pipeline is done.
        };

        let forward_fut = async {
            while let Some(ev) = rx.recv().await {
                if let AppEvent::Stage(stage) = ev {
                    let done = matches!(stage, StageEvent::Done | StageEvent::Fatal { .. });
                    let mut guard = app.lock().await;
                    guard.apply_stage(stage);
                    let _ = tx.send(guard.clone());
                    if done {
                        completed.store(true, Ordering::SeqCst);
                    }
                }
            }
        };

        tokio::join!(pipeline_fut, forward_fut);
    })
}

/// Abort the active run if `id` matches it and it has not completed: cancel
/// its task (killing any in-flight `claude`/`git` child processes) and clean
/// up epic worktrees, mirroring the TUI's abort path. A no-op for an unknown
/// or already-finished id.
pub async fn abort(id: &str) {
    // Take the handle out and release the global lock before the awaits below,
    // so a slow task teardown or worktree cleanup never blocks other start,
    // abort, or subscribe requests.
    let handle = {
        let mut active = ACTIVE.lock().await;
        let matches_active = matches!(
            active.as_ref(),
            Some(handle) if handle.id == id && !handle.completed.load(Ordering::SeqCst)
        );
        if !matches_active {
            return;
        }
        active.take()
    };
    if let Some(handle) = handle {
        handle.task.abort();
        let _ = handle.task.await;
        if let Err(e) = worktree::cleanup_all(&handle.repo).await {
            eprintln!("warning: could not clean up worktrees after abort: {e}");
        }
    }
}

/// The current `App` snapshot and a live receiver of further snapshots for
/// the run identified by `id`, or `None` if it is not the active run.
pub async fn subscribe(id: &str) -> Option<(App, broadcast::Receiver<App>)> {
    let active = ACTIVE.lock().await;
    let handle = active.as_ref()?;
    if handle.id != id {
        return None;
    }
    let snapshot = handle.app.lock().await.clone();
    Some((snapshot, handle.tx.subscribe()))
}
