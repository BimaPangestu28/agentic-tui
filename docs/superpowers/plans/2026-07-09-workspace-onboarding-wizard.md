# Workspace Onboarding Wizard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the `no workspaces configured` abort with an interactive onboarding wizard that auto-scans the filesystem for git repositories, lets the user pick some, saves them to `workspaces.toml`, and continues into the picker.

**Architecture:** Two pure, unit-tested helpers in `workspace.rs` (`scan_for_repos`, `save_workspaces`) do the filesystem and persistence work. The interactive layer is a blocking three-screen loop `run_onboarding` in `main.rs`, drawn by three new `ui.rs` render functions, wired into `resolve_workspace`. The picker gains an `a` hotkey that re-enters the wizard, mediated by a new `PickerOutcome` enum so screens never nest.

**Tech Stack:** Rust 2021, ratatui + crossterm (TUI), toml + serde (persistence), anyhow (errors).

## Global Constraints

- Edition 2021; keep `Cargo.lock` pinned; do not run `cargo update`.
- No `unwrap()` / `expect()` / `panic!` in production code paths (tests may use them).
- Comment/prose style: direct, no em dashes, no contractions in English prose.
- Descriptive names; verbs for functions, nouns for types.
- Every task leaves `make verify` (fmt-check, clippy `--all-targets -- -D warnings`, tests) green.
- **Binary-crate dead-code reality:** this is a `bin` crate, so an item referenced only from a `#[cfg(test)]` module is still dead in the normal binary build and `clippy -D warnings` fails on it. `scan_for_repos`, `save_workspaces`, their private helpers, the scan consts, and the `WorkspacesOut`/`RawWorkspaceOut` structs are therefore each annotated with `#[allow(dead_code)]` when introduced in Tasks 1 and 2, as temporary scaffolding. Task 3 wires all of them into the real binary code path and removes every one of these `#[allow(dead_code)]` attributes as its final implementation step, verifying the gate stays green without them. No `#[allow(dead_code)]` may remain after Task 3.
- Conventional commits (`feat:`, `refactor:`, `docs:`). Commit after every task.
- Work happens on the existing `feat/workspace-onboarding` branch.

## File Structure

| File | Change |
|---|---|
| `src/workspace.rs` | Add `DEFAULT_SCAN_DEPTH`, `MAX_SCAN_RESULTS` consts; `scan_for_repos`, `save_workspaces`, and private helpers `workspaces_from_paths`, `base_name`; a serializable output struct; unit tests. |
| `src/ui.rs` | Add `render_scan_root_input`, `render_scanning`, `render_repo_checklist`; add `a` to the picker title. |
| `src/main.rs` | Add `PickerOutcome` enum and `run_onboarding`; change `run_picker` to return `PickerOutcome`; rewrite `resolve_workspace` to onboard when empty and loop on `Add`. |
| `README.md` | Update "Configuring workspaces" and the picker keys; drop the empty-config error note. |

---

### Task 1: `scan_for_repos` filesystem scan

**Files:**
- Modify: `src/workspace.rs`

**Interfaces:**
- Consumes: existing `Workspace { name: String, path: PathBuf }`.
- Produces:
  - `pub const DEFAULT_SCAN_DEPTH: usize = 6;`
  - `pub const MAX_SCAN_RESULTS: usize = 500;`
  - `pub fn scan_for_repos(root: &Path, max_depth: usize) -> Vec<Workspace>`

- [ ] **Step 1: Add the `HashMap` import**

At the top of `src/workspace.rs` the imports are `use std::path::{Path, PathBuf};` and `use serde::Deserialize;`. Add below them:

```rust
use std::collections::HashMap;
```

- [ ] **Step 2: Write the failing tests**

Add these two tests inside the existing `#[cfg(test)] mod tests { ... }` block in `src/workspace.rs`:

