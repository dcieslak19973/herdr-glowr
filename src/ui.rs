//! Rendering the doc view(s): header, body (doc pane(s) + file list), and footer.
//!
//! The layout is a one-row header, a body split into the rendered doc region (left) and the
//! markdown file list (right), and a one-row footer action bar. In split mode (`app.split`)
//! the doc region itself divides into two side-by-side panes, `docs[0]` and `docs[1]`, each
//! with its own focus highlight, cursor, scroll, and selection — [`doc_rect`] is the one
//! place that computes which half a pane paints into, so every geometry helper below and
//! [`render`] agree by construction (`G5`). While composing, the comment box is spliced
//! inline into the doc pane under the selected block; the comments-list overlay is drawn on
//! top when open. Rendering reads `App` only; all state changes live in `app.rs`.

use std::rc::Rc;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Focus, FooterAction, Mode, Tier};
use crate::comments::{Author, Status, StoredComment};
use crate::markdown;
use crate::theme::Palette;

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let p = panes(area, app.list_pct);

    render_header(frame, app, p.header);
    render_doc_view(frame, app, 0, app.focus == Focus::DocA, doc_rect(area, app.list_pct, app.split, 0));
    if app.split {
        render_doc_view(frame, app, 1, app.focus == Focus::DocB, doc_rect(area, app.list_pct, app.split, 1));
    }
    render_file_list(frame, app, p.files);
    render_footer(frame, app, p.footer);

    if let Mode::CommentsList { cursor } = &app.mode {
        render_comments_list(frame, app, area, *cursor);
    }
}

/// The vertical bands: header, body, footer. The comment input is inline in the doc pane,
/// not a band of its own. The footer action bar is one row — it fits by dropping the
/// least-relevant actions, not by wrapping.
fn vrows(area: Rect) -> Rc<[Rect]> {
    Layout::vertical([Constraint::Length(1), Constraint::Min(3), Constraint::Length(1)]).split(area)
}

/// The frame's layout rects: the doc pane, the file pane, and the whole body band. One
/// place computes the vertical bands and the horizontal split, so every geometry helper and
/// the renderer agree by construction (a layout change can't desync hit-testing from paint).
struct Panes {
    header: Rect,
    doc: Rect,
    files: Rect,
    body: Rect,
    footer: Rect,
}

fn panes(area: Rect, list_pct: u16) -> Panes {
    let rows = vrows(area);
    let body = rows[1];
    let split = Layout::horizontal([
        Constraint::Percentage(100 - list_pct),
        Constraint::Percentage(list_pct),
    ])
    .split(body);
    Panes { header: rows[0], doc: split[0], files: split[1], body, footer: rows[2] }
}

/// The `pane`'s doc rect: the whole doc region when not split, else its left (`pane == 0`)
/// or right (`pane == 1`) half. One place computes the split, so [`render`]'s paint and
/// every geometry helper below agree on where each pane's content lives (`G5`).
fn doc_rect(area: Rect, list_pct: u16, split: bool, pane: usize) -> Rect {
    let doc_area = panes(area, list_pct).doc;
    if !split {
        return doc_area;
    }
    let halves =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(doc_area);
    halves[pane.min(1)]
}

/// The whole body band (between the header and footer), for divider hit-testing.
pub fn body_rect(area: Rect) -> Rect {
    vrows(area)[1]
}

/// Whether `(col, row)` lands on the draggable divider between the two panes.
pub fn hit_divider(area: Rect, list_pct: u16, col: u16, row: u16) -> bool {
    let p = panes(area, list_pct);
    let in_body = row >= p.body.y && row < p.body.y + p.body.height;
    // A 3-column grab zone straddling the abutting pane borders.
    in_body && col + 1 >= p.files.x && col <= p.files.x + 1
}

/// The file-row index a click at `(col, row)` lands on, or `None` if outside the list.
/// `file_scroll` is the top visible row, so a click maps to the scrolled-to row.
pub fn hit_file(
    area: Rect,
    list_pct: u16,
    col: u16,
    row: u16,
    n_files: usize,
    file_scroll: usize,
) -> Option<usize> {
    let inner = inner_rect(panes(area, list_pct).files);
    if !contains(inner, col, row) {
        return None;
    }
    let idx = (row - inner.y) as usize + file_scroll;
    (idx < n_files).then_some(idx)
}

