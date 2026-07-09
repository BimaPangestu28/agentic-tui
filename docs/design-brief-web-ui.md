# Design Brief: Agentic Orchestrator Web UI

You are designing the web UI for **agentic-tui**, an all-Rust local tool. This
brief is self-contained: everything you need to produce a design is below. Read
it fully before starting.

## 1. What the product is

`agentic-tui` takes a software goal (for example "add per-tenant rate limiting to
the API gateway"), asks Claude Code to break it into **epics**, then runs each
epic as an isolated `claude -p` session inside its own git worktree, verifies it,
and merges passing work into an integration branch. It is a **local, single-user
developer tool**: the user runs it on their own machine, it starts a loopback web
server (127.0.0.1) and opens the browser. There is no multi-user, no auth, no
public deployment.

The mental model is a **build/CI-style orchestration dashboard**: you kick off a
run and watch epics move across a kanban board (Todo -> In progress -> Review ->
Done, or Blocked) while a live log streams and a budget meter ticks up.

## 2. Current state (what you are improving)

The UI is a **Leptos (Rust/WASM) client-side single-page app**, styled with a
single plain **CSS file** (`crates/web/style.css`). There is a first-pass dark
theme in place, but it is utilitarian and unrefined. Your job is to design a
polished, coherent visual system. The markup is fixed Leptos components that
already emit **semantic class names** (listed per screen below); your design must
be expressible as CSS targeting those class names (plus, if useful, small
additive markup like wrapper elements or badges, which can be added on request).

**Implementation reality to respect:**
- Pure CSS only. No CSS framework, no build step beyond `trunk`, no external
  CDNs, no remote fonts/images (the app is served offline from an embedded
  bundle). Use system font stacks. Any icon should be a Unicode glyph or inline
  SVG.
- Must be responsive (usable from ~1000px down to a narrow window). Wide content
  (the kanban board, the log) should scroll inside its own container, never break
  the page layout.
- Accessible: sufficient contrast (WCAG AA), visible focus states, do not rely on
  color alone to convey epic status (pair color with a label/icon).
- A dark theme is the current default and fits the tool. You may also design a
  light theme; if so, drive both from CSS custom properties so switching is a
  variable swap.

## 3. Design goals

1. Make it read as a **real product**, not a raw HTML page: clear hierarchy,
   spacing rhythm, considered typography, purposeful color.
2. The **live run dashboard is the hero screen** — the kanban board, budget
   meter, and streaming log should feel alive and legible at a glance.
3. Keep it **calm and information-dense** the way good developer tools are
   (think Linear, Vercel dashboard, GitHub Actions), not flashy.
4. A cohesive design system: a small, documented set of color, type, spacing, and
   component tokens reused across all three screens.

## 4. The three screens to design

### Screen A — Workspaces (route `/`, the landing page)

Purpose: pick a git repository to run against, or add new ones.

Content and states:
- A **list of configured workspaces**. Each row shows a `name` (short label) and
  a `path` (absolute filesystem path, e.g. `/home/user/projects/greentic`).
  Clicking a workspace navigates to the New-run screen for it. There may be 0 to
  ~40 workspaces.
- An **"Add workspace" panel** (collapsible; auto-expanded when the list is
  empty): a text input for a folder path, a **Scan** button, then a **checklist**
  of discovered git repos (each with name + path + a checkbox, pre-checked), and
  a **Save** button.
- States: initial load, empty (no workspaces yet), scanning (button shows
  "Scanning..."), scan results present, saving, and error messages.

Class hooks: `.workspaces-view`, `.workspace-list`, `.workspace-row`,
`.workspace-name`, `.workspace-path`, `.add-workspace-panel`, `.scan-results`,
`.scan-result-row`, `.error`.

### Screen B — New run (route `/run/new?workspace=<name>`)

Purpose: enter the goal and options, optionally run a clarification pass, start
the run.

Content and states:
- A **form**: a multi-line **goal** textarea (this is the primary input, give it
  weight), optional **base branch** and **integration branch** text inputs
  (advanced/secondary), a **verify command** input, and a **"refine before
  planning"** checkbox.
- The optional **refine flow** is a small multi-step sequence rendered inline:
  1. `Answering`: a list of clarifying questions, each with an answer input.
  2. `Confirming`: the refined goal shown in an editable field to accept.
  3. `Submitting`, and an `Error` state showing a validation message inline.
- Design the form to feel focused; the goal should dominate, options secondary,
  the refine steps should read as a guided sub-flow (cards or a stepper).

Class hooks: `.new-run-view`, `.new-run-form`, `.field`, `.field.checkbox`,
`.hint`, `.refine-answering`, `.refine-question`, `.refine-confirm`, `.error`.

### Screen C — Run dashboard (route `/run/:id`, the hero screen)

Purpose: watch a live run. State arrives over a WebSocket as full snapshots.

Content and states:
- **Header**: the run `goal` (may be multi-line), the `workspace` name, and a
  **budget meter** — a progress bar plus text `"$0.4210 / $10.0000"` (spent /
  total). The bar fills as cost accrues.
- An **Abort** button (destructive; ends the run).
- A **five-column kanban board**, columns in this fixed order with these exact
  labels: **Todo**, **In progress**, **Review**, **Done**, **Blocked**. Each
  column has a header and a stack of **epic cards**. A card shows the epic
  `title`, its `id` (short, monospace), a `status` label, and, only in Todo, an
  **"on hold"** marker for epics still waiting on dependencies. Give each column a
  distinct, meaningful accent (suggested: Todo neutral/gray, In progress amber,
  Review purple/blue, Done green, Blocked red) — but never rely on color alone.
- A **live log pane**: a scrolling, monospace stream of stage output lines. It
  should have a bounded height and scroll internally.
- **Final report** (shown when the run finishes): a list of epics with final
  status and per-epic cost, a total cost, and a reminder line that merged work is
  on the integration branch.
- States: `Connecting...` (before the first snapshot), running (live updates),
  finished (report shown), and error.

Class hooks: `.run-view`, `.run-header`, `.run-goal`, `.run-workspace`,
`.budget-bar`, `.budget-bar-fill`, `.budget-text`, `.kanban-board`,
`.kanban-column` (contains an `<h3>` header and `.kanban-cards`), `.kanban-card`,
`.kanban-card-title`, `.kanban-card-id`, `.kanban-card-status`,
`.kanban-card-hold`, `.log-pane`, `.log-line`, `.final-report`, `.report-rows`,
`.report-row`, `.report-id`, `.report-title`, `.report-status`, `.report-cost`,
`.report-total`, `.report-hint`, `.error`.

There is also a global **app bar** (`.app-bar`) across the top on every screen:
the product name "Agentic Orchestrator" (with a hexagon glyph) linking back to
Workspaces. The page content lives in `.app-main` (a centered, max-width column).

## 5. Data shapes (use realistic content in mockups)

- Workspace: `{ name: "greentic", path: "/home/bima/projects/greentic", base?: "develop", integration?: "agentic-wip" }`.
- Epic: `{ id: "epic-2", title: "Add /healthz endpoint", status, cost: 0.83 }`.
  Statuses map to columns: Pending->Todo, Running->In progress, Verifying->Review,
  Merged->Done, Failed/Skipped/Conflict->Blocked.
- Run: `{ goal, workspace, total_cost: 0.42, budget: 10.0, phase: Planning|Running|Done|Failed, epics: [...], log: ["[plan] session init (claude-opus)", "[epic-1] tool: Edit", ...] }`.
- A realistic run has 3-8 epics spread across the columns, a dozen-plus log lines,
  and cost in the single-digit dollars.

## 6. Deliverable

Any of these is fine; pick what the requester asks for, else default to the first:

1. **Directly restyle the app**: rewrite/extend `crates/web/style.css` (and, if
   needed, propose small additive markup changes to the Leptos views in
   `crates/web/src/views/*.rs` and `main.rs`). Keep it pure CSS, self-contained,
   responsive, accessible. This is the most useful outcome because it ships.
2. **A Figma design** of the three screens plus the design tokens, if a Figma
   workflow is requested (a Figma integration is available).
3. **Static HTML/CSS mockups** of the three screens (self-contained files) for
   review before implementation.

Whatever the format, also deliver the **design system**: the color palette (as
named tokens), type scale, spacing scale, radius/border/shadow tokens, and the
component specs (buttons, inputs, cards, badges, the kanban column, the budget
meter, the log line). Document them so they can live in CSS custom properties.

## 7. Tone and brand

Developer-tool, precise, calm, a little technical. Dark-first. The one brand
motif is the hexagon "⬡". No marketing gloss. Favor legibility and density over
decoration. The current palette (a GitHub-dark-like set) is a reasonable starting
point but you are free to define your own — just keep contrast and the
status-color semantics (green = done/good, red = blocked/bad, amber = in
progress, neutral = todo).

## 8. Repository pointers (for the implementation deliverable)

- Stylesheet: `crates/web/style.css` (linked from `crates/web/index.html` via a
  `data-trunk rel="css"` link).
- Views: `crates/web/src/views/workspaces.rs`, `new_run.rs`, `run.rs`; shell and
  app bar in `crates/web/src/main.rs`.
- Build the web UI to check your work: `cd crates/web && trunk build` (requires
  the `wasm32-unknown-unknown` target and `trunk`). Run the whole app with
  `make run` from the repo root, which opens the browser.
- Shared data types (for exact field names) are in `crates/shared/src/lib.rs`.