```rust
#[test]
fn scan_finds_repos_and_prunes_noise() {
    let base = std::env::temp_dir().join(format!("scan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("repoA/.git")).unwrap();
    // A repo nested inside another repo must not be found: we do not descend
    // into a directory that is already a repo.
    std::fs::create_dir_all(base.join("repoA/inner/.git")).unwrap();
    std::fs::create_dir_all(base.join("group/repoB/.git")).unwrap();
    // Pruned locations: build noise and hidden directories.
    std::fs::create_dir_all(base.join("node_modules/ghost/.git")).unwrap();
    std::fs::create_dir_all(base.join(".hidden/repoC/.git")).unwrap();

    let found = scan_for_repos(&base, DEFAULT_SCAN_DEPTH);
    let paths: std::collections::HashSet<_> = found.iter().map(|w| w.path.clone()).collect();

    assert!(paths.contains(&base.join("repoA")));
    assert!(paths.contains(&base.join("group/repoB")));
    assert!(
        !paths.contains(&base.join("repoA/inner")),
        "must not descend into a repo"
    );
    assert!(
        !paths.contains(&base.join("node_modules/ghost")),
        "node_modules must be pruned"
    );
    assert!(
        !paths.contains(&base.join(".hidden/repoC")),
        "hidden directories must be pruned"
    );
    assert_eq!(found.len(), 2);

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn scan_disambiguates_duplicate_names() {
    let base = std::env::temp_dir().join(format!("scan-dup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("x/proj/.git")).unwrap();
    std::fs::create_dir_all(base.join("y/proj/.git")).unwrap();

    let found = scan_for_repos(&base, DEFAULT_SCAN_DEPTH);
    let mut names: Vec<_> = found.iter().map(|w| w.name.clone()).collect();
    names.sort();
    assert_eq!(names, vec!["x/proj".to_string(), "y/proj".to_string()]);

    let _ = std::fs::remove_dir_all(&base);
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test scan_ 2>&1 | tail -20`
Expected: FAIL to compile with "cannot find function `scan_for_repos`".

- [ ] **Step 4: Implement `scan_for_repos` and its helpers**

Add this above the `#[cfg(test)]` block in `src/workspace.rs` (after `validate`):

```rust
/// Maximum directory depth `scan_for_repos` descends from its root.
pub const DEFAULT_SCAN_DEPTH: usize = 6;

/// Upper bound on how many repositories a single scan returns, so a huge tree
/// cannot hang the wizard.
pub const MAX_SCAN_RESULTS: usize = 500;

/// Recursively scan `root` for git repositories, descending at most `max_depth`
/// directory levels. A directory that contains a `.git` entry is a repository
/// and is not descended into. Hidden directories (names starting with `.`) and
/// heavy build directories are pruned. Unreadable directories are skipped
/// silently. Results are sorted by path, capped at `MAX_SCAN_RESULTS`, and named
/// by their directory, disambiguated by parent directory on a name collision.
pub fn scan_for_repos(root: &Path, max_depth: usize) -> Vec<Workspace> {
    const PRUNE: [&str; 4] = ["node_modules", "target", "dist", "build"];
    let mut repos: Vec<PathBuf> = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if repos.len() >= MAX_SCAN_RESULTS {
            break;
        }
        if dir.join(".git").exists() {
            repos.push(dir);
            continue;
        }
        if depth >= max_depth {
            continue;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || PRUNE.contains(&name.as_ref()) {
                continue;
            }
            stack.push((path, depth + 1));
        }
    }
    repos.sort();
    repos.truncate(MAX_SCAN_RESULTS);
    workspaces_from_paths(repos)
}

/// The final path component as a display name, falling back to the whole path.
fn base_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

/// Turn repo paths into workspaces, disambiguating any names shared by more than
/// one repo with a `parent/name` label.
fn workspaces_from_paths(paths: Vec<PathBuf>) -> Vec<Workspace> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for path in &paths {
        *counts.entry(base_name(path)).or_insert(0) += 1;
    }
    paths
        .into_iter()
        .map(|path| {
            let base = base_name(&path);
            let name = if counts.get(&base).copied().unwrap_or(0) > 1 {
                match path.parent().and_then(|parent| parent.file_name()) {
                    Some(parent) => format!("{}/{}", parent.to_string_lossy(), base),
                    None => base,
                }
            } else {
                base
            };
            Workspace { name, path }
        })
        .collect()
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test scan_ 2>&1 | tail -20`
Expected: PASS for `scan_finds_repos_and_prunes_noise` and `scan_disambiguates_duplicate_names`.

