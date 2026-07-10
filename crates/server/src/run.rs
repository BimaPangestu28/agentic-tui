//! Run manager: a session registry of pipeline runs, one active run per
//! workspace at a time. Starts a run in a spawned task, applies its
//! `StageEvent`s to an `App`, and broadcasts a snapshot of the `App` after
//! each event so any number of WebSocket subscribers can follow the same run.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::{broadcast, mpsc, Mutex};
use tokio::task::JoinHandle;

use shared::{App, EpicStatus, Language, StageEvent};

use crate::workspace::{Repo, Workspace};
use crate::{config, orchestrator, run_pipeline, run_store, workspace, worktree};

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

/// Why `retry` rejected a request: no such run (404), the run is still in
/// flight (409), or the epic is not in a retryable blocked state (400).
#[derive(Debug, Clone, PartialEq)]
pub enum RetryError {
    NotFound,
    RunActive,
    NotBlocked,
}

impl RetryError {
    pub fn message(&self) -> String {
        match self {
            RetryError::NotFound => "no such run".to_string(),
            RetryError::RunActive => {
                "the run is still in flight; wait for it to finish before retrying an epic"
                    .to_string()
            }
            RetryError::NotBlocked => "only a failed or conflicted epic can be retried".to_string(),
        }
    }
}

/// Why `resume` rejected a request: no such run (404), the run is still in
/// flight (409), the run has nothing left to resume (400), or its saved plan
/// could not be read (400).
#[derive(Debug, Clone, PartialEq)]
pub enum ResumeError {
    NotFound,
    RunActive,
    NotResumable,
    NoPlan,
}

