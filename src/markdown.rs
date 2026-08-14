//! Markdown parse + render + source mapping.
use crate::highlight::Highlighter;
use crate::theme::Palette;
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::ops::Range;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The smallest independently-commentable unit of a rendered document.
#[derive(Clone, Debug)]
pub struct Block {
    /// 1-based inclusive source line range in the file.
    pub source_start: u32,
    pub source_end: u32,
    /// Styled terminal lines for this block (filled by rendering, empty until then).
    pub rendered: Vec<Line<'static>>,
    /// Structural kind of this block, as produced by the markdown parser.
    pub(crate) kind: BlockKind,
    /// Flattened inline content for text-bearing blocks.
    pub(crate) inlines: Vec<Inline>,
}

/// One painted row of a rendered document.
#[derive(Clone, Debug)]
pub struct RenderRow {
    pub line: Line<'static>,
    /// Index into the document's block list.
    pub block: usize,
    /// True for the first row of its block.
    pub first_of_block: bool,
}

/// A fully rendered document: its blocks and the flattened rows.
#[derive(Clone, Debug, Default)]
pub struct Document {
    pub source: String,
    pub blocks: Vec<Block>,
}

/// A flattened inline-formatting node, built from pulldown-cmark's inline event stream.
#[derive(Clone, Debug, PartialEq)]
pub enum Inline {
    Text(String),
    Code(String),
    Emph(Vec<Inline>),
    Strong(Vec<Inline>),
    Strike(Vec<Inline>),
    Link { label: Vec<Inline>, url: String },
    Image { alt: String },
    SoftBreak,
    HardBreak,
}

/// The structural kind of a [`Block`], as distinguished by the parser.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum BlockKind {
    Heading(u8),
    Paragraph,
    Item {
        ordered: bool,
        marker: String,
        depth: u8,
        task: Option<bool>,
    },
    Quote,
    Code {
        lang: Option<String>,
        line: String,
    },
    TableRow {
        header: bool,
        cells: Vec<Vec<Inline>>,
    },
    Rule,
    Html(String),
}