- [ ] **Step 6: Verify the whole gate is green**

Run: `make verify`
Expected: fmt-check, clippy, and all tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/workspace.rs
git commit -m "feat: scan the filesystem for git repositories

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `save_workspaces` persistence

**Files:**
- Modify: `src/workspace.rs`

**Interfaces:**
- Consumes: existing `load_workspaces`, `Workspace`.
- Produces:
  - `pub fn save_workspaces(config_path: &Path, workspaces: &[Workspace]) -> anyhow::Result<()>`

- [ ] **Step 1: Add the `Serialize` and `HashSet` imports**

`src/workspace.rs` already has `use serde::Deserialize;`. Change it to bring in `Serialize` too:

```rust
use serde::{Deserialize, Serialize};
```

The `HashMap` import from Task 1 is present; add `HashSet` beside it:

```rust
use std::collections::{HashMap, HashSet};
```

- [ ] **Step 2: Write the failing test**

Add inside `#[cfg(test)] mod tests`:

```rust
#[test]
fn save_unions_with_existing_and_round_trips() {
    let dir = std::env::temp_dir().join(format!("save-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let config = dir.join("nested/workspaces.toml");

    let first = vec![
        Workspace { name: "a".to_string(), path: PathBuf::from("/tmp/a") },
        Workspace { name: "b".to_string(), path: PathBuf::from("/tmp/b") },
    ];
    save_workspaces(&config, &first).unwrap();
    let loaded = load_workspaces(&config).unwrap();
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0], first[0]);

    // A second save with an overlapping path unions by path; the existing name
    // for /tmp/b is kept, and /tmp/c is added.
    let more = vec![
        Workspace { name: "b-renamed".to_string(), path: PathBuf::from("/tmp/b") },
        Workspace { name: "c".to_string(), path: PathBuf::from("/tmp/c") },
    ];
    save_workspaces(&config, &more).unwrap();
    let loaded = load_workspaces(&config).unwrap();
    assert_eq!(loaded.len(), 3, "union should be a, b, c with no duplicate b");
    let b = loaded.iter().find(|w| w.path == PathBuf::from("/tmp/b")).unwrap();
    assert_eq!(b.name, "b", "existing name is kept on a path conflict");

    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test save_unions 2>&1 | tail -20`
Expected: FAIL to compile with "cannot find function `save_workspaces`".

- [ ] **Step 4: Implement `save_workspaces` and its output struct**

Add near the top of `src/workspace.rs`, just after the `RawWorkspace` struct:

Each item is marked `#[allow(dead_code)]` because it is dead in the normal binary build until Task 3 wires it in (see Global Constraints). Task 3 removes these attributes.

```rust
#[derive(Serialize)]
#[allow(dead_code)]
struct WorkspacesOut {
    workspace: Vec<RawWorkspaceOut>,
}

#[derive(Serialize)]
#[allow(dead_code)]
struct RawWorkspaceOut {
    name: String,
    path: String,
}
```

Add this function after `load_workspaces`:

