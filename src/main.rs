//! Thin binary entry point.
//!
//! Full CLI dispatch (`comment`/`skill-path`/`skill-install`, `sidebar`,
//! `--resolve-plugin-config`) lands once `cli`, `sidebar`, and `config` grow past their
//! Task 1 stubs; for now every invocation launches the TUI.

fn main() -> std::process::ExitCode {
    match herdr_glowr::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("glowr: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