/// The number of file rows visible in the file pane, used to clamp the file-list scroll.
pub fn file_viewport_height(area: Rect, list_pct: u16) -> usize {
    inner_rect(panes(area, list_pct).files).height as usize
}

/// Whether `(col, row)` falls in the file pane, so the wheel scrolls the list it is over.
pub fn in_files_pane(area: Rect, list_pct: u16, col: u16, row: u16) -> bool {
    contains(panes(area, list_pct).files, col, row)
}

/// The render-row index (into `markdown::layout_rows(&app.docs[0].doc, ..)`) a click at
/// `(col, row)` lands on, or `None` if outside the doc pane. `heights` (display rows per
/// render row, including any spliced comment cards — see [`doc_row_heights`]) and
/// `doc_scroll` reproduce the painted window, so a click on a card or a wrapped row maps to
/// the right render row.
// Eight independent geometry inputs a click hit-test needs — grouping them into a struct
// would just move the same fields one level out, not reduce the coupling.
#[allow(clippy::too_many_arguments)]
pub fn hit_doc(
    area: Rect,
    list_pct: u16,
    split: bool,
    pane: usize,
    col: u16,
    row: u16,
    heights: &[usize],
    doc_scroll: usize,
) -> Option<usize> {
    let inner = inner_rect(doc_rect(area, list_pct, split, pane));
    if !contains(inner, col, row) {
        return None;
    }
    let target = (row - inner.y) as usize;
    let mut acc = 0;
    for (ri, h) in heights.iter().enumerate().skip(doc_scroll) {
        acc += h;
        if target < acc {
            return Some(ri);
        }
    }
    None
}

/// The number of doc rows visible in `pane`'s doc, used to clamp its scroll.
pub fn doc_viewport_height(area: Rect, list_pct: u16, split: bool, pane: usize) -> usize {
    inner_rect(doc_rect(area, list_pct, split, pane)).height as usize
}

/// The display height of each row `markdown::layout_rows` produces for `app.docs[pane]`: 1,
/// plus any spliced comment-card lines when the row is the last one of a block that has a
/// comment. Shares `layout_rows` and `comment_cards` with [`render_doc_view`], so what this
/// measures is exactly what gets painted — scroll-clamping and hit-testing can't desync from
/// the card splice, **in `Mode::Browse`**.
///
/// This does *not* account for an open inline composer: while `render_doc_view` is composing
/// on this pane, it replaces the scrolled window with a fixed above/box/below split anchored
/// to the selected block (see its "Composing:" branch), which these row heights don't model.
/// Callers (click hit-testing) must not route through `hit_doc`/`doc_row_heights` while
/// `app.mode` is `Mode::Composing` for the doc's pane — the composer has no click target of
/// its own yet.
pub fn doc_row_heights(app: &App, area: Rect, pane: usize) -> Vec<usize> {
    let width = inner_rect(doc_rect(area, app.list_pct, app.split, pane)).width as usize;
    let p = &app.palette;
    let doc_pane = &app.docs[pane];
    let rows = markdown::layout_rows(&doc_pane.doc, width, app.wrap);
    let cards = app.comment_cards(pane);
    rows.iter()
        .enumerate()
        .map(|(i, row)| {
            let is_last = i + 1 == rows.len() || rows[i + 1].block != row.block;
            let card_h: usize = if is_last {
                cards[row.block]
                    .iter()
                    .filter_map(|&ci| app.comments.get(ci))
                    .map(|sc| comment_card_lines(sc, width, p).len())
                    .sum()
            } else {
                0
            };
            1 + card_h
        })
        .collect()
}

/// Rows the inline comment box occupies at the doc pane's `width`: the wrapped body height
/// (so the box grows as text wraps, not only on explicit newlines) plus the two borders.
pub fn composer_height(app: &App, width: usize) -> usize {
    box_rows(&app.input, composer_content_width(width)).len() + 2
}

