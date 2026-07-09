# Agentic Orchestrator

![Rust](https://img.shields.io/badge/Rust-stable-orange?logo=rust&logoColor=white)
![UI](https://img.shields.io/badge/UI-Leptos%20(wasm)-1f6feb)
![Async](https://img.shields.io/badge/async-tokio-000000)
![Requires](https://img.shields.io/badge/requires-Claude%20Code%20CLI-8a2be2)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey)
![Version](https://img.shields.io/badge/version-0.1.0-blue)

A Rust tool that takes a goal, breaks it into epics with Claude Code, then
implements and verifies each epic in an isolated git worktree before merging
passing work into an integration branch. It drives `claude -p` as a
subprocess for every stage. Running the binary starts a local web server and
opens a browser tab that shows live progress; there is no terminal UI.

## How it works

```
workspace picker --> Plan stage --> plan.json
                                            |
                                     epics (parallel, dependency-ordered)
                                            |
                          Implement (worktree) --> Verify (VERIFY_CMD) --> Integrate (merge)
```

- **Workspace picker.** The landing page in the browser loads workspaces from
  `~/.config/agentic-tui/workspaces.toml` and lists them. Selecting one opens
  the new-run form for that workspace.
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
  `agentic-integration` branch by default, in the order the epics finish. Merges happen
  in a dedicated worktree so they never disturb the workspace's main working
  tree. A merge conflict is reported, not auto-resolved: the epic's branch
  and worktree are kept so you can merge it by hand.

When the run ends, the tool prints a report: one line per epic with its final
status (merged, failed, skipped, or conflict), the total cost, and, if any
epic merged, a reminder that the work is on the integration branch (by default
`agentic-integration`) in the workspace and needs to be reviewed and merged to
your main branch by hand.

## Configuring workspaces

Workspaces live in `~/.config/agentic-tui/workspaces.toml`. Each entry needs a
`name` and a `path`; `~` in the path expands to your home directory.

```toml
# ~/.config/agentic-tui/workspaces.toml
[[workspace]]
name = "greentic"
path = "~/Works/personal/greentic"
base = "develop"              # optional: worktree base ref (default HEAD)
integration = "agentic-wip"   # optional: merge target (default agentic-integration)
```

`base` and `integration` are optional per-workspace defaults. The base branch
and integration branch fields on the new-run form override the matching field
for that run.

Add one `[[workspace]]` block per project. Every workspace path must be an
existing directory that is a git repository (it must contain a `.git`), or the
run fails with a clear error before any Claude session starts.

You do not have to write this file by hand. If the workspace list is empty,
the landing page opens an onboarding panel that scans a folder you choose
(your home directory by default) for git repositories, lets you check the
ones you want, and saves them here for you. The "Add workspace" button opens
the same panel at any time to add more.

## Prerequisites

- A modern Rust toolchain (stable channel; the crate targets an edition-2021,
  current-stable compiler with no legacy pin).
- The `wasm32-unknown-unknown` target: `rustup target add wasm32-unknown-unknown`.
- `trunk`, the build tool for the Leptos web crate: `cargo install --locked trunk`.
- The Claude Code CLI installed on PATH.
- A subscription login (`claude login`). Do not set `ANTHROPIC_API_KEY` if you
  want to use the subscription limit, because if it is set it takes
  precedence over the subscription and you are billed per token.
- git, available on PATH. The orchestrator shells out to it for worktrees,
  branches, and merges.

## Run

```bash
make run
```

This is equivalent to `cargo run -p agentic-tui`. The first build runs `trunk
build` for the web crate before the server compiles, because the server
embeds the built web assets at compile time; see `make build` below. The
binary then starts a server on an ephemeral loopback port, prints the URL,
and opens it in your default browser. Pass `--no-open` to skip opening the
browser and use the printed URL yourself:

```bash
cargo run -p agentic-tui -- --no-open
```

In the browser, the landing page lists configured workspaces (or the
onboarding panel if none are configured yet). Selecting a workspace opens the
new-run form, where you enter the goal and, optionally, a base branch, an
integration branch, and a verify command; each falls back to the workspace's
configured default, then to the built-in default, if left blank.

Before planning, the tool runs a goal-refine step by default: a short `claude`
pass reads the repository, rewrites your goal to be more specific, and may
ask a few clarifying questions. The form shows them one at a time; a second
pass folds your answers into a final goal, and you confirm (and can edit)
that goal before planning starts. Uncheck "Refine the goal before planning"
to skip the whole step and plan with your goal as entered. The refine cost
counts toward the run budget.

By default each run branches its epic worktrees from the workspace `HEAD` and
merges passing epics into `agentic-integration`. The base branch field is the
branch, tag, or commit new epic worktrees start from (and the integration
branch is created from it on first use). The integration branch field is
where passing epics merge; if that branch already exists, the work merges
into it directly, so pointing it at a real branch such as `develop` merges
there without a manual review step. An invalid base branch aborts the run
before any Claude session starts, and the form reports the error.

Once the run starts, the dashboard opens automatically: a header with the
goal, workspace, and running cost; a five-column kanban board (Todo, In
progress, Review, Done, Blocked) with one card per epic; a scrolling log
pane; and an Abort button. Aborting kills in-flight epic sessions; epics that
already merged stay merged on the integration branch. When the run ends, the
dashboard shows a final report: one line per epic with its final status
(merged, failed, skipped, or conflict) and the total cost.

## Config knobs (`src/config.rs`)

- `MODEL_PLAN` and `MODEL_EPIC` select which model runs each stage. Plan
  defaults to `opus` because plan quality drives every epic's accuracy. Epics
  default to `sonnet` to save on your limit.
- `MODEL_REFINE`, `REFINE_BUDGET_USD`, and `REFINE_MAX_QUESTIONS` tune the
  goal-refine step (model, per-pass budget, and how many clarifying questions it
  may ask). Uncheck "Refine the goal before planning" on the new-run form to
  skip refining entirely.
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
- `DEFAULT_VERIFY_CMD` is the verify command used when the verify command
  field on the new-run form is left blank. It defaults to `make verify`;
  override it per project on that form if the project uses something else
  (`npm test`, `pytest`, and so on).
- `PERMISSION_MODE` and the `plan_prompt` / `epic_prompt` functions define the
  exact prompts sent to Claude Code for each stage. Edit them here to change
  how the Tech Lead plans or how an epic session behaves.

## Notes

- stderr from `claude` is nulled so it does not interleave with the server's
  own logging. For debugging, change `Stdio::null()` in `engine.rs` to piped
  and read it in a separate task.
- `plan.json` is written to `.agentic-plan.json` at the workspace root and is
  gitignored, along with `.agentic-worktrees/`, the directory that holds every
  epic's worktree and the dedicated integration worktree.
- The engine (`engine.rs`) is a single generic `run_stage` function shared by
  the Plan stage and every epic session; only the working directory, model,
  tool list, and prompt differ between them.

## Known limitations (v1)

- On a mid-flight abort (clicking Abort in the browser while epics are still
  running), the child processes are killed and the `.agentic-worktrees/`
  directory is removed so the workspace is left tidy. Merged work survives on
  `agentic-integration`. Navigating away after the run has finished leaves the
  worktrees in place, so a conflict worktree kept for a manual merge is
  preserved.
- The global budget is checked before starting each new epic. Epics already
  in flight still finish, so the final cost can slightly exceed the budget.
- Merge conflicts are reported and left on the `agentic/<epic-id>` branch for
  manual merge. They are not auto-resolved.
- The integration branch (default `agentic-integration`) is checked out in a
  dedicated worktree under `.agentic-worktrees/.integration`, which is left in
  place after a normal run. If you point the integration branch field at a
  branch you also want to check out in your main tree, remove that worktree
  first (`git worktree remove`). A target branch that is already checked out
  in the main working tree is rejected before the run starts.

## License

MIT. See [LICENSE](LICENSE).
