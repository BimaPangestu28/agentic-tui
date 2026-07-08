# Agentic Orchestrator + Workspace Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend the single-stage PRD generator into a full agentic orchestrator that takes a goal, plans epics and tasks, and drives worktree-isolated parallel `claude -p` sessions that write code, verified before merge, with a TUI workspace picker.

**Architecture:** Three stages — Plan (one `claude -p` writes `plan.json`), Implement (a Rust scheduler runs one `claude -p` per epic in its own git worktree, respecting dependencies and a parallel cap, then runs a verify command), and Integrate (merge passing epics into an integration branch). The correctness core (workspace loading, plan parsing/topology, scheduler state machine) is pure logic, split from IO so it is unit-testable without spawning `claude` or `git`.

**Tech Stack:** Rust 2021, tokio (async + process), ratatui + crossterm (TUI), serde + serde_json (plan.json), toml (workspaces.toml), dirs (config path), anyhow (errors).

## Green-build invariant

Every task leaves the whole crate compiling and every test passing. New
config items and prompts are added alongside the old single-stage ones
(Tasks 1-6); the coupled reshape of `event`/`app`/`engine`/`ui`/`main` and the
removal of the obsolete single-stage code happen together in one atomic
switchover (Task 7). No task ever leaves `cargo build` red.

## Global Constraints

- Edition `2021`; must build on rustc 1.75 (keep `Cargo.lock` pinned; do not run `cargo update`).
- Prose/comment style in code and docs: write directly, no em dashes, no contractions in English prose, no AI-sounding filler.
- Naming: descriptive names, verbs for functions (`load_workspaces`, `parse_plan`), nouns for types (`Workspace`, `Scheduler`). No generic `manager`/`data`.
- One responsibility per file. New modules: `workspace.rs`, `plan.rs`, `orchestrator.rs`, `worktree.rs`.
- Every session runs with cwd = selected workspace root. Sessions never mutate the workspace root directly during Implement; each epic works in its own worktree.
- Tool allowlists are exact: PLAN = `Read,Glob,Grep,Write,WebSearch,WebFetch,Skill`; EPIC = `Read,Glob,Grep,Edit,Write,Bash,WebSearch,WebFetch,Skill`.
- Defaults: `MAX_PARALLEL_EPICS = 3`, `EPIC_BUDGET_USD = 2.0`, `GLOBAL_BUDGET_USD = 10.0`, `VERIFY_CMD = "make verify"`.
- Commit after every task with a `feat:`/`test:`/`refactor:`/`docs:` message.

## File Structure

| File | Responsibility |
|---|---|
| `Cargo.toml` | Dependencies (add serde, toml, dirs) |
| `src/config.rs` | Knobs, tool allowlists, plan + epic prompts |
| `src/workspace.rs` (new) | Load/validate `workspaces.toml`, `~` expansion, `Workspace` |
| `src/plan.rs` (new) | `Plan`/`Epic`/`Task` structs, parse `plan.json`, validate, topological order |
| `src/orchestrator.rs` (new) | Pure `Scheduler` state machine + async `run` driver |
| `src/worktree.rs` (new) | Per-epic git worktree create/remove + merge to integration branch |
| `src/engine.rs` | Generic `run_stage(spec, tx)` spawning `claude -p` |
| `src/event.rs` | `AppEvent` enum (streaming + epic lifecycle) |
| `src/app.rs` | `App` state: phase + per-epic views + log |
| `src/ui.rs` | Picker render, multi-epic progress, final report |
| `src/main.rs` | Arg parsing, picker loop, stage sequencing, abort handling |
| `README.md`, `Makefile` | Updated usage + targets |

---

### Task 1: Dependencies and config knobs (additive)

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/config.rs`

**Interfaces:**
- Consumes: nothing (first task).
- Produces (added alongside the existing `BUDGET_USD`, `MODEL_PRD`, `PRD_TOOLS`, `PRD_MAX_TURNS`, `PERMISSION_MODE`, `Stage`, `prd_stage`, `prd_prompt`, `STYLE`, which all stay):
  `GLOBAL_BUDGET_USD: f64`, `EPIC_BUDGET_USD: f64`, `MODEL_PLAN: &str`, `MODEL_EPIC: &str`, `PLAN_TOOLS: &str`, `EPIC_TOOLS: &str`, `PLAN_MAX_TURNS: u32`, `EPIC_MAX_TURNS: u32`, `MAX_PARALLEL_EPICS: usize`, `DEFAULT_VERIFY_CMD: &str`.

- [ ] **Step 1: Add dependencies to `Cargo.toml`**

Replace the `[dependencies]` block with:

```toml
[dependencies]
tokio = { version = "1.38", features = ["rt-multi-thread", "macros", "process", "io-util", "sync", "time"] }
ratatui = "0.28"
crossterm = "0.28"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
dirs = "5"
anyhow = "1"
```

- [ ] **Step 2: Run build to fetch new crates**

Run: `cargo build 2>&1 | tail -5`
Expected: PASS (compiles; new crates downloaded).

- [ ] **Step 3: Append orchestrator knobs to `src/config.rs`**

Do NOT remove anything. Append at the end of `src/config.rs`:

```rust
// --- Orchestrator knobs (single-stage items above are removed in the switchover) ---

// Global cost circuit breaker across every session in a run.
pub const GLOBAL_BUDGET_USD: f64 = 10.0;
// Budget for a single stage (plan or one epic).
pub const EPIC_BUDGET_USD: f64 = 2.0;

// Models. Plan quality drives epic accuracy, so plan defaults to opus.
pub const MODEL_PLAN: &str = "opus";
pub const MODEL_EPIC: &str = "sonnet";

// Read-only + Write for planning. Adds Edit and Bash for epics that write code.
pub const PLAN_TOOLS: &str = "Read,Glob,Grep,Write,WebSearch,WebFetch,Skill";
pub const EPIC_TOOLS: &str = "Read,Glob,Grep,Edit,Write,Bash,WebSearch,WebFetch,Skill";

pub const PLAN_MAX_TURNS: u32 = 20;
pub const EPIC_MAX_TURNS: u32 = 40;

// How many epics may run in parallel.
pub const MAX_PARALLEL_EPICS: usize = 3;

// Command run inside each epic worktree to decide if the epic passed.
pub const DEFAULT_VERIFY_CMD: &str = "make verify";
```

- [ ] **Step 4: Run build**

Run: `cargo build 2>&1 | tail -5`
Expected: PASS. Warnings about unused constants are acceptable.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/config.rs
git commit -m "feat: add orchestrator dependencies and config knobs"
```

---

### Task 2: Workspace loading (`workspace.rs`)

