//! `on_key`/`on_mouse` integration tests: real `KeyEvent`/`MouseEvent`s driven straight
//! through `App`, no real terminal (`ratatui::crossterm` is re-exported so tests use the
//! same event types the real event loop does).

use std::time::SystemTime;

use herdr_glowr::app::{App, Focus, Mode};
use herdr_glowr::comments::Store;
use herdr_glowr::config::CommentSync;
use herdr_glowr::file_list::FileEntry;
use herdr_glowr::{markdown, ui};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;

fn key(c: char) -> KeyEvent {
    KeyEvent::from(KeyCode::Char(c))
}

fn tab() -> KeyEvent {
    KeyEvent::from(KeyCode::Tab)
}

fn left_click(col: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: col,
        row,
        modifiers: KeyModifiers::empty(),
    }
}

/// Scan `area` for a `(col, row)` where `ui::hit_file` lands on file index `idx` — robust to
/// the exact pane geometry, unlike a hand-computed column.
fn find_file_click(area: Rect, list_pct: u16, n_files: usize, idx: usize) -> (u16, u16) {
    for row in 0..area.height {
        for col in 0..area.width {
            // `mouse_down` checks the divider before the file list, so a column inside its
            // grab zone would resize instead of selecting — skip those.
            if !ui::hit_divider(area, list_pct, col, row)
                && ui::hit_file(area, list_pct, col, row, n_files, 0) == Some(idx)
            {
                return (col, row);
            }
        }
    }
    panic!("no click position hits file row {idx}");
}

/// Scan `area` for a `(col, row)` where `ui::hit_doc` (via the render rows it maps onto)
/// lands on `pane`'s block `target_block` — the same two-step lookup `App::mouse_down` does.
fn find_doc_click(app: &App, area: Rect, pane: usize, target_block: usize) -> (u16, u16) {
    let heights = ui::doc_row_heights(app, area, pane);
    let width = ui::doc_inner_width(area, app.list_pct, app.split, pane);
    let rows = markdown::layout_rows(&app.docs[pane].doc, width, app.wrap);
    for row in 0..area.height {
        for col in 0..area.width {
            if let Some(row_ix) = ui::hit_doc(
                area,
                app.list_pct,
                app.split,
                pane,
                col,
                row,
                &heights,
                app.docs[pane].scroll,
            ) && rows.get(row_ix).is_some_and(|r| r.block == target_block)
            {
                return (col, row);
            }
        }
    }
    panic!("no click position hits doc pane {pane} block {target_block}");
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

#[test]
fn mouse_click_in_doc_pane_moves_block_cursor() {
    let mut app = App::for_test_with_path("# H\n\na\n\nb\n", Some("p.md"));
    assert_eq!(app.docs[0].cursor_block, 0);
    let (col, row) = find_doc_click(&app, AREA, 0, 2); // block 2 = "b"
    app.on_mouse(left_click(col, row), AREA);
    assert_eq!(app.focus, Focus::DocA);
    assert_eq!(app.docs[0].cursor_block, 2);
}

#[test]
fn mouse_click_on_file_row_loads_it() {
    let mut app = App::for_test_with_path("# H\n", Some("p.md"));
    app.files =
        vec![FileEntry { path: "other.md".into(), mtime: SystemTime::now(), ignored: false }];
    let (col, row) = find_file_click(AREA, app.list_pct, app.files.len(), 0);
    app.on_mouse(left_click(col, row), AREA);
    assert_eq!(app.focus, Focus::List);
    assert_eq!(app.file_cursor, 0);
    assert_eq!(app.docs[0].path.as_deref(), Some("other.md"));
}

#[test]
fn comment_sync_on_send_keeps_new_comments_memory_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::for_test_with_path("# H\n\npara\n", Some("a.md"));
    app.store = Some(Store::at(dir.path().join("comments")));
    app.set_comment_sync_for_test(CommentSync::OnSend);
    app.docs[0].cursor_block = 1;
    app.add_comment(0, "note".into());
    assert_eq!(app.comments.open_user_comments().len(), 1, "the comment is still in memory");
    assert!(!dir.path().join("comments").exists(), "on-send must not touch the store until export");
}

#[test]
fn comment_sync_immediate_persists_new_comments_right_away() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::for_test_with_path("# H\n\npara\n", Some("a.md"));
    app.store = Some(Store::at(dir.path().join("comments")));
    app.set_comment_sync_for_test(CommentSync::Immediate); // the default; set explicitly for clarity
    app.docs[0].cursor_block = 1;
    app.add_comment(0, "note".into());
    assert_eq!(app.comments.open_user_comments().len(), 1);
    let written = std::fs::read_dir(dir.path().join("comments")).unwrap().count();
    assert_eq!(written, 1, "immediate sync writes the comment file right away");
}