/// Byte offset of the start of each 1-based source line; index 0 = line 1.
pub fn line_index(source: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

/// 1-based line number containing byte offset `byte`, given `index` from [`line_index`].
pub fn line_of(index: &[usize], byte: usize) -> u32 {
    // last line-start <= byte
    match index.binary_search(&byte) {
        Ok(i) => (i + 1) as u32,
        Err(i) => i as u32, // i is count of starts <= byte
    }
}

/// One frame of inline-building state, mirroring the nesting of inline containers
/// (emphasis, strong, links, ...) while folding pulldown-cmark's event stream into
/// an [`Inline`] tree.
enum InlineFrame {
    /// A normal accumulator: paragraph/heading/item/cell/emphasis/strong/link/... content.
    Nodes(Vec<Inline>),
    /// An image's alt text, flattened to plain text (images only carry a flat alt string).
    Alt(String),
}

/// Flattens an [`Inline`] node to its visible plain text (recursing through formatting
/// wrappers), for building an image's flat `alt` string from arbitrarily nested markup.
fn flatten_text(inline: &Inline, out: &mut String) {
    match inline {
        Inline::Text(t) | Inline::Code(t) => out.push_str(t),
        Inline::Emph(v) | Inline::Strong(v) | Inline::Strike(v) => {
            for n in v {
                flatten_text(n, out);
            }
        }
        Inline::Link { label, .. } => {
            for n in label {
                flatten_text(n, out);
            }
        }
        Inline::Image { alt } => out.push_str(alt),
        Inline::SoftBreak | Inline::HardBreak => out.push(' '),
    }
}

fn push_inline(stack: &mut [InlineFrame], inline: Inline) {
    match stack.last_mut() {
        Some(InlineFrame::Nodes(v)) => v.push(inline),
        Some(InlineFrame::Alt(s)) => flatten_text(&inline, s),
        None => {}
    }
}

fn pop_nodes(stack: &mut Vec<InlineFrame>) -> Vec<Inline> {
    match stack.pop() {
        Some(InlineFrame::Nodes(v)) => v,
        Some(InlineFrame::Alt(s)) => vec![Inline::Text(s)],
        None => Vec::new(),
    }
}

fn heading_level_u8(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// A list item's leading marker token (e.g. `-`, `*`, `1.`), read straight from the
/// source so numbering/bullet style always matches what the author wrote.
fn item_marker(source: &str, range: &Range<usize>) -> String {
    let text = &source[range.start..range.end.min(source.len())];
    text.trim_start()
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect()
}

/// Per-item state tracked between `Start(Item)` and `End(Item)`, supporting nested lists.
struct ItemCtx {
    ordered: bool,
    marker: String,
    depth: u8,
    task: Option<bool>,
}

/// Parse `source` into blocks with source-line spans, structural kind, and flattened
/// inline content. Rendering to terminal `Line`s happens later; `Block::rendered` is
/// always empty here.
pub fn parse_blocks(source: &str) -> Vec<Block> {
    let idx = line_index(source);
    let opts = Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH;

    let mut blocks: Vec<Block> = Vec::new();
    let mut inline_stack: Vec<InlineFrame> = Vec::new();
    let mut link_url_stack: Vec<String> = Vec::new();

    let mut quote_depth: u32 = 0;
    let mut list_stack: Vec<bool> = Vec::new(); // ordered flag per open list
    let mut item_depth: u32 = 0;
    let mut item_ctx_stack: Vec<ItemCtx> = Vec::new();

    let mut in_code_block = false;
    let mut code_lang: Option<String> = None;
    let mut code_text = String::new();
    let mut code_start_offset: Option<usize> = None;

    let mut in_html_block = false;
    let mut html_text = String::new();

    let mut table_block_indices: Vec<usize> = Vec::new();
    let mut current_row_cells: Vec<Vec<Inline>> = Vec::new();

    let span = |range: &Range<usize>| -> (u32, u32) {
        (
            line_of(&idx, range.start),
            line_of(&idx, range.end.saturating_sub(1)),
        )
    };

    for (event, range) in Parser::new_ext(source, opts).into_offset_iter() {
        match event {
            // Several tags below open an inline-accumulator frame with an identical body;
            // kept as separate arms (rather than one combined pattern) so the match reads by
            // markdown construct, not by incidental implementation overlap.
            #[allow(clippy::match_same_arms)]
            Event::Start(tag) => match tag {
                Tag::Heading { .. } | Tag::TableCell => {
                    inline_stack.push(InlineFrame::Nodes(Vec::new()));
                }
                Tag::Paragraph => {
                    if item_depth == 0 {
                        inline_stack.push(InlineFrame::Nodes(Vec::new()));
                    }
                }
                Tag::BlockQuote(_) => quote_depth += 1,
                Tag::CodeBlock(kind) => {
                    in_code_block = true;
                    code_lang = match kind {
                        CodeBlockKind::Fenced(lang) if !lang.is_empty() => Some(lang.to_string()),
                        _ => None,
                    };
                    code_text.clear();
                    code_start_offset = None;
                }
                Tag::HtmlBlock => {
                    in_html_block = true;
                    html_text.clear();
                }
                Tag::List(start) => list_stack.push(start.is_some()),
                Tag::Item => {
                    item_depth += 1;
                    item_ctx_stack.push(ItemCtx {
                        ordered: list_stack.last().copied().unwrap_or(false),
                        marker: item_marker(source, &range),
                        depth: list_stack.len() as u8,
                        task: None,
                    });
                    inline_stack.push(InlineFrame::Nodes(Vec::new()));
                }
                Tag::Table(_) => table_block_indices.clear(),
                Tag::TableHead | Tag::TableRow => current_row_cells = Vec::new(),
                Tag::Emphasis | Tag::Strong | Tag::Strikethrough => {
                    inline_stack.push(InlineFrame::Nodes(Vec::new()));
                }
                Tag::Link { dest_url, .. } => {
                    link_url_stack.push(dest_url.to_string());
                    inline_stack.push(InlineFrame::Nodes(Vec::new()));
                }
                Tag::Image { .. } => inline_stack.push(InlineFrame::Alt(String::new())),
                _ => {}
            },
            Event::End(tag_end) => match tag_end {
                TagEnd::Heading(level) => {
                    let inlines = pop_nodes(&mut inline_stack);
                    let (source_start, source_end) = span(&range);
                    blocks.push(Block {
                        source_start,
                        source_end,
                        rendered: Vec::new(),
                        kind: BlockKind::Heading(heading_level_u8(level)),
                        inlines,
                    });
                }
                TagEnd::Paragraph => {
                    if item_depth == 0 {
                        let inlines = pop_nodes(&mut inline_stack);
                        let kind = if quote_depth > 0 {
                            BlockKind::Quote
                        } else {
                            BlockKind::Paragraph
                        };
                        let (source_start, source_end) = span(&range);
                        blocks.push(Block {
                            source_start,
                            source_end,
                            rendered: Vec::new(),
                            kind,
                            inlines,
                        });
                    }
                    // Else: this paragraph belongs to a loose list item; its text already
                    // flowed straight into the enclosing item's inline frame.
                }
                TagEnd::BlockQuote(_) => quote_depth = quote_depth.saturating_sub(1),
                TagEnd::CodeBlock => {
                    in_code_block = false;
                    if let Some(start) = code_start_offset {
                        let base_line = line_of(&idx, start);
                        for (i, line) in code_text.lines().enumerate() {
                            let ln = base_line + i as u32;
                            blocks.push(Block {
                                source_start: ln,
                                source_end: ln,
                                rendered: Vec::new(),
                                kind: BlockKind::Code {
                                    lang: code_lang.clone(),
                                    line: line.to_string(),
                                },
                                inlines: Vec::new(),
                            });
                        }
                    }
                    code_lang = None;
                    code_text.clear();
                    code_start_offset = None;
                }
                TagEnd::HtmlBlock => {
                    in_html_block = false;
                    let (source_start, source_end) = span(&range);
                    blocks.push(Block {
                        source_start,
                        source_end,
                        rendered: Vec::new(),
                        kind: BlockKind::Html(std::mem::take(&mut html_text)),
                        inlines: Vec::new(),
                    });
                }
                TagEnd::List(_) => {
                    list_stack.pop();
                }
                TagEnd::Item => {
                    item_depth -= 1;
                    let inlines = pop_nodes(&mut inline_stack);
                    let ctx = item_ctx_stack
                        .pop()
                        .expect("End(Item) without matching Start(Item)");
                    let (source_start, source_end) = span(&range);
                    blocks.push(Block {
                        source_start,
                        source_end,
                        rendered: Vec::new(),
                        kind: BlockKind::Item {
                            ordered: ctx.ordered,
                            marker: ctx.marker,
                            depth: ctx.depth,
                            task: ctx.task,
                        },
                        inlines,
                    });
                }
                TagEnd::TableCell => {
                    let cell = pop_nodes(&mut inline_stack);
                    current_row_cells.push(cell);
                }
                TagEnd::TableHead | TagEnd::TableRow => {
                    let (source_start, source_end) = span(&range);
                    blocks.push(Block {
                        source_start,
                        source_end,
                        rendered: Vec::new(),
                        kind: BlockKind::TableRow {
                            header: tag_end == TagEnd::TableHead,
                            cells: std::mem::take(&mut current_row_cells),
                        },
                        inlines: Vec::new(),
                    });
                    table_block_indices.push(blocks.len() - 1);
                }
                TagEnd::Table => {
                    // The delimiter row (`|---|---|`) has no event of its own; fold the gap
                    // it leaves between consecutive row blocks into the preceding row.
                    for i in 0..table_block_indices.len().saturating_sub(1) {
                        let cur = table_block_indices[i];
                        let next = table_block_indices[i + 1];
                        let next_start = blocks[next].source_start;
                        if blocks[cur].source_end + 1 < next_start {
                            blocks[cur].source_end = next_start - 1;
                        }
                    }
                    table_block_indices.clear();
                }
                TagEnd::Emphasis => {
                    let v = pop_nodes(&mut inline_stack);
                    push_inline(&mut inline_stack, Inline::Emph(v));
                }
                TagEnd::Strong => {
                    let v = pop_nodes(&mut inline_stack);
                    push_inline(&mut inline_stack, Inline::Strong(v));
                }
                TagEnd::Strikethrough => {
                    let v = pop_nodes(&mut inline_stack);
                    push_inline(&mut inline_stack, Inline::Strike(v));
                }
                TagEnd::Link => {
                    let label = pop_nodes(&mut inline_stack);
                    let url = link_url_stack.pop().unwrap_or_default();
                    push_inline(&mut inline_stack, Inline::Link { label, url });
                }
                TagEnd::Image => {
                    // Tag::Image always pushes an `Alt` frame; nothing else can be on top.
                    let alt = match inline_stack.pop() {
                        Some(InlineFrame::Alt(s)) => s,
                        _ => String::new(),
                    };
                    push_inline(&mut inline_stack, Inline::Image { alt });
                }
                _ => {}
            },
            Event::Text(text) => {
                if in_code_block {
                    if code_start_offset.is_none() {
                        code_start_offset = Some(range.start);
                    }
                    code_text.push_str(&text);
                } else {
                    push_inline(&mut inline_stack, Inline::Text(text.to_string()));
                }
            }
            Event::Code(text) => push_inline(&mut inline_stack, Inline::Code(text.to_string())),
            Event::Html(text) => {
                if in_html_block {
                    html_text.push_str(&text);
                }
            }
            Event::InlineHtml(text) => {
                push_inline(&mut inline_stack, Inline::Text(text.to_string()));
            }
            Event::SoftBreak => push_inline(&mut inline_stack, Inline::SoftBreak),
            Event::HardBreak => push_inline(&mut inline_stack, Inline::HardBreak),
            Event::Rule => {
                let (source_start, source_end) = span(&range);
                blocks.push(Block {
                    source_start,
                    source_end,
                    rendered: Vec::new(),
                    kind: BlockKind::Rule,
                    inlines: Vec::new(),
                });
            }
            Event::TaskListMarker(checked) => {
                if let Some(ctx) = item_ctx_stack.last_mut() {
                    ctx.task = Some(checked);
                }
            }
            _ => {}
        }
    }

    blocks
}

/// A nominal column count used to size blocks that have no natural terminal-width input
/// at this layer (tables, thematic rules); the row layer wraps/truncates to the real
/// pane width later.
const NOMINAL_WIDTH: usize = 80;

/// The narrowest a table column is padded to, so a one-column table (or a very wide
/// table) still reads as columns rather than collapsing to zero-width cells.
const MIN_COL_WIDTH: usize = 4;

/// Parse `source` and render every block's `kind`/`inlines` into styled, owned
/// [`Line`]s against `palette`, highlighting fenced/indented code via `hl`.
pub fn render_document(source: &str, palette: &Palette, hl: &Highlighter) -> Document {
    let mut blocks = parse_blocks(source);
    for block in &mut blocks {
        block.rendered = render_block(&block.kind, &block.inlines, palette, hl);
    }
    Document { source: source.to_string(), blocks }
}

/// Render one block's `kind`/`inlines` to its terminal lines.
fn render_block(
    kind: &BlockKind,
    inlines: &[Inline],
    palette: &Palette,
    hl: &Highlighter,
) -> Vec<Line<'static>> {
    match kind {
        BlockKind::Heading(level) => vec![render_heading(*level, inlines, palette)],
        BlockKind::Paragraph => render_inlines(inlines, Style::default().fg(palette.text), palette),
        BlockKind::Item { ordered, marker, depth, task } => {
            render_item(*ordered, marker, *depth, *task, inlines, palette)
        }
        BlockKind::Quote => render_quote(inlines, palette),
        BlockKind::Code { lang, line } => vec![render_code_line(lang.as_deref(), line, palette, hl)],
        BlockKind::TableRow { header, cells } => vec![render_table_row(*header, cells, palette)],
        BlockKind::Rule => vec![render_rule(palette)],
        BlockKind::Html(raw) => render_html(raw, palette),
    }
}

/// A heading: bold, colored by level (h1 `mauve`, h2 `lavender` — this palette has no
/// dedicated `blue`, so `lavender` stands in for the spec's "blue" — else `text`),
/// prefixed with a dimmed `#`×level marker.
fn render_heading(level: u8, inlines: &[Inline], palette: &Palette) -> Line<'static> {
    let color = match level {
        1 => palette.mauve,
        2 => palette.lavender,
        _ => palette.text,
    };
    let style = Style::default().fg(color).add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::styled(
        format!("{} ", "#".repeat(level as usize)),
        Style::default().fg(palette.overlay0).add_modifier(Modifier::DIM),
    )];
    if let Some(first) = render_inlines(inlines, style, palette).into_iter().next() {
        spans.extend(first.spans);
    }
    Line::from(spans)
}

