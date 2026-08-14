//! `on_key` integration tests: real `KeyEvent`s driven straight through `App::on_key`,
//! no real terminal (`ratatui::crossterm` is re-exported so tests use the same event
//! types the real event loop does).

use std::time::SystemTime;

use herdr_glowr::app::{App, Focus, Mode};
use herdr_glowr::file_list::FileEntry;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

fn key(c: char) -> KeyEvent {
    KeyEvent::from(KeyCode::Char(c))
}

fn tab() -> KeyEvent {
    KeyEvent::from(KeyCode::Tab)
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

#[test]
fn split_list_selection_loads_into_last_focused_doc_pane() {
    let mut app = App::for_test_with_path("# A\n", Some("a.md"));
    app.files = vec![
        FileEntry { path: "a.md".into(), mtime: SystemTime::now(), ignored: false },
        FileEntry { path: "b.md".into(), mtime: SystemTime::now(), ignored: false },
    ];
    app.split = true;
    // Tab List -> DocA -> DocB: cycle_focus remembers DocB as the file list's load target.
    app.on_key(tab(), AREA);
    app.on_key(tab(), AREA);
    assert_eq!(app.focus, Focus::DocB);
    // Tab back to the list; selecting a file now loads it into DocB, not DocA.
    app.on_key(tab(), AREA);
    assert_eq!(app.focus, Focus::List);
    app.on_key(key('j'), AREA); // file_cursor 0 -> 1 ("b.md")
    assert_eq!(app.docs[1].path.as_deref(), Some("b.md"));
    assert_eq!(app.docs[0].path.as_deref(), Some("a.md"));
}