impl ResumeError {
    pub fn message(&self) -> String {
        match self {
            ResumeError::NotFound => "no such run".to_string(),
            ResumeError::RunActive => {
                "the run is still in flight; wait for it to finish before resuming".to_string()
            }
            ResumeError::NotResumable => {
                "this run has no unfinished epics to resume".to_string()
            }
            ResumeError::NoPlan => "the saved plan for this run could not be read".to_string(),
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
    repo_names: Vec<String>,
    completed: Arc<AtomicBool>,
    // Context to re-run a single blocked epic after the run finished: where the
    // persisted plan lives, the resolved repos, and the goal/verify the run
    // used. The plan itself is re-read from `.agentic-plan.json` at retry time.
    plan_cwd: PathBuf,
    repos: Arc<HashMap<String, orchestrator::RepoRun>>,
    goal: String,
    default_verify: String,
    language: Language,
}

/// The static context a run needs to write a persisted snapshot: its identity,
/// config, and the repos it targets, in display order. The changing `App` is
/// passed separately to `persist`.
#[derive(Clone)]
struct PersistCtx {
    id: String,
    workspace: String,
    goal: String,
    default_verify: String,
    language: Language,
    plan_cwd: PathBuf,
    repos: Vec<run_store::PersistedRepo>,
    // The run's own plan, when known at ctx-build time. `Some` pins persistence
    // to that exact plan regardless of what the shared `.agentic-plan.json`
    // holds later (see `resume`, which seeds this from the run's saved plan).
    // `None` falls back to reading the shared file at persist time.
    plan_json: Option<String>,
}

/// Build a `PersistCtx` from a run's resolved config. `repo_names` fixes the
/// display order; `repos` supplies each repo's refs.
#[allow(clippy::too_many_arguments)]
fn build_persist_ctx(
    id: &str,
    workspace: &str,
    goal: &str,
    default_verify: &str,
    language: Language,
    plan_cwd: &Path,
    repo_names: &[String],
    repos: &HashMap<String, orchestrator::RepoRun>,
) -> PersistCtx {
    let persisted_repos = repo_names
        .iter()
        .filter_map(|name| {
            repos.get(name).map(|rc| run_store::PersistedRepo {
                name: name.clone(),
                path: rc.path.clone(),
                base_ref: rc.base_ref.clone(),
                integration_branch: rc.integration_branch.clone(),
            })
        })
        .collect();
    PersistCtx {
        id: id.to_string(),
        workspace: workspace.to_string(),
        goal: goal.to_string(),
        default_verify: default_verify.to_string(),
        language,
        plan_cwd: plan_cwd.to_path_buf(),
        repos: persisted_repos,
        // `None` means "read the shared `.agentic-plan.json` at persist time" —
        // correct for a fresh start/retry run, whose shared file still holds
        // its own plan while it is the active run in the workspace, and which
        // has no plan yet at ctx-build time. `resume` overrides this with the
        // run's own saved plan before spawning, since by then the shared file
        // may belong to a later run in the same workspace.
        plan_json: None,
    }
}

/// True for events that change a run's persisted state (lifecycle, cost, and
/// terminal). Streaming log events are skipped so persistence does not hammer
/// the disk on every line.
fn should_persist(ev: &StageEvent) -> bool {
    !matches!(
        ev,
        StageEvent::StageLog { .. }
            | StageEvent::StageAssistant { .. }
            | StageEvent::StageTool { .. }
    )
}

/// Write a snapshot of `app` for `ctx` into the default run store. A no-op
/// while the run is still Planning, so a run interrupted before its plan
/// exists leaves no file to resume.
fn persist(ctx: &PersistCtx, app: &App) {
    persist_to(&run_store::runs_dir(), ctx, app);
}

/// `persist` against an explicit directory, for tests.
fn persist_to(dir: &Path, ctx: &PersistCtx, app: &App) {
    if app.phase == shared::Phase::Planning {
        return;
    }
    // The run's own plan lives at plan_cwd/.agentic-plan.json for the whole
    // active run (the workspace-busy guard blocks a concurrent overwrite) —
    // unless `ctx.plan_json` already pins the run's own plan (set by `resume`,
    // whose shared file may have been overwritten by a later run since).
    let plan_json = match &ctx.plan_json {
        Some(plan_json) => plan_json.clone(),
        None => std::fs::read_to_string(ctx.plan_cwd.join(".agentic-plan.json"))
            .unwrap_or_default(),
    };
    let run = run_store::PersistedRun {
        id: ctx.id.clone(),
        workspace: ctx.workspace.clone(),
        goal: ctx.goal.clone(),
        default_verify: ctx.default_verify.clone(),
        language: ctx.language,
        plan_cwd: ctx.plan_cwd.clone(),
        repos: ctx.repos.clone(),
        plan_json,
        app: app.clone(),
    };
    if let Err(e) = run_store::save(dir, &run) {
        eprintln!("warning: could not persist run {}: {e}", ctx.id);
    }
}

/// Transform a snapshot loaded from disk into an honest post-restart state. A
/// run still mid-flight (`Implementing`) becomes `Failed`, and any epic caught
/// `Running` or `Verifying` becomes `Failed` with a restart reason, so the
/// board never shows a run as active when nothing is driving it. Terminal
/// snapshots (`Done`, or a run that already `Failed`) are left untouched.
fn recover_interrupted(app: &mut App) {
    if app.phase != shared::Phase::Implementing {
        return;
    }
    app.phase = shared::Phase::Failed;
    app.error = Some(
        "Interrupted by a server restart. Resume to continue the unfinished epics.".to_string(),
    );
    for epic in &mut app.epics {
        if matches!(epic.status, EpicStatus::Running | EpicStatus::Verifying) {
            epic.status = EpicStatus::Failed;
            epic.reason = Some("interrupted by a server restart".to_string());
        }
    }
}

/// Derive a git-branch-safe integration branch name from the run's goal, e.g.
/// "Add a /healthz endpoint" -> "agentic/add-a-healthz-endpoint". Lowercases
/// ASCII, turns every run of other characters into a single `-`, trims dashes,
/// and caps the slug so the branch stays readable. An empty or all-punctuation
/// goal falls back to "agentic/run".
fn integration_branch_for(goal: &str) -> String {
    const MAX_SLUG: usize = 40;
    let mut slug = String::with_capacity(MAX_SLUG);
    let mut prev_dash = false;
    for ch in goal.chars() {
        if slug.len() >= MAX_SLUG {
            break;
        }
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !slug.is_empty() {
            slug.push('-');
            prev_dash = true;
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "agentic/run".to_string()
    } else {
        format!("agentic/{slug}")
    }
}

/// The next run id to hand out so a new run never collides with a rehydrated
/// one: one past the largest numeric id on disk, or 1 when the store is empty.
fn next_id_after(runs: &[run_store::PersistedRun]) -> u64 {
    runs.iter()
        .filter_map(|run| run.id.parse::<u64>().ok())
        .max()
        .map(|max| max + 1)
        .unwrap_or(1)
}

/// True when a run can be resumed: it has ended in `Failed` and still has at
/// least one epic that has not merged.
fn resumable(app: &App) -> bool {
    app.phase == shared::Phase::Failed
        && app.epics.iter().any(|epic| epic.status != EpicStatus::Merged)
}

/// Rebuild the in-memory registry from the disk store at startup. Loads every
/// persisted run, recovers interrupted state, inserts a read-only handle
/// (`task: None`, `completed: true`) per run, and advances `NEXT_ID` past the
/// largest rehydrated id. Called once from `serve` before the router accepts
/// requests, so no lock contention with live runs is possible here.
pub async fn rehydrate() {
    let persisted = run_store::load_all(&run_store::runs_dir());
    if persisted.is_empty() {
        return;
    }
    let next_id = next_id_after(&persisted);

    let mut guard = RUNS.lock().await;
    let runs = guard.get_or_insert_with(HashMap::new);
    for persisted_run in persisted {
        let mut app = persisted_run.app;
        recover_interrupted(&mut app);

        let repos: HashMap<String, orchestrator::RepoRun> = persisted_run
            .repos
            .iter()
            .map(|repo| {
                (
                    repo.name.clone(),
                    orchestrator::RepoRun {
                        path: repo.path.clone(),
                        base_ref: repo.base_ref.clone(),
                        integration_branch: repo.integration_branch.clone(),
                    },
                )
            })
            .collect();
        let repo_names: Vec<String> = persisted_run.repos.iter().map(|r| r.name.clone()).collect();
        let repo_paths: Vec<PathBuf> = persisted_run.repos.iter().map(|r| r.path.clone()).collect();

        let (tx, _rx) = broadcast::channel::<App>(SNAPSHOT_CHANNEL_CAPACITY);
        let handle = RunHandle {
            id: persisted_run.id.clone(),
            workspace: persisted_run.workspace,
            app: Arc::new(Mutex::new(app)),
            tx,
            task: None,
            repo_paths,
            repo_names,
            completed: Arc::new(AtomicBool::new(true)),
            plan_cwd: persisted_run.plan_cwd,
            repos: Arc::new(repos),
            goal: persisted_run.goal,
            default_verify: persisted_run.default_verify,
            language: persisted_run.language,
        };
        runs.insert(persisted_run.id, handle);
    }
    NEXT_ID.store(next_id, Ordering::SeqCst);
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
    let verify_cmd = req
        .verify
        .clone()
        .unwrap_or_else(|| config::DEFAULT_VERIFY_CMD.to_string());

    // Resolve every repo up front (expanding `~` and canonicalizing its path),
    // exactly as the single-repo path did before this reshape.
    let workspace = Workspace {
        name: workspace_name.clone(),
        repos: req
            .workspace
            .repos
            .iter()
            .map(|repo| {
                let path = workspace::expand_tilde(&repo.path);
                let path = path.canonicalize().unwrap_or(path);
                Repo {
                    name: repo.name.clone(),
                    path,
                    base: repo.base.clone(),
                    integration: repo.integration.clone(),
                }
            })
            .collect(),
    };
    if workspace.repos.is_empty() {
        return Err(StartError::Invalid(
            "a run needs at least one repository".to_string(),
        ));
    }

    // Fail-fast gates, run PER repo before touching the registry so an invalid
    // request never contends with a real run. Each error names the offending
    // repo. The resolved `RepoRun` map is keyed by repo name, as the plan's
    // epics tag their repo by name.
    let mut repos: HashMap<String, orchestrator::RepoRun> = HashMap::new();
    for repo in &workspace.repos {
        let base_ref = repo.base.clone().unwrap_or_else(|| "HEAD".to_string());
        // The integration branch is derived from the goal by default (the
        // new-run form no longer asks for one). A `~/.config` workspace entry
        // may still pin an explicit integration branch as an override.
        let integration = repo
            .integration
            .clone()
            .unwrap_or_else(|| integration_branch_for(&req.goal));

        worktree::verify_ref(&repo.path, &base_ref)
            .await
            .map_err(|e| StartError::Invalid(format!("repo '{}': {e}", repo.name)))?;
        if integration.trim().is_empty() {
            return Err(StartError::Invalid(format!(
                "repo '{}': the integration branch requires a name",
                repo.name
            )));
        }
        let current_branch = worktree::current_branch(&repo.path)
            .await
            .map_err(|e| StartError::Invalid(format!("repo '{}': {e}", repo.name)))?;
        if current_branch.as_deref() == Some(integration.as_str()) {
            return Err(StartError::Invalid(format!(
                "repo '{}': cannot merge into '{integration}': it is checked out in the workspace. Check out a different branch first, or choose another integration target.",
                repo.name
            )));
        }

        repos.insert(
            repo.name.clone(),
            orchestrator::RepoRun {
                path: repo.path.clone(),
                base_ref,
                integration_branch: integration,
            },
        );
    }

    let repo_paths: Vec<PathBuf> = workspace.repos.iter().map(|r| r.path.clone()).collect();
    let repo_names: Vec<String> = workspace.repos.iter().map(|r| r.name.clone()).collect();
    // Planning runs at the shared parent of every repo so the plan stage can
    // see all of them. For a one-repo group this is the repo's parent.
    let plan_cwd = workspace::common_root(&workspace);

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
    )));
    let (tx, _rx) = broadcast::channel::<App>(SNAPSHOT_CHANNEL_CAPACITY);
    let completed = Arc::new(AtomicBool::new(false));

