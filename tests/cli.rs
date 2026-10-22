//! End-to-end checks of the `assemblr` executable's entry point.

use std::process::{Command, Output};

fn assemblr(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_assemblr"))
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("failed to run assemblr: {error}"))
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn help_describes_the_tool() {
    let output = assemblr(&["--help"]);

    assert!(output.status.success());
    let stdout = stdout(&output);
    assert!(stdout.contains("Usage: assemblr"), "{stdout}");
    assert!(
        stdout.contains("Compose and maintain opinionated, reusable software units"),
        "{stdout}"
    );
}

#[test]
fn version_reports_the_package_version_and_the_source_commit() {
    let output = assemblr(&["--version"]);

    assert!(output.status.success());
    // The commit is absent when the build had neither Git nor ASSEMBLR_COMMIT.
    let expected = format!(
        r"^assemblr {}( \([0-9a-zA-Z]{{1,12}}\))?$",
        regex_lite::escape(env!("CARGO_PKG_VERSION"))
    );
    let re = regex_lite::Regex::new(&expected).expect("valid pattern");
    assert!(re.is_match(stdout(&output).trim()), "{}", stdout(&output));
}

#[test]
fn missing_arguments_print_help_and_fail() {
    let output = assemblr(&[]);

    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("Usage: assemblr"));
}

#[test]
fn unknown_commands_and_arguments_are_rejected() {
    let output = assemblr(&["unit", "inspect"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("unrecognized subcommand"));

    let output = assemblr(&["--unknown"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("unexpected argument"));
}

#[test]
fn help_offers_only_implemented_commands() {
    let output = assemblr(&["--help"]);

    let stdout = stdout(&output);
    assert!(stdout.contains("  init "), "{stdout}");
    assert!(stdout.contains("  compose "), "{stdout}");
    assert!(stdout.contains("  registry "), "{stdout}");
    assert!(stdout.contains("  unit "), "{stdout}");

    let output = assemblr(&["unit", "--help"]);
    let help = self::stdout(&output);
    let commands: Vec<&str> = help
        .lines()
        .skip_while(|line| *line != "Commands:")
        .skip(1)
        .take_while(|line| !line.is_empty())
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert_eq!(
        commands,
        [
            "init", "build", "check", "publish", "amend", "update", "help"
        ],
        "{help}"
    );

    let output = assemblr(&["unit", "amend", "--help"]);
    let help = self::stdout(&output);
    assert!(help.contains("<FILE>"), "{help}");
    assert!(help.contains("--unit <IDENTITY>"), "{help}");

    let output = assemblr(&["unit", "update", "--help"]);
    let help = self::stdout(&output);
    assert!(help.contains("[IDENTITY]..."), "{help}");

    let output = assemblr(&["unit", "init", "--help"]);
    let help = self::stdout(&output);
    assert!(help.contains("<IDENTITY> [DIRECTORY]"), "{help}");
    assert!(help.contains("--description <TEXT>"), "{help}");
}