/// The text width inside the comment box: the doc pane width minus its two borders.
pub fn composer_content_width(width: usize) -> usize {
    width.saturating_sub(2).max(1)
}

/// The `pane`'s doc inner content width for the full terminal `area`, so `on_key` can
/// reserve the comment box without a `Frame` (mirrors [`doc_viewport_height`]).
pub fn doc_inner_width(area: Rect, list_pct: u16, split: bool, pane: usize) -> usize {
    inner_rect(doc_rect(area, list_pct, split, pane)).width as usize
}

/// The header band: `glowr`, then a right-aligned `[ Send (N) ]` button naming exactly the
/// open, user-authored comment count a send would deliver (`CommentStore::sendable`).
fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    let p = &app.palette;
    let width = area.width as usize;
    let title = " glowr";
    let button = format!("[ Send ({}) ]", app.comments.sendable());
    let pad = width.saturating_sub(title.width() + button.width() + 1);
    let spans = vec![
        Span::styled(title, Style::default().fg(p.text).add_modifier(Modifier::BOLD)),
        Span::raw(" ".repeat(pad)),
        Span::styled(button, Style::default().fg(p.peach).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
    ];
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(p.surface0)),
        area,
    );
}

/// The doc pane: `app.docs[pane]` rendered via `markdown::layout_rows`, with comment cards
/// spliced under their block's last visible row, the cursor/selection fill applied to every
/// row in the selected block range, windowed by `doc.scroll` — or, while composing on this
/// pane, the input box spliced under the selection instead of the window tail.
fn render_doc_view(frame: &mut Frame, app: &App, pane: usize, focused: bool, area: Rect) {
    let p = &app.palette;
    let doc_pane = &app.docs[pane];
    let title = doc_pane.path.as_deref().unwrap_or("(no file)");
    let block = bordered(title, focused, p);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if doc_pane.doc.blocks.is_empty() {
        frame.render_widget(dim_paragraph("select a file to read", p), inner);
        return;
    }

    let height = inner.height as usize;
    if height == 0 {
        return;
    }
    let width = inner.width as usize;
    let rows = markdown::layout_rows(&doc_pane.doc, width, app.wrap);
    if rows.is_empty() {
        return;
    }
    let (lo, hi) = app.selection_range(pane);
    let cards = app.comment_cards(pane);
    let fill = p.cursor_bg(focused);

    // One render row → its line (filled when its block is the cursor/selection), then any
    // saved-comment cards anchored to it (only on the block's last row).
    let row_lines = |i: usize| -> Vec<Line<'static>> {
        let row = &rows[i];
        let selected = row.block >= lo && row.block <= hi;
        let mut line = row.line.clone();
        if let Some(pad) = width.checked_sub(line.width()).filter(|w| *w > 0) {
            line.push_span(Span::raw(" ".repeat(pad)));
        }
        let line = if selected { line.style(Style::default().bg(fill)) } else { line };
        let mut out = vec![line];
        let is_last = i + 1 == rows.len() || rows[i + 1].block != row.block;
        if is_last {
            for &ci in &cards[row.block] {
                if let Some(sc) = app.comments.get(ci) {
                    out.extend(comment_card_lines(sc, width, p));
                }
            }
        }
        out
    };
    let display =
        |range: std::ops::Range<usize>| -> Vec<Line<'static>> { range.flat_map(&row_lines).collect() };

    let total = rows.len();
    let composing_here = matches!(app.mode, Mode::Composing { .. }) && app.focus_pane() == pane;
    if !composing_here {
        let mut out = display(doc_pane.scroll..total);
        out.truncate(height);
        frame.render_widget(Paragraph::new(out), inner);
        return;
    }

    // Composing: splice the input box under the last selected block's row, in display rows.
    // Cap the box at height-1 so a comment taller than the viewport can't hide its anchor.
    let box_h = composer_height(app, width).min(height.saturating_sub(1)).max(1);
    let doc_budget = height - box_h;
    let hi_block = hi.min(doc_pane.doc.blocks.len().saturating_sub(1));
    let anchor_row = rows.iter().rposition(|r| r.block == hi_block).unwrap_or(total.saturating_sub(1));
    let above_all = display(doc_pane.scroll..anchor_row + 1);
    let above: Vec<Line> = if above_all.len() > doc_budget {
        above_all[above_all.len() - doc_budget..].to_vec()
    } else {
        above_all
    };
    let remaining = doc_budget - above.len();
    let mut below = display(anchor_row + 1..total);
    below.truncate(remaining);

    let slots = Layout::vertical([
        Constraint::Length(above.len() as u16),
        Constraint::Length(box_h as u16),
        Constraint::Length(below.len() as u16),
    ])
    .split(inner);
    if !above.is_empty() {
        frame.render_widget(Paragraph::new(above), slots[0]);
    }
    render_composer(frame, app, slots[1]);
    if !below.is_empty() {
        frame.render_widget(Paragraph::new(below), slots[2]);
    }
}

