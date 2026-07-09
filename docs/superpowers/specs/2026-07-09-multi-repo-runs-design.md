# Multi-Repo Runs Design

## Problem

A workspace today is exactly one git repository (`Workspace { name, path, base,
integration }`, `workspace.rs:7-13`). The whole pipeline is bound to that single
`PathBuf`: `RunConfig.repo` (`orchestrator.rs:157`), the plan session cwd
(`lib.rs:44-54`), every epic worktree, verify, and merge (`orchestrator.rs`,
`worktree.rs`), and the "one active run per workspace" busy check (`run.rs`).

Real projects are often a *group* of repositories. The user's `greentic`
directory is not a repo at all: it is a container of 60+ sibling repos
(`greentic`, `greentic-billing`, `greentic-designer`, `component-*`, ...), each
with its own `.git`. There is no way to give one goal to the tool and have it
work across several of those repos in a single run.

## Goal

One run (one goal) can span multiple git repositories inside a single workspace.
A single plan session reads all repos in the workspace, breaks the goal into
epics, and tags each epic with the repo it belongs to. The orchestrator then
implements, verifies, and merges each epic inside its own repo, with a per-repo
integration branch. One dashboard shows the whole run, every epic card labelled
with its repo.

## Decisions (locked during brainstorming)

- **Workspace = a named group of repos.** Config gains a nested repo list;
  legacy single-repo `[[workspace]]` entries keep working (read as a one-repo
  group).
- **Repo scope = all repos in the workspace.** No per-run repo picker. The
  planner sees every repo and decides which ones the goal touches.
- **One unified plan.** Each epic carries a `repo` field naming its target repo.
- **Verify is planner-determined, per epic.** The plan session picks a verify
  command suited to each epic's repo and writes it into `plan.json`; a run-level
  default (`make verify`) is the fallback when the planner leaves it blank. There
  is no `verify` field in the config.
- **Cross-repo dependencies order work only.** `epic-B` (repo Y) may
  `depends_on` `epic-A` (repo X): B waits for A to merge, but B's worktree still
  branches from repo Y's own base. Code does not cross repo boundaries. Only a
  *same-repo* dependency makes an epic branch from that repo's integration branch
  (inheriting merged work), exactly as today.
- **Per-repo base and integration branch.** Each repo resolves its own base ref
  and integration branch from its config field, then the built-in default.
- **Global budget across all repos.** One cost total, one brake, as today.

## Non-goals (v1)

- No cross-repo code sharing (no dependency-version bumps, no path-dep rewiring
  between repos). Cross-repo deps are ordering only.
- No per-run repo subset picker. Scope is always the whole workspace.
- No change to the scheduler's dependency/parallel-cap logic, the retry-once
  behavior, or run persistence (still session-scoped, in memory).

## Data model

### Workspace and Repo (`workspace.rs`)

```rust
pub struct Repo {
    pub name: String,
    pub path: PathBuf,
    pub base: Option<String>,        // per-repo, fallback default HEAD
    pub integration: Option<String>, // per-repo, fallback agentic-integration
}

pub struct Workspace {
    pub name: String,
    pub repos: Vec<Repo>,
}
```

TOML shape (new):

```toml
[[workspace]]
name = "greentic"

  [[workspace.repo]]
  name = "greentic"
  path = "~/projects/Works/greentic/greentic"
  base = "main"                # optional
  integration = "agentic-wip"  # optional

  [[workspace.repo]]
  name = "greentic-billing"
  path = "~/projects/Works/greentic/greentic-billing"
```

**Backward compatibility.** A legacy entry with a flat `path` (and optional
`base`/`integration`) and no `[[workspace.repo]]` blocks is parsed as a one-repo
workspace whose single `Repo` takes the entry's `name`, `path`, `base`, and
`integration`. Existing `~/.config/agentic-tui/workspaces.toml` files keep
working unchanged. The deserializer accepts either shape per entry; a hand-set
value on either shape survives a `save_workspaces` re-serialization.

**Validation** (`workspace::validate`):
- The workspace has at least one repo.
- Repo names are unique within the workspace (epic `repo` tags resolve by name).
- Every repo path is an existing directory containing `.git` (the current
  single-repo check, applied to each repo, naming the offending repo).

**Common root.** A helper `workspace::common_root(&Workspace) -> PathBuf` returns
the deepest directory that is an ancestor of every repo path (for `greentic`,
the container folder). Used as the plan and refine session cwd. With a single
repo it is that repo's parent; degrade gracefully to `/` only if repos share no
deeper ancestor.

### Plan (`plan.rs`)

```rust
pub struct Epic {
    pub id: String,
    pub title: String,
    pub repo: String,            // NEW: which repo this epic targets
    #[serde(default)]
    pub verify: Option<String>,  // NEW: planner-chosen verify cmd, else default
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub acceptance: Vec<String>,
    #[serde(default)]
    pub tasks: Vec<Task>,
}
```

