//! Thin binary entry point: `--resolve-plugin-config`, the agent CLI (`comment`,
//! `skill-path`, `skill-install`), the standalone `sidebar` launcher, or — with no recognized
//! subcommand — the TUI itself.

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();

    if args.get(1).map(String::as_str) == Some("--resolve-plugin-config") {
        if let Err(error) = herdr_glowr::config::print_plugin_config() {
            eprintln!("glowr: {error}");
            return std::process::ExitCode::FAILURE;
        }
        return std::process::ExitCode::SUCCESS;
    }

    if matches!(args.get(1).map(String::as_str), Some("comment" | "skill-path" | "skill-install")) {
        return herdr_glowr::cli::run(args);
    }

    if args.get(1).map(String::as_str) == Some("sidebar") {
        return herdr_glowr::sidebar::run(&args[2..]);
    }

    match herdr_glowr::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("glowr: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
