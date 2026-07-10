# Run Persistence and Resume Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist every run to disk so runs survive a server restart as history, and let the user manually resume a run interrupted mid-flight.

**Architecture:** Each run is written to `~/.config/agentic-tui/runs/<id>.json` on every lifecycle event (not on streaming log lines, and never while still Planning). On startup the server rehydrates these files into the in-memory `RUNS` registry, marking interrupted runs as Failed. A "Resume run" button re-invokes the orchestrator with the run's stored plan, seeding already-merged epics as complete so the existing DAG scheduler re-runs only the unfinished ones.

**Tech Stack:** Rust, tokio, axum, serde/serde_json, anyhow, Leptos (wasm web UI).

## Global Constraints

- Run store lives at `~/.config/agentic-tui/runs/`, one `<id>.json` file per run, same config dir as `workspace::default_config_path()`.
- Persistence writes only when `app.phase != Planning` (a run interrupted during planning leaves no file).
- Persistence fires only on non-streaming events (never on `StageLog` / `StageAssistant` / `StageTool`).
- Writes are atomic: serialize to `<id>.json.tmp`, then rename to `<id>.json`.
- Resume is manual and per-run; nothing resumes automatically on startup.
- Prose in comments follows the repo style: direct, no em dashes, no contractions in English prose.
- Every serde wire/persist type gets a JSON round-trip test, matching the existing test style in `crates/shared/src/lib.rs` and `crates/server/src/plan.rs`.

---

## File Structure

- `crates/server/src/plan.rs` — add `Serialize` to `Task`, `Epic`, `Plan`.
- `crates/server/src/orchestrator.rs` — add `Serialize`/`Deserialize` to `RepoRun`; extract the scheduler driver into `drive`; add `run_resume`.
- `crates/server/src/run_store.rs` — NEW. Owns disk I/O for persisted runs: `PersistedRun`, `PersistedRepo`, `runs_dir`, `save`, `load_all`, `delete`.
- `crates/server/src/run.rs` — persistence hook (`PersistCtx`, `should_persist`, `persist`), `rehydrate`, `resume`, `ResumeError`, `spawn_resume`.
- `crates/server/src/lib.rs` — declare `pub mod run_store;`.
- `crates/server/src/http.rs` — `POST /api/runs/{id}/resume` route + handler; call `run::rehydrate()` in `serve()`.
- `crates/server/tests/run_resume.rs` — NEW. End-to-end `run_resume` test with a fake `claude`.
- `crates/web/src/api.rs` — `resume_run(id)` client.
- `crates/web/src/views/run.rs` — "Resume run" button + interrupted banner.

---

## Task 1: Serde derives on plan and RepoRun types

**Files:**
- Modify: `crates/server/src/plan.rs:5`, `:7`, `:15`, `:31`
- Modify: `crates/server/src/orchestrator.rs:158`

**Interfaces:**
- Produces: `plan::{Task, Epic, Plan}` implement `serde::Serialize` + `serde::Deserialize`; `orchestrator::RepoRun` implements `serde::{Serialize, Deserialize}` + `Clone`.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/server/src/plan.rs` (after the last test, before the closing `}`):

```rust
    #[test]
    fn plan_round_trips_through_json() {
        let plan = parse_plan(plan_json()).unwrap();
        let json = serde_json::to_string(&plan).expect("Plan must serialize");
        let back: Plan = serde_json::from_str(&json).expect("Plan must deserialize");
        assert_eq!(plan, back);
    }
