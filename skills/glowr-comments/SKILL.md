---
name: glowr-comments
description: Read, act on, and leave line-anchored comments on plan/spec markdown shared with the herdr-glowr viewer. Use when the user asks you to address their plan/spec comments, or to review a plan or spec doc and leave comments in glowr.
---

# glowr comments

The glowr sidebar and you share one comment store per worktree. Comments are anchored
to `file:start[-end]` with a verbatim markdown snippet. Find the binary as `herdr-glowr` on
PATH (the plugin install links it into `~/.local/bin`); if not found, use
`$HERDR_PLUGIN_ROOT/bin/herdr-glowr` when that env var is set; otherwise ask the user
for the plugin root. Run every command from the repo you are working in.

## Read the user's comments

    herdr-glowr comment list            # open comments, human-readable, ids first
    herdr-glowr comment list --json     # full documents

Trust the `lines` snippet over the line number — the doc may have moved since the
comment was written. Find the snippet in the file, then act.

## The loop

1. `comment list` — see what's open.
2. Revise the doc: edit the plan or spec markdown to address each comment.
3. `herdr-glowr comment resolve <id>` — mark it done. Do not resolve what you did
   not address; say so instead.
4. Leave your own notes where you changed or noticed something:

       herdr-glowr comment add --file docs/plan.md --start 25 \
         --lines '- [ ] Add a retry to the upload step' \
         --text "This step also needs a timeout, or a stuck upload hangs the run"

   `--author agent` is the default; keep it. Notes render as cards in the user's
   pane within a second — no notification step is needed.

## Rules

- Never `comment rm` a user's comment; `resolve` is yours, `rm` is theirs.
- One comment per finding, at the tightest line range that shows it.
- Keep `--text` to a sentence or two; the markdown snippet is visible next to the card.
