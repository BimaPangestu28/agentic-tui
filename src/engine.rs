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
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue, // non-JSON lines are ignored
        };
        match value.get("type").and_then(|t| t.as_str()) {
            Some("system") if value.get("subtype").and_then(|s| s.as_str()) == Some("init") => {
                let model = value.get("model").and_then(|m| m.as_str()).unwrap_or("");
                let _ = tx.send(AppEvent::StageLog {
                    tag: tag.to_string(),
                    line: format!("session init ({model})"),
                });
            }
            Some("assistant") => {
                if let Some(content) = value
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for block in content {
                        match block.get("type").and_then(|t| t.as_str()) {
                            Some("text") => {
                                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                    let first = text.trim().lines().next().unwrap_or("").trim();
                                    if !first.is_empty() {
                                        let preview: String = first.chars().take(120).collect();
                                        let _ = tx.send(AppEvent::StageAssistant {
                                            tag: tag.to_string(),
                                            text: preview,
                                        });
                                    }
                                }
                            }
                            Some("tool_use") => {
                                if let Some(name) = block.get("name").and_then(|n| n.as_str()) {
                                    let _ = tx.send(AppEvent::StageTool {
                                        tag: tag.to_string(),
                                        name: name.to_string(),
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            Some("result") => {
                cost = value
                    .get("total_cost_usd")
                    .and_then(|c| c.as_f64())
                    .unwrap_or(0.0);
                let is_error = value
                    .get("is_error")
                    .and_then(|e| e.as_bool())
                    .unwrap_or(false);
                let subtype = value.get("subtype").and_then(|s| s.as_str()).unwrap_or("");
                ok = !is_error && (subtype.is_empty() || subtype == "success");
            }
            _ => {}
        }
    }

    let _ = child.wait().await;
    Ok(Outcome { cost, ok })
}
