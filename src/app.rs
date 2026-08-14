//! Application state: the terminal-free core of the `glowr` viewer.
//!
//! `App` owns every piece of session state — the file list, up to two open documents
//! (single view or split), the block cursor/selection per pane, and the comment store —
//! without touching the terminal. `src/main.rs` calls [`run_tui`] to drive it through a
//! real event loop; `src/ui.rs` renders it read-only. Keeping this module terminal-free
//! means every transition here is a plain, synchronously-testable method call: no
//! `Frame`, no `crossterm::Event`, no I/O beyond the optional [`Store`] a comment is
//! persisted through.

use std::path::PathBuf;

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};

use crate::comments::{Author, Store};
use crate::file_list::FileEntry;
use crate::logln;
use crate::markdown::{Block, Document};
use crate::model::{Comment, CommentStore};
use crate::theme::{self, Palette};
use crate::ui;

/// One open document: its rendered form, the reviewer's block cursor and selection
/// within it, and enough to map a selection back onto verbatim source bytes.
#[derive(Debug, Clone, Default)]
pub struct DocPane {
    /// Repo-relative path this pane was loaded from; `None` for an unopened pane.
    pub path: Option<String>,
    /// The rendered document (source + parsed/styled blocks).
    pub doc: Document,
    /// Index into `doc.blocks` the cursor currently rests on.
    pub cursor_block: usize,
    /// The other end of an in-progress selection, if any; `cursor_block` is the moving
    /// end.
    pub sel_anchor: Option<usize>,
    /// Scroll offset in rendered rows, maintained by the view layer.
    pub scroll: usize,
    /// Byte offset of the start of each 1-based source line in `doc.source`
    /// ([`crate::markdown::line_index`]), cached at load so [`App::anchor`] never
    /// re-scans the source.
    pub line_starts: Vec<usize>,
}

/// Which pane currently has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    /// The file-list sidebar.
    #[default]
    List,
    /// The first document pane (the only one shown unless `split`).
    DocA,
    /// The second document pane, visible only when `split`.
    DocB,
}

/// The reviewer's current interaction mode.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Mode {
    /// Reading and navigating; the default.
    #[default]
    Browse,
    /// Composing a comment in `App::input`. `editing` names the index into `comments`
    /// being revised, or `None` for a brand-new comment.
    Composing { editing: Option<usize> },
    /// The comment list overlay is open, with its own cursor into `comments`.
    CommentsList { cursor: usize },
}

/// A footer action — what the bar offers for the current context. Semantic only:
/// `ui::render_footer` maps each to its key glyph and label and styles it by [`Tier`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FooterAction {
    Comment,
    Select,
    ClearSelection,
    EditComment,
    DeleteComment,
    JumpComment,
    /// Switch focus between the file list and the doc pane; the label names the destination.
    CycleFocus,
    Send,
    List,
    Copy,
    /// Toggle the split-doc layout (Task 9).
    Split,
    Save,
    Newline,
    Cancel,
    CloseList,
    /// Flip the resolve/reopen status of the highlighted row (comments-list overlay only).
    ResolveComment,
    Quit,
}

/// A footer action's visual weight, and its survival priority when the line is too narrow:
/// `Orientation` is dropped first, then trailing `Normal` actions; `Primary` is never dropped.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    Primary,
    Normal,
    Orientation,
}

/// The file-list pane's default width, as a percentage of the terminal width.
const DEFAULT_LIST_PCT: u16 = 32;

/// The full state of a viewer session.
#[derive(Debug)]
pub struct App {
    /// The worktree root every relative path (files, comments) is resolved against.
    pub repo: PathBuf,
    /// The markdown files discovered in `repo`, newest-modified first.
    pub files: Vec<FileEntry>,
    /// Index into `files` the file-list cursor rests on.
    pub file_cursor: usize,
    /// Scroll offset (rows) into `files`, kept in view by whatever moves `file_cursor`.
    pub file_scroll: usize,
    /// The two document panes; only `docs[0]` is shown unless `split`.
    pub docs: [DocPane; 2],
    /// Whether both panes are visible side by side.
    pub split: bool,
    /// Which pane has keyboard focus.
    pub focus: Focus,
    /// The on-disk comment store for `repo`, or `None` when unavailable (e.g. not a git
    /// worktree) — [`App::add_comment`] degrades to in-memory-only persistence then.
    pub store: Option<Store>,
    /// The session's view over every comment: freshly added, and synced in from `store`.
    pub comments: CommentStore,
    /// Width of the file-list pane, as a percentage of the terminal width.
    pub list_pct: u16,
    /// Whether document body text soft-wraps to the pane width.
    pub wrap: bool,
    /// The current interaction mode.
    pub mode: Mode,
    /// Text of the comment currently being composed (`Mode::Composing`).
    pub input: String,
    /// Caret position within `input`, in bytes.
    pub caret: usize,
    /// Set once the user has asked to quit.
    pub should_quit: bool,
    /// The active palette every renderer paints from.
    pub palette: Palette,
}

