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
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::Rect;

use crate::comments::{Author, Status, Store};
use crate::config::{self, CommentSync, PluginConfig};
use crate::export::{Agent, Clipboard, ExportTarget, format_all};
use crate::file_list::{self, FileEntry};
use crate::highlight::Highlighter;
use crate::logln;
use crate::markdown::{self, Block, Document};
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

/// The file-list pane width's resize bounds — `[`/`]` clamp within these so a drag or
/// repeated keypress can't collapse either pane to nothing.
const MIN_LIST_PCT: u16 = 15;
const MAX_LIST_PCT: u16 = 60;

/// The full state of a viewer session.
// `split`/`wrap`/`should_quit`/`show_ignored`/`resume_list` are independent toggles, not a
// state machine in disguise, so the excessive-bools lint does not apply (mirrors reviewr).
#[allow(clippy::struct_excessive_bools)]
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
    /// Caret position within `input`, as a char index (not a byte offset) — matches the
    /// composer's char-wise editing (`ui::box_rows`/`caret_rowcol`).
    pub caret: usize,
    /// Set once the user has asked to quit.
    pub should_quit: bool,
    /// The active palette every renderer paints from.
    pub palette: Palette,
    /// The syntax highlighter for the active theme; rebuilt on a theme change so a doc
    /// loaded afterward (`load_pane`) renders with matching colors.
    highlighter: Highlighter,
    /// The last `Store::signature()` this session observed; `check_comment_store` (the poll
    /// tick) re-syncs `comments` from disk only when it has moved.
    comments_signature: u64,
    /// Whether gitignored markdown files are included in `files`, from the plugin config.
    show_ignored: bool,
    /// Set by `start_edit` when it opens the composer from the comments-list overlay, so
    /// `submit_comment`/`cancel_comment` return there instead of `Mode::Browse`.
    resume_list: bool,
    /// Which doc pane the file list's `j`/`k` loads a selection into — the last doc pane
    /// `cycle_focus` moved focus onto (`DocA` → `0`, `DocB` → `1`); reset to `0` whenever
    /// split mode is left, so a list selection never silently lands in a hidden pane.
    /// Defaults to `0` so single-pane browsing is unaffected.
    list_target_pane: usize,
    /// When a fresh comment persists to `store`: immediately (default), or held in memory
    /// until `App::export` flushes it (`config::CommentSync::OnSend`). From the plugin
    /// config's `comment_sync` key; agent-authored comments (CLI-written) are unaffected —
    /// this only gates the TUI's own `add_comment`.
    comment_sync: CommentSync,
    /// Whether a mouse drag is currently moving the pane divider — `on_mouse` sets this on a
    /// divider grab and clears it on button-up; `on_key` also clears it, so a keypress mid-drag
    /// (opening a modal) can't strand it `true`.
    resizing: bool,
}

/// `App`'s defaults for a session that has not yet loaded a repo: an empty file list, a
/// comfortable file-list width, wrap on (the common markdown-reading default), and the
/// default theme's palette — everything a bare `run_tui` needs before Task 9's real
/// initialization (repo scan, config, theme override) replaces it.
impl Default for App {
    fn default() -> Self {
        let theme = theme::resolve(None);
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
            palette: theme.palette,
            highlighter: Highlighter::new(theme.syntax),
            comments_signature: 0,
            show_ignored: false,
            resume_list: false,
            list_target_pane: 0,
            comment_sync: CommentSync::Immediate,
            resizing: false,
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
        let end_byte =
            doc_pane.line_starts.get(end as usize).copied().unwrap_or(doc_pane.doc.source.len());
        let lines = doc_pane.doc.source[start_byte..end_byte].trim_end_matches('\n').to_string();
        Some((start, end, lines))
    }

    /// Move `pane`'s cursor by `delta` blocks (negative = up), clamped to the document's
    /// block range, and drop any active selection — plain navigation, not extension.
    pub fn move_cursor(&mut self, pane: usize, delta: isize) {
        let doc_pane = &mut self.docs[pane];
        doc_pane.cursor_block =
            clamp_cursor(doc_pane.cursor_block, delta, doc_pane.doc.blocks.len());
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
        doc_pane.cursor_block =
            clamp_cursor(doc_pane.cursor_block, delta, doc_pane.doc.blocks.len());
    }

    /// Drop `pane`'s active selection, if any, leaving the cursor where it is.
    pub fn clear_selection(&mut self, pane: usize) {
        self.docs[pane].sel_anchor = None;
    }