/// The file-list pane: `app.files`, one row per markdown file — basename bright, parent
/// directories dimmed, a git-ignored file's basename dimmed too. No change markers or +/-
/// stats (unlike herdr-reviewr's diff-aware file list — glowr's list has neither).
fn render_file_list(frame: &mut Frame, app: &App, area: Rect) {
    let p = &app.palette;
    let block = bordered("Files", app.focus == Focus::List, p);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.files.is_empty() {
        frame.render_widget(dim_paragraph("no markdown files", p), inner);
        return;
    }

    let width = inner.width as usize;
    let items: Vec<ListItem> = app
        .files
        .iter()
        .enumerate()
        .skip(app.file_scroll)
        .take(inner.height as usize)
        .map(|(i, entry)| {
            let fill = (i == app.file_cursor).then(|| p.cursor_bg(app.focus == Focus::List));
            file_row_item(&entry.path, width, fill, entry.ignored, p)
        })
        .collect();
    frame.render_widget(List::new(items), inner);
}

/// A file row: the basename bright, its parent directories dimmed. A path too wide for the
/// row keeps its tail behind a leading `…/`.
fn file_row_item(path: &str, width: usize, fill: Option<Color>, ignored: bool, p: &Palette) -> ListItem<'static> {
    let shown = elide_head(path, width.max(1));
    let (dim, base) = match shown.rfind('/') {
        Some(s) => (&shown[..=s], &shown[s + 1..]),
        None => ("", shown.as_str()),
    };
    let mut spans = Vec::new();
    if !dim.is_empty() {
        spans.push(Span::styled(dim.to_string(), Style::default().fg(p.overlay0)));
    }
    // A git-ignored file recedes into a dim basename.
    let base_style = if ignored { Style::default().fg(p.overlay0) } else { text_style(p) };
    spans.push(Span::styled(base.to_string(), base_style));
    selectable_row(spans, width, fill)
}

/// Shorten `name` to `max` columns by eliding its head behind a leading `…`, preferring to
/// cut at a path separator so a partial directory name never shows.
fn elide_head(name: &str, max: usize) -> String {
    if name.width() <= max {
        return name.to_string();
    }
    let budget = max.saturating_sub(1); // a column for the `…`
    let mut tail = String::new();
    let mut w = 0;
    for ch in name.chars().rev() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > budget {
            break;
        }
        tail.insert(0, ch);
        w += cw;
    }
    if let Some(slash) = tail.find('/') {
        tail = tail[slash..].to_string();
    }
    format!("…{tail}")
}

