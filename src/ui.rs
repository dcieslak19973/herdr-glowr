//! Frame rendering: header, body, and footer bands.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::Paragraph;

use crate::app::App;

/// Draw the three-band layout: a header, the (currently empty) document body, and a
/// footer key hint. `app` is unused until later tasks add document state.
pub fn render(frame: &mut Frame, _app: &App) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)])
        .split(frame.area());

    frame.render_widget(Paragraph::new("glowr"), rows[0]);
    frame.render_widget(Paragraph::new(""), rows[1]);
    frame.render_widget(Paragraph::new("q quit"), rows[2]);
}