    /// Anchor `pane`'s current selection and record it as a new comment: persisted to
    /// `store` (as `Author::User`) immediately when `comment_sync` is `Immediate` (the
    /// default) and one is open — a persistence failure is logged and otherwise ignored,
    /// never lost from the in-memory view. Under `comment_sync: on-send` it stays
    /// memory-only until `App::export` flushes it (`persist_open_user_comments`). Always
    /// appended to `comments`. A no-op when the pane has no blocks to anchor to.
    pub fn add_comment(&mut self, pane: usize, text: String) {
        let Some((start, end, lines)) = self.anchor(pane) else { return };
        let file = self.docs[pane].path.clone().unwrap_or_default();
        let comment = Comment { file, start, end, lines, text };
        if self.comment_sync == CommentSync::Immediate
            && let Some(store) = &self.store
            && let Err(e) = store.add(&comment, Author::User)
        {
            logln!("app: failed to persist comment: {}", e.0);
        }
        self.comments.add(comment);
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
        self.comments
            .iter()
            .position(|sc| sc.comment.file == file && comment_in_block(&sc.comment, block))
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
                Some(
                    Comment { file, start, end, lines: String::new(), text: String::new() }
                        .location(),
                )
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

/// Task 9: focus/layout, file loading, the comment composer's caret editor, comment
/// actions (edit/delete/resolve/jump/list), export, and the poll-tick refresh — everything
/// `on_key` and `run_tui` drive. Still terminal-free: `on_key` takes a plain `KeyEvent` and
/// the frame `Rect` (for composer-width and scroll-reveal geometry only), never a `Frame` or
/// a `crossterm::Event`.
impl App {
    /// Build the initial session for `run_tui`: the cwd as the repo root, its comment store
    /// (`None` when it is not a git worktree), the configured theme, a fresh markdown scan,
    /// and the newest file loaded into the primary pane (no path argument — see
    /// `docs/superpowers/specs/2026-08-13-herdr-glowr-design.md`, "no argument ... the
    /// newest file is selected on start").
    fn new(cfg: &PluginConfig) -> App {
        let repo = std::env::current_dir().unwrap_or_default();
        let theme = theme::resolve(Some(cfg.theme()));
        let highlighter = Highlighter::new(theme.syntax);
        let store = Store::open(&repo).ok();
        let mut comments = CommentStore::new();
        if let Some(store) = &store {
            comments.replace(store.load());
        }
        let comments_signature = store.as_ref().map_or(0, Store::signature);
        let files = file_list::markdown_files(&repo, cfg.show_ignored());
        let mut app = App {
            repo,
            files,
            file_cursor: 0,
            file_scroll: 0,
            docs: [DocPane::default(), DocPane::default()],
            split: false,
            focus: Focus::List,
            store,
            comments,
            list_pct: DEFAULT_LIST_PCT,
            wrap: true,
            mode: Mode::default(),
            input: String::new(),
            caret: 0,
            should_quit: false,
            palette: theme.palette,
            highlighter,
            comments_signature,
            show_ignored: cfg.show_ignored(),
            resume_list: false,
            list_target_pane: 0,
            comment_sync: cfg.comment_sync(),
            resizing: false,
        };
        if let Some(path) = app.files.first().map(|f| f.path.clone()) {
            app.load_pane(0, &path);
        }
        app
    }

    // ---- focus & layout --------------------------------------------------------------

    /// `Tab`: `List` → `DocA` → `DocB` (only while `split`) → `List`. Moving onto a doc pane
    /// remembers it as `list_target_pane` — where the file list's `j`/`k` loads next.
    pub fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::List => Focus::DocA,
            Focus::DocA if self.split => Focus::DocB,
            Focus::DocA | Focus::DocB => Focus::List,
        };
        match self.focus {
            Focus::DocA => self.list_target_pane = 0,
            Focus::DocB => self.list_target_pane = 1,
            Focus::List => {}
        }
    }

    /// Flip split-doc mode. Leaving split while `DocB` is focused falls back to `DocA`, and
    /// resets `list_target_pane` to `0` — `DocB` is never a valid focus, or file-list load,
    /// target while its pane is hidden.
    pub fn toggle_split(&mut self) {
        self.split = !self.split;
        if !self.split {
            if self.focus == Focus::DocB {
                self.focus = Focus::DocA;
            }
            self.list_target_pane = 0;
        }
    }

    pub fn toggle_wrap(&mut self) {
        self.wrap = !self.wrap;
    }

    /// Resize the file-list pane by `delta` percentage points, clamped to
    /// `MIN_LIST_PCT..=MAX_LIST_PCT`.
    pub fn resize_list(&mut self, delta: i16) {
        let next = (self.list_pct as i16 + delta).clamp(MIN_LIST_PCT as i16, MAX_LIST_PCT as i16);
        self.list_pct = next as u16;
    }

    // ---- file list ---------------------------------------------------------------------

    /// Move the file-list cursor by `delta`, load the file it lands on into the focused doc
    /// pane, and scroll the list to keep it visible. A no-op with no files.
    pub fn move_file_cursor(&mut self, delta: isize, area: Rect) {
        if self.files.is_empty() {
            return;
        }
        self.select_file(clamp_cursor(self.file_cursor, delta, self.files.len()), area);
    }

    /// Select file `idx`, load it into `list_target_pane` (the doc pane `cycle_focus` last
    /// moved focus onto — `DocA` unless the reviewer `Tab`bed onto `DocB` first), and reveal
    /// the cursor in the file list. This is how a split-mode reviewer gets a *different* doc
    /// into each pane: `Tab` to `DocB`, `Tab` back to the list, then `j`/`k` a file — it loads
    /// into `DocB`, not `DocA`.
    fn select_file(&mut self, idx: usize, area: Rect) {
        let Some(entry) = self.files.get(idx) else { return };
        self.file_cursor = idx;
        let path = entry.path.clone();
        self.load_pane(self.list_target_pane, &path);
        self.reveal_file_cursor(area);
    }

