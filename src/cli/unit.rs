//! `assemblr unit`: authoring operations on the unit repository in the
//! current directory.
//!
//! The result goes to stdout. Diagnostics, and changes to the lockfile, go to
//! stderr.

use std::path::Path;
use std::process::ExitCode;

use assemblr::adapters::{
    BuildFailure, BuildProblem, Built, LockfileOutcome, ManifestFileError, PublishState, Registry,
    build_unit,
};
use assemblr::domain::{LockChange, LockError, ResolutionError, UnitIdentity};
use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub(super) enum UnitCommand {
    /// Compose the unit in the current directory with its published
    /// dependencies into `build/`.
    Build,
}

pub(super) fn run(command: UnitCommand) -> ExitCode {
    match command {
        UnitCommand::Build => build(),
    }
}

fn build() -> ExitCode {
    let root = match std::env::current_dir() {
        Ok(root) => root,
        Err(error) => {
            return fail(
                &format!("could not read the current directory: {error}"),
                None,
            );
        }
    };
    let registry = match Registry::locate() {
        Ok(registry) => registry,
        Err(error) => return fail(&error.to_string(), None),
    };
    match build_unit(&root, &registry) {
        Ok(built) => {
            report_lockfile(&built);
            if let Some((path, error)) = built.leftover() {
                eprintln!(
                    "warning: could not delete the previous build at {}: {error}; the next \
                     build deletes it",
                    path.display()
                );
            }
            println!(
                "Built {} {} into build/: {} file(s), with {} dependenc{}.",
                built.unit(),
                built.version(),
                built.files(),
                built.dependencies(),
                if built.dependencies() == 1 {
                    "y"
                } else {
                    "ies"
                }
            );
            ExitCode::SUCCESS
        }
        Err(failure) => {
            let hint = hint(&failure, &root);
            fail(&failure.to_string(), Some(&hint))
        }
    }
}

/// Reports on stderr how the lockfile changed.
fn report_lockfile(built: &Built) {
    let changes: Vec<String> = built
        .changes()
        .iter()
        .map(|(identity, change)| describe(identity, change))
        .collect();
    match built.lockfile() {
        LockfileOutcome::Unchanged => {}
        LockfileOutcome::Created if changes.is_empty() => {
            eprintln!("note: created assemblr-lock.json, with no dependencies to lock");
        }
        LockfileOutcome::Created => {
            eprintln!("note: created assemblr-lock.json: {}", changes.join(", "));
        }
        LockfileOutcome::Updated => {
            eprintln!("note: updated assemblr-lock.json: {}", changes.join(", "));
        }
    }
}

fn describe(identity: &UnitIdentity, change: &LockChange) -> String {
    match change {
        LockChange::Added(version) => format!("{identity} {version} (new)"),
        LockChange::Changed { from, to } => format!("{identity} {from} -> {to}"),
        LockChange::Retired(version) => format!("{identity} {version} (no longer required)"),
    }
}

/// What remains after a failure, and what the author can do about it.
fn hint(failure: &BuildFailure, root: &Path) -> String {
    let problems = match failure {
        BuildFailure::Publish(error) => {
            return match error.state() {
                PublishState::Unchanged => {
                    "nothing was changed; fix the problem and run `assemblr unit build` again"
                        .to_owned()
                }
                PublishState::BuildRemoved | PublishState::LockfileWithoutBuild => {
                    "there is no build/ now; run `assemblr unit build` again".to_owned()
                }
            };
        }
        BuildFailure::Rejected(problems) => problems,
    };
    let mut hint =
        "the build was rejected; build/ and assemblr-lock.json were not changed".to_owned();
    if let Some(advice) = problems.iter().find_map(|problem| advice(problem, root)) {
        hint.push_str("; ");
        hint.push_str(&advice);
    }
    hint
}

/// What deleting the lockfile does, which is not a repair.
const DELETING_THE_LOCKFILE: &str = "deleting assemblr-lock.json is not a repair: it discards \
     every locked version, and the next build resolves every dependency again, possibly to \
     other versions";

/// Advice for the first problem that has one.
fn advice(problem: &BuildProblem, root: &Path) -> Option<String> {
    Some(match problem {
        BuildProblem::Manifest(ManifestFileError::Missing { .. }) => format!(
            "{} is not a unit repository's top level; run `assemblr unit build` in the \
             directory that holds `manifest.jsonc`",
            root.display()
        ),
        BuildProblem::Lockfile(_) => format!("fix assemblr-lock.json; {DELETING_THE_LOCKFILE}"),
        BuildProblem::Lock(LockError::ContentChanged { .. }) => format!(
            "published versions never change: restore the tag in the unit's repository; \
             {DELETING_THE_LOCKFILE}"
        ),
        BuildProblem::Resolution(ResolutionError::Unavailable { .. }) => {
            "run `assemblr registry sync` to index newly published versions".to_owned()
        }
        BuildProblem::Resolution(ResolutionError::Missing { .. }) => {
            "register the dependency's repository with `assemblr registry add <path>`".to_owned()
        }
        BuildProblem::Source { .. } => "if a registered repository was moved or deleted, \
                                        `assemblr registry remove <identity>` removes its \
                                        registration"
            .to_owned(),
        BuildProblem::Published { .. } => {
            "run `assemblr registry sync` to check the registered repositories again".to_owned()
        }
        _ => return None,
    })
}

fn fail(message: &str, hint: Option<&str>) -> ExitCode {
    eprintln!("error: {}", message.replace('\n', "\nerror: "));
    if let Some(hint) = hint {
        eprintln!("hint: {hint}");
    }
    ExitCode::FAILURE
}
