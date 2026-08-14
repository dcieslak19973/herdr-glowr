---
Status: Draft
Created: 2026-08-13
Last edited: 2026-08-13
---

# herdr-glowr design

A terminal **markdown viewer** in a herdr pane with **bidirectional line-anchored
comments** — a `glow`-like reader for the plan/spec markdown a coding agent writes before
it starts coding, with reviewr's comment loop bolted on. You read the rendered plan, comment
on the blocks you want changed, send the notes to the agent; the agent revises the doc and
leaves its own notes back. You never leave the terminal.

This is a sibling to [`herdr-reviewr`](https://github.com/dcieslak19973/herdr-reviewr): it
reuses reviewr's comment store, theme, config, herdr-host, CLI, and plugin machinery
verbatim, and replaces reviewr's **diff engine** with a **markdown rendering engine**.

## Purpose and the loop

Harnesses (Claude Code and others) increasingly produce a design/spec and an implementation
plan as markdown *before* writing code. glowr is where the human reviews that prose:

```
agent writes plan.md / spec.md → open glowr → read it rendered
→ comment on a block → send the comments to the agent → add a line, hit enter
→ agent revises the doc and resolves / replies → repeat
```

One binary (`herdr-glowr`, Rust + ratatui) runs in a herdr pane pointed at one git
worktree. It renders in the real terminal, so fonts and colors are whatever the user runs.

## Scope

- A **markdown file list** of the worktree's `.md`/`.markdown` files, newest-modified first.
- A **rendered document** pane (glow-style) for the selected file.
- A **split mode** that renders two documents side by side (e.g. a spec and its plan) for the
  review cycle, each independently scrolled and commented; a key collapses back to one.
- **Block-anchored comments**: select one or more rendered blocks, write a note; it renders as
  a card under the block and persists to a shared store.
- **Export** of comments to the agent's input pane or the clipboard.
- An **agent-facing CLI** (`comment add/list/resolve/rm`) over the same store, plus a bundled
  skill so the agent knows the loop.
- **Themes** (18 palettes, dark and light), poll-based refresh, keyboard and mouse.

## Non-goals

- **No diff, no git scopes.** No uncommitted/branch/last-turn views, no last-turn baseline,
  no `refs/glowr/`. glowr reads working-tree file content only. (This is the load-bearing
  difference from reviewr — `diff.rs`, `git.rs`, `turn.rs` do not exist here.)
- **No PR/forge tab, no browser open.** `forge/` and `browser.rs` are dropped.
- **No markdown editing.** glowr never writes the worktree; it only reads docs and writes
  under the git dir (the comment store). The agent edits docs; glowr does not.
- **No reply threads, categories, or severities.** Flat notes with resolve, text only — same
  as reviewr.
- **No line-number rebasing.** The snippet keeps a comment locatable after the doc shifts.
- **No non-markdown rendering.** glowr renders markdown; other files never appear in the list.
- **No shared store with reviewr.** glowr's store is its own directory under the git dir.

## Reuse strategy

glowr starts as a copy of the reviewr crate. Modules fall into three groups:

| Keep (near-verbatim) | Strip entirely | Build new |
| --- | --- | --- |
| `comments.rs` (per-comment JSON store), `theme.rs` (palettes), `config.rs`, `log.rs`, `sidebar.rs` (toggle/open/close orchestration), `herdr.rs` (agent-pane discovery + send), `export.rs` (comment blocks), `cli.rs` (subcommands, `skill-*`), `main.rs` (dispatch), plugin manifest + `install.sh`/`install.ps1` | `diff.rs`, `git.rs`, `turn.rs`, `forge/`, `browser.rs`, `highlight.rs`'s diff-tinting logic (its `syntect` setup is retained for code fences), the PR tab, scope machinery | `markdown.rs` (parse + render + source mapping), block cursor & selection in `app.rs`, markdown file discovery in `file_list.rs`, the two-doc split in `ui.rs` |

Kept modules keep their public shape. `model.rs` is simplified: `Scope`/`ChangeKind`/diff
`Side` handling collapse — see [Comment model](#comment-model).

Renamed throughout: `reviewr` → `glowr`, plugin id `dcieslak19973.reviewr` →
`dcieslak19973.glowr`, store dir `reviewr/comments/` → `glowr/comments/`, skill
`reviewr-comments` → `glowr-comments`, pane title `reviewr` → `glowr`.

## Comment model

The central object is unchanged from reviewr — a note on a run of source lines in one file,
carrying the verbatim snippet it points at:

```json
{
  "id": "c-1752264012345-7f3a",
  "author": "user",
  "status": "open",
  "created_at": "2026-08-13T18:40:12Z",
  "file": "docs/plans/auth.md",
  "start": 14,
  "end": 18,
  "lines": "## Phase 1\n1. Scaffold the crate\n2. Wire the workspace",
  "text": "split this phase — scaffolding and wiring are separate PRs"
}
```

| field | type | meaning |
| --- | --- | --- |
| `id` | string | `c-<epoch-ms>-<4 hex>`, unique and sortable (reviewr's `new_id`) |
| `author` | enum | `user` (TUI) or `agent` (CLI default) |
| `status` | enum | `open` or `resolved` |
| `created_at` | string | ISO-8601 `…Z` |
| `file` | string | repo-relative path to the markdown file |
| `start` / `end` | integer | 1-based **source-line** range in the markdown file, `end ≥ start` |
| `lines` | string | the verbatim **markdown source** of that range (the anchor) |
| `text` | string | free-form note, possibly multi-line |

Differences from reviewr's model:

- **No `side`.** reviewr distinguishes `new`/`old` diff sides; glowr has no diff, so every
  comment anchors to the current file's source lines. `side` is dropped from the schema, the
  CLI (`--side` removed), and the export header (no ` (removed)` suffix). Existing reviewr
  stores are a different directory, so there is no migration concern.
- **`lines` is raw markdown source**, not diff lines with `+`/`-`/space markers — the exact
  bytes of `start..=end` in the file. The agent locates the block by this snippet.

Anchor rules (as reviewr): `lines` is authoritative; `start`/`end` orient a human and are
never re-bound as the doc shifts; the range is always contiguous.

### Store

Identical mechanism to reviewr, different directory: one JSON file per comment at
`<git-dir>/glowr/comments/<id>.json`, `<git-dir>` from `git rev-parse --git-dir` (per
worktree automatically). Exclusive-create add, tmp-then-rename mutate, unknown fields
survive a rewrite, a corrupt file is skipped and logged, the TUI re-reads on a cheap
per-tick signature change so an agent's CLI write appears without user action. This is
`comments.rs` unchanged except the directory name and the removal of the `side` field from
`to_value`/`from_value`.

## Markdown rendering

`markdown.rs` is the new core. It parses a document's source with **`pulldown-cmark`** and
renders it to styled ratatui content, keeping a source-line mapping for anchoring.

### Parse → block model

`pulldown-cmark` emits an event stream with **byte-offset ranges** into the source
(`Parser::into_offset_iter`). glowr folds the stream into a `Vec<Block>`, where a `Block` is
the smallest independently-commentable unit:

| markdown construct | block granularity |
| --- | --- |
| heading (`#`..`######`) | one block |
| paragraph | one block |
| list item | one block per item (a nested list's items are their own blocks) |
| blockquote | one block per contained paragraph, rendered with a quote bar |
| fenced/indented code | **one block per source line** (line-level granularity inside code) |
| table | one block per row (header row included) |
| thematic break (`---`) | one block |
| HTML block | one block, rendered as dimmed raw text |

Each `Block` records:

- `source_start` / `source_end`: 1-based source line range (from the event's byte offsets,
  mapped through a precomputed line-start index).
- `rendered: Vec<Line<'static>>`: the styled terminal lines for the block, produced from its
  inline events (bold, italic, inline code, links rendered as `text (url)` or just text,
  strikethrough) and its block style (heading size/color, list marker/indent, quote bar,
  code with `syntect` highlighting via the retained highlighter).

### Rendered rows and mapping

A document renders to a flat `Vec<RenderRow>`, each row carrying:

- the styled `Line` to paint,
- the index of the `Block` it belongs to,
- whether it is that block's first row (for cursor/selection painting and card placement).

This mirrors reviewr's `Row`/`FileDiff` plumbing so `ui.rs`'s scroll, wrap, hit-testing, and
card-splicing helpers carry over with the row source swapped. Line wrapping reuses reviewr's
`wrap_segments`/`row_height` machinery.

### Element support (v1)

CommonMark + GFM tables, task lists, strikethrough. Rendered: headings, paragraphs,
emphasis/strong/code/strikethrough spans, links (label shown, URL in a dim trailing paren),
images (alt text with an `image` chip), bullet/ordered/task lists with nesting indent,
blockquotes with a colored bar, fenced code with syntax highlighting, tables (aligned
columns within the pane width), thematic breaks, raw HTML (dimmed). Out of scope for v1:
footnotes rendered as links, definition lists, embedded HTML layout. These render as their
nearest plain-text fallback, never an error.

## Block cursor and selection

The doc pane's cursor is a **block cursor**, not a line cursor.

- `j`/`k` (and `↑`/`↓`) move the cursor to the previous/next block; the whole current block
  is highlighted (reviewr's selection fill, applied to every row of the block).
- `PageUp`/`PageDown`, `Ctrl+U`/`Ctrl+D` scroll by rows as in reviewr; the cursor follows to
  the nearest visible block.
- `v` starts a selection at the cursor block; `j`/`k` extend it across **adjacent** blocks
  (contiguous by source line). Mouse click-drag selects blocks the same way. `esc` clears.
- `c` comments on the selection — or the cursor block if none. The selection's anchor is
  `start = min source_start`, `end = max source_end` across selected blocks; `lines` = the
  verbatim source `start..=end`. Because blocks are contiguous and source-ordered, the
  snippet never omits hidden text.
- Inside a fenced code block, blocks are per-line, so a comment can pin a single code line.
- `e`/`d` edit/delete the comment under the cursor; `n`/`N` jump to next/previous comment;
  `l` opens the comments list; `s` sends, `y` copies — all as in reviewr.

Comment cards splice read-only under the last row of their anchor block, styled exactly as
reviewr's cards (location title, `agent` chip for agent comments, muted when resolved,
hide-resolved toggle). `app.rs`'s comment-card layout/paint stay; only the anchor mapping
(block → row) changes.

## Layout and TUI

The frame is reviewr's three vertical bands — header, body, footer — with the body reworked:

- **Header**: `glowr` title, and a right-aligned `[ Send (N) ]` button (N = open
  user-authored comments). No tab bar (glowr has one view), no scope chip.
- **Body, single mode (default)**: rendered doc on the left, markdown file list on the right,
  a draggable divider between them (reviewr's `list_pct` resize with `[`/`]` and mouse).
- **Body, split mode**: the doc side divides into two rendered doc panes side by side; the
  file list stays on the right. Each doc pane has its own cursor, scroll, and selection;
  focus cycles list → doc A → doc B with `Tab`. Toggle split with `` ` `` (backtick).
- **Footer**: the context's actions, packed and prioritized as in reviewr
  (`v select · c comment · s send · Tab focus · [ ] width · q quit`, plus `` ` `` split).

Focus, the inline comment composer (`render_composer`, the caret/wrap editor), the comments
list overlay (`l`), mouse hit-testing, and line wrap (`w`) are reviewr's, retargeted from
diff rows to render rows.

### File discovery

`file_list.rs` lists the worktree's markdown files (`*.md`, `*.markdown`), sorted by
descending mtime so a just-written plan surfaces at the top. `.gitignore` is respected by
default (an ignored `node_modules/**/*.md` never clutters the list); a config toggle
(`show_ignored`) can include them, dimmed. No change annotations, no `+/-` stats, no tree of
non-markdown dirs — just the flat, path-labeled, mtime-sorted list. A directory grouping
(showing parent dirs) is retained from reviewr's list rendering for readability.

When launched with a path argument (`herdr-glowr <file.md>`), that file opens selected; in
split mode a second path argument opens in doc B. Inside the herdr pane no argument is
passed, so the newest file is selected on start.

## CLI (agent-facing)

`cli.rs` unchanged in shape; `--side` removed, store dir changed. Run with no subcommand it
launches the TUI.

```
herdr-glowr comment add --file <path> --start <n> [--end <n>]
                        [--lines <snippet>] [--author user|agent] --text <text>
herdr-glowr comment list [--json] [--all]
herdr-glowr comment resolve <id>
herdr-glowr comment rm <id>
herdr-glowr sidebar [toggle|open|close|auto-open]
herdr-glowr skill-path
herdr-glowr skill-install [--target <dir> | --project] [--copy] [--force]
```

| flag | default | notes |
| --- | --- | --- |
| `--file`, `--start`, `--text` | — | required; unknown/valueless flag → usage error (exit 2) |
| `--end` | `--start` | single-line comment needs only `--start` |
| `--lines` | `""` | agent note need not carry a snippet |
| `--author` | `agent` | `user` or `agent` |

`add` prints the new id (exit 0). `list` defaults to open, `--all` includes resolved; human
rows `<id>  <status>  <author>  <file>:<start>-<end>  <first line of text>`; `--json` prints
the full documents. `resolve`/`rm` on an unknown id exit 1 naming it. All subcommands
resolve the store from the cwd's git dir and exit 1 with one stderr line when that fails.

### Export

One block per **open, user-authored** comment, to the agent input or the clipboard (agent
comments are never sent back). Header `path:start-end` (no ` (removed)` — there is no diff
side), body the `lines` verbatim, footer the `text` trimmed with 2+ newlines collapsed, one
blank line between blocks, ordered by `file` then `start`. Send persists every open user
comment first, injects the blocks into the resolved agent pane without submitting, and
focuses it. Copy writes the same blocks to the clipboard. Neither clears or resolves.

## herdr host integration

`herdr-plugin.toml` mirrors reviewr's, renamed:

| entry | id | does |
| --- | --- | --- |
| pane | `sidebar` (+ `-windows` twin) | runs the `herdr-glowr` binary, title `glowr` |
| actions | `toggle`/`open`/`close` (+ twins), `skill-install` | manage the pane, install the skill |
| event | `worktree.created` | auto-opens the pane **only when `auto_open = true`** |

`auto_open` **defaults `false`** for glowr (a fresh worktree has no plan yet; the user opens
glowr when a doc lands), overridable in config. Placement (`toggle_placement`,
`toggle_direction`), the `open`/`close`/`toggle` convergence rules, and the "send to the sole
agent in the tab, else the workspace" resolution are reviewr's `sidebar.rs`/`herdr.rs`
verbatim. Send/copy work without the herdr CLI; without it, sending reports the error and
points at the clipboard copy (`y`).

## Configuration

`$HERDR_PLUGIN_CONFIG_DIR/config.toml`, a subset of reviewr's (no `base_branches`):

```toml
theme = "catppuccin-mocha"     # any of the 18 palettes
toggle_placement = "split"     # split | overlay | zoomed | tab
toggle_direction = "right"     # right | down, split only
auto_open = false              # auto-open on worktree.created (default false)
show_ignored = false           # include .gitignored markdown in the list, dimmed
comment_sync = "immediate"     # immediate | on-send (as reviewr)
```

Invalid plugin config follows reviewr's `config.md` contract (a clear error, no half-open).

## Agent skill

`skills/glowr-comments/SKILL.md`, adapted from `reviewr-comments`: read the user's comments
on the plan/spec with `herdr-glowr comment list`, trust the `lines` snippet over the line
number, revise the doc, `resolve` what you addressed, and `comment add` your own notes where
you changed or questioned something. `skill-install`/`skill-path` resolve it from the running
binary's install location with the dev-checkout fallback, as reviewr.

## Invariants

| # | Always true |
| --- | --- |
| G1 | glowr never writes the worktree. It writes only under the git dir — the `glowr/comments/` store — never a doc, the index, or a branch. |
| G2 | A comment, saved or being typed, is never lost to a refresh, a store merge, or the agent's doc edits. Only an explicit TUI `d` or CLI `rm` removes it; an unreadable store file is skipped, never deleted. |
| G3 | A comment leaves the store only by delete or `rm`. Send/export persists but never consumes it. |
| G4 | A comment's `lines` snippet is the verbatim source of `start..=end` at write time; the range is contiguous and source-ordered. |
| G5 | Rendered-row geometry used for scroll/wrap/hit-testing/cards is computed from one source of truth, so paint and hit-testing cannot desync (reviewr's invariant, retargeted). |
| G6 | The crate forbids `unsafe`. |

## Dependencies

reviewr's set minus the diff/forge-only ones, plus the parser:

- **add**: `pulldown-cmark` (parser with source offsets).
- **keep**: `ratatui`, `syntect` + `two-face` (code-fence highlighting), `unicode-width`,
  `toml`, `serde_json`, `anyhow`.
- **drop**: `similar` (diffing — no longer needed).

## Roadmap (not in v1)

- Live theme switching and in-doc search.
- A "changed since last agent turn" marker on the file list (without full diff scopes).
- Rendering footnotes/definition lists as first-class blocks.
- Following relative links between docs (open the target in the other split pane).

## Open questions

- **Split-mode toggle key**: `` ` `` (backtick) is the tentative binding; confirm it does not
  collide with a herdr global chord in practice.
- **Table wrapping**: very wide tables in a narrow sidebar — v1 clips with a horizontal scroll
  (reuse the diff pane's `h_scroll`); a reflow mode is a later refinement.