/// A list item: `depth*2`-space indent, then a marker (`•` bullet / `N.` ordered /
/// `[ ]`/`[x]` task), then the item's inline content. Continuation lines (from a
/// `HardBreak`) align under the marker rather than repeating it.
fn render_item(
    ordered: bool,
    marker: &str,
    depth: u8,
    task: Option<bool>,
    inlines: &[Inline],
    palette: &Palette,
) -> Vec<Line<'static>> {
    let indent = " ".repeat(depth as usize * 2);
    let marker_text = match task {
        Some(true) => "[x] ".to_string(),
        Some(false) => "[ ] ".to_string(),
        None if ordered => format!("{marker} "),
        None => "• ".to_string(),
    };
    let cont_indent = " ".repeat(indent.width() + marker_text.width());
    let marker_style = Style::default().fg(palette.text);
    let body = render_inlines(inlines, Style::default().fg(palette.text), palette);
    body.into_iter()
        .enumerate()
        .map(|(i, line)| {
            let mut spans = if i == 0 {
                vec![Span::raw(indent.clone()), Span::styled(marker_text.clone(), marker_style)]
            } else {
                vec![Span::raw(cont_indent.clone())]
            };
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}

/// A blockquote: each body line prefixed with a `▍` bar in `overlay1`, body text in
/// `subtext0` italic.
fn render_quote(inlines: &[Inline], palette: &Palette) -> Vec<Line<'static>> {
    let bar_style = Style::default().fg(palette.overlay1);
    let body_style = Style::default().fg(palette.subtext0).add_modifier(Modifier::ITALIC);
    render_inlines(inlines, body_style, palette)
        .into_iter()
        .map(|line| {
            let mut spans = vec![Span::styled("▍ ", bar_style)];
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}

/// One fenced/indented code line, highlighted for `lang` (falling back to plain
/// `palette.text` when the highlighter has no match) on a subtle `surface0` code
/// background.
fn render_code_line(lang: Option<&str>, text: &str, palette: &Palette, hl: &Highlighter) -> Line<'static> {
    let bg = palette.surface0;
    let highlighted = hl.highlight(text, lang);
    let spans: Vec<Span<'static>> = match highlighted.into_iter().next() {
        Some(line_spans) if !line_spans.is_empty() => line_spans
            .into_iter()
            .map(|s| {
                let (r, g, b) = s.color;
                Span::styled(s.text, Style::default().fg(Color::Rgb(r, g, b)).bg(bg))
            })
            .collect(),
        _ => vec![Span::styled(text.to_string(), Style::default().fg(palette.text).bg(bg))],
    };
    Line::from(spans)
}

