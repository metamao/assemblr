//! `assemblr unit`: authoring operations on the unit repository in the
//! current directory.
//!
//! The result goes to stdout. Diagnostics, and changes to the lockfile, go to
//! stderr.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use assemblr::adapters::{
    AmendFailure, AmendProblem, AmendState, BuildFailure, BuildProblem, Built, LockfileOutcome,
    ManifestFileError, PublishState, Registry, amend_unit, build_unit,
};
use assemblr::domain::{LockChange, LockError, ResolutionError, UnitIdentity};
use clap::{Args, Subcommand};

#[derive(Debug, Subcommand)]
pub(super) enum UnitCommand {
    /// Compose the unit in the current directory with its published
    /// dependencies into `build/`.
    Build,
    /// Copy a dependency's file to `amendments/` for editing, and declare
    /// the amendment in `manifest.jsonc`.
    Amend(AmendArgs),
}

#[derive(Debug, Args)]
pub(super) struct AmendArgs {
    /// The file to amend: a name, found in the `src/` of the direct
    /// dependencies at their locked versions, or a path in that `src/`.
    /// A template matches by its name with or without `.assemblr.tmpl`.
    file: String,
    /// Search only this direct dependency, such as `@owner/unit`.
    #[arg(long, value_name = "IDENTITY")]
    unit: Option<String>,
}

pub(super) fn run(command: UnitCommand) -> ExitCode {
    match command {
        UnitCommand::Build => build(),
        UnitCommand::Amend(args) => amend(&args),
    }
}

/// The current directory and the registry, which every authoring command
/// needs.
fn locate() -> Result<(PathBuf, Registry), ExitCode> {
    let root = std::env::current_dir().map_err(|error| {
        fail(
            &format!("could not read the current directory: {error}"),
            None,
        )
    })?;
    let registry = Registry::locate().map_err(|error| fail(&error.to_string(), None))?;
    Ok((root, registry))
}

fn amend(args: &AmendArgs) -> ExitCode {
    let unit = match args.unit.as_deref().map(UnitIdentity::parse) {
        None => None,
        Some(Ok(unit)) => Some(unit),
        Some(Err(error)) => return fail(&format!("--unit: {error}"), None),
    };
    let (root, registry) = match locate() {
        Ok(located) => located,
        Err(code) => return code,
    };
    match amend_unit(&root, &registry, &args.file, unit.as_ref()) {
        Ok(amended) => {
            let found = amended.file();
            let copy = if amended.kept_existing() {
                "kept the existing copy at"
            } else {
                "created"
            };
            println!(
                "Amended {found}: {copy} amendments/{} and declared it in manifest.jsonc.",
                found.path()
            );
            ExitCode::SUCCESS
        }
        Err(failure) => {
            let hint = amend_hint(&failure, &root);
            fail(&failure.to_string(), hint.as_deref())
        }
    }
}

/// What remains after a failed amendment, and what the author can do.
fn amend_hint(failure: &AmendFailure, root: &Path) -> Option<String> {
    let problems = match failure {
        AmendFailure::Write(error) => {
            return Some(match error.state() {
                AmendState::Unchanged => {
                    "nothing was changed; fix the problem and run the command again".to_owned()
                }
                AmendState::CopyWithoutDeclaration => "the copy was created but not declared in \
                                                       manifest.jsonc; run the same command again \
                                                       to declare it"
                    .to_owned(),
            });
        }
        AmendFailure::Rejected(problems) => problems,
    };
    let mut hint = "nothing was changed".to_owned();
    let advice = problems.iter().find_map(|problem| match problem {
        AmendProblem::Manifest(ManifestFileError::Missing { .. }) => Some(format!(
            "{} is not a unit repository's top level; run `assemblr unit amend` in the \
             directory that holds `manifest.jsonc`",
            root.display()
        )),
        AmendProblem::NoLockfile | AmendProblem::LockfileStale { .. } => {
            Some("run `assemblr unit build` first, which writes the lockfile".to_owned())
        }
        AmendProblem::Build(problem) => advice(problem, root),
        AmendProblem::Resolution(error) => advice(&BuildProblem::Resolution(error.clone()), root),
        _ => None,
    });
    if let Some(advice) = advice {
        hint.push_str("; ");
        hint.push_str(&advice);
    }
    Some(hint)
}

fn build() -> ExitCode {
    let (root, registry) = match locate() {
        Ok(located) => located,
        Err(code) => return code,
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
        BuildProblem::AmendmentBase(_) => {
            "a build never changes the locked version of an amended unit".to_owned()
        }
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
