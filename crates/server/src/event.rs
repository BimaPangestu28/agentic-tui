//! Events that flow through the channel to the UI.
//!
//! The wire events (the terminal-independent subset the server also intends
//! to forward to a browser) live in `shared::StageEvent`. Only the two
//! variants that make sense solely on a terminal, raw key input and the
//! redraw tick, stay here.

use crossterm::event::KeyEvent;

pub use shared::EpicMeta;

#[derive(Debug, Clone)]
pub enum AppEvent {
    Stage(shared::StageEvent),
    Input(KeyEvent),
    Tick,
}
