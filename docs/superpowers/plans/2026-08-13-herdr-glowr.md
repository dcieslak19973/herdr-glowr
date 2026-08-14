# herdr-glowr Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `herdr-glowr`, a terminal markdown viewer in a herdr pane with bidirectional line-anchored comments, for reviewing agent-generated plan/spec docs.

**Architecture:** Fork the `herdr-reviewr` crate as the skeleton. Keep its comment store, theme, config, herdr-host (send/sidebar), CLI, and plugin machinery near-verbatim; strip the diff/git/PR machinery; replace the diff engine with a `pulldown-cmark`-based markdown rendering engine that maps every rendered row back to its source lines so comments anchor to source-line ranges of rendered blocks.

**Tech Stack:** Rust (edition 2024, rust-version 1.90), `ratatui` 0.30, `pulldown-cmark` (new), `syntect` + `two-face` (code-fence highlighting), `serde_json`, `toml`, `anyhow`, `unicode-width`. Dev: `tempfile`.

**Spec:** `docs/superpowers/specs/2026-08-13-herdr-glowr-design.md`

**Reference source (read-only, for copy/adapt):** `D:/git/herdr-reviewr/` — the sibling crate. Every "copy from reviewr" step names the exact source file.

## Global Constraints

- Crate/binary name: `herdr-glowr`. Plugin id: `dcieslak19973.glowr`. Pane title: `glowr`.
- Comment store dir: `<git-dir>/glowr/comments/<id>.json` (NOT `reviewr/`).
- Skill dir/name: `skills/glowr-comments/SKILL.md`, skill name `glowr-comments`.
- `#![forbid(unsafe_code)]` — via `[lints.rust] unsafe_code = "forbid"` in `Cargo.toml` (invariant G6).
- Comment schema has **no `side` field** (no diff): `{id, author, status, created_at, file, start, end, lines, text}`. `lines` is verbatim markdown source of `start..=end`.
- glowr NEVER writes the worktree; it writes only under the git dir (the store) — invariant G1.
- `auto_open` defaults to **`false`**.
- Lints from reviewr's `Cargo.toml` are retained verbatim (clippy pedantic + the pragmatic allows).
- Rename token map applied to every copied file: `reviewr` → `glowr`, `Reviewr` → `Glowr`, `REVIEWR` → `GLOWR`, `dcieslak19973.reviewr` → `dcieslak19973.glowr`, `herdr-reviewr` → `herdr-glowr`, `reviewr-comments` → `glowr-comments`.
- Commit after every task's final green step. Commit messages use Conventional Commits.

---

## File Structure

Created/modified in `D:/git/herdr-glowr/`:

- `Cargo.toml` — crate manifest (copy+trim from reviewr).
- `herdr-plugin.toml` — plugin manifest (copy+rename from reviewr).
- `rust-toolchain.toml`, `rustfmt.toml`, `.gitignore` — copy verbatim.
- `herdr/install.sh`, `herdr/install.ps1` — copy+rename.
- `src/main.rs` — entry dispatch (copy+trim: drop nothing; subcommands identical).
- `src/lib.rs` — module list + `run()` TUI entry (adapt: drop diff/git/turn/forge/browser/PR).
- `src/comments.rs` — per-comment JSON store (copy; drop `side`, dir → glowr).
- `src/model.rs` — `Comment`, `CommentStore` (copy; drop `Side`/`Scope`/`ChangeKind`/`ChangedFile`, drop `side`/`diff_anchored`).
- `src/export.rs` — comment export blocks + clipboard/agent targets (copy; `location()` has no `(removed)`).
- `src/theme.rs` — palettes (copy verbatim).
- `src/config.rs` — plugin config (copy; drop `base_branches`, add `show_ignored`).
- `src/log.rs` — logging (copy verbatim).
- `src/sidebar.rs` — toggle/open/close/auto-open orchestration (copy+rename).
- `src/herdr.rs` — agent-pane discovery + send (copy+rename; drop turn tracking).
- `src/highlight.rs` — `syntect` setup for code fences (copy; keep the highlighter, drop diff-tint helpers).
- `src/cli.rs` — agent CLI subcommands + skill-install (copy; drop `--side`).
- `src/markdown.rs` — **NEW**: parse + render + source mapping.
- `src/file_list.rs` — **NEW/rewrite**: markdown file discovery, mtime-sorted.
- `src/app.rs` — **rewrite**: glowr state machine (block cursor/selection, single/split view).
- `src/ui.rs` — **rewrite**: render header/file-list/doc(s)/composer/comments-list/footer.
- `skills/glowr-comments/SKILL.md` — agent skill (copy+adapt).
- `tests/` — `render.rs`, `comments_cli.rs`, `markdown.rs`, `file_list.rs`, `app_flow.rs`, `common/mod.rs`.
- `README.md` — user docs (adapt).

---

## Task 1: Scaffold the crate skeleton (compiles, empty TUI)

Get a compiling crate with the reuse-verbatim modules in place and a stub markdown view, so later tasks have a green baseline.

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `rustfmt.toml`, `.gitignore`
- Create: `src/main.rs`, `src/lib.rs`, `src/log.rs`, `src/theme.rs`, `src/highlight.rs`
- Create: stub `src/app.rs`, `src/ui.rs`, `src/markdown.rs`, `src/file_list.rs`

**Interfaces:**
- Produces: `herdr_glowr::run() -> anyhow::Result<()>` (TUI entry); module tree usable by later tasks.

- [ ] **Step 1: Create `Cargo.toml`** — copy `D:/git/herdr-reviewr/Cargo.toml`, then: set `name = "herdr-glowr"`, `version = "0.1.0"`, update `description`/`repository`, keep `edition`/`rust-version`/lints/profile verbatim. Set dependencies to:

