//! Markdown parse + render + source mapping.
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::text::Line;
use std::ops::Range;

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