```

Add to the `tests` module in `crates/server/src/orchestrator.rs`:

```rust
    #[test]
    fn repo_run_round_trips_through_json() {
        let rc = RepoRun {
            path: std::path::PathBuf::from("/tmp/x"),
            base_ref: "main".to_string(),
            integration_branch: "agentic-integration".to_string(),
        };
        let json = serde_json::to_string(&rc).expect("RepoRun must serialize");
        let back: RepoRun = serde_json::from_str(&json).expect("RepoRun must deserialize");
        assert_eq!(back.path, rc.path);
        assert_eq!(back.base_ref, rc.base_ref);
        assert_eq!(back.integration_branch, rc.integration_branch);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p agentic-tui --lib plan_round_trips repo_run_round_trips 2>&1 | tail -20`
Expected: FAIL — `Plan`/`RepoRun` do not implement `Serialize`.

- [ ] **Step 3: Add the derives**

In `crates/server/src/plan.rs`, change the import on line 5 from:

```rust
use serde::Deserialize;
```
to:
```rust
use serde::{Deserialize, Serialize};
```

Then change each of the three derive lines (`Task` at :7, `Epic` at :15, `Plan` at :31) from:

```rust
#[derive(Debug, Clone, PartialEq, Deserialize)]
```
to:
```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
```

In `crates/server/src/orchestrator.rs`, change the `RepoRun` derive at line 158 from:

```rust
#[derive(Clone)]
pub struct RepoRun {
```
to:
```rust
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct RepoRun {
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p agentic-tui --lib plan_round_trips repo_run_round_trips 2>&1 | tail -20`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/plan.rs crates/server/src/orchestrator.rs
git commit -m "feat: make plan and RepoRun types serializable"
```

---

## Task 2: The run_store module

**Files:**
- Create: `crates/server/src/run_store.rs`
- Modify: `crates/server/src/lib.rs:9-18` (add `pub mod run_store;`)

**Interfaces:**
- Consumes: `plan::Plan` (Task 1), `orchestrator::RepoRun` (Task 1), `shared::App`.
- Produces:
  - `run_store::PersistedRepo { name: String, path: PathBuf, base_ref: String, integration_branch: String }` — `Clone, Serialize, Deserialize`.
  - `run_store::PersistedRun { id: String, workspace: String, goal: String, default_verify: String, plan_cwd: PathBuf, repos: Vec<PersistedRepo>, plan_json: String, app: shared::App }` — `Clone, Serialize, Deserialize`.
  - `run_store::runs_dir() -> PathBuf`
  - `run_store::save(dir: &Path, run: &PersistedRun) -> anyhow::Result<()>`
  - `run_store::load_all(dir: &Path) -> Vec<PersistedRun>`
  - `run_store::delete(dir: &Path, id: &str) -> anyhow::Result<()>`

- [ ] **Step 1: Create the module with its full contents**

Create `crates/server/src/run_store.rs`:

```rust
//! On-disk store for runs, so the run registry survives a server restart.
//!
//! Each run is one JSON file at `~/.config/agentic-tui/runs/<id>.json`, in the
//! same config directory as the workspace registry. The run manager writes a
//! snapshot on every lifecycle event and reads them all back at startup. The
//! functions take an explicit `dir` so tests can point at a temporary
//! directory instead of the user's real config.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use shared::App;

/// One repository a run targeted, in the order the run listed it. Rebuilds the
/// `orchestrator::RepoRun` map, the ordered repo names, and the repo paths a
/// live run needs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PersistedRepo {
    pub name: String,
    pub path: PathBuf,
    pub base_ref: String,
    pub integration_branch: String,
}

/// Everything needed to show a run as history and to resume it: its identity
/// and config, its own copy of the plan JSON, and the last `App` snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedRun {
    pub id: String,
    pub workspace: String,
    pub goal: String,
    pub default_verify: String,
    pub plan_cwd: PathBuf,
    pub repos: Vec<PersistedRepo>,
    /// This run's own copy of `.agentic-plan.json`, so resume never depends on
    /// the shared plan file at the workspace root, which a later run overwrites.
    pub plan_json: String,
    pub app: App,
}

/// Default location of the run store: `~/.config/agentic-tui/runs/`.
pub fn runs_dir() -> PathBuf {
    let base = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join(".config").join("agentic-tui").join("runs")
}

/// Persist one run to `<dir>/<id>.json`. Writes to a `.tmp` sibling first and
/// renames, so a crash mid-write never leaves a torn file. Creates `dir` if it
/// does not exist.
pub fn save(dir: &Path, run: &PersistedRun) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|e| anyhow::anyhow!("could not create {}: {e}", dir.display()))?;
    let json = serde_json::to_string_pretty(run)?;
    let final_path = dir.join(format!("{}.json", run.id));
    let tmp_path = dir.join(format!("{}.json.tmp", run.id));
    std::fs::write(&tmp_path, json)
        .map_err(|e| anyhow::anyhow!("could not write {}: {e}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, &final_path)
        .map_err(|e| anyhow::anyhow!("could not rename into {}: {e}", final_path.display()))?;
    Ok(())
}

/// Load every run in `dir`, skipping (with a warning) any file that does not
/// parse, so one corrupt file never blocks startup. Ignores `.tmp` leftovers
/// and anything that is not a `.json` file. Returns an empty vec if `dir` does
/// not exist.
pub fn load_all(dir: &Path) -> Vec<PersistedRun> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut runs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<PersistedRun>(&text) {
                Ok(run) => runs.push(run),
                Err(e) => eprintln!("warning: skipping unreadable run {}: {e}", path.display()),
            },
            Err(e) => eprintln!("warning: could not read run {}: {e}", path.display()),
        }
    }
    runs
}

/// Remove a run's file. A missing file is not an error.
pub fn delete(dir: &Path, id: &str) -> anyhow::Result<()> {
    let path = dir.join(format!("{id}.json"));
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow::anyhow!("could not delete {}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::{EpicStatus, EpicView, Phase};

    fn sample_run(id: &str) -> PersistedRun {
        let mut app = App::new("add a health check".to_string(), "greentic".to_string());
        app.phase = Phase::Implementing;
        app.total_cost = 0.42;
        app.epics = vec![EpicView {
            id: "epic-1".to_string(),
            title: "First".to_string(),
            status: EpicStatus::Merged,
            cost: 0.2,
            repo: "greentic".to_string(),
            depends_on: vec![],
            reason: None,
        }];
        PersistedRun {
            id: id.to_string(),
            workspace: "greentic".to_string(),
            goal: "add a health check".to_string(),
            default_verify: "make verify".to_string(),
            plan_cwd: PathBuf::from("/tmp/greentic"),
            repos: vec![PersistedRepo {
                name: "greentic".to_string(),
                path: PathBuf::from("/tmp/greentic"),
                base_ref: "main".to_string(),
                integration_branch: "agentic-integration".to_string(),
            }],
            plan_json: r#"{"epics":[]}"#.to_string(),
            app,
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("run-store-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn save_then_load_round_trips_a_run() {
        let dir = temp_dir("round-trip");
        let run = sample_run("1");
        save(&dir, &run).unwrap();
        let loaded = load_all(&dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "1");
        assert_eq!(loaded[0].workspace, "greentic");
        assert_eq!(loaded[0].app.total_cost, 0.42);
        assert_eq!(loaded[0].repos, run.repos);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_all_skips_a_corrupt_file() {
        let dir = temp_dir("corrupt");
        save(&dir, &sample_run("1")).unwrap();
        std::fs::write(dir.join("2.json"), "{ not valid json").unwrap();
        let loaded = load_all(&dir);
        assert_eq!(loaded.len(), 1, "the good run survives a bad neighbor");
        assert_eq!(loaded[0].id, "1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_leaves_no_tmp_file_behind() {
        let dir = temp_dir("no-tmp");
        save(&dir, &sample_run("1")).unwrap();
        assert!(dir.join("1.json").exists());
        assert!(!dir.join("1.json.tmp").exists(), "the tmp file is renamed away");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_removes_a_run_and_ignores_a_missing_one() {
        let dir = temp_dir("delete");
        save(&dir, &sample_run("1")).unwrap();
        delete(&dir, "1").unwrap();
        assert!(load_all(&dir).is_empty());
        delete(&dir, "1").expect("deleting a missing run is not an error");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_all_on_a_missing_dir_is_empty() {
        let dir = temp_dir("missing");
        assert!(load_all(&dir).is_empty());
    }
}
```

- [ ] **Step 2: Declare the module**

In `crates/server/src/lib.rs`, add `pub mod run_store;` to the module list (keep it alphabetical, after `pub mod run;` on line 16):

```rust
pub mod run;
pub mod run_store;
pub mod workspace;
```

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cargo test -p agentic-tui --lib run_store 2>&1 | tail -20`
Expected: PASS (5 tests).

- [ ] **Step 4: Commit**

```bash
git add crates/server/src/run_store.rs crates/server/src/lib.rs
git commit -m "feat: add a disk store for persisted runs"
```

---

## Task 3: orchestrator::run_resume

**Files:**
- Modify: `crates/server/src/orchestrator.rs:257-419` (extract `drive`, add `run_resume`)
- Create: `crates/server/tests/run_resume.rs`

**Interfaces:**
- Consumes: existing `Scheduler`, `RunConfig`, `run_epic`, `epic_base_ref`.
- Produces: `orchestrator::run_resume(plan: &Plan, config: RunConfig, seed_merged: &[String], tx: UnboundedSender<StageEvent>) -> anyhow::Result<()>`. Epics whose ids are in `seed_merged` are pre-marked succeeded, so the scheduler skips them and their dependents inherit the already-merged work.

- [ ] **Step 1: Write the failing end-to-end test**

Create `crates/server/tests/run_resume.rs`:

```rust
//! `run_resume` skips epics that are seeded as already merged and runs only the
//! rest. Uses a fake `claude` on PATH, mirroring `tests/multi_repo.rs`.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use agentic_tui::orchestrator::{self, RepoRun, RunConfig};
use agentic_tui::plan::parse_plan;
use shared::StageEvent;
use tokio::sync::mpsc;

static PATH_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-b", "main"]);
    git(dir, &["config", "user.email", "t@t.t"]);
    git(dir, &["config", "user.name", "t"]);
    std::fs::write(dir.join("base.txt"), "base\n").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-m", "base"]);
}

fn install_fake_claude(bin_dir: &Path) {
    std::fs::create_dir_all(bin_dir).unwrap();
    let script = bin_dir.join("claude");
    std::fs::write(
        &script,
        "#!/bin/sh\n\
         echo hi > from_epic.txt\n\
         git add -A\n\
         git commit -m 'epic work' >/dev/null 2>&1\n\
         echo '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"total_cost_usd\":0.1}'\n",
    )
    .unwrap();
    let mut perms = std::fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).unwrap();
}

#[tokio::test]
async fn run_resume_skips_seeded_epics_and_runs_the_rest() {
    let _guard = PATH_ENV_LOCK.lock().await;

    let base = std::env::temp_dir().join(format!("run-resume-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let repo = base.join("greentic");
    let bin_dir = base.join("bin");
    init_repo(&repo);
    install_fake_claude(&bin_dir);

    let original_path = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{original_path}", bin_dir.display()));

    // Two independent epics. epic-a is seeded as already merged, so only epic-b
    // should run.
    let plan = parse_plan(
        r#"{"epics":[
            {"id":"epic-a","title":"A","repo":"greentic","verify":"true","depends_on":[]},
            {"id":"epic-b","title":"B","repo":"greentic","verify":"true","depends_on":[]}
        ]}"#,
    )
    .unwrap();

    let mut repos = HashMap::new();
    repos.insert(
        "greentic".to_string(),
        RepoRun {
            path: repo.clone(),
            base_ref: "HEAD".to_string(),
            integration_branch: "agentic-integration".to_string(),
        },
    );
    let config = RunConfig {
        repos,
        goal: "resume".to_string(),
        default_verify: "true".to_string(),
        initial_cost: 0.0,
    };

    let (tx, mut rx) = mpsc::unbounded_channel::<StageEvent>();
    let driver = tokio::spawn(async move {
        orchestrator::run_resume(&plan, config, &["epic-a".to_string()], tx).await
    });

    let mut started: Vec<String> = Vec::new();
    while let Some(ev) = rx.recv().await {
        if let StageEvent::EpicStarted { id, .. } = ev {
            started.push(id);
        }
    }
    driver.await.unwrap().unwrap();

    std::env::set_var("PATH", original_path);

    assert_eq!(
        started,
        vec!["epic-b".to_string()],
        "only the non-seeded epic runs"
    );

    let _ = std::fs::remove_dir_all(&base);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p agentic-tui --test run_resume 2>&1 | tail -20`
Expected: FAIL — `run_resume` does not exist.

- [ ] **Step 3: Extract `drive` and add `run_resume`**

In `crates/server/src/orchestrator.rs`, change the `run` function signature and body. Replace the header at line 257:

```rust
pub async fn run(
    plan: &Plan,
    config: RunConfig,
    tx: UnboundedSender<StageEvent>,
) -> anyhow::Result<()> {
    let epics_by_id: HashMap<String, Epic> = plan
```

with this (add `run`, `run_resume`, and a `drive` wrapper; `drive` keeps the existing body verbatim except it takes a prebuilt scheduler):

```rust
pub async fn run(
    plan: &Plan,
    config: RunConfig,
    tx: UnboundedSender<StageEvent>,
) -> anyhow::Result<()> {
    let scheduler = Scheduler::new(plan, config::MAX_PARALLEL_EPICS);
    drive(plan, config, scheduler, tx).await
}

/// Resume a run: seed the epics in `seed_merged` as already succeeded, then
/// drive the scheduler over everything else. The seeded epics are skipped by
/// `next_ready` (they are no longer Pending) and their dependents inherit the
/// merged work already on the integration branch.
pub async fn run_resume(
    plan: &Plan,
    config: RunConfig,
    seed_merged: &[String],
    tx: UnboundedSender<StageEvent>,
) -> anyhow::Result<()> {
    let mut scheduler = Scheduler::new(plan, config::MAX_PARALLEL_EPICS);
    for id in seed_merged {
        scheduler.mark_succeeded(id);
    }
    drive(plan, config, scheduler, tx).await
}

async fn drive(
    plan: &Plan,
    config: RunConfig,
    scheduler: Scheduler,
    tx: UnboundedSender<StageEvent>,
) -> anyhow::Result<()> {
    let epics_by_id: HashMap<String, Epic> = plan
```

Then, further down in that same body, delete the old line that built the scheduler locally (currently line 273):

```rust
    let scheduler = Arc::new(Mutex::new(Scheduler::new(plan, config::MAX_PARALLEL_EPICS)));
```
and replace it with (wrap the passed-in scheduler):
```rust
    let scheduler = Arc::new(Mutex::new(scheduler));
```

Leave the rest of the function body (the `loop`, the spawned epic tasks, the final `Done` send) exactly as it is. It now lives inside `drive`.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p agentic-tui --test run_resume 2>&1 | tail -20`
Expected: PASS. Also confirm the existing orchestrator tests still pass:
Run: `cargo test -p agentic-tui --test multi_repo 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/orchestrator.rs crates/server/tests/run_resume.rs
git commit -m "feat: add run_resume that seeds merged epics"
```

---

## Task 4: Persistence hook in the run manager

**Files:**
- Modify: `crates/server/src/run.rs` (add `PersistCtx`, `should_persist`, `persist`, `build_persist_ctx`; wire into `start`, `spawn_pipeline`, `spawn_retry`, `abort`)

**Interfaces:**
- Consumes: `run_store::{PersistedRepo, PersistedRun, runs_dir, save}` (Task 2), `orchestrator::RepoRun`.
- Produces (private to `run.rs`):
  - `struct PersistCtx { id: String, workspace: String, goal: String, default_verify: String, plan_cwd: PathBuf, repos: Vec<run_store::PersistedRepo> }` — `Clone`.
  - `fn build_persist_ctx(id: &str, workspace: &str, goal: &str, default_verify: &str, plan_cwd: &Path, repo_names: &[String], repos: &HashMap<String, orchestrator::RepoRun>) -> PersistCtx`
  - `fn should_persist(ev: &StageEvent) -> bool`
  - `fn persist(ctx: &PersistCtx, app: &App)` — no-op while `app.phase == Planning`.

- [ ] **Step 1: Write the failing unit tests**

Add a `tests` module at the end of `crates/server/src/run.rs` (the file has no `#[cfg(test)]` module yet; add one before the final newline):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use shared::Phase;

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
            plan_cwd: plan_cwd.clone(),
            repos: vec![],
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
}
```

Note: the test calls a `persist_to(dir, ctx, app)` helper so it can target a temp dir. `persist` (used by production) is `persist_to(&run_store::runs_dir(), ctx, app)`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p agentic-tui --lib run::tests 2>&1 | tail -20`
Expected: FAIL — `should_persist`, `PersistCtx`, `persist_to` do not exist.

- [ ] **Step 3: Add the persistence helpers**

In `crates/server/src/run.rs`, add near the top after the imports (the file already imports `PathBuf`; add `Path` to the `std::path` import on line 7 so it reads `use std::path::{Path, PathBuf};`). Add these items after the `RunHandle` struct definition (after line 88):

```rust
/// The static context a run needs to write a persisted snapshot: its identity,
/// config, and the repos it targets, in display order. The changing `App` is
/// passed separately to `persist`.
#[derive(Clone)]
struct PersistCtx {
    id: String,
    workspace: String,
    goal: String,
    default_verify: String,
    plan_cwd: PathBuf,
    repos: Vec<run_store::PersistedRepo>,
}

/// Build a `PersistCtx` from a run's resolved config. `repo_names` fixes the
/// display order; `repos` supplies each repo's refs.
fn build_persist_ctx(
    id: &str,
    workspace: &str,
    goal: &str,
    default_verify: &str,
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
        plan_cwd: plan_cwd.to_path_buf(),
        repos: persisted_repos,
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
    // active run (the workspace-busy guard blocks a concurrent overwrite).
    let plan_json = std::fs::read_to_string(ctx.plan_cwd.join(".agentic-plan.json"))
        .unwrap_or_default();
    let run = run_store::PersistedRun {
        id: ctx.id.clone(),
        workspace: ctx.workspace.clone(),
        goal: ctx.goal.clone(),
        default_verify: ctx.default_verify.clone(),
        plan_cwd: ctx.plan_cwd.clone(),
        repos: ctx.repos.clone(),
        plan_json,
        app: app.clone(),
    };
    if let Err(e) = run_store::save(dir, &run) {
        eprintln!("warning: could not persist run {}: {e}", ctx.id);
    }
}
```

Add the `run_store` import to the `use crate::{...}` line (line 17) so it reads:

```rust
use crate::{config, orchestrator, run_pipeline, run_store, workspace, worktree};
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p agentic-tui --lib run::tests 2>&1 | tail -20`
Expected: PASS (2 tests).

- [ ] **Step 5: Wire persistence into the spawn tasks and abort**

Thread a `PersistCtx` into `spawn_pipeline` and `spawn_retry` and call `persist` on qualifying events.

In `start` (around line 203, before the `spawn_pipeline` call), build the ctx from the locals already in scope:

```rust
    let persist_ctx = build_persist_ctx(
        &id,
        &workspace_name,
        &goal_for_retry,
        &verify_for_retry,
        &plan_cwd_for_retry,
        &repo_names,
        &repos,
    );
```

Note: `workspace_name`, `repo_names`, and `repos` are moved into the `RunHandle` later in `start`. Build `persist_ctx` before those moves (place this block immediately after `plan_cwd_for_retry` is defined on line 201), and pass `persist_ctx.clone()` into `spawn_pipeline`.

Change the `spawn_pipeline` signature (line 240) to accept the ctx as a new final parameter:

```rust
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
    persist_ctx: PersistCtx,
) -> JoinHandle<()> {
```

Inside `spawn_pipeline`, in the `forward_fut` loop (lines 272-282), add the persist call after applying the event and broadcasting:

```rust
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
```

Update the `spawn_pipeline(...)` call in `start` (lines 203-212) to pass `persist_ctx` as the last argument:

```rust
    let task = spawn_pipeline(
        app.clone(),
        tx.clone(),
        completed.clone(),
        plan_cwd,
        (*repos).clone(),
        req.goal,
        verify_cmd,
        req.refine_cost,
        persist_ctx,
    );
```

Do the same for `spawn_retry`. Change its signature (line 343) to take `persist_ctx: PersistCtx` as a new final parameter, and update its `forward_fut` (lines 391-397):

```rust
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
```

Update the `spawn_retry(...)` call inside `retry` (lines 323-333) to pass a freshly built ctx. Inside `retry`, before the `spawn_retry` call, build it from the handle's fields:

```rust
    let persist_ctx = build_persist_ctx(
        &handle.id,
        &handle.workspace,
        &handle.goal,
        &handle.default_verify,
        &handle.plan_cwd,
        &handle.repo_names,
        &handle.repos,
    );
```

and add `persist_ctx` as the final argument to the `spawn_retry(...)` call.

Finally, make `abort` persist the aborted state. In `abort`, extend the Phase 1 clone (lines 422-427) to also capture what a ctx needs, then persist after Phase 3. Change the captured tuple to include the ctx:

```rust
            Some(handle) if !handle.completed.load(Ordering::SeqCst) => {
                handle.completed.store(true, Ordering::SeqCst);
                let persist_ctx = build_persist_ctx(
                    &handle.id,
                    &handle.workspace,
                    &handle.goal,
                    &handle.default_verify,
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
```

Update the destructuring on line 432:

```rust
    let Some((task, app, tx, repo_paths, persist_ctx)) = target else {
        return;
    };
```

And in Phase 3 (after the `tx.send(app.clone())` on line 451), add:

```rust
        persist(&persist_ctx, &app);
```

- [ ] **Step 6: Verify the crate builds and all tests pass**

Run: `cargo test -p agentic-tui 2>&1 | tail -25`
Expected: PASS. In particular `run::tests`, `run_store`, and `run_resume` are green, and the build has no unused-import or arity errors.

- [ ] **Step 7: Commit**

```bash
git add crates/server/src/run.rs
git commit -m "feat: persist run snapshots on every lifecycle event"
```

---

## Task 5: Rehydrate persisted runs on startup

**Files:**
- Modify: `crates/server/src/run.rs` (add `recover_interrupted`, `next_id_after`, `rehydrate`)

**Interfaces:**
- Consumes: `run_store::{PersistedRun, load_all, runs_dir}` (Task 2), `PersistCtx`/`build_persist_ctx` are not needed here.
- Produces:
  - `fn recover_interrupted(app: &mut App)` — an `Implementing` snapshot becomes `Failed` with a restart error; its `Running`/`Verifying` epics become `Failed` with a restart reason; terminal snapshots are left unchanged.
  - `fn next_id_after(runs: &[run_store::PersistedRun]) -> u64`
  - `pub async fn rehydrate()` — loads the store, rebuilds handles into `RUNS`, advances `NEXT_ID`.

- [ ] **Step 1: Write the failing unit tests**

Add to the `tests` module in `crates/server/src/run.rs`:

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p agentic-tui --lib run::tests 2>&1 | tail -20`
Expected: FAIL — `recover_interrupted`, `next_id_after` do not exist.

- [ ] **Step 3: Add the recovery helpers and rehydrate**

In `crates/server/src/run.rs`, add these functions after `persist_to` (from Task 4):

```rust
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

/// The next run id to hand out so a new run never collides with a rehydrated
/// one: one past the largest numeric id on disk, or 1 when the store is empty.
fn next_id_after(runs: &[run_store::PersistedRun]) -> u64 {
    runs.iter()
        .filter_map(|run| run.id.parse::<u64>().ok())
        .max()
        .map(|max| max + 1)
        .unwrap_or(1)
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
        };
        runs.insert(persisted_run.id, handle);
    }
    NEXT_ID.store(next_id, Ordering::SeqCst);
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p agentic-tui --lib run::tests 2>&1 | tail -20`
Expected: PASS (recover + next_id tests green).

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/run.rs
git commit -m "feat: rehydrate persisted runs into the registry on startup"
```

---

## Task 6: Resume a run

**Files:**
- Modify: `crates/server/src/run.rs` (add `ResumeError`, `resumable`, `resume`, `spawn_resume`)

**Interfaces:**
- Consumes: `run_store::{load_all, runs_dir}` (Task 2), `orchestrator::run_resume` (Task 3), `plan::parse_plan`, `worktree::cleanup_all`, `PersistCtx`/`build_persist_ctx`/`should_persist`/`persist` (Task 4).
- Produces:
  - `enum ResumeError { NotFound, RunActive, NotResumable, NoPlan }` with `fn message(&self) -> String`.
  - `fn resumable(app: &App) -> bool` — `phase == Failed` and at least one epic is not `Merged`.
  - `pub async fn resume(run_id: &str) -> Result<(), ResumeError>`.

- [ ] **Step 1: Write the failing unit tests**

Add to the `tests` module in `crates/server/src/run.rs`:

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p agentic-tui --lib run::tests 2>&1 | tail -20`
Expected: FAIL — `resumable`, `ResumeError` do not exist.

- [ ] **Step 3: Add the error type, predicate, resume, and spawn_resume**

In `crates/server/src/run.rs`, add the error type after `RetryError` (after line 66):

```rust
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
```

Add the predicate near the other free helpers (after `next_id_after`):

```rust
/// True when a run can be resumed: it has ended in `Failed` and still has at
/// least one epic that has not merged.
fn resumable(app: &App) -> bool {
    app.phase == shared::Phase::Failed
        && app.epics.iter().any(|epic| epic.status != EpicStatus::Merged)
}
```

Add `resume` and `spawn_resume` after `retry`/`spawn_retry`:

```rust
/// Resume a finished-but-unfinished run: re-run every epic that has not merged,
/// seeding the already-merged epics so the scheduler skips them. Reads the
/// run's own saved plan from the disk store (not the shared `.agentic-plan.json`,
/// which a later run may have overwritten). Cleans up any leftover worktrees
/// first so `worktree::create` does not trip over a stale branch. Flips the run
/// back to active so no new run starts on the workspace and `abort` can tear the
/// resume down.
pub async fn resume(run_id: &str) -> Result<(), ResumeError> {
    // Read the saved plan and merged-epic set outside the RUNS lock; the plan
    // lives on disk, not in the handle.
    let persisted = run_store::load_all(&run_store::runs_dir());
    let saved = persisted
        .into_iter()
        .find(|run| run.id == run_id)
        .ok_or(ResumeError::NotFound)?;
    let plan = crate::plan::parse_plan(&saved.plan_json).map_err(|_| ResumeError::NoPlan)?;

    // Validate and claim the run under the RUNS lock.
    let (app, tx, completed, plan_cwd, repos, goal, default_verify, repo_names, repo_paths) = {
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
            let _ = initial_cost_from(&app);
            (
                app.epics
                    .iter()
                    .filter(|epic| epic.status == EpicStatus::Merged)
                    .map(|epic| epic.id.clone())
                    .collect::<Vec<String>>(),
                app.total_cost,
            )
        };
        handle.completed.store(false, Ordering::SeqCst);
        let persist_ctx = build_persist_ctx(
            &handle.id,
            &handle.workspace,
            &handle.goal,
            &handle.default_verify,
            &handle.plan_cwd,
            &handle.repo_names,
            &handle.repos,
        );
        let task = spawn_resume(
            handle.app.clone(),
            handle.tx.clone(),
            handle.completed.clone(),
            handle.repos.clone(),
            handle.goal.clone(),
            handle.default_verify.clone(),
            plan,
            seed_merged,
            initial_cost,
            handle.repo_paths.clone(),
            persist_ctx,
        );
        handle.task = Some(task);
        // Values consumed above; nothing more needed from the handle.
        (
            handle.app.clone(),
            handle.tx.clone(),
            handle.completed.clone(),
            handle.plan_cwd.clone(),
            handle.repos.clone(),
            handle.goal.clone(),
            handle.default_verify.clone(),
            handle.repo_names.clone(),
            handle.repo_paths.clone(),
        )
    };
    // The tuple binding above documents what the handle holds; the spawn already
    // captured what it needs. Drop the unused clones.
    let _ = (app, tx, completed, plan_cwd, repos, goal, default_verify, repo_names, repo_paths);
    Ok(())
}
```

Note: the tuple return in the lock block above is over-specified. Simplify by removing the trailing tuple. Use this cleaner body for `resume` instead of the block above (this is the version to type):

```rust
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
    let persist_ctx = build_persist_ctx(
        &handle.id,
        &handle.workspace,
        &handle.goal,
        &handle.default_verify,
        &handle.plan_cwd,
        &handle.repo_names,
        &handle.repos,
    );
    let task = spawn_resume(
        handle.app.clone(),
        handle.tx.clone(),
        handle.completed.clone(),
        handle.repos.clone(),
        handle.goal.clone(),
        handle.default_verify.clone(),
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p agentic-tui --lib run::tests 2>&1 | tail -20`
Expected: PASS (`resumable`, `resume_error_messages` green).

- [ ] **Step 5: Verify the whole crate still builds and tests pass**

Run: `cargo test -p agentic-tui 2>&1 | tail -25`
Expected: PASS across lib + integration tests.

- [ ] **Step 6: Commit**

```bash
git add crates/server/src/run.rs
git commit -m "feat: resume an interrupted run from its saved plan"
```

---

## Task 7: HTTP resume endpoint and startup rehydrate

**Files:**
- Modify: `crates/server/src/http.rs` (import `ResumeError`; add `resume_run` handler + route; call `run::rehydrate()` in `serve`)

**Interfaces:**
- Consumes: `run::resume` and `run::ResumeError` (Task 6), `run::rehydrate` (Task 5).
- Produces: route `POST /api/runs/{id}/resume`.

- [ ] **Step 1: Add the handler and route**

In `crates/server/src/http.rs`, extend the `run` import on line 23:

```rust
use crate::run::{self, ResumeError, RetryError, StartError};
```

Add the handler after `retry_epic` (after line 131):

```rust
/// `POST /api/runs/{id}/resume`: resume a finished run that still has
/// unfinished epics, re-running everything not yet merged. 404 for an unknown
/// run, 409 while the run is still active, 400 if there is nothing to resume or
/// the saved plan could not be read.
async fn resume_run(Path(id): Path<String>) -> Response {
    match run::resume(&id).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e @ ResumeError::NotFound) => (StatusCode::NOT_FOUND, e.message()).into_response(),
        Err(e @ ResumeError::RunActive) => (StatusCode::CONFLICT, e.message()).into_response(),
        Err(e @ (ResumeError::NotResumable | ResumeError::NoPlan)) => {
            (StatusCode::BAD_REQUEST, e.message()).into_response()
        }
    }
}
```

Add the route in `router()` (after the retry route on line 223):

```rust
        .route("/api/runs/{id}/epics/{epic_id}/retry", post(retry_epic))
        .route("/api/runs/{id}/resume", post(resume_run))
        .route("/api/runs/{id}/events", get(run_events))