```rust
/// Persist `workspaces` to `config_path`, merging with any entries already
/// saved there. Entries are unioned by path and the existing name wins on a
/// path conflict. The parent directory is created if it does not exist.
#[allow(dead_code)]
pub fn save_workspaces(config_path: &Path, workspaces: &[Workspace]) -> anyhow::Result<()> {
    let mut merged: Vec<Workspace> = load_workspaces(config_path).unwrap_or_default();
    let mut seen: HashSet<PathBuf> = merged.iter().map(|w| w.path.clone()).collect();
    for workspace in workspaces {
        if seen.insert(workspace.path.clone()) {
            merged.push(workspace.clone());
        }
    }

    let out = WorkspacesOut {
        workspace: merged
            .iter()
            .map(|w| RawWorkspaceOut {
                name: w.name.clone(),
                path: w.path.to_string_lossy().to_string(),
            })
            .collect(),
    };
    let text = toml::to_string(&out)?;

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(config_path, text)?;
    Ok(())
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test save_unions 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 6: Verify the whole gate is green**

Run: `make verify`
Expected: all green.

- [ ] **Step 7: Commit**

```bash
git add src/workspace.rs
git commit -m "feat: save workspaces to the config file, merging by path

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Wizard screens, loop, and picker wiring

This task lands the full interactive layer in one commit. It must be whole because each new piece (render functions, `run_onboarding`, `PickerOutcome`) is only used by the next; splitting would leave an unused function and fail clippy's dead-code check. There are no unit tests for the TUI, consistent with the rest of `ui.rs`; verification is a manual run.

**Files:**
- Modify: `src/ui.rs`
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `workspace::scan_for_repos`, `workspace::save_workspaces`, `workspace::load_workspaces`, `workspace::expand_tilde`, `workspace::default_config_path`, `workspace::DEFAULT_SCAN_DEPTH`, `Workspace`.
- Produces (ui):
  - `pub fn render_scan_root_input(f: &mut Frame, root: &str)`
  - `pub fn render_scanning(f: &mut Frame, root: &str)`
  - `pub fn render_repo_checklist(f: &mut Frame, repos: &[Workspace], selected: usize, checked: &[bool], root: &str)`
- Produces (main):
  - `enum PickerOutcome { Chosen(Workspace), Add, Quit }`
  - `fn run_onboarding(config_path: &std::path::Path) -> anyhow::Result<Option<Vec<Workspace>>>`
  - `fn run_picker(workspaces: &[Workspace]) -> anyhow::Result<PickerOutcome>` (changed return type)

- [ ] **Step 1: Add the three wizard render functions to `src/ui.rs`**

`src/ui.rs` already imports everything needed (`Layout`, `Constraint`, `Direction`, `Color`, `Modifier`, `Style`, `Line`, `Span`, `Block`, `Borders`, `List`, `ListItem`, `Paragraph`, `Wrap`, `Frame`) and `crate::workspace::Workspace`. Add these functions after `render_goal_input`:

```rust
/// Onboarding screen one: an editable path to scan for git repositories.
pub fn render_scan_root_input(f: &mut Frame, root: &str) {
    let area = f.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);
    let input = Paragraph::new(Line::from(vec![
        Span::raw(root.to_string()),
        Span::styled("\u{2588}", Style::default().fg(Color::Cyan)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Scan which folder for git repos? (Enter to scan, Esc to quit) "),
    );
    f.render_widget(input, rows[0]);
    let help = Paragraph::new(
        "No workspaces configured yet. Point at a folder and I will find the git repositories inside it.",
    )
    .wrap(Wrap { trim: true })
    .block(Block::default().borders(Borders::ALL).title(" Onboarding "));
    f.render_widget(help, rows[1]);
}

/// Onboarding screen two: a brief status while the scan runs.
pub fn render_scanning(f: &mut Frame, root: &str) {
    let area = f.area();
    let message = Paragraph::new(format!("Scanning {root} for git repositories..."))
        .block(Block::default().borders(Borders::ALL).title(" Onboarding "));
    f.render_widget(message, area);
}

/// Onboarding screen three: a checklist of found repositories.
pub fn render_repo_checklist(
    f: &mut Frame,
    repos: &[Workspace],
    selected: usize,
    checked: &[bool],
    root: &str,
) {
    let area = f.area();
    if repos.is_empty() {
        let message = Paragraph::new(format!(
            "No git repositories found under {root}.\nPress r to scan a different folder, or Esc to quit."
        ))
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" No repositories found "),
        );
        f.render_widget(message, area);
        return;
    }
    let items: Vec<ListItem> = repos
        .iter()
        .enumerate()
        .map(|(index, workspace)| {
            let mark = if checked.get(index).copied().unwrap_or(false) {
                "[x]"
            } else {
                "[ ]"
            };
            let cursor = if index == selected { ">" } else { " " };
            let style = if index == selected {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(vec![Span::styled(
                format!(
                    "{cursor} {mark} {}  {}",
                    workspace.name,
                    workspace.path.display()
                ),
                style,
            )]))
        })
        .collect();
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Pick workspaces (up/down, Space toggle, Enter save, r rescan, Esc quit) "),
    );
    f.render_widget(list, area);
}
```