    /// Read `path` (repo-relative) from disk, lossily as UTF-8 — empty when unreadable —
    /// render it, and replace `docs[pane]` with the freshly loaded pane.
    fn load_pane(&mut self, pane: usize, path: &str) {
        let content = std::fs::read(self.repo.join(path))
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        let doc = markdown::render_document(&content, &self.palette, &self.highlighter);
        let line_starts = markdown::line_index(&content);
        self.docs[pane] = DocPane {
            path: Some(path.to_string()),
            doc,
            cursor_block: 0,
            sel_anchor: None,
            scroll: 0,
            line_starts,
        };
    }

    /// Scroll the file list so `file_cursor` is on screen — the minimal nudge.
    fn reveal_file_cursor(&mut self, area: Rect) {
        if self.files.is_empty() {
            self.file_scroll = 0;
            return;
        }
        let cursor = self.file_cursor.min(self.files.len() - 1);
        let heights = vec![1usize; self.files.len()];
        let viewport = ui::file_viewport_height(area, self.list_pct);
        self.file_scroll = keep_in_view(cursor, self.file_scroll, &heights, viewport);
    }

    /// Scroll `pane`'s doc so `cursor_block`'s row is on screen. A no-op while composing on
    /// this pane — `doc_row_heights` doesn't model the composer's spliced layout (`ui.rs`).
    fn reveal_pane(&mut self, pane: usize, area: Rect) {
        if matches!(self.mode, Mode::Composing { .. }) && self.focus_pane() == pane {
            return;
        }
        if self.docs[pane].doc.blocks.is_empty() {
            return;
        }
        let heights = ui::doc_row_heights(self, area, pane);
        if heights.is_empty() {
            return;
        }
        let width = ui::doc_inner_width(area, self.list_pct, self.split, pane);
        let rows = markdown::layout_rows(&self.docs[pane].doc, width, self.wrap);
        let cursor_block = self.docs[pane].cursor_block;
        let row_ix = rows.iter().position(|r| r.block == cursor_block).unwrap_or(0);
        let viewport = ui::doc_viewport_height(area, self.list_pct, self.split, pane);
        self.docs[pane].scroll = keep_in_view(row_ix, self.docs[pane].scroll, &heights, viewport);
    }

    // ---- mouse ---------------------------------------------------------------------------

    /// Handle one mouse event. `area` is the terminal frame's rect, exactly as `on_key` uses
    /// it for geometry. Terminal-free and synchronously testable, mirroring `on_key`. The
    /// composer and the comments-list overlay capture the screen and are keyboard-driven, so
    /// the mouse is inert under them — a click can't drive the panes painted underneath
    /// (mirrors reviewr).
    pub fn on_mouse(&mut self, m: MouseEvent, area: Rect) {
        if self.composing() || matches!(self.mode, Mode::CommentsList { .. }) {
            return;
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => self.mouse_down(m.column, m.row, area),
            MouseEventKind::Drag(MouseButton::Left) => self.mouse_drag(m.column, m.row, area),
            MouseEventKind::Up(MouseButton::Left) => self.resizing = false,
            MouseEventKind::ScrollDown => self.wheel(m.column, m.row, area, 3),
            MouseEventKind::ScrollUp => self.wheel(m.column, m.row, area, -3),
            _ => {}
        }
    }

    /// A left click: the divider (start a resize drag), the `[ Send (N) ]` button, a file
    /// row, or a doc pane's block — checked in that order (the divider's grab zone can
    /// straddle the doc/file border; the header row never overlaps the body, so order
    /// between it and the rest doesn't matter).
    fn mouse_down(&mut self, col: u16, row: u16, area: Rect) {
        if ui::hit_divider(area, self.list_pct, col, row) {
            self.resizing = true;
            return;
        }
        if ui::hit_send_button(area, self.list_pct, self, col, row) {
            self.export(&Agent);
            return;
        }
        if let Some(idx) =
            ui::hit_file(area, self.list_pct, col, row, self.files.len(), self.file_scroll)
        {
            self.focus = Focus::List;
            self.select_file(idx, area);
            return;
        }
        for &pane in self.visible_panes() {
            let heights = ui::doc_row_heights(self, area, pane);
            let Some(row_ix) = ui::hit_doc(
                area,
                self.list_pct,
                self.split,
                pane,
                col,
                row,
                &heights,
                self.docs[pane].scroll,
            ) else {
                continue;
            };
            let width = ui::doc_inner_width(area, self.list_pct, self.split, pane);
            let rows = markdown::layout_rows(&self.docs[pane].doc, width, self.wrap);
            if let Some(r) = rows.get(row_ix) {
                self.docs[pane].cursor_block = r.block;
            }
            self.docs[pane].sel_anchor = None;
            self.focus = if pane == 0 { Focus::DocA } else { Focus::DocB };
            self.list_target_pane = pane;
            return;
        }
    }

