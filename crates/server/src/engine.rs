//! Engine: drives Claude Code headless (`claude -p`) as a subprocess, reads the
//! stream-json NDJSON line by line, and emits tagged events to the UI. Used by
//! both the Plan stage and each epic session.

use std::path::Path;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;

use crate::config;
use crate::event::AppEvent;
use shared::StageEvent;

pub struct StageSpec<'a> {
    pub tag: &'a str,
    pub cwd: &'a Path,
    pub model: &'a str,
    pub tools: &'a str,
    pub max_turns: u32,
    pub budget_usd: f64,
    pub prompt: &'a str,
}

pub struct Outcome {
    pub cost: f64,
    pub ok: bool,
}

/// A meaningful message decoded from one NDJSON line of the `claude` stream.
/// One line can yield several messages (an assistant turn carries a content
/// array of text and tool_use blocks), so parsing returns a `Vec`.
#[derive(Debug, PartialEq)]
enum StageMessage {
    Init { model: String },
    Assistant { preview: String },
    Tool { name: String },
    Result { cost: f64, ok: bool },
}

/// Preview text sent to the UI for an assistant text block: the first non-empty
/// line, capped at 120 characters. Kept separate so the cap is testable.
fn assistant_preview(text: &str) -> Option<String> {
    let first = text.trim().lines().next().unwrap_or("").trim();
    if first.is_empty() {
        return None;
    }
    Some(first.chars().take(120).collect())
}

/// Decode one line of `claude`'s stream-json output into zero or more messages.
/// Blank lines, non-JSON lines, and JSON we do not act on yield an empty `Vec`.
fn parse_stage_line(line: &str) -> Vec<StageMessage> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return Vec::new(); // non-JSON lines are ignored
    };
    match value.get("type").and_then(|t| t.as_str()) {
        Some("system") if value.get("subtype").and_then(|s| s.as_str()) == Some("init") => {
            let model = value.get("model").and_then(|m| m.as_str()).unwrap_or("");
            vec![StageMessage::Init {
                model: model.to_string(),
            }]
        }
        Some("assistant") => {
            let Some(content) = value
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            else {
                return Vec::new();
            };
            let mut out = Vec::new();
            for block in content {
                match block.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(preview) = block
                            .get("text")
                            .and_then(|t| t.as_str())
                            .and_then(assistant_preview)
                        {
                            out.push(StageMessage::Assistant { preview });
                        }
                    }
                    Some("tool_use") => {
                        if let Some(name) = block.get("name").and_then(|n| n.as_str()) {
                            out.push(StageMessage::Tool {
                                name: name.to_string(),
                            });
                        }
                    }
                    _ => {}
                }
            }
            out
        }
        Some("result") => {
            let cost = value
                .get("total_cost_usd")
                .and_then(|c| c.as_f64())
                .unwrap_or(0.0);
            let is_error = value
                .get("is_error")
                .and_then(|e| e.as_bool())
                .unwrap_or(false);
            let subtype = value.get("subtype").and_then(|s| s.as_str()).unwrap_or("");
            let ok = !is_error && (subtype.is_empty() || subtype == "success");
            vec![StageMessage::Result { cost, ok }]
        }
        _ => Vec::new(),
    }
}

