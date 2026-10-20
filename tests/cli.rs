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
fn version_reports_the_package_version() {
    let output = assemblr(&["--version"]);

    assert!(output.status.success());
    assert_eq!(
        stdout(&output).trim(),
        format!("assemblr {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn missing_arguments_print_help_and_fail() {
    let output = assemblr(&[]);

    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("Usage: assemblr"));
}

#[test]
fn unknown_commands_and_arguments_are_rejected() {
    let output = assemblr(&["unit"]);
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
    assert!(stdout.contains("  registry "), "{stdout}");
    assert!(!stdout.contains("  unit "), "{stdout}");
}
