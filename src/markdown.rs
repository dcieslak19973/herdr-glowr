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
#[derive(Clone, Debug)]
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
#[derive(Clone, Debug)]
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

fn push_inline(stack: &mut [InlineFrame], inline: Inline) {
    match stack.last_mut() {
        Some(InlineFrame::Nodes(v)) => v.push(inline),
        Some(InlineFrame::Alt(s)) => {
            if let Inline::Text(t) | Inline::Code(t) = &inline {
                s.push_str(t);
            }
        }
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
                    let alt = match inline_stack.pop() {
                        Some(InlineFrame::Alt(s)) => s,
                        Some(InlineFrame::Nodes(v)) => v
                            .into_iter()
                            .map(|n| if let Inline::Text(t) = n { t } else { String::new() })
                            .collect(),
                        None => String::new(),
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