/// A saved comment as inline display lines: a quiet box titled with the comment's location
/// holding its wrapped text. Spliced read-only under the commented block so a submitted
/// comment stays visible while reading. An agent's comment carries an ` agent ` chip (the
/// `mauve` accent) ahead of the title; a resolved comment renders its whole card in the
/// muted `overlay1` tone with a `resolved` marker in the title.
fn comment_card_lines(sc: &StoredComment, width: usize, p: &Palette) -> Vec<Line<'static>> {
    const INDENT: usize = 2;
    let c = &sc.comment;
    let resolved = sc.status == Status::Resolved;
    let box_w = width.saturating_sub(INDENT).max(10);
    let text_w = box_w.saturating_sub(4).max(1); // inside "│ " … " │"
    let border = Style::default().fg(if resolved { p.overlay1 } else { p.overlay0 });
    let title_fg = if resolved { p.overlay1 } else { p.peach };
    let title = Style::default().fg(title_fg).add_modifier(Modifier::BOLD);
    let body_style = Style::default().fg(if resolved { p.overlay1 } else { p.text });
    let pad = || Span::raw(" ".repeat(INDENT));

    let label_text = if resolved {
        format!(" comment · {} · resolved ", c.location())
    } else {
        format!(" comment · {} ", c.location())
    };
    let chip = " agent ";
    let is_agent = sc.author == Author::Agent;
    let chip_run = if is_agent { chip.width() + 1 } else { 0 }; // the chip plus its trailing dash
    let label = truncate_width(&label_text, box_w.saturating_sub(3 + chip_run));
    let fill = box_w.saturating_sub(3 + chip_run + label.width());
    let mut top = vec![pad(), Span::styled("╭─", border)];
    if is_agent {
        top.push(Span::styled(chip, Style::default().fg(p.mauve).add_modifier(Modifier::BOLD)));
        top.push(Span::styled("─", border));
    }
    top.push(Span::styled(label, title));
    top.push(Span::styled(format!("{}╮", "─".repeat(fill)), border));
    let mut lines = vec![Line::from(top)];

    for logical in c.text.split('\n') {
        for piece in wrap_text(logical, text_w) {
            let gap = " ".repeat(text_w.saturating_sub(piece.width()));
            lines.push(Line::from(vec![
                pad(),
                Span::styled("│ ", border),
                Span::styled(piece, body_style),
                Span::styled(format!("{gap} │"), border),
            ]));
        }
    }

    lines.push(Line::from(vec![
        pad(),
        Span::styled(format!("╰{}╯", "─".repeat(box_w.saturating_sub(2))), border),
    ]));
    lines
}

/// Truncate `s` to `max` display columns, marking a cut with a trailing `…`.
fn truncate_width(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > max.saturating_sub(1) {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

/// Word-wrap a plain string to `width` columns, reusing `markdown::wrap_line` (via a
/// one-span `Line`) so the break rule (last space, hard-break an over-wide word,
/// width-aware) matches the doc body exactly.
fn wrap_text(s: &str, width: usize) -> Vec<String> {
    markdown::wrap_line(&Line::from(s.to_string()), width)
        .into_iter()
        .map(|l| l.spans.iter().map(|sp| sp.content.as_ref()).collect::<String>())
        .collect()
}

/// The inline comment input box, drawn at `area` (under the selection in the doc pane).
fn render_composer(frame: &mut Frame, app: &App, area: Rect) {
    let p = &app.palette;
    let loc = app.pending_location().unwrap_or_else(|| "comment".to_string());
    let editing = matches!(app.mode, Mode::Composing { editing: Some(_) });
    let title = if editing { format!("edit · {loc}") } else { format!("comment · {loc}") };
    let block =
        Block::default().borders(Borders::ALL).border_style(Style::default().fg(p.peach)).title(title);
    let content_w = composer_content_width(area.width as usize);
    let body = Paragraph::new(composer_lines(app, content_w)).block(block);
    frame.render_widget(body, area);
}

/// The comment box's display lines at `content_w`: each input line word-wrapped, with the
/// caret drawn as a block at its mapped (row, column). An empty box shows a placeholder.
fn composer_lines(app: &App, content_w: usize) -> Vec<Line<'static>> {
    let p = &app.palette;
    if app.input.is_empty() {
        return vec![Line::from(vec![
            Span::styled(" ", caret_style(p)),
            Span::styled("Leave a comment…", Style::default().fg(p.overlay0)),
        ])];
    }
    let rows = box_rows(&app.input, content_w);
    let (caret_row, caret_col) = caret_rowcol(&rows, app.caret);
    rows.iter()
        .enumerate()
        .map(|(i, (_, text))| {
            if i == caret_row { row_with_caret(text, caret_col, p) } else { Line::from(text.clone()) }
        })
        .collect()
}

/// The block-cursor style: the character under the caret shown dark-on-peach.
fn caret_style(p: &Palette) -> Style {
    Style::default().fg(p.surface0).bg(p.peach)
}

/// One box row with the caret block over the character at `col` (a trailing block at the end).
fn row_with_caret(text: &str, col: usize, p: &Palette) -> Line<'static> {
    let chars: Vec<char> = text.chars().collect();
    let col = col.min(chars.len());
    let left: String = chars[..col].iter().collect();
    let mut spans = vec![Span::raw(left)];
    if col < chars.len() {
        spans.push(Span::styled(chars[col].to_string(), caret_style(p)));
        spans.push(Span::raw(chars[col + 1..].iter().collect::<String>()));
    } else {
        spans.push(Span::styled(" ".to_string(), caret_style(p)));
    }
    Line::from(spans)
}

