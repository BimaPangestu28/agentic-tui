# Goal Refine Step Design

## Problem

Today the flow jumps straight from the entered goal to the Plan stage: a raw,
often ambiguous one-line goal is fed directly into `plan_prompt`, and any
ambiguity is resolved by the planner guessing. There is no chance to sharpen the
goal or answer a clarifying question before real planning budget is spent.

## Goal

Insert an optional goal-refine step between goal entry and planning. It runs one
`claude -p` pass that reads the repository, rewrites the goal to be specific, and
proposes up to a few clarifying questions. The user answers them one at a time, a
second pass folds the answers into a final goal, and the user confirms (and may
edit) that goal before the Plan stage runs. The step is on by default and can be
skipped with `--no-refine` or by pressing Esc.

## Non-goals

- No persistent interactive chat session with Claude (this is two discrete
  one-shot passes, not a conversation).
- No change to the Plan, Implement, Verify, or Integrate stages.
- No more than one clarification round.

## Flow

```
goal (from CLI or the TUI goal input)
  |  (skipped entirely if --no-refine)
  v
refine pass 1  -- claude -p reads the repo, writes .agentic-refine.json:
  |               { "refined_goal": "...", "questions": ["...", ...] }
  |
  |-- questions is empty  -----------------------------\
  v                                                     |
answer questions one at a time (Enter next, empty skips |
that question, Esc skips the whole refine step)         |
  v                                                     |
refine pass 2  -- claude -p folds the answers into a    |
  |               final goal, writes { "refined_goal" } |
  v                                                     v
goal confirm screen: final refined goal in an editable field (Enter accepts)
  v
run_pipeline (Plan -> epics) with the confirmed goal
```

- **Zero questions:** if pass 1 returns an empty `questions` list, skip the
  answer screens and pass 2 entirely, and take the confirm screen straight to
  pass 1's `refined_goal`.
- **Esc** at any refine screen skips refining and proceeds to planning with the
  original goal. **Ctrl-C** cancels the whole run.
- **Failure fallback:** if a refine pass errors (spawn failure, budget, or the
  JSON is missing or unparseable), the step logs nothing to the run and falls
  back to the original goal, then proceeds to planning. Refining never blocks a
  run.

## Architecture

The step reuses the existing `plan.json` pattern exactly: each refine pass is a
one-shot `claude -p` invocation driven by `engine::run_stage`, told to write a
JSON file (`.agentic-refine.json` at the repo root, gitignored like
`.agentic-plan.json`). We read and parse that file. No streaming UI, no
interactive Claude session, and `engine.rs` is unchanged.

`run_refine` is a blocking flow on its own alternate screen, structured like
`run_picker` / `run_goal_input` / `run_onboarding`, called from `main` after the
goal is known and before the run pipeline is spawned. It is `async` because it
awaits `engine::run_stage`; while a pass runs it draws a static "refining" frame,
then reads the output file. Between passes it uses the blocking crossterm read
loop for the answer and confirm screens. Refine event streaming is discarded (a
throwaway channel), since the main run UI does not exist yet at this point.

The two refine passes count toward the run's budget: `run_refine` returns the
accumulated refine cost, which is threaded into the pipeline as initial cost so
`total_cost` and the global budget gate include it.

## Components

### `src/config.rs`

- `MODEL_REFINE: &str = "sonnet"` (cheap; the goal-sharpening does not need opus).
- `REFINE_TOOLS: &str` = the same read-only-plus-Write set as `PLAN_TOOLS`
  (`Read,Glob,Grep,Write,WebSearch,WebFetch,Skill`). No Edit or Bash.
- `REFINE_MAX_TURNS: u32` (for example 12).
- `REFINE_BUDGET_USD: f64` (for example 0.20) capping each refine pass.
- `REFINE_MAX_QUESTIONS: usize = 5` (stated in the prompt and enforced by
  truncating the parsed list).