    /// A left-button drag: resize the divider while `resizing`, else extend the block
    /// selection under the pointer in whichever doc pane it is over (anchoring the selection
    /// on the first drag tick, like `v` does).
    fn mouse_drag(&mut self, col: u16, row: u16, area: Rect) {
        if self.resizing {
            let body = ui::body_rect(area);
            self.drag_divider(body.width, col.saturating_sub(body.x));
            return;
        }
        for &pane in self.visible_panes() {
            let heights = ui::doc_row_heights(self, area, pane);
            let Some(row_ix) = ui::hit_doc(
                area,
                self.list_pct,
                self.split,
                pane,
                col,
                row,
                &heights,
                self.docs[pane].scroll,
            ) else {
                continue;
            };
            let width = ui::doc_inner_width(area, self.list_pct, self.split, pane);
            let rows = markdown::layout_rows(&self.docs[pane].doc, width, self.wrap);
            if let Some(r) = rows.get(row_ix) {
                self.drag_select_to(pane, r.block);
            }
            return;
        }
    }

    /// Extend `pane`'s selection to `block`, anchoring at the current cursor on the first
    /// drag tick — the mouse equivalent of `v` then `j`/`k`.
    fn drag_select_to(&mut self, pane: usize, block: usize) {
        if block >= self.docs[pane].doc.blocks.len() {
            return;
        }
        if self.docs[pane].sel_anchor.is_none() {
            self.docs[pane].sel_anchor = Some(self.docs[pane].cursor_block);
        }
        self.docs[pane].cursor_block = block;
        self.focus = if pane == 0 { Focus::DocA } else { Focus::DocB };
        self.list_target_pane = pane;
    }

    /// Set `list_pct` so the divider sits at body column `x` (a mouse drag). `x` is measured
    /// from the body's left edge; the file list spans from there to the right edge.
    fn drag_divider(&mut self, body_width: u16, x: u16) {
        if body_width == 0 {
            return;
        }
        let list_cols = body_width.saturating_sub(x.min(body_width));
        let pct = (u32::from(list_cols) * 100 / u32::from(body_width)) as u16;
        self.list_pct = pct.clamp(MIN_LIST_PCT, MAX_LIST_PCT);
    }

    /// Route a wheel tick to whichever pane the pointer is over: the file list, or a doc
    /// pane.
    fn wheel(&mut self, col: u16, row: u16, area: Rect, delta: isize) {
        if ui::in_files_pane(area, self.list_pct, col, row) {
            self.wheel_files(delta, area);
            return;
        }
        for &pane in self.visible_panes() {
            if ui::in_doc_pane(area, self.list_pct, self.split, pane, col, row) {
                self.wheel_doc(pane, delta, area);
                return;
            }
        }
    }

    /// Wheel-scroll the file list's viewport, leaving the selection untouched. Bounded so it
    /// never shows a blank tail.
    fn wheel_files(&mut self, delta: isize, area: Rect) {
        if self.files.is_empty() {
            return;
        }
        let viewport = ui::file_viewport_height(area, self.list_pct);
        let next = offset_by(self.file_scroll, delta);
        self.file_scroll = next.min(self.files.len().saturating_sub(viewport));
    }

    /// Wheel-scroll `pane`'s doc viewport, leaving `cursor_block` put — so wheeling to read
    /// context never moves what a comment would anchor to. Bounded by the summed row heights
    /// so it never shows a blank tail.
    fn wheel_doc(&mut self, pane: usize, delta: isize, area: Rect) {
        if self.docs[pane].doc.blocks.is_empty() {
            return;
        }
        let heights = ui::doc_row_heights(self, area, pane);
        if heights.is_empty() {
            return;
        }
        let viewport = ui::doc_viewport_height(area, self.list_pct, self.split, pane);
        let total: usize = heights.iter().sum();
        let next = offset_by(self.docs[pane].scroll, delta);
        self.docs[pane].scroll = next.min(total.saturating_sub(viewport));
    }