/// `App`'s defaults for a session that has not yet loaded a repo: an empty file list, a
/// comfortable file-list width, wrap on (the common markdown-reading default), and the
/// default theme's palette — everything a bare `run_tui` needs before Task 9's real
/// initialization (repo scan, config, theme override) replaces it.
impl Default for App {
    fn default() -> Self {
        App {
            repo: PathBuf::new(),
            files: Vec::new(),
            file_cursor: 0,
            file_scroll: 0,
            docs: [DocPane::default(), DocPane::default()],
            split: false,
            focus: Focus::default(),
            store: None,
            comments: CommentStore::default(),
            list_pct: DEFAULT_LIST_PCT,
            wrap: true,
            mode: Mode::default(),
            input: String::new(),
            caret: 0,
            should_quit: false,
            palette: theme::resolve(None).palette,
        }
    }
}

impl App {
    /// Inclusive, ordered block-index range of `pane`'s current selection: the cursor
    /// alone when there is no active selection, otherwise `cursor_block`/`sel_anchor` in
    /// ascending order.
    pub fn selection_range(&self, pane: usize) -> (usize, usize) {
        let doc_pane = &self.docs[pane];
        match doc_pane.sel_anchor {
            Some(anchor) => (anchor.min(doc_pane.cursor_block), anchor.max(doc_pane.cursor_block)),
            None => (doc_pane.cursor_block, doc_pane.cursor_block),
        }
    }

    /// The comment anchor `pane`'s current selection maps to: `(start, end, lines)` where
    /// `start`/`end` are 1-based source line numbers — the widest span the selected
    /// blocks cover — and `lines` is the verbatim source text of lines `start..=end`,
    /// with the trailing newline trimmed. `None` for a pane with no blocks to anchor to.
    pub fn anchor(&self, pane: usize) -> Option<(u32, u32, String)> {
        let doc_pane = &self.docs[pane];
        if doc_pane.doc.blocks.is_empty() {
            return None;
        }
        let (lo, hi) = self.selection_range(pane);
        let selected = &doc_pane.doc.blocks[lo..=hi];
        let start = selected.iter().map(|b| b.source_start).min().unwrap_or(1);
        let end = selected.iter().map(|b| b.source_end).max().unwrap_or(start);

        let start_byte = doc_pane.line_starts.get((start - 1) as usize).copied().unwrap_or(0);
        let end_byte = doc_pane
            .line_starts
            .get(end as usize)
            .copied()
            .unwrap_or(doc_pane.doc.source.len());
        let lines = doc_pane.doc.source[start_byte..end_byte].trim_end_matches('\n').to_string();
        Some((start, end, lines))
    }

    /// Move `pane`'s cursor by `delta` blocks (negative = up), clamped to the document's
    /// block range, and drop any active selection — plain navigation, not extension.
    pub fn move_cursor(&mut self, pane: usize, delta: isize) {
        let doc_pane = &mut self.docs[pane];
        doc_pane.cursor_block = clamp_cursor(doc_pane.cursor_block, delta, doc_pane.doc.blocks.len());
        doc_pane.sel_anchor = None;
    }

    /// Anchor a new selection at `pane`'s current cursor block.
    pub fn start_selection(&mut self, pane: usize) {
        let doc_pane = &mut self.docs[pane];
        doc_pane.sel_anchor = Some(doc_pane.cursor_block);
    }

    /// Move `pane`'s cursor by `delta` blocks, keeping (or starting, if none was active)
    /// the selection anchor at the block the cursor was on before the move.
    pub fn extend_selection(&mut self, pane: usize, delta: isize) {
        let doc_pane = &mut self.docs[pane];
        if doc_pane.sel_anchor.is_none() {
            doc_pane.sel_anchor = Some(doc_pane.cursor_block);
        }
        doc_pane.cursor_block = clamp_cursor(doc_pane.cursor_block, delta, doc_pane.doc.blocks.len());
    }

