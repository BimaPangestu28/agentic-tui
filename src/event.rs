//! Events that flow through the channel to the UI.

use crossterm::event::KeyEvent;

#[derive(Debug, Clone)]
pub struct EpicMeta {
    pub id: String,
    pub title: String,
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum AppEvent {
    Input(KeyEvent),
    Tick,
    // Streaming from a session. `tag` is "plan" or an epic id.
    StageLog { tag: String, line: String },
    StageAssistant { tag: String, text: String },
    StageTool { tag: String, name: String },
    // Lifecycle.
    PlanReady { epics: Vec<EpicMeta> },
    EpicStarted { id: String, title: String },
    EpicVerifying { id: String },
    EpicSucceeded { id: String, cost: f64 },
    EpicFailed { id: String, reason: String },
    EpicSkipped { id: String },
    EpicMerged { id: String },
    EpicConflict { id: String },
    Cost(f64),
    Fatal(String),
    Done,
}
