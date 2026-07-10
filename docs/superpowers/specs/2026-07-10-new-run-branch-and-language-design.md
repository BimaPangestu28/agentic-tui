# New-run branch selection + output language — design

Date: 2026-07-10
Status: Approved (brainstorming)

## Problem

The new-run form (`/run/new`) currently exposes only a goal, a default verify
command, and a refine toggle. It shows the in-scope repos read-only. Two gaps:

1. **No branch control.** Each repo's base ref (where the epic worktree is cut
   from) and integration branch (where epic work merges into) are whatever was
   saved on the workspace config, defaulting to `HEAD` / `agentic-integration`.
   The user cannot pick, per run, which branch to build from or merge into —
   even though this decides the entire worktree/merge topology.
2. **No output language.** Every prompt the agent runs is English-only (a
   hardcoded `STYLE` constant). There is no way to have the agent communicate
   with the user in another language.

## Goals

- Let the user, on the new-run form, choose **per repo**: the base branch
  (dropdown of the repo's real branches) and the integration branch (dropdown
  of real branches, or a typed new name).
- Let the user choose an **output language** (English default, Indonesian) that
  governs only the agent's user-facing prose — clarifying questions, summaries,
  and log narration. Code, identifiers, comments, and commit messages stay in
  their conventional language (English).

## Non-goals

- Editing branch choices does **not** update the saved workspace config; it is
  a per-run override only.
- No remote-branch listing (local heads only). Can be added later.
- Language does not translate code, commit messages, or identifiers.
- No new languages beyond English + Indonesian in this pass.

## Design

### A. Branch selection (mostly reuses existing plumbing)

The data path already exists end to end: `Repo.base` / `Repo.integration`
(`workspace.rs`) → `RepoDto.base` / `.integration` (`shared/src/lib.rs`) →
POSTed back verbatim in `StartRunRequest.workspace` → resolved into
`RepoRun.base_ref` / `.integration_branch` in `run::start`. The form loads
these values but never surfaces or edits them. So **no change to `RunConfig`,
`RepoRun`, or the orchestrator is needed** — only a branch-listing endpoint and
the form UI.

**1. Server — new endpoint `GET /api/repo/branches?path=<repo path>`**

- New git helper (in `worktree.rs`, alongside `verify_ref`): list local heads
  via `git for-each-ref --format=%(refname:short) refs/heads`, and read the
  current branch via `git rev-parse --abbrev-ref HEAD` (may be `HEAD` when
  detached).
- Returns `RepoBranchesResponse { branches: Vec<String>, current: Option<String> }`.
- Path is tilde-expanded/canonicalized like `run::start` does. A non-git or
  missing path returns `400` with a message.
- Route added next to the others in `http.rs`.

**2. Shared — new DTO**

```rust
pub struct RepoBranchesResponse {
    pub branches: Vec<String>,
    pub current: Option<String>,
}
```

**3. Web `api.rs`** — `list_branches(path: &str) -> Result<RepoBranchesResponse, String>`.

**4. Web `new_run.rs`**

- New per-repo state: a map/vec keyed by repo path holding the fetched branch
  list, the selected `base`, and the (typed-or-picked) `integration`.
- On load (after the workspace resolves), fetch branches for each repo in
  parallel (`spawn_local` per repo).
- Render, per repo, under the existing repo row:
  - **Base**: a `<select>` of the repo's branches. Pre-selected value =
    `RepoDto.base` if set, else `current`, else the first branch.
  - **Integration**: an editable combobox — `<input list="branches-<i>">` bound
    to a `<datalist>` of the repo's branches — so the user can pick an existing
    branch or type a new name. Pre-filled = `RepoDto.integration` if set, else
    `agentic-integration`.
- While branches load: show a small "loading branches…" hint; if the fetch
  fails, fall back to plain text inputs pre-filled with the config/default
  values so the form is never blocked.
- On submit (both the direct and the refine→plan paths): rebuild the
  `WorkspaceDto` so each `RepoDto.base` / `.integration` carries the chosen
  values (`Some(...)`), then POST as today.

Server-side validation is unchanged and still applies: `verify_ref` on the base,
reject an empty integration name, reject an integration branch that is the one
currently checked out in the repo's main working tree.

### B. Output language (net-new, thin)

