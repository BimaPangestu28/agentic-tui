//! Events that flow through the channel to the UI.

use crossterm::event::KeyEvent;

#[derive(Debug)]
pub enum AppEvent {
    Input(KeyEvent),
    Tick,
    Started { model: String, out_path: String },
    Log(String),
    Assistant(String),
    ToolUse(String),
    Cost(f64),
    Finished { cost: f64, ok: bool },
    Fatal(String),
}