```

- [ ] **Step 2: Rehydrate on startup**

In `serve()` (after the imports resolve and before binding, i.e. as the first line of the function body at line 235), add:

```rust
pub async fn serve(open_browser: bool) -> anyhow::Result<()> {
    // Recover runs from previous sessions before accepting any request.
    run::rehydrate().await;
    let bind_addr =
        std::env::var("AGENTIC_ADDR").unwrap_or_else(|_| "127.0.0.1:0".to_string());
```

- [ ] **Step 3: Verify the crate builds and tests pass**

Run: `cargo test -p agentic-tui 2>&1 | tail -20`
Expected: PASS, no unused-import warnings for `ResumeError`.

- [ ] **Step 4: Commit**

```bash
git add crates/server/src/http.rs
git commit -m "feat: expose resume endpoint and rehydrate runs on startup"
```

---

## Task 8: Web UI resume button and interrupted banner

**Files:**
- Modify: `crates/web/src/api.rs` (add `resume_run`)
- Modify: `crates/web/src/views/run.rs` (resume button + interrupted banner)

**Interfaces:**
- Consumes: `POST /api/runs/{id}/resume` (Task 7).
- Produces: `api::resume_run(id: &str) -> Result<(), String>`; a "Resume run" button and interrupted banner in the run view.

- [ ] **Step 1: Add the API client**

In `crates/web/src/api.rs`, add after `retry_epic` (after line ~129), mirroring `abort_run`:

```rust
/// `POST /api/runs/{id}/resume` -> re-run every unfinished epic of a failed or
/// interrupted run. Takes no body.
pub async fn resume_run(id: &str) -> Result<(), String> {
    let response = Request::post(&format!("/api/runs/{id}/resume"))
        .send()
        .await
        .map_err(|err| format!("failed to reach the resume endpoint: {err}"))?;
    let status = response.status();
    if !(200..300).contains(&status) {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("resume failed with status {status}: {body}"));
    }
    Ok(())
}
```

- [ ] **Step 2: Add resume state and handler in the run view**

In `crates/web/src/views/run.rs`, inside `Run()` next to the abort signals (after line 292), add:

```rust
    let resuming = RwSignal::new(false);
    let resume_error = RwSignal::new(None::<String>);
    let on_resume = {
        let run_id = run_id.clone();
        move |_| {
            if resuming.get_untracked() {
                return;
            }
            resuming.set(true);
            resume_error.set(None);
            let run_id = run_id.clone();
            spawn_local(async move {
                if let Err(err) = api::resume_run(&run_id).await {
                    resume_error.set(Some(err));
                }
                resuming.set(false);
            });
        }
    };
