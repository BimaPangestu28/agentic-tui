//! The goal-refine step: a claude pass rewrites the goal and proposes
//! clarifying questions written to `.agentic-refine.json`; the browser
//! collects the user's answers, then a second pass finalizes the goal.
//! Reuses the `plan.json` pattern: each pass writes a JSON file we parse here.

use serde::Deserialize;

use std::path::Path;

use tokio::sync::mpsc;

use shared::{Language, StageEvent};

use crate::config;
use crate::engine;

/// The JSON a refine pass writes to `.agentic-refine.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct RefineResult {
    pub refined_goal: String,
    #[serde(default)]
    pub questions: Vec<String>,
}

/// Parse the JSON a refine pass wrote. A missing `questions` defaults to empty;
/// an empty `refined_goal` or malformed JSON is an error, which the caller turns
/// into a fall back to the original goal.
pub fn parse_refine(json: &str) -> anyhow::Result<RefineResult> {
    let result: RefineResult = serde_json::from_str(json)?;
    if result.refined_goal.trim().is_empty() {
        anyhow::bail!("refine result has an empty refined_goal");
    }
    Ok(result)
}

/// Run one refine pass. Returns the cost incurred (which is real money even if
/// the session failed to write a usable file) together with the parsed result
/// or the read/parse error. The cost is 0.0 only when the session did not run.
async fn run_refine_pass(
    root: &Path,
    prompt: &str,
    out_path: &Path,
    tx: &mpsc::UnboundedSender<StageEvent>,
) -> (f64, anyhow::Result<RefineResult>) {
    let _ = std::fs::remove_file(out_path);
    let spec = engine::StageSpec {
        tag: "refine",
        cwd: root,
        model: config::MODEL_REFINE,
        tools: config::REFINE_TOOLS,
        max_turns: config::REFINE_MAX_TURNS,
        prompt,
    };
    let outcome = match engine::run_stage(&spec, tx).await {
        Ok(outcome) => outcome,
        Err(e) => return (0.0, Err(e)),
    };
    let result = std::fs::read_to_string(out_path)
        .map_err(|e| anyhow::anyhow!(".agentic-refine.json was not written: {e}"))
        .and_then(|text| parse_refine(&text));
    (outcome.cost, result)
}

/// Run refine pass 1 for the HTTP API: rewrite the goal and gather
/// clarifying questions, truncated to `REFINE_MAX_QUESTIONS`. On failure
/// (the pass errored, or `.agentic-refine.json` was unreadable or
/// unparseable), falls back to the original goal with no questions. The cost
/// is real either way, since it is billed once the session runs.
pub async fn questions(root: &Path, goal: &str, language: Language) -> (String, Vec<String>, f64) {
    let (tx, _rx) = mpsc::unbounded_channel::<StageEvent>();
    let out_path = root.join(".agentic-refine.json");
    let (cost, result) = run_refine_pass(
        root,
        &config::refine_questions_prompt(goal, &out_path.to_string_lossy(), language),
        &out_path,
        &tx,
    )
    .await;
    match result {
        Ok(result) => {
            let mut questions = result.questions;
            questions.truncate(config::REFINE_MAX_QUESTIONS);
            (result.refined_goal, questions, cost)
        }
        Err(_) => (goal.to_string(), Vec::new(), cost),
    }
}

/// Run refine pass 2 for the HTTP API: fold the user's answers into a final
/// goal. On failure, falls back to the original goal; the cost is still
/// reported.
pub async fn finalize(
    root: &Path,
    goal: &str,
    answers: &[(String, String)],
    language: Language,
) -> (String, f64) {
    let (tx, _rx) = mpsc::unbounded_channel::<StageEvent>();
    let out_path = root.join(".agentic-refine.json");
    let (cost, result) = run_refine_pass(
        root,
        &config::refine_finalize_prompt(goal, answers, &out_path.to_string_lossy(), language),
        &out_path,
        &tx,
    )
    .await;
    match result {
        Ok(result) => (result.refined_goal, cost),
        Err(_) => (goal.to_string(), cost),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_goal_with_questions() {
        let json = r#"{"refined_goal":"Add a health check endpoint at /healthz","questions":["Which port?","Auth required?"]}"#;
        let result = parse_refine(json).unwrap();
        assert_eq!(
            result.refined_goal,
            "Add a health check endpoint at /healthz"
        );
        assert_eq!(result.questions, vec!["Which port?", "Auth required?"]);
    }

    #[test]
    fn a_missing_questions_field_defaults_to_empty() {
        let result = parse_refine(r#"{"refined_goal":"Do the thing"}"#).unwrap();
        assert!(result.questions.is_empty());
    }

    #[test]
    fn an_empty_refined_goal_is_an_error() {
        assert!(parse_refine(r#"{"refined_goal":"   ","questions":[]}"#).is_err());
    }

    #[test]
    fn malformed_json_is_an_error() {
        assert!(parse_refine("not json at all").is_err());
    }
}
