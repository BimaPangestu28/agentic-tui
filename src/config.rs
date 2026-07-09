//! Knobs for the multi-stage orchestrator: permission mode, prose style, model
//! and tool selection per stage, and the Plan and Epic prompts.

pub const PERMISSION_MODE: &str = "acceptEdits";

const STYLE: &str = "Write directly and concisely. Do not use em dashes. Do \
not use contractions in English prose. Avoid AI-sounding filler.";

// Global cost brake: once accumulated cost reaches this, the orchestrator stops
// starting new epics. Epics already in flight still finish.
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

// Refine stage. Sharpening the goal does not need opus, so it defaults to
// sonnet. Read-only plus Write (for the result file), no Edit or Bash.
pub const MODEL_REFINE: &str = "sonnet";
pub const REFINE_TOOLS: &str = "Read,Glob,Grep,Write,WebSearch,WebFetch,Skill";
pub const REFINE_MAX_TURNS: u32 = 12;
pub const REFINE_BUDGET_USD: f64 = 0.20;
pub const REFINE_MAX_QUESTIONS: usize = 5;

// How many epics may run in parallel.
pub const MAX_PARALLEL_EPICS: usize = 3;

// Command run inside each epic worktree to decide if the epic passed.
pub const DEFAULT_VERIFY_CMD: &str = "make verify";

/// Prompt for the Plan stage. Claude explores the workspace and writes a
/// machine-readable plan.json (epics with tasks, dependencies, acceptance).
pub fn plan_prompt(goal: &str, out_path: &str) -> String {
    format!(
        "You are a Tech Lead decomposing a goal into an implementation plan for a \
repository. {style}\n\n\
GOAL:\n{goal}\n\n\
Step 1. Understand this repository with Glob and Grep. Detect language, \
framework, layout, and conventions so the plan fits the real code.\n\
Step 2. Break the goal into epics. Each epic is a coherent unit one engineer \
can implement in one session. Split each epic into concrete tasks. Record \
dependencies between epics with epic ids in depends_on. Keep epics as \
independent as possible so they can run in parallel.\n\
Step 3. Write ONLY a JSON file to {out} with this exact shape and nothing else:\n\
{{\"epics\":[{{\"id\":\"epic-1\",\"title\":\"...\",\"depends_on\":[],\
\"acceptance\":[\"verifiable item\"],\"tasks\":[{{\"id\":\"epic-1-t1\",\
\"title\":\"...\",\"detail\":\"...\"}}]}}]}}\n\
Use short kebab-case ids. Every depends_on entry must be an id that exists. Do \
not create cycles. Do not write any other file.\n\
Step 4. After writing, print the number of epics and a one line summary.",
        style = STYLE,
        goal = goal,
        out = out_path,
    )
}

/// Prompt for the first refine pass. Claude reads the repo, rewrites the goal to
/// be specific, and lists clarifying questions, writing them to a JSON file.
pub fn refine_questions_prompt(goal: &str, out_path: &str) -> String {
    format!(
        "You are a Tech Lead sharpening a goal before planning work on a \
repository. {style}\n\n\
GOAL:\n{goal}\n\n\
Step 1. Understand this repository with Glob and Grep so your rewrite and \
questions fit the real code.\n\
Step 2. Rewrite the goal so it is specific and actionable.\n\
Step 3. List at most {max} clarifying questions whose answers would materially \
change the plan. Ask only genuinely useful questions. If the goal is already \
clear, use an empty list.\n\
Step 4. Write ONLY a JSON file to {out} with this exact shape and nothing else:\n\
{{\"refined_goal\":\"...\",\"questions\":[\"...\"]}}\n\
Do not write any other file.",
        style = STYLE,
        goal = goal,
        max = REFINE_MAX_QUESTIONS,
        out = out_path,
    )
}

/// Prompt for the second refine pass. Given the original goal and the user's
/// answers, produce one final goal, writing it to the same JSON file.
pub fn refine_finalize_prompt(goal: &str, answers: &[(String, String)], out_path: &str) -> String {
    let qa: String = answers
        .iter()
        .map(|(question, answer)| {
            let answer = if answer.is_empty() {
                "(no answer)"
            } else {
                answer
            };
            format!("Q: {question}\nA: {answer}\n")
        })
        .collect();
    format!(
        "You are a Tech Lead finalizing a goal before planning. {style}\n\n\
ORIGINAL GOAL:\n{goal}\n\n\
CLARIFICATIONS:\n{qa}\n\
Produce one specific, actionable goal statement that folds in the answers \
above. Write ONLY a JSON file to {out} with this exact shape and nothing \
else:\n\
{{\"refined_goal\":\"...\",\"questions\":[]}}\n\
Do not write any other file.",
        style = STYLE,
        goal = goal,
        qa = qa,
        out = out_path,
    )
}

/// Prompt for one epic session. Runs inside that epic's worktree and implements
/// the epic's tasks, then runs the verification command itself as a check.
pub fn epic_prompt(goal: &str, epic: &crate::plan::Epic, verify_cmd: &str) -> String {
    let tasks: String = epic
        .tasks
        .iter()
        .map(|task| format!("- {} ({}): {}\n", task.title, task.id, task.detail))
        .collect();
    let acceptance: String = epic
        .acceptance
        .iter()
        .map(|item| format!("- {item}\n"))
        .collect();
    format!(
        "You are implementing one epic of a larger goal, working in an isolated \
git worktree. {style}\n\n\
OVERALL GOAL:\n{goal}\n\n\
THIS EPIC: {title}\n\n\
TASKS:\n{tasks}\n\
ACCEPTANCE CRITERIA:\n{acceptance}\n\
Implement every task with Edit and Write. Follow existing conventions in the \
repository. When done, run `{verify}` with Bash and fix anything it reports \
until it passes. Do not stop to ask questions, this run is non-interactive. \
Commit your work with git when the epic is complete.",
        style = STYLE,
        goal = goal,
        title = epic.title,
        tasks = tasks,
        acceptance = acceptance,
        verify = verify_cmd,
    )
}
