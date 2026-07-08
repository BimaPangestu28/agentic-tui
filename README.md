# Agentic Orchestrator TUI

![Rust](https://img.shields.io/badge/Rust-1.75%2B-orange?logo=rust&logoColor=white)
![TUI](https://img.shields.io/badge/TUI-ratatui-1f6feb)
![Async](https://img.shields.io/badge/async-tokio-000000)
![Requires](https://img.shields.io/badge/requires-Claude%20Code%20CLI-8a2be2)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey)
![Version](https://img.shields.io/badge/version-0.1.0-blue)

A Rust + ratatui tool that takes a goal, breaks it into epics with Claude Code,
then implements and verifies each epic in an isolated git worktree before
merging passing work into an integration branch. It drives `claude -p` as a
subprocess for every stage and shows live progress in the terminal.

## How it works

```
workspace picker (or --workspace) --> Plan stage --> plan.json
                                            |
                                     epics (parallel, dependency-ordered)
                                            |
                          Implement (worktree) --> Verify (VERIFY_CMD) --> Integrate (merge)
```

- **Workspace picker.** On startup the tool loads workspaces from
  `~/.config/agentic-tui/workspaces.toml`. If you pass `--workspace <name|path>`
  the picker is skipped and that workspace is used directly. If you pass a
  path that is not in the config file, it is used as-is with its directory
  name as the label.
- **Plan.** A single `claude -p` session reads the workspace with Glob and
  Grep, then writes a `plan.json` describing epics, each with tasks, an
  `acceptance` list, and `depends_on` links to other epics. The orchestrator
  parses and validates this file: unique ids, no unknown dependencies, no
  cycles.
- **Implement.** Epics run as separate `claude -p` sessions. Every epic gets
  its own git worktree and branch (`agentic/<epic-id>`), checked out from the
  workspace `HEAD`, so epics never touch each other's working tree or the
  main checkout while they work. The scheduler starts any epic whose
  dependencies have succeeded, up to `MAX_PARALLEL_EPICS` at a time. If an
  epic fails, every epic that depends on it (transitively) is skipped;
  independent epics keep running.
- **Verify.** After an epic session finishes, the orchestrator runs
  `VERIFY_CMD` inside that epic's worktree. A non-zero exit fails the epic. A
  failed epic is retried once from a fresh worktree before being marked
  failed for good.
- **Integrate.** Each epic that passes verification is merged into the
  `agentic-integration` branch, in the order the epics finish. Merges happen
  in a dedicated worktree so they never disturb the workspace's main working
  tree. A merge conflict is reported, not auto-resolved: the epic's branch
  and worktree are kept so you can merge it by hand.

When the run ends, the tool prints a report: one line per epic with its final
status (merged, failed, skipped, or conflict), the total cost, and, if any
epic merged, a reminder that the work is on `agentic-integration` in the
workspace and needs to be reviewed and merged to your main branch by hand.

## Configuring workspaces

Workspaces live in `~/.config/agentic-tui/workspaces.toml`. Each entry needs a
`name` and a `path`; `~` in the path expands to your home directory.

```toml
# ~/.config/agentic-tui/workspaces.toml
[[workspace]]
name = "greentic"
path = "~/Works/personal/greentic"
```

Add one `[[workspace]]` block per project. Every workspace path must be an
existing directory that is a git repository (it must contain a `.git`), or
the run fails with a clear error before any Claude session starts.

## Prerequisites

- The Claude Code CLI installed on PATH.
- A subscription login (`claude login`). Do not set `ANTHROPIC_API_KEY` if you
  want to use the subscription limit, because if it is set it takes
  precedence over the subscription and you are billed per token.
- git, available on PATH. The orchestrator shells out to it for worktrees,
  branches, and merges.
- A Rust toolchain. If you use a recent rustc, you may delete `Cargo.lock` and
  run `cargo update` for fresh dependencies. The bundled `Cargo.lock` is
  intentionally pinned to older versions so it can build on rustc 1.75.

## Run

```bash
cargo run -- "Add per-tenant rate limiting in the API gateway"
```

With no `--workspace`, this opens the workspace picker (up/down, Enter, `q`
to quit). To skip the picker, pass a configured name or a raw path:

```bash
cargo run -- "Add a health check endpoint" --workspace greentic
cargo run -- "Add a health check endpoint" --workspace /path/to/repo
```

Override the verify command per run with `--verify`:

```bash
cargo run -- "Add a health check endpoint" --workspace greentic --verify "npm test"
```

Press `q` or Ctrl-C at any point to abort. In-flight epic sessions are killed;
epics that already merged stay merged on `agentic-integration`.

Equivalent shortcuts through the Makefile:

```bash
make run GOAL="Add a health check endpoint" WORKSPACE=greentic
```

## Config knobs (`src/config.rs`)

- `MODEL_PLAN` and `MODEL_EPIC` select which model runs each stage. Plan
  defaults to `opus` because plan quality drives every epic's accuracy. Epics
  default to `sonnet` to save on your limit.
- `GLOBAL_BUDGET_USD` stops the orchestrator from starting new epics once
  accumulated cost crosses this line. Epics already running still finish.
- `EPIC_BUDGET_USD` caps the cost of a single stage session (the plan, or one
  epic).
- `PLAN_TOOLS` and `EPIC_TOOLS` are the allowed tool lists for each stage,
  pre-approved so a run does not stop to ask for permission. Plan gets
  read-only tools plus `Write`, for `plan.json`. Epics add `Edit` and `Bash`
  so they can change code and run commands.
- `PLAN_MAX_TURNS` and `EPIC_MAX_TURNS` cap how many turns a session may take
  before the orchestrator gives up on it.
- `MAX_PARALLEL_EPICS` is the parallel cap the scheduler enforces.
- `DEFAULT_VERIFY_CMD` is the verify command used when `--verify` is not
  passed. It defaults to `make verify`; override it per project with
  `--verify` if the project uses something else (`npm test`, `pytest`, and so
  on).
- `PERMISSION_MODE` and the `plan_prompt` / `epic_prompt` functions define the
  exact prompts sent to Claude Code for each stage. Edit them here to change
  how the Tech Lead plans or how an epic session behaves.

## Notes

- stderr from `claude` is nulled so it does not corrupt the TUI screen. For
  debugging, change `Stdio::null()` in `engine.rs` to piped and read it in a
  separate task.
- `plan.json` is written to `.agentic-plan.json` at the workspace root and is
  gitignored, along with `.agentic-worktrees/`, the directory that holds every
  epic's worktree and the dedicated integration worktree.
- The engine (`engine.rs`) is a single generic `run_stage` function shared by
  the Plan stage and every epic session; only the working directory, model,
  tool list, and prompt differ between them.

## Known limitations (v1)

- On abort (q/Ctrl-C), running child processes are killed but the
  `.agentic-worktrees/` directory is left in place. The next run reuses it or
  force-recreates it.
- The global budget is checked before starting each new epic. Epics already
  in flight still finish, so the final cost can slightly exceed the budget.
- Merge conflicts are reported and left on the `agentic/<epic-id>` branch for
  manual merge. They are not auto-resolved.