    // Keep clones for the retry context before `spawn_pipeline` consumes the
    // originals. `repos` is shared behind an `Arc` so the retry path reads the
    // same resolved repos the run used.
    let repos = Arc::new(repos);
    let goal_for_retry = req.goal.clone();
    let verify_for_retry = verify_cmd.clone();
    let plan_cwd_for_retry = plan_cwd.clone();
    let language = req.language;

    let persist_ctx = build_persist_ctx(
        &id,
        &workspace_name,
        &goal_for_retry,
        &verify_for_retry,
        language,
        &plan_cwd_for_retry,
        &repo_names,
        &repos,
    );

    let task = spawn_pipeline(
        app.clone(),
        tx.clone(),
        completed.clone(),
        plan_cwd,
        (*repos).clone(),
        req.goal,
        verify_cmd,
        language,
        req.refine_cost,
        persist_ctx,
    );

    runs.insert(
        id.clone(),
        RunHandle {
            id: id.clone(),
            workspace: workspace_name,
            app,
            tx,
            task: Some(task),
            repo_paths,
            repo_names,
            completed,
            plan_cwd: plan_cwd_for_retry,
            repos,
            goal: goal_for_retry,
            default_verify: verify_for_retry,
            language,
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
    language: Language,
    refine_cost: f64,
    persist_ctx: PersistCtx,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let (pipeline_tx, mut rx) = mpsc::unbounded_channel::<StageEvent>();

        let pipeline_fut = async move {
            if let Err(e) = run_pipeline(
                &plan_cwd,
                repos,
                &goal,
                &default_verify,
                language,
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
                let persist_this = should_persist(&stage);
                let mut guard = app.lock().await;
                guard.apply_stage(stage);
                let _ = tx.send(guard.clone());
                if persist_this {
                    persist(&persist_ctx, &guard);
                }
                if done {
                    completed.store(true, Ordering::SeqCst);
                }
            }
        };

        tokio::join!(pipeline_fut, forward_fut);
    })
}

