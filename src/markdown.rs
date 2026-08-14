//! Markdown parse + render + source mapping.
use ratatui::text::Line;

/// The smallest independently-commentable unit of a rendered document.
#[derive(Clone, Debug)]
pub struct Block {
    /// 1-based inclusive source line range in the file.
    pub source_start: u32,
    pub source_end: u32,
    /// Styled terminal lines for this block.
    pub rendered: Vec<Line<'static>>,
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
