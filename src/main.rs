//! The `assemblr` command-line interface.

mod cli;

use std::process::ExitCode;

fn main() -> ExitCode {
    cli::run()
}