- [ ] **Step 2: Show the `a` key in the picker title**

In `src/ui.rs`, in `render_picker`, change the title line:

```rust
            .title(" Select workspace (up/down, Enter, q to quit) "),
```

to:

```rust
            .title(" Select workspace (up/down, Enter, a to add, q to quit) "),
```

- [ ] **Step 3: Add the `PickerOutcome` enum in `src/main.rs`**

In `src/main.rs`, add this enum just above `fn resolve_workspace`:

```rust
/// What the workspace picker returned: a chosen workspace, a request to add more
/// via the onboarding wizard, or a quit.
enum PickerOutcome {
    Chosen(Workspace),
    Add,
    Quit,
}
```

- [ ] **Step 4: Change `run_picker` to return `PickerOutcome`**

Replace the whole `run_picker` function in `src/main.rs` with:

```rust
/// Blocking picker loop on its own alternate screen.
fn run_picker(workspaces: &[Workspace]) -> anyhow::Result<PickerOutcome> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;
    let mut selected = 0usize;
    let outcome = loop {
        terminal.draw(|f| ui::render_picker(f, workspaces, selected))?;
        if let Event::Key(key) = crossterm::event::read()? {
            match key.code {
                KeyCode::Up => selected = selected.saturating_sub(1),
                KeyCode::Down if selected + 1 < workspaces.len() => selected += 1,
                KeyCode::Enter => break PickerOutcome::Chosen(workspaces[selected].clone()),
                KeyCode::Char('a') => break PickerOutcome::Add,
                KeyCode::Char('q') => break PickerOutcome::Quit,
                _ => {}
            }
        }
    };
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(outcome)
}
```

Note the empty-list `bail!` is gone: `resolve_workspace` now guarantees a non-empty list before calling `run_picker`.

- [ ] **Step 5: Add `run_onboarding` in `src/main.rs`**

Add this function after `run_picker`:

