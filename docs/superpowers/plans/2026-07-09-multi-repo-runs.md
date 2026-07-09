# Multi-Repo Runs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One run (one goal) spans multiple git repositories in a workspace. A single plan session tags each epic with its target repo; the orchestrator implements, verifies, and merges each epic inside that repo, with a per-repo integration branch. Verify is planner-chosen per epic; cross-repo dependencies order work only.

**Architecture:** Split into two phases that each keep `make verify` green. **Phase A** generalizes the *engine* to N repos while the web UI and every web-facing wire type stay unchanged (the web still sends one repo, so N=1 from the web): `RunConfig` carries a `repos` map, epics gain a `repo`, the orchestrator resolves each epic's repo config. **Phase B** reshapes `WorkspaceDto` and the config into repo groups and wires the web UI to send and display N repos. The scheduler, retry-once, budget brake, and worktree helpers are unchanged in shape.

**Tech Stack:** Rust 2021, serde/toml (config + wire), tokio (async git/process), anyhow (errors), Leptos 0.8 CSR + leptos_router 0.8 (web), gloo-net (fetch).

## Global Constraints

- Edition 2021; keep `Cargo.lock` pinned; do not run `cargo update`.
- No `unwrap()`/`expect()`/`panic!` in production code (tests may use them).
- Prose/comment style: no em dashes, no contractions in English prose. Descriptive names; verbs for functions, nouns for types.
- The `shared` crate must stay `wasm32-unknown-unknown`-safe: pure serde types only, no `std::process`, `tokio`, `Instant`, or other OS deps. New wire types (`RepoDto`, added fields) are plain `#[derive(Serialize, Deserialize)]`.
- Every task leaves **`make verify` green**. Verify by running `make verify; echo "exit: $?"` and confirming the printed exit code is `0` — never judge by grepping test output alone. `make verify` runs `trunk build` (web), fmt-check, clippy `--all-targets -- -D warnings`, and tests.
- Adding a field to a struct makes the compiler flag every `{ ... }` literal that omits it; fix each one the compiler reports rather than hunting manually.
- **Phase A (Tasks 1-4) must not break any web-facing wire type.** Changes to `StartRunRequest`, `RunSummary`, `EpicView`, `EpicMeta`, `WorkspaceDto` in Phase A are additive only (new fields, existing fields kept), so `crates/web` compiles and behaves exactly as before with no web edits. The reshaping (dropping fields, changing `WorkspaceDto`) happens in Phase B alongside the web edits.
- Leptos 0.8: `<A>` takes `attr:class`, not `class`. This app is CSR-only; component creation is the mount point.
- Conventional commits; commit after every task. Work on branch `feat/multi-repo-runs` (already checked out).

## File Structure

| File | Phase | Change |
|---|---|---|
| `crates/server/src/plan.rs` | A | `Epic.repo`, `Epic.verify`; `fill_missing_repo`; `validate_repos`; tests |
| `crates/shared/src/lib.rs` | A | `repo` on `EpicMeta`/`EpicView`; `EpicStarted.repo`; `apply_stage` threads repo; `RunSummary.repos` (additive); tests |
| `crates/server/src/orchestrator.rs` | A | `RepoRun`; `RunConfig.repos`/`default_verify`; `run_epic` per-repo; per-repo merge lock; same-repo base-ref; tests |
| `crates/server/src/config.rs` | A | `plan_prompt` gains a repos list + `repo`/`verify` in the JSON shape |
| `crates/server/src/lib.rs` | A | `run_pipeline` takes `plan_cwd` + `repos` map + `default_verify`; fills + validates epic repos |
| `crates/server/src/run.rs` | A | build a one-repo `RunConfig.repos`; store repo paths; abort cleans every repo; `RunSummary.repos` |
| `crates/server/tests/multi_repo.rs` | A | new: a 2-repo plan merges each epic into its own repo's integration branch |
| `crates/shared/src/lib.rs` | B | `RepoDto`; `WorkspaceDto { name, repos }`; `ScanResponse.repos: Vec<RepoDto>`; `StartRunRequest` drops `base`/`into`; `RunSummary` drops `path`; refine `repo`->`root`; tests |
| `crates/server/src/workspace.rs` | B | `Repo`; `Workspace.repos`; nested + legacy TOML; `validate` per repo; `common_root`; scan grouping; tests |
| `crates/server/src/http.rs` | B | DTO conversions with repos; scan returns `RepoDto`s; refine root; tests |
| `crates/server/src/run.rs` | B | build multi-repo `RunConfig.repos` from `workspace.repos`; per-repo gates; `plan_cwd = common_root` |
| `crates/server/src/refine.rs` | B | read the common root |
| `crates/web/src/api.rs` | B | DTO shape updates |
| `crates/web/src/views/workspaces.rs` | B | repo-group rows + counts; onboarding groups repos into one named workspace |
| `crates/web/src/views/new_run.rs` | B | send `workspace.repos`; read-only in-scope repo list; drop base/into fields; default-verify override |
| `crates/web/src/views/run.rs` | B | repo badge per card; report grouped by repo; header repo count |
| `crates/web/src/views/dashboard.rs`, `crates/web/src/components.rs` | B | workspace + repo count from `RunSummary.repos` |
| `README.md`, `docs/agentic-orchestrator-module-usage.md` | B | document repo groups, nested config, per-epic repo/verify, per-repo integration |

---

## Phase A: engine goes multi-repo (web unchanged)

### Task 1: Epic gains repo and verify; repo-aware plan validation

**Files:**
- Modify: `crates/server/src/plan.rs`