/// Run a single `claude -p` invocation to completion, parsing its event stream.
pub async fn run_stage(
    spec: &StageSpec<'_>,
    tx: &UnboundedSender<AppEvent>,
) -> anyhow::Result<Outcome> {
    let mut cmd = Command::new("claude");
    cmd.arg("-p")
        .arg(spec.prompt)
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose")
        .arg("--model")
        .arg(spec.model)
        .arg("--allowedTools")
        .arg(spec.tools)
        .arg("--permission-mode")
        .arg(config::PERMISSION_MODE)
        .arg("--max-turns")
        .arg(spec.max_turns.to_string())
        .arg("--max-budget-usd")
        .arg(format!("{:.2}", spec.budget_usd))
        .current_dir(spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| {
        anyhow::anyhow!("failed to spawn `claude` (make sure the CLI is installed on PATH): {e}")
    })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("could not capture stdout"))?;
    let mut lines = BufReader::new(stdout).lines();

    let mut cost = 0.0f64;
    let mut ok = false;
    let tag = spec.tag;

    while let Some(line) = lines.next_line().await? {
        for message in parse_stage_line(&line) {
            match message {
                StageMessage::Init { model } => {
                    let _ = tx.send(AppEvent::Stage(StageEvent::StageLog {
                        tag: tag.to_string(),
                        line: format!("session init ({model})"),
                    }));
                }
                StageMessage::Assistant { preview } => {
                    let _ = tx.send(AppEvent::Stage(StageEvent::StageAssistant {
                        tag: tag.to_string(),
                        text: preview,
                    }));
                }
                StageMessage::Tool { name } => {
                    let _ = tx.send(AppEvent::Stage(StageEvent::StageTool {
                        tag: tag.to_string(),
                        name,
                    }));
                }
                StageMessage::Result {
                    cost: line_cost,
                    ok: line_ok,
                } => {
                    cost = line_cost;
                    ok = line_ok;
                }
            }
        }
    }

    let _ = child.wait().await;
    Ok(Outcome { cost, ok })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_and_non_json_lines_are_ignored() {
        assert!(parse_stage_line("").is_empty());
        assert!(parse_stage_line("   ").is_empty());
        assert!(parse_stage_line("not json at all").is_empty());
        assert!(parse_stage_line(r#"{"type":"unknown"}"#).is_empty());
    }

    #[test]
    fn init_line_yields_the_model() {
        let line = r#"{"type":"system","subtype":"init","model":"claude-opus"}"#;
        assert_eq!(
            parse_stage_line(line),
            vec![StageMessage::Init {
                model: "claude-opus".to_string()
            }]
        );
    }

    #[test]
    fn init_without_model_falls_back_to_empty() {
        let line = r#"{"type":"system","subtype":"init"}"#;
        assert_eq!(
            parse_stage_line(line),
            vec![StageMessage::Init {
                model: String::new()
            }]
        );
    }

    #[test]
    fn a_non_init_system_line_is_ignored() {
        assert!(parse_stage_line(r#"{"type":"system","subtype":"other"}"#).is_empty());
    }

    #[test]
    fn assistant_text_preview_keeps_first_line_only() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"first line\nsecond line"}]}}"#;
        assert_eq!(
            parse_stage_line(line),
            vec![StageMessage::Assistant {
                preview: "first line".to_string()
            }]
        );
    }

    #[test]
    fn assistant_preview_is_capped_at_120_chars() {
        let long = "x".repeat(200);
        let line = format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{long}"}}]}}}}"#
        );
        match parse_stage_line(&line).as_slice() {
            [StageMessage::Assistant { preview }] => assert_eq!(preview.chars().count(), 120),
            other => panic!("expected one assistant message, got {other:?}"),
        }
    }

    #[test]
    fn blank_assistant_text_is_dropped() {
        let line =
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"   \n  "}]}}"#;
        assert!(parse_stage_line(line).is_empty());
    }

    #[test]
    fn an_assistant_turn_can_yield_text_and_tools_together() {
        let line = r#"{"type":"assistant","message":{"content":[
            {"type":"text","text":"looking around"},
            {"type":"tool_use","name":"Grep"},
            {"type":"tool_use","name":"Read"}
        ]}}"#;
        assert_eq!(
            parse_stage_line(line),
            vec![
                StageMessage::Assistant {
                    preview: "looking around".to_string()
                },
                StageMessage::Tool {
                    name: "Grep".to_string()
                },
                StageMessage::Tool {
                    name: "Read".to_string()
                },
            ]
        );
    }

    #[test]
    fn result_success_is_ok_with_cost() {
        let line =
            r#"{"type":"result","subtype":"success","is_error":false,"total_cost_usd":0.42}"#;
        assert_eq!(
            parse_stage_line(line),
            vec![StageMessage::Result {
                cost: 0.42,
                ok: true
            }]
        );
    }

    #[test]
    fn result_with_error_flag_is_not_ok() {
        let line = r#"{"type":"result","subtype":"success","is_error":true,"total_cost_usd":0.1}"#;
        assert_eq!(
            parse_stage_line(line),
            vec![StageMessage::Result {
                cost: 0.1,
                ok: false
            }]
        );
    }

    #[test]
    fn result_with_a_failure_subtype_is_not_ok() {
        let line = r#"{"type":"result","subtype":"error_max_turns","is_error":false,"total_cost_usd":0.3}"#;
        assert_eq!(
            parse_stage_line(line),
            vec![StageMessage::Result {
                cost: 0.3,
                ok: false
            }]
        );
    }

    #[test]
    fn result_without_cost_defaults_to_zero() {
        let line = r#"{"type":"result","subtype":"success","is_error":false}"#;
        assert_eq!(
            parse_stage_line(line),
            vec![StageMessage::Result {
                cost: 0.0,
                ok: true
            }]
        );
    }
}