    /// The doc pane indices a click/drag/wheel should consider — both when split, else only
    /// the sole visible pane.
    fn visible_panes(&self) -> &'static [usize] {
        const BOTH: [usize; 2] = [0, 1];
        if self.split { &BOTH } else { &BOTH[..1] }
    }

    // ---- comment composer: caret editor -------------------------------------------------

    pub fn composing(&self) -> bool {
        matches!(self.mode, Mode::Composing { .. })
    }

    /// Open the composer for a brand-new comment anchored to the focused pane's current
    /// selection (or lone cursor block). A no-op outside a doc pane, or on an empty doc.
    fn start_comment(&mut self) {
        if !matches!(self.focus, Focus::DocA | Focus::DocB) {
            return;
        }
        let pane = self.focus_pane();
        if self.docs[pane].doc.blocks.is_empty() {
            return;
        }
        self.input.clear();
        self.caret = 0;
        self.resume_list = false;
        self.mode = Mode::Composing { editing: None };
    }

    /// Open the composer over the targeted comment's existing text — the comment under the
    /// doc cursor in a doc pane, or the highlighted row in the comments-list overlay. Submit
    /// or cancel returns to the comments list when it was opened from there.
    fn start_edit(&mut self) {
        let Some(idx) = self.target_comment() else { return };
        let Some(sc) = self.comments.get(idx) else { return };
        self.resume_list = matches!(self.mode, Mode::CommentsList { .. });
        self.input.clone_from(&sc.comment.text);
        self.caret = self.input.chars().count();
        self.mode = Mode::Composing { editing: Some(idx) };
    }

    /// Run a char-wise edit on `input`: collect it into a `Vec<char>` with the caret as an
    /// in-range index, hand both to `f`, then reassemble and re-clamp the caret. A no-op
    /// outside `Mode::Composing`.
    fn edit_input(&mut self, f: impl FnOnce(&mut Vec<char>, &mut usize)) {
        if !self.composing() {
            return;
        }
        let mut v: Vec<char> = self.input.chars().collect();
        let mut caret = self.caret.min(v.len());
        f(&mut v, &mut caret);
        self.caret = caret.min(v.len());
        self.input = v.into_iter().collect();
    }

    /// Move the caret with a function of the current `Vec<char>` view. A no-op outside
    /// `Mode::Composing`.
    fn move_caret(&mut self, f: impl FnOnce(&[char], usize) -> usize) {
        if self.composing() {
            let v: Vec<char> = self.input.chars().collect();
            self.caret = f(&v, self.caret.min(v.len()));
        }
    }

    pub fn input_push(&mut self, ch: char) {
        self.edit_input(|v, caret| {
            v.insert(*caret, ch);
            *caret += 1;
        });
    }

    pub fn input_backspace(&mut self) {
        self.edit_input(|v, caret| {
            if *caret > 0 {
                v.remove(*caret - 1);
                *caret -= 1;
            }
        });
    }

    pub fn input_delete_forward(&mut self) {
        self.edit_input(|v, caret| {
            if *caret < v.len() {
                v.remove(*caret);
            }
        });
    }

    /// Delete the word before the caret (`Ctrl+W`): trailing whitespace, then the
    /// non-whitespace run before it.
    pub fn input_delete_word(&mut self) {
        self.edit_input(|v, caret| {
            let start = word_start(v, *caret);
            v.drain(start..*caret);
            *caret = start;
        });
    }

    /// Delete from the start of the logical line to the caret (`Ctrl+U`).
    pub fn input_kill_to_start(&mut self) {
        self.edit_input(|v, caret| {
            let start = line_start(v, *caret);
            v.drain(start..*caret);
            *caret = start;
        });
    }

    /// Delete from the caret to the end of the logical line (`Ctrl+K`).
    pub fn input_kill_to_end(&mut self) {
        self.edit_input(|v, caret| {
            let end = line_end(v, *caret);
            v.drain(*caret..end);
        });
    }

    pub fn caret_left(&mut self) {
        self.move_caret(|_, caret| caret.saturating_sub(1));
    }

    pub fn caret_right(&mut self) {
        self.move_caret(|v, caret| (caret + 1).min(v.len()));
    }

    pub fn caret_home(&mut self) {
        self.move_caret(line_start);
    }

    pub fn caret_end(&mut self) {
        self.move_caret(line_end);
    }

    pub fn caret_word_left(&mut self) {
        self.move_caret(word_start);
    }

    pub fn caret_word_right(&mut self) {
        self.move_caret(word_end);
    }

    fn cancel_comment(&mut self) {
        self.leave_compose();
    }

    /// Save the in-progress comment — editing the targeted one, or adding a new one anchored
    /// to the focused pane's selection — then leave compose mode. Blank text cancels instead.
    fn submit_comment(&mut self) {
        let Mode::Composing { editing } = self.mode else { return };
        let text = self.input.trim().to_string();
        if text.is_empty() {
            self.cancel_comment();
            return;
        }
        if let Some(idx) = editing {
            if self.comments.edit(idx, text) {
                self.persist_comment(idx);
            }
        } else {
            let pane = self.focus_pane();
            self.add_comment(pane, text);
        }
        let pane = self.focus_pane();
        self.docs[pane].sel_anchor = None;
        self.leave_compose();
    }

    /// Leave compose mode: clear the input, and return to the comments-list overlay when
    /// `start_edit` opened the composer from it (and any comments remain), else `Browse`.
    fn leave_compose(&mut self) {
        let editing = if let Mode::Composing { editing } = self.mode { editing } else { None };
        self.input.clear();
        self.caret = 0;
        let resume = std::mem::take(&mut self.resume_list);
        if resume && !self.comments.is_empty() {
            let cursor = editing.unwrap_or(0).min(self.comments.len() - 1);
            self.mode = Mode::CommentsList { cursor };
        } else {
            self.mode = Mode::Browse;
        }
    }

    /// Persist the comment at `index` to `store`, when one is open; updates
    /// `comments_signature` so the next poll tick doesn't see its own write as external.
    fn persist_comment(&mut self, index: usize) {
        let Some(store) = &self.store else { return };
        let Some(sc) = self.comments.get(index) else { return };
        match store.put(sc) {
            Ok(()) => self.comments_signature = store.signature(),
            Err(e) => logln!("app: failed to persist comment: {}", e.0),
        }
    }

    // ---- comment actions: edit/delete/resolve/jump/list ---------------------------------

    /// The comment index to act on: the comments-list overlay's highlighted row, or the
    /// comment under the focused doc pane's cursor.
    fn target_comment(&self) -> Option<usize> {
        match self.mode {
            Mode::CommentsList { cursor } => (cursor < self.comments.len()).then_some(cursor),
            _ => self.comment_under_cursor(self.focus_pane()),
        }
    }

    /// Delete the targeted comment (`d`) — from memory and, once persisted, from disk too.
    pub fn delete_comment(&mut self) {
        let Some(idx) = self.target_comment() else { return };
        let Some(sc) = self.comments.get(idx) else { return };
        let id = sc.id.clone();
        self.comments.take(idx);
        if let Some(store) = &self.store {
            match store.remove(&id) {
                Ok(_) => self.comments_signature = store.signature(),
                Err(e) => logln!("app: failed to remove comment: {}", e.0),
            }
        }
        if let Mode::CommentsList { cursor } = &mut self.mode {
            *cursor = (*cursor).min(self.comments.len().saturating_sub(1));
        }
        // Don't strand the reviewer in an empty overlay.
        if self.comments.is_empty() {
            self.close_list();
        }
    }

    /// Flip the targeted comment's open/resolved status (`x`), in memory and on disk.
    pub fn resolve_selected_comment(&mut self) {
        let Some(idx) = self.target_comment() else { return };
        let Some(sc) = self.comments.get(idx) else { return };
        let next = match sc.status {
            Status::Open => Status::Resolved,
            Status::Resolved => Status::Open,
        };
        let id = sc.id.clone();
        self.comments.set_status(idx, next);
        if let Some(store) = &self.store {
            match store.set_status(&id, next) {
                Ok(_) => self.comments_signature = store.signature(),
                Err(e) => logln!("app: failed to persist resolve: {}", e.0),
            }
        }
    }

    /// Move the focused pane's cursor to the next (`dir >= 0`) or previous commented block
    /// (`n`/`N`). A no-op outside a doc pane, or with nothing commented.
    pub fn jump_comment(&mut self, dir: isize) {
        if !matches!(self.focus, Focus::DocA | Focus::DocB) {
            return;
        }
        let pane = self.focus_pane();
        let mut commented: Vec<usize> = self
            .comment_cards(pane)
            .iter()
            .enumerate()
            .filter(|(_, cards)| !cards.is_empty())
            .map(|(block, _)| block)
            .collect();
        if commented.is_empty() {
            return;
        }
        commented.sort_unstable();
        let cur = self.docs[pane].cursor_block;
        let target = if dir >= 0 {
            commented.iter().copied().find(|&b| b > cur).or_else(|| commented.first().copied())
        } else {
            commented.iter().rev().copied().find(|&b| b < cur).or_else(|| commented.last().copied())
        };
        if let Some(block) = target {
            self.docs[pane].cursor_block = block;
            self.docs[pane].sel_anchor = None;
        }
    }

    /// Open the comments-list overlay (`l`). A no-op with no comments.
    pub fn open_list(&mut self) {
        if !self.comments.is_empty() {
            self.mode = Mode::CommentsList { cursor: 0 };
        }
    }

    pub fn close_list(&mut self) {
        if matches!(self.mode, Mode::CommentsList { .. }) {
            self.mode = Mode::Browse;
        }
    }

    /// Move the comments-list overlay's cursor by `delta`. A no-op outside the overlay.
    pub fn list_move(&mut self, delta: isize) {
        if let Mode::CommentsList { cursor } = &mut self.mode
            && !self.comments.is_empty()
        {
            *cursor = clamp_cursor(*cursor, delta, self.comments.len());
        }
    }

    // ---- export --------------------------------------------------------------------------

    /// Send (or copy) every open, user-authored comment: persist them all first (so a failed
    /// export never loses one that only lived in memory), then hand `format_all`'s text to
    /// `target`. Never consumes or resolves a comment (`G3`). A no-op with nothing to send.
    pub fn export(&mut self, target: &dyn ExportTarget) {
        self.persist_open_user_comments();
        let refs: Vec<&Comment> =
            self.comments.open_user_comments().into_iter().map(|sc| &sc.comment).collect();
        if refs.is_empty() {
            logln!("app: export skipped, no open user comments");
            return;
        }
        let text = format_all(&refs);
        let n = refs.len();
        match target.export(&text) {
            Ok(()) => logln!("app: exported {n} comment(s) to {}", target.label()),
            Err(e) => logln!("app: export to {} failed: {e:#}", target.label()),
        }
    }

    fn persist_open_user_comments(&mut self) {
        let Some(store) = &self.store else { return };
        for sc in self.comments.open_user_comments() {
            if let Err(e) = store.put(sc) {
                logln!("app: failed to persist comment {}: {}", sc.id, e.0);
            }
        }
        self.comments_signature = store.signature();
    }

    // ---- poll-tick refresh -----------------------------------------------------------

    /// Re-sync `comments` from `store` when its change signature has moved since the last
    /// check — an agent's CLI write shows up on the next poll tick without the reviewer
    /// doing anything. A no-op with no disk store.
    fn check_comment_store(&mut self) {
        let Some(store) = &self.store else { return };
        let sig = store.signature();
        if sig != self.comments_signature {
            self.comments.replace(store.load());
            self.comments_signature = sig;
            if let Mode::CommentsList { cursor } = &mut self.mode {
                *cursor = (*cursor).min(self.comments.len().saturating_sub(1));
            }
        }
    }

    /// Re-scan `repo` for markdown files, keeping the file cursor on the same path when it
    /// still exists in the refreshed list.
    fn refresh_files(&mut self) {
        let current = self.files.get(self.file_cursor).map(|f| f.path.clone());
        self.files = file_list::markdown_files(&self.repo, self.show_ignored);
        self.file_cursor = current
            .and_then(|p| self.files.iter().position(|f| f.path == p))
            .unwrap_or(0)
            .min(self.files.len().saturating_sub(1));
    }

    // ---- input dispatch --------------------------------------------------------------

    /// Handle one key press. `area` is the terminal frame's rect — used only for
    /// composer-width (`↑`/`↓` caret motion) and scroll-reveal geometry, never painted.
    /// Terminal-free and synchronously testable: no `Frame`, no `crossterm::Event`.
    pub fn on_key(&mut self, key: KeyEvent, area: Rect) {
        use KeyCode::{Backspace, Char, Delete, Down, End, Enter, Esc, Home, Left, Right, Tab, Up};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // A keypress ends any in-progress divider drag, so opening a modal mid-drag (which
        // makes the mouse handler ignore the releasing `Up`) can't strand `resizing` true.
        self.resizing = false;

        if self.composing() {
            let alt = key.modifiers.contains(KeyModifiers::ALT);
            let alt_or_shift = key.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::SHIFT);
            let word = alt || ctrl;
            let pane = self.focus_pane();
            let cw = ui::composer_content_width(ui::doc_inner_width(
                area,
                self.list_pct,
                self.split,
                pane,
            ));
            match key.code {
                Esc => self.cancel_comment(),
                Enter if alt_or_shift => self.input_push('\n'),
                Enter => self.submit_comment(),
                Char('j') if ctrl => self.input_push('\n'),
                Char('w') if ctrl => self.input_delete_word(),
                Char('a') if ctrl => self.caret_home(),
                Char('e') if ctrl => self.caret_end(),
                Char('u') if ctrl => self.input_kill_to_start(),
                Char('k') if ctrl => self.input_kill_to_end(),
                Char('b') if alt => self.caret_word_left(),
                Char('f') if alt => self.caret_word_right(),
                Left if word => self.caret_word_left(),
                Right if word => self.caret_word_right(),
                Left => self.caret_left(),
                Right => self.caret_right(),
                Up => self.caret = ui::caret_vertical(&self.input, self.caret, cw, false),
                Down => self.caret = ui::caret_vertical(&self.input, self.caret, cw, true),
                Home => self.caret_home(),
                End => self.caret_end(),
                Delete => self.input_delete_forward(),
                Backspace => self.input_backspace(),
                Char(c) if !ctrl => self.input_push(c),
                _ => {}
            }
            return;
        }

        if matches!(self.mode, Mode::CommentsList { .. }) {
            match key.code {
                Esc | Char('l' | 'q') => self.close_list(),
                Char('j') | Down => self.list_move(1),
                Char('k') | Up => self.list_move(-1),
                Char('s') => self.export(&Agent),
                Char('y') => self.export(&Clipboard),
                Char('e') => self.start_edit(),
                Char('d') => self.delete_comment(),
                Char('x') => self.resolve_selected_comment(),
                _ => {}
            }
            return;
        }

        // `Mode::Browse` from here: keys available regardless of which pane has focus.
        match key.code {
            Char('q') => {
                self.should_quit = true;
                return;
            }
            Tab => {
                self.cycle_focus();
                return;
            }
            Char('`') => {
                self.toggle_split();
                return;
            }
            Char('w') => {
                self.toggle_wrap();
                return;
            }
            Char(']') => {
                self.resize_list(4);
                return;
            }
            Char('[') => {
                self.resize_list(-4);
                return;
            }
            Char('l') => {
                self.open_list();
                return;
            }
            Char('s') => {
                self.export(&Agent);
                return;
            }
            Char('y') => {
                self.export(&Clipboard);
                return;
            }
            _ => {}
        }

        if self.focus == Focus::List {
            match key.code {
                Char('j') | Down => self.move_file_cursor(1, area),
                Char('k') | Up => self.move_file_cursor(-1, area),
                _ => {}
            }
            return;
        }

        // A doc pane has focus.
        let pane = self.focus_pane();
        match key.code {
            Char('j') | Down => {
                if self.docs[pane].sel_anchor.is_some() {
                    self.extend_selection(pane, 1);
                } else {
                    self.move_cursor(pane, 1);
                }
                self.reveal_pane(pane, area);
            }
            Char('k') | Up => {
                if self.docs[pane].sel_anchor.is_some() {
                    self.extend_selection(pane, -1);
                } else {
                    self.move_cursor(pane, -1);
                }
                self.reveal_pane(pane, area);
            }
            Char('v') => self.start_selection(pane),
            Esc => self.clear_selection(pane),
            Char('c') => self.start_comment(),
            Char('e') => self.start_edit(),
            Char('d') => self.delete_comment(),
            Char('n') => {
                self.jump_comment(1);
                self.reveal_pane(pane, area);
            }
            Char('N') => {
                self.jump_comment(-1);
                self.reveal_pane(pane, area);
            }
            _ => {}
        }
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

