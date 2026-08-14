//! Render tests: drive `ui::render` through ratatui's `TestBackend` and assert on the
//! painted buffer, so the layout and component wiring are checked for real.

mod common;

use std::time::SystemTime;

use common::buffer_to_string;
use herdr_glowr::app::App;
use herdr_glowr::file_list::FileEntry;
use herdr_glowr::ui;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Color;

#[test]
fn renders_title_doc_text_and_file_list() {
    let app = App::for_test_with_path("# Plan\n\nstep one\n", Some("plan.md"));
    let backend = TestBackend::new(80, 20);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| ui::render(f, &app)).unwrap();
    let buf = term.backend().buffer().clone();
    let text = buffer_to_string(&buf);
    assert!(text.contains("glowr"));
    assert!(text.contains("Plan"));
    assert!(text.contains("step one"));
    assert!(text.contains("plan.md"));
}

#[test]
fn renders_comment_card_under_block() {
    let mut app = App::for_test_with_path("# Plan\n\nstep one\n", Some("plan.md"));
    app.docs[0].cursor_block = 1;
    app.add_comment(0, "clarify".into());
    let backend = TestBackend::new(80, 20);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| ui::render(f, &app)).unwrap();
    assert!(buffer_to_string(term.backend().buffer()).contains("clarify"));
}

#[test]
fn split_renders_two_docs() {
    let mut app = App::for_test_with_path("# AAA\n", Some("a.md"));
    app.load_into_test(1, "b.md", "# BBB\n"); // put a second doc in pane B
    app.split = true;
    let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
    term.draw(|f| ui::render(f, &app)).unwrap();
    let t = buffer_to_string(term.backend().buffer());
    assert!(t.contains("AAA") && t.contains("BBB"));
}

/// The style of the first cell of `needle`'s first occurrence in `buf`, scanning row by
/// row (ascii-only callers, so byte offset == column offset). `None` if `needle` never
/// appears — lets a style assertion also prove the text was actually painted.
fn cell_fg_at(buf: &Buffer, needle: &str) -> Option<Color> {
    let area = buf.area;
    for y in 0..area.height {
        let row: String =
            (0..area.width).map(|x| buf.cell((x, y)).map_or(" ", |c| c.symbol()).to_string()).collect();
        if let Some(byte_off) = row.find(needle) {
            let col = row[..byte_off].chars().count() as u16;
            return buf.cell((area.x + col, area.y + y)).map(|c| c.fg);
        }
    }
    None
}

#[test]
fn renders_file_list_rows_dim_ignored_basenames() {
    let mut app = App::for_test_with_path("# Plan\n\nstep one\n", Some("plan.md"));
    app.files = vec![
        FileEntry { path: "bright.md".into(), mtime: SystemTime::now(), ignored: false },
        FileEntry { path: "dim.md".into(), mtime: SystemTime::now(), ignored: true },
    ];
    let backend = TestBackend::new(80, 20);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| ui::render(f, &app)).unwrap();
    let buf = term.backend().buffer().clone();
    let text = buffer_to_string(&buf);
    // Both files actually painted as rows in the file-list pane (not a coincidental match
    // against the doc pane's border title, which is "plan.md" — neither test name appears
    // there).
    assert!(text.contains("bright.md"));
    assert!(text.contains("dim.md"));

    let bright_fg = cell_fg_at(&buf, "bright.md").expect("bright.md row painted");
    let dim_fg = cell_fg_at(&buf, "dim.md").expect("dim.md row painted");
    assert_eq!(bright_fg, app.palette.text, "a non-ignored basename paints in the body color");
    assert_eq!(dim_fg, app.palette.overlay0, "an ignored basename paints dimmed");
    assert_ne!(bright_fg, dim_fg);
}