**Interfaces:**
- Produces: `Epic { id, title, repo: String, verify: Option<String>, depends_on, acceptance, tasks }`.
- Produces: `Plan::fill_missing_repo(&mut self, repo_name: &str)` sets `repo` on every epic whose `repo` is empty.
- Produces: `Plan::validate_repos(&self, repo_names: &[String]) -> anyhow::Result<()>` requires every `epic.repo` non-empty and present in `repo_names`.

- [ ] **Step 1: Write failing tests**

Add to the `#[cfg(test)] mod tests` block in `crates/server/src/plan.rs`:

```rust
#[test]
fn parses_epic_repo_and_verify() {
    let json = r#"{"epics":[
        {"id":"a","title":"A","repo":"greentic","verify":"cargo test"},
        {"id":"b","title":"B","repo":"billing","depends_on":["a"]}
    ]}"#;
    let plan = parse_plan(json).unwrap();
    assert_eq!(plan.epics[0].repo, "greentic");
    assert_eq!(plan.epics[0].verify.as_deref(), Some("cargo test"));
    assert_eq!(plan.epics[1].repo, "billing");
    assert_eq!(plan.epics[1].verify, None);
}

#[test]
fn fill_missing_repo_only_fills_blanks() {
    let mut plan = parse_plan(
        r#"{"epics":[{"id":"a","title":"A"},{"id":"b","title":"B","repo":"x"}]}"#,
    )
    .unwrap();
    plan.fill_missing_repo("solo");
    assert_eq!(plan.epics[0].repo, "solo");
    assert_eq!(plan.epics[1].repo, "x", "an already-set repo is left alone");
}

#[test]
fn validate_repos_requires_known_nonempty_repo() {
    let names = vec!["greentic".to_string(), "billing".to_string()];

    let ok = parse_plan(
        r#"{"epics":[{"id":"a","title":"A","repo":"greentic","depends_on":[]},
                    {"id":"b","title":"B","repo":"billing","depends_on":["a"]}]}"#,
    )
    .unwrap();
    assert!(ok.validate_repos(&names).is_ok(), "cross-repo dep is allowed");

    let unknown = parse_plan(r#"{"epics":[{"id":"a","title":"A","repo":"ghost"}]}"#).unwrap();
    assert!(unknown.validate_repos(&names).is_err());

    let blank = parse_plan(r#"{"epics":[{"id":"a","title":"A"}]}"#).unwrap();
    assert!(blank.validate_repos(&names).is_err(), "empty repo is rejected");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p agentic-tui plan:: 2>&1 | tail -20`
Expected: compile failure (unknown field `repo`/`verify`; no `fill_missing_repo`/`validate_repos`).

- [ ] **Step 3: Add the fields and methods**

In `crates/server/src/plan.rs`, extend `Epic`:

```rust
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Epic {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub verify: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub acceptance: Vec<String>,
    #[serde(default)]
    pub tasks: Vec<Task>,
}
```

Add to `impl Plan`:

```rust
/// Set `repo` on every epic that has none. Used when a run targets exactly
/// one repo, so the planner may omit the repo tag.
pub fn fill_missing_repo(&mut self, repo_name: &str) {
    for epic in &mut self.epics {
        if epic.repo.is_empty() {
            epic.repo = repo_name.to_string();
        }
    }
}

/// Every epic's `repo` must be non-empty and name one of `repo_names`.
pub fn validate_repos(&self, repo_names: &[String]) -> anyhow::Result<()> {
    for epic in &self.epics {
        if epic.repo.is_empty() {
            anyhow::bail!("epic {} has no repo", epic.id);
        }
        if !repo_names.iter().any(|name| name == &epic.repo) {
            anyhow::bail!("epic {} names unknown repo {}", epic.id, epic.repo);
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p agentic-tui plan:: 2>&1 | tail -20`
Expected: PASS. The existing plan tests still pass (`repo`/`verify` default).

- [ ] **Step 5: Confirm the whole build is green and commit**

Run: `make verify; echo "exit: $?"` (expect `exit: 0`).

```bash
git add crates/server/src/plan.rs
git commit -m "feat: tag plan epics with a repo and optional verify command"
```

---

### Task 2: Thread epic repo through the wire state

**Files:**
- Modify: `crates/shared/src/lib.rs`
- Modify: `crates/server/src/lib.rs` (builds `EpicMeta`)
- Modify: `crates/server/src/orchestrator.rs` (sends `EpicStarted`)

**Interfaces:**
- Consumes: `Epic.repo` (Task 1).
- Produces: `EpicMeta { id, title, repo: String, depends_on }`; `EpicView { id, title, status, cost, repo: String, depends_on }`; `StageEvent::EpicStarted { id, title, repo }`; `RunSummary` gains `repos: Vec<String>` (additive, `path` kept).

- [ ] **Step 1: Write failing tests**

In `crates/shared/src/lib.rs` tests, update `plan_ready_seeds_a_pending_card_per_epic` to build `EpicMeta` with `repo` and assert it lands on the card, and add an `EpicStarted` repo test:

```rust
#[test]
fn plan_ready_seeds_repo_on_each_card() {
    let mut app = App::new("goal".to_string(), "ws".to_string(), 10.0);
    app.apply_stage(StageEvent::PlanReady {
        epics: vec![EpicMeta {
            id: "a".to_string(),
            title: "A".to_string(),
            repo: "greentic".to_string(),
            depends_on: vec![],
        }],
    });
    assert_eq!(app.epics[0].repo, "greentic");
}

#[test]
fn epic_started_carries_repo_onto_a_new_card() {
    let mut app = App::new("g".to_string(), "ws".to_string(), 10.0);
    app.apply_stage(StageEvent::EpicStarted {
        id: "z".to_string(),
        title: "Z".to_string(),
        repo: "billing".to_string(),
    });
    let card = app.epics.iter().find(|e| e.id == "z").unwrap();
    assert_eq!(card.repo, "billing");
}
```

