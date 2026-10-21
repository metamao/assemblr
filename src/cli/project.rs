//! `assemblr init` and `assemblr compose`: consumer projects.
//!
//! The result goes to stdout. Diagnostics, and what a composition could
//! not carry or store, go to stderr.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use assemblr::adapters::{
    BuildProblem, ComposeFailure, ComposeProblem, Composed, InitProjectFailure, Registry,
    StoreError, WriteStep, compose_project, init_project,
};
use assemblr::domain::{LockChange, LockError, ResolutionError, UnitIdentity};
use clap::Args;

#[derive(Debug, Args)]
pub(super) struct InitArgs {
    /// The project's directory, created if it does not exist. Defaults to
    /// the current directory.
    directory: Option<PathBuf>,
}

pub(super) fn init(args: &InitArgs) -> ExitCode {
    let directory = args.directory.clone().unwrap_or_else(|| PathBuf::from("."));
    match init_project(&directory) {
        Ok(initialized) if initialized.created() => {
            println!(
                "Initialized a consumer project in {}: created {}assemblr.jsonc.",
                initialized.directory().display(),
                if initialized.directory_created() {
                    "the directory and "
                } else {
                    ""
                }
            );
            println!(
                "Next: declare units in assemblr.jsonc, or run `assemblr compose` once they are \
                 declared."
            );
            ExitCode::SUCCESS
        }
        Ok(initialized) => {
            println!(
                "{} is already a consumer project: assemblr.jsonc is in place; nothing changed.",
                initialized.directory().display()
            );
            ExitCode::SUCCESS
        }
        Err(failure) => {
            let hint = match &failure {
                InitProjectFailure::UnitRepository(_) => {
                    "nothing was changed; run `assemblr init` in a directory that is not a unit's \
                     repository"
                }
                _ => "nothing was changed",
            };
            fail(&failure.to_string(), Some(hint))
        }
    }
}

pub(super) fn compose() -> ExitCode {
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
    match compose_project(&root, &registry) {
        Ok(composed) => {
            report(&composed);
            ExitCode::SUCCESS
        }
        Err(failure) => {
            if let ComposeFailure::Rejected {
                reverted: Some(reverted),
                ..
            } = &failure
            {
                report_reverted(reverted);
            }
            let hint = hint(&failure, &root);
            fail(&failure.to_string(), Some(&hint))
        }
    }
}

fn report(composed: &Composed) {
    if let Some(reverted) = composed.reverted() {
        report_reverted(reverted);
    }
    for note in composed.notes() {
        eprintln!("warning: {note}");
    }
    if composed.unchanged() {
        println!(
            "Nothing to compose: the project already holds the declared composition ({}).",
            composed.base()
        );
        return;
    }
    let changes: Vec<String> = composed
        .changes()
        .iter()
        .map(|(identity, change)| describe(identity, change))
        .collect();
    if !changes.is_empty() {
        eprintln!("note: created assemblr-lock.json: {}", changes.join(", "));
    }
    println!(
        "Composed {} unit{} into {} file(s); the reconciliation base {} is stored in .assemblr/.",
        composed.units(),
        if composed.units() == 1 { "" } else { "s" },
        composed.files(),
        composed.base()
    );
}

fn report_reverted(reverted: &assemblr::adapters::Reverted) {
    eprintln!(
        "note: reverted a composition that was interrupted earlier: removed {} file(s) it \
             had written{}",
        reverted.removed().len(),
        if reverted.kept().is_empty() {
            String::new()
        } else {
            format!(
                ", kept {} whose content changed since ({})",
                reverted.kept().len(),
                reverted
                    .kept()
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    );
}

fn describe(identity: &UnitIdentity, change: &LockChange) -> String {
    match change {
        LockChange::Added(version) => format!("{identity} {version} (new)"),
        LockChange::Changed { from, to } => format!("{identity} {from} -> {to}"),
        LockChange::Retired(version) => format!("{identity} {version} (no longer required)"),
    }
}

/// What remains after a failure, and what the user can do about it.
fn hint(failure: &ComposeFailure, root: &Path) -> String {
    let problems = match failure {
        ComposeFailure::Write { step, .. } => {
            return match step {
                WriteStep::Store | WriteStep::Journal => {
                    "the project's files were not written; fix the problem and run `assemblr \
                     compose` again"
                        .to_owned()
                }
                WriteStep::File(_) | WriteStep::Lock => "the composition is recorded in \
                                                         .assemblr/journal.json; the next \
                                                         `assemblr compose` reverts what was \
                                                         written and starts over"
                    .to_owned(),
                WriteStep::Finish => "the composition is complete; remove .assemblr/journal.json \
                                      by hand, or the next `assemblr compose` will revert it"
                    .to_owned(),
            };
        }
        ComposeFailure::Rejected { problems, .. } => problems,
    };
    let mut hint = "the composition was rejected and the project was not changed".to_owned();
    let advice = problems.iter().find_map(|problem| match problem {
        ComposeProblem::NoDeclaration(_) => Some(format!(
            "{} is not a consumer project's top level; run `assemblr init` to create one, or \
             `assemblr compose` in the directory that holds `assemblr.jsonc`",
            root.display()
        )),
        ComposeProblem::Existing(_) => Some(
            "move the existing content away, or compose into an empty project; composing over \
             existing content is not supported yet"
                .to_owned(),
        ),
        ComposeProblem::Reserved(_) => Some(
            "a unit cannot deliver Assemblr's own files; the unit's author must move or remove \
             them"
                .to_owned(),
        ),
        ComposeProblem::Changed { .. } => Some(
            "revert assemblr.jsonc to the composed declaration, or wait for the composition \
             of changes, which a later increment adds"
                .to_owned(),
        ),
        ComposeProblem::Base(StoreError::Missing(_)) => Some(
            "the stored base in .assemblr/ is missing: restore it from version control; a \
             project cannot be reconciled without it"
                .to_owned(),
        ),
        ComposeProblem::Base(_) => Some(
            "restore .assemblr/ from version control; a base that does not verify is never used"
                .to_owned(),
        ),
        ComposeProblem::ForeignLockfile(_) => Some(
            "remove or move the unit's lockfile; `assemblr compose` writes the project's own"
                .to_owned(),
        ),
        ComposeProblem::Build(problem) => build_advice(problem),
        _ => None,
    });
    if let Some(advice) = advice {
        hint.push_str("; ");
        hint.push_str(&advice);
    }
    hint
}

/// Advice for the problems a unit build also reports.
fn build_advice(problem: &BuildProblem) -> Option<String> {
    Some(match problem {
        BuildProblem::Lock(LockError::ContentChanged { .. }) => {
            "published versions never change: restore the tag in the unit's repository".to_owned()
        }
        BuildProblem::Resolution(ResolutionError::Unavailable { .. }) => {
            "run `assemblr registry sync` to index newly published versions".to_owned()
        }
        BuildProblem::Resolution(ResolutionError::Missing { .. }) => {
            "register the unit's repository with `assemblr registry add <path>`".to_owned()
        }
        BuildProblem::Source { .. } => "if a registered repository was moved or deleted, \
                                        `assemblr registry remove <identity>` removes its \
                                        registration"
            .to_owned(),
        BuildProblem::Published { .. } => {
            "run `assemblr registry sync` to check the registered repositories again".to_owned()
        }
        BuildProblem::Merge { .. } => {
            "the units' amendments of one file must agree; the amenders' authors must change \
             the copies or expose the value as a parameter"
                .to_owned()
        }
        BuildProblem::Composition(_) => {
            "supply the missing values in assemblr.jsonc `options`, or change the selection"
                .to_owned()
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
