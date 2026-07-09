# Configurable Base and Integration Branch Design

## Problem

Two branch choices are hardcoded in the orchestrator:

- Independent epics are always created from `HEAD` (`orchestrator.rs:190-191`), so
  a run is rooted at whatever branch the workspace happens to have checked out.
- Passing epics are always merged into `agentic-integration`, which is created
  from `HEAD` on first use (`worktree.rs`).

A user cannot say "base this run on `develop`" or "merge the passing work into
`<some branch>`" without checking out a different branch first and editing code.

## Goal

Make both configurable:

- **base** (the ref new epic worktrees and the integration branch start from),
  defaulting to `HEAD`.
- **integration** (the branch passing epics merge into), defaulting to
  `agentic-integration`.

Each is resolvable from a CLI flag, a per-workspace config field, or the
built-in default, with precedence **flag > workspace config > default**.

## Non-goals

- No change to how epics are scheduled, verified, or how dependent epics inherit
  merged work (they still base off the integration branch).
- No new safety gate on the merge target: if the integration branch already
  exists, passing epics merge into it as-is (including a real branch such as
  `develop`), by explicit choice.

## Behavior

- Independent epics are created from the resolved **base** ref instead of the
  literal `"HEAD"`. Dependent epics still base off the integration branch.
- The integration branch, when it does not exist yet, is created from the
  resolved **base** ref (not `HEAD`), so a run based on `develop` produces an
  integration branch rooted at `develop`. When it already exists, it is used
  as-is and passing epics merge into it.
- **Fail-fast validation:** before any `claude` session starts, the resolved
  base ref must resolve in the repo (`git rev-parse --verify`). An invalid base
  aborts the run with a clear error. The integration branch is not required to
  exist (it is created from base when missing).
- The default resolution preserves today's behavior exactly: with no flag and no
  config, base is `HEAD` and integration is `agentic-integration`.

## Configuration surface

- **CLI:** `--base <ref>` and `--into <branch>`.
- **`workspaces.toml`:** optional `base` and `integration` fields per
  `[[workspace]]`. Omitted when unset; onboarding-scanned workspaces leave them
  unset.

```toml
[[workspace]]
name = "greentic"
path = "~/Works/greentic"
base = "develop"              # optional
integration = "agentic-wip"   # optional
```

Resolution per field: `flag.or(workspace_field).unwrap_or(default)`.

## Components

### `src/workspace.rs`

- `Workspace` gains `base: Option<String>` and `integration: Option<String>`.
- `RawWorkspace` (deserialize) gains `#[serde(default)] base: Option<String>` and
  `integration: Option<String>`.
- `RawWorkspaceOut` (serialize) gains the two fields with
  `#[serde(skip_serializing_if = "Option::is_none")]` so a workspace without them
  serializes to just `name`/`path` (clean TOML, no `base = ""`).
- `save_workspaces` carries the fields through the merge so a hand-set
  `base`/`integration` survives an onboarding re-save. Scan-produced workspaces
  set both to `None`.
- Existing `Workspace` literals in the code and tests gain the two `None` fields.

### `src/main.rs`

- `Args` gains `base: Option<String>` and `into: Option<String>`; `parse_args`
  reads `--base <v>` and `--into <v>`.
- A pure helper `resolve_setting(flag: Option<&str>, configured: Option<&str>,
  default: &str) -> String` implements the precedence and is unit-tested.
- After the workspace is chosen, resolve `base_ref` and `integration` from
  (flag, workspace field, default) and pass both into `run_pipeline` ->
  `RunConfig`.
- Before the plan stage, validate the base ref (see worktree helper); abort with
  a clear error if it does not resolve.
- `print_report` reports the actual integration branch name, not the literal
  `agentic-integration`.

### `src/orchestrator.rs`

- `RunConfig` gains `pub base_ref: String`.
- Independent epics use `config.base_ref.clone()` instead of `"HEAD".to_string()`
  (`orchestrator.rs:190-191`).
- The `merge_into` call passes `config.base_ref` so the integration branch is
  created from base when missing.

### `src/worktree.rs`

- `merge_into(repo, branch, integration_branch, base_ref)` gains a `base_ref`
  parameter; the "create the integration branch on first use" step uses
  `base_ref` instead of the literal `"HEAD"`.
- Add `pub async fn verify_ref(repo: &Path, reference: &str) -> anyhow::Result<()>`
  that runs `git rev-parse --verify <reference>` and returns a clear error if the
  ref does not resolve. Called from `main` before the run.

### `README.md`

- Document `--base`/`--into`, the `base`/`integration` config fields, the
  precedence, and that targeting an existing branch merges into it directly.

## Error handling

- Invalid base ref: `verify_ref` fails, the run aborts before any `claude`
  session, with a message naming the ref and the workspace.
- A malformed or missing integration branch is never an error: it is created
  from base. A merge conflict into the integration branch is reported exactly as
  today (kept on the epic branch for a manual merge).

## Testing

- `resolve_setting`: flag wins over config and default; config wins over default;
  default when both absent. Pure unit tests.
- `workspace.rs`: `save_workspaces` round-trips `base`/`integration` (set and
  unset); a config with the fields parses; a config without them defaults both to
  `None`; a serialized workspace without the fields has no `base`/`integration`
  keys.
- `worktree.rs`: an integration branch created from a non-HEAD base inherits that
  base's content (create a `develop` branch with a marker file, run an epic based
  on it, assert the integration branch has the marker); merging into a
  pre-existing integration branch merges into it rather than recreating it.
- `worktree::verify_ref`: succeeds for an existing ref, errors for a missing one.

## Files

| File | Change |
|---|---|
| `src/workspace.rs` | `base`/`integration` on `Workspace` and raw structs; round-trip in `save_workspaces`; tests |
| `src/main.rs` | `--base`/`--into` args; `resolve_setting`; resolve and pass both; validate base; report actual branch |
| `src/orchestrator.rs` | `RunConfig.base_ref`; independent epics use it; pass base to `merge_into` |
| `src/worktree.rs` | `merge_into` base_ref param; `verify_ref`; tests |
| `README.md` | document flags, config fields, precedence, and direct-merge behavior |
