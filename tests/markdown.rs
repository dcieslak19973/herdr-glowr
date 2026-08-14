use herdr_glowr::markdown::{line_index, line_of, parse_blocks};

#[test]
fn line_of_maps_bytes_to_1_based_lines() {
    let src = "a\nbb\nccc\n";
    let idx = line_index(src);
    assert_eq!(line_of(&idx, 0), 1);
    assert_eq!(line_of(&idx, 2), 2); // 'b'
    assert_eq!(line_of(&idx, 5), 3); // 'c'
}

#[test]
fn parse_blocks_splits_heading_paragraph_and_list_items() {
    let src = "# Title\n\nA para.\n\n- one\n- two\n";
    let blocks = parse_blocks(src);
    // heading(1), paragraph(3), list item(5), list item(6)
    let spans: Vec<(u32, u32)> = blocks.iter().map(|b| (b.source_start, b.source_end)).collect();
    assert_eq!(spans, vec![(1, 1), (3, 3), (5, 5), (6, 6)]);
}

#[test]
fn parse_blocks_code_fence_is_one_block_per_line() {
    let src = "```\nlet x = 1;\nlet y = 2;\n```\n";
    let blocks = parse_blocks(src);
    // two code lines → two blocks anchored to lines 2 and 3
    assert_eq!(
        blocks.iter().map(|b| b.source_start).collect::<Vec<_>>(),
        vec![2, 3]
    );
}

#[test]
fn parse_blocks_table_is_one_block_per_row() {
    let src = "| a | b |\n|---|---|\n| 1 | 2 |\n";
    let blocks = parse_blocks(src);
    // header row (line 1) and data row (line 3); delimiter row (line 2) folds into header
    let starts: Vec<u32> = blocks.iter().map(|b| b.source_start).collect();
    assert!(starts.contains(&1) && starts.contains(&3));
}

#[test]
fn parse_blocks_table_header_span_folds_in_delimiter_row() {
    let src = "| a | b |\n|---|---|\n| 1 | 2 |\n";
    let blocks = parse_blocks(src);
    let spans: Vec<(u32, u32)> = blocks.iter().map(|b| (b.source_start, b.source_end)).collect();
    // header covers lines 1-2 (the delimiter row has no event of its own), data row is line 3.
    assert_eq!(spans, vec![(1, 2), (3, 3)]);
}

#[test]
fn parse_blocks_blockquote_is_one_block_per_paragraph() {
    let src = "> first\n> still first\n\n> second\n";
    let blocks = parse_blocks(src);
    let spans: Vec<(u32, u32)> = blocks.iter().map(|b| (b.source_start, b.source_end)).collect();
    assert_eq!(spans, vec![(1, 2), (4, 4)]);
}

#[test]
fn parse_blocks_thematic_break_is_one_block() {
    let src = "text\n\n---\n\nmore\n";
    let blocks = parse_blocks(src);
    let spans: Vec<(u32, u32)> = blocks.iter().map(|b| (b.source_start, b.source_end)).collect();
    assert_eq!(spans, vec![(1, 1), (3, 3), (5, 5)]);
}

#[test]
fn parse_blocks_html_block_is_a_single_block() {
    let src = "<div>\nraw html\n</div>\n";
    let blocks = parse_blocks(src);
    let spans: Vec<(u32, u32)> = blocks.iter().map(|b| (b.source_start, b.source_end)).collect();
    assert_eq!(spans, vec![(1, 3)]);
}

#[test]
fn parse_blocks_task_list_items_still_split_one_per_item() {
    let src = "- [ ] todo\n- [x] done\n";
    let blocks = parse_blocks(src);
    let starts: Vec<u32> = blocks.iter().map(|b| b.source_start).collect();
    assert_eq!(starts, vec![1, 2]);
}

#[test]
fn parse_blocks_indented_code_is_one_block_per_line() {
    let src = "para\n\n    line one\n    line two\n";
    let blocks = parse_blocks(src);
    let spans: Vec<(u32, u32)> = blocks.iter().map(|b| (b.source_start, b.source_end)).collect();
    // paragraph(1), then two indented code lines anchored to lines 3 and 4
    assert_eq!(spans, vec![(1, 1), (3, 3), (4, 4)]);
}
