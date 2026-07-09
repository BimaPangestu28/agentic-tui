# Configurable Base and Integration Branch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the worktree base ref and the merge target branch configurable via `--base`/`--into` flags and per-workspace `base`/`integration` config, precedence flag > config > default, preserving today's `HEAD` / `agentic-integration` behavior when unset.

**Architecture:** `base`/`integration` become optional fields on `Workspace` (and round-trip through the config). `main` resolves each by precedence into `RunConfig`, which the orchestrator already threads to `worktree::create`/`merge_into`. Independent epics and the first-time integration-branch creation switch from the literal `"HEAD"` to the resolved base ref. The base ref is validated up front.

**Tech Stack:** Rust 2021, serde/toml (config), tokio (async git), anyhow (errors).

## Global Constraints

- Edition 2021; keep `Cargo.lock` pinned; do not run `cargo update`.
- No `unwrap()`/`expect()`/`panic!` in production code (tests may use them).
- Comment/prose style: no em dashes, no contractions in English prose.
- Descriptive names; verbs for functions, nouns for types.
- Every task leaves `make verify` (fmt-check, clippy `--all-targets -- -D warnings`, tests) green. Adding the two `Workspace` fields will make the compiler flag every `Workspace { ... }` literal and every `RunConfig { ... }` literal that omits the new field; fix each one the compiler reports (there is no need to hunt them manually).
- No `#[allow(dead_code)]` is needed: Task 1's new fields are read by the serialize/parse paths, and Task 2 wires everything it adds.
- Conventional commits. Commit after every task. Work on branch `feat/configurable-branches`.

## File Structure

| File | Change |
|---|---|
| `src/workspace.rs` | `base`/`integration` on `Workspace`, `RawWorkspace`, `RawWorkspaceOut`; carry them through parse and `save_workspaces`; tests |
| `src/worktree.rs` | `merge_into` gains a `base_ref` param (integration branch created from it); add `verify_ref`; tests |
| `src/orchestrator.rs` | `RunConfig.base_ref`; independent epics use it; pass base to `merge_into` |
| `src/main.rs` | `--base`/`--into` args; `resolve_setting`; resolve and thread both; validate base; report actual branch |
| `README.md` | document flags, config fields, precedence, direct-merge behavior |

---

### Task 1: Workspace base and integration fields

**Files:**
- Modify: `src/workspace.rs`
- Modify: `src/main.rs` (one `Workspace` literal)

**Interfaces:**
- Produces: `Workspace { name, path, base: Option<String>, integration: Option<String> }`; parse and `save_workspaces` carry both.

- [ ] **Step 1: Write failing tests**

Add these to the `#[cfg(test)] mod tests` block in `src/workspace.rs`:

```rust
#[test]
fn parses_optional_base_and_integration() {
    let toml_text = r#"
[[workspace]]
name = "a"
path = "/tmp/a"
base = "develop"
integration = "agentic-wip"

[[workspace]]
name = "b"
path = "/tmp/b"
"#;
    let ws = parse_workspaces_str(toml_text).unwrap();
    assert_eq!(ws[0].base.as_deref(), Some("develop"));
    assert_eq!(ws[0].integration.as_deref(), Some("agentic-wip"));
    assert_eq!(ws[1].base, None);
    assert_eq!(ws[1].integration, None);
}

#[test]
fn save_round_trips_base_and_integration() {
    let dir = std::env::temp_dir().join(format!("save-branches-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let config = dir.join("workspaces.toml");
    let list = vec![
        Workspace {
            name: "a".to_string(),
            path: PathBuf::from("/tmp/a"),
            base: Some("develop".to_string()),
            integration: Some("agentic-wip".to_string()),
        },
        Workspace {
            name: "b".to_string(),
            path: PathBuf::from("/tmp/b"),
            base: None,
            integration: None,
        },
    ];
    save_workspaces(&config, &list).unwrap();
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(
        !text.contains("base = \"\""),
        "an unset field must not serialize as an empty key"
    );

    let loaded = load_workspaces(&config).unwrap();
    let a = loaded.iter().find(|w| w.name == "a").unwrap();
    assert_eq!(a.base.as_deref(), Some("develop"));
    assert_eq!(a.integration.as_deref(), Some("agentic-wip"));
    let b = loaded.iter().find(|w| w.name == "b").unwrap();
    assert_eq!(b.base, None);
    assert_eq!(b.integration, None);

    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --bin agentic-tui workspace:: 2>&1 | tail -20`
