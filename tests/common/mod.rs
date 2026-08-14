//! A real on-disk git repo for integration tests. Every helper shells out to the
//! actual `git` binary, so tests exercise the same surface the app does at runtime.
//!
//! `dead_code`/`unreachable_pub` are allowed because each test binary includes this
//! module and uses only the subset of helpers it needs.
#![allow(dead_code, unreachable_pub)]

use std::path::Path;
use std::process::Command;

use ratatui::buffer::Buffer;
use tempfile::TempDir;

pub struct TempRepo {
    dir: TempDir,
}

impl TempRepo {
    /// A fresh repo on branch `main` with an identity configured.
    pub fn new() -> Self {
        let repo = Self { dir: TempDir::new().expect("tempdir") };
        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&["config", "user.email", "test@herdr.test"]);
        repo.git(&["config", "user.name", "Test"]);
        repo
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Run `git -C <repo> <args>`, asserting success, returning stdout.
    pub fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git").arg("-C").arg(self.path()).args(args).output().expect("git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    pub fn write(&self, rel: &str, contents: &str) {
        let path = self.path().join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(path, contents).expect("write");
    }

    pub fn remove(&self, rel: &str) {
        std::fs::remove_file(self.path().join(rel)).expect("remove");
    }

    /// Stage everything and commit.
    pub fn commit_all(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
    }
}

/// Concatenate every cell's symbol, row by row, into one string (rows separated by `\n`) —
/// a plain-text view of a rendered `Buffer` for substring assertions in render tests.
pub fn buffer_to_string(buffer: &Buffer) -> String {
    let area = buffer.area;
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            if let Some(cell) = buffer.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}
