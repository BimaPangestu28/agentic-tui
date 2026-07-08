# Workspace Onboarding Wizard Design

## Problem

On first run with no `--workspace` and no `~/.config/agentic-tui/workspaces.toml`,
the tool aborts with `no workspaces configured`. A new user has no guided way to
register a workspace short of hand-writing TOML. The picker is also a dead end:
once it is showing, there is no way to add a repository without quitting and
editing the config file.

## Goal

Replace the "no workspaces configured" abort with an interactive onboarding
wizard that auto-scans the filesystem for git repositories, lets the user pick
one or more, saves them to `workspaces.toml`, and continues into the normal
picker. Add an `a` (add) hotkey to the picker so the wizard is reachable after
onboarding too.

## Non-goals

- No file-browser navigation (a flat scan result is enough).
- No editing or deleting of existing entries from the TUI.
- `--workspace <path>` and a populated config are unchanged.

## Flow

```
resolve_workspace:
  --workspace given                -> use it (unchanged)
  config has workspaces            -> run_picker (unchanged, now with `a` hotkey)
  config empty / missing           -> run_onboarding -> run_picker

run_onboarding (blocking, own alternate screen):
  [1] Root input   prefilled with $HOME, editable; Enter starts the scan; Esc quits
  [2] Scanning...  brief status while scan_for_repos walks the root
  [3] Checklist    scan results; Space toggles, Enter saves+continues,
                   r returns to [1], Esc quits
        |
        v
  save_workspaces(config_path, picked)  (merged with any existing entries)
        |
        v
  returns the full saved list to resolve_workspace, which feeds run_picker
```

- Zero results: screen [3] shows "no git repositories found under <root>" with
  the same `r` / `Esc` keys. Never an error, never a crash.
- Cancel at any screen returns `None`; `main` prints `no workspace selected` and
  exits 0, matching today's picker-quit behavior.

## Components

### `workspace.rs`

- `pub fn scan_for_repos(root: &Path, max_depth: usize) -> Vec<Workspace>`
  - Walks `root` (sync `std::fs`) collecting directories that contain a `.git`
    entry. A matched repo is recorded and not descended into (no nested
    workspaces).
  - Prunes: `.git`, `node_modules`, `target`, `dist`, `build`, and any hidden
    directory (name starts with `.`). Unreadable directories are skipped
    silently (best-effort, permission errors are not fatal).
  - Name is the repo directory's file name. On duplicate names, disambiguate by
    appending the parent directory name.
  - Sorted by path; capped at `MAX_SCAN_RESULTS` (500) so a huge tree cannot
    hang the wizard.
- `pub fn save_workspaces(config_path: &Path, workspaces: &[Workspace]) -> anyhow::Result<()>`
  - Loads any existing entries, merges by path (union, existing wins on name),
    creates the parent directory if missing, and serializes to TOML.
- `MAX_SCAN_RESULTS` and `DEFAULT_SCAN_DEPTH` (6) as module consts.

### `ui.rs`

- `pub fn render_scan_root_input(f, root_buffer: &str)` — one-line editable path
  input, same border/cursor style as `render_goal_input`.
- `pub fn render_repo_checklist(f, repos: &[Workspace], selected: usize, checked: &[bool], root: &str)`
  — list with `[x]` / `[ ]` markers and a cursor, reusing the picker's list
  style. Shows the empty-result message when `repos` is empty.

### `main.rs`

- `fn run_onboarding(config_path: &Path) -> anyhow::Result<Option<Vec<Workspace>>>`
  — the three-screen blocking loop on its own alternate screen, mirroring
  `run_picker` / `run_goal_input`.
- `resolve_workspace` calls `run_onboarding` when the loaded list is empty and
  feeds the result into `run_picker`.
- `run_picker` gains an `a` key: it calls `run_onboarding`, merges/saves the
  result into its working list, and redraws. The picker owns a mutable
  `Vec<Workspace>` so it can grow in place.

## Error handling

- Scan: permission or IO errors on a subtree are skipped, not surfaced.
- Save: a write failure shows a clear message; the wizard still returns the
  picked list so the current session proceeds without persistence.
- Chosen workspaces still pass the existing `validate` before a run starts, so a
  repo that stops being a git repo between scan and run fails with today's clear
  error.

## Testing

- `scan_for_repos`: build a temp tree with fake repos (dir + `.git`), nested
  repos, and pruned dirs (`node_modules`, hidden); assert detection, pruning,
  no-descend-into-repo, and name disambiguation.
- `save_workspaces`: save a list, `load_workspaces` it back, assert round-trip;
  save again with an overlapping path, assert the union has no duplicates.
- TUI render functions are not unit-tested, consistent with the rest of `ui.rs`.

## Files

| File | Change |
|---|---|
| `src/workspace.rs` | add `scan_for_repos`, `save_workspaces`, consts; tests |
| `src/ui.rs` | add `render_scan_root_input`, `render_repo_checklist` |
| `src/main.rs` | add `run_onboarding`; wire into `resolve_workspace`; `a` hotkey in `run_picker` |
| `README.md` | update "Configuring workspaces"; drop the old empty-config error note |