Expected: compile failure (the `Workspace` literals reference fields `base`/`integration` that do not exist yet).

- [ ] **Step 3: Add the fields to the structs**

In `src/workspace.rs`, change `Workspace`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Workspace {
    pub name: String,
    pub path: PathBuf,
    pub base: Option<String>,
    pub integration: Option<String>,
}
```

Change `RawWorkspace`:

```rust
#[derive(Debug, Deserialize)]
struct RawWorkspace {
    name: String,
    path: String,
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    integration: Option<String>,
}
```

Change `RawWorkspaceOut`:

```rust
#[derive(Serialize)]
struct RawWorkspaceOut {
    name: String,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    integration: Option<String>,
}
```

- [ ] **Step 4: Carry the fields through parse and save**

In `parse_workspaces_str`, change the map closure:

```rust
        .map(|raw| Workspace {
            name: raw.name,
            path: expand_tilde(&raw.path),
            base: raw.base,
            integration: raw.integration,
        })
```

In `save_workspaces`, change the `RawWorkspaceOut` map closure:

```rust
            .map(|w| RawWorkspaceOut {
                name: w.name.clone(),
                path: w.path.to_string_lossy().to_string(),
                base: w.base.clone(),
                integration: w.integration.clone(),
            })
```

- [ ] **Step 5: Fix every remaining `Workspace` literal the compiler flags**

Run `cargo build 2>&1 | tail -30` and add `base: None, integration: None` to each `Workspace { ... }` literal that the compiler reports as missing fields. These are:

- `src/workspace.rs` in `workspaces_from_paths`: `Workspace { name, path }` becomes `Workspace { name, path, base: None, integration: None }`.
- `src/main.rs` in `resolve_workspace` (the raw `--workspace <path>` branch): `Workspace { name, path }` becomes `Workspace { name, path, base: None, integration: None }`.
- Any existing test literal in `src/workspace.rs` (for example in `validate_rejects_a_path_that_is_not_a_directory` and `save_unions_with_existing_and_round_trips`): add `base: None, integration: None`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --bin agentic-tui workspace:: 2>&1 | tail -20`
Expected: PASS, including the two new tests.

- [ ] **Step 7: Verify the gate is green**

Run: `make verify`
Expected: fmt-check, clippy, and all tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/workspace.rs src/main.rs
git commit -m "feat: add optional base and integration fields to a workspace

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Thread the base ref and integration branch through the run

This task changes `worktree::merge_into`'s signature, `RunConfig`, the orchestrator's epic/merge calls, and `main`'s resolution and validation together, because they are compile-coupled: changing the signature or struct alone would not build. Everything it adds is used, so no dead-code scaffolding is needed.

**Files:**
- Modify: `src/worktree.rs`
- Modify: `src/orchestrator.rs`
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `Workspace.base`/`Workspace.integration` (Task 1).
- Produces:
  - `worktree::merge_into(repo, branch, integration_branch, base_ref)`.
  - `worktree::verify_ref(repo, reference) -> anyhow::Result<()>`.
  - `orchestrator::RunConfig.base_ref: String`.
  - `main`: `Args.base`/`Args.into`; `resolve_setting(flag, configured, default) -> String`; `run_pipeline(repo, goal, verify_cmd, base_ref, integration, refine_cost, tx)`.

- [ ] **Step 1: `merge_into` creates the integration branch from a base ref**

In `src/worktree.rs`, change `merge_into`'s signature and the first-use creation. The signature:

```rust
pub async fn merge_into(
    repo: &Path,
    branch: &str,
    integration_branch: &str,
) -> anyhow::Result<MergeResult> {
```

becomes:

```rust
pub async fn merge_into(
    repo: &Path,
    branch: &str,
    integration_branch: &str,
    base_ref: &str,
) -> anyhow::Result<MergeResult> {
```

Find the first-use creation block:

```rust
    // Create the integration branch from HEAD on first use.
    let integration_exists = run_git(repo, &["rev-parse", "--verify", integration_branch])
        .await?
        .status
        .success();
    if !integration_exists {
        run_git_checked(repo, &["branch", integration_branch, "HEAD"]).await?;
    }
```