```toml
[dependencies]
anyhow = "1"
ratatui = "0.30"
serde_json = "1"
syntect = { version = "5.3.0", default-features = false, features = ["default-fancy"] }
toml = "0.8"
two-face = "0.5.1"
unicode-width = "0.2"
pulldown-cmark = { version = "0.12", default-features = false }

[dev-dependencies]
tempfile = "3"
```

(Note: `similar` is dropped; `pulldown-cmark` is added.)

- [ ] **Step 2: Copy verbatim** `rust-toolchain.toml`, `rustfmt.toml`, `.gitignore`, `src/log.rs`, `src/theme.rs` from reviewr. Apply the rename token map to any `reviewr` strings (theme/log have none in identifiers; check doc comments).

- [ ] **Step 3: Copy `src/highlight.rs`** from reviewr; keep the `syntect`/`two-face` highlighter construction and the "highlight a line of code in a given language, return styled spans" helper. Delete any function that tints spans red/green for diff add/remove. Leave a `pub fn highlight_code_line(&self, line: &str) -> Vec<(Style-or-Rgb, String)>`-style API intact for code fences (match reviewr's existing return type).

- [ ] **Step 4: Write `src/lib.rs`** — module declarations and the TUI entry:

```rust
#![forbid(unsafe_code)]
pub mod app;
pub mod cli;
pub mod comments;
pub mod config;
pub mod export;
pub mod file_list;
pub mod herdr;
pub mod highlight;
pub mod log;
pub mod markdown;
pub mod model;
pub mod sidebar;
pub mod theme;
pub mod ui;

use anyhow::Result;

/// Launch the TUI in the current terminal, pointed at the cwd's git worktree.
pub fn run() -> Result<()> {
    app::run_tui()
}
```

(Modules `cli`, `comments`, `config`, `export`, `herdr`, `model`, `sidebar` are created in later tasks; for Step 4 to compile, create empty stub files `// stub` for each not-yet-created module OR order Task 1 to create them as empty modules. Create empty stubs now: `src/cli.rs`, `src/comments.rs`, `src/config.rs`, `src/export.rs`, `src/herdr.rs`, `src/model.rs`, `src/sidebar.rs`, each containing only a module doc comment. Later tasks replace them.)

- [ ] **Step 5: Write `src/main.rs`** — copy reviewr's `main.rs` verbatim, apply rename map (the `reviewr:` error prefix → `glowr:`, `herdr_reviewr::` → `herdr_glowr::`). It dispatches `--resolve-plugin-config`, `comment`/`skill-path`/`skill-install` → `cli::run`, `sidebar` → `sidebar::run`, else `run()`.

- [ ] **Step 6: Write stub `src/markdown.rs`** with the types later tasks flesh out, minimal enough to compile:

```rust
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
```

- [ ] **Step 7: Write stub `src/file_list.rs`, `src/app.rs`, `src/ui.rs`** minimal to compile: `app::run_tui() -> anyhow::Result<()>` that sets up a ratatui terminal, draws an empty frame with the title `glowr`, and quits on `q`. Model this terminal setup on reviewr's `app.rs` event loop but with an empty body. `ui::render(frame, &App)` draws the three bands (header "glowr", empty body, footer "q quit"). `file_list.rs` may be an empty module doc for now.

- [ ] **Step 8: Build and run**

Run: `cargo build`
Expected: compiles with warnings allowed but no errors.
Run: `cargo run` in a terminal; press `q`.
Expected: an empty `glowr`-titled frame appears and quits cleanly.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat: scaffold herdr-glowr crate skeleton"
```

---

## Task 2: Comment model and store (no `side`, glowr dir)

**Files:**
- Modify/replace: `src/model.rs`, `src/comments.rs`
- Test: `src/comments.rs` `#[cfg(test)]` module (port reviewr's), `tests/comments_cli.rs` deferred to Task 10.

**Interfaces:**
- Produces:
  - `model::Comment { file: String, start: u32, end: u32, lines: String, text: String }` with `fn location(&self) -> String` = `path:start` or `path:start-end` (no `(removed)`).
  - `model::CommentStore` (in-memory Vec view; port reviewr's methods, drop side-specific ones).
  - `comments::{Author, Status, StoredComment, Store, new_id, now_iso}` with `StoredComment { id, author, status, created_at, comment: Comment }`.
  - `Store::open(repo: &Path) -> Result<Store, StoreError>`, `add`, `list`, `set_status`, `remove`, `load`, and the per-tick `signature()` — same names/signatures as reviewr minus `side`.

- [ ] **Step 1: Write failing test** in `src/comments.rs` tests — port reviewr's store tests, removing `side`. Add one asserting round-trip has no `side` key:

```rust
#[test]
fn stored_comment_json_has_no_side() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().join("comments")); // test ctor bypassing git-dir lookup
    let id = store.add(&Comment {
        file: "plan.md".into(), start: 3, end: 5,
        lines: "## Phase 1\n1. do".into(), text: "split".into(),
    }, Author::User).unwrap();
    let raw = std::fs::read_to_string(dir.path().join("comments").join(format!("{id}.json"))).unwrap();
    assert!(!raw.contains("\"side\""));
    assert!(raw.contains("\"file\": \"plan.md\""));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test comments::tests::stored_comment_json_has_no_side`
Expected: FAIL (types not defined).

- [ ] **Step 3: Implement `src/model.rs`** — copy reviewr's `model.rs`, then delete `Scope`, `ChangeKind`, `ChangedFile`, `Side`, and from `Comment` delete `side` and `diff_anchored`. Rewrite `location()`:

```rust
impl Comment {
    pub fn location(&self) -> String {
        if self.start == self.end {
            format!("{}:{}", self.file, self.start)
        } else {
            format!("{}:{}-{}", self.file, self.start, self.end)
        }
    }
}
```

Keep `CommentStore` and its methods; drop any that reference `Side`/`diff_anchored`/scope.

- [ ] **Step 4: Implement `src/comments.rs`** — copy reviewr's, then: change the store subdir literal from `reviewr` to `glowr` (in the git-dir-relative path construction); remove `side` from `to_value`/`from_value`; remove `side_str`/`side_parse`; drop the `diff_anchored` handling in `StoredComment`/`load` (comment has no such field now). Add a test-only constructor `Store::at(dir: PathBuf) -> Store` (or `#[cfg(test)] pub`) so tests bypass git-dir lookup.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib comments`
Expected: PASS (all ported store tests + the new one).

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: comment model and store without diff side"
```

---

## Task 3: Source-line index and block parser

Parse markdown into `Vec<Block>` with correct 1-based source line ranges. Rendering of block content is Task 4; this task only produces block boundaries + source spans.

**Files:**
- Modify: `src/markdown.rs`
- Test: `tests/markdown.rs`

**Interfaces:**
- Consumes: `pulldown-cmark` `Parser::into_offset_iter`.
- Produces:
  - `markdown::line_index(source: &str) -> Vec<usize>` — byte offset of each line start (index 0 = line 1).
  - `markdown::line_of(index: &[usize], byte: usize) -> u32` — 1-based line containing a byte offset.
  - `markdown::parse_blocks(source: &str) -> Vec<Block>` — blocks with `source_start`/`source_end` filled, `rendered` empty (filled in Task 4).

- [ ] **Step 1: Write failing tests** in `tests/markdown.rs`:

```rust
use herdr_glowr::markdown::{line_index, line_of, parse_blocks};

#[test]
fn line_of_maps_bytes_to_1_based_lines() {
    let src = "a\nbb\nccc\n";
    let idx = line_index(src);
    assert_eq!(line_of(&idx, 0), 1);
    assert_eq!(line_of(&idx, 2), 2);   // 'b'
    assert_eq!(line_of(&idx, 5), 3);   // 'c'
}

#[test]
fn parse_blocks_splits_heading_paragraph_and_list_items() {
    let src = "# Title\n\nA para.\n\n- one\n- two\n";
    let blocks = parse_blocks(src);
    // heading(1), paragraph(3), list item(5), list item(6)
    let spans: Vec<(u32,u32)> = blocks.iter().map(|b| (b.source_start, b.source_end)).collect();
    assert_eq!(spans, vec![(1,1),(3,3),(5,5),(6,6)]);
}

#[test]
fn parse_blocks_code_fence_is_one_block_per_line() {
    let src = "```\nlet x = 1;\nlet y = 2;\n```\n";
    let blocks = parse_blocks(src);
    // two code lines → two blocks anchored to lines 2 and 3
    assert_eq!(blocks.iter().map(|b| b.source_start).collect::<Vec<_>>(), vec![2,3]);
}

#[test]
fn parse_blocks_table_is_one_block_per_row() {
    let src = "| a | b |\n|---|---|\n| 1 | 2 |\n";
    let blocks = parse_blocks(src);
    // header row (line 1) and data row (line 3); delimiter row (line 2) folds into header
    let starts: Vec<u32> = blocks.iter().map(|b| b.source_start).collect();
    assert!(starts.contains(&1) && starts.contains(&3));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test markdown`
Expected: FAIL (functions unimplemented).

- [ ] **Step 3: Implement `line_index`/`line_of`:**

```rust
pub fn line_index(source: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' { starts.push(i + 1); }
    }
    starts
}

pub fn line_of(index: &[usize], byte: usize) -> u32 {
    // last line-start <= byte
    match index.binary_search(&byte) {
        Ok(i) => (i + 1) as u32,
        Err(i) => i as u32, // i is count of starts <= byte
    }
}
```

- [ ] **Step 4: Implement `parse_blocks`** using `pulldown_cmark::Parser::new_ext(source, Options::all()).into_offset_iter()`. Fold the `(Event, Range<usize>)` stream into blocks. Enable GFM: `Options::ENABLE_TABLES | ENABLE_TASKLISTS | ENABLE_STRIKETHROUGH`. Algorithm:
  - Maintain a stack of open container kinds (list, blockquote, table).
  - On `Start(Heading|Paragraph)` at the top container level, open a block; on its matching `End`, close it, computing `source_start = line_of(idx, range.start)`, `source_end = line_of(idx, range.end.saturating_sub(1))`.
  - `Start(Item)`/`End(Item)`: one block per list item (use the item's offset range).
  - `Start(CodeBlock)`..`End(CodeBlock)`: split the fenced text on `\n`; emit one block per non-empty code line, `source_start=source_end` = that line's number (compute from the code block's start offset + cumulative line count; simplest: `line_of(idx, text_range.start)` incremented per line).
  - `Start(Table)`: capture header row range as one block; each `TableRow` as its own block; the delimiter line has no event, so it naturally folds into the header block's span.
  - `Rule` (thematic break): one block for its line.
  - `Html`/`Start(HtmlBlock)`: one block for its offset range.
  - Store the inline events per block too (a `Vec<Event<'static>>` field on a private builder) so Task 4 can render them without re-parsing — OR re-slice `source[range]` in Task 4. **Decision: store `pub(crate) events: Vec<OwnedEvent>` on `Block`** (owned via `Event::into_static`-equivalent: map borrowed `CowStr` to `String`), so Task 4 renders from events. Add that field now (Task 4 consumes it).

  Add to `Block`:
```rust
    pub(crate) kind: BlockKind,        // Heading(u8)|Paragraph|Item{ordered,marker,depth,task:Option<bool>}|Quote|Code{lang:Option<String>,line:String}|TableRow{header:bool,cells:Vec<Vec<Inline>>}|Rule|Html(String)
    pub(crate) inlines: Vec<Inline>,   // for text-bearing blocks
```
  where `Inline` is a flattened inline model built in this task from inline events:
```rust
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
```
  Build `Inline` trees by folding inline events between a block's `Start`/`End`. This keeps Task 4 (styling) independent of the parser.

- [ ] **Step 5: Run to verify pass**

Run: `cargo test --test markdown`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: markdown block parser with source-line mapping"
```

---

## Task 4: Render blocks to styled ratatui lines

Fill each `Block::rendered` with themed `Line`s from its `kind`/`inlines`, using the theme palette and the code highlighter.

**Files:**
- Modify: `src/markdown.rs`
- Test: `tests/markdown.rs`

**Interfaces:**
- Consumes: `theme::Palette`, `highlight` (code fences), `Block`/`Inline`/`BlockKind` from Task 3.
- Produces: `markdown::render_document(source: &str, palette: &Palette, hl: &Highlighter) -> Document` — returns `Document` with `blocks` fully rendered (`Block::rendered` non-empty).

- [ ] **Step 1: Write failing tests:**

```rust
use herdr_glowr::{markdown::render_document, theme::Palette, highlight::Highlighter};

fn plain(doc: &herdr_glowr::markdown::Document) -> Vec<String> {
    doc.blocks.iter().flat_map(|b| b.rendered.iter().map(line_text)).collect()
}
fn line_text(l: &ratatui::text::Line) -> String {
    l.spans.iter().map(|s| s.content.as_ref()).collect()
}

#[test]
fn heading_renders_with_hash_prefix_text() {
    let doc = render_document("# Hello\n", &Palette::default(), &Highlighter::new());
    assert!(plain(&doc).iter().any(|s| s.contains("Hello")));
}

#[test]
fn bullet_item_renders_marker_and_text() {
    let doc = render_document("- item one\n", &Palette::default(), &Highlighter::new());
    let text: String = plain(&doc).join("\n");
    assert!(text.contains("item one"));
    assert!(text.contains("•") || text.contains("-"));
}

#[test]
fn link_shows_label_and_url() {
    let doc = render_document("[docs](http://x)\n", &Palette::default(), &Highlighter::new());
    let text: String = plain(&doc).join("\n");
    assert!(text.contains("docs") && text.contains("http://x"));
}

#[test]
fn code_fence_line_is_rendered_verbatim() {
    let doc = render_document("```rust\nlet x = 1;\n```\n", &Palette::default(), &Highlighter::new());
    assert!(plain(&doc).iter().any(|s| s.contains("let x = 1;")));
}
```

(`Palette::default()` and `Highlighter::new()` must exist — add a `Default` impl / `new` ctor if reviewr's differ; adapt the test to the actual constructors.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test markdown`
Expected: FAIL.

- [ ] **Step 3: Implement `render_document`** — call `parse_blocks`, then for each block build `rendered: Vec<Line<'static>>`:
  - **Heading(n)**: bold, colored by level (h1 `mauve`/`peach`, h2 `blue`, else `text`), prefixed with the `#`×n dimmed or a level glyph; one line (wrapping handled later by the row layer).
  - **Paragraph**: render `inlines` to `Vec<Span>` via `render_inlines(&inlines, base_style, palette)`; one `Line` (multi-line only via HardBreak).
  - **Item**: indent by `depth*2`, marker `•` (bullet) / `N.` (ordered) / `[ ]`/`[x]` (task), then inline spans.
  - **Quote**: prefix each line with a `▍` bar in `overlay1`, body in `subtext0` italic.
  - **Code{lang,line}**: highlight `line` via `hl` for `lang`, fall back to plain `text` color; render as one `Line` with a subtle code background if the palette defines one.
  - **TableRow{header,cells}**: compute per-column widths across the table (pass the whole table's rows together — implement table rendering by grouping consecutive `TableRow` blocks; simplest: render each row independently padding each cell to a fixed share of a nominal width, with header bold). Column alignment refinement is roadmap; v1 pads to even columns.
  - **Rule**: a full-width `─` line in `overlay0`.
  - **Html(raw)**: dimmed `overlay0` verbatim lines.
  - `render_inlines`: fold `Inline` into spans — `Text` base style; `Code` on `code` bg/`green`; `Emph` italic; `Strong` bold; `Strike` crossed (or `overlay1` if unsupported); `Link` label spans then ` (url)` in `blue`/underline; `Image` `alt` with a dim `image:` chip; `SoftBreak` → space; `HardBreak` → new `Line`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test --test markdown`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: render markdown blocks to themed ratatui lines"
```

---

## Task 5: Flatten to render rows with wrap + heights

Produce the paint-ready `Vec<RenderRow>` for a document at a given width, and per-row display heights, reusing reviewr's greedy wrap. This is the geometry single-source (invariant G5).

**Files:**
- Modify: `src/markdown.rs`
- Test: `tests/markdown.rs`

**Interfaces:**
- Consumes: `Document` (rendered blocks), a wrap helper.
- Produces:
  - `markdown::wrap_line(line: &Line, width: usize) -> Vec<Line<'static>>` — greedy word-wrap of a styled line into `width` columns (port reviewr `ui.rs::wrap_segments`/`Cell` logic into `markdown.rs`, generalized from diff cells to arbitrary spans).
  - `markdown::layout_rows(doc: &Document, width: usize, wrap: bool) -> Vec<RenderRow>` — flatten blocks to rows; when `wrap`, each block line expands to its wrapped rows (continuations carry `first_of_block=false`); when `!wrap`, one row per line.
  - `markdown::row_source_line(doc, rows, row_ix) -> u32` — the source line for a row (its block's `source_start`).

- [ ] **Step 1: Write failing tests:**

```rust
use herdr_glowr::markdown::{render_document, layout_rows};
use herdr_glowr::{theme::Palette, highlight::Highlighter};

#[test]
fn layout_wraps_long_paragraph_into_multiple_rows() {
    let long = "word ".repeat(40);
    let doc = render_document(&format!("{long}\n"), &Palette::default(), &Highlighter::new());
    let rows = layout_rows(&doc, 20, true);
    assert!(rows.len() > 1);
    assert!(rows[0].first_of_block);
    assert!(!rows[1].first_of_block);
    assert!(rows.iter().all(|r| r.block == 0));
}

#[test]
fn layout_no_wrap_is_one_row_per_line() {
    let doc = render_document("# H\n\npara\n", &Palette::default(), &Highlighter::new());
    let rows = layout_rows(&doc, 80, false);
    assert_eq!(rows.iter().filter(|r| r.first_of_block).count(), 2); // heading + paragraph
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test --test markdown`; Expected FAIL.

- [ ] **Step 3: Implement** `wrap_line` (port `wrap_segments` + `Cell` from reviewr `ui.rs`, taking `&[Span]` instead of a diff `Row`; use `unicode-width` for cell widths, drop the diff change-bar/gutter logic), then `layout_rows` (iterate blocks; for each rendered `Line`, wrap or not, push `RenderRow{ line, block: bi, first_of_block: (line_ix==0 && seg_ix==0) }`).

- [ ] **Step 4: Run to verify pass** — `cargo test --test markdown`; Expected PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: flatten rendered blocks to wrapped render rows"
```

---

## Task 6: Markdown file discovery (mtime-sorted)

**Files:**
- Modify: `src/file_list.rs`
- Test: `tests/file_list.rs`

**Interfaces:**
- Produces: `file_list::markdown_files(repo: &Path, show_ignored: bool) -> Vec<FileEntry>` where `FileEntry { path: String /* repo-relative, forward-slashed */, mtime: SystemTime, ignored: bool }`, sorted by descending mtime then path. Uses `git ls-files` for tracked + (optionally) untracked non-ignored, filtered to `.md`/`.markdown`; `ignored` entries only when `show_ignored`.

- [ ] **Step 1: Write failing test** in `tests/file_list.rs` using a temp git repo (port `tests/common/git_repo.rs` helper from reviewr — copy it into `tests/common/mod.rs`):

```rust
mod common;
use common::TempRepo;
use herdr_glowr::file_list::markdown_files;

#[test]
fn lists_markdown_newest_first_and_skips_non_markdown() {
    let repo = TempRepo::new();
    repo.write("old.md", "# old");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    repo.write("new.md", "# new");
    repo.write("code.rs", "fn main(){}");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "init"]);
    let files = markdown_files(repo.path(), false);
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["new.md", "old.md"]); // newest first, no code.rs
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test --test file_list`; Expected FAIL.

- [ ] **Step 3: Implement `markdown_files`** — run `git -C repo ls-files --cached --others --exclude-standard` for the base set; when `show_ignored`, also `--ignored` with `--others`; keep entries whose extension is `md`/`markdown` (case-insensitive); stat each for mtime (relative to repo root); sort by `(Reverse(mtime), path)`; normalize separators to `/`.

- [ ] **Step 4: Run to verify pass** — `cargo test --test file_list`; Expected PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: markdown file discovery sorted by mtime"
```

---

## Task 7: App state — block cursor, selection, anchor

The terminal-free state machine core: hold documents, a block cursor per doc pane, selection, and the comment store; compute a comment anchor from a selection.

**Files:**
- Modify: `src/app.rs`
- Test: `src/app.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: `markdown::{Document, layout_rows, Block}`, `model::{Comment, CommentStore}`, `comments::{Store, Author, Status, StoredComment}`, `file_list::FileEntry`.
- Produces (used by `ui.rs` in Task 8 and event loop in Task 9):
  - `app::App` with fields: `repo: PathBuf`, `files: Vec<FileEntry>`, `docs: [DocPane; 2]`, `split: bool`, `focus: Focus`, `store: Option<Store>`, `comments: CommentStore`, `list_pct: u16`, `wrap: bool`, `mode: Mode`, `input: String`, `caret: usize`, `should_quit: bool`.
  - `DocPane { path: Option<String>, doc: Document, cursor_block: usize, sel_anchor: Option<usize>, scroll: usize }`.
  - `enum Focus { List, DocA, DocB }`, `enum Mode { Browse, Composing{editing:Option<usize>}, CommentsList{cursor:usize} }`.
  - `App::selection_range(&self, pane: usize) -> (usize, usize)` — inclusive block index range (ordered).
  - `App::anchor(&self, pane: usize) -> Option<(u32, u32, String)>` — `(start, end, lines)` from the selection: `start=min source_start`, `end=max source_end`, `lines = source[byte-of-start-line ..= byte-of-end-line]` verbatim.
  - `App::move_cursor(&mut self, pane, delta)`, `start_selection`, `extend_selection`, `clear_selection`.
  - `App::add_comment(&mut self, pane, text)` — computes anchor, writes to store (Author::User), pushes to `comments`.

- [ ] **Step 1: Write failing tests** in `src/app.rs`:

```rust
#[test]
fn anchor_spans_selected_blocks_verbatim() {
    let src = "# Title\n\nfirst para\n\nsecond para\n";
    let mut app = App::for_test(src);           // helper: single-doc app with rendered doc
    // blocks: heading(1), para(3), para(5)
    app.docs[0].cursor_block = 1;               // "first para"
    app.start_selection(0);
    app.extend_selection(0, 1);                 // extend to "second para" (block 2)
    let (start, end, lines) = app.anchor(0).unwrap();
    assert_eq!((start, end), (3, 5));
    assert_eq!(lines, "first para\n\nsecond para"); // verbatim source lines 3..=5
}

#[test]
fn add_comment_persists_and_appears_in_store_view() {
    let mut app = App::for_test("# H\n\npara\n");
    app.docs[0].cursor_block = 1;
    app.add_comment(0, "note".into());
    assert_eq!(app.comments.open_user_comments().len(), 1);
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test --lib app`; Expected FAIL.

- [ ] **Step 3: Implement** the types and methods above. `anchor` maps block source lines back to bytes via a stored `line_index` on `DocPane` (add `line_starts: Vec<usize>` to `DocPane`, filled at load), slicing `source[line_starts[start-1] .. end_byte]` where `end_byte` is the end of source line `end` (next line-start or EOF), then `trim_end_matches('\n')`. Provide `#[cfg(test)] fn for_test(src: &str) -> App` building a one-doc app via `render_document` with a default palette + highlighter and a `CommentStore` backed by an in-memory/temp store or `None` (store optional; `add_comment` no-ops persistence when `store` is `None` but still updates the in-memory `comments`). Port `CommentStore::open_user_comments`/`sendable` from reviewr.

- [ ] **Step 4: Run to verify pass** — `cargo test --lib app`; Expected PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: app state with block cursor, selection, and anchoring"
```

---

## Task 8: Render the single-doc view (list + doc + cards + composer + list overlay)

**Files:**
- Modify: `src/ui.rs`, `src/app.rs` (footer actions, comment-card helpers)
- Test: `tests/render.rs` (ratatui `TestBackend` snapshot-style assertions)

**Interfaces:**
- Consumes: `App`, `markdown::layout_rows`, `theme::Palette`.
- Produces: `ui::render(frame, &App)` painting header (`glowr` + `[ Send (N) ]`), body (doc left, file list right, draggable divider), footer; comment cards spliced under their block's last visible row; inline composer under the selection when `Mode::Composing`; comments-list overlay when `Mode::CommentsList`. Reuse from reviewr `ui.rs`: `render_composer`, `render_comments_list`, `render_footer`, `bordered`, `selectable_row`, `centered`, hit-test helpers (`hit_file`, `hit_divider`, `body_rect`), and the comment-card renderer (`comment_card_lines`) — retarget the card's location text to `Comment::location()` (no `(removed)`), keep the `agent` chip and resolved styling.

- [ ] **Step 1: Write failing test** in `tests/render.rs`:

```rust
use ratatui::{backend::TestBackend, Terminal};
use herdr_glowr::{ui, app::App};

#[test]
fn renders_title_doc_text_and_file_list() {
    let mut app = App::for_test_with_file("plan.md", "# Plan\n\nstep one\n");
    let backend = TestBackend::new(80, 20);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| ui::render(f, &app)).unwrap();
    let buf = term.backend().buffer().clone();
    let text = buffer_to_string(&buf); // helper concatenating cells row-by-row
    assert!(text.contains("glowr"));
    assert!(text.contains("Plan"));
    assert!(text.contains("step one"));
    assert!(text.contains("plan.md"));
}

#[test]
fn renders_comment_card_under_block() {
    let mut app = App::for_test_with_file("plan.md", "# Plan\n\nstep one\n");
    app.docs[0].cursor_block = 1;
    app.add_comment(0, "clarify".into());
    let backend = TestBackend::new(80, 20);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| ui::render(f, &app)).unwrap();
    assert!(buffer_to_string(&term.backend().buffer()).contains("clarify"));
}
```

Add `buffer_to_string` to `tests/common/mod.rs`.

- [ ] **Step 2: Run to verify failure** — `cargo test --test render`; Expected FAIL.

- [ ] **Step 3: Implement** `ui::render` and helpers. Port reviewr's `panes`/`vrows` split (doc pane replaces diff pane), `render_file_list` simplified (no stats/markers; dim ignored; group by dir like reviewr's basename/dim-parent rendering), a new `render_doc_view` that: computes `layout_rows(&doc, width, app.wrap)`, splices `comment_card_lines` after the last visible row of each block that has comments (filtered by `card_visible`), applies the block-cursor/selection fill to every row whose `block` is the cursor block or within the selection range, windows by `doc.scroll`, and paints. Provide the geometry helpers (`doc_row_heights`, `hit_doc`) mirroring reviewr's `diff_row_heights`/`hit_diff` but over render rows + cards.

- [ ] **Step 4: Run to verify pass** — `cargo test --test render`; Expected PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: single-doc TUI view with comment cards and composer"
```

---

## Task 9: Event loop, key bindings, and split-doc mode

Wire real terminal input to `App`, including `Tab` focus cycle, block navigation/selection, comment/edit/delete/resolve, send/copy, width resize, wrap, and backtick split toggle. Two-doc split rendering.

**Files:**
- Modify: `src/app.rs` (event handling), `src/ui.rs` (split layout), `tests/app_flow.rs`

**Interfaces:**
- Consumes: everything from Tasks 5–8; `export::{Agent, Clipboard, format_all}`; `herdr` (agent send).
- Produces: `app::run_tui() -> Result<()>`; `App::on_key(&mut self, key: KeyEvent, area: Rect)` (terminal-free, testable); `App::toggle_split`, `App::cycle_focus`, `App::export(target)`.

- [ ] **Step 1: Write failing tests** in `tests/app_flow.rs` (drive `on_key`, no real terminal):

```rust
use herdr_glowr::app::App;
use ratatui::layout::Rect;
use crossterm::event::{KeyEvent, KeyCode}; // ratatui re-exports crossterm; use its path

fn key(c: char) -> KeyEvent { KeyEvent::from(KeyCode::Char(c)) }
const AREA: Rect = Rect { x:0, y:0, width:80, height:24 };

#[test]
fn v_then_j_then_c_selects_and_opens_composer() {
    let mut app = App::for_test_with_file("p.md", "# H\n\na\n\nb\n");
    app.focus_doc_for_test(0);
    app.docs[0].cursor_block = 1;
    app.on_key(key('v'), AREA);
    app.on_key(key('j'), AREA);        // extend to block 2
    app.on_key(key('c'), AREA);
    assert!(matches!(app.mode, herdr_glowr::app::Mode::Composing{..}));
    assert_eq!(app.selection_range(0), (1, 2));
}

#[test]
fn backtick_toggles_split() {
    let mut app = App::for_test_with_file("p.md", "# H\n");
    assert!(!app.split);
    app.on_key(key('`'), AREA);
    assert!(app.split);
    app.on_key(key('`'), AREA);
    assert!(!app.split);
}