/// Re-run a single blocked epic of a finished run. Rejects with `NotFound` for
/// an unknown run, `RunActive` while the run is still in flight, or
/// `NotBlocked` if the epic is not currently Failed or Conflict. On success it
/// flips the run back to active (so no new run starts on the workspace and
/// `abort` can tear the retry down) and spawns a task that re-runs just that
/// epic, applying its events to the same `App` all subscribers already follow.
pub async fn retry(run_id: &str, epic_id: &str) -> Result<(), RetryError> {
    // Everything happens under one RUNS lock so validating, claiming the run,
    // and storing the retry task are atomic: no start/abort/retry can race in
    // the window between them. The only await inside is a brief `app` lock,
    // which no path holds while waiting on RUNS, so it cannot deadlock.
    let mut guard = RUNS.lock().await;
    let runs = guard.get_or_insert_with(HashMap::new);
    let handle = runs.get_mut(run_id).ok_or(RetryError::NotFound)?;
    if !handle.completed.load(Ordering::SeqCst) {
        return Err(RetryError::RunActive);
    }

    // The epic must be blocked with work of its own to redo. A Skipped epic
    // only waits on a failed dependency, so retrying it directly is a no-op;
    // the user retries the dependency instead.
    let initial_cost = {
        let app = handle.app.lock().await;
        let epic = app
            .epics
            .iter()
            .find(|candidate| candidate.id == epic_id)
            .ok_or(RetryError::NotBlocked)?;
        if !matches!(epic.status, EpicStatus::Failed | EpicStatus::Conflict) {
            return Err(RetryError::NotBlocked);
        }
        app.total_cost
    };

    handle.completed.store(false, Ordering::SeqCst);
    let persist_ctx = build_persist_ctx(
        &handle.id,
        &handle.workspace,
        &handle.goal,
        &handle.default_verify,
        handle.language,
        &handle.plan_cwd,
        &handle.repo_names,
        &handle.repos,
    );
    let task = spawn_retry(
        handle.app.clone(),
        handle.tx.clone(),
        handle.completed.clone(),
        handle.plan_cwd.clone(),
        handle.repos.clone(),
        handle.goal.clone(),
        handle.default_verify.clone(),
        handle.language,
        epic_id.to_string(),
        initial_cost,
        persist_ctx,
    );
    handle.task = Some(task);
    Ok(())
}

