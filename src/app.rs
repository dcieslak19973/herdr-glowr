//! Application state and the TUI entry point.
//!
//! This module owns the terminal lifecycle and event loop for the `glowr` markdown
//! viewer. State is intentionally minimal until later tasks add the document model
//! and comment panes; `src/main.rs` calls [`run_tui`] to launch it.

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};

use crate::ui;

/// The full state of the viewer session.
#[derive(Debug, Default)]
pub struct App {
    /// Set once the user has asked to quit.
    pub should_quit: bool,
}

impl App {
    /// Handle one key press, mutating state. `q` requests a quit.
    fn on_key(&mut self, code: KeyCode) {
        if code == KeyCode::Char('q') {
            self.should_quit = true;
        }
    }
}

/// Entry point: set up the terminal, run the event loop, and restore it on exit (even
/// on an error, so a panic-free failure never leaves the terminal in raw mode).
pub fn run_tui() -> Result<()> {
    let mut terminal = ratatui::init();
    let mut app = App::default();
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    result
}

/// Draw-then-wait loop: paint the current state, then block for the next key press.
fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    while !app.should_quit {
        terminal.draw(|frame| ui::render(frame, app))?;
        if let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.on_key(key.code);
        }
    }
    Ok(())
}
