//! herdr-glowr — a herdr-native markdown viewer.
//!
//! Browse a markdown document and leave line-range comments, sent back to the agent
//! (or the clipboard) — entirely in a herdr pane.
//!
//! This crate is split into a thin binary (`src/main.rs`) and this library. `src/app.rs`
//! owns the terminal lifecycle and event loop; it maps input events onto `App` methods
//! and renders with [`ui`].
#![forbid(unsafe_code)]
pub mod app;
pub mod cli;
pub mod comments;
pub mod config;
pub mod export;
pub mod file_list;
pub mod herdr;
pub mod highlight;
#[macro_use]
pub mod log;
pub mod markdown;
pub mod model;
pub mod sidebar;
pub mod theme;
pub mod ui;

use anyhow::Result;

/// Launch the TUI in the current terminal, pointed at the cwd's git worktree.
pub fn run() -> Result<()> {
    app::run_tui()
}
