//! State rendered by the UI: the run phase, one view per epic, and a log.
//!
//! The types themselves now live in the `shared` crate so they can be reused
//! by a future wasm web client. Re-exported here so existing `crate::app::X`
//! paths keep compiling.

pub use shared::{is_on_hold, kanban_column, App, EpicStatus, EpicView, KanbanColumn, Phase};