    /// Drop `pane`'s active selection, if any, leaving the cursor where it is.
    pub fn clear_selection(&mut self, pane: usize) {
        self.docs[pane].sel_anchor = None;
    }

    /// Anchor `pane`'s current selection and record it as a new comment: persisted to
    /// `store` (as `Author::User`) when one is open — a persistence failure is logged and
    /// otherwise ignored, never lost from the in-memory view — and always appended to
    /// `comments`. A no-op when the pane has no blocks to anchor to.
    pub fn add_comment(&mut self, pane: usize, text: String) {
        let Some((start, end, lines)) = self.anchor(pane) else { return };
        let file = self.docs[pane].path.clone().unwrap_or_default();
        let comment = Comment { file, start, end, lines, text };
        if let Some(store) = &self.store
            && let Err(e) = store.add(&comment, Author::User)
        {
            logln!("app: failed to persist comment: {}", e.0);
        }
        self.comments.add(comment);
    }

    /// Handle one key press, mutating state. `q` requests a quit.
    fn on_key(&mut self, code: KeyCode) {
        if code == KeyCode::Char('q') {
            self.should_quit = true;
        }
    }

    /// The doc pane index `focus` currently points at: `Focus::DocB` (Task 9's split mode)
    /// selects the second pane; the file list and `Focus::DocA` both fall back to the first.
    pub fn focus_pane(&self) -> usize {
        usize::from(self.focus == Focus::DocB)
    }

    /// For each block in `pane`'s document, the indices into `self.comments` whose card
    /// renders after it — the last block a comment's line range overlaps, so a comment
    /// spanning several blocks still shows exactly one card, anchored closest to its end.
    pub fn comment_cards(&self, pane: usize) -> Vec<Vec<usize>> {
        let doc_pane = &self.docs[pane];
        let mut cards = vec![Vec::new(); doc_pane.doc.blocks.len()];
        let Some(file) = doc_pane.path.as_deref() else { return cards };
        for (ci, sc) in self.comments.iter().enumerate() {
            if sc.comment.file != file {
                continue;
            }
            if let Some(last) =
                doc_pane.doc.blocks.iter().rposition(|b| comment_in_block(&sc.comment, b))
            {
                cards[last].push(ci);
            }
        }
        cards
    }

    /// The store index of a comment anchored to `pane`'s current cursor block, if any —
    /// names the edit/delete/jump actions in the footer.
    pub fn comment_under_cursor(&self, pane: usize) -> Option<usize> {
        let doc_pane = &self.docs[pane];
        let file = doc_pane.path.as_deref()?;
        let block = doc_pane.doc.blocks.get(doc_pane.cursor_block)?;
        self.comments.iter().position(|sc| sc.comment.file == file && comment_in_block(&sc.comment, block))
    }

    /// The `path:start-end` the composer is anchored to — the pending selection for a new
    /// comment, or the existing comment's location when editing. `None` outside
    /// `Mode::Composing`.
    pub fn pending_location(&self) -> Option<String> {
        match &self.mode {
            Mode::Composing { editing: Some(i) } => {
                self.comments.get(*i).map(|sc| sc.comment.location())
            }
            Mode::Composing { editing: None } => {
                let pane = self.focus_pane();
                let (start, end, _) = self.anchor(pane)?;
                let file = self.docs[pane].path.clone().unwrap_or_default();
                Some(Comment { file, start, end, lines: String::new(), text: String::new() }.location())
            }
            Mode::Browse | Mode::CommentsList { .. } => None,
        }
    }