/// Move `scroll` the minimal amount so the row at `cursor` fits within a `viewport`-tall
/// window, given each row's display `heights`. Scrolls up when the cursor is above the top,
/// advances the top until the cursor's row fits, then pulls back so the bottom isn't left
/// blank — the shared "keep the cursor visible" rule for both the doc pane and the file list
/// (which passes all-height-1 rows, where this degenerates to plain row arithmetic).
fn keep_in_view(cursor: usize, scroll: usize, heights: &[usize], viewport: usize) -> usize {
    if viewport == 0 || heights.is_empty() {
        return 0;
    }
    let cursor = cursor.min(heights.len() - 1);
    let mut top = scroll.min(cursor);
    while top < cursor && heights[top..=cursor].iter().sum::<usize>() > viewport {
        top += 1;
    }
    while top > 0 && heights[top - 1..].iter().sum::<usize>() <= viewport {
        top -= 1;
    }
    top
}

/// Move a scroll offset by `delta` rows, saturating at 0 — the wheel's raw motion, before the
/// caller clamps the upper bound once the viewport is known (`wheel_files`/`wheel_doc`).
fn offset_by(scroll: usize, delta: isize) -> usize {
    if delta >= 0 {
        scroll.saturating_add(delta.unsigned_abs())
    } else {
        scroll.saturating_sub(delta.unsigned_abs())
    }
}