/// Wrap one logical line's `chars` to `width` display columns, returning contiguous half-open
/// char ranges (every char is in exactly one row, so a char index maps cleanly to a row). A
/// greedy word wrap that keeps the break space on its row; an over-wide word hard-breaks.
fn box_wrap(chars: &[char], width: usize) -> Vec<(usize, usize)> {
    if chars.is_empty() {
        return vec![(0, 0)];
    }
    let w = width.max(1);
    let mut rows = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let (mut col, mut i, mut last_space) = (0usize, start, None);
        while i < chars.len() {
            let cw = UnicodeWidthChar::width(chars[i]).unwrap_or(0);
            if col + cw > w && i > start {
                break;
            }
            col += cw;
            if chars[i] == ' ' {
                last_space = Some(i);
            }
            i += 1;
        }
        // Break after the last space that fits (keeping it on this row), else hard-break.
        let end = if i < chars.len() {
            last_space.filter(|&s| s + 1 > start).map_or(i, |s| s + 1)
        } else {
            i
        };
        rows.push((start, end));
        start = end;
    }
    rows
}

/// The box's visual rows over the whole `input`: `(start_char_index, text)` per row, wrapping
/// each logical line (split on `\n`) with [`box_wrap`]. A trailing newline yields an empty row.
fn box_rows(input: &str, width: usize) -> Vec<(usize, String)> {
    let chars: Vec<char> = input.chars().collect();
    let mut rows = Vec::new();
    let mut i = 0;
    loop {
        let line_end = chars[i..].iter().position(|&c| c == '\n').map_or(chars.len(), |p| i + p);
        for (a, b) in box_wrap(&chars[i..line_end], width) {
            rows.push((i + a, chars[i + a..i + b].iter().collect::<String>()));
        }
        match chars[line_end..].first() {
            Some('\n') => {
                i = line_end + 1;
                if i == chars.len() {
                    rows.push((i, String::new())); // a trailing newline opens an empty row
                    break;
                }
            }
            _ => break,
        }
    }
    if rows.is_empty() {
        rows.push((0, String::new()));
    }
    rows
}

/// Map a caret char index to its `(row, col)` in the box rows: the last row that starts at or
/// before the caret, with the column clamped to that row's length.
fn caret_rowcol(rows: &[(usize, String)], caret: usize) -> (usize, usize) {
    let row = rows.iter().rposition(|(start, _)| *start <= caret).unwrap_or(0);
    let (start, text) = &rows[row];
    (row, (caret - start).min(text.chars().count()))
}

/// The new caret char index after moving up (`down == false`) or down one wrapped row within
/// the comment box, keeping the column where the target row allows. For `on_key`'s composer
/// `↑`/`↓` handling.
pub fn caret_vertical(input: &str, caret: usize, content_w: usize, down: bool) -> usize {
    let rows = box_rows(input, content_w);
    let (row, col) = caret_rowcol(&rows, caret);
    let target = if down { (row + 1).min(rows.len() - 1) } else { row.saturating_sub(1) };
    let (start, text) = &rows[target];
    start + col.min(text.chars().count())
}