    /// The actions the footer offers for the current context, most-relevant first, each
    /// tagged with its visual tier. Pure — a context → action mapping, unit-tested without a
    /// terminal. `ui::render_footer` maps each to a key+label, styles it by tier, and drops
    /// the least relevant (orientation first) to fit one line.
    pub fn footer_actions(&self) -> Vec<(FooterAction, Tier)> {
        use FooterAction as A;
        use Tier::{Normal, Orientation, Primary};

        match &self.mode {
            Mode::Composing { .. } => {
                return vec![(A::Save, Primary), (A::Cancel, Normal), (A::Newline, Normal)];
            }
            Mode::CommentsList { .. } => {
                return vec![
                    (A::Send, Primary),
                    (A::CloseList, Normal),
                    (A::ResolveComment, Normal),
                    (A::Copy, Normal),
                    (A::EditComment, Normal),
                    (A::DeleteComment, Normal),
                ];
            }
            Mode::Browse => {}
        }

        let mut out: Vec<(FooterAction, Tier)> = Vec::new();
        let mut cycle_is_primary = false;

        if self.focus == Focus::List {
            out.push((A::CycleFocus, Primary));
            cycle_is_primary = true;
        } else {
            let pane = self.focus_pane();
            let doc_pane = &self.docs[pane];
            if doc_pane.doc.blocks.is_empty() {
                out.push((A::CycleFocus, Primary));
                cycle_is_primary = true;
            } else if doc_pane.sel_anchor.is_some() {
                out.push((A::Comment, Primary));
                out.push((A::ClearSelection, Normal));
            } else if self.comment_under_cursor(pane).is_some() {
                out.push((A::EditComment, Primary));
                out.push((A::DeleteComment, Normal));
                out.push((A::JumpComment, Normal));
            } else {
                out.push((A::Comment, Primary));
                out.push((A::Select, Normal));
            }
        }

        if !self.comments.is_empty() {
            out.insert(1, (A::Send, Normal));
            out.push((A::List, Normal));
        }

        if !cycle_is_primary {
            out.push((A::CycleFocus, Orientation));
        }
        out.push((A::Split, Orientation));
        out.push((A::Quit, Orientation));
        out
    }
}

/// `cursor + delta`, clamped to `0..len` (or `0` for an empty document).
fn clamp_cursor(cursor: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let next = cursor as isize + delta;
    next.clamp(0, len as isize - 1) as usize
}

/// Whether comment `c`'s line range overlaps `block`'s source line range.
fn comment_in_block(c: &Comment, block: &Block) -> bool {
    c.start <= block.source_end && block.source_start <= c.end
}

/// Entry point: set up the terminal, run the event loop, and restore it on exit (even
/// on an error, so a panic-free failure never leaves the terminal in raw mode).
pub fn run_tui() -> Result<()> {
    let mut terminal = ratatui::init();
    let mut app = App::default();
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    result
}

/// Draw-then-wait loop: paint the current state, then block for the next key press.
fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    while !app.should_quit {
        terminal.draw(|frame| ui::render(frame, app))?;
        if let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.on_key(key.code);
        }
    }
    Ok(())
}

impl App {
    /// Build a single-pane `App` around `src`, rendered with the crate's default theme — a
    /// terminal-free `App` for `app`/`ui` tests, with no repo on disk. `store` is `None`;
    /// `add_comment` still updates `comments` in-memory. Not `#[cfg(test)]`-gated: the
    /// integration tests in `tests/` link this crate as a normal dependency (no `--cfg
    /// test`), so a helper only they use must still be compiled unconditionally to be
    /// reachable from there.
    pub fn for_test(src: &str) -> App {
        Self::for_test_with_path(src, None)
    }

    /// As [`App::for_test`], but with `docs[0].path` set — for tests that exercise
    /// `Comment::file`/`Comment::location` or the file-list pane.
    pub fn for_test_with_path(src: &str, path: Option<&str>) -> App {
        let palette = theme::resolve(None).palette;
        let highlighter = crate::highlight::Highlighter::new(theme::resolve(None).syntax);
        let doc = crate::markdown::render_document(src, &palette, &highlighter);
        let line_starts = crate::markdown::line_index(src);
        let pane = DocPane {
            path: path.map(str::to_string),
            doc,
            cursor_block: 0,
            sel_anchor: None,
            scroll: 0,
            line_starts,
        };
        App { docs: [pane, DocPane::default()], store: None, palette, ..Default::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::App;

    #[test]
    fn anchor_spans_selected_blocks_verbatim() {
        let src = "# Title\n\nfirst para\n\nsecond para\n";
        let mut app = App::for_test(src); // helper: single-doc app with rendered doc
        // blocks: heading(1), para(3), para(5)
        app.docs[0].cursor_block = 1; // "first para"
        app.start_selection(0);
        app.extend_selection(0, 1); // extend to "second para" (block 2)
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
}