**1. Shared — `Language` enum + request fields**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    #[default]
    English,
    Indonesian,
}
```

- A `label()` / `Display` gives the human name used in prompts ("English",
  "Indonesian").
- Add `language: Language` (with `#[serde(default)]`) to `StartRunRequest`,
  `RefineQuestionsRequest`, and `RefineFinalizeRequest`. `#[serde(default)]`
  keeps older payloads and the resume path deserializing to English.

**2. Server `config.rs` — prompt injection**

- Each builder (`plan_prompt`, `refine_questions_prompt`,
  `refine_finalize_prompt`, `epic_prompt`) gains a `language: Language`
  parameter.
- A helper `language_directive(language) -> &'static str` returns `""` for
  English (zero behavior change from today) and, for Indonesian, a directive
  such as: *"Communicate with the user in Indonesian: write clarifying
  questions, plan/epic titles, summaries, and any prose addressed to the user in
  Indonesian. Keep code, identifiers, comments, and commit messages in English."*
- The directive is appended near `STYLE` in each prompt.

**3. Threading**

- `StartRunRequest.language` → `run::start` → `spawn_pipeline` → `run_pipeline`
  → `RunConfig.language` (new field) → read where `epic_prompt` and
  `plan_prompt` are called.
- Refine handlers (`http.rs`) pass `request.language` into `refine::questions` /
  `refine::finalize`, which pass it to the refine prompt builders.

**4. Persistence / resume**

- `RunHandle` gains `language`, and `run_store::PersistedRun` gains
  `language: Language` (`#[serde(default)]`), so resume and retry rebuild the
  same `RunConfig.language`. Older persisted runs default to English.

**5. Web `new_run.rs`** — a `<select>` (English default, Indonesian) bound to a
`language` signal, sent in `StartRunRequest` and in both refine requests.

## Data flow

```
new_run.rs form
  ├─ per-repo base/integration selects ─┐
  ├─ language select ───────────────────┤
  └─ goal / verify / refine ────────────┤
                                        ▼
        StartRunRequest { workspace{repos[]{base,integration}}, goal, verify, language, refine_cost }
                                        ▼ POST /api/runs
        run::start ─ resolve repos ─► RepoRun{base_ref,integration_branch}
                   ─ language ───────► RunConfig.language
                                        ▼
        run_pipeline → plan_prompt(lang) → orchestrator → epic_prompt(lang)
        (refine flow: RefineQuestionsRequest.language → refine_questions_prompt(lang))
```

## Error handling

- `GET /api/repo/branches`: invalid/non-git path → `400` + message; UI falls
  back to free-text branch inputs.
- Empty branch list (new repo, no commits): endpoint returns `{ branches: [],
  current: None }`; base select shows the `current`/config value as the sole
  option and integration stays free-text.
- Existing `run::start` validations remain the guardrail for bad base refs and
  integration collisions.
- Unknown/absent `language` in a payload → `English` via `#[serde(default)]`.

## Testing

- **Unit (server):** parse of `git for-each-ref` output into `branches` +
  `current`; `language_directive` returns `""` for English and a non-empty
  Indonesian directive; each prompt builder includes the directive only for
  Indonesian.
- **HTTP (`crates/server/tests/http_api.rs`):** `GET /api/repo/branches`
  against a temp git repo returns its branches and current; `POST /api/runs`
  with `language: "indonesian"` and explicit per-repo base/integration starts a
  run and resolves the expected `RepoRun` values.
- **Resume:** a persisted run without a `language` field deserializes to English
  (serde default) and rebuilds cleanly.

## Files touched

- `crates/shared/src/lib.rs` — `Language`, `RepoBranchesResponse`, new request
  fields.
- `crates/server/src/worktree.rs` — branch-listing helper.
- `crates/server/src/http.rs` — `/api/repo/branches` handler + route; pass
  language through refine handlers.
- `crates/server/src/config.rs` — `language` params + `language_directive`.
- `crates/server/src/run.rs` — thread `language` into `RunConfig`; `RunHandle`
  field; resume/retry.
- `crates/server/src/lib.rs` — `RunConfig.language`; pass to prompt builders.
- `crates/server/src/orchestrator.rs` — `RunConfig.language` field; use in
  `epic_prompt` call.
- `crates/server/src/refine.rs` — `language` param through `questions`/`finalize`.
- `crates/server/src/run_store.rs` — persisted `language`.
- `crates/web/src/api.rs` — `list_branches`.
- `crates/web/src/views/new_run.rs` — per-repo branch controls + language select.