Update the existing `plan_ready_seeds_a_pending_card_per_epic` test's `EpicMeta` literals to include `repo: String::new()`. Update `run_summary_round_trips_through_json` to set `repos: vec!["greentic".to_string()]` and assert it round-trips.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p shared 2>&1 | tail -20`
Expected: compile failure (missing `repo` field on `EpicMeta`/`EpicView`/`EpicStarted`, missing `repos` on `RunSummary`).

- [ ] **Step 3: Add the fields in `shared`**

- `EpicView`: add `pub repo: String,` (place after `cost`).
- `EpicMeta`: add `pub repo: String,` (after `title`).
- `StageEvent::EpicStarted`: change to `EpicStarted { id: String, title: String, repo: String }`.
- `RunSummary`: add `pub repos: Vec<String>,` (keep `path` for now).
- In `apply_stage`:
  - `PlanReady`: build each `EpicView` with `repo: meta.repo`.
  - `EpicStarted { id, title, repo }`: when pushing a new `EpicView`, set `repo`; when the epic already exists, leave its repo. Update the log line unchanged.
  - `EpicSkipped`: the pushed placeholder `EpicView` sets `repo: String::new()`.

The `EpicView` literal built in `PlanReady`:

```rust
.map(|meta| EpicView {
    id: meta.id,
    title: meta.title,
    status: EpicStatus::Pending,
    cost: 0.0,
    repo: meta.repo,
    depends_on: meta.depends_on,
})
```

- [ ] **Step 4: Fix the two server construction sites**

- `crates/server/src/lib.rs`: the `EpicMeta { id, title, depends_on }` map gains `repo: epic.repo.clone()`.
- `crates/server/src/orchestrator.rs`: the `StageEvent::EpicStarted { id, title }` send gains `repo: epic.repo.clone()`.

- [ ] **Step 5: Run tests + whole build, then commit**

Run: `cargo test -p shared 2>&1 | tail -20` (expect PASS), then `make verify; echo "exit: $?"` (expect `exit: 0`).

```bash
git add crates/shared/src/lib.rs crates/server/src/lib.rs crates/server/src/orchestrator.rs
git commit -m "feat: carry each epic's repo through the run state"
```

---

### Task 3: Per-repo RunConfig and orchestration

**Files:**
- Modify: `crates/server/src/orchestrator.rs`
- Modify: `crates/server/src/config.rs` (`plan_prompt`)
- Modify: `crates/server/src/lib.rs` (`run_pipeline`)
- Modify: `crates/server/src/run.rs` (build a one-repo map; abort; `RunSummary.repos`)

**Interfaces:**
- Consumes: `Epic.repo`/`verify`, `Plan::fill_missing_repo`/`validate_repos` (Task 1), `EpicStarted.repo` (Task 2).
- Produces: `orchestrator::RepoRun { path: PathBuf, base_ref: String, integration_branch: String }`.
- Produces: `orchestrator::RunConfig { repos: HashMap<String, RepoRun>, goal: String, default_verify: String, budget_usd: f64, initial_cost: f64 }`.
- Produces: `run_pipeline(plan_cwd: &Path, repos: HashMap<String, RepoRun>, goal: &str, default_verify: &str, refine_cost: f64, tx: &UnboundedSender<StageEvent>)`.
- Produces: `config::plan_prompt(goal: &str, out_path: &str, repos: &[(String, String)]) -> String`.

- [ ] **Step 1: Write the failing orchestrator test**

Add to `orchestrator.rs` tests a unit test for the base-ref decision. Extract the base-ref choice into a pure helper so it is testable without git:

```rust
/// The ref an epic's worktree branches from: its repo's integration branch
/// when a dependency lives in the SAME repo (so it inherits merged work),
/// otherwise its repo's base ref. Cross-repo deps do not change the base.
fn epic_base_ref(epic: &Epic, repo_by_id: &HashMap<String, String>, rc: &RepoRun) -> String {
    let has_same_repo_dep = epic
        .depends_on
        .iter()
        .any(|dep| repo_by_id.get(dep) == Some(&epic.repo));
    if has_same_repo_dep {
        rc.integration_branch.clone()
    } else {
        rc.base_ref.clone()
    }
}
```

Test:

```rust
#[test]
fn base_ref_uses_integration_only_for_a_same_repo_dep() {
    let rc = RepoRun {
        path: std::path::PathBuf::from("/tmp/x"),
        base_ref: "main".to_string(),
        integration_branch: "agentic-integration".to_string(),
    };
    let mut repo_by_id = HashMap::new();
    repo_by_id.insert("a".to_string(), "greentic".to_string());
    repo_by_id.insert("b".to_string(), "greentic".to_string());
    repo_by_id.insert("c".to_string(), "billing".to_string());

    // same-repo dependency -> integration
    let same = Epic { id: "b".into(), title: "B".into(), repo: "greentic".into(),
        verify: None, depends_on: vec!["a".into()], acceptance: vec![], tasks: vec![] };
    assert_eq!(epic_base_ref(&same, &repo_by_id, &rc), "agentic-integration");

    // cross-repo dependency only -> base
    let cross = Epic { id: "c".into(), title: "C".into(), repo: "billing".into(),
        verify: None, depends_on: vec!["a".into()], acceptance: vec![], tasks: vec![] };
    assert_eq!(epic_base_ref(&cross, &repo_by_id, &rc), "main");

    // no dependency -> base
    let free = Epic { id: "a".into(), title: "A".into(), repo: "greentic".into(),
        verify: None, depends_on: vec![], acceptance: vec![], tasks: vec![] };
    assert_eq!(epic_base_ref(&free, &repo_by_id, &rc), "main");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p agentic-tui orchestrator:: 2>&1 | tail -20`