Change it to use `base_ref`:

```rust
    // Create the integration branch from the base ref on first use.
    let integration_exists = run_git(repo, &["rev-parse", "--verify", integration_branch])
        .await?
        .status
        .success();
    if !integration_exists {
        run_git_checked(repo, &["branch", integration_branch, base_ref]).await?;
    }
```

- [ ] **Step 2: Add `verify_ref`**

In `src/worktree.rs`, add after `merge_into`:

```rust
/// Confirm a git ref resolves in the repo, so an invalid base fails the run
/// before any session starts.
pub async fn verify_ref(repo: &Path, reference: &str) -> anyhow::Result<()> {
    let resolves = run_git(repo, &["rev-parse", "--verify", reference])
        .await?
        .status
        .success();
    if !resolves {
        anyhow::bail!("base ref does not resolve in the repository: {reference}");
    }
    Ok(())
}
```

- [ ] **Step 3: Update the worktree tests for the new signature and add coverage**

Every existing `merge_into(&tmp, ..., "integration")` call in `src/worktree.rs` tests now needs a fourth argument (the compiler will flag each one). Pass `"HEAD"` to each to keep its behavior (for example `merge_into(&tmp, &wt.branch, "integration", "HEAD")`).

Then add two tests inside the `#[cfg(test)] mod tests` block:

```rust
#[tokio::test]
async fn integration_branch_is_created_from_the_base_ref() {
    let tmp = std::env::temp_dir().join(format!("wt-basref-{}", std::process::id()));
    let _ = tokio::fs::remove_dir_all(&tmp).await;
    tokio::fs::create_dir_all(&tmp).await.unwrap();
    init_repo(&tmp).await;

    // A develop branch that main does not have.
    git(&tmp, &["branch", "develop"]).await;
    git(&tmp, &["checkout", "develop"]).await;
    tokio::fs::write(tmp.join("develop_only.txt"), "d\n")
        .await
        .unwrap();
    git(&tmp, &["add", "-A"]).await;
    git(&tmp, &["commit", "-m", "develop"]).await;
    git(&tmp, &["checkout", "main"]).await;

    // An epic based on develop, merged with base_ref = develop.
    let wt = create(&tmp, "epic-1", "develop").await.unwrap();
    tokio::fs::write(wt.path.join("feature.txt"), "f\n")
        .await
        .unwrap();
    git(&wt.path, &["add", "-A"]).await;
    git(&wt.path, &["commit", "-m", "epic-1"]).await;
    assert_eq!(
        merge_into(&tmp, &wt.branch, "integration", "develop")
            .await
            .unwrap(),
        MergeResult::Merged
    );

    // The integration branch was rooted at develop, so it carries develop_only.txt
    // (which main lacks) as well as the epic's feature.txt.
    let integ = tmp.join(".agentic-worktrees/.integration");
    assert!(
        integ.join("develop_only.txt").exists(),
        "integration must be created from the base ref (develop)"
    );
    assert!(integ.join("feature.txt").exists());

    let _ = tokio::fs::remove_dir_all(&tmp).await;
}

#[tokio::test]
async fn verify_ref_accepts_a_real_ref_and_rejects_a_missing_one() {
    let tmp = std::env::temp_dir().join(format!("wt-verify-{}", std::process::id()));
    let _ = tokio::fs::remove_dir_all(&tmp).await;
    tokio::fs::create_dir_all(&tmp).await.unwrap();
    init_repo(&tmp).await;

    assert!(verify_ref(&tmp, "HEAD").await.is_ok());
    assert!(verify_ref(&tmp, "no-such-branch").await.is_err());

    let _ = tokio::fs::remove_dir_all(&tmp).await;
}
```

- [ ] **Step 4: Add `base_ref` to `RunConfig` and use it in the orchestrator**

In `src/orchestrator.rs`, add the field to `RunConfig`:

```rust
pub struct RunConfig {
    pub repo: PathBuf,
    pub goal: String,
    pub verify_cmd: String,
    pub integration_branch: String,
    pub base_ref: String,
    pub budget_usd: f64,
    pub initial_cost: f64,
}
```

Change the independent-epic base (the `let base_ref = if epic.depends_on.is_empty()` block):

```rust
        let base_ref = if epic.depends_on.is_empty() {
            config.base_ref.clone()
        } else {
            config.integration_branch.clone()
        };
```

