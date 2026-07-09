# Agentic Orchestrator

![Rust](https://img.shields.io/badge/Rust-stable-orange?logo=rust&logoColor=white)
![UI](https://img.shields.io/badge/UI-Leptos%20(wasm)-1f6feb)
![Async](https://img.shields.io/badge/async-tokio-000000)
![Requires](https://img.shields.io/badge/requires-Claude%20Code%20CLI-8a2be2)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey)
![Version](https://img.shields.io/badge/version-0.1.0-blue)

A Rust tool that takes a goal, breaks it into epics with Claude Code across
one or more git repositories, then implements and verifies each epic in an
isolated git worktree before merging passing work into that repo's
integration branch. It drives `claude -p` as a subprocess for every stage.
Running the binary starts a local web server and opens a browser tab that
shows live progress; there is no terminal UI.

## How it works

```
workspace picker --> Plan stage --> plan.json (epics tagged with repo + verify)
                                            |
                                     epics (parallel, dependency-ordered)
                                            |
          Implement (worktree in the epic's repo) --> Verify (epic's verify command) --> Integrate (merge into that repo's integration branch)
```

- **Workspace picker.** The landing page in the browser loads workspaces from
  `~/.config/agentic-tui/workspaces.toml` and lists them. A workspace is a
  named group of one or more git repositories that a run targets together.
  Selecting one opens the new-run form for every repo in that group.
- **Plan.** A single `claude -p` session reads every repo in the workspace
  with Glob and Grep, then writes a `plan.json` describing epics, each with
  tasks, an `acceptance` list, a `repo` tag naming which of the workspace's
  repos the epic targets, a `verify` command suited to that repo, and
  `depends_on` links to other epics (possibly in a different repo). The
  orchestrator parses and validates this file: unique ids, no unknown
  dependencies, no cycles, and every `repo` tag names a repo in the
  workspace.
- **Implement.** Epics run as separate `claude -p` sessions, each inside its
  target repo. Every epic gets its own git worktree and branch
  (`agentic/<epic-id>`) inside that repo, checked out from that repo's base
  ref by default, or from that repo's integration branch when the epic
  depends on another epic in the SAME repo, so it inherits that repo's
  already-merged work. A dependency on an epic in a DIFFERENT repo only
  orders the work, since code is not shared across repos: the dependent epic
  still branches from its own repo's base ref and simply waits for the
  cross-repo dependency to finish first. Epics never touch each other's
  working tree or the main checkout of any repo while they work. The
  scheduler starts any epic whose dependencies have succeeded, up to
  `MAX_PARALLEL_EPICS` at a time across all repos. If an epic fails, every
  epic that depends on it (transitively) is skipped; independent epics keep
  running.
- **Verify.** After an epic session finishes, the orchestrator runs that
  epic's verify command, chosen by the planner for its repo, falling back to
  the run's default verify command if the planner left it unset, inside that
  epic's worktree. A non-zero exit fails the epic. A failed epic is retried
  once from a fresh worktree before being marked failed for good.
- **Integrate.** Each epic that passes verification is merged into its own
  repo's integration branch (`agentic-integration` by default, or the branch
  configured for that repo), in the order epics finish. Merges within a repo
  happen in a dedicated worktree so they never disturb that repo's main
  working tree; merges in different repos proceed independently of one
  another. A merge conflict is reported, not auto-resolved: the epic's branch
  and worktree are kept so you can merge it by hand.

When the run ends, the tool prints a report: one line per epic with its final
status (merged, failed, skipped, or conflict), the total cost, and, for every
repo with a merged epic, a reminder that the work is on that repo's
integration branch and needs to be reviewed and merged to your main branch by
hand.

## Configuring workspaces

Workspaces live in `~/.config/agentic-tui/workspaces.toml`. A workspace is a
named GROUP of one or more git repositories that a run targets together; one
goal can span every repo in the group in a single run. Each repo entry needs
a `name` and a `path`; `~` in a path expands to your home directory.

```toml
# ~/.config/agentic-tui/workspaces.toml
[[workspace]]
name = "greentic"

  [[workspace.repo]]
  name = "greentic"
  path = "~/projects/Works/greentic/greentic"
  base = "main"                 # optional, per-repo
  integration = "agentic-wip"   # optional, per-repo

  [[workspace.repo]]
  name = "greentic-billing"
  path = "~/projects/Works/greentic/greentic-billing"
```

`base` and `integration` are optional settings on each repo, not on the
workspace as a whole: `base` is the worktree ref that repo's epics branch
from (default `HEAD`), and `integration` is the branch that repo's passing
epics merge into (default `agentic-integration`). There are no per-run
base/integration form fields anymore; every repo's base and integration
branch come from its own entry here. There is likewise no `verify` field in
this file: verify is chosen by the planner per epic, suited to that epic's
repo (see "How it works" above).

A legacy flat `[[workspace]]` entry with a `path` (and optional `base` /
`integration`) still works and is read as a one-repo group named after the
workspace, so existing configs keep working unchanged:

```toml
[[workspace]]
name = "portfolio"
path = "~/Works/personal/portfolio"
base = "develop"
```

Add one `[[workspace]]` block per group. Every repo path must be an existing
directory that is a git repository (it must contain a `.git`), or the run
fails with a clear error naming the offending repo, before any Claude session
starts.

You do not have to write this file by hand. If the workspace list is empty,
the landing page opens an onboarding panel that scans a folder you choose
(your home directory by default) for git repositories, lets you check the
ones you want, and saves all of them as one grouped workspace under a name
you choose. The "Add workspace" button opens the same panel at any time to
add more.

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

In the browser, the app opens on a Dashboard that lists every run of the
session (live and finished). Only one run per workspace can be in flight at a
time. Workspaces are managed at `/workspaces` (or via the onboarding panel if
none are configured yet). Run history is session-scoped, kept in memory while
the server runs and not persisted to disk.

To start a new run, select a workspace to open the new-run form. It shows the
goal field, a read-only "Repos in scope" list naming every repo in the
workspace group, and a default verify command, which falls back to the
built-in default if left blank. Base and integration branches are no longer
form fields; each repo's base and integration branch come from that repo's
own config entry (default `HEAD` and `agentic-integration` respectively), as
described in "Configuring workspaces" above.

Before planning, the tool runs a goal-refine step by default: a short `claude`
pass reads the workspace's repos, rewrites your goal to be more specific, and
may ask a few clarifying questions. The form shows them one at a time; a second
pass folds your answers into a final goal, and you confirm (and can edit)
that goal before planning starts. Uncheck "Refine the goal before planning"
to skip the whole step and plan with your goal as entered. The refine cost
counts toward the run budget.

By default each repo branches its epic worktrees from that repo's `HEAD` and
merges its passing epics into `agentic-integration`. A repo's configured
`base` is the branch, tag, or commit its epic worktrees start from (and its
integration branch is created from that base on first use, if it does not
already exist). A repo's configured `integration` is where its passing epics
merge; if that branch already exists, the work merges into it directly, so
pointing it at a real branch such as `develop` merges there without a manual
review step. An invalid base ref in any repo aborts the run before any Claude
session starts, and the form reports the error naming that repo.

Once the run starts, the dashboard opens automatically: a header with the
goal, workspace, and running cost; a five-column kanban board (Todo, In
progress, Review, Done, Blocked) with one card per epic; a scrolling log
pane; and an Abort button. Aborting kills in-flight epic sessions; epics that
already merged stay merged on their repo's integration branch. When the run
ends, the dashboard shows a final report: one line per epic with its final
status (merged, failed, skipped, or conflict) and the total cost.

## Config knobs (`crates/server/src/config.rs`)

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
- `DEFAULT_VERIFY_CMD` is the fallback verify command for an epic whose
  planner-chosen `verify` is unset, and the default shown in the new-run
  form's default verify command field when left blank. It defaults to `make
  verify`; the planner normally picks a command per epic suited to that
  epic's repo (`npm test`, `pytest`, and so on), so this mostly matters as a
  safety net.
- `PERMISSION_MODE` and the `plan_prompt` / `epic_prompt` functions define the
  exact prompts sent to Claude Code for each stage. `plan_prompt` lists every
  repo in the workspace and instructs the planner to tag each epic with one
  of them plus a verify command suited to it. Edit these functions here to
  change how the Tech Lead plans or how an epic session behaves.

## Notes

- stderr from `claude` is nulled so it does not interleave with the server's
  own logging. For debugging, change `Stdio::null()` in `engine.rs` to piped
  and read it in a separate task.
- `plan.json` is written to `.agentic-plan.json` at the shared root of the
  workspace's repos (their common parent directory; for a one-repo group,
  that repo's parent), and is gitignored, along with `.agentic-worktrees/` in
  each repo, the directory that holds that repo's epic worktrees and its
  dedicated integration worktree.
- The engine (`engine.rs`) is a single generic `run_stage` function shared by
  the Plan stage and every epic session; only the working directory, model,
  tool list, and prompt differ between them.

## Known limitations (v1)

- On a mid-flight abort (clicking Abort in the browser while epics are still
  running), the child processes are killed and the `.agentic-worktrees/`
  directory is removed in every repo the run targets, so each repo is left
  tidy. Merged work survives on each repo's integration branch. Navigating
  away after the run has finished leaves the worktrees in place, so a
  conflict worktree kept for a manual merge is preserved.
- The global budget is checked before starting each new epic, across all
  repos in the run. Epics already in flight still finish, so the final cost
  can slightly exceed the budget.
- Merge conflicts are reported and left on the `agentic/<epic-id>` branch, in
  that epic's repo, for manual merge. They are not auto-resolved.
- Each repo's integration branch (default `agentic-integration`) is checked
  out in a dedicated worktree under that repo's `.agentic-worktrees/.integration`,
  which is left in place after a normal run. If you point a repo's
  integration branch at a branch you also want to check out in that repo's
  main tree, remove that worktree first (`git worktree remove`). A target
  branch that is already checked out in a repo's main working tree is
  rejected before the run starts.

## License

MIT. See [LICENSE](LICENSE).
