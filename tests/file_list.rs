mod common;
use common::TempRepo;
use herdr_glowr::file_list::markdown_files;

#[test]
fn lists_markdown_newest_first_and_skips_non_markdown() {
    let repo = TempRepo::new();
    repo.write("old.md", "# old");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    repo.write("new.md", "# new");
    repo.write("code.rs", "fn main(){}");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "init"]);
    let files = markdown_files(repo.path(), false);
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["new.md", "old.md"]); // newest first, no code.rs
}