Change the `merge_into` call to pass the base ref:

```rust
                            worktree::merge_into(
                                &config.repo,
                                &wt.branch,
                                &config.integration_branch,
                                &config.base_ref,
                            )
```

If any `RunConfig { ... }` literal in the orchestrator tests now fails to compile (missing `base_ref`), add `base_ref: "HEAD".to_string()` to it.

- [ ] **Step 5: Parse `--base`/`--into` in `main`**

In `src/main.rs`, add to `Args`:

```rust
struct Args {
    goal: String,
    workspace: Option<String>,
    verify: Option<String>,
    no_refine: bool,
    base: Option<String>,
    into: Option<String>,
}
```

In `parse_args`, add the two flags (mirroring `--workspace`) and include them in the returned `Args`:

```rust
    let mut verify = None;
    let mut no_refine = false;
    let mut base = None;
    let mut into = None;
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
            "--base" => {
                i += 1;
                base = raw.get(i).cloned();
            }
            "--into" => {
                i += 1;
                into = raw.get(i).cloned();
            }
            "--no-refine" => no_refine = true,
            other => goal_parts.push(other.to_string()),
        }
        i += 1;
    }
    let goal = goal_parts.join(" ").trim().to_string();
    Some(Args {
        goal,
        workspace,
        verify,
        no_refine,
        base,
        into,
    })
```

Update the usage string in `main` to include the flags:

```rust
            eprintln!(
                "usage: agentic-tui [\"<goal>\"] [--workspace <name|path>] [--verify \"<cmd>\"] [--base <ref>] [--into <branch>] [--no-refine]"
            );
```

- [ ] **Step 6: Add `resolve_setting` and a test**

In `src/main.rs`, add the helper (near `parse_args`):

```rust
/// Resolve a setting by precedence: the CLI flag, then the workspace config,
/// then the built-in default.
fn resolve_setting(flag: Option<&str>, configured: Option<&str>, default: &str) -> String {
    flag.or(configured).unwrap_or(default).to_string()
}
```

Add a test module at the end of `src/main.rs` (there is none yet):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_setting_prefers_flag_then_config_then_default() {
        assert_eq!(
            resolve_setting(Some("flag"), Some("config"), "default"),
            "flag"
        );
        assert_eq!(resolve_setting(None, Some("config"), "default"), "config");
        assert_eq!(resolve_setting(None, None, "default"), "default");
    }
}
```

- [ ] **Step 7: Resolve and validate the branches in `main`**

In `main`, the block after the workspace is chosen currently is:

```rust
    workspace::validate(&selected)?;
    let repo = selected
        .path
        .canonicalize()
        .unwrap_or(selected.path.clone());
    let verify_cmd = args
        .verify
        .clone()
        .unwrap_or_else(|| config::DEFAULT_VERIFY_CMD.to_string());
```

Add resolution and base validation immediately after it:

```rust
    let base_ref = resolve_setting(args.base.as_deref(), selected.base.as_deref(), "HEAD");
    let integration = resolve_setting(
        args.into.as_deref(),
        selected.integration.as_deref(),
        "agentic-integration",
    );
    worktree::verify_ref(&repo, &base_ref).await?;
```

- [ ] **Step 8: Thread the branches into `run_pipeline` and the report**

Find the pipeline spawn:

```rust
    let pipeline_tx = tx.clone();
    let repo_run = repo.clone();
    let goal_run = goal.clone();
    let verify_run = verify_cmd.clone();
    let pipeline_handle = tokio::spawn(async move {
        if let Err(e) =
            run_pipeline(&repo_run, &goal_run, &verify_run, refine_cost, &pipeline_tx).await
        {
            let _ = pipeline_tx.send(AppEvent::Fatal(e.to_string()));
        }
    });
```

Change it to capture and pass the two branches:

```rust
    let pipeline_tx = tx.clone();
    let repo_run = repo.clone();
    let goal_run = goal.clone();
    let verify_run = verify_cmd.clone();
    let base_run = base_ref.clone();
    let integration_run = integration.clone();
    let pipeline_handle = tokio::spawn(async move {
        if let Err(e) = run_pipeline(
            &repo_run,
            &goal_run,
            &verify_run,
            &base_run,
            &integration_run,
            refine_cost,
            &pipeline_tx,
        )
        .await
        {
            let _ = pipeline_tx.send(AppEvent::Fatal(e.to_string()));
        }
    });
