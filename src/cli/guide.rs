//! `assemblr guide`: how a coding agent uses Assemblr, for this version.
//!
//! The text is `guide.md`, compiled into the binary, so that it always
//! describes the commands that the installed version has. Instructions
//! installed elsewhere, such as a project's `AGENTS.md`, only point here, and
//! the command's name is a compatibility promise for them.

use std::io::{self, Write};
use std::process::ExitCode;

use super::diagnostic::fail;

const GUIDE: &str = include_str!("guide.md");

pub(super) fn run() -> ExitCode {
    match io::stdout().lock().write_all(GUIDE.as_bytes()) {
        // A reader that stops early, like `head`, has what it wanted.
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(error) => fail(&format!("could not print the guide: {error}"), None),
    }
}

#[cfg(test)]
mod tests {
    use clap::error::ErrorKind;
    use clap::{Command, CommandFactory};

    use super::GUIDE;
    use crate::cli::Cli;

    /// Every `assemblr ...` command line quoted in `text` that the CLI's own
    /// parser rejects: an unknown subcommand, flag, or argument, or a flag
    /// on the wrong command. A placeholder such as `<path or URL>` stands for
    /// one value, a line without a command's required arguments still names
    /// it, and values themselves are not checked.
    fn unknown_commands(text: &str, cli: &Command) -> Vec<String> {
        let mut unknown = Vec::new();
        for quoted in text.split('`').skip(1).step_by(2) {
            let Some(rest) = quoted.strip_prefix("assemblr ") else {
                continue;
            };
            if rest.starts_with('<') {
                continue; // `assemblr <command> --help` names no command.
            }
            let mut words = vec!["assemblr".to_owned()];
            let mut placeholder = false;
            for word in rest.split_whitespace() {
                if placeholder {
                    placeholder = !word.ends_with('>');
                } else if word.starts_with('<') {
                    words.push("value".to_owned());
                    placeholder = !word.ends_with('>');
                } else {
                    words.push(word.to_owned());
                }
            }
            let accepted = match cli.clone().try_get_matches_from(words) {
                Ok(_) => true,
                Err(error) => matches!(
                    error.kind(),
                    ErrorKind::DisplayHelp
                        | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
                        | ErrorKind::MissingRequiredArgument
                        | ErrorKind::MissingSubcommand
                        | ErrorKind::InvalidValue
                        | ErrorKind::ValueValidation
                ),
            };
            if !accepted {
                unknown.push(quoted.to_owned());
            }
        }
        unknown
    }

    #[test]
    fn every_command_the_guide_names_exists() {
        let unknown = unknown_commands(GUIDE, &Cli::command());
        assert!(unknown.is_empty(), "the guide names {unknown:#?}");
    }

    #[test]
    fn a_command_or_flag_that_does_not_exist_is_reported() {
        let text = "Run `assemblr frobnicate`, `assemblr unit shrink`, \
                    `assemblr compose --resume`, `assemblr compose continue`, \
                    `assemblr compose -c`, `assemblr unit build --continue`, \
                    `assemblr add @acme/web --option a=b`, `assemblr registry add <path or URL>`, \
                    `assemblr add`, and `assemblr <command> --help`.";
        assert_eq!(
            unknown_commands(text, &Cli::command()),
            [
                "assemblr frobnicate",
                "assemblr unit shrink",
                "assemblr compose --resume",
                "assemblr compose continue",
                "assemblr compose -c",
                "assemblr unit build --continue",
            ]
        );
    }

    #[test]
    fn the_guide_is_printed_as_plain_text() {
        // The guide is printed as it is; a template marker would show.
        assert!(!GUIDE.contains("{{"), "{GUIDE}");
        assert!(GUIDE.ends_with('\n'));
    }
}