**Files:**
- Create: `src/workspace.rs`
- Modify: `src/main.rs` (add `mod workspace;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub struct Workspace { pub name: String, pub path: PathBuf }`
  - `pub fn default_config_path() -> PathBuf`
  - `pub fn expand_tilde(raw: &str) -> PathBuf`
  - `pub fn load_workspaces(config_path: &Path) -> anyhow::Result<Vec<Workspace>>`
  - `pub fn validate(workspace: &Workspace) -> anyhow::Result<()>`

- [ ] **Step 1: Register the module**

In `src/main.rs`, add `mod workspace;` next to the other `mod` lines (after `mod ui;`). Add `#[allow(dead_code)]` above it is not needed; the picker in Task 7 uses it, and unused-warnings are acceptable meanwhile.

- [ ] **Step 2: Write the failing tests**

Create `src/workspace.rs` with only the test module and the type it needs:

```rust
//! Loading and validating workspaces from `~/.config/agentic-tui/workspaces.toml`.
//! A workspace is a single project root that every session runs inside.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Workspace {
    pub name: String,
    pub path: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_valid_workspace_list() {
        let toml_text = r#"
[[workspace]]
name = "greentic"
path = "/tmp/greentic"

[[workspace]]
name = "portfolio"
path = "/tmp/portfolio"
"#;
        let workspaces = parse_workspaces_str(toml_text).unwrap();
        assert_eq!(workspaces.len(), 2);
        assert_eq!(workspaces[0].name, "greentic");
        assert_eq!(workspaces[0].path, PathBuf::from("/tmp/greentic"));
    }

    #[test]
    fn expands_a_leading_tilde_to_the_home_directory() {
        std::env::set_var("HOME", "/home/tester");
        let expanded = expand_tilde("~/Works/greentic");
        assert_eq!(expanded, PathBuf::from("/home/tester/Works/greentic"));
    }

    #[test]
    fn leaves_absolute_paths_untouched() {
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
    }

    #[test]
    fn rejects_malformed_toml() {
        let result = parse_workspaces_str("this is not = valid = toml");
        assert!(result.is_err());
    }

    #[test]
    fn validate_rejects_a_path_that_is_not_a_directory() {
        let workspace = Workspace {
            name: "ghost".to_string(),
            path: PathBuf::from("/nonexistent/path/here"),
        };
        assert!(validate(&workspace).is_err());
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test workspace:: 2>&1 | tail -20`
Expected: FAIL to compile with "cannot find function `parse_workspaces_str`" and similar.

- [ ] **Step 4: Write the implementation**

Insert this above the `#[cfg(test)]` block in `src/workspace.rs`:

```rust
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct WorkspacesFile {
    #[serde(default)]
    workspace: Vec<RawWorkspace>,
}

#[derive(Debug, Deserialize)]
struct RawWorkspace {
    name: String,
    path: String,
}

/// Default location of the workspace registry.
pub fn default_config_path() -> PathBuf {
    let base = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join(".config").join("agentic-tui").join("workspaces.toml")
}

/// Expand a single leading `~` to the home directory. Other paths pass through.
pub fn expand_tilde(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(rest)
    } else if raw == "~" {
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
    } else {
        PathBuf::from(raw)
    }
}

/// Parse workspace entries from a TOML string, expanding paths.
fn parse_workspaces_str(text: &str) -> anyhow::Result<Vec<Workspace>> {
    let parsed: WorkspacesFile = toml::from_str(text)?;
    let workspaces = parsed
        .workspace
        .into_iter()
        .map(|raw| Workspace {
            name: raw.name,
            path: expand_tilde(&raw.path),
        })
        .collect();
    Ok(workspaces)
}

/// Load workspaces from a config file on disk.
pub fn load_workspaces(config_path: &Path) -> anyhow::Result<Vec<Workspace>> {
    let text = std::fs::read_to_string(config_path).map_err(|e| {
        anyhow::anyhow!("could not read {}: {e}", config_path.display())
    })?;
    parse_workspaces_str(&text)
}

/// Ensure a workspace points at a real git repository directory.
pub fn validate(workspace: &Workspace) -> anyhow::Result<()> {
    if !workspace.path.is_dir() {
        anyhow::bail!("workspace path is not a directory: {}", workspace.path.display());
    }
    if !workspace.path.join(".git").exists() {
        anyhow::bail!(
            "workspace is not a git repository (no .git): {}",
            workspace.path.display()
        );
    }
    Ok(())
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test workspace:: 2>&1 | tail -20`
Expected: PASS (5 tests).

- [ ] **Step 6: Confirm the whole crate still builds**

Run: `cargo build 2>&1 | tail -5`
Expected: PASS (unused-warnings acceptable).

- [ ] **Step 7: Commit**

```bash
git add src/workspace.rs src/main.rs
git commit -m "feat: load and validate workspaces from config"
```

---

### Task 3: Plan model and topological ordering (`plan.rs`)

**Files:**
- Create: `src/plan.rs`
- Modify: `src/main.rs` (add `mod plan;`)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub struct Task { pub id: String, pub title: String, pub detail: String }`
  - `pub struct Epic { pub id: String, pub title: String, pub depends_on: Vec<String>, pub acceptance: Vec<String>, pub tasks: Vec<Task> }`
  - `pub struct Plan { pub epics: Vec<Epic> }`
  - `pub fn parse_plan(json: &str) -> anyhow::Result<Plan>`
  - `impl Plan { pub fn validate(&self) -> anyhow::Result<()>; pub fn topological_order(&self) -> anyhow::Result<Vec<String>> }`

- [ ] **Step 1: Register the module**

In `src/main.rs`, add `mod plan;` next to the other `mod` lines.

- [ ] **Step 2: Write the failing tests**

Create `src/plan.rs`:

```rust
//! The plan model. A plan is a list of epics, each with tasks, acceptance
//! criteria, and dependencies on other epics. Parsed from `plan.json` written
//! by the Plan stage.

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Epic {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub acceptance: Vec<String>,
    #[serde(default)]
    pub tasks: Vec<Task>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Plan {
    pub epics: Vec<Epic>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_json() -> &'static str {
        r#"{
          "epics": [
            {"id": "a", "title": "Base", "depends_on": [], "acceptance": ["x"],
             "tasks": [{"id": "a1", "title": "do a1", "detail": "details"}]},
            {"id": "b", "title": "Build on base", "depends_on": ["a"], "tasks": []}
          ]
        }"#
    }

    #[test]
    fn parses_epics_with_tasks_and_dependencies() {
        let plan = parse_plan(plan_json()).unwrap();
        assert_eq!(plan.epics.len(), 2);
        assert_eq!(plan.epics[0].tasks[0].id, "a1");
        assert_eq!(plan.epics[1].depends_on, vec!["a".to_string()]);
    }

    #[test]
    fn validate_accepts_a_well_formed_plan() {
        let plan = parse_plan(plan_json()).unwrap();
        assert!(plan.validate().is_ok());
    }

    #[test]
    fn validate_rejects_a_dependency_on_an_unknown_epic() {
        let plan = parse_plan(
            r#"{"epics":[{"id":"a","title":"t","depends_on":["ghost"]}]}"#,
        )
        .unwrap();
        assert!(plan.validate().is_err());
    }

    #[test]
    fn validate_rejects_duplicate_epic_ids() {
        let plan = parse_plan(
            r#"{"epics":[{"id":"a","title":"t"},{"id":"a","title":"u"}]}"#,
        )
        .unwrap();
        assert!(plan.validate().is_err());
    }

    #[test]
    fn validate_rejects_a_dependency_cycle() {
        let plan = parse_plan(
            r#"{"epics":[
                {"id":"a","title":"t","depends_on":["b"]},
                {"id":"b","title":"u","depends_on":["a"]}
            ]}"#,
        )
        .unwrap();
        assert!(plan.validate().is_err());
    }

    #[test]
    fn topological_order_places_dependencies_first() {
        let plan = parse_plan(plan_json()).unwrap();
        let order = plan.topological_order().unwrap();
        let pos_a = order.iter().position(|id| id == "a").unwrap();
        let pos_b = order.iter().position(|id| id == "b").unwrap();
        assert!(pos_a < pos_b);
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test plan:: 2>&1 | tail -20`
Expected: FAIL to compile ("cannot find function `parse_plan`", "no method `validate`").

- [ ] **Step 4: Write the implementation**

Insert above the `#[cfg(test)]` block:

```rust
use std::collections::{HashMap, HashSet};

/// Parse a plan from JSON text.
pub fn parse_plan(json: &str) -> anyhow::Result<Plan> {
    let plan: Plan = serde_json::from_str(json)
        .map_err(|e| anyhow::anyhow!("invalid plan.json: {e}"))?;
    Ok(plan)
}

impl Plan {
    /// Check ids are unique, dependencies exist, and there is no cycle.
    pub fn validate(&self) -> anyhow::Result<()> {
        let mut ids: HashSet<&str> = HashSet::new();
        for epic in &self.epics {
            if !ids.insert(epic.id.as_str()) {
                anyhow::bail!("duplicate epic id: {}", epic.id);
            }
        }
        for epic in &self.epics {
            for dep in &epic.depends_on {
                if !ids.contains(dep.as_str()) {
                    anyhow::bail!("epic {} depends on unknown epic {}", epic.id, dep);
                }
            }
        }
        // topological_order fails if and only if there is a cycle.
        self.topological_order()?;
        Ok(())
    }

    /// Return epic ids in dependency order (each epic after all its deps).
    /// Ties break by the order epics appear in the plan, for determinism.
    pub fn topological_order(&self) -> anyhow::Result<Vec<String>> {
        let mut remaining_deps: HashMap<&str, HashSet<&str>> = HashMap::new();
        for epic in &self.epics {
            remaining_deps.insert(
                epic.id.as_str(),
                epic.depends_on.iter().map(|d| d.as_str()).collect(),
            );
        }

        let mut order: Vec<String> = Vec::new();
        while order.len() < self.epics.len() {
            // Pick the first epic (in plan order) whose deps are all placed.
            let next = self.epics.iter().find(|epic| {
                let already_placed = order.iter().any(|id| id == &epic.id);
                let deps_ready = remaining_deps[epic.id.as_str()].is_empty();
                !already_placed && deps_ready
            });
            match next {
                Some(epic) => {
                    order.push(epic.id.clone());
                    for deps in remaining_deps.values_mut() {
                        deps.remove(epic.id.as_str());
                    }
                }
                None => anyhow::bail!("dependency cycle detected in plan"),
            }
        }
        Ok(order)
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test plan:: 2>&1 | tail -20`
Expected: PASS (6 tests).

- [ ] **Step 6: Commit**

```bash
git add src/plan.rs src/main.rs
git commit -m "feat: add plan model with validation and topological order"
```

---

### Task 4: Scheduler state machine (`orchestrator.rs`, pure part)

**Files:**
- Create: `src/orchestrator.rs`
- Modify: `src/main.rs` (add `mod orchestrator;`)

**Interfaces:**
- Consumes: `plan::Plan` (Task 3).
- Produces:
  - `pub enum EpicState { Pending, Running, Succeeded, Failed, Skipped }` (derives `Clone, Copy, PartialEq, Eq, Debug`)
  - `pub struct Scheduler { .. }`
  - `impl Scheduler { pub fn new(plan: &Plan, max_parallel: usize) -> Self; pub fn next_ready(&self) -> Vec<String>; pub fn mark_running(&mut self, id: &str); pub fn mark_succeeded(&mut self, id: &str); pub fn mark_failed(&mut self, id: &str); pub fn state(&self, id: &str) -> Option<EpicState>; pub fn running_count(&self) -> usize; pub fn is_done(&self) -> bool; pub fn snapshot(&self) -> Vec<(String, EpicState)> }`

- [ ] **Step 1: Register the module**

In `src/main.rs`, add `mod orchestrator;`.

- [ ] **Step 2: Write the failing tests**

Create `src/orchestrator.rs`:

```rust
//! Epic scheduler. The `Scheduler` is a pure state machine: it decides which
//! epics may run now (dependencies satisfied, under the parallel cap) and
//! records outcomes, cascading skips to dependents of failed epics. The async
//! driver that actually spawns sessions is added in a later task.

use std::collections::HashMap;

use crate::plan::Plan;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpicState {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::parse_plan;

    fn scheduler(json: &str, max_parallel: usize) -> Scheduler {
        let plan = parse_plan(json).unwrap();
        Scheduler::new(&plan, max_parallel)
    }

    fn diamond() -> &'static str {
        // a -> b, a -> c, (b,c) -> d
        r#"{"epics":[
            {"id":"a","title":"a"},
            {"id":"b","title":"b","depends_on":["a"]},
            {"id":"c","title":"c","depends_on":["a"]},
            {"id":"d","title":"d","depends_on":["b","c"]}
        ]}"#
    }

    #[test]
    fn only_dependency_free_epics_are_ready_at_first() {
        let sched = scheduler(diamond(), 3);
        assert_eq!(sched.next_ready(), vec!["a".to_string()]);
    }

    #[test]
    fn dependents_become_ready_after_their_dependency_succeeds() {
        let mut sched = scheduler(diamond(), 3);
        sched.mark_running("a");
        sched.mark_succeeded("a");
        let mut ready = sched.next_ready();
        ready.sort();
        assert_eq!(ready, vec!["b".to_string(), "c".to_string()]);
    }

    #[test]
    fn the_parallel_cap_limits_how_many_are_ready() {
        let mut sched = scheduler(diamond(), 1);
        sched.mark_running("a");
        sched.mark_succeeded("a");
        assert_eq!(sched.next_ready().len(), 1);
    }

    #[test]
    fn running_epics_consume_parallel_slots() {
        let mut sched = scheduler(diamond(), 2);
        sched.mark_running("a");
        sched.mark_succeeded("a");
        sched.mark_running("b");
        // cap 2, one running (b), so one more slot -> exactly one ready (c).
        assert_eq!(sched.next_ready(), vec!["c".to_string()]);
    }

    #[test]
    fn a_failed_epic_skips_its_transitive_dependents() {
        let mut sched = scheduler(diamond(), 3);
        sched.mark_running("a");
        sched.mark_failed("a");
        assert_eq!(sched.state("b"), Some(EpicState::Skipped));
        assert_eq!(sched.state("c"), Some(EpicState::Skipped));
        assert_eq!(sched.state("d"), Some(EpicState::Skipped));
        assert!(sched.is_done());
    }

    #[test]
    fn independent_epics_survive_a_failure() {
        let mut sched = scheduler(
            r#"{"epics":[
                {"id":"a","title":"a"},
                {"id":"b","title":"b","depends_on":["a"]},
                {"id":"x","title":"x"}
            ]}"#,
            3,
        );
        sched.mark_running("a");
        sched.mark_failed("a");
        assert_eq!(sched.state("b"), Some(EpicState::Skipped));
        assert_eq!(sched.state("x"), Some(EpicState::Pending));
        assert_eq!(sched.next_ready(), vec!["x".to_string()]);
    }

    #[test]
    fn is_done_only_when_no_epic_is_pending_or_running() {
        let mut sched = scheduler(diamond(), 3);
        assert!(!sched.is_done());
        for id in ["a", "b", "c", "d"] {
            sched.mark_running(id);
            sched.mark_succeeded(id);
        }
        assert!(sched.is_done());
    }

    #[test]
    fn snapshot_reports_every_epic_state() {
        let mut sched = scheduler(diamond(), 3);
        sched.mark_running("a");
        sched.mark_failed("a");
        let snap = sched.snapshot();
        assert_eq!(snap.len(), 4);
        assert!(snap.iter().any(|(id, s)| id == "a" && *s == EpicState::Failed));
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test orchestrator:: 2>&1 | tail -20`
Expected: FAIL to compile ("cannot find type `Scheduler`").

- [ ] **Step 4: Write the implementation**

Insert above the `#[cfg(test)]` block:

```rust
pub struct Scheduler {
    order: Vec<String>,
    deps: HashMap<String, Vec<String>>,
    states: HashMap<String, EpicState>,
    max_parallel: usize,
}

impl Scheduler {
    pub fn new(plan: &Plan, max_parallel: usize) -> Self {
        let mut deps = HashMap::new();
        let mut states = HashMap::new();
        let mut order = Vec::new();
        for epic in &plan.epics {
            order.push(epic.id.clone());
            deps.insert(epic.id.clone(), epic.depends_on.clone());
            states.insert(epic.id.clone(), EpicState::Pending);
        }
        Self { order, deps, states, max_parallel }
    }

    pub fn state(&self, id: &str) -> Option<EpicState> {
        self.states.get(id).copied()
    }

    pub fn running_count(&self) -> usize {
        self.states.values().filter(|s| **s == EpicState::Running).count()
    }

    /// Ids that may start now: Pending, all deps Succeeded, in plan order,
    /// limited to the free parallel slots.
    pub fn next_ready(&self) -> Vec<String> {
        let free_slots = self.max_parallel.saturating_sub(self.running_count());
        if free_slots == 0 {
            return Vec::new();
        }
        let mut ready = Vec::new();
        for id in &self.order {
            if self.states[id] != EpicState::Pending {
                continue;
            }
            let deps_ready = self.deps[id]
                .iter()
                .all(|dep| self.states.get(dep) == Some(&EpicState::Succeeded));
            if deps_ready {
                ready.push(id.clone());
                if ready.len() == free_slots {
                    break;
                }
            }
        }
        ready
    }

    pub fn mark_running(&mut self, id: &str) {
        self.set(id, EpicState::Running);
    }

    pub fn mark_succeeded(&mut self, id: &str) {
        self.set(id, EpicState::Succeeded);
    }

    /// Mark an epic failed, then skip every Pending epic that (transitively)
    /// depends on a failed or skipped epic.
    pub fn mark_failed(&mut self, id: &str) {
        self.set(id, EpicState::Failed);
        self.cascade_skips();
    }

    pub fn is_done(&self) -> bool {
        self.states.values().all(|s| {
            *s != EpicState::Pending && *s != EpicState::Running
        })
    }

    /// A copy of every epic id and its current state.
    pub fn snapshot(&self) -> Vec<(String, EpicState)> {
        self.order
            .iter()
            .map(|id| (id.clone(), self.states[id]))
            .collect()
    }

    fn set(&mut self, id: &str, state: EpicState) {
        if let Some(slot) = self.states.get_mut(id) {
            *slot = state;
        }
    }

    fn cascade_skips(&mut self) {
        loop {
            let mut changed = false;
            let to_skip: Vec<String> = self
                .order
                .iter()
                .filter(|id| self.states[*id] == EpicState::Pending)
                .filter(|id| {
                    self.deps[*id].iter().any(|dep| {
                        matches!(
                            self.states.get(dep),
                            Some(EpicState::Failed) | Some(EpicState::Skipped)
                        )
                    })
                })
                .cloned()
                .collect();
            for id in to_skip {
                self.set(&id, EpicState::Skipped);
                changed = true;
            }
            if !changed {
                break;
            }
        }
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test orchestrator:: 2>&1 | tail -20`
Expected: PASS (8 tests).

- [ ] **Step 6: Commit**

```bash
git add src/orchestrator.rs src/main.rs
git commit -m "feat: add pure epic scheduler state machine"
```

---

### Task 5: Worktree management (`worktree.rs`)

**Files:**
- Create: `src/worktree.rs`
- Modify: `src/main.rs` (add `mod worktree;`)

**Interfaces:**
- Consumes: nothing beyond `std`/`tokio`.
- Produces:
  - `pub struct EpicWorktree { pub id: String, pub path: PathBuf, pub branch: String }`
  - `pub enum MergeResult { Merged, Conflict }` (derives `Debug, Clone, PartialEq`)
  - `pub async fn create(repo: &Path, epic_id: &str) -> anyhow::Result<EpicWorktree>`
  - `pub async fn remove(repo: &Path, worktree: &EpicWorktree) -> anyhow::Result<()>`
  - `pub async fn merge_into(repo: &Path, branch: &str, integration_branch: &str) -> anyhow::Result<MergeResult>`

- [ ] **Step 1: Register the module**

In `src/main.rs`, add `mod worktree;`.

- [ ] **Step 2: Write the integration test**

Create `src/worktree.rs`. This test shells out to real `git` against a temp repo, so run it serially:

```rust
//! Per-epic git worktree lifecycle: create an isolated worktree and branch for
//! an epic, remove it, and merge a passing epic branch into the integration
//! branch. Merge conflicts are reported, never auto-resolved.

use std::path::{Path, PathBuf};

use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct EpicWorktree {
    pub id: String,
    pub path: PathBuf,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MergeResult {
    Merged,
    Conflict,
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn git(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(repo)
            .status()
            .await
            .unwrap();
        assert!(status.success(), "git {:?} failed", args);
    }

    async fn init_repo(dir: &Path) {
        git(dir, &["init", "-b", "main"]).await;
        git(dir, &["config", "user.email", "t@t.t"]).await;
        git(dir, &["config", "user.name", "t"]).await;
        tokio::fs::write(dir.join("base.txt"), "base\n").await.unwrap();
        git(dir, &["add", "-A"]).await;
        git(dir, &["commit", "-m", "base"]).await;
    }

    #[tokio::test]
    async fn create_and_merge_a_clean_epic() {
        let tmp = std::env::temp_dir().join(format!("wt-clean-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        tokio::fs::create_dir_all(&tmp).await.unwrap();
        init_repo(&tmp).await;

        let wt = create(&tmp, "epic-1").await.unwrap();
        tokio::fs::write(wt.path.join("feature.txt"), "hi\n").await.unwrap();
        git(&wt.path, &["add", "-A"]).await;
        git(&wt.path, &["commit", "-m", "epic-1 work"]).await;

        let result = merge_into(&tmp, &wt.branch, "integration").await.unwrap();
        assert_eq!(result, MergeResult::Merged);
        assert!(tmp.join("feature.txt").exists());

        remove(&tmp, &wt).await.unwrap();
        let _ = tokio::fs::remove_dir_all(&tmp).await;
    }

    #[tokio::test]
    async fn a_conflicting_epic_is_reported_not_resolved() {
        let tmp = std::env::temp_dir().join(format!("wt-conflict-{}", std::process::id()));
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        tokio::fs::create_dir_all(&tmp).await.unwrap();
        init_repo(&tmp).await;

        let wt1 = create(&tmp, "epic-1").await.unwrap();
        tokio::fs::write(wt1.path.join("base.txt"), "from epic-1\n").await.unwrap();
        git(&wt1.path, &["commit", "-am", "epic-1"]).await;
        assert_eq!(merge_into(&tmp, &wt1.branch, "integration").await.unwrap(), MergeResult::Merged);

        let wt2 = create(&tmp, "epic-2").await.unwrap();
        tokio::fs::write(wt2.path.join("base.txt"), "from epic-2\n").await.unwrap();
        git(&wt2.path, &["commit", "-am", "epic-2"]).await;
        assert_eq!(merge_into(&tmp, &wt2.branch, "integration").await.unwrap(), MergeResult::Conflict);

        let _ = tokio::fs::remove_dir_all(&tmp).await;
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test worktree:: -- --test-threads=1 2>&1 | tail -20`
Expected: FAIL to compile ("cannot find function `create`").

- [ ] **Step 4: Write the implementation**

Insert above the `#[cfg(test)]` block:

```rust
async fn run_git(repo: &Path, args: &[&str]) -> anyhow::Result<std::process::Output> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("failed to run git {:?}: {e}", args))?;
    Ok(output)
}

async fn run_git_checked(repo: &Path, args: &[&str]) -> anyhow::Result<()> {
    let output = run_git(repo, args).await?;
    if !output.status.success() {
        anyhow::bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Directory where epic worktrees live, next to the repo.
fn worktrees_root(repo: &Path) -> PathBuf {
    repo.join(".agentic-worktrees")
}

/// Create a worktree and branch for an epic, based off the repo HEAD.
pub async fn create(repo: &Path, epic_id: &str) -> anyhow::Result<EpicWorktree> {
    let branch = format!("agentic/{epic_id}");
    let path = worktrees_root(repo).join(epic_id);
    let path_str = path.to_string_lossy().to_string();
    let _ = run_git(repo, &["worktree", "remove", "--force", &path_str]).await;
    let _ = run_git(repo, &["branch", "-D", &branch]).await;
    run_git_checked(repo, &["worktree", "add", "-b", &branch, &path_str, "HEAD"]).await?;
    Ok(EpicWorktree { id: epic_id.to_string(), path, branch })
}

/// Remove an epic worktree and delete its branch.
pub async fn remove(repo: &Path, worktree: &EpicWorktree) -> anyhow::Result<()> {
    let path_str = worktree.path.to_string_lossy().to_string();
    let _ = run_git(repo, &["worktree", "remove", "--force", &path_str]).await;
    let _ = run_git(repo, &["branch", "-D", &worktree.branch]).await;
    Ok(())
}

/// Merge an epic branch into the integration branch, creating it from HEAD on
/// first use. Returns Conflict (and aborts the merge) if it does not apply cleanly.
pub async fn merge_into(
    repo: &Path,
    branch: &str,
    integration_branch: &str,
) -> anyhow::Result<MergeResult> {
    let exists = run_git(repo, &["rev-parse", "--verify", integration_branch])
        .await?
        .status
        .success();
    if !exists {
        run_git_checked(repo, &["branch", integration_branch, "HEAD"]).await?;
    }
    run_git_checked(repo, &["checkout", integration_branch]).await?;
    let merge = run_git(repo, &["merge", "--no-edit", branch]).await?;
    if merge.status.success() {
        Ok(MergeResult::Merged)
    } else {
        let _ = run_git(repo, &["merge", "--abort"]).await;
        Ok(MergeResult::Conflict)
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test worktree:: -- --test-threads=1 2>&1 | tail -20`
Expected: PASS (2 tests). Requires `git` on PATH.

- [ ] **Step 6: Confirm the whole crate still builds**

Run: `cargo build 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/worktree.rs src/main.rs
git commit -m "feat: add per-epic git worktree create, remove, and merge"
```

---

### Task 6: Plan and epic prompts (additive)

**Files:**
- Modify: `src/config.rs`

**Interfaces:**
- Consumes: `crate::plan::Epic` (Task 3), existing `STYLE`.
- Produces:
  - `pub fn plan_prompt(goal: &str, out_path: &str) -> String`
  - `pub fn epic_prompt(goal: &str, epic: &crate::plan::Epic, verify_cmd: &str) -> String`

- [ ] **Step 1: Append `plan_prompt` to `src/config.rs`**

Do not remove `prd_prompt` yet. Append:

```rust
/// Prompt for the Plan stage. Claude explores the workspace and writes a
/// machine-readable plan.json (epics with tasks, dependencies, acceptance).
pub fn plan_prompt(goal: &str, out_path: &str) -> String {
    format!(
        "You are a Tech Lead decomposing a goal into an implementation plan for a \
repository. {style}\n\n\
GOAL:\n{goal}\n\n\
Step 1. Understand this repository with Glob and Grep. Detect language, \
framework, layout, and conventions so the plan fits the real code.\n\
Step 2. Break the goal into epics. Each epic is a coherent unit one engineer \
can implement in one session. Split each epic into concrete tasks. Record \
dependencies between epics with epic ids in depends_on. Keep epics as \
independent as possible so they can run in parallel.\n\
Step 3. Write ONLY a JSON file to {out} with this exact shape and nothing else:\n\
{{\"epics\":[{{\"id\":\"epic-1\",\"title\":\"...\",\"depends_on\":[],\
\"acceptance\":[\"verifiable item\"],\"tasks\":[{{\"id\":\"epic-1-t1\",\
\"title\":\"...\",\"detail\":\"...\"}}]}}]}}\n\
Use short kebab-case ids. Every depends_on entry must be an id that exists. Do \
not create cycles. Do not write any other file.\n\
Step 4. After writing, print the number of epics and a one line summary.",
        style = STYLE,
        goal = goal,
        out = out_path,
    )
}
```

- [ ] **Step 2: Append `epic_prompt` to `src/config.rs`**

```rust
/// Prompt for one epic session. Runs inside that epic's worktree and implements
/// the epic's tasks, then runs the verification command itself as a check.
pub fn epic_prompt(goal: &str, epic: &crate::plan::Epic, verify_cmd: &str) -> String {
    let tasks: String = epic
        .tasks
        .iter()
        .map(|task| format!("- {} ({}): {}\n", task.title, task.id, task.detail))
        .collect();
    let acceptance: String = epic
        .acceptance
        .iter()
        .map(|item| format!("- {item}\n"))
        .collect();
    format!(
        "You are implementing one epic of a larger goal, working in an isolated \
git worktree. {style}\n\n\
OVERALL GOAL:\n{goal}\n\n\
THIS EPIC: {title}\n\n\
TASKS:\n{tasks}\n\
ACCEPTANCE CRITERIA:\n{acceptance}\n\
Implement every task with Edit and Write. Follow existing conventions in the \
repository. When done, run `{verify}` with Bash and fix anything it reports \
until it passes. Do not stop to ask questions, this run is non-interactive. \
Commit your work with git when the epic is complete.",
        style = STYLE,
        goal = goal,
        title = epic.title,
        tasks = tasks,
        acceptance = acceptance,
        verify = verify_cmd,
    )
}
```

- [ ] **Step 3: Run build**

Run: `cargo build 2>&1 | tail -10`
Expected: PASS (crate still compiles; `prd_prompt` and the new prompts coexist).

- [ ] **Step 4: Commit**

```bash
git add src/config.rs
git commit -m "feat: add plan and epic prompts"
```

---

### Task 7: Pipeline switchover (event, app, engine, ui, main, orchestrator driver)

This is the atomic reshape from single-stage to multi-stage. It replaces five
files, adds the orchestrator async driver, and removes the obsolete
single-stage config items in one commit so the crate never goes red. The code
for every file is given in full below; this is transcription plus verification.

**Files:**
- Modify (replace contents): `src/event.rs`, `src/app.rs`, `src/engine.rs`, `src/ui.rs`, `src/main.rs`
- Modify: `src/orchestrator.rs` (append the driver)
- Modify: `src/config.rs` (remove obsolete single-stage items)

**Interfaces:**
- Consumes: `workspace`, `plan`, `orchestrator::Scheduler`, `worktree`, `config` prompts and knobs — all from Tasks 1-6.
- Produces: the runnable binary. Key new signatures:
  - `engine::StageSpec<'a> { tag, cwd, model, tools, max_turns, budget_usd, prompt }`, `engine::Outcome { cost, ok }`, `engine::run_stage(&StageSpec, &UnboundedSender<AppEvent>) -> Result<Outcome>`
  - `orchestrator::RunConfig { repo, goal, verify_cmd, integration_branch }`, `orchestrator::run(&Plan, RunConfig, UnboundedSender<AppEvent>) -> Result<()>`
  - `app::{App, Phase, EpicStatus, EpicView}`, `ui::{render, render_picker}`

- [ ] **Step 1: Replace `src/event.rs`**

```rust
//! Events that flow through the channel to the UI.

use crossterm::event::KeyEvent;

#[derive(Debug, Clone)]
pub enum AppEvent {
    Input(KeyEvent),
    Tick,
    // Streaming from a session. `tag` is "plan" or an epic id.
    StageLog { tag: String, line: String },
    StageAssistant { tag: String, text: String },
    StageTool { tag: String, name: String },
    // Lifecycle.
    PlanReady { epic_count: usize },
    EpicStarted { id: String, title: String },
    EpicVerifying { id: String },
    EpicSucceeded { id: String, cost: f64 },
    EpicFailed { id: String, reason: String },
    EpicSkipped { id: String },
    EpicMerged { id: String },
    EpicConflict { id: String },
    Cost(f64),
    Fatal(String),
    Done,
}
```

- [ ] **Step 2: Replace `src/app.rs`**

```rust
//! State rendered by the UI: the run phase, one view per epic, and a log.

use std::collections::VecDeque;
use std::time::Instant;

use crate::event::AppEvent;

const LOG_CAP: usize = 2000;

#[derive(Clone, Copy, PartialEq)]
pub enum Phase {
    Planning,
    Implementing,
    Done,
    Failed,
}

#[derive(Clone, Copy, PartialEq)]
pub enum EpicStatus {
    Pending,
    Running,
    Verifying,
    Merged,
    Failed,
    Skipped,
    Conflict,
}

#[derive(Clone)]
pub struct EpicView {
    pub id: String,
    pub title: String,
    pub status: EpicStatus,
    pub cost: f64,
}

pub struct App {
    pub goal: String,
    pub workspace: String,
    pub phase: Phase,
    pub epics: Vec<EpicView>,
    pub log: VecDeque<String>,
    pub total_cost: f64,
    pub budget: f64,
    pub error: Option<String>,
    pub spinner: usize,
    pub started: Instant,
    pub elapsed_secs: u64,
}

impl App {
    pub fn new(goal: String, workspace: String, budget: f64) -> Self {
        Self {
            goal,
            workspace,
            phase: Phase::Planning,
            epics: Vec::new(),
            log: VecDeque::new(),
            total_cost: 0.0,
            budget,
            error: None,
            spinner: 0,
            started: Instant::now(),
            elapsed_secs: 0,
        }
    }

    pub fn tick(&mut self) {
        self.spinner = self.spinner.wrapping_add(1);
        if self.phase == Phase::Planning || self.phase == Phase::Implementing {
            self.elapsed_secs = self.started.elapsed().as_secs();
        }
    }

    fn push_log(&mut self, line: String) {
        self.log.push_back(line);
        while self.log.len() > LOG_CAP {
            self.log.pop_front();
        }
    }

    fn epic_mut(&mut self, id: &str) -> Option<&mut EpicView> {
        self.epics.iter_mut().find(|e| e.id == id)
    }

    fn set_status(&mut self, id: &str, status: EpicStatus) {
        if let Some(epic) = self.epic_mut(id) {
            epic.status = status;
        }
    }

    pub fn apply(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::StageLog { tag, line } => self.push_log(format!("[{tag}] {line}")),
            AppEvent::StageAssistant { tag, text } => {
                self.push_log(format!("[{tag}] . {text}"))
            }
            AppEvent::StageTool { tag, name } => {
                self.push_log(format!("[{tag}] tool: {name}"))
            }
            AppEvent::PlanReady { epic_count } => {
                self.phase = Phase::Implementing;
                self.push_log(format!("plan ready: {epic_count} epics"));
            }
            AppEvent::EpicStarted { id, title } => {
                if self.epic_mut(&id).is_none() {
                    self.epics.push(EpicView {
                        id: id.clone(),
                        title: title.clone(),
                        status: EpicStatus::Running,
                        cost: 0.0,
                    });
                } else {
                    self.set_status(&id, EpicStatus::Running);
                }
                self.push_log(format!("epic {id} started: {title}"));
            }
            AppEvent::EpicVerifying { id } => self.set_status(&id, EpicStatus::Verifying),
            AppEvent::EpicSucceeded { id, cost } => {
                if let Some(epic) = self.epic_mut(&id) {
                    epic.cost = cost;
                }
                self.push_log(format!("epic {id} passed verify"));
            }
            AppEvent::EpicMerged { id } => {
                self.set_status(&id, EpicStatus::Merged);
                self.push_log(format!("epic {id} merged"));
            }
            AppEvent::EpicFailed { id, reason } => {
                self.set_status(&id, EpicStatus::Failed);
                self.push_log(format!("epic {id} failed: {reason}"));
            }
            AppEvent::EpicSkipped { id } => {
                if self.epic_mut(&id).is_none() {
                    self.epics.push(EpicView {
                        id: id.clone(),
                        title: String::new(),
                        status: EpicStatus::Skipped,
                        cost: 0.0,
                    });
                } else {
                    self.set_status(&id, EpicStatus::Skipped);
                }
            }
            AppEvent::EpicConflict { id } => {
                self.set_status(&id, EpicStatus::Conflict);
                self.push_log(format!("epic {id} merge conflict, needs manual merge"));
            }
            AppEvent::Cost(c) => self.total_cost = c,
            AppEvent::Fatal(s) => {
                self.phase = Phase::Failed;
                self.error = Some(s.clone());
                self.push_log(format!("! FATAL: {s}"));
            }
            AppEvent::Done => {
                if self.phase != Phase::Failed {
                    self.phase = Phase::Done;
                }
            }
            AppEvent::Input(_) | AppEvent::Tick => {}
        }
    }
}
```

- [ ] **Step 3: Replace `src/engine.rs`**

```rust
//! Engine: drives Claude Code headless (`claude -p`) as a subprocess, reads the
//! stream-json NDJSON line by line, and emits tagged events to the UI. Used by
//! both the Plan stage and each epic session.

use std::path::Path;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;

use crate::config;
use crate::event::AppEvent;

pub struct StageSpec<'a> {
    pub tag: &'a str,
    pub cwd: &'a Path,
    pub model: &'a str,
    pub tools: &'a str,
    pub max_turns: u32,
    pub budget_usd: f64,
    pub prompt: &'a str,
}

pub struct Outcome {
    pub cost: f64,
    pub ok: bool,
}

/// Run a single `claude -p` invocation to completion, parsing its event stream.
pub async fn run_stage(
    spec: &StageSpec<'_>,
    tx: &UnboundedSender<AppEvent>,
) -> anyhow::Result<Outcome> {
    let mut cmd = Command::new("claude");
    cmd.arg("-p")
        .arg(spec.prompt)
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose")
        .arg("--model")
        .arg(spec.model)
        .arg("--allowedTools")
        .arg(spec.tools)
        .arg("--permission-mode")
        .arg(config::PERMISSION_MODE)
        .arg("--max-turns")
        .arg(spec.max_turns.to_string())
        .arg("--max-budget-usd")
        .arg(format!("{:.2}", spec.budget_usd))
        .current_dir(spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = cmd.spawn().map_err(|e| {
        anyhow::anyhow!("failed to spawn `claude` (make sure the CLI is installed on PATH): {e}")
    })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("could not capture stdout"))?;
    let mut lines = BufReader::new(stdout).lines();

    let mut cost = 0.0f64;
    let mut ok = false;
    let tag = spec.tag;

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue, // non-JSON lines are ignored
        };
        match value.get("type").and_then(|t| t.as_str()) {
            Some("system") => {
                if value.get("subtype").and_then(|s| s.as_str()) == Some("init") {
                    let model = value.get("model").and_then(|m| m.as_str()).unwrap_or("");
                    let _ = tx.send(AppEvent::StageLog {
                        tag: tag.to_string(),
                        line: format!("session init ({model})"),
                    });
                }
            }
            Some("assistant") => {
                if let Some(content) = value
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for block in content {
                        match block.get("type").and_then(|t| t.as_str()) {
                            Some("text") => {
                                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                    let first = text.trim().lines().next().unwrap_or("").trim();
                                    if !first.is_empty() {
                                        let preview: String = first.chars().take(120).collect();
                                        let _ = tx.send(AppEvent::StageAssistant {
                                            tag: tag.to_string(),
                                            text: preview,
                                        });
                                    }
                                }
                            }
                            Some("tool_use") => {
                                if let Some(name) = block.get("name").and_then(|n| n.as_str()) {
                                    let _ = tx.send(AppEvent::StageTool {
                                        tag: tag.to_string(),
                                        name: name.to_string(),
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            Some("result") => {
                cost = value
                    .get("total_cost_usd")
                    .and_then(|c| c.as_f64())
                    .unwrap_or(0.0);
                let is_error =
                    value.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);
                let subtype = value.get("subtype").and_then(|s| s.as_str()).unwrap_or("");
                ok = !is_error && (subtype.is_empty() || subtype == "success");
            }
            _ => {}
        }
    }

    let _ = child.wait().await;
    Ok(Outcome { cost, ok })
}
```

- [ ] **Step 4: Append the orchestrator driver to `src/orchestrator.rs`**

Add these imports at the top of `src/orchestrator.rs` (below the existing `use` lines):

```rust
use std::path::PathBuf;
use std::sync::Arc;

use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::Mutex;

use crate::config;
use crate::engine::{self, StageSpec};
use crate::event::AppEvent;
use crate::plan::Epic;
use crate::worktree::{self, MergeResult};
```

Then append (after the `impl Scheduler` block, before the `#[cfg(test)]` module):

```rust
pub struct RunConfig {
    pub repo: PathBuf,
    pub goal: String,
    pub verify_cmd: String,
    pub integration_branch: String,
}

/// Run `verify_cmd` inside a worktree. Returns true on exit code 0.
async fn run_verify(worktree_path: &std::path::Path, verify_cmd: &str) -> bool {
    let status = Command::new("sh")
        .arg("-c")
        .arg(verify_cmd)
        .current_dir(worktree_path)
        .status()
        .await;
    matches!(status, Ok(s) if s.success())
}

/// Run one epic: create worktree, run the session, then verify. On failure,
/// retry once. Returns Ok(Some(worktree)) if it passed (ready to merge),
/// Ok(None) if it failed after retry.
async fn run_epic(
    epic: &Epic,
    config: &RunConfig,
    tx: &UnboundedSender<AppEvent>,
) -> anyhow::Result<Option<worktree::EpicWorktree>> {
    for attempt in 0..2 {
        let wt = worktree::create(&config.repo, &epic.id).await?;
        let prompt = crate::config::epic_prompt(&config.goal, epic, &config.verify_cmd);
        let spec = StageSpec {
            tag: &epic.id,
            cwd: &wt.path,
            model: crate::config::MODEL_EPIC,
            tools: crate::config::EPIC_TOOLS,
            max_turns: crate::config::EPIC_MAX_TURNS,
            budget_usd: crate::config::EPIC_BUDGET_USD,
            prompt: &prompt,
        };
        let outcome = engine::run_stage(&spec, tx).await?;
        let _ = tx.send(AppEvent::Cost(outcome.cost));
        let _ = tx.send(AppEvent::EpicVerifying { id: epic.id.clone() });
        if outcome.ok && run_verify(&wt.path, &config.verify_cmd).await {
            let _ = tx.send(AppEvent::EpicSucceeded {
                id: epic.id.clone(),
                cost: outcome.cost,
            });
            return Ok(Some(wt));
        }
        let _ = worktree::remove(&config.repo, &wt).await;
        if attempt == 0 {
            let _ = tx.send(AppEvent::StageLog {
                tag: epic.id.clone(),
                line: "verify failed, retrying once".to_string(),
            });
        }
    }
    Ok(None)
}

/// Drive the whole Implement + Integrate flow. Schedules epics respecting
/// dependencies and the parallel cap, verifies each, and merges passing epics
/// into the integration branch in the order they finish.
pub async fn run(
    plan: &Plan,
    config: RunConfig,
    tx: UnboundedSender<AppEvent>,
) -> anyhow::Result<()> {
    let epics_by_id: HashMap<String, Epic> = plan
        .epics
        .iter()
        .map(|e| (e.id.clone(), e.clone()))
        .collect();
    let scheduler = Arc::new(Mutex::new(Scheduler::new(plan, config::MAX_PARALLEL_EPICS)));
    let config = Arc::new(config);
    let merge_lock = Arc::new(Mutex::new(()));
    let mut handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    loop {
        let ready = {
            let sched = scheduler.lock().await;
            if sched.is_done() {
                break;
            }
            sched.next_ready()
        };

        if ready.is_empty() {
            if let Some(handle) = handles.pop() {
                let _ = handle.await;
            } else {
                break;
            }
            continue;
        }

        for id in ready {
            {
                let mut sched = scheduler.lock().await;
                sched.mark_running(&id);
            }
            let epic = epics_by_id[&id].clone();
            let _ = tx.send(AppEvent::EpicStarted {
                id: epic.id.clone(),
                title: epic.title.clone(),
            });
            let scheduler = scheduler.clone();
            let config = config.clone();
            let tx = tx.clone();
            let merge_lock = merge_lock.clone();
            handles.push(tokio::spawn(async move {
                match run_epic(&epic, &config, &tx).await {
                    Ok(Some(wt)) => {
                        let merged = {
                            let _guard = merge_lock.lock().await;
                            worktree::merge_into(
                                &config.repo,
                                &wt.branch,
                                &config.integration_branch,
                            )
                            .await
                        };
                        match merged {
                            Ok(MergeResult::Merged) => {
                                let _ = tx.send(AppEvent::EpicMerged { id: epic.id.clone() });
                                let mut sched = scheduler.lock().await;
                                sched.mark_succeeded(&epic.id);
                            }
                            Ok(MergeResult::Conflict) => {
                                let _ = tx.send(AppEvent::EpicConflict { id: epic.id.clone() });
                                let mut sched = scheduler.lock().await;
                                sched.mark_failed(&epic.id);
                            }
                            Err(e) => {
                                let _ = tx.send(AppEvent::EpicFailed {
                                    id: epic.id.clone(),
                                    reason: e.to_string(),
                                });
                                let mut sched = scheduler.lock().await;
                                sched.mark_failed(&epic.id);
                            }
                        }
                        let _ = worktree::remove(&config.repo, &wt).await;
                    }
                    Ok(None) => {
                        let _ = tx.send(AppEvent::EpicFailed {
                            id: epic.id.clone(),
                            reason: "verify failed after retry".to_string(),
                        });
                        let mut sched = scheduler.lock().await;
                        sched.mark_failed(&epic.id);
                    }
                    Err(e) => {
                        let _ = tx.send(AppEvent::EpicFailed {
                            id: epic.id.clone(),
                            reason: e.to_string(),
                        });
                        let mut sched = scheduler.lock().await;
                        sched.mark_failed(&epic.id);
                    }
                }
                let sched = scheduler.lock().await;
                for (eid, state) in sched.snapshot() {
                    if state == EpicState::Skipped {
                        let _ = tx.send(AppEvent::EpicSkipped { id: eid });
                    }
                }
            }));
        }
    }

    for handle in handles {
        let _ = handle.await;
    }
    let _ = tx.send(AppEvent::Done);
    Ok(())
}
```

Note: `HashMap` is already imported at the top of the file from Task 4.

- [ ] **Step 5: Replace `src/ui.rs`**

```rust
//! TUI rendering: a workspace picker screen, then the run view (header, epic
//! list, log, status footer).

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Wrap},
    Frame,
};

use crate::app::{App, EpicStatus, EpicView, Phase};
use crate::workspace::Workspace;

const SPINNER: [&str; 4] = ["|", "/", "-", "\\"];

/// Workspace picker screen shown before a run starts.
pub fn render_picker(f: &mut Frame, workspaces: &[Workspace], selected: usize) {
    let area = f.area();
    let items: Vec<ListItem> = workspaces
        .iter()
        .enumerate()
        .map(|(index, workspace)| {
            let marker = if index == selected { "> " } else { "  " };
            let style = if index == selected {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(vec![Span::styled(
                format!("{marker}{}  {}", workspace.name, workspace.path.display()),
                style,
            )]))
        })
        .collect();
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Select workspace (up/down, Enter, q to quit) "),
    );
    f.render_widget(list, area);
}

pub fn render(f: &mut Frame, app: &App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(app.epics.len().min(8) as u16 + 2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(f, app, chunks[0]);
    render_epics(f, app, chunks[1]);
    render_log(f, app, chunks[2]);
    render_footer(f, app, chunks[3]);
}

fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(area);

    let info = vec![
        Line::from(vec![
            Span::styled("Goal      ", Style::default().fg(Color::DarkGray)),
            Span::raw(truncate(&app.goal, 70)),
        ]),
        Line::from(vec![
            Span::styled("Workspace ", Style::default().fg(Color::DarkGray)),
            Span::styled(truncate(&app.workspace, 70), Style::default().fg(Color::Cyan)),
        ]),
    ];
    let info_p = Paragraph::new(info)
        .block(Block::default().borders(Borders::ALL).title(" Agentic Orchestrator "));
    f.render_widget(info_p, cols[0]);

    let ratio = if app.budget > 0.0 {
        (app.total_cost / app.budget).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title(" Budget "))
        .gauge_style(Style::default().fg(if ratio > 0.85 { Color::Red } else { Color::Green }))
        .ratio(ratio)
        .label(format!("${:.3} / ${:.2}", app.total_cost, app.budget));
    f.render_widget(gauge, cols[1]);
}

fn status_glyph(status: EpicStatus) -> (&'static str, Color) {
    match status {
        EpicStatus::Pending => ("pending  ", Color::DarkGray),
        EpicStatus::Running => ("running  ", Color::Yellow),
        EpicStatus::Verifying => ("verifying", Color::Yellow),
        EpicStatus::Merged => ("merged   ", Color::Green),
        EpicStatus::Failed => ("failed   ", Color::Red),
        EpicStatus::Skipped => ("skipped  ", Color::DarkGray),
        EpicStatus::Conflict => ("conflict ", Color::Magenta),
    }
}

fn render_epics(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .epics
        .iter()
        .map(|epic: &EpicView| {
            let (label, color) = status_glyph(epic.status);
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {label} "), Style::default().fg(color)),
                Span::raw(format!("{}  {}", epic.id, truncate(&epic.title, 60))),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Epics "));
    f.render_widget(list, area);
}

fn render_log(f: &mut Frame, app: &App, area: Rect) {
    let inner_h = area.height.saturating_sub(2) as usize;
    let total = app.log.len();
    let start = total.saturating_sub(inner_h);
    let lines: Vec<Line> = app.log.iter().skip(start).map(|l| Line::from(l.clone())).collect();
    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" Log "))
        .wrap(Wrap { trim: false });
    f.render_widget(paragraph, area);
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let (icon, label, color) = match app.phase {
        Phase::Planning => (
            SPINNER[app.spinner % SPINNER.len()],
            format!("planning... {}s", app.elapsed_secs),
            Color::Yellow,
        ),
        Phase::Implementing => (
            SPINNER[app.spinner % SPINNER.len()],
            format!("implementing... {}s", app.elapsed_secs),
            Color::Yellow,
        ),
        Phase::Done => ("ok", "done".to_string(), Color::Green),
        Phase::Failed => ("x", "failed".to_string(), Color::Red),
    };
    let line = Line::from(vec![
        Span::styled(
            format!(" {icon} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(label, Style::default().fg(color)),
        Span::styled("   q: quit/abort", Style::default().fg(Color::DarkGray)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{t}...")
    }
}
```

- [ ] **Step 6: Replace `src/main.rs`**

```rust
//! Agentic orchestrator TUI. Picks a workspace, plans a goal into epics, then
//! drives worktree-isolated `claude -p` sessions that implement and verify each
//! epic, merging passing epics into an integration branch.
//!
//! Usage:
//!   cargo run -- "<goal>" [--workspace <name|path>] [--verify "<cmd>"]
//!
//! Prerequisites: the Claude Code CLI on PATH, a subscription login, and git.

mod app;
mod config;
mod engine;
mod event;
mod orchestrator;
mod plan;
mod ui;
mod workspace;

use std::io::stdout;
use std::time::Duration;

use crossterm::{
    event::{Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::mpsc;

use app::{App, Phase};
use event::AppEvent;
use workspace::Workspace;

struct Args {
    goal: String,
    workspace: Option<String>,
    verify: Option<String>,
}

fn parse_args() -> Option<Args> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut goal_parts: Vec<String> = Vec::new();
    let mut workspace = None;
    let mut verify = None;
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--workspace" => {
                i += 1;
                workspace = raw.get(i).cloned();
            }
            "--verify" => {
                i += 1;
                verify = raw.get(i).cloned();
            }
            other => goal_parts.push(other.to_string()),
        }
        i += 1;
    }
    let goal = goal_parts.join(" ").trim().to_string();
    if goal.is_empty() {
        None
    } else {
        Some(Args { goal, workspace, verify })
    }
}

/// Resolve the chosen workspace: match `--workspace` by name or path, otherwise
/// show the picker. Returns None if the user quits the picker.
fn resolve_workspace(
    args: &Args,
    workspaces: &[Workspace],
) -> anyhow::Result<Option<Workspace>> {
    if let Some(wanted) = &args.workspace {
        if let Some(found) = workspaces.iter().find(|w| &w.name == wanted) {
            return Ok(Some(found.clone()));
        }
        let path = workspace::expand_tilde(wanted);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "workspace".to_string());
        return Ok(Some(Workspace { name, path }));
    }
    run_picker(workspaces)
}

/// Blocking picker loop on its own alternate screen.
fn run_picker(workspaces: &[Workspace]) -> anyhow::Result<Option<Workspace>> {
    if workspaces.is_empty() {
        anyhow::bail!(
            "no workspaces configured. Add entries to {} or pass --workspace <path>",
            workspace::default_config_path().display()
        );
    }
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;
    let mut selected = 0usize;
    let chosen = loop {
        terminal.draw(|f| ui::render_picker(f, workspaces, selected))?;
        if let Event::Key(key) = crossterm::event::read()? {
            match key.code {
                KeyCode::Up => selected = selected.saturating_sub(1),
                KeyCode::Down => {
                    if selected + 1 < workspaces.len() {
                        selected += 1;
                    }
                }
                KeyCode::Enter => break Some(workspaces[selected].clone()),
                KeyCode::Char('q') => break None,
                _ => {}
            }
        }
    };
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(chosen)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = match parse_args() {
        Some(a) => a,
        None => {
            eprintln!("usage: agentic-tui \"<goal>\" [--workspace <name|path>] [--verify \"<cmd>\"]");
            std::process::exit(1);
        }
    };

    let workspaces = workspace::load_workspaces(&workspace::default_config_path())
        .unwrap_or_default();
    let selected = match resolve_workspace(&args, &workspaces)? {
        Some(w) => w,
        None => {
            println!("no workspace selected");
            return Ok(());
        }
    };
    workspace::validate(&selected)?;
    let repo = selected.path.canonicalize().unwrap_or(selected.path.clone());
    let verify_cmd = args.verify.clone().unwrap_or_else(|| config::DEFAULT_VERIFY_CMD.to_string());

    let mut app = App::new(args.goal.clone(), selected.name.clone(), config::GLOBAL_BUDGET_USD);

    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();

    let input_tx = tx.clone();
    std::thread::spawn(move || loop {
        match crossterm::event::read() {
            Ok(Event::Key(key)) => {
                if input_tx.send(AppEvent::Input(key)).is_err() {
                    break;
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    });

    let tick_tx = tx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(200));
        loop {
            interval.tick().await;
            if tick_tx.send(AppEvent::Tick).is_err() {
                break;
            }
        }
    });

    let pipeline_tx = tx.clone();
    let repo_run = repo.clone();
    let goal_run = args.goal.clone();
    let verify_run = verify_cmd.clone();
    tokio::spawn(async move {
        if let Err(e) = run_pipeline(&repo_run, &goal_run, &verify_run, &pipeline_tx).await {
            let _ = pipeline_tx.send(AppEvent::Fatal(e.to_string()));
        }
    });

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    loop {
        terminal.draw(|f| ui::render(f, &app))?;
        match rx.recv().await {
            Some(AppEvent::Input(key)) => match (key.code, key.modifiers) {
                (KeyCode::Char('q'), _) => break,
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => break,
                _ => {}
            },
            Some(AppEvent::Tick) => app.tick(),
            Some(other) => {
                let done = matches!(other, AppEvent::Done | AppEvent::Fatal(_));
                app.apply(other);
                if done {
                    terminal.draw(|f| ui::render(f, &app))?;
                }
            }
            None => break,
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    print_report(&app, &repo);
    Ok(())
}

/// Plan the goal, then run the orchestrator.
async fn run_pipeline(
    repo: &std::path::Path,
    goal: &str,
    verify_cmd: &str,
    tx: &mpsc::UnboundedSender<AppEvent>,
) -> anyhow::Result<()> {
    let plan_path = repo.join(".agentic-plan.json");
    let plan_path_str = plan_path.to_string_lossy().to_string();
    let prompt = config::plan_prompt(goal, &plan_path_str);
    let spec = engine::StageSpec {
        tag: "plan",
        cwd: repo,
        model: config::MODEL_PLAN,
        tools: config::PLAN_TOOLS,
        max_turns: config::PLAN_MAX_TURNS,
        budget_usd: config::EPIC_BUDGET_USD,
        prompt: &prompt,
    };
    let outcome = engine::run_stage(&spec, tx).await?;
    let _ = tx.send(AppEvent::Cost(outcome.cost));

    let plan_text = std::fs::read_to_string(&plan_path)
        .map_err(|e| anyhow::anyhow!("plan.json was not written: {e}"))?;
    let parsed = plan::parse_plan(&plan_text)?;
    parsed.validate()?;
    let _ = tx.send(AppEvent::PlanReady { epic_count: parsed.epics.len() });

    let run_config = orchestrator::RunConfig {
        repo: repo.to_path_buf(),
        goal: goal.to_string(),
        verify_cmd: verify_cmd.to_string(),
        integration_branch: "agentic-integration".to_string(),
    };
    orchestrator::run(&parsed, run_config, tx.clone()).await?;
    Ok(())
}

fn print_report(app: &App, repo: &std::path::Path) {
    println!("\n=== Run report ===");
    println!("Workspace: {}", app.workspace);
    println!("Goal: {}", app.goal);
    for epic in &app.epics {
        let status = match epic.status {
            app::EpicStatus::Merged => "merged",
            app::EpicStatus::Failed => "failed",
            app::EpicStatus::Skipped => "skipped",
            app::EpicStatus::Conflict => "conflict (manual merge)",
            _ => "incomplete",
        };
        println!("  [{status}] {} {}", epic.id, epic.title);
    }
    println!("Total cost ~${:.4}", app.total_cost);
    match app.phase {
        Phase::Done => {
            println!(
                "Merged work is on branch 'agentic-integration' in {}. Review and merge to your main branch.",
                repo.display()
            );
        }
        Phase::Failed => {
            if let Some(e) = &app.error {
                eprintln!("Run failed: {e}");
            }
        }
        _ => {}
    }
}
```

- [ ] **Step 7: Remove obsolete single-stage items from `src/config.rs`**

Delete these now-unused items: `BUDGET_USD`, `MODEL_PRD`, `PRD_TOOLS`, `PRD_MAX_TURNS`, the `Stage` struct, `prd_stage`, and `prd_prompt`. Keep `PERMISSION_MODE`, `STYLE`, all orchestrator knobs, `plan_prompt`, and `epic_prompt`.

- [ ] **Step 8: Build the whole crate**

Run: `cargo build 2>&1 | tail -20`
Expected: PASS. If clippy-style unused warnings remain, they are acceptable; hard errors are not.

- [ ] **Step 9: Run the full test suite**

Run: `cargo test -- --test-threads=1 2>&1 | tail -20`
Expected: PASS (workspace 5, plan 6, orchestrator 8, worktree 2).

- [ ] **Step 10: Commit**

```bash
git add src/event.rs src/app.rs src/engine.rs src/orchestrator.rs src/ui.rs src/main.rs src/config.rs
git commit -m "feat: switch pipeline to multi-stage orchestrator"
```

---

### Task 8: Docs, Makefile, gitignore, and end-to-end verification

**Files:**
- Modify: `README.md`
- Modify: `Makefile`
- Modify: `.gitignore`

**Interfaces:**
- Consumes: the finished binary.
- Produces: updated docs and a manual end-to-end check.

- [ ] **Step 1: Ignore orchestrator scratch artifacts**

Append to `.gitignore`:

```
# Orchestrator scratch state
/.agentic-plan.json
/.agentic-worktrees/
```

- [ ] **Step 2: Update the `Makefile` run target**

Replace the `run` target and its `GOAL` variable near the top with:

```makefile
GOAL ?= Add a health check endpoint
WORKSPACE ?=

.PHONY: run
run: ## Run the orchestrator (GOAL="..." WORKSPACE=name|path)
	$(CARGO) run -- "$(GOAL)" $(if $(WORKSPACE),--workspace "$(WORKSPACE)",)
```

Leave every other target (`build`, `release`, `check`, `fmt`, `fmt-check`, `lint`, `test`, `verify`, `clean`, `help`) unchanged.

- [ ] **Step 3: Rewrite `README.md`**

Replace the body with content describing the new flow: workspace picker, the `workspaces.toml` format, the three stages (Plan, Implement, Integrate), worktree isolation, verification via `VERIFY_CMD`, the config knobs in `src/config.rs`, and the `agentic-integration` output branch. Keep the prerequisites section (Claude Code CLI, subscription login, git, Rust toolchain). Use the direct, no-em-dash style. Include this sample the reader can copy into their config:

````markdown
```toml
# ~/.config/agentic-tui/workspaces.toml
[[workspace]]
name = "greentic"
path = "~/Works/personal/greentic"
```
````

- [ ] **Step 4: Run formatting, lint, and tests**

Run: `make verify 2>&1 | tail -20`
Expected: PASS (fmt-check, clippy with `-D warnings`, tests). Fix any clippy findings inline, then re-run until clean.

- [ ] **Step 5: End-to-end smoke test against a throwaway git repo**

Run:

```bash
mkdir -p /tmp/agentic-smoke && cd /tmp/agentic-smoke && git init -b main && printf '# smoke\n' > README.md && git add -A && git commit -m init
cd /Users/bimapangestu/Desktop/Works/personal/claude-agentic-loop
cargo run -- "Add a CONTRIBUTING.md with a short contribution guide" --workspace /tmp/agentic-smoke --verify "true"
```

Expected: the picker is skipped (explicit `--workspace`), a plan is produced, at least one epic runs in a worktree, and the report prints. Confirm `/tmp/agentic-smoke` has an `agentic-integration` branch (`git -C /tmp/agentic-smoke branch`). `--verify "true"` makes verification always pass for the smoke test.

If `claude` is not on PATH in this environment, record that the smoke test could not run and note it for the human, rather than marking it passed.

- [ ] **Step 6: Commit**

```bash
git add README.md Makefile .gitignore
git commit -m "docs: update README and Makefile for the orchestrator flow"
```

---

## Self-Review

**Spec coverage:**
- Workspace picker + `workspaces.toml` + `--workspace` — Tasks 2, 7. ✓
- Plan stage writing `plan.json` + schema — Tasks 6, 7. ✓
- plan.json parsing + validation + topology — Task 3. ✓
- Scheduler (deps, parallel cap, failure cascade, independent continuation) — Task 4. ✓
- Engine generic `run_stage` (cwd + tools) — Task 7. ✓
- Worktree per epic + merge + conflict detection — Task 5. ✓
- Orchestrator driver (parallel pool, retry, verify gate, ordered merge) — Task 7. ✓
- Verification via `VERIFY_CMD` in worktree — Tasks 7 (`run_verify`), 7/8 (`--verify`). ✓
- Failure policy (failed not merged, dependents skipped, independents continue, report) — Tasks 4, 7. ✓
- Autonomous + abort (`q` / Ctrl-C) — Task 7. ✓
- Config knobs + tool allowlists + new deps — Tasks 1, 6. ✓
- Error handling (missing config, non-git workspace, missing/invalid plan) — Tasks 2, 7. ✓
- README + Makefile — Task 8. ✓

**Green-build invariant:** Tasks 1-6 only add items; the old single-stage code keeps compiling. Task 7 replaces the coupled files and removes the obsolete config items in one commit. Every task ends with a passing `cargo build` (Tasks 1, 2, 5, 6, 7) or module tests plus build.

**Placeholder scan:** README rewrite (Task 8 Step 3) is described prose with an explicit section list and a concrete sample, not code — acceptable. All code steps contain complete code.

**Type consistency:** `StageSpec` fields are identical in engine (Task 7 Step 3), the driver (Task 7 Step 4), and `run_pipeline` (Task 7 Step 6). `EpicState` (scheduler-internal) and `EpicStatus` (UI view) are deliberately distinct. `AppEvent` variants match across `event.rs`, `app.rs`, `engine.rs`, and `orchestrator.rs`. `Scheduler` methods (`next_ready`, `mark_running`, `mark_succeeded`, `mark_failed`, `snapshot`, `is_done`, `running_count`, `state`) are defined in Task 4 and used in Task 7.

## Notes on verification strategy

The pure cores (workspace, plan, scheduler) are covered by unit tests. Worktree
is covered by integration tests against a temp git repo. The orchestrator async
driver, the engine subprocess, and the TUI are verified by `cargo build`,
`clippy`, and the Task 8 end-to-end smoke test, because they shell out to
`claude` and `git` and cannot be meaningfully unit tested without heavy mocking.