```rust
/// Blocking onboarding wizard on its own alternate screen. Scans a folder for
/// git repositories, lets the user pick some, saves them, and returns the full
/// saved workspace list. Returns None if the user cancels.
fn run_onboarding(config_path: &std::path::Path) -> anyhow::Result<Option<Vec<Workspace>>> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    enum Screen {
        Root,
        List,
    }
    let mut screen = Screen::Root;
    let mut root = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let mut repos: Vec<Workspace> = Vec::new();
    let mut checked: Vec<bool> = Vec::new();
    let mut selected = 0usize;

    let result: Option<Vec<Workspace>> = loop {
        match screen {
            Screen::Root => {
                terminal.draw(|f| ui::render_scan_root_input(f, &root))?;
                if let Event::Key(key) = crossterm::event::read()? {
                    match (key.code, key.modifiers) {
                        (KeyCode::Char('c'), KeyModifiers::CONTROL) => break None,
                        (KeyCode::Esc, _) => break None,
                        (KeyCode::Enter, _) => {
                            let dir = workspace::expand_tilde(root.trim());
                            terminal.draw(|f| ui::render_scanning(f, &root))?;
                            repos = workspace::scan_for_repos(&dir, workspace::DEFAULT_SCAN_DEPTH);
                            checked = vec![false; repos.len()];
                            selected = 0;
                            screen = Screen::List;
                        }
                        (KeyCode::Backspace, _) => {
                            root.pop();
                        }
                        (KeyCode::Char(c), _) => root.push(c),
                        _ => {}
                    }
                }
            }
            Screen::List => {
                terminal.draw(|f| ui::render_repo_checklist(f, &repos, selected, &checked, &root))?;
                if let Event::Key(key) = crossterm::event::read()? {
                    match (key.code, key.modifiers) {
                        (KeyCode::Char('c'), KeyModifiers::CONTROL) => break None,
                        (KeyCode::Esc, _) => break None,
                        (KeyCode::Char('r'), _) => screen = Screen::Root,
                        (KeyCode::Up, _) => selected = selected.saturating_sub(1),
                        (KeyCode::Down, _) if selected + 1 < repos.len() => selected += 1,
                        (KeyCode::Char(' '), _) if !repos.is_empty() => {
                            checked[selected] = !checked[selected];
                        }
                        (KeyCode::Enter, _) => {
                            let picked: Vec<Workspace> = repos
                                .iter()
                                .zip(checked.iter())
                                .filter(|(_, &is_checked)| is_checked)
                                .map(|(workspace, _)| workspace.clone())
                                .collect();
                            if picked.is_empty() {
                                continue;
                            }
                            match workspace::save_workspaces(config_path, &picked) {
                                Ok(()) => {
                                    let all = workspace::load_workspaces(config_path)
                                        .unwrap_or(picked);
                                    break Some(all);
                                }
                                // Persist failed (for example a read-only disk):
                                // still proceed with the picks for this session.
                                Err(_) => break Some(picked),
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(result)
}
```

- [ ] **Step 6: Rewrite `resolve_workspace` to onboard and loop**

Replace the whole `resolve_workspace` function in `src/main.rs` with:

```rust
/// Resolve the chosen workspace. `--workspace` matches by name or path. With no
/// flag and an empty config, the onboarding wizard runs first; otherwise the
/// picker shows. The picker's `a` key re-enters the wizard and refreshes the
/// list. Returns None if the user quits.
fn resolve_workspace(args: &Args, workspaces: &[Workspace]) -> anyhow::Result<Option<Workspace>> {
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

    let config = workspace::default_config_path();
    let mut list: Vec<Workspace> = if workspaces.is_empty() {
        match run_onboarding(&config)? {
            Some(all) => all,
            None => return Ok(None),
        }
    } else {
        workspaces.to_vec()
    };

    loop {
        match run_picker(&list)? {
            PickerOutcome::Chosen(workspace) => return Ok(Some(workspace)),
            PickerOutcome::Quit => return Ok(None),
            PickerOutcome::Add => {
                if let Some(all) = run_onboarding(&config)? {
                    list = all;
                }
            }
        }
    }
}
```

- [ ] **Step 7: Remove the temporary dead-code allows**

Every item from Tasks 1 and 2 is now reached from the binary code path (`scan_for_repos`, `save_workspaces`, `load_workspaces`, `expand_tilde`, `default_config_path`, `DEFAULT_SCAN_DEPTH` via `run_onboarding`; the consts and structs via those functions), so the temporary scaffolding attributes are no longer needed. In `src/workspace.rs`, delete every `#[allow(dead_code)]` attribute that Tasks 1 and 2 added (on `DEFAULT_SCAN_DEPTH`, `MAX_SCAN_RESULTS`, `scan_for_repos`, `base_name`, `workspaces_from_paths`, `WorkspacesOut`, `RawWorkspaceOut`, and `save_workspaces`).

Verify none remain:

```bash
grep -rn "allow(dead_code)" src/
# expect: no output
```

- [ ] **Step 8: Verify the gate is green**

Run: `make verify`
Expected: fmt-check, clippy (no dead-code or unused warnings, and none suppressed), and all tests pass. If clippy now reports any item as dead, it means that item is not actually wired into the binary path yet; wire it rather than re-adding an allow.

