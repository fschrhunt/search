//! The `search` entry point: parse the command line and run it.

use search::cli::args::{self, Command};

#[tokio::main]
async fn main() {
    let command = match args::parse(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("search: {message}");
            eprintln!("{}", args::usage());
            std::process::exit(2);
        }
    };
    let code = search::cli::run::execute(command).await;
    std::process::exit(code);
}

/// Referenced so `Command` stays exported from the binary's own view.
#[allow(dead_code)]
fn _command_hint(_: Command) {}
