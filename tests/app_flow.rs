//! `on_key` integration tests: real `KeyEvent`s driven straight through `App::on_key`,
//! no real terminal (`ratatui::crossterm` is re-exported so tests use the same event
//! types the real event loop does).

use herdr_glowr::app::{App, Mode};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

fn key(c: char) -> KeyEvent {
    KeyEvent::from(KeyCode::Char(c))
}

const AREA: Rect = Rect { x: 0, y: 0, width: 80, height: 24 };

#[test]
fn v_then_j_then_c_selects_and_opens_composer() {
    let mut app = App::for_test_with_path("# H\n\na\n\nb\n", Some("p.md"));
    app.focus_doc_for_test(0);
    app.docs[0].cursor_block = 1;
    app.on_key(key('v'), AREA);
    app.on_key(key('j'), AREA); // extend to block 2
    app.on_key(key('c'), AREA);
    assert!(matches!(app.mode, Mode::Composing { .. }));
    assert_eq!(app.selection_range(0), (1, 2));
}

#[test]
fn backtick_toggles_split() {
    let mut app = App::for_test_with_path("# H\n", Some("p.md"));
    assert!(!app.split);
    app.on_key(key('`'), AREA);
    assert!(app.split);
    app.on_key(key('`'), AREA);
    assert!(!app.split);
}

#[test]
fn q_quits() {
    let mut app = App::for_test_with_path("# H\n", Some("p.md"));
    app.on_key(key('q'), AREA);
    assert!(app.should_quit);
}
