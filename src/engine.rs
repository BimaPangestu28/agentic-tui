//! Engine: drives Claude Code headless (`claude -p`) as a subprocess, reads the
//! stream-json NDJSON line by line, and emits events to the UI.

use std::path::Path;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;

use crate::config::{self, Stage};
use crate::event::AppEvent;

pub struct Outcome {
    pub cost: f64,
    pub ok: bool,
}

/// Run a single `claude -p` invocation. Each stage runs its own internal agent
/// loop to completion. We only parse its event stream.
pub async fn run_stage(
    repo: &Path,
    stage: &Stage,
    prompt: &str,
    tx: &UnboundedSender<AppEvent>,
) -> anyhow::Result<Outcome> {
    let mut cmd = Command::new("claude");
    cmd.arg("-p")
        .arg(prompt)
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose")
        .arg("--model")
        .arg(stage.model)
        .arg("--allowedTools")
        .arg(stage.tools)
        .arg("--permission-mode")
        .arg(config::PERMISSION_MODE)
        .arg("--max-turns")
        .arg(stage.max_turns.to_string())
        // per-stage budget; the global circuit breaker lives in the pipeline
        .arg("--max-budget-usd")
        .arg("2.00")
        .current_dir(repo)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // stderr is nulled so it does not block or corrupt the TUI screen.
        // For debugging, pipe stderr and read it in a separate task.
        .stderr(Stdio::null());

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

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue, // non-JSON lines are ignored
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("system") => {
                if v.get("subtype").and_then(|s| s.as_str()) == Some("init") {
                    let model = v.get("model").and_then(|m| m.as_str()).unwrap_or("");
                    let _ = tx.send(AppEvent::Log(format!("session init ({model})")));
                }
            }
            Some("assistant") => {
                if let Some(content) = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for block in content {
                        match block.get("type").and_then(|t| t.as_str()) {
                            Some("text") => {
                                if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                                    let first = t.trim().lines().next().unwrap_or("").trim();
                                    if !first.is_empty() {
                                        let preview: String = first.chars().take(120).collect();
                                        let _ = tx.send(AppEvent::Assistant(preview));
                                    }
                                }
                            }
                            Some("tool_use") => {
                                if let Some(name) = block.get("name").and_then(|n| n.as_str()) {
                                    let _ = tx.send(AppEvent::ToolUse(name.to_string()));
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            Some("result") => {
                cost = v
                    .get("total_cost_usd")
                    .and_then(|c| c.as_f64())
                    .unwrap_or(0.0);
                let is_error = v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);
                let subtype = v.get("subtype").and_then(|s| s.as_str()).unwrap_or("");
                ok = !is_error && (subtype.is_empty() || subtype == "success");
            }
            _ => {}
        }
    }

    let _ = child.wait().await;
    Ok(Outcome { cost, ok })
}