```

Change `run_pipeline`'s signature:

```rust
async fn run_pipeline(
    repo: &std::path::Path,
    goal: &str,
    verify_cmd: &str,
    refine_cost: f64,
    tx: &mpsc::UnboundedSender<AppEvent>,
) -> anyhow::Result<()> {
```

to:

```rust
async fn run_pipeline(
    repo: &std::path::Path,
    goal: &str,
    verify_cmd: &str,
    base_ref: &str,
    integration: &str,
    refine_cost: f64,
    tx: &mpsc::UnboundedSender<AppEvent>,
) -> anyhow::Result<()> {
```

Change the `RunConfig` construction inside `run_pipeline`:

```rust
    let run_config = orchestrator::RunConfig {
        repo: repo.to_path_buf(),
        goal: goal.to_string(),
        verify_cmd: verify_cmd.to_string(),
        integration_branch: integration.to_string(),
        base_ref: base_ref.to_string(),
        budget_usd: config::GLOBAL_BUDGET_USD,
        initial_cost: refine_cost + outcome.cost,
    };
```

Change the report call `print_report(&app, &repo);` to pass the integration branch:

```rust
    print_report(&app, &repo, &integration);
```

Change `print_report`'s signature and the merged-work line. The signature:

```rust
fn print_report(app: &App, repo: &std::path::Path) {
```

becomes:

```rust
fn print_report(app: &App, repo: &std::path::Path, integration: &str) {
```

and the merged-work message:

```rust
                "Merged work is on branch 'agentic-integration' in {}. Review and merge to your main branch.",
                repo.display()
```

becomes:

```rust
                "Merged work is on branch '{integration}' in {}. Review and merge to your main branch.",
                repo.display()
```

- [ ] **Step 9: Verify the gate is green**

Run: `make verify`
Expected: fmt-check, clippy, and all tests pass (including the new worktree and `resolve_setting` tests).

- [ ] **Step 10: Commit**

```bash
git add src/worktree.rs src/orchestrator.rs src/main.rs
git commit -m "feat: configurable base ref and integration branch per run

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Documentation

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Document the flags in the Run section**

In `README.md`, after the `--verify` example block in the Run section, add:

```markdown
By default each run branches its epic worktrees from the workspace `HEAD` and
merges passing epics into `agentic-integration`. Override either with a flag:

```bash
cargo run -- "Add a health check endpoint" --workspace greentic --base develop --into agentic-wip
```

`--base <ref>` is the branch, tag, or commit new epic worktrees start from (and
the integration branch is created from it on first use). `--into <branch>` is
where passing epics merge; if that branch already exists, the work merges into
it directly, so pointing it at a real branch such as `develop` merges there
without a manual review step. An invalid `--base` aborts the run before any
Claude session starts.
```

- [ ] **Step 2: Document the config fields**

In `README.md`, in the "Configuring workspaces" section, extend the example and note the optional fields. Replace the example block:

```markdown
```toml
# ~/.config/agentic-tui/workspaces.toml
[[workspace]]
name = "greentic"
path = "~/Works/personal/greentic"
```
```

with:

```markdown
```toml
# ~/.config/agentic-tui/workspaces.toml
[[workspace]]
name = "greentic"
path = "~/Works/personal/greentic"
base = "develop"              # optional: worktree base ref (default HEAD)
integration = "agentic-wip"   # optional: merge target (default agentic-integration)
```

`base` and `integration` are optional per-workspace defaults. A `--base` or
`--into` flag on the command line overrides the matching field for that run.
```

- [ ] **Step 3: Verify the gate is green**

Run: `make verify`
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: document configurable base and integration branches

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Notes for the implementer

- The whole feature is testable without the TUI: the base/merge behavior is git
  operations covered by async worktree tests, and `resolve_setting` is a pure
  function. No manual smoke test is required for the gate, though the controller
  may run one.
- Preserve today's behavior exactly when nothing is set: `resolve_setting`
  returns `HEAD` / `agentic-integration`, and every existing worktree test passes
  `"HEAD"` to `merge_into`.
- Do not change how dependent epics base off the integration branch, or how merge
  conflicts are reported.
