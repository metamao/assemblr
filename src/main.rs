//! The `assemblr` command-line interface.

use clap::Parser;

/// Compose and maintain opinionated, reusable software units.
#[derive(Debug, Parser)]
#[command(name = "assemblr", version, arg_required_else_help = true)]
struct Cli {}

fn main() {
    Cli::parse();
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