/// One table row: `header` rows render bold; every cell is rendered (preserving its
/// inline styling) then padded to an even share of [`NOMINAL_WIDTH`], separated by a
/// dim `│`. Column alignment refinement is roadmap — v1 pads to even columns.
fn render_table_row(header: bool, cells: &[Vec<Inline>], palette: &Palette) -> Line<'static> {
    let base = Style::default().fg(palette.text);
    let style = if header { base.add_modifier(Modifier::BOLD) } else { base };
    let ncols = cells.len().max(1);
    let col_width = (NOMINAL_WIDTH / ncols).max(MIN_COL_WIDTH);
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, cell) in cells.iter().enumerate() {
        let cell_line = render_inlines(cell, style, palette).into_iter().next().unwrap_or_default();
        let content_width: usize = cell_line.spans.iter().map(|s| s.content.width()).sum();
        spans.extend(cell_line.spans);
        if content_width < col_width {
            spans.push(Span::styled(" ".repeat(col_width - content_width), style));
        }
        if i + 1 < cells.len() {
            spans.push(Span::styled(" │ ", Style::default().fg(palette.overlay0)));
        }
    }
    Line::from(spans)
}

/// A full-width thematic break, in `overlay0`.
fn render_rule(palette: &Palette) -> Line<'static> {
    Line::from(Span::styled("─".repeat(NOMINAL_WIDTH), Style::default().fg(palette.overlay0)))
}

