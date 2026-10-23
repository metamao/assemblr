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
    use clap::{Command, CommandFactory};

    use super::GUIDE;
    use crate::cli::Cli;

    /// Every `assemblr ...` command line quoted in `text` whose subcommands or
    /// long flags the CLI does not define.
    fn unknown_commands(text: &str, cli: &Command) -> Vec<String> {
        let mut unknown = Vec::new();
        for quoted in text.split('`').skip(1).step_by(2) {
            let Some(rest) = quoted.strip_prefix("assemblr ") else {
                continue;
            };
            let mut command = cli;
            for word in rest.split_whitespace() {
                if let Some(flag) = word.strip_prefix("--") {
                    let known = flag == "help"
                        || command
                            .get_arguments()
                            .any(|argument| argument.get_long() == Some(flag));
                    if !known {
                        unknown.push(quoted.to_owned());
                    }
                } else if word.chars().all(|c| c.is_ascii_lowercase()) {
                    // A plain word right after a command with subcommands
                    // must be one of them; anything else is an argument.
                    if command.has_subcommands() {
                        match command.find_subcommand(word) {
                            Some(sub) => command = sub,
                            None => {
                                unknown.push(quoted.to_owned());
                                break;
                            }
                        }
                    }
                } else if command.has_subcommands() && !word.starts_with('<') {
                    unknown.push(quoted.to_owned());
                    break;
                }
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
                    `assemblr compose --resume`, and `assemblr add @acme/web --option a=b`.";
        assert_eq!(
            unknown_commands(text, &Cli::command()),
            [
                "assemblr frobnicate",
                "assemblr unit shrink",
                "assemblr compose --resume"
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
