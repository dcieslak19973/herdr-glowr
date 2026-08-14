//! File list pane: the document tree and its selection state.

use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

/// A markdown file discovered in a repo's worktree.
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// Repo-relative path, forward-slash separated.
    pub path: String,
    /// Last-modified time of the file on disk.
    pub mtime: SystemTime,
    /// Whether this file is excluded from the tracked/untracked set by `.gitignore`.
    pub ignored: bool,
}

/// List markdown (`.md`/`.markdown`, case-insensitive) files in `repo`'s worktree,
/// sorted newest-modified first (ties broken by path, ascending).
///
/// Tracked files and untracked-but-not-ignored files are always included. When
/// `show_ignored` is set, gitignored markdown files are also included and marked
/// `ignored: true`.
pub fn markdown_files(repo: &Path, show_ignored: bool) -> Vec<FileEntry> {
    let mut entries = Vec::new();
    list_files(repo, &["--cached", "--others", "--exclude-standard"], false, &mut entries);
    if show_ignored {
        list_files(repo, &["--others", "--ignored", "--exclude-standard"], true, &mut entries);
    }
    entries.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.path.cmp(&b.path)));
    entries
}

/// Run `git -C repo ls-files <args>` and append markdown matches to `out`.
fn list_files(repo: &Path, args: &[&str], ignored: bool, out: &mut Vec<FileEntry>) {
    let Ok(output) = Command::new("git").arg("-C").arg(repo).arg("ls-files").args(args).output()
    else {
        return;
    };
    if !output.status.success() {
        return;
    }
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if !is_markdown(line) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(repo.join(line)) else { continue };
        let Ok(mtime) = meta.modified() else { continue };
        out.push(FileEntry { path: line.replace('\\', "/"), mtime, ignored });
    }
}

/// Whether `path` has a `.md`/`.markdown` extension (case-insensitive).
fn is_markdown(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
}