- [ ] **Step 9: Manual verification of the first-run wizard**

The scan should never trigger a real Claude run during testing. Abort before that.

```bash
# Confirm no config exists yet (this session already verified it is absent).
ls ~/.config/agentic-tui/workspaces.toml   # expect: No such file or directory

# Build a tiny scannable tree with one real git repo.
mkdir -p /tmp/wiz-demo/sample && git -C /tmp/wiz-demo/sample init -q

cargo run -- "demo goal"
```

In the TUI:
1. Screen one shows an editable path prefilled with your home directory. Backspace it and type `/tmp/wiz-demo`, then press Enter.
2. Screen three lists `sample` with `[ ]`. Press Space to check it (`[x]`), then Enter.
3. The normal picker appears listing `sample`. Press `q` to quit here (do NOT press Enter, which would start a real run).

Then confirm the config was written:

```bash
cat ~/.config/agentic-tui/workspaces.toml
# expect a [[workspace]] block with name = "sample" and path = "/tmp/wiz-demo/sample"
```

- [ ] **Step 10: Manual verification of the `a` hotkey**

With the config now populated:

```bash
mkdir -p /tmp/wiz-demo/second && git -C /tmp/wiz-demo/second init -q
cargo run -- "demo goal"
```

1. The picker shows `sample`. Press `a`.
2. The wizard opens. Type `/tmp/wiz-demo`, Enter, check `second`, Enter.
3. The picker now lists both `sample` and `second`. Press `q`.

Confirm both are saved:

```bash
cat ~/.config/agentic-tui/workspaces.toml   # expect both sample and second
rm -rf /tmp/wiz-demo ~/.config/agentic-tui/workspaces.toml   # clean up the demo
```

- [ ] **Step 11: Commit**

```bash
git add src/ui.rs src/main.rs src/workspace.rs
git commit -m "feat: onboard workspaces with an interactive scan wizard

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Documentation

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Update the "Configuring workspaces" section**

In `README.md`, replace this paragraph:

```markdown
Add one `[[workspace]]` block per project. Every workspace path must be an
existing directory that is a git repository (it must contain a `.git`), or
the run fails with a clear error before any Claude session starts.
```

with:

```markdown
Add one `[[workspace]]` block per project. Every workspace path must be an
existing directory that is a git repository (it must contain a `.git`), or the
run fails with a clear error before any Claude session starts.

You do not have to write this file by hand. On the first run with no
`--workspace` and no config, an onboarding wizard scans a folder you choose
(your home directory by default) for git repositories, lets you pick the ones
you want with Space, and saves them here for you. From the picker you can press
`a` at any time to run the wizard again and add more.
```

- [ ] **Step 2: Update the picker key hint in the "Run" section**

In `README.md`, replace:

```markdown
With no `--workspace`, this opens the workspace picker (up/down, Enter, `q`
to quit). To skip the picker, pass a configured name or a raw path:
```

with:

```markdown
With no `--workspace`, this opens the workspace picker (up/down, Enter, `a` to
add a workspace, `q` to quit). The first time you run it with no configured
workspaces, the onboarding wizard runs instead. To skip both, pass a configured
name or a raw path:
```

- [ ] **Step 3: Verify the gate is green**

Run: `make verify`
Expected: all green (README changes do not affect the build, but keep the habit).

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: document the workspace onboarding wizard

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Notes for the implementer

- The scan runs synchronously on the wizard's blocking loop. With pruning and the
  `MAX_SCAN_RESULTS` cap this is fast enough that the brief `render_scanning`
  frame is the only feedback needed; do not add async or a spinner.
- `run_onboarding` and `run_picker` each fully set up and tear down their own
  alternate screen, so the `a`-key round trip in `resolve_workspace` never nests
  raw-mode or alternate-screen state.
- A malformed `workspaces.toml` makes `load_workspaces` return an error, which
  `main` already turns into an empty list via `unwrap_or_default`, so a broken
  config now routes into onboarding rather than aborting. This is acceptable.
