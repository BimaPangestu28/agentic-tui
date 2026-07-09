//! View components for the web UI, one module per route.

pub mod dashboard;
pub mod new_run;
pub mod run;
pub mod workspaces;

pub use dashboard::Dashboard;
pub use new_run::NewRun;
pub use run::Run;
pub use workspaces::Workspaces;