- `refine_questions_prompt(goal, out_path) -> String`: instructs Claude to read
  the repo, rewrite the goal to be specific and actionable, and list at most
  `REFINE_MAX_QUESTIONS` clarifying questions whose answers would materially
  change the plan (empty list if the goal is already clear), then write
  `{ "refined_goal": "...", "questions": [...] }` to `out_path` and no other
  file. Uses the shared `STYLE`.
- `refine_finalize_prompt(goal, qa_pairs, out_path) -> String`: given the
  original goal and the answered questions, produce one specific refined goal
  incorporating the answers, and write `{ "refined_goal": "...", "questions": [] }`
  to `out_path`. May run without re-reading the repo.

### `src/refine.rs` (new)

- `RefineResult { refined_goal: String, questions: Vec<String> }` with
  `#[serde(default)]` on `questions`.
- `parse_refine(json: &str) -> anyhow::Result<RefineResult>` (pure, unit-tested).
- `RefineOutcome { goal: Option<String>, cost: f64 }` where `goal == None` means
  the user cancelled the run (Ctrl-C).
- `pub async fn run(repo: &Path, goal: &str) -> anyhow::Result<RefineOutcome>`:
  the blocking flow described above. Truncates parsed questions to
  `REFINE_MAX_QUESTIONS`. On any pass failure returns `RefineOutcome { goal:
  Some(original_goal), cost: accumulated }`.

### `src/ui.rs`

- `render_refining(f, note: &str)`: a status frame shown while a pass runs.
- `render_refine_question(f, question: &str, index: usize, total: usize, answer: &str)`:
  one clarifying question with an editable answer field, styled like
  `render_goal_input`.
- `render_goal_confirm(f, goal: &str)`: the final refined goal in an editable
  field, Enter to accept.

### `src/main.rs`

- Parse `--no-refine` into `Args`.
- After the goal is resolved (CLI or `run_goal_input`) and before spawning the
  pipeline: if not `--no-refine`, call `refine::run(&repo, &goal).await`. On
  `RefineOutcome { goal: None, .. }` print a cancel message and return; otherwise
  use the returned goal and thread the returned cost into the pipeline.

### `.gitignore`, `README.md`

- Gitignore `.agentic-refine.json`.
- Document the refine step and `--no-refine` in the usage and Run sections.

## JSON contract

Both passes write the same shape; pass 2 leaves `questions` empty:

```json
{ "refined_goal": "string", "questions": ["string", "..."] }
```

Parsing is lenient: a missing `questions` defaults to empty; a missing or empty
`refined_goal`, missing file, or invalid JSON is a parse failure that triggers
the original-goal fallback.

## Error handling

- Refine pass spawn/exec failure, non-zero result, missing file, or unparseable
  JSON: fall back to the original goal, continue to planning.
- Empty `refined_goal` in the JSON: treated as a parse failure (fallback).
- Esc during any refine screen: skip refining, use the original goal.
- Ctrl-C: cancel the whole run.

## Testing

- `parse_refine`: valid full object; object with no `questions` (defaults empty);
  empty `refined_goal` is an error; malformed JSON is an error. Unit tests in
  `refine.rs`.
- Question truncation to `REFINE_MAX_QUESTIONS` is unit-testable as a pure helper
  if extracted, otherwise covered by the parse tests plus a truncation test.
- TUI render functions are not unit-tested, consistent with `ui.rs`.
- The end-to-end refine flow is verified manually or with a pty smoke test, as
  the onboarding wizard was.

## Files

| File | Change |
|---|---|
| `src/config.rs` | refine knobs and the two refine prompts |
| `src/refine.rs` | new: `RefineResult`, `parse_refine`, `RefineOutcome`, `run`; tests |
| `src/ui.rs` | `render_refining`, `render_refine_question`, `render_goal_confirm` |
| `src/main.rs` | `mod refine`; `--no-refine`; call `refine::run` and thread cost |
| `.gitignore` | ignore `.agentic-refine.json` |
| `README.md` | document the refine step and `--no-refine` |