/// Spawn one task that re-reads the persisted plan and re-runs a single epic,
/// forwarding its events into the run's `App` and broadcasting each snapshot,
/// then marks the run completed when the epic's event channel closes. Mirrors
/// `spawn_pipeline` so abort tears it down the same way.
#[allow(clippy::too_many_arguments)]
fn spawn_retry(
    app: Arc<Mutex<App>>,
    tx: broadcast::Sender<App>,
    completed: Arc<AtomicBool>,
    plan_cwd: PathBuf,
    repos: Arc<HashMap<String, orchestrator::RepoRun>>,
    goal: String,
    default_verify: String,
    language: Language,
    epic_id: String,
    initial_cost: f64,
    persist_ctx: PersistCtx,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let (pipeline_tx, mut rx) = mpsc::unbounded_channel::<StageEvent>();

        let pipeline_fut = async move {
            let plan_path = plan_cwd.join(".agentic-plan.json");
            let loaded = std::fs::read_to_string(&plan_path)
                .map_err(|e| anyhow::anyhow!("could not read the saved plan: {e}"))
                .and_then(|text| crate::plan::parse_plan(&text));
            match loaded {
                Ok(plan) => {
                    if let Err(e) = orchestrator::retry_epic(
                        &plan,
                        &epic_id,
                        &repos,
                        &goal,
                        &default_verify,
                        language,
                        initial_cost,
                        pipeline_tx.clone(),
                    )
                    .await
                    {
                        let _ = pipeline_tx.send(StageEvent::StageLog {
                            tag: epic_id.clone(),
                            line: format!("retry failed: {e}"),
                        });
                    }
                }
                Err(e) => {
                    let _ = pipeline_tx.send(StageEvent::StageLog {
                        tag: epic_id.clone(),
                        line: format!("retry failed: {e}"),
                    });
                }
            }
            // `pipeline_tx` and its clone drop here, closing the channel.
        };

        let forward_fut = async {
            while let Some(stage) = rx.recv().await {
                let persist_this = should_persist(&stage);
                let mut app = app.lock().await;
                app.apply_stage(stage);
                let _ = tx.send(app.clone());
                if persist_this {
                    persist(&persist_ctx, &app);
                }
            }
        };

        tokio::join!(pipeline_fut, forward_fut);
        completed.store(true, Ordering::SeqCst);
    })
}

/// Resume a finished-but-unfinished run: re-run every epic that has not merged,
/// seeding the already-merged epics so the scheduler skips them. Reads the
/// run's own saved plan from the disk store (not the shared `.agentic-plan.json`,
/// which a later run may have overwritten). Cleans up any leftover worktrees
/// first so `worktree::create` does not trip over a stale branch. Flips the run
/// back to active so no new run starts on the workspace and `abort` can tear the
/// resume down.
pub async fn resume(run_id: &str) -> Result<(), ResumeError> {
    let persisted = run_store::load_all(&run_store::runs_dir());
    let saved = persisted
        .into_iter()
        .find(|run| run.id == run_id)
        .ok_or(ResumeError::NotFound)?;
    let plan = crate::plan::parse_plan(&saved.plan_json).map_err(|_| ResumeError::NoPlan)?;

    let mut guard = RUNS.lock().await;
    let runs = guard.get_or_insert_with(HashMap::new);
    let handle = runs.get_mut(run_id).ok_or(ResumeError::NotFound)?;
    if !handle.completed.load(Ordering::SeqCst) {
        return Err(ResumeError::RunActive);
    }
    let (seed_merged, initial_cost) = {
        let app = handle.app.lock().await;
        if !resumable(&app) {
            return Err(ResumeError::NotResumable);
        }
        let seed: Vec<String> = app
            .epics
            .iter()
            .filter(|epic| epic.status == EpicStatus::Merged)
            .map(|epic| epic.id.clone())
            .collect();
        (seed, app.total_cost)
    };

    handle.completed.store(false, Ordering::SeqCst);
    let mut persist_ctx = build_persist_ctx(
        &handle.id,
        &handle.workspace,
        &handle.goal,
        &handle.default_verify,
        handle.language,
        &handle.plan_cwd,
        &handle.repo_names,
        &handle.repos,
    );
    // Pin persistence to the plan that actually drives this resume (the run's
    // own saved plan), not whatever the shared `.agentic-plan.json` holds now
    // — a later run in the same workspace may have overwritten it since.
    persist_ctx.plan_json = Some(saved.plan_json.clone());
    let task = spawn_resume(
        handle.app.clone(),
        handle.tx.clone(),
        handle.completed.clone(),
        handle.repos.clone(),
        handle.goal.clone(),
        handle.default_verify.clone(),
        handle.language,
        plan,
        seed_merged,
        initial_cost,
        handle.repo_paths.clone(),
        persist_ctx,
    );
    handle.task = Some(task);
    Ok(())
}