/// The key glyph and label for a footer action; an empty label renders the glyph alone. The
/// `CycleFocus` and `Send` labels depend on `app` (the destination pane, the comment count).
fn action_key_label(app: &App, action: FooterAction) -> (String, String) {
    use FooterAction as A;
    let (k, l): (&str, &str) = match action {
        A::Comment => ("c", "comment"),
        A::Select => ("v", "select"),
        A::ClearSelection => ("esc", "clear"),
        A::EditComment => ("e", "edit"),
        A::DeleteComment => ("d", "delete"),
        A::JumpComment => ("n/N", "jump"),
        A::CycleFocus => {
            return ("⇥".into(), if app.focus == Focus::List { "doc" } else { "files" }.into());
        }
        A::Send => return ("s".into(), format!("send {}", app.comments.sendable())),
        A::List => ("l", "list"),
        A::Copy => ("y", "copy"),
        A::Split => ("`", "split"),
        A::Save => ("enter", "save"),
        A::Newline => ("⇧⏎", "newline"),
        A::Cancel => ("esc", "cancel"),
        A::CloseList => ("esc", "close"),
        A::ResolveComment => ("x", "resolve"),
        A::Quit => ("q", ""),
    };
    (k.into(), l.into())
}

/// A tier's `(key, label)` styles: the primary bright and bold, normal actions readable, the
/// orientation cluster dim so the eye lands on what to do, not on the always-there anchors.
fn tier_styles(tier: Tier, p: &Palette) -> (Style, Style) {
    match tier {
        Tier::Primary => (Style::default().fg(p.peach).add_modifier(Modifier::BOLD), text_style(p)),
        Tier::Normal => (Style::default().fg(p.lavender), Style::default().fg(p.subtext0)),
        Tier::Orientation => (Style::default().fg(p.overlay0), Style::default().fg(p.overlay0)),
    }
}

/// Render a run of actions as ` · `-separated `key label` spans, styled per tier.
fn action_spans(app: &App, acts: &[(FooterAction, Tier)]) -> Vec<Span<'static>> {
    let p = &app.palette;
    let mut spans = Vec::new();
    for (i, &(action, tier)) in acts.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(p.overlay0)));
        }
        let (key, label) = action_key_label(app, action);
        let (key_style, label_style) = tier_styles(tier, p);
        spans.push(Span::styled(key, key_style));
        if !label.is_empty() {
            spans.push(Span::styled(format!(" {label}"), label_style));
        }
    }
    spans
}

/// The footer action bar: the context's actions (primary highlighted) packed left, the dim
/// orientation cluster packed right, fitting one line — orientation dropped first, then
/// trailing `Normal` actions, with a trailing `…` marking anything clipped.
fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let p = &app.palette;
    let w = area.width as usize;
    let all = app.footer_actions();
    let (mut left_acts, orient_acts): (Vec<_>, Vec<_>) =
        all.into_iter().partition(|&(_, t)| t != Tier::Orientation);

    let build_left = |acts: &[(FooterAction, Tier)]| -> Vec<Span<'static>> {
        let mut spans = vec![Span::raw(" ")];
        spans.extend(action_spans(app, acts));
        spans
    };
    let orient: Vec<Span> = if orient_acts.is_empty() {
        Vec::new()
    } else {
        let mut spans = vec![Span::styled("│ ", Style::default().fg(p.overlay0))];
        spans.extend(action_spans(app, &orient_acts));
        spans
    };
    let orient_w: usize = orient.iter().map(Span::width).sum();

    let mut left = build_left(&left_acts);
    let line_width = |s: &[Span]| -> usize { s.iter().map(Span::width).sum() };
    let fits_with_orient = !orient.is_empty() && line_width(&left) + 1 + orient_w <= w;

    let spans = if fits_with_orient {
        // Leave one trailing cell so the last hint (`q`) doesn't butt against the edge.
        let pad = w.saturating_sub(line_width(&left) + orient_w + 1);
        left.push(Span::raw(" ".repeat(pad)));
        left.extend(orient);
        left
    } else {
        // Orientation is dropped; trim trailing `Normal` actions until the line fits, leaving
        // room for the `…` that marks the drop. The primary action is never trimmed.
        let dropped_orient = !orient.is_empty();
        let mut popped = false;
        while line_width(&left) + 2 > w
            && left_acts.len() > 1
            && left_acts.last().is_some_and(|&(_, t)| t == Tier::Normal)
        {
            left_acts.pop();
            popped = true;
            left = build_left(&left_acts);
        }
        if dropped_orient || popped || line_width(&left) + 2 > w {
            left.push(Span::styled(" …", Style::default().fg(p.overlay0)));
        }
        left
    };

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(p.surface0)),
        area,
    );
}

