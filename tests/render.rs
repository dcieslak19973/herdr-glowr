//! Render tests: drive `ui::render` through ratatui's `TestBackend` and assert on the
//! painted buffer, so the layout and component wiring are checked for real.

mod common;

use common::buffer_to_string;
use herdr_glowr::app::App;
use herdr_glowr::ui;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

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
