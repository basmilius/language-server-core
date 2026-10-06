//! The process of a language server: its arguments, and LSP over stdin and stdout.

use std::process::ExitCode;

use lsp_server::Connection;

use crate::BoxError;

/// The stack of the thread the server runs on. A recursive descent parser recurses as deeply as
/// the input nests, and the default stack of a spawned thread is small.
const STACK_SIZE: usize = 64 << 20;

/// What the arguments ask for.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Serve,
    Print(String),
    Refuse(String),
}

fn command(mut args: impl Iterator<Item = String>, name: &str, version: &str, usage: &str) -> Command {
    match args.next().as_deref() {
        None | Some("--stdio") => Command::Serve,
        Some("--version" | "-V") => Command::Print(format!("{name} {version}")),
        Some("--help" | "-h") => Command::Print(usage.to_string()),
        Some(other) => Command::Refuse(format!("unknown argument '{other}'\n\n{usage}")),
    }
}

/// The `main` of a language server called `name`: `--version`, `--help`, and otherwise (or with
/// `--stdio`) `run` on a connection over stdin and stdout, on a thread with a large stack. Exits
/// with 2 for an argument it does not know and 1 when the server fails.
pub fn run_stdio(
    name: &str,
    version: &str,
    usage: &str,
    run: impl FnOnce(Connection) -> Result<(), BoxError> + Send + 'static,
) -> ExitCode {
    match command(std::env::args().skip(1), name, version, usage) {
        Command::Serve => {}
        Command::Print(text) => {
            println!("{text}");
            return ExitCode::SUCCESS;
        }
        Command::Refuse(text) => {
            eprintln!("{text}");
            return ExitCode::from(2);
        }
    }
    let (connection, threads) = Connection::stdio();
    let worker = std::thread::Builder::new()
        .name(name.to_string())
        .stack_size(STACK_SIZE)
        .spawn(move || run(connection));
    let result = match worker {
        Ok(handle) => handle
            .join()
            .unwrap_or_else(|_| Err("the server thread panicked".into())),
        Err(error) => Err(error.to_string().into()),
    };
    let io_result = threads.join();
    match (result, io_result) {
        (Ok(()), Ok(())) => ExitCode::SUCCESS,
        (Err(error), _) => {
            eprintln!("{name}: {error}");
            ExitCode::FAILURE
        }
        (_, Err(error)) => {
            eprintln!("{name}: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const USAGE: &str = "sql-language-server [--stdio]";

    fn command_of(args: &[&str]) -> Command {
        command(
            args.iter().map(|arg| arg.to_string()),
            "sql-language-server",
            "1.2.3",
            USAGE,
        )
    }

    #[test]
    fn serves_without_arguments_and_with_stdio() {
        assert_eq!(command_of(&[]), Command::Serve);
        assert_eq!(command_of(&["--stdio"]), Command::Serve);
    }

    #[test]
    fn prints_the_version_and_the_usage() {
        let version = Command::Print("sql-language-server 1.2.3".to_string());
        assert_eq!(command_of(&["--version"]), version);
        assert_eq!(command_of(&["-V"]), version);
        assert_eq!(command_of(&["--help"]), Command::Print(USAGE.to_string()));
        assert_eq!(command_of(&["-h"]), Command::Print(USAGE.to_string()));
    }

    #[test]
    fn refuses_an_argument_it_does_not_know_with_the_usage() {
        assert_eq!(
            command_of(&["--tcp"]),
            Command::Refuse(format!("unknown argument '--tcp'\n\n{USAGE}"))
        );
    }
}