/// The comments-list overlay: every comment, newest last, with `cursor` (from
/// `Mode::CommentsList`) filled as the active row.
fn render_comments_list(frame: &mut Frame, app: &App, area: Rect, cursor: usize) {
    let p = &app.palette;
    let popup = centered(area, 80, 60);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.mauve))
        .title(format!("Comments ({})", app.comments.len()));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let width = inner.width as usize;
    let items: Vec<ListItem> = app
        .comments
        .iter()
        .enumerate()
        .map(|(i, sc)| {
            // Author column: `@agent` tinted mauve (mirroring the card's chip), `@you`
            // otherwise.
            let (author_text, author_fg) = match sc.author {
                Author::Agent => ("@agent ", p.mauve),
                Author::User => ("@you ", p.subtext0),
            };
            let author = Span::styled(author_text, Style::default().fg(author_fg));
            let loc = Span::styled(
                sc.comment.location(),
                Style::default().fg(p.mauve).add_modifier(Modifier::BOLD),
            );
            let mut spans =
                vec![author, loc, Span::styled(format!("  {}", sc.comment.text), text_style(p))];
            if sc.status == Status::Resolved {
                spans.push(Span::styled("  resolved", Style::default().fg(p.overlay1)));
            }
            // The list overlay is the active modal, so its row reads at full brightness.
            selectable_row(spans, width, (i == cursor).then_some(p.surface2))
        })
        .collect();
    frame.render_widget(List::new(items), inner);
}

/// The default body text color.
fn text_style(p: &Palette) -> Style {
    Style::default().fg(p.text)
}

/// A list row, highlighted with the shared selection fill (full width) when `selected` —
/// the same treatment the doc cursor uses, so every cursor in the UI reads the same. The
/// fill is applied per span (with a trailing pad) so it spans the full width under the
/// `List` widget, matching the doc pane's `Paragraph` rows.
fn selectable_row(mut spans: Vec<Span<'static>>, width: usize, fill: Option<Color>) -> ListItem<'static> {
    if let Some(bg) = fill {
        let used: usize = spans.iter().map(Span::width).sum();
        if width > used {
            spans.push(Span::raw(" ".repeat(width - used)));
        }
        for s in &mut spans {
            s.style = s.style.bg(bg).add_modifier(Modifier::BOLD);
        }
    }
    ListItem::new(Line::from(spans))
}

/// A pane's bordered frame: a focused pane gets a lavender border, an unfocused one recedes
/// to a surface tone.
fn bordered(title: &str, focused: bool, p: &Palette) -> Block<'static> {
    let color = if focused { p.lavender } else { p.surface2 };
    Block::default().borders(Borders::ALL).border_style(Style::default().fg(color)).title(title.to_string())
}

fn dim_paragraph<'a>(text: &'a str, p: &Palette) -> Paragraph<'a> {
    Paragraph::new(text).style(Style::default().fg(p.overlay0))
}

/// Whether `(col, row)` falls inside `rect`.
fn contains(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.x
        && col < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}

/// The content area inside a one-cell border.
fn inner_rect(outer: Rect) -> Rect {
    Rect {
        x: outer.x.saturating_add(1),
        y: outer.y.saturating_add(1),
        width: outer.width.saturating_sub(2),
        height: outer.height.saturating_sub(2),
    }
}

/// A `Rect` centered in `area` at `pct_x` × `pct_y` percent of its size.
fn centered(area: Rect, pct_x: u16, pct_y: u16) -> Rect {
    let v = Layout::vertical([
        Constraint::Percentage((100 - pct_y) / 2),
        Constraint::Percentage(pct_y),
        Constraint::Percentage((100 - pct_y) / 2),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - pct_x) / 2),
        Constraint::Percentage(pct_x),
        Constraint::Percentage((100 - pct_x) / 2),
    ])
    .split(v[1])[1]
}
