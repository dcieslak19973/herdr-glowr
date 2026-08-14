//! Formatting comments and exporting them to the agent or clipboard.
//!
//! See `docs/superpowers/specs/2026-08-13-herdr-glowr-design.md`. A comment becomes a block of
//! `location`, the source snippet, then the text. Sending never consumes a comment —
//! `App::export` persists every open, user-authored comment and leaves it in place, so a
//! comment stays visible (and resolvable) after being sent.

use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::herdr;
use crate::model::Comment;

/// One comment as its export block: location, snippet, then text.
pub fn format_comment(comment: &Comment) -> String {
    format!("{}\n{}\n{}", comment.location(), comment.lines, normalize_text(&comment.text))
}

/// Comment text for export: drop `\r`, trim trailing space per line, and drop blank
/// lines so a multi-line comment can never introduce the blank-line block separator.
fn normalize_text(text: &str) -> String {
    text.replace('\r', "")
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Many comments, sorted by file then start line, one blank line between blocks.
pub fn format_all(comments: &[&Comment]) -> String {
    let mut sorted = comments.to_vec();
    sorted.sort_by(|a, b| a.file.cmp(&b.file).then(a.start.cmp(&b.start)));
    sorted.iter().map(|c| format_comment(c)).collect::<Vec<_>>().join("\n\n")
}

/// A destination comments can be exported to. Export succeeds or errors as a whole.
pub trait ExportTarget {
    fn export(&self, text: &str) -> Result<()>;
    fn label(&self) -> &'static str;
}

/// Whether `name` resolves to an executable on `PATH` — a dependency-free `which`. On unix a
/// file in a `PATH` directory is the executable; on Windows the on-disk name usually carries
/// a `PATHEXT` extension (`clip` → `clip.exe`), so each extension is tried too.
fn on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else { return false };
    // Read once, not per `PATH` directory. Only one case is probed per extension: Windows file
    // lookups are case-insensitive, so a single `is_file()` check matches the on-disk name
    // regardless of how `PATHEXT` or `name` happen to be cased.
    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
            .split(';')
            .filter(|ext| !ext.is_empty())
            .map(str::to_string)
            .collect()
    } else {
        Vec::new()
    };
    std::env::split_paths(&path).any(|dir| {
        dir.join(name).is_file()
            || extensions.iter().any(|ext| dir.join(format!("{name}{ext}")).is_file())
    })
}

/// A clipboard tool and the args that make it read stdin into the system clipboard. Tried in
/// order — the first one present on `PATH` wins. macOS ships `pbcopy`; Linux needs one of these
/// installed (Wayland `wl-copy`, or X11 `xclip`/`xsel`); Windows ships `clip`. OSC 52 is
/// roadmap.
const CLIPBOARD_TOOLS: &[(&str, &[&str])] = &[
    ("pbcopy", &[]),
    ("wl-copy", &[]),
    ("xclip", &["-selection", "clipboard"]),
    ("xsel", &["--clipboard", "--input"]),
    ("clip", &[]),
];

/// The system clipboard, via the first available platform clipboard tool.
#[derive(Debug)]
pub struct Clipboard;

impl ExportTarget for Clipboard {
    fn label(&self) -> &'static str {
        "clipboard"
    }

    fn export(&self, text: &str) -> Result<()> {
        let (cmd, args) = select_tool(CLIPBOARD_TOOLS, on_path).context(
            "no clipboard tool found (install wl-clipboard, xclip, or xsel) — \
             use \"Add all to chat\" instead",
        )?;
        let mut child = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawning {cmd}"))?;
        child
            .stdin
            .as_mut()
            .with_context(|| format!("{cmd} stdin unavailable"))?
            .write_all(text.as_bytes())
            .with_context(|| format!("writing to {cmd}"))?;
        if !child.wait().with_context(|| format!("waiting for {cmd}"))?.success() {
            bail!("{cmd} exited non-zero");
        }
        Ok(())
    }
}

/// The first clipboard tool the `present` predicate accepts, preserving list order.
fn select_tool(
    tools: &'static [(&'static str, &'static [&'static str])],
    present: impl Fn(&str) -> bool,
) -> Option<(&'static str, &'static [&'static str])> {
    tools.iter().copied().find(|(cmd, _)| present(cmd))
}

/// The agent pane: fill its input via `herdr agent send`, then focus it.
#[derive(Debug)]
pub struct Agent;

impl ExportTarget for Agent {
    fn label(&self) -> &'static str {
        "agent"
    }

    fn export(&self, text: &str) -> Result<()> {
        let pane = herdr::resolve_agent_pane()?;
        herdr::send_text(&pane, text)?;
        // Focus is a convenience once the text is delivered; a focus failure must NOT fail the
        // export, or the comments stay unconsumed and the next Send duplicates the whole review.
        let _ = herdr::focus(&pane);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{CLIPBOARD_TOOLS, format_all, format_comment, select_tool};
    use crate::model::Comment;

    #[test]
    fn clipboard_tool_selection_prefers_list_order_and_can_be_empty() {
        // None present -> no tool (the caller surfaces the "install one" error).
        assert!(select_tool(CLIPBOARD_TOOLS, |_| false).is_none());
        // Only an X11 tool present -> it's chosen, with its selection args.
        assert_eq!(
            select_tool(CLIPBOARD_TOOLS, |c| c == "xclip"),
            Some(("xclip", &["-selection", "clipboard"][..]))
        );
        // When several are present, earlier in the list wins (pbcopy over xclip).
        assert_eq!(
            select_tool(CLIPBOARD_TOOLS, |c| c == "pbcopy" || c == "xclip").map(|(cmd, _)| cmd),
            Some("pbcopy")
        );
    }

    fn comment(file: &str, start: u32, end: u32, lines: &str, text: &str) -> Comment {
        Comment { file: file.into(), start, end, lines: lines.into(), text: text.into() }
    }

    #[test]
    fn block_is_location_snippet_text() {
        let c = comment(
            "specs/plan.md",
            40,
            41,
            "## Old heading\nsome body text",
            "this heading looks wrong",
        );
        assert_eq!(
            format_comment(&c),
            "specs/plan.md:40-41\n## Old heading\nsome body text\nthis heading looks wrong"
        );
    }

    #[test]
    fn single_line_location_has_no_range() {
        let c = comment("a.md", 38, 38, "- an item", "still needed");
        assert_eq!(format_comment(&c), "a.md:38\n- an item\nstill needed");
    }

    #[test]
    fn multiline_text_keeps_breaks_but_drops_blank_lines() {
        let c = comment("a.md", 1, 1, "# H", "first line\n\n  \nsecond line\n");
        assert_eq!(format_comment(&c), "a.md:1\n# H\nfirst line\nsecond line");
    }

    #[test]
    fn all_sorts_by_file_then_start_with_blank_separator() {
        let b = comment("b.md", 5, 5, "x", "two");
        let a2 = comment("a.md", 20, 20, "y", "later");
        let a1 = comment("a.md", 3, 3, "z", "earlier");
        let out = format_all(&[&b, &a2, &a1]);
        assert_eq!(out, "a.md:3\nz\nearlier\n\na.md:20\ny\nlater\n\nb.md:5\nx\ntwo");
    }
}
