//! The goal-refine step: a claude pass rewrites the goal and proposes
//! clarifying questions written to `.agentic-refine.json`, the user answers
//! them, a second pass finalizes the goal, and the user confirms it. Reuses the
//! `plan.json` pattern: each pass writes a JSON file we parse here.

use serde::Deserialize;

/// The JSON a refine pass writes to `.agentic-refine.json`.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct RefineResult {
    pub refined_goal: String,
    #[serde(default)]
    pub questions: Vec<String>,
}

/// The result of the whole refine flow. `goal` is `None` only when the user
/// cancelled the run.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RefineOutcome {
    pub goal: Option<String>,
    pub cost: f64,
}

/// Parse the JSON a refine pass wrote. A missing `questions` defaults to empty;
/// an empty `refined_goal` or malformed JSON is an error, which the caller turns
/// into a fall back to the original goal.
#[allow(dead_code)]
pub fn parse_refine(json: &str) -> anyhow::Result<RefineResult> {
    let result: RefineResult = serde_json::from_str(json)?;
    if result.refined_goal.trim().is_empty() {
        anyhow::bail!("refine result has an empty refined_goal");
    }
    Ok(result)
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