Expected: compile failure (`RepoRun`, `epic_base_ref` do not exist; `RunConfig` shape differs).

- [ ] **Step 3: Reshape `RunConfig` and `run_epic`**

Replace `RunConfig` and add `RepoRun`:

```rust
pub struct RepoRun {
    pub path: PathBuf,
    pub base_ref: String,
    pub integration_branch: String,
}

pub struct RunConfig {
    pub repos: HashMap<String, RepoRun>,
    pub goal: String,
    pub default_verify: String,
    pub budget_usd: f64,
    pub initial_cost: f64,
}
```

`run_epic` gains the resolved repo config and the base ref chosen by `epic_base_ref`. Change its signature to `run_epic(epic: &Epic, rc: &RepoRun, base_ref: &str, verify_cmd: &str, spent: &Arc<Mutex<f64>>, tx: &UnboundedSender<StageEvent>)`. Inside, replace `config.repo` with `rc.path`, `config.base_ref`/`config.integration_branch` with `base_ref`/`rc.integration_branch`, and `config.verify_cmd` with `verify_cmd`. The retry loop, cost accounting, and verify are otherwise unchanged.

In `run`:
- Build `let repo_by_id: HashMap<String, String> = plan.epics.iter().map(|e| (e.id.clone(), e.repo.clone())).collect();`
- Build a per-repo merge lock map: `let merge_locks: HashMap<String, Arc<Mutex<()>>> = config.repos.keys().map(|name| (name.clone(), Arc::new(Mutex::new(())))).collect();` wrapped so the spawned tasks can clone the lock for `epic.repo`.
- When spawning an epic task: resolve `let rc = config.repos.get(&epic.repo)`; if `None`, send `EpicFailed { reason: "epic names unknown repo <name>" }`, `mark_failed`, and skip (defensive; validation should prevent it). Otherwise compute `let base = epic_base_ref(&epic, &repo_by_id, rc);` and `let verify = epic.verify.clone().unwrap_or_else(|| config.default_verify.clone());`, then `run_epic(&epic, rc, &base, &verify, &spent, &tx)`.
- The merge step locks `merge_locks[&epic.repo]` (per repo) instead of the single global `merge_lock`, and calls `worktree::merge_into(&rc.path, &wt.branch, &rc.integration_branch, &rc.base_ref)` and `worktree::remove(&rc.path, &wt)`.

Keep the scheduler, budget brake, skip cascade, and `Done` exactly as they are.

- [ ] **Step 4: Update `config::plan_prompt`**

Change the signature to `pub fn plan_prompt(goal: &str, out_path: &str, repos: &[(String, String)]) -> String`. Add a REPOS block listing `- <name>: <absolute path>` for each entry, and change the instructions and JSON shape so each epic includes `"repo"` (one of the listed names) and `"verify"` (a command suited to that repo). The JSON shape line becomes:

```
{"epics":[{"id":"epic-1","title":"...","repo":"<repo name>","verify":"<verify command>","depends_on":[],"acceptance":["verifiable item"],"tasks":[{"id":"epic-1-t1","title":"...","detail":"..."}]}]}
```

Add to the prose: assign every epic to exactly one repo from the list; a dependency may name an epic in another repo (that only orders the work; code is not shared across repos); choose a `verify` command appropriate to the epic's repo toolchain.

- [ ] **Step 5: Update `run_pipeline`**