/// The start of the logical line (after the previous `\n`, or 0) containing char `caret`.
fn line_start(v: &[char], caret: usize) -> usize {
    v[..caret].iter().rposition(|&c| c == '\n').map_or(0, |p| p + 1)
}

/// The end of the logical line (the next `\n`, or the end) containing char `caret`.
fn line_end(v: &[char], caret: usize) -> usize {
    v[caret..].iter().position(|&c| c == '\n').map_or(v.len(), |p| caret + p)
}

/// The start of the word before `caret`: skip trailing whitespace, then the word run.
fn word_start(v: &[char], caret: usize) -> usize {
    let mut i = caret;
    while i > 0 && v[i - 1].is_whitespace() {
        i -= 1;
    }
    while i > 0 && !v[i - 1].is_whitespace() {
        i -= 1;
    }
    i
}

/// The end of the word after `caret`: skip leading whitespace, then the word run.
fn word_end(v: &[char], caret: usize) -> usize {
    let mut i = caret;
    while i < v.len() && v[i].is_whitespace() {
        i += 1;
    }
    while i < v.len() && !v[i].is_whitespace() {
        i += 1;
    }
    i
}

/// How often the poll tick re-checks the on-disk comment store and rescans the file list —
/// picks up an agent's CLI writes and new files without the reviewer doing anything.
const POLL: Duration = Duration::from_millis(1000);

