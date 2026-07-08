# PRD Generator TUI

A Rust + ratatui tool that generates a PRD from a goal, which you then hand off
to Claude Code for implementation. The loop drives Claude Code headless
(`claude -p`) as a subprocess and shows streaming progress in the terminal.

## How it works

```
goal --> claude -p (read repo + research + write PRD) --> docs/prd/<slug>.md
              |
       stream-json event --> tokio channel --> TUI (log, cost, status)
```

- The engine spawns `claude -p` with `--output-format stream-json`, parses the
  NDJSON line by line, and emits events over a `tokio::mpsc` channel to the UI.
- The ratatui UI renders the header (goal + cost gauge), a live log panel, and a
  status footer. Keyboard input runs on a separate thread, merged into the same
  channel.
- The PRD output is written to `docs/prd/<slug>.md`. The path is computed in Rust
  so it can be displayed and used for the handoff command at the end.

## Prerequisites

- The Claude Code CLI installed on PATH.
- A subscription login (`claude login`). Do not set `ANTHROPIC_API_KEY` if you
  want to use the subscription limit, because if it is set it takes precedence
  over the subscription and you are billed per token.
- A Rust toolchain. If you use a recent rustc, you may delete `Cargo.lock` and
  run `cargo update` for fresh dependencies. The bundled `Cargo.lock` is
  intentionally pinned to older versions so it can build on rustc 1.75.

## Run

```bash
cargo run -- "Add per-tenant rate limiting in the API gateway" --repo /path/to/repo
```

Press `q` to quit. When it finishes, the tool prints the PRD path and a
ready-to-paste handoff command for the Claude Code implementation session.

## Knobs (`src/config.rs`)

- `MODEL_PRD` defaults to `opus`. Switch to `sonnet` to save on your limit.
- `BUDGET_USD` the estimated cost limit, shown as a gauge.
- `PRD_TOOLS` the allowed tools (read-only plus Write), pre-approved so the run
  does not block asking for permission.

## Notes

- stderr from `claude` is nulled so it does not corrupt the TUI screen. For
  debugging, change `Stdio::null()` in `engine.rs` to piped and read it in a
  separate task.
- This is intentionally single-stage (PRD only). The engine + event + UI
  structure is already general, so adding another stage (plan, review, test) is
  just a matter of extending the pipeline without changing the foundation.