Rewrite the signature to `run_pipeline(plan_cwd: &Path, repos: HashMap<String, RepoRun>, goal: &str, default_verify: &str, refine_cost: f64, tx: &UnboundedSender<StageEvent>)`. Inside:
- `plan_path = plan_cwd.join(".agentic-plan.json")`.
- Build `let repo_list: Vec<(String, String)> = { let mut v: Vec<_> = repos.iter().map(|(name, rc)| (name.clone(), rc.path.to_string_lossy().to_string())).collect(); v.sort(); v };` and pass to `plan_prompt`.
- Plan `StageSpec.cwd = plan_cwd`.
- After parse: `let names: Vec<String> = repos.keys().cloned().collect();` then `if names.len() == 1 { parsed.fill_missing_repo(&names[0]); }` then `parsed.validate()?; parsed.validate_repos(&names)?;`.
- `EpicMeta` build gains `repo: epic.repo.clone()` (already added in Task 2's server edit; keep).
- Build `RunConfig { repos, goal: goal.to_string(), default_verify: default_verify.to_string(), budget_usd: config::GLOBAL_BUDGET_USD, initial_cost: refine_cost + outcome.cost }` and call `orchestrator::run(&parsed, run_config, tx.clone())`.

- [ ] **Step 6: Update `run.rs::start` to build a one-repo map (Phase A parity)**

In `start`, after resolving `base_ref`, `integration`, and `verify_cmd`, build a single-entry map keyed by the workspace name and call the new pipeline. Replace the `spawn_pipeline(... repo, verify_cmd, base_ref, integration ...)` wiring with the map form:

```rust
let repo_name = workspace_name.clone();
let mut repos = std::collections::HashMap::new();
repos.insert(
    repo_name.clone(),
    orchestrator::RepoRun {
        path: repo.clone(),
        base_ref: base_ref.clone(),
        integration_branch: integration.clone(),
    },
);
```

`spawn_pipeline` changes to move `plan_cwd` (= `repo.clone()`), `repos`, `goal`, `verify_cmd` (as `default_verify`), and `refine_cost` into the task, calling `run_pipeline(&plan_cwd, repos, &goal, &verify_cmd, refine_cost, &pipeline_tx)`. Store the repo paths for abort: `RunHandle` gains `repo_paths: Vec<PathBuf>` set to `vec![repo.clone()]`; keep the existing `repo` field or replace its use in `abort` with a loop over `repo_paths` (see Step 7). `RunSummary` build in `list()` sets `repos: vec![handle.workspace.clone()]` and keeps `path` from the first repo path (`repo_paths.first()`).

Import `orchestrator` in `run.rs` (`use crate::{... orchestrator ...}`) and drop the now-unused single-repo params from `spawn_pipeline`.

- [ ] **Step 7: Make abort clean every repo**

In `abort`, Phase 1 clones `handle.repo_paths.clone()` instead of a single `repo`. Phase 4 loops:

```rust
for repo in repo_paths {
    if let Err(e) = worktree::cleanup_all(&repo).await {
        eprintln!("warning: could not clean up worktrees for {}: {e}", repo.display());
    }
}
```

- [ ] **Step 8: Run tests + whole build, then commit**

Run: `cargo test -p agentic-tui 2>&1 | tail -30` (expect PASS, including the existing `run_manager` integration tests, which still exercise a one-repo run), then `make verify; echo "exit: $?"` (expect `exit: 0`).

```bash
git add crates/server/src/orchestrator.rs crates/server/src/config.rs crates/server/src/lib.rs crates/server/src/run.rs
git commit -m "feat: orchestrate epics per repo with a RunConfig repos map"
```

---

### Task 4: Multi-repo integration test

**Files:**
- Create: `crates/server/tests/multi_repo.rs`

**Interfaces:**
- Consumes: `agentic_tui::orchestrator::{run, RunConfig, RepoRun}`, `agentic_tui::plan::parse_plan`, a fake `claude` on `PATH`.

- [ ] **Step 1: Write the failing integration test**

Create `crates/server/tests/multi_repo.rs`. Two temp git repos, a fake `claude` that commits a marker file in its cwd then prints a stream-json result, a plan whose two epics target the two repos, and an assertion that each repo's integration branch received its epic's marker file. Use `verify` = `true`.

```rust
//! A run whose plan spans two repos merges each epic into its own repo's
//! integration branch. Exercises the engine's multi-repo path directly at the
//! orchestrator level, before the web UI exposes repo groups.

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
    let status = Command::new("git").args(args).current_dir(repo).status().unwrap();
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

/// A fake `claude` that writes and commits a file named after its cwd's basename,
/// then prints one stream-json result line. Each epic runs in its own worktree,
/// so committing there and merging leaves the file on the integration branch.
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
async fn a_two_repo_plan_merges_each_epic_into_its_own_integration_branch() {
    let _guard = PATH_ENV_LOCK.lock().await;

    let base = std::env::temp_dir().join(format!("multi-repo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let repo_a = base.join("greentic");
    let repo_b = base.join("billing");
    let bin_dir = base.join("bin");
    init_repo(&repo_a);
    init_repo(&repo_b);
    install_fake_claude(&bin_dir);

    let original_path = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{original_path}", bin_dir.display()));

    let plan = parse_plan(
        r#"{"epics":[
            {"id":"epic-a","title":"A","repo":"greentic","verify":"true","depends_on":[]},
            {"id":"epic-b","title":"B","repo":"billing","verify":"true","depends_on":[]}
        ]}"#,
    )
    .unwrap();

    let mut repos = HashMap::new();
    repos.insert("greentic".to_string(), RepoRun {
        path: repo_a.clone(), base_ref: "HEAD".to_string(),
        integration_branch: "agentic-integration".to_string() });
    repos.insert("billing".to_string(), RepoRun {
        path: repo_b.clone(), base_ref: "HEAD".to_string(),
        integration_branch: "agentic-integration".to_string() });

    let config = RunConfig {
        repos,
        goal: "spread work".to_string(),
        default_verify: "true".to_string(),
        budget_usd: 100.0,
        initial_cost: 0.0,
    };

    let (tx, mut rx) = mpsc::unbounded_channel::<StageEvent>();
    let driver = tokio::spawn(async move { orchestrator::run(&plan, config, tx).await });
    // Drain events so the channel does not fill; wait for the driver to finish.
    while rx.recv().await.is_some() {}
    driver.await.unwrap().unwrap();

    std::env::set_var("PATH", original_path);

    let a_file = repo_a.join(".agentic-worktrees/.integration/from_epic.txt");
    let b_file = repo_b.join(".agentic-worktrees/.integration/from_epic.txt");
    assert!(a_file.exists(), "greentic integration branch must have the epic's file");
    assert!(b_file.exists(), "billing integration branch must have the epic's file");

    let _ = std::fs::remove_dir_all(&base);
}
```

- [ ] **Step 2: Run to verify it fails, then passes**

Run: `cargo test -p agentic-tui --test multi_repo 2>&1 | tail -30`
Expected first: FAIL or compile error if any Task 3 wiring is off; once Task 3 is correct, PASS. If it fails, the defect is in Task 3, not the test.

- [ ] **Step 3: Whole build green and commit**

Run: `make verify; echo "exit: $?"` (expect `exit: 0`).

```bash
git add crates/server/tests/multi_repo.rs
git commit -m "test: a two-repo plan merges each epic into its own integration branch"
```

---

## Phase B: expose repo groups to config and the web

### Task 5: Reshape the workspace DTO and config into repo groups

This is the coupled reshape: `WorkspaceDto` is shared by the server and the web, so `shared`, the server, and the web change together to stay green. Keep behavior at parity (a single-repo group behaves exactly as a single-repo workspace does today). No new UX yet; Tasks 6-8 add it.

**Files:**
- Modify: `crates/shared/src/lib.rs`
- Modify: `crates/server/src/workspace.rs`
- Modify: `crates/server/src/http.rs`
- Modify: `crates/server/src/run.rs`
- Modify: `crates/server/src/refine.rs`
- Modify: `crates/web/src/api.rs`
- Modify: `crates/web/src/views/{workspaces.rs,new_run.rs,run.rs,dashboard.rs}`, `crates/web/src/components.rs` (minimal edits to compile)

**Interfaces:**
- Produces: `RepoDto { name: String, path: String, base: Option<String>, integration: Option<String> }`.
- Produces: `WorkspaceDto { name: String, repos: Vec<RepoDto> }`.
- Produces: `ScanResponse { repos: Vec<RepoDto> }` (unchanged name, new element type).
- Produces: `StartRunRequest { workspace: WorkspaceDto, goal: String, verify: Option<String>, refine_cost: f64 }` (drops `base`, `into`).
- Produces: `RunSummary { id, workspace, goal, phase, total_cost, budget, epics, repos }` (drops `path`).
- Produces: server `Repo { name, path: PathBuf, base: Option<String>, integration: Option<String> }`, `Workspace { name, repos: Vec<Repo> }`, `workspace::common_root(&Workspace) -> PathBuf`.
- Produces: `RefineQuestionsRequest`/`RefineFinalizeRequest` field `repo` renamed to `root`.

- [ ] **Step 1: Write failing tests (shared + workspace)**

In `crates/shared/src/lib.rs` tests, replace `workspace_dto_round_trips_through_json` and `start_run_request_round_trips_through_json` with the new shapes, and update `run_summary_round_trips_through_json` to drop `path` and keep `repos`:

```rust
#[test]
fn workspace_dto_round_trips_with_repos() {
    let dto = WorkspaceDto {
        name: "greentic".to_string(),
        repos: vec![
            RepoDto { name: "greentic".to_string(), path: "/tmp/greentic".to_string(),
                base: Some("main".to_string()), integration: None },
            RepoDto { name: "billing".to_string(), path: "/tmp/billing".to_string(),
                base: None, integration: None },
        ],
    };
    let json = serde_json::to_string(&dto).unwrap();
    let back: WorkspaceDto = serde_json::from_str(&json).unwrap();
    assert_eq!(dto, back);
    assert_eq!(back.repos.len(), 2);
}
```

In `crates/server/src/workspace.rs` tests, add nested-parse, legacy-parse, validate, and common_root tests:

```rust
#[test]
fn parses_a_nested_multi_repo_workspace() {
    let toml_text = r#"
[[workspace]]
name = "greentic"

  [[workspace.repo]]
  name = "greentic"
  path = "/tmp/greentic/greentic"
  base = "main"

  [[workspace.repo]]
  name = "billing"
  path = "/tmp/greentic/billing"
"#;
    let ws = parse_workspaces_str(toml_text).unwrap();
    assert_eq!(ws.len(), 1);
    assert_eq!(ws[0].repos.len(), 2);
    assert_eq!(ws[0].repos[0].name, "greentic");
    assert_eq!(ws[0].repos[0].base.as_deref(), Some("main"));
    assert_eq!(ws[0].repos[1].name, "billing");
}

#[test]
fn parses_a_legacy_flat_workspace_as_one_repo() {
    let toml_text = r#"
[[workspace]]
name = "greentic"
path = "/tmp/greentic/greentic"
base = "develop"
"#;
    let ws = parse_workspaces_str(toml_text).unwrap();
    assert_eq!(ws.len(), 1);
    assert_eq!(ws[0].repos.len(), 1, "a flat entry becomes a one-repo group");
    assert_eq!(ws[0].repos[0].name, "greentic");
    assert_eq!(ws[0].repos[0].path, PathBuf::from("/tmp/greentic/greentic"));
    assert_eq!(ws[0].repos[0].base.as_deref(), Some("develop"));
}

#[test]
fn validate_rejects_an_empty_repo_list_and_duplicate_names() {
    let empty = Workspace { name: "x".into(), repos: vec![] };
    assert!(validate(&empty).is_err());
}

#[test]
fn common_root_is_the_shared_parent_of_sibling_repos() {
    let ws = Workspace {
        name: "greentic".into(),
        repos: vec![
            Repo { name: "a".into(), path: PathBuf::from("/home/u/greentic/a"),
                base: None, integration: None },
            Repo { name: "b".into(), path: PathBuf::from("/home/u/greentic/b"),
                base: None, integration: None },
        ],
    };
    assert_eq!(common_root(&ws), PathBuf::from("/home/u/greentic"));
}
```

Keep the existing `save_*` tests but update the `Workspace`/literal shapes (repos-based). Add a save round-trip that writes the nested shape and reads it back.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p shared 2>&1 | tail -20` and `cargo test -p agentic-tui workspace:: 2>&1 | tail -20`
Expected: compile failures for the new shapes.

- [ ] **Step 3: Reshape the `shared` wire types**

- Add `RepoDto` (fields above, `#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]`).
- `WorkspaceDto` becomes `{ name: String, repos: Vec<RepoDto> }`.
- `ScanResponse.repos` becomes `Vec<RepoDto>`.
- `StartRunRequest`: drop `base` and `into`; keep `workspace`, `goal`, `verify`, `refine_cost`.
- `RunSummary`: drop `path`; keep `repos: Vec<String>` (from Task 2).
- `RefineQuestionsRequest`/`RefineFinalizeRequest`: rename field `repo` to `root`.
- Update the round-trip tests accordingly (Step 1).

- [ ] **Step 4: Reshape server `workspace.rs`**

- Add `Repo { name, path: PathBuf, base: Option<String>, integration: Option<String> }` and `Workspace { name: String, repos: Vec<Repo> }`.
- Deserialization accepts either shape per entry. Use an untagged/optional design:

```rust
#[derive(Debug, Deserialize)]
struct RawWorkspace {
    name: String,
    // Legacy single-repo fields:
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    integration: Option<String>,
    // Nested repo list:
    #[serde(default)]
    repo: Vec<RawRepo>,
}

#[derive(Debug, Deserialize)]
struct RawRepo {
    name: String,
    path: String,
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    integration: Option<String>,
}
```

In `parse_workspaces_str`, map each `RawWorkspace` to a `Workspace`: if `repo` is non-empty, build `repos` from it; else if `path` is `Some`, build a one-repo group named after the workspace (`name`) with that `path`/`base`/`integration`; else it is an error (a workspace with neither a `path` nor a `repo` list). Expand `~` on each repo path.
- Serialization writes the nested `[[workspace.repo]]` shape always (a one-repo group serializes to one `[[workspace.repo]]`), with `skip_serializing_if = "Option::is_none"` on `base`/`integration`.
- `save_workspaces` unions by workspace name (not path); the existing name/group wins on conflict.
- `validate(&Workspace)`: non-empty repos; unique repo names; each repo path is a dir containing `.git` (name the offending repo in the error).
- Add `common_root(&Workspace) -> PathBuf`: the longest shared path prefix of all repo paths; for one repo, its parent. Implement by folding `Path::ancestors` or comparing components.
- `scan_for_repos` keeps finding repos but now returns `Vec<Repo>` (its `workspaces_from_paths` builds `Repo`s, not `Workspace`s). The grouping into a named `Workspace` happens at the HTTP/save layer (Task 6), so `scan_for_repos` returns the raw repo list.

- [ ] **Step 5: Update server `http.rs`**

- `impl From<&Workspace> for WorkspaceDto`: map `repos` to `Vec<RepoDto>`.
- `to_workspace(dto)`: build a `Workspace` with `repos` from `dto.repos` (expand `~` per repo).
- Add `RepoDto <-> Repo` conversions.
- `scan_workspaces`: return `ScanResponse { repos: Vec<RepoDto> }` from `scan_for_repos` (map each `Repo` to `RepoDto`).
- `refine_questions`/`refine_finalize`: read `request.root` instead of `request.repo`.
- Fix the `dto_conversion_round_trips_a_workspace` and `save_then_list_round_trips_through_the_dto_boundary` and refine tests to the new shapes (the refine tests pass `root` and still assert cost/goal).

- [ ] **Step 6: Update server `run.rs::start` to consume the repo list**

- Build `RunConfig.repos` from `req.workspace.repos`: for each repo, resolve `base_ref` (`repo.base`, else `"HEAD"`) and `integration` (`repo.integration`, else `"agentic-integration"`); run the fail-fast gates per repo (`worktree::verify_ref`, non-empty integration, integration not the checked-out branch), returning `StartError::Invalid` with a message naming the repo on failure.
- `plan_cwd = workspace::common_root(&to_workspace(&req.workspace))`.
- `RunHandle.repo_paths` = every repo path; `RunSummary.repos` = every repo name.
- Drop the request `base`/`into` reads (fields are gone). `verify` becomes the `default_verify`.
- Busy check stays keyed by workspace name.

- [ ] **Step 7: Update `refine.rs`**

`questions`/`finalize` already take a `repo: &Path`; rename the parameter to `root` for clarity and pass it through unchanged (it is now the common root). No behavior change.

- [ ] **Step 8: Update `crates/web` to compile (minimal, parity)**

- `api.rs`: `refine_questions`/`refine_finalize` take `root: &str` (was `repo`); build requests with `root`. `start_run`, `scan`, `save` use the new DTOs.
- `views/workspaces.rs`: the `For` over `workspaces` keys by `workspace.name` (not `path`); each row shows the workspace name and a repo count (`workspace.repos.len()`). The scan `For` keys by `repo.path`; `checked_paths` still tracks repo paths. `on_save` builds ONE `WorkspaceDto` from the checked `RepoDto`s (temporary: name it after the scanned root's basename; Task 6 adds a proper name field). Keep it compiling and functional.
- `views/new_run.rs`: the workspace is a `WorkspaceDto` with `repos`. Refine calls pass the common root: use the first repo's parent, or send `workspace.repos[0].path` (Task 7 refines this). The subtitle shows the workspace name and repo count. `StartRunRequest` no longer has `base`/`into`; remove those from the two request literals and keep `verify`. The base/into inputs may remain in the form but are unused for now (Task 7 removes them).
- `views/run.rs` and `views/dashboard.rs` and `components.rs`: any read of `RunSummary.path` becomes a read of `RunSummary.repos` (e.g., show the first repo or the count). Keep compiling.

- [ ] **Step 9: Run the full build and commit**

Run: `cargo test -p shared 2>&1 | tail -20`, `cargo test -p agentic-tui 2>&1 | tail -30`, then `make verify; echo "exit: $?"` (expect `exit: 0`; this includes `trunk build`, so the web must compile).

```bash
git add -A
git commit -m "feat: reshape workspaces into repo groups end to end"
```

---

### Task 6: Onboarding groups scanned repos into one named workspace

**Files:**
- Modify: `crates/web/src/views/workspaces.rs`

**Interfaces:**
- Consumes: `api::scan` (returns `RepoDto`s), `api::save` (persists `WorkspaceDto`s).

- [ ] **Step 1: Add a group-name input to the scan panel**

After a scan returns repos, show a text input for the workspace (group) name, prefilled with the scanned root's basename. `on_save` builds one `WorkspaceDto { name: <group name>, repos: <checked RepoDtos> }` and calls `api::save(&[dto])`. Validation: the group name must be non-empty and at least one repo checked, else set `save_error`.

- [ ] **Step 2: Show grouped workspaces in the list**

Each `workspace-row` shows the name and `{n} repos`, and (optionally) an expandable list of repo paths. Keep the row a link to `/run/new?workspace=<name>`.

- [ ] **Step 3: Verify manually via a scan smoke test and commit**

Run `make verify; echo "exit: $?"` (expect `exit: 0`). Sanity-check the flow with the running server if convenient (scan a folder with 2+ repos, name the group, save, confirm one workspace with the repos appears). Commit:

```bash
git add crates/web/src/views/workspaces.rs
git commit -m "feat: save a scanned folder as one repo-group workspace"
```

---

### Task 7: New-run form for repo groups

**Files:**
- Modify: `crates/web/src/views/new_run.rs`

**Interfaces:**
- Consumes: `WorkspaceDto.repos`, `StartRunRequest { workspace, goal, verify, refine_cost }`, refine root.

- [ ] **Step 1: Replace base/into inputs with a read-only in-scope repo list**

Remove the "Base branch" and "Integration branch" inputs (base/integration are per-repo config now). Keep the "Verify command" input, relabelled "Default verify command" with the hint that the planner may override it per repo. Below the goal, render a read-only list of the workspace's repos (name + path) titled "Repos in scope" so the user sees what the run covers.

- [ ] **Step 2: Send the new request and use the common root for refine**

`on_start`/`on_plan` build `StartRunRequest { workspace: selected, goal, verify: normalize(verify_input), refine_cost }`. Refine calls (`api::refine_questions`/`refine_finalize`) pass the workspace common root; compute it client-side as the shared path prefix of `selected.repos`, or send the first repo's path if a shared prefix helper is not worth adding (document the choice in a comment). Keep the refine flow states (`Editing`/`Answering`/`Confirming`/`Submitting`/`Error`) unchanged.

- [ ] **Step 3: Update the subtitle and verify**

The page-head subtitle shows `Workspace <name> · <n> repos`. Run `make verify; echo "exit: $?"` (expect `exit: 0`). Commit:

```bash
git add crates/web/src/views/new_run.rs
git commit -m "feat: new-run form shows in-scope repos and drops per-run branch fields"
```

---

### Task 8: Run dashboard shows each epic's repo

**Files:**
- Modify: `crates/web/src/views/run.rs`
- Modify: `crates/web/src/views/dashboard.rs`, `crates/web/src/components.rs`

**Interfaces:**
- Consumes: `EpicView.repo`, `RunSummary.repos`.

- [ ] **Step 1: Repo badge on each kanban card**

In `epic_card`, render a repo badge (`<span class="kanban-card-repo">{epic.repo}</span>`) in the card meta row when `epic.repo` is non-empty. Add a `.kanban-card-repo` rule to `crates/web/style.css` consistent with the existing card-meta styles (small, muted, monospace).

- [ ] **Step 2: Group the final report by repo**

In `final_report`, group the epic rows under a small per-repo subheading (the repo name) and, after each group, note that repo's integration branch holds its merged work. Keep the total-cost row and the merged-work hint.

- [ ] **Step 3: Repo count on run cards**

In `dashboard.rs` and the `components.rs` runs-switcher, where a run is shown, display the workspace name and `{repos.len()} repos` from `RunSummary.repos`.

- [ ] **Step 4: Verify and commit**

Run `make verify; echo "exit: $?"` (expect `exit: 0`). Commit:

```bash
git add crates/web/src/views/run.rs crates/web/src/views/dashboard.rs crates/web/src/components.rs crates/web/style.css
git commit -m "feat: label runs and epics with their repo in the web UI"
```

---

### Task 9: Documentation

**Files:**
- Modify: `README.md`
- Modify: `docs/agentic-orchestrator-module-usage.md` (if present; otherwise the primary usage doc)

**Interfaces:** none (docs only).

- [ ] **Step 1: Document repo groups**

Update the "Configuring workspaces" section to describe a workspace as a group of repos with the nested `[[workspace.repo]]` shape, note that legacy flat entries still work as one-repo groups, and show the greentic example (the `greentic` container grouping several repos).

- [ ] **Step 2: Document the multi-repo run behavior**

In "How it works" and "Run", describe that one goal can span the workspace's repos, the planner tags each epic with its repo and picks a verify command per repo, cross-repo dependencies order work only (code is not shared across repos), and each repo gets its own integration branch. Update the onboarding-scan description (a scan now saves one grouped workspace).

- [ ] **Step 3: Verify and commit**

Run `make verify; echo "exit: $?"` (expect `exit: 0`; docs do not affect the build but keep the habit). Commit:

```bash
git add README.md docs/
git commit -m "docs: document multi-repo workspaces and runs"
```

---

## Final whole-branch review

After Task 9, dispatch the final whole-branch review (superpowers:requesting-code-review) on the most capable model, over the range `git merge-base main HEAD`..`HEAD`. Focus areas: the per-repo merge-lock correctness (no cross-repo serialization, no same-repo race), the abort cleanup loop covering every repo, the config backward-compat parse (legacy flat entries), and that Phase A left no dead single-repo code paths after Phase B. Then use superpowers:finishing-a-development-branch.
