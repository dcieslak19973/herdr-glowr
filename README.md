# herdr-glowr

A markdown plan/spec viewer for [herdr](https://herdr.dev). Your agent writes a PLAN.md or a
SPEC.md before it writes code. You read it in a pane beside the chat, comment on the parts you
want changed, and send the notes back. You never leave the terminal.

What you get, in one persistent pane pointed at a git worktree:

- **File list** — every markdown file in the worktree, newest first, so a freshly written plan
  sorts to the top.
- **Rendered markdown** — headings, lists, code fences and tables painted as blocks, not raw
  text.
- **Split view** — press `` ` `` to divide the pane into two rendered docs side by side, e.g. a
  spec on the left and its plan on the right.
- **Block comments** — select one or more rendered blocks and write a note. It stays visible as
  a card under the block instead of hiding behind a marker.
- **Send** — one keystroke drops every comment into the agent's input. You add context and hit
  enter.
- **Themes** — 18 named palettes in dark and light, one config line away. Catppuccin, Dracula,
  Nord, Gruvbox, Tokyo Night, Rosé Pine, Solarized, and more.

It **never edits your worktree** and sends nothing on its own. It writes only under the repo's
git dir: a shared comment store your agent reads and writes through a few CLI subcommands (see
[Working with agents](#working-with-agents)).

## Requirements

- **herdr ≥ 0.7.5** (the plugin system).
- **git** on `PATH`.
- A **truecolor (24-bit)** terminal with Unicode box-drawing support. Pick a theme that matches
  its light or dark background (see [Theme](#theme)).
- **macOS, Linux, or Windows.**

## Install

From the herdr marketplace. You get a prebuilt binary, no Rust toolchain:

```bash
herdr plugin install dcieslak19973/herdr-glowr
```

> **`Error { kind: NotFound, message: "program not found" }`** during
> `herdr plugin install` means herdr could not spawn `git` — it is not installed or not on
> `PATH` in this shell. Install [Git for Windows](https://gitforwindows.org/) (or
> `git` via your package manager), open a fresh shell, and re-run the install.

The sidebar does **not** auto-open by default (`auto_open = false`) — there's nothing to review
until a plan or spec doc exists. Set `auto_open = true` to open it for every newly created
worktree instead (see [Configuration](#configuration)). To toggle it on demand, bind a key to
the **glowr: toggle sidebar** action in your herdr config. Keybindings live in user config, not
in the plugin manifest:

```toml
[[keys.command]]
key = "cmd+g"
type = "plugin_action"
command = "dcieslak19973.glowr.toggle"   # <plugin_id>.<action_id> — note the id, not the name
```

`cmd+…` chords reach herdr. macOS swallows `alt+…`. With no key bound, run the action once with
`herdr plugin action invoke toggle --plugin dcieslak19973.glowr`.

> **Windows:** action ids carry a `-windows` suffix — bind
> `dcieslak19973.glowr.toggle-windows`, not `.toggle`, and invoke
> `herdr plugin action invoke toggle-windows --plugin dcieslak19973.glowr`. Same for `open`
> and `close` below.

`install.sh` also symlinks the binary onto `PATH` at `~/.local/bin/herdr-glowr`, so the
`herdr-glowr` CLI (see [Working with agents](#working-with-agents)) works directly once that
directory is on your `PATH`. On Windows, `install.ps1` does not modify `PATH`; it prints the
installed binary's absolute path, so run `herdr-glowr` commands via that path instead.

Beside `toggle` there are two explicit actions, made for scripts and layout plugins. `open` opens
the sidebar and does nothing when one is already open. `close` closes it and does nothing when
none is. Bind or invoke them the same way, as `dcieslak19973.glowr.open` and
`dcieslak19973.glowr.close`.

## Quick start

The core loop takes four keys. Open the sidebar next to your agent and:

1. **Pick a doc.** The worktree's markdown files are in the right pane, newest first. `j` / `k`
   moves the cursor. The rendered doc opens on the left as you go.
2. **Split, if you want two docs open.** Press `` ` `` to divide the pane into doc A and doc B,
   `Tab` to move focus between the file list and each doc pane.
3. **Select and comment.** Press `v`, then `j` / `k` to extend the selection over rendered
   blocks (or click-drag). Press `c`, type your note, `Enter` to save. It stays on screen as a
   card under the block.
4. **Send.** When you're done, press `s`. Every comment lands in the agent's input. You add
   context and send.

The footer always shows the keys that work right now, so you can learn it by using it. The table
below is the full reference.

## Controls

**Getting around**

| Key | Action |
| --- | --- |
| `j` `k` · `↑` `↓` | Move the cursor in the focused pane |
| `Tab` | Cycle focus — file list → doc A → doc B (while split) → file list |
| `` ` `` | Toggle the split — one rendered doc, or doc A and doc B side by side |
| `w` | Toggle line wrap |
| `]` `[` | Widen / narrow the file list |
| `q` | Quit |

**Reviewing** (in a doc pane)

| Key | Action |
| --- | --- |
| `v` | Start a block selection, then `j` / `k` to extend (or click-drag) |
| `c` | Comment on the selection — or on the current block |
| `e` `d` | Edit / delete the comment under the cursor |
| `n` `N` | Jump to the next / previous comment |
| `l` | List every comment |
| `s` | Send all comments to the agent |
| `y` | Copy all comments to the clipboard |
| `esc` | Clear the selection |

**In the comments list** (`l`)

| Key | Action |
| --- | --- |
| `j` `k` | Move the highlighted row |
| `e` `d` | Edit / delete the highlighted comment |
| `x` | Resolve / reopen the highlighted comment |
| `s` `y` | Send / copy, same as in the doc pane |
| `esc` `l` `q` | Close the list |

**In the comment box**

| Key | Action |
| --- | --- |
| `Enter` | Save the comment |
| `Esc` | Cancel |
| `Shift+Enter` · `Alt+Enter` · `Ctrl+J` | Insert a newline |

Plus the usual caret moves: arrows, `Home` / `End`, `Ctrl+A` / `Ctrl+E`, word-jump with
`Alt+b` / `Alt+f`, and `Ctrl+W` / `Ctrl+U` / `Ctrl+K` to delete by word or to the line edge.

herdr is mouse-native, so clicking a file, dragging to select blocks, clicking the `Send`
button, and the scroll wheel all work too.

## Working with agents

Comments aren't only for you to write and send — they're a two-way channel with the coding
agent, backed by one shared store per repo (`<git-dir>/glowr/comments/`, one JSON file per
comment). Your agent reads and writes it through subcommands on this same binary; you keep using
the TUI exactly as above.

### Install the skill

The universal path works across harnesses — Claude Code, Gemini CLI, GitHub Copilot, OpenCode,
Amp, Codex and more — via the [skills CLI](https://github.com/skills-sh/skills), verified working
against this repo:

```bash
npx skills add dcieslak19973/herdr-glowr --skill glowr-comments -g
```

`-g` installs globally (every harness's personal skills directory, e.g.
`~/.claude/skills` for Claude Code); omit it to install per-project instead, into each harness's
project-level directory in the current repo. Either way, once installed it's in every session's
skill list: "address my plan comments" works with no `skill-path`/`load that skill` preamble.

If you'd rather not use `npx`, `herdr-glowr` installs the skill itself, offline, from the
already-installed plugin — no npm required. After `herdr plugin install`, the binary is available
as `herdr-glowr` *if* `~/.local/bin` is on your `PATH` (`install.sh` links it there; see
[Install](#install)):

```bash
herdr-glowr skill-install             # ~/.claude/skills/glowr-comments (Claude Code, personal)
herdr-glowr skill-install --project   # ./.agents/skills/glowr-comments (universal, project-level)
```

If `~/.local/bin` isn't on `PATH`, skip the bare command and invoke the plugin action instead,
which runs the same binary by its plugin-root path and needs no `PATH` entry:

```bash
herdr plugin action invoke skill-install --plugin dcieslak19973.glowr
```

`--project` installs into `.agents/skills/`, the location read by Gemini CLI, GitHub Copilot,
OpenCode, Amp, Antigravity and others (Claude Code reads it too, via the skills ecosystem
tooling; Codex and Cursor also read `.claude/skills/`). Commit that path and every agent session
opened in the repo picks it up, no per-user install step at all. `--project` and `--target` are
mutually exclusive.

Variants, either mode:

- `--copy` installs a real file instead of a symlink (e.g. if your platform or setup makes
  symlinks awkward). Windows falls back to `--copy` behavior automatically, with a note on
  stderr, since it can't always create symlinks.
- `--target <dir>` installs somewhere else entirely, e.g. a specific harness's project-level
  directory:

  ```bash
  herdr-glowr skill-install --target .claude/skills/glowr-comments
  ```
- Re-running is idempotent: an unchanged install prints `already installed at <path>` and exits
  0. A conflicting file at the target exits 1 naming it; add `--force` to replace it.

### Make it proactive (CLAUDE.md)

Installing the skill covers "the agent knows how, once asked." It doesn't make the agent check
comments unprompted — for that, put this in your `CLAUDE.md` (loaded every session, unlike the
skill list, which is only consulted when the agent decides it's relevant):

```
Plan/spec feedback happens in the herdr-glowr sidebar — when starting work or when
review feedback is mentioned, run `herdr-glowr comment list` and address open comments.
```

`skill-install` prints this same snippet after a fresh install, as a copy-pasteable reminder.
Without it, the most common failure mode is the agent simply not knowing glowr exists until you
say so.

### Other agents/harnesses

For agents that read `AGENTS.md` instead of (or in addition to) `CLAUDE.md`, add the same
pointer line there. For anything without a skill system at all, fall back to the generic prompt,
which works in any agent session in the repo:

```
Run `herdr-glowr skill-path`, load that skill, then review this plan and leave
comments in glowr.
```

Every bare `herdr-glowr` in these snippets assumes the install-time PATH link (see
[Install](#install)); if the shell can't find it, use the plugin-root path or
`herdr plugin action invoke skill-install --plugin dcieslak19973.glowr`.

`skill-path` prints the bundled skill's location. The agent loads it, lists your open comments
(`herdr-glowr comment list`), acts on them, and leaves its own findings as cards in your doc —
you'll see an `agent`-chipped card within a poll tick, no notification needed.

### The reverse flow

Leave comments the usual way (`c`, type, `Enter`), then tell the agent:

```
address the comments I left in glowr
```

It runs `herdr-glowr comment list`, addresses each one, and `comment resolve <id>`s what it
handled. You'll see the card dim in the doc; the comments list (`l`) still shows it, marked
`resolved`, so you can reopen it with `x` if needed.

### `comment_sync`: when your comments become visible to the agent

```toml
# ~/.config/herdr/plugins/config/dcieslak19973.glowr/config.toml
comment_sync = "immediate"   # default; or "on-send"
```

- **`immediate`** (default) — every comment you save persists to the store right away, so you can
  tell the agent to address your review at any point, not only after a send.
- **`on-send`** — your comments stay pane-local until you press `s`, which persists them (and
  exports as always). Nothing reaches the agent's view of the store before that keystroke, if you
  prefer the older "nothing leaves without a keystroke" posture.
- Either setting is about *your* comments only — the agent's own comments are always written to
  the store immediately and always rendered in your pane, regardless of this key.

Sending (`s`) does not remove comments from the store: an exported comment stays `open` and
resolvable, so a send doubles as a durable note rather than a one-shot handoff.

## Configuration

Everything is set in glowr's own config file:

```text
~/.config/herdr/plugins/config/dcieslak19973.glowr/config.toml
```

Create the file if it does not exist yet. herdr hands this directory to the plugin as
`$HERDR_PLUGIN_CONFIG_DIR`, and the path above is where it lives on disk. Note that this is
glowr's file, not herdr's. Settings added to herdr's own `~/.config/herdr/config.toml` never
reach glowr.

The file accepts these six keys:

```toml
theme = "tokyo-night"
toggle_placement = "overlay"
toggle_direction = "down"
auto_open = true
show_ignored = false
comment_sync = "on-send"
```

- `theme` — the UI + syntax theme (see [Theme](#theme)); default `catppuccin`.
- `toggle_placement` — where the `toggle` action opens the sidebar: `split` (default),
  `overlay`, `zoomed`, or `tab`.
- `toggle_direction` — split direction: `right` (default) or `down`.
- `auto_open` — open the sidebar automatically for a newly created worktree; default `false`.
- `show_ignored` — include gitignored markdown files in the file list, dimmed; default `false`.
- `comment_sync` — when *your* comments reach the shared store the agent reads; see
  [Working with agents](#working-with-agents) above. Default `immediate`.

A missing file or omitted key uses its default. Any unknown key, wrong type, or invalid value
makes the whole file invalid. glowr never applies the valid-looking parts. The sidebar then
shows only the config error, and actions or events exit non-zero without touching the workspace.
Fix the file and the running sidebar recovers on its next refresh. Replace the file atomically
if your editor or config manager might expose a partial save.

### Theme

One theme colors the whole UI, chrome and syntax together. Set it in glowr's config file.
glowr re-reads the file on refresh, so editing it and refreshing re-themes without a relaunch:

```toml
# ~/.config/herdr/plugins/config/dcieslak19973.glowr/config.toml
theme = "tokyo-night"
```

Pick a name that matches your terminal's light or dark background. The pane keeps the
terminal's background, so a light theme on a dark terminal reads poorly, and so does the
reverse. Available:

- **Dark:** `catppuccin`, `catppuccin-frappe`, `catppuccin-macchiato`, `dracula`, `nord`,
  `gruvbox`, `one-dark`, `solarized`, `monokai`, `tokyo-night`, `rose-pine`.
- **Light:** `catppuccin-latte`, `gruvbox-light`, `one-light`, `solarized-light`, `github-light`,
  `tokyo-night-day`, `rose-pine-dawn`.

Names match herdr's where both ship a palette. An unknown config name is an error.