/// Spawn a task that cleans up leftover worktrees, then drives
/// `orchestrator::run_resume` over the saved plan, forwarding events into the
/// run's `App` and persisting on qualifying events. Marks the run completed
/// when the event channel closes. Mirrors `spawn_pipeline` so `abort` tears it
/// down the same way.
#[allow(clippy::too_many_arguments)]
fn spawn_resume(
    app: Arc<Mutex<App>>,
    tx: broadcast::Sender<App>,
    completed: Arc<AtomicBool>,
    repos: Arc<HashMap<String, orchestrator::RepoRun>>,
    goal: String,
    default_verify: String,
    language: Language,
    plan: crate::plan::Plan,
    seed_merged: Vec<String>,
    initial_cost: f64,
    repo_paths: Vec<PathBuf>,
    persist_ctx: PersistCtx,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        // Clear any worktrees a killed run left behind so create() does not
        // fail on a stale agentic/<id> branch. Merged work is safe on the
        // integration branch; conflict worktrees are re-run from scratch.
        for repo in &repo_paths {
            if let Err(e) = worktree::cleanup_all(repo).await {
                eprintln!(
                    "warning: could not clean up worktrees for {}: {e}",
                    repo.display()
                );
            }
        }

        let (pipeline_tx, mut rx) = mpsc::unbounded_channel::<StageEvent>();
        let config = orchestrator::RunConfig {
            repos: (*repos).clone(),
            goal,
            default_verify,
            initial_cost,
            language,
        };

        let pipeline_fut = async move {
            if let Err(e) =
                orchestrator::run_resume(&plan, config, &seed_merged, pipeline_tx.clone()).await
            {
                let _ = pipeline_tx.send(StageEvent::Fatal {
                    reason: e.to_string(),
                });
            }
        };

        let forward_fut = async {
            while let Some(stage) = rx.recv().await {
                let persist_this = should_persist(&stage);
                let mut app = app.lock().await;
                app.apply_stage(stage);
                let _ = tx.send(app.clone());
                if persist_this {
                    persist(&persist_ctx, &app);
                }
            }
        };

        tokio::join!(pipeline_fut, forward_fut);
        completed.store(true, Ordering::SeqCst);
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
                let persist_ctx = build_persist_ctx(
                    &handle.id,
                    &handle.workspace,
                    &handle.goal,
                    &handle.default_verify,
                    handle.language,
                    &handle.plan_cwd,
                    &handle.repo_names,
                    &handle.repos,
                );
                Some((
                    handle.task.take(),
                    handle.app.clone(),
                    handle.tx.clone(),
                    handle.repo_paths.clone(),
                    persist_ctx,
                ))
            }
            _ => None,
        }
    };
    let Some((task, app, tx, repo_paths, persist_ctx)) = target else {
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
        persist(&persist_ctx, &app);
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
/// the names of the repos it targets, and a handle to its live `App`.
type RunSnapshot = (String, String, Vec<String>, Arc<Mutex<App>>);

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
                    h.repo_names.clone(),
                    h.app.clone(),
                )
            })
            .collect()
    };
    // Phase 2: build summaries by locking each app briefly, RUNS already
    // released.
    let mut out = Vec::with_capacity(handles.len());
    for (id, workspace, repos, app) in handles {
        let app = app.lock().await;
        // The run's repos come from its config, not the epics: every repo the
        // workspace group targets.
        out.push(shared::RunSummary {
            id,
            workspace,
            goal: app.goal.clone(),
            phase: app.phase,
            total_cost: app.total_cost,
            epics: app.epics.clone(),
            repos,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::{EpicView, Phase};

    #[test]
    fn should_persist_ignores_streaming_events_only() {
        assert!(!should_persist(&StageEvent::StageLog {
            tag: "plan".into(),
            line: "hi".into(),
        }));
        assert!(!should_persist(&StageEvent::StageAssistant {
            tag: "plan".into(),
            text: "hi".into(),
        }));
        assert!(!should_persist(&StageEvent::StageTool {
            tag: "plan".into(),
            name: "Read".into(),
            input: String::new(),
        }));
        assert!(should_persist(&StageEvent::Cost { total: 1.0 }));
        assert!(should_persist(&StageEvent::Done));
        assert!(should_persist(&StageEvent::EpicMerged { id: "a".into() }));
    }

    #[test]
    fn persist_writes_a_snapshot_once_past_planning() {
        let dir = std::env::temp_dir().join(format!("persist-hook-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let plan_cwd = dir.join("cwd");
        std::fs::create_dir_all(&plan_cwd).unwrap();
        std::fs::write(plan_cwd.join(".agentic-plan.json"), r#"{"epics":[]}"#).unwrap();

        let ctx = PersistCtx {
            id: "7".to_string(),
            workspace: "greentic".to_string(),
            goal: "g".to_string(),
            default_verify: "make verify".to_string(),
            language: Language::English,
            plan_cwd: plan_cwd.clone(),
            repos: vec![],
            plan_json: None,
        };
        let runs = dir.join("runs");

        // Planning phase writes nothing.
        let mut app = App::new("g".to_string(), "greentic".to_string());
        persist_to(&runs, &ctx, &app);
        assert!(run_store::load_all(&runs).is_empty());

        // Implementing phase writes a snapshot carrying the plan JSON.
        app.phase = Phase::Implementing;
        persist_to(&runs, &ctx, &app);
        let loaded = run_store::load_all(&runs);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "7");
        assert_eq!(loaded[0].plan_json, r#"{"epics":[]}"#);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Guards the resume-persist defect: once `PersistCtx.plan_json` pins the
    /// run's own plan (as `resume` does), persisting must write that plan even
    /// when the shared `.agentic-plan.json` now holds a different, later run's
    /// plan (e.g. run 2 re-planned in the same workspace after run 1 failed).
    #[test]
    fn persist_prefers_ctx_plan_over_a_since_overwritten_shared_file() {
        let dir = std::env::temp_dir().join(format!("persist-resume-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let plan_cwd = dir.join("cwd");
        std::fs::create_dir_all(&plan_cwd).unwrap();

        let plan_1 = r#"{"epics":[{"id":"a","title":"A","repo":"r","depends_on":[]}]}"#;
        let plan_2 = r#"{"epics":[{"id":"z","title":"Z","repo":"r","depends_on":[]}]}"#;
        // Simulate a later run having overwritten the shared plan file since
        // this (resumed) run's own plan was saved.
        std::fs::write(plan_cwd.join(".agentic-plan.json"), plan_2).unwrap();

        let ctx = PersistCtx {
            id: "1".to_string(),
            workspace: "greentic".to_string(),
            goal: "g".to_string(),
            default_verify: "make verify".to_string(),
            language: Language::English,
            plan_cwd: plan_cwd.clone(),
            repos: vec![],
            plan_json: Some(plan_1.to_string()),
        };
        let runs = dir.join("runs");

        let mut app = App::new("g".to_string(), "greentic".to_string());
        app.phase = Phase::Implementing;
        persist_to(&runs, &ctx, &app);

        let loaded = run_store::load_all(&runs);
        assert_eq!(loaded.len(), 1);
        assert_eq!(
            loaded[0].plan_json, plan_1,
            "the ctx's own plan must win over the shared file"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recover_interrupted_fails_a_mid_flight_run() {
        let mut app = App::new("g".to_string(), "ws".to_string());
        app.phase = Phase::Implementing;
        app.epics = vec![
            EpicView {
                id: "a".into(),
                title: "A".into(),
                status: EpicStatus::Running,
                cost: 0.0,
                repo: "r".into(),
                depends_on: vec![],
                reason: None,
            },
            EpicView {
                id: "b".into(),
                title: "B".into(),
                status: EpicStatus::Merged,
                cost: 0.1,
                repo: "r".into(),
                depends_on: vec![],
                reason: None,
            },
            EpicView {
                id: "c".into(),
                title: "C".into(),
                status: EpicStatus::Pending,
                cost: 0.0,
                repo: "r".into(),
                depends_on: vec![],
                reason: None,
            },
        ];

        recover_interrupted(&mut app);

        assert_eq!(app.phase, Phase::Failed);
        assert!(app.error.is_some());
        assert_eq!(app.epics[0].status, EpicStatus::Failed, "Running becomes Failed");
        assert!(app.epics[0].reason.is_some());
        assert_eq!(app.epics[1].status, EpicStatus::Merged, "Merged is kept");
        assert_eq!(app.epics[2].status, EpicStatus::Pending, "Pending is kept");
    }

    #[test]
    fn recover_interrupted_leaves_a_finished_run_alone() {
        let mut app = App::new("g".to_string(), "ws".to_string());
        app.phase = Phase::Done;
        app.epics = vec![EpicView {
            id: "a".into(),
            title: "A".into(),
            status: EpicStatus::Merged,
            cost: 0.1,
            repo: "r".into(),
            depends_on: vec![],
            reason: None,
        }];
        recover_interrupted(&mut app);
        assert_eq!(app.phase, Phase::Done);
        assert!(app.error.is_none());
        assert_eq!(app.epics[0].status, EpicStatus::Merged);
    }

    #[test]
    fn next_id_after_is_one_past_the_max_numeric_id() {
        let dir = std::env::temp_dir().join(format!("next-id-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for id in ["2", "5", "3"] {
            let mut app = App::new("g".to_string(), "ws".to_string());
            app.phase = Phase::Done;
            let run = run_store::PersistedRun {
                id: id.to_string(),
                workspace: "ws".to_string(),
                goal: "g".to_string(),
                default_verify: "make verify".to_string(),
                language: Language::English,
                plan_cwd: std::path::PathBuf::from("/tmp"),
                repos: vec![],
                plan_json: r#"{"epics":[]}"#.to_string(),
                app,
            };
            run_store::save(&dir, &run).unwrap();
        }
        let runs = run_store::load_all(&dir);
        assert_eq!(next_id_after(&runs), 6);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn next_id_after_starts_at_one_when_empty() {
        assert_eq!(next_id_after(&[]), 1);
    }

    #[test]
    fn integration_branch_for_slugs_the_goal() {
        assert_eq!(
            integration_branch_for("Add a /healthz endpoint"),
            "agentic/add-a-healthz-endpoint"
        );
        assert_eq!(integration_branch_for("do nothing"), "agentic/do-nothing");
        // Leading/trailing/adjacent punctuation collapses to single dashes and
        // never leaves a leading or trailing dash.
        assert_eq!(
            integration_branch_for("  --Fix!! the   bug.  "),
            "agentic/fix-the-bug"
        );
        // Non-ASCII becomes a separator, keeping the ref ASCII-safe.
        assert_eq!(integration_branch_for("Café münchen"), "agentic/caf-m-nchen");
        // An empty or all-punctuation goal still yields a valid branch.
        assert_eq!(integration_branch_for(""), "agentic/run");
        assert_eq!(integration_branch_for("!!! ???"), "agentic/run");
    }

    #[test]
    fn integration_branch_for_caps_the_slug_length() {
        let long = "a".repeat(200);
        let branch = integration_branch_for(&long);
        // "agentic/" + at most 40 slug chars.
        assert!(branch.starts_with("agentic/"));
        assert!(branch.len() <= "agentic/".len() + 40, "got {branch}");
    }

    #[test]
    fn resumable_needs_a_failed_run_with_unfinished_work() {
        let mut app = App::new("g".to_string(), "ws".to_string());
        app.epics = vec![EpicView {
            id: "a".into(),
            title: "A".into(),
            status: EpicStatus::Failed,
            cost: 0.0,
            repo: "r".into(),
            depends_on: vec![],
            reason: None,
        }];

        app.phase = Phase::Implementing;
        assert!(!resumable(&app), "a running run is not resumable");

        app.phase = Phase::Failed;
        assert!(resumable(&app), "a failed run with a non-merged epic is resumable");

        app.epics[0].status = EpicStatus::Merged;
        assert!(!resumable(&app), "nothing left to resume when all epics merged");
    }

    #[test]
    fn resume_error_messages_are_distinct() {
        assert_ne!(ResumeError::NotFound.message(), ResumeError::RunActive.message());
        assert_ne!(ResumeError::NotResumable.message(), ResumeError::NoPlan.message());
    }
}