/// Entry point: resolve the plugin config, build the session, run the event loop, and
/// restore the terminal on exit (even on an error, so a panic-free failure never leaves the
/// terminal in raw mode).
pub fn run_tui() -> Result<()> {
    let cfg = config::plugin_config().unwrap_or_else(|e| {
        logln!("run_tui: invalid plugin config, using defaults: {e}");
        PluginConfig::default()
    });
    let mut app = App::new(&cfg);
    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let result = event_loop(&mut terminal, &mut app);
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

/// Draw, then wait up to the poll deadline for a keypress or mouse event; refresh on each
/// tick.
fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    let mut last_poll = Instant::now();
    while !app.should_quit {
        terminal.draw(|frame| ui::render(frame, app))?;
        let size = terminal.size()?;
        let area = Rect::new(0, 0, size.width, size.height);
        let timeout = POLL.saturating_sub(last_poll.elapsed());
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.on_key(key, area),
                Event::Mouse(m) => app.on_mouse(m, area),
                _ => {}
            }
        }
        if last_poll.elapsed() >= POLL {
            app.check_comment_store();
            app.refresh_files();
            last_poll = Instant::now();
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
        App {
            docs: [pane, DocPane::default()],
            store: None,
            palette,
            highlighter,
            ..Default::default()
        }
    }

    /// Focus doc pane `pane` (`0` = `DocA`, `1` = `DocB`) directly, bypassing `cycle_focus` —
    /// for `on_key` tests that need to start already inside a doc pane.
    pub fn focus_doc_for_test(&mut self, pane: usize) {
        self.focus = if pane == 0 { Focus::DocA } else { Focus::DocB };
    }

    /// Load `src` into `docs[pane]` directly, no disk I/O — for tests that need a second
    /// pane populated (e.g. split-mode rendering).
    pub fn load_into_test(&mut self, pane: usize, path: &str, src: &str) {
        let highlighter = crate::highlight::Highlighter::new(theme::resolve(None).syntax);
        let doc = crate::markdown::render_document(src, &self.palette, &highlighter);
        let line_starts = crate::markdown::line_index(src);
        self.docs[pane] = DocPane {
            path: Some(path.to_string()),
            doc,
            cursor_block: 0,
            sel_anchor: None,
            scroll: 0,
            line_starts,
        };
    }

    /// Set `comment_sync` directly — for tests exercising `add_comment`'s on-send gating
    /// without going through `App::new`/the plugin config.
    pub fn set_comment_sync_for_test(&mut self, sync: CommentSync) {
        self.comment_sync = sync;
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
