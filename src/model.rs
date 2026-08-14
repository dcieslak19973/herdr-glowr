//! In-memory review model: comments over a markdown document.
//!
//! A comment's lifecycle (id, author, status) lives in [`crate::comments::StoredComment`];
//! `CommentStore` is the TUI-session view over a `Vec` of them. A refresh never drops a
//! comment — only delete, or an external agent removing its file, does.

use crate::comments::{Author, Status, StoredComment, new_id, now_iso};

/// A reviewer comment anchored to a run of markdown source lines, carrying the snippet.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Comment {
    pub file: String,
    pub start: u32,
    pub end: u32,
    /// Verbatim markdown source lines the comment anchors to (`start..=end`).
    pub lines: String,
    pub text: String,
}

impl Comment {
    /// The `path:start-end` (or `path:line`) location.
    pub fn location(&self) -> String {
        if self.start == self.end {
            format!("{}:{}", self.file, self.start)
        } else {
            format!("{}:{}-{}", self.file, self.start, self.end)
        }
    }
}

/// The in-memory comment list for one worktree review session: every entry carries its
/// lifecycle metadata (id/author/status) from the moment it is written, so a TUI-authored
/// comment is indistinguishable in shape from one synced in from the on-disk store.
#[derive(Default, Debug)]
pub struct CommentStore {
    items: Vec<StoredComment>,
}

impl CommentStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &StoredComment> {
        self.items.iter()
    }

    pub fn get(&self, index: usize) -> Option<&StoredComment> {
        self.items.get(index)
    }

    /// The current index of the comment with id `id`, or `None` if it no longer exists. Every
    /// action held across a poll tick (an edit in progress, an overlay keystroke) must re-resolve
    /// through this rather than trust a previously-read index — a disk sync can replace and
    /// re-sort the whole set between the moment an index was read and the moment it is used, so
    /// a stale index can silently name a different (or no) comment.
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.items.iter().position(|sc| sc.id == id)
    }

    /// Append a comment written just now in the TUI, wrapping it with fresh lifecycle
    /// metadata (a new id, `Author::User`, `Status::Open`) — every comment a reviewer writes
    /// starts here. Returns its index.
    pub fn add(&mut self, comment: Comment) -> usize {
        self.items.push(StoredComment {
            id: new_id(),
            author: Author::User,
            status: Status::Open,
            created_at: now_iso(),
            comment,
        });
        self.items.len() - 1
    }

    /// Replace the text of the comment at `index`. Returns `false` if out of range.
    pub fn edit(&mut self, index: usize, text: String) -> bool {
        if let Some(c) = self.items.get_mut(index) {
            c.comment.text = text;
            true
        } else {
            false
        }
    }

    /// Flip the status of the comment at `index`. Returns `false` if out of range.
    pub fn set_status(&mut self, index: usize, status: Status) -> bool {
        if let Some(c) = self.items.get_mut(index) {
            c.status = status;
            true
        } else {
            false
        }
    }

    /// Remove and return the comment at `index` (the only way a comment leaves the set from
    /// the TUI side).
    pub fn take(&mut self, index: usize) -> Option<StoredComment> {
        if index < self.items.len() { Some(self.items.remove(index)) } else { None }
    }

    /// Replace the whole set — the result of a disk-sync merge.
    pub fn replace(&mut self, items: Vec<StoredComment>) {
        self.items = items;
    }

    /// Open, user-authored comments — exactly what a send/export sends and a Send button
    /// counts. An agent's own comments and already-resolved ones are never part of the payload.
    pub fn open_user_comments(&self) -> Vec<&StoredComment> {
        self.items
            .iter()
            .filter(|sc| sc.status == Status::Open && sc.author == Author::User)
            .collect()
    }

    /// The count `open_user_comments` would return, without allocating the `Vec`.
    pub fn sendable(&self) -> usize {
        self.items
            .iter()
            .filter(|sc| sc.status == Status::Open && sc.author == Author::User)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::{Author, Comment, CommentStore, Status};

    fn comment(file: &str, start: u32, end: u32, text: &str) -> Comment {
        Comment { file: file.into(), start, end, lines: "x".into(), text: text.into() }
    }

    #[test]
    fn location_formats_range_and_single() {
        let mut c = comment("a.rs", 40, 52, "x");
        assert_eq!(c.location(), "a.rs:40-52");
        c.end = 40;
        assert_eq!(c.location(), "a.rs:40");
    }

    #[test]
    fn add_get_edit() {
        let mut s = CommentStore::new();
        let i = s.add(comment("a.rs", 1, 1, "first"));
        assert_eq!(s.len(), 1);
        assert_eq!(s.get(i).unwrap().comment.text, "first");
        assert!(s.edit(i, "second".into()));
        assert_eq!(s.get(i).unwrap().comment.text, "second");
        assert!(!s.edit(99, "nope".into()));
    }

    #[test]
    fn add_wraps_fresh_lifecycle_metadata() {
        let mut s = CommentStore::new();
        let i = s.add(comment("a.rs", 1, 1, "first"));
        let sc = s.get(i).unwrap();
        assert!(sc.id.starts_with("c-"), "a fresh id: {}", sc.id);
        assert_eq!(sc.author, Author::User);
        assert_eq!(sc.status, Status::Open);
    }

    #[test]
    fn set_status_flips_in_place_and_reports_out_of_range() {
        let mut s = CommentStore::new();
        let i = s.add(comment("a.rs", 1, 1, "one"));
        assert!(s.set_status(i, Status::Resolved));
        assert_eq!(s.get(i).unwrap().status, Status::Resolved);
        assert!(!s.set_status(99, Status::Open));
    }

    #[test]
    fn take_removes_one_and_replace_rebuilds_the_set() {
        let mut s = CommentStore::new();
        s.add(comment("a.rs", 1, 1, "one"));
        s.add(comment("b.rs", 2, 2, "two"));
        let taken = s.take(0).unwrap();
        assert_eq!(taken.comment.text, "one");
        assert_eq!(s.len(), 1);
        assert!(s.take(5).is_none());
        s.replace(Vec::new());
        assert!(s.is_empty());
    }

    #[test]
    fn open_user_comments_filters_by_status_and_author() {
        let mut s = CommentStore::new();
        let open_user = s.add(comment("a.rs", 1, 1, "one"));
        let resolved_user = s.add(comment("a.rs", 2, 2, "two"));
        s.set_status(resolved_user, Status::Resolved);
        s.items.push(super::StoredComment {
            id: "c-1-aaaa".into(),
            author: Author::Agent,
            status: Status::Open,
            created_at: "2024-01-01T00:00:00Z".into(),
            comment: comment("a.rs", 3, 3, "agent"),
        });
        assert_eq!(s.open_user_comments().len(), 1);
        assert_eq!(s.open_user_comments()[0].comment.text, "one");
        assert_eq!(s.sendable(), 1);
        let _ = open_user;
    }
}
