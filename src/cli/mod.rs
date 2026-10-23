//! Argument parsing and dispatch.

mod advice;
mod diagnostic;
mod info;
mod project;
mod registry;
mod search;
mod unit;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// Compose and maintain projects from reusable, versioned units
///
/// A unit is a versioned, reusable part of a project, kept in its own Git
/// repository. Authors create and publish units with `assemblr unit`;
/// projects select units in `assemblr.jsonc` and compose them.
#[derive(Debug, Parser)]
#[command(
    name = "assemblr",
    version = env!("ASSEMBLR_VERSION"),
    arg_required_else_help = true,
    after_long_help = "Start a project:\n  \
        assemblr registry add ../web      register a unit's repository\n  \
        assemblr init                     create assemblr.jsonc here\n  \
        assemblr add @acme/web            select the unit and compose the project\n\n\
        Create a unit:\n  \
        assemblr unit init @acme/web      create web/ with a manifest\n  \
        assemblr unit build               build it into build/\n  \
        assemblr unit publish             tag the committed version\n\n\
        Run `assemblr <command> --help` for details."
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create a project's assemblr.jsonc
    ///
    /// The project starts with no units; `assemblr add` selects them.
    Init(project::InitArgs),
    /// Compose the project from assemblr.jsonc
    ///
    /// Writes the files of the units that `assemblr.jsonc` selects, keeping
    /// your own changes to them. A change on both sides of a text file is
    /// left as conflict markers: resolve them and run `assemblr compose
    /// --continue`, or run `assemblr compose --abort` to undo the
    /// composition.
    #[command(after_long_help = "Examples:\n  \
        assemblr compose\n  \
        assemblr compose --continue")]
    Compose(project::ComposeArgs),
    /// Add a unit to the project and compose it
    ///
    /// Selects the unit in `assemblr.jsonc` and composes the project.
    /// Without a version requirement, the unit is recorded with `^` on the
    /// version chosen. A unit with a required parameter needs `--option`.
    #[command(after_long_help = "Examples:\n  \
        assemblr add @acme/web\n  \
        assemblr add @acme/web@~1.2\n  \
        assemblr add @acme/web --option appName=shop --option port=8080")]
    Add(project::AddArgs),
    /// Remove a unit from the project and compose it
    ///
    /// Removes the unit and its options from `assemblr.jsonc`, then composes
    /// the project, which removes the unit's files.
    #[command(after_long_help = "Example:\n  assemblr remove @acme/web")]
    Remove(project::RemoveArgs),
    /// Update units to the newest versions allowed and compose
    ///
    /// Moves the versions in `assemblr-lock.json` within the requirements in
    /// `assemblr.jsonc`, then composes the project. `assemblr.jsonc` is not
    /// changed.
    #[command(after_long_help = "Examples:\n  \
        assemblr update\n  \
        assemblr update @acme/web")]
    Update(project::UpdateArgs),
    /// Show a registered unit's versions, parameters, and more
    ///
    /// Reads the local registry, offline. Shows the unit's description,
    /// versions, dependencies, parameters, options, and amendments.
    #[command(after_long_help = "Examples:\n  \
        assemblr info @acme/web\n  \
        assemblr info @acme/web@1.2.0 --json")]
    Info(info::InfoArgs),
    /// Find registered units by name or description
    ///
    /// Lists the units whose name or description contains every term,
    /// ignoring case. Reads the local registry, offline.
    #[command(after_long_help = "Examples:\n  \
        assemblr search cloudflare\n  \
        assemblr search cloudflare worker --json")]
    Search(search::SearchArgs),
    /// Manage the local registry of unit repositories
    #[command(subcommand)]
    Registry(registry::RegistryCommand),
    /// Create, build, and publish units
    #[command(subcommand)]
    Unit(unit::UnitCommand),
}

/// Parses the arguments and runs the command.
pub(crate) fn run() -> ExitCode {
    match Cli::parse().command {
        Command::Init(args) => project::init(&args),
        Command::Compose(args) => project::compose(&args),
        Command::Add(args) => project::add(&args),
        Command::Remove(args) => project::remove(&args),
        Command::Update(args) => project::update(&args),
        Command::Info(args) => info::run(&args),
        Command::Search(args) => search::run(&args),
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

    /// Every summary fits the command list and `-h`, as
    /// `docs/user-facing-copy.md` asks.
    #[test]
    fn every_summary_is_one_short_line() {
        fn check(command: &clap::Command, path: &str, long: &mut Vec<String>) {
            for sub in command.get_subcommands() {
                let path = format!("{path} {}", sub.get_name());
                if let Some(about) = sub.get_about() {
                    let about = about.to_string();
                    if about.chars().count() > 60 || about.ends_with('.') {
                        long.push(format!("{path}: {about}"));
                    }
                }
                check(sub, &path, long);
            }
        }
        let mut long = Vec::new();
        check(&Cli::command(), "assemblr", &mut long);
        assert!(long.is_empty(), "{long:#?}");
    }
}