#[test]
fn q_quits() {
    let mut app = App::for_test_with_file("p.md", "# H\n");
    app.on_key(key('q'), AREA);
    assert!(app.should_quit);
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test --test app_flow`; Expected FAIL.

- [ ] **Step 3: Implement `on_key`** — a match over `(app.mode, app.focus, key)` mirroring reviewr's key map minus tabs/scopes/PR: `q` quit; `Tab` `cycle_focus`; in a doc pane `j/k` move block, `v` start sel, `esc` clear, `c` compose (`add_comment` on save), `e/d` edit/delete under cursor, `n/N` jump comment, `l` open comments list, `s` send (`export(Agent)`), `y` copy (`export(Clipboard)`), `w` wrap toggle, `[`/`]` resize, `` ` `` toggle split; in list focus `j/k` select file → load into the focused doc pane; composer keys reuse reviewr's caret editor (`caret_vertical`, word ops). Then implement `run_tui` as reviewr's event loop retargeted to `on_key`, with the poll-based store re-read (reuse `Store::signature`) and file-list refresh. Port `App::export` from reviewr (persist open user comments, then `format_all` → target).

- [ ] **Step 4: Implement split rendering** in `ui.rs` — when `app.split`, divide the doc region into two side-by-side `render_doc_view`s (doc A, doc B), each with its own focus highlight; file list stays on the right. Add a `render.rs` test asserting two docs paint:

```rust
#[test]
fn split_renders_two_docs() {
    let mut app = App::for_test_with_file("a.md", "# AAA\n");
    app.load_into_test(1, "b.md", "# BBB\n"); // put a second doc in pane B
    app.split = true;
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 20)).unwrap();
    term.draw(|f| herdr_glowr::ui::render(f, &app)).unwrap();
    let t = buffer_to_string(&term.backend().buffer());
    assert!(t.contains("AAA") && t.contains("BBB"));
}
```

- [ ] **Step 5: Run to verify pass** — `cargo test --test app_flow --test render`; Expected PASS. Then `cargo run` and manually confirm: open a `.md`, `v`+`j` select, `c` comment, `` ` `` split, `q` quit.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: event loop, key bindings, and split-doc mode"
```

---

## Task 10: Agent CLI, export format, config, sidebar, herdr host

Bring over the agent-facing surface and plugin plumbing.

**Files:**
- Modify: `src/cli.rs`, `src/export.rs`, `src/config.rs`, `src/sidebar.rs`, `src/herdr.rs`, `herdr-plugin.toml`, `herdr/install.sh`, `herdr/install.ps1`
- Test: `tests/comments_cli.rs`

**Interfaces:**
- Produces: `cli::run(args: Vec<String>) -> ExitCode` with subcommands `comment add/list/resolve/rm` (no `--side`), `skill-path`, `skill-install`; `config::print_plugin_config`; `sidebar::run`.

- [ ] **Step 1: Write failing test** in `tests/comments_cli.rs` (port reviewr's, drop `--side`):

```rust
mod common;
use common::TempRepo;
use std::process::Command;

fn bin() -> &'static str { env!("CARGO_BIN_EXE_herdr-glowr") }

#[test]
fn add_then_list_roundtrips_without_side() {
    let repo = TempRepo::new();
    repo.git(&["init"]);
    let out = Command::new(bin()).current_dir(repo.path())
        .args(["comment","add","--file","plan.md","--start","3","--text","fix this"])
        .output().unwrap();
    assert!(out.status.success());
    let id = String::from_utf8(out.stdout).unwrap();
    assert!(id.trim().starts_with("c-"));
    let list = Command::new(bin()).current_dir(repo.path())
        .args(["comment","list","--json"]).output().unwrap();
    let json = String::from_utf8(list.stdout).unwrap();
    assert!(json.contains("\"file\": \"plan.md\""));
    assert!(!json.contains("\"side\""));
}

#[test]
fn add_rejects_unknown_side_flag() {
    let repo = TempRepo::new();
    repo.git(&["init"]);
    let out = Command::new(bin()).current_dir(repo.path())
        .args(["comment","add","--file","p.md","--start","1","--side","old","--text","x"])
        .output().unwrap();
    assert_eq!(out.status.code(), Some(2)); // unknown flag → usage error
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test --test comments_cli`; Expected FAIL.

- [ ] **Step 3: Implement**:
  - `src/cli.rs`: copy reviewr's, remove the `--side` flag parsing and `parse_side`/`side_str`; `comment_add` builds `Comment { file, start, end, lines, text }`; `stored_to_json` drops `side`; `USAGE` string updated (`herdr-glowr …`, no `--side`); `skill` source resolves `skills/glowr-comments/SKILL.md`.
  - `src/export.rs`: copy reviewr's; `format_comment` already uses `Comment::location()` (now no `(removed)`) and `comment.lines` — no `+/-` assumptions; keep `Clipboard`/`Agent`/`format_all`.
  - `src/config.rs`: copy reviewr's; delete `base_branches`; add `show_ignored: bool` (default false) and keep `theme`, `toggle_placement`, `toggle_direction`, `auto_open` (default **false**), `comment_sync`.
  - `src/sidebar.rs`, `src/herdr.rs`: copy+rename; delete turn-tracking (`TurnTracker`, `refs/reviewr/…`) from `herdr.rs`; keep agent-pane discovery + `herdr agent send` + focus, and sidebar toggle/open/close/auto-open.
  - `herdr-plugin.toml`: copy reviewr's; rename all ids to `dcieslak19973.glowr`/`glowr`, pane title `glowr`, commands invoke `herdr-glowr`. Keep the macOS/linux + windows twins and the `-windows` action-id suffixes.
  - `herdr/install.sh`, `install.ps1`: copy+rename (`herdr-reviewr` → `herdr-glowr`, release repo → glowr's).

- [ ] **Step 4: Run to verify pass** — `cargo test --test comments_cli`; Expected PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: agent CLI, export, config, and herdr plugin plumbing"
```

---

## Task 11: Agent skill and README

**Files:**
- Create: `skills/glowr-comments/SKILL.md`, `README.md`

- [ ] **Step 1: Write `skills/glowr-comments/SKILL.md`** — adapt reviewr's `SKILL.md`: name `glowr-comments`; description "Read, act on, and leave line-anchored comments on plan/spec markdown shared with the herdr-glowr viewer."; binary `herdr-glowr`; the loop = `comment list` → revise the doc → `comment resolve <id>` → `comment add --file <doc>.md --start N --lines '<markdown>' --text '…'`; trust the `lines` snippet over line numbers; never `rm` a user comment. Remove diff-specific wording (`+`/`-` markers).

- [ ] **Step 2: Write `README.md`** — adapt reviewr's top sections: what glowr is (markdown plan/spec reviewer), install via marketplace, the core loop (open, pick doc, `v` select, `c` comment, `s` send), controls table (drop scope/tab/PR rows; add `` ` `` split), "Working with agents" pointing at `glowr-comments` + the CLAUDE.md snippet, themes. Remove PR/forge/scopes sections.

- [ ] **Step 3: Verify skill path resolves**

Run: `cargo run -- skill-path`
Expected: prints the path to `skills/glowr-comments/SKILL.md` (dev-checkout fallback), exit 0.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "docs: glowr-comments skill and README"
```

---

## Task 12: Full-suite verification and manual smoke

**Files:** none (verification only), plus any fixes surfaced.

- [ ] **Step 1: Format and lint**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings`
Expected: clean (fix any clippy findings inline; the pedantic allows from `Cargo.toml` apply).

- [ ] **Step 2: Full test suite**

Run: `cargo test`
Expected: all unit + integration tests pass.

- [ ] **Step 3: Manual smoke** — from a git repo containing a couple of `.md` files:

Run: `cargo run`
Verify: newest `.md` selected; `Tab` into doc; `j/k` move the block cursor; `v`+`j` select blocks; `c` writes a card that appears under the block; `s` (with an agent pane present) or `y` exports `path:start-end\n<markdown>\n<text>`; `` ` `` splits into two docs; second doc loads via the list; `q` quits cleanly with the terminal restored.

Run (separately): `herdr-glowr comment list` in that repo after adding one via the TUI.
Verify: the comment shows with `<id>  open  user  <file>:<start>-<end>  <text>` and no `(removed)`.

- [ ] **Step 4: Commit any fixes**

```bash
git add -A
git commit -m "chore: verification fixes"
```

---

## Self-Review (completed by plan author)

**Spec coverage:** viewer + comments (Tasks 3–9), block anchoring (Task 7), pulldown-cmark block model + source mapping (Tasks 3–5), single/split layout (Tasks 8–9), markdown file discovery mtime-sorted (Task 6), store without `side` (Task 2), CLI + export (Task 10), herdr host + `auto_open=false` (Task 10), skill + config (Tasks 10–11), themes (Task 1 copy), invariants G1/G2/G6 (Tasks 1–2, store), G5 geometry single-source (Task 5). No PR/forge/git/scopes tasks — correctly out of scope.

**Placeholders:** none — every code step carries real code or an exact copy-source + token map.

**Type consistency:** `Comment{file,start,end,lines,text}`, `StoredComment{id,author,status,created_at,comment}`, `Document/Block/RenderRow/Inline/BlockKind`, `DocPane`, `Focus`, `Mode`, `render_document`/`parse_blocks`/`layout_rows`/`line_index`/`line_of`, `markdown_files`/`FileEntry`, `App::{anchor,selection_range,add_comment,on_key,export,toggle_split,cycle_focus}` — names used consistently across tasks.

**Note for executors:** reviewr module bodies are only summarized in this plan (structural), not pasted in full. For each "copy from reviewr `src/X.rs`" step, open the reference file at `D:/git/herdr-reviewr/src/X.rs`, copy it, then apply the rename token map and the named deletions. The novel modules (`markdown.rs`, and the `app.rs`/`ui.rs`/`file_list.rs` rewrites) carry full guidance and tests here.