```

- [ ] **Step 3: Render the resume button and interrupted banner**

In the `Some(snapshot)` arm, compute resumability alongside `is_finished` (after line 339):

```rust
                    let is_finished = matches!(snapshot.phase, Phase::Done | Phase::Failed);
                    // Resumable when the run failed with epics still unfinished:
                    // the restart-recovery path and ordinary failures both land
                    // here, and resume re-runs every non-merged epic.
                    let can_resume = matches!(snapshot.phase, Phase::Failed)
                        && snapshot
                            .epics
                            .iter()
                            .any(|epic| !matches!(epic.status, EpicStatus::Merged));
                    let restart_error = snapshot.error.clone();
```

Replace the `run-actions` block (lines 382-398, the `(!is_finished).then(...)`) so it offers Abort while running and Resume when resumable:

```rust
                            {(!is_finished)
                                .then(|| {
                                    view! {
                                        <div class="run-actions">
                                            <button
                                                type="button"
                                                class="btn-danger"
                                                disabled=move || aborting.get()
                                                on:click=on_abort.clone()
                                            >
                                                {move || {
                                                    if aborting.get() { "Aborting..." } else { "Abort run" }
                                                }}
                                            </button>
                                        </div>
                                    }
                                })}
                            {can_resume
                                .then(|| {
                                    view! {
                                        <div class="run-actions">
                                            <button
                                                type="button"
                                                class="btn-primary"
                                                disabled=move || resuming.get()
                                                on:click=on_resume.clone()
                                            >
                                                {move || {
                                                    if resuming.get() { "Resuming..." } else { "Resume run" }
                                                }}
                                            </button>
                                        </div>
                                    }
                                })}