/// A raw HTML block, verbatim, dimmed `overlay0` — one `Line` per source line.
fn render_html(raw: &str, palette: &Palette) -> Vec<Line<'static>> {
    let style = Style::default().fg(palette.overlay0).add_modifier(Modifier::DIM);
    let lines: Vec<Line<'static>> =
        raw.lines().map(|l| Line::from(Span::styled(l.to_string(), style))).collect();
    if lines.is_empty() { vec![Line::from(Span::styled(String::new(), style))] } else { lines }
}

/// Fold `inlines` into styled, owned [`Line`]s against `base`: `Text` keeps `base`;
/// `Code` gets `green`-on-`surface0`; `Emph`/`Strong`/`Strike` layer modifiers over
/// `base`; `Link` renders its label then ` (url)` in `lavender`/underline; `Image`
/// renders a dim `image:` chip then its alt text; `SoftBreak` is a space; `HardBreak`
/// starts a new `Line`.
fn render_inlines(inlines: &[Inline], base: Style, palette: &Palette) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    render_inlines_into(inlines, base, palette, &mut lines, &mut current);
    lines.push(Line::from(current));
    lines
}

fn render_inlines_into(
    inlines: &[Inline],
    base: Style,
    palette: &Palette,
    lines: &mut Vec<Line<'static>>,
    current: &mut Vec<Span<'static>>,
) {
    for inline in inlines {
        match inline {
            Inline::Text(text) => current.push(Span::styled(text.clone(), base)),
            Inline::Code(text) => current.push(Span::styled(
                text.clone(),
                Style::default().fg(palette.green).bg(palette.surface0),
            )),
            Inline::Emph(nodes) => {
                render_inlines_into(nodes, base.add_modifier(Modifier::ITALIC), palette, lines, current);
            }
            Inline::Strong(nodes) => {
                render_inlines_into(nodes, base.add_modifier(Modifier::BOLD), palette, lines, current);
            }
            Inline::Strike(nodes) => {
                let style = base.add_modifier(Modifier::CROSSED_OUT).fg(palette.overlay1);
                render_inlines_into(nodes, style, palette, lines, current);
            }
            Inline::Link { label, url } => {
                render_inlines_into(label, base, palette, lines, current);
                current.push(Span::raw(" ("));
                current.push(Span::styled(
                    url.clone(),
                    Style::default().fg(palette.lavender).add_modifier(Modifier::UNDERLINED),
                ));
                current.push(Span::raw(")"));
            }
            Inline::Image { alt } => {
                current.push(Span::styled(
                    "image: ",
                    Style::default().fg(palette.overlay0).add_modifier(Modifier::DIM),
                ));
                if !alt.is_empty() {
                    current.push(Span::styled(alt.clone(), base));
                }
            }
            Inline::SoftBreak => current.push(Span::raw(" ")),
            Inline::HardBreak => lines.push(Line::from(std::mem::take(current))),
        }
    }
}