`Plan::validate` additionally checks that every `epic.repo` matches a repo name
in the run's repo set. Validation takes the set of valid repo names (passed in,
since `plan.rs` has no `Workspace`). Existing checks (unique ids, known deps, no
cycle) are unchanged. Cross-repo deps are allowed (a dep need not share the
dependent's repo).

### Shared wire types (`shared/lib.rs`)

- `RepoDto { name, path, base, integration }` (new) and `WorkspaceDto { name,
  repos: Vec<RepoDto> }` (replaces the flat `name`/`path`/`base`/`integration`).
- `EpicMeta` and `EpicView` gain `repo: String`, so the dashboard can badge each
  card and group the report by repo. `PlanReady`/`EpicStarted` carry the repo.
- `RunSummary` replaces `path: String` with `repos: Vec<String>` (repo names in
  the run) so the multi-run dashboard can show a repo count. `workspace` stays.
- `App::new` and the `StageEvent` application in `apply_stage` thread the epic
  `repo` through (seed `EpicView.repo` from `PlanReady`/`EpicStarted`).
- `RefineQuestionsRequest`/`RefineFinalizeRequest`: `repo: String` is repurposed
  as the refine root (the workspace common root); rename to `root` for clarity.
- `StartRunRequest`: `workspace` carries the repo list. The single-repo `base`
  and `into` fields are dropped (base/integration are per-repo config now);
  `verify: Option<String>` stays as the run-level default-verify override. `goal`
  and `refine_cost` are unchanged.

### RunConfig (`orchestrator.rs`)

```rust
pub struct RepoRun {
    pub path: PathBuf,
    pub base_ref: String,
    pub integration_branch: String,
}

pub struct RunConfig {
    pub repos: HashMap<String, RepoRun>, // keyed by repo name
    pub goal: String,
    pub default_verify: String,          // fallback when epic.verify is None
    pub budget_usd: f64,
    pub initial_cost: f64,
}
```

## Behavior / flow

### Start (`run.rs::start`)

- `req.workspace` (a `WorkspaceDto`) now carries a repo list. Convert to native
  `Workspace`, `workspace::validate` it (each repo is a real git repo, unique
  names, non-empty).
- For each repo resolve `base_ref` (repo field, else `"HEAD"`) and
  `integration` (repo field, else `"agentic-integration"`), then run the same
  fail-fast gates the single-repo path runs today, **per repo**:
  `worktree::verify_ref(base_ref)` and the "integration branch is not the repo's
  checked-out branch" guard. Any failure aborts with a message naming the repo,
  before any session starts.
- Busy check stays keyed by the workspace name (one active run per workspace).
- Build the `RunConfig.repos` map and the plan cwd (`common_root`), pass them to
  `run_pipeline`.

### Plan (`lib.rs::run_pipeline`, `config::plan_prompt`)

- Session cwd = the workspace common root. `plan.json` is written there as
  `.agentic-plan.json`.
- `plan_prompt` gains a repos section listing each repo's `name -> absolute
  path`. It instructs the planner to: explore only the repos relevant to the
  goal (it need not scan all), assign every epic a `repo` (one of the listed
  names), pick a `verify` command appropriate to that repo, and record deps
  (which may cross repos). The JSON shape shown in the prompt adds `"repo"` and
  `"verify"` per epic.
- After parse, `plan.validate(&repo_names)` runs. `PlanReady` carries each epic's
  `repo`.

### Orchestrate (`orchestrator.rs`)

- The `Scheduler` is unchanged (pure ids/deps/parallel-cap).
- `run_epic` resolves `config.repos[&epic.repo]` for the epic's path, base, and
  integration branch. Verify command = `epic.verify` or `config.default_verify`.
- Base-ref choice: if the epic has a dependency **in the same repo**, branch from
  that repo's integration branch (inherit merged deps); otherwise branch from
  that repo's base ref. Cross-repo deps do not affect the base ref, only the
  schedule order (already enforced by the scheduler).
- Worktrees live under each repo's own `.agentic-worktrees/`. Branch name
  `agentic/<epic-id>` (epic ids are globally unique in the plan).
- Merge goes into the epic's repo integration branch, inside that repo. Replace
  the single global `merge_lock` with a per-repo lock (a `HashMap<String,
  Arc<Mutex<()>>>` keyed by repo name) so merges in different repos run in
  parallel while merges within one repo stay serialized.
- Budget brake and `Done` are unchanged (global cost).

### Abort / cleanup (`run.rs::abort`)

- `RunHandle` stores the set of repo paths touched by the run (from
  `RunConfig.repos`). Abort calls `worktree::cleanup_all` for **each** repo path,
  not one. Merged work survives on each repo's integration branch, as today.

### Refine (`refine.rs`, `http.rs`)

- Refine reads the workspace common root instead of a single repo. The handlers
  pass the common root as the refine cwd. Prompt wording generalizes "the
  repository" to "these repositories" but is otherwise unchanged.

## UI (`crates/web`)

- **Workspaces view + onboarding** (`views/workspaces.rs`): a workspace row shows
  its repo count and lists its repos. The onboarding scan groups all repos found
  under the chosen root into **one** workspace (the user names the group), rather
  than producing one workspace per repo. `POST /api/workspaces` accepts the
  grouped shape.
- **New-run** (`views/new_run.rs`): no repo picker (scope is all repos). Show the
  goal field, a read-only list of the repos that will be in scope, the refine
  toggle, and an optional default-verify override. Base/integration are per-repo
  config, not form fields.
- **Run dashboard** (`views/run.rs`): each kanban card gains a repo badge
  (`epic.repo`). The final report groups rows by repo and notes each repo's
  integration branch. The header shows the workspace name and repo count.
- **Dashboard + runs switcher** (`views/dashboard.rs`, `components.rs`): a run
  card shows the workspace and its repo count from `RunSummary.repos`.

## Error handling

- A repo path that is not a git repo, or a base ref that does not resolve, or an
  integration branch checked out in a repo's main tree: the run aborts before any
  session, with a message naming the specific repo.
- An epic tagged with a repo name not in the workspace: `plan.validate` fails
  with a clear error (the run reports it as a fatal plan error, as today).
- A merge conflict in a repo: reported (`EpicConflict`), the epic branch and
  worktree kept in that repo for a manual merge, exactly as today.
- A workspace with zero repos: rejected at validation with a clear message.

## Migration / compatibility

- Existing flat `workspaces.toml` entries load as one-repo workspaces; the user's
  current `greentic` -> `greentic/greentic` entry keeps working with no edit.
- The `WorkspaceDto` shape change is internal (the web UI is updated in the same
  change); no external consumers.
- README + the module-usage doc are updated to describe repo groups, the nested
  config shape, the planner's per-epic repo/verify tagging, and per-repo
  integration branches.

## Testing

- `workspace.rs`: parse a nested multi-repo entry; parse a legacy flat entry into
  a one-repo workspace; `save_workspaces` round-trips the nested shape (set and
  unset `base`/`integration`); `validate` rejects an empty repo list, duplicate
  repo names, and a repo path with no `.git` (naming it); `common_root` returns
  the shared container for sibling repos and a repo's parent for a single repo.
- `plan.rs`: parse an epic with `repo` and `verify`; `validate` rejects an epic
  whose `repo` is not in the set; a cross-repo dependency validates; the existing
  id/dep/cycle checks still hold.
- `orchestrator.rs`: `run_epic` uses the epic's repo config (path/base/
  integration); base-ref selection uses the integration branch only for a
  same-repo dependency and the base ref for a cross-repo dependency or no
  dependency; scheduler tests unchanged.
- `worktree.rs`: unchanged (already takes a repo path per call); no new tests
  beyond confirming the existing suite passes with the per-repo call sites.
- `shared/lib.rs`: `WorkspaceDto`/`RepoDto`, `EpicView.repo`, and `RunSummary`
  (`repos`) round-trip through JSON; `apply_stage` seeds `EpicView.repo` from
  `PlanReady`/`EpicStarted`.
- Integration (`crates/server/tests`): a fake-`claude` plan spanning two repos
  merges each epic into its own repo's integration branch; an epic tagged with an
  unknown repo fails the run before any epic session.

## Files

| File | Change |
|---|---|
| `crates/server/src/workspace.rs` | `Repo` struct; `Workspace.repos`; nested + legacy TOML parse/serialize; `validate` per repo; `common_root`; onboarding scan grouping; tests |
| `crates/server/src/plan.rs` | `Epic.repo`, `Epic.verify`; `validate(repo_names)`; tests |
| `crates/server/src/orchestrator.rs` | `RepoRun`, `RunConfig.repos`/`default_verify`; `run_epic` per-repo resolution; same-repo vs cross-repo base ref; per-repo merge lock; tests |
| `crates/server/src/lib.rs` | `run_pipeline` takes the repo map + common root + default verify; thread `repo` into `PlanReady` |
| `crates/server/src/config.rs` | `plan_prompt` repos section + `repo`/`verify` in the JSON shape; refine prompt wording |
| `crates/server/src/run.rs` | per-repo resolve + fail-fast gates; `RunConfig.repos`; store repo paths in `RunHandle`; abort cleans every repo; `RunSummary.repos` |
| `crates/server/src/refine.rs` | read the common root instead of a single repo |
| `crates/server/src/http.rs` | `WorkspaceDto <-> Workspace` with repo list; `to_workspace`; scan grouping; refine root; tests |
| `crates/shared/src/lib.rs` | `RepoDto`, `WorkspaceDto.repos`; `EpicMeta`/`EpicView.repo`; `RunSummary.repos`; `apply_stage` threads repo; tests |
| `crates/web/src/views/workspaces.rs` | repo-group rows + counts; onboarding groups repos into one workspace |
| `crates/web/src/views/new_run.rs` | read-only in-scope repo list; default-verify override; no repo picker |
| `crates/web/src/views/run.rs` | repo badge per card; report grouped by repo; per-repo integration note |
| `crates/web/src/views/dashboard.rs`, `crates/web/src/components.rs` | workspace + repo count on run cards / switcher |
| `crates/web/src/api.rs` | DTO shape updates for the repo list |
| `README.md`, module-usage doc | document repo groups, nested config, per-epic repo/verify, per-repo integration |
