//! Knobs for the PRD generator: model, budget, permission mode, and PRD prompt.

pub const BUDGET_USD: f64 = 2.0;

// Model for the PRD. The PRD quality determines the accuracy of the subsequent
// Claude Code implementation session, so the default is opus. Switch to
// "sonnet" if you want to save on your limit.
pub const MODEL_PRD: &str = "opus";

// Read-only tools + Write, plus Skill so the run can invoke Superpowers skills
// (for example superpowers:writing-plans for the task breakdown). Allowed tools
// are pre-approved, so an unattended run does not block asking for permission.
pub const PRD_TOOLS: &str = "Read,Glob,Grep,Write,WebSearch,WebFetch,Skill";
pub const PRD_MAX_TURNS: u32 = 15;
pub const PERMISSION_MODE: &str = "acceptEdits";

#[derive(Clone)]
pub struct Stage {
    pub model: &'static str,
    pub tools: &'static str,
    pub max_turns: u32,
}

pub fn prd_stage() -> Stage {
    Stage {
        model: MODEL_PRD,
        tools: PRD_TOOLS,
        max_turns: PRD_MAX_TURNS,
    }
}

const STYLE: &str = "Write directly and concisely. Do not use em dashes. Do \
not use contractions in English prose. Avoid AI-sounding filler.";

/// PRD prompt. The goal and output path are injected from Rust so we know
/// exactly where the file is written and can show the path at the end.
pub fn prd_prompt(goal: &str, out_path: &str) -> String {
    format!(
        "You are a Tech Lead writing a Product Requirements Document. The goal \
below will be implemented later in a separate Claude Code session, so this PRD \
is the single source of truth for that session. Make it concrete, grounded in \
the actual repository, and testable. {style}\n\n\
GOAL:\n{goal}\n\n\
Step 1. Understand this repository with Glob and Grep. Detect language, \
framework, layout, and existing conventions so the PRD fits the real code, not \
a generic assumption.\n\
Step 2. If the goal is genuinely ambiguous on scope, make a reasonable \
decision and record it under Assumptions. Do not stop to ask, this run is \
non-interactive.\n\
Step 3. If a pattern or library needs verifying, do a short WebSearch. Keep it \
light.\n\
Step 4. Invoke the superpowers:writing-plans skill to structure the ordered \
Implementation task breakdown. Run it non-interactively: do not stop to ask \
questions, use what you learned about the repository as the input, and fold the \
resulting plan into the PRD as the task breakdown checklist. If the skill is \
unavailable, continue without it.\n\
Step 5. Write the PRD to {out} with these sections: Summary, Problem \
statement, Goals and non-goals, Assumptions, User stories, Functional \
requirements, Acceptance criteria as a checklist of verifiable items, \
Technical scope referencing real files and modules in THIS repo by path, Out \
of scope, Testing strategy per acceptance criterion, Open questions, and the \
ordered Implementation task breakdown from Step 4 as a checklist.\n\
Step 6. After writing, print the path to the PRD and a two line scope summary.",
        style = STYLE,
        goal = goal,
        out = out_path,
    )
}

// --- Orchestrator knobs (single-stage items above are removed in the switchover) ---

// Global cost circuit breaker across every session in a run.
pub const GLOBAL_BUDGET_USD: f64 = 10.0;
// Budget for a single stage (plan or one epic).
pub const EPIC_BUDGET_USD: f64 = 2.0;

// Models. Plan quality drives epic accuracy, so plan defaults to opus.
pub const MODEL_PLAN: &str = "opus";
pub const MODEL_EPIC: &str = "sonnet";

// Read-only + Write for planning. Adds Edit and Bash for epics that write code.
pub const PLAN_TOOLS: &str = "Read,Glob,Grep,Write,WebSearch,WebFetch,Skill";
pub const EPIC_TOOLS: &str = "Read,Glob,Grep,Edit,Write,Bash,WebSearch,WebFetch,Skill";

pub const PLAN_MAX_TURNS: u32 = 20;
pub const EPIC_MAX_TURNS: u32 = 40;

// How many epics may run in parallel.
pub const MAX_PARALLEL_EPICS: usize = 3;

// Command run inside each epic worktree to decide if the epic passed.
pub const DEFAULT_VERIFY_CMD: &str = "make verify";