/// Flatten `doc`'s rendered blocks into paint-ready rows at `width` columns: this is the
/// single source of row geometry (invariant G5), so what this function measures is what the
/// pane later paints. When `wrap`, each block's rendered `Line` expands to its greedy-wrapped
/// rows via [`wrap_line`] (continuations carry `first_of_block: false`); when `!wrap`, each
/// `Line` becomes exactly one row. `first_of_block` is set only for a block's very first row.
pub fn layout_rows(doc: &Document, width: usize, wrap: bool) -> Vec<RenderRow> {
    let mut rows = Vec::new();
    for (block_ix, block) in doc.blocks.iter().enumerate() {
        for (line_ix, line) in block.rendered.iter().enumerate() {
            if wrap {
                for (seg_ix, wrapped) in wrap_line(line, width).into_iter().enumerate() {
                    rows.push(RenderRow {
                        line: wrapped,
                        block: block_ix,
                        first_of_block: line_ix == 0 && seg_ix == 0,
                    });
                }
            } else {
                rows.push(RenderRow { line: line.clone(), block: block_ix, first_of_block: line_ix == 0 });
            }
        }
    }
    rows
}

/// The source line a row belongs to: its block's `source_start`.
pub fn row_source_line(doc: &Document, rows: &[RenderRow], row_ix: usize) -> u32 {
    doc.blocks[rows[row_ix].block].source_start
}