```

Add an interrupted/error banner and the resume error message just inside the run view. Directly after the existing `abort_error` block (lines 320-322), add:

```rust
            {move || {
                resume_error.get().map(|err| view! { <p class="error">{err}</p> })
            }}
```

To surface the restart reason, render `restart_error` above the header. Immediately after the `view! {` that opens the `Some(snapshot)` markup (the `<div class="run-header">` on line 373), add a sibling banner before it. Wrap the header return so it reads:

```rust
                    view! {
                        {restart_error
                            .filter(|_| can_resume)
                            .map(|reason| view! {
                                <div class="run-status-banner interrupted">{reason}</div>
                            })}
                        <div class="run-header">
```

(The rest of the header markup is unchanged.)

- [ ] **Step 4: Build the web crate to verify it compiles**

Run: `cargo check -p web 2>&1 | tail -20`
Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add crates/web/src/api.rs crates/web/src/views/run.rs
git commit -m "feat: add a Resume run button and interrupted banner"
```

---

## Task 9: Full-crate verification

**Files:** none (verification only)

- [ ] **Step 1: Run the whole workspace test suite**

Run: `cargo test 2>&1 | tail -30`
Expected: PASS across `agentic-tui` (lib + `multi_repo` + `run_resume`), `shared`, and any web tests.

- [ ] **Step 2: Build the release binary to confirm the server links**

Run: `cargo build -p agentic-tui 2>&1 | tail -10`
Expected: builds clean.

- [ ] **Step 3: Commit any incidental fixes**

If steps 1-2 required fixes, commit them:

```bash
git add -A
git commit -m "fix: resolve build and test issues from run persistence work"
```

---

## Self-Review Notes

- **Spec coverage:** data model (Task 2), atomic writes (Task 2), persist-only-past-Planning + non-streaming (Task 4), rehydrate + interrupted transform + NEXT_ID (Task 5), run-level resume seeding merged epics + worktree cleanup (Tasks 3, 6), HTTP resume + startup rehydrate (Task 7), web button + interrupted indicator (Task 8), tests for round-trip / skip-corrupt / rehydrate / run_resume (Tasks 2, 3, 4, 5, 6). All spec sections map to a task.
- **Type consistency:** `PersistedRun`/`PersistedRepo` fields are identical across Tasks 2, 4, 5, 6. `run_resume(plan, config, seed_merged, tx)` signature matches between Task 3 (definition) and Task 6 (call). `ResumeError` variants match between Task 6 and Task 7.
- **Known follow-up (out of scope):** `crates/web/src/views/run.rs` may need a `.run-status-banner.interrupted` CSS rule in `crates/web/style.css` for the banner tint; the banner renders regardless. Add a rule if the plain banner looks unstyled.
