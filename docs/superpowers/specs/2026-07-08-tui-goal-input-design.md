# TUI Goal Input — Design

Date: 2026-07-08
Status: Approved for planning
Project: `agentic-tui`

## Overview

Let the operator type the goal in the TUI when it is not passed on the command
line, instead of requiring it as a CLI argument. Presentation/entry only; the
pipeline is unchanged.

## Flow

```
parse args -> resolve workspace (picker if no --workspace)
           -> if goal is empty: show a goal input screen in the TUI
           -> validate workspace -> run pipeline
```

- The goal stays an optional CLI positional argument (fast path). If given, the
  input screen is skipped. If empty, the input screen appears after the
  workspace is chosen, so the prompt can name the workspace.
- Cancelling the input screen (Esc or Ctrl-C) exits cleanly without running.

## Goal input screen

A blocking screen using the same raw-mode/alternate-screen pattern as the
workspace picker (it runs before the async pipeline starts). It shows a bordered
box titled `Goal for <workspace> (Enter to run, Esc to cancel)` and the text
typed so far with a block cursor. Keys:

- Printable character: append to the buffer.
- Backspace: remove the last character.
- Enter: submit if the trimmed buffer is non-empty (otherwise ignored).
- Esc or Ctrl-C: cancel, return no goal, exit.

## Components (changes)

- `src/ui.rs`: add `pub fn render_goal_input(f: &mut Frame, workspace: &str, buffer: &str)`.
- `src/main.rs`: add `fn run_goal_input(workspace: &str) -> anyhow::Result<Option<String>>`
  (blocking loop, returns `None` on cancel). `parse_args` no longer treats an
  empty goal as an error (it always returns the parsed args). `main` calls
  `run_goal_input` when `args.goal` is empty and uses the resulting goal.

## Error handling and edge cases

- Goal given on the CLI: input screen skipped entirely.
- Empty buffer on Enter: ignored, the screen stays up.
- Cancel: print a short message and exit with success, nothing runs.

## Testing strategy

Consistent with the workspace picker, the input loop is verified by build plus a
manual run in a real terminal (it needs a TTY). No unit test for the blocking
key loop.

## Non-goals

- No multi-line goal editing, history, or autocomplete.
- No change to the pipeline, scheduler, worktree, or engine.