/// Greedy word-wrap of a styled `line` into `width` columns, breaking at the last space that
/// fits (hard-breaking a single word wider than the column); leading spaces on a continuation
/// row are dropped. An empty line still yields one (empty) row so it occupies a row. Ported
/// from herdr-reviewr's diff-pane `wrap_segments`/`Cell`, generalized from per-glyph diff
/// cells (fg color + emphasis flag) to arbitrary ratatui [`Span`]s (full [`Style`]), with the
/// diff change-bar/gutter/line-number concerns dropped.
pub fn wrap_line(line: &Line, width: usize) -> Vec<Line<'static>> {
    let cells = line_cells(line);
    let width = width.max(1);
    wrap_segments(&cells, width)
        .into_iter()
        .map(|(start, end)| Line::from(cells_to_spans(&cells[start..end])))
        .collect()
}

/// Tabs expand to this many columns.
const TAB: usize = 4;

/// One display cell of a styled line: a glyph, its terminal column width (via
/// `unicode-width`, so wide CJK/emoji glyphs measure as the two columns they paint), and the
/// style of the span it came from.
struct Cell {
    ch: char,
    w: usize,
    style: Style,
}

/// Expand a line's spans into display cells: tabs become spaces to the next tab stop, and
/// each char carries its column width and its span's style.
fn line_cells(line: &Line) -> Vec<Cell> {
    let mut cells = Vec::new();
    let mut col = 0usize; // display column, so tab stops land right after wide glyphs too
    for span in &line.spans {
        let style = span.style;
        for ch in span.content.chars() {
            if ch == '\t' {
                for _ in 0..(TAB - col % TAB) {
                    cells.push(Cell { ch: ' ', w: 1, style });
                    col += 1;
                }
            } else {
                let w = UnicodeWidthChar::width(ch).unwrap_or(0);
                cells.push(Cell { ch, w, style });
                col += w;
            }
        }
    }
    cells
}

/// Greedy word wrap over display cells into half-open ranges, one per display row.
///
/// Breaks at the last space that fits within `width`, falling back to a hard break when a
/// single glyph run is wider than the column. Leading spaces on a continuation are dropped so
/// a break landing just before a space doesn't leave an almost-empty row. An empty line still
/// yields one (empty) range so it occupies a row. [`wrap_line`] and [`layout_rows`] share this
/// so what's measured matches what's painted.
fn wrap_segments(cells: &[Cell], width: usize) -> Vec<(usize, usize)> {
    if cells.is_empty() {
        return vec![(0, 0)];
    }
    let mut segs = Vec::new();
    let mut start = 0;
    while start < cells.len() {
        // Take as many cells as fit within `width` columns, always at least one (so a glyph
        // wider than the column still gets its own row rather than stalling).
        let mut col = 0;
        let mut limit = start;
        while limit < cells.len() {
            let cw = cells[limit].w;
            if col + cw > width && limit > start {
                break;
            }
            col += cw;
            limit += 1;
        }
        if limit == cells.len() {
            segs.push((start, cells.len()));
            break;
        }
        // More cells follow; prefer breaking just after the last space that fits.
        let brk = (start..limit).rev().find(|&i| cells[i].ch == ' ').map(|i| i + 1);
        let end = brk.filter(|&e| e > start).unwrap_or(limit);
        segs.push((start, end));
        start = end;
        while start < cells.len() && cells[start].ch == ' ' {
            start += 1;
        }
    }
    segs
}

