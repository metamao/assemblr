//! Argument parsing and dispatch.

mod registry;
mod unit;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// Compose and maintain opinionated, reusable software units.
#[derive(Debug, Parser)]
#[command(name = "assemblr", version, arg_required_else_help = true)]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Manage the local registry of unit repositories.
    #[command(subcommand)]
    Registry(registry::RegistryCommand),
    /// Author the unit in the current directory.
    #[command(subcommand)]
    Unit(unit::UnitCommand),
}

/// Parses the arguments and runs the command.
pub(crate) fn run() -> ExitCode {
    match Cli::parse().command {
        Command::Registry(command) => registry::run(command),
        Command::Unit(command) => unit::run(command),
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::Cli;

    #[test]
    fn command_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
