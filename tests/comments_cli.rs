//! Integration tests for the `comment` CLI subcommands, spawning the real binary against a
//! real git repo (`tests/common/mod.rs`).

mod common;
use common::TempRepo;
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_herdr-glowr")
}

#[test]
fn add_then_list_roundtrips_without_side() {
    let repo = TempRepo::new();
    let out = Command::new(bin())
        .current_dir(repo.path())
        .args(["comment", "add", "--file", "plan.md", "--start", "3", "--text", "fix this"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let id = String::from_utf8(out.stdout).unwrap();
    assert!(id.trim().starts_with("c-"));

    let list = Command::new(bin())
        .current_dir(repo.path())
        .args(["comment", "list", "--json"])
        .output()
        .unwrap();
    assert!(list.status.success());
    let json = String::from_utf8(list.stdout).unwrap();
    // On-disk (and CLI-echoed) JSON is compact — no space after the colon.
    assert!(json.contains("\"file\":\"plan.md\""), "compact json: {json}");
    assert!(!json.contains("\"side\""));
}

#[test]
fn add_rejects_unknown_side_flag() {
    let repo = TempRepo::new();
    let out = Command::new(bin())
        .current_dir(repo.path())
        .args(["comment", "add", "--file", "p.md", "--start", "1", "--side", "old", "--text", "x"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2)); // unknown flag -> usage error
}