/// Build spans from display cells, merging runs of equal style.
fn cells_to_spans(cells: &[Cell]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut cur: Option<Style> = None;
    for cell in cells {
        if cur != Some(cell.style) {
            if let Some(style) = cur {
                spans.push(Span::styled(std::mem::take(&mut buf), style));
            }
            cur = Some(cell.style);
        }
        buf.push(cell.ch);
    }
    if let Some(style) = cur {
        spans.push(Span::styled(buf, style));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::{line_index, parse_blocks, BlockKind, Inline};

    #[test]
    fn line_index_returns_byte_offset_of_each_line_start() {
        assert_eq!(line_index("a\nbb\n"), vec![0, 2, 5]);
        assert_eq!(line_index("no newline"), vec![0]);
    }

    #[test]
    fn emphasis_builds_a_nested_inline_node() {
        let blocks = parse_blocks("*em*\n");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, BlockKind::Paragraph);
        assert_eq!(
            blocks[0].inlines,
            vec![Inline::Emph(vec![Inline::Text("em".to_string())])]
        );
    }

    #[test]
    fn strong_builds_a_nested_inline_node() {
        let blocks = parse_blocks("**strong**\n");
        assert_eq!(
            blocks[0].inlines,
            vec![Inline::Strong(vec![Inline::Text("strong".to_string())])]
        );
    }

    #[test]
    fn strikethrough_builds_a_nested_inline_node() {
        let blocks = parse_blocks("~~strike~~\n");
        assert_eq!(
            blocks[0].inlines,
            vec![Inline::Strike(vec![Inline::Text("strike".to_string())])]
        );
    }

    #[test]
    fn inline_code_builds_a_code_node() {
        let blocks = parse_blocks("`code`\n");
        assert_eq!(blocks[0].inlines, vec![Inline::Code("code".to_string())]);
    }

    #[test]
    fn link_builds_label_and_url() {
        let blocks = parse_blocks("[label](http://x)\n");
        assert_eq!(
            blocks[0].inlines,
            vec![Inline::Link {
                label: vec![Inline::Text("label".to_string())],
                url: "http://x".to_string(),
            }]
        );
    }

    #[test]
    fn image_alt_flattens_nested_formatting() {
        // Regression: alt text used to drop everything but bare Text/Code nodes, so
        // "**bold**" inside an image's alt disappeared entirely.
        let blocks = parse_blocks("![a **bold** b](img.png)\n");
        assert_eq!(
            blocks[0].inlines,
            vec![Inline::Image {
                alt: "a bold b".to_string()
            }]
        );
    }

    #[test]
    fn soft_break_separates_lines_within_a_paragraph() {
        let blocks = parse_blocks("line one\nline two\n");
        assert_eq!(
            blocks[0].inlines,
            vec![
                Inline::Text("line one".to_string()),
                Inline::SoftBreak,
                Inline::Text("line two".to_string()),
            ]
        );
    }

    #[test]
    fn hard_break_from_a_trailing_backslash() {
        let blocks = parse_blocks("line one\\\nline two\n");
        assert_eq!(
            blocks[0].inlines,
            vec![
                Inline::Text("line one".to_string()),
                Inline::HardBreak,
                Inline::Text("line two".to_string()),
            ]
        );
    }

    #[test]
    fn ordered_item_marker_and_flags() {
        let blocks = parse_blocks("1. one\n2. two\n");
        assert_eq!(
            blocks[0].kind,
            BlockKind::Item {
                ordered: true,
                marker: "1.".to_string(),
                depth: 1,
                task: None,
            }
        );
        assert_eq!(
            blocks[1].kind,
            BlockKind::Item {
                ordered: true,
                marker: "2.".to_string(),
                depth: 1,
                task: None,
            }
        );
    }

    #[test]
    fn task_list_items_carry_checked_state() {
        let blocks = parse_blocks("- [ ] todo\n- [x] done\n");
        assert_eq!(
            blocks[0].kind,
            BlockKind::Item {
                ordered: false,
                marker: "-".to_string(),
                depth: 1,
                task: Some(false),
            }
        );
        assert_eq!(
            blocks[1].kind,
            BlockKind::Item {
                ordered: false,
                marker: "-".to_string(),
                depth: 1,
                task: Some(true),
            }
        );
    }

    #[test]
    fn table_rows_carry_header_flag_and_cells() {
        let blocks = parse_blocks("| a | b |\n|---|---|\n| 1 | 2 |\n");
        assert_eq!(
            blocks[0].kind,
            BlockKind::TableRow {
                header: true,
                cells: vec![
                    vec![Inline::Text("a".to_string())],
                    vec![Inline::Text("b".to_string())],
                ],
            }
        );
        assert_eq!(
            blocks[1].kind,
            BlockKind::TableRow {
                header: false,
                cells: vec![
                    vec![Inline::Text("1".to_string())],
                    vec![Inline::Text("2".to_string())],
                ],
            }
        );
    }
}
