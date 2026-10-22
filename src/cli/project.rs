//! `assemblr init`, `compose`, `add`, and `remove`: consumer projects.
//!
//! The result goes to stdout. Diagnostics, and what a composition could
//! not carry or store, go to stderr.

use super::advice::{Audience, composition_advice, cross_version_advice, distinct, merge_advice};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use assemblr::adapters::{
    BuildProblem, ComposeFailure, ComposeProblem, ComposeRecovery, Composed, CrossVersionReason,
    DeclarationChange, FileChange, InitProjectFailure, Registry, StoreError, WriteStep,
    abort_compose, add_unit, compose_project, continue_compose, init_project, remove_unit,
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

#[derive(Debug, Args)]
pub(super) struct AddArgs {
    /// The unit to select, `@owner/name`, optionally followed by `@` and a
    /// version requirement, as in `@owner/name@~1.2`. Without one, the
    /// unit is recorded with a caret requirement on the version the
    /// resolution picks.
    unit: String,
}

#[derive(Debug, Args)]
pub(super) struct RemoveArgs {
    /// The explicitly selected unit to remove, `@owner/name`.
    unit: String,
}

/// `@owner/name` and the requirement after a second `@`, if any.
pub(super) fn split_unit(argument: &str) -> Result<(UnitIdentity, Option<String>), String> {
    let (identity, requirement) = match argument.get(1..).and_then(|rest| rest.find('@')) {
        Some(at) => (&argument[..=at], Some(argument[at + 2..].to_owned())),
        None => (argument, None),
    };
    if requirement.as_deref() == Some("") {
        return Err(format!(
            "`{argument}` ends with `@` and names no version requirement"
        ));
    }
    identity
        .parse::<UnitIdentity>()
        .map(|identity| (identity, requirement))
        .map_err(|error| format!("`{identity}` is not a unit identity: {error}"))
}

pub(super) fn add(args: &AddArgs) -> ExitCode {
    let (identity, requirement) = match split_unit(&args.unit) {
        Ok(parsed) => parsed,
        Err(message) => {
            return fail(
                &message,
                Some("write the unit as @owner/name, or @owner/name@<requirement>"),
            );
        }
    };
    run(Some(&identity), |root, registry| {
        add_unit(root, registry, &identity, requirement.as_deref())
    })
}

pub(super) fn remove(args: &RemoveArgs) -> ExitCode {
    let (identity, requirement) = match split_unit(&args.unit) {
        Ok(parsed) => parsed,
        Err(message) => return fail(&message, Some("write the unit as @owner/name")),
    };
    if requirement.is_some() {
        return fail(
            &format!("`{}` names a version requirement", args.unit),
            Some("`assemblr remove` takes the unit only: @owner/name"),
        );
    }
    run(Some(&identity), |root, registry| {
        remove_unit(root, registry, &identity)
    })
}

#[derive(Debug, Args)]
pub(super) struct ComposeArgs {
    /// Finalize a composition that left conflicts in files, once you have
    /// resolved them: no conflict marker may remain.
    #[arg(long = "continue", conflicts_with = "abort")]
    resume: bool,
    /// Give up a composition that left conflicts in files: restore the
    /// project, `assemblr.jsonc` included, to what it held before.
    #[arg(long)]
    abort: bool,
}

pub(super) fn compose(args: &ComposeArgs) -> ExitCode {
    if args.resume {
        return settle(|root| continue_compose(root).map(|continued| report_continued(&continued)));
    }
    if args.abort {
        return settle(|root| abort_compose(root).map(|recovery| report_aborted(&recovery)));
    }
    run(None, compose_project)
}

/// Runs `--continue` or `--abort` on the project in the current directory.
fn settle(operation: impl FnOnce(&Path) -> Result<(), ComposeFailure>) -> ExitCode {
    let root = match std::env::current_dir() {
        Ok(root) => root,
        Err(error) => {
            return fail(
                &format!("could not read the current directory: {error}"),
                None,
            );
        }
    };
    match operation(&root) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            let hint = hint(&failure, &root);
            fail(&failure.to_string(), Some(&hint))
        }
    }
}

fn report_continued(continued: &assemblr::adapters::Continued) {
    if continued.completed() {
        println!(
            "The composition had been continued already; removed its journal. The \
             reconciliation base is {}.",
            continued.base()
        );
        return;
    }
    println!(
        "Resolved {} file(s) and finalized the composition; the reconciliation base {} is \
         stored in .assemblr/ and assemblr-lock.json is updated.",
        continued.files().len(),
        continued.base()
    );
    for path in continued.files() {
        println!("  resolved {path}");
    }
}

fn report_aborted(recovery: &ComposeRecovery) {
    match recovery {
        ComposeRecovery::Completed => println!(
            "The composition had been continued already, so there is nothing to abort; removed \
             its journal."
        ),
        ComposeRecovery::Reverted {
            removed,
            restored,
            kept,
        } => {
            println!(
                "Aborted the composition: removed {} file(s) it had created and restored {} it \
                 had changed or removed, assemblr.jsonc included where it was rewritten.",
                removed.len(),
                restored.len()
            );
            for path in kept {
                eprintln!(
                    "warning: kept {path}: it changed since the composition wrote it, so it is \
                     yours now"
                );
            }
        }
    }
}

/// Runs `operation` on the project in the current directory and reports
/// it. `unit` is the unit `add` or `remove` was given.
fn run(
    unit: Option<&UnitIdentity>,
    operation: impl FnOnce(&Path, &Registry) -> Result<Composed, ComposeFailure>,
) -> ExitCode {
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
    match operation(&root, &registry) {
        Ok(composed) => {
            report_declaration(unit, &composed);
            report(&composed);
            if composed.pending().is_empty() {
                ExitCode::SUCCESS
            } else {
                fail(
                    &format!(
                        "{} file(s) hold conflicts between your changes and the composition",
                        composed.pending().len()
                    ),
                    Some(
                        "resolve the conflict markers in each file, then run `assemblr compose \
                         --continue`; `assemblr compose --abort` restores the project instead",
                    ),
                )
            }
        }
        Err(failure) => {
            if let ComposeFailure::Rejected {
                recovered: Some(recovered),
                ..
            } = &failure
            {
                report_recovered(recovered);
            }
            let hint = hint(&failure, &root);
            fail(&failure.to_string(), Some(&hint))
        }
    }
}

fn report_declaration(unit: Option<&UnitIdentity>, composed: &Composed) {
    let (Some(unit), Some(change)) = (unit, composed.declaration()) else {
        return;
    };
    match change {
        DeclarationChange::Added { requirement } => {
            println!("Selected {unit} ({requirement}) in assemblr.jsonc.");
        }
        DeclarationChange::Replaced { from, to } => {
            println!("Changed the requirement of {unit} from {from} to {to} in assemblr.jsonc.")
        }
        DeclarationChange::Kept { requirement } => {
            println!("{unit} is already selected ({requirement}); assemblr.jsonc is unchanged.")
        }
        DeclarationChange::Removed { options } => println!(
            "Removed {unit}{} from assemblr.jsonc.",
            if *options { " and its options" } else { "" }
        ),
    }
}

fn report(composed: &Composed) {
    if let Some(recovered) = composed.recovered() {
        report_recovered(recovered);
    }
    for note in composed.notes() {
        eprintln!("warning: {note}");
    }
    if !composed.pending().is_empty() {
        report_pending(composed);
        return;
    }
    // A declaration that changed without changing the composition, as when
    // a unit that is in the composition already is selected.
    let declaration_only = composed.declaration().is_some()
        && composed.paths().is_empty()
        && composed.changes().is_empty()
        && !composed.first();
    if composed.unchanged() || declaration_only {
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
        eprintln!(
            "note: {} assemblr-lock.json: {}",
            if composed.first() {
                "created"
            } else {
                "updated"
            },
            changes.join(", ")
        );
    }
    if composed.first() {
        println!(
            "Composed {} unit{} into {} file(s); the reconciliation base {} is stored in \
             .assemblr/.",
            composed.units(),
            if composed.units() == 1 { "" } else { "s" },
            composed.files(),
            composed.base()
        );
        return;
    }
    let count = |change: FileChange| {
        composed
            .paths()
            .values()
            .filter(|found| **found == change)
            .count()
    };
    println!(
        "Reconciled the project with {} unit{} ({} file(s)): created {}, modified {}, merged {}, \
         removed {}; the reconciliation base {} is stored in .assemblr/.",
        composed.units(),
        if composed.units() == 1 { "" } else { "s" },
        composed.files(),
        count(FileChange::Created),
        count(FileChange::Modified),
        count(FileChange::Merged),
        count(FileChange::Removed),
        composed.base()
    );
    for (path, change) in composed.paths() {
        println!("  {} {path}", describe_change(*change));
    }
}

/// A composition that waits for the consumer: the lock is not written yet.
fn report_pending(composed: &Composed) {
    println!(
        "Reconciled the project with {} unit{} ({} file(s)), but {} file(s) hold conflicts; the \
         rest of the composition is applied and its lock waits for you:",
        composed.units(),
        if composed.units() == 1 { "" } else { "s" },
        composed.files(),
        composed.pending().len(),
    );
    for (path, change) in composed.paths() {
        println!("  {} {path}", describe_change(*change));
    }
    println!(
        "Each conflicted file holds your content, the last composition's, and the declared \
         composition's between <<<<<<<, |||||||, =======, and >>>>>>> lines."
    );
}

fn describe_change(change: FileChange) -> &'static str {
    match change {
        FileChange::Created => "created",
        FileChange::Modified => "modified",
        FileChange::Merged => "merged",
        FileChange::Removed => "removed",
        FileChange::Conflicted => "conflict",
    }
}

fn report_recovered(recovered: &ComposeRecovery) {
    match recovered {
        ComposeRecovery::Completed => eprintln!(
            "note: completed a composition that was interrupted earlier, after its files and \
             lock were written: removed its journal"
        ),
        ComposeRecovery::Reverted {
            removed,
            restored,
            kept,
        } => eprintln!(
            "note: reverted a composition that was interrupted earlier: removed {} file(s) it \
             had created, restored {} it had changed or removed{}",
            removed.len(),
            restored.len(),
            if kept.is_empty() {
                String::new()
            } else {
                format!(
                    ", kept {} whose content changed since ({})",
                    kept.len(),
                    kept.iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        ),
    }
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
                WriteStep::Store | WriteStep::Previous | WriteStep::Journal => {
                    "the project's files were not changed; fix the problem and run `assemblr \
                     compose` again"
                        .to_owned()
                }
                WriteStep::File(_)
                | WriteStep::Declaration
                | WriteStep::Pending
                | WriteStep::Lock => {
                    "the composition is recorded in .assemblr/journal.json; the next `assemblr \
                     compose` reverts what was changed, assemblr.jsonc included, and starts over"
                        .to_owned()
                }
                WriteStep::Finish => "the composition is complete except for its journal; the \
                                      next `assemblr compose` removes it"
                    .to_owned(),
            };
        }
        ComposeFailure::Rejected { problems, .. } => problems,
    };
    let mut hint = "the composition was rejected and the project was not changed".to_owned();
    let advice = distinct(problems.iter().filter_map(|problem| {
        match problem {
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
        ComposeProblem::LocallyIgnored(_) => Some(
            "whether a file enters the tracked base must not depend on one machine's rules: add \
             the file to the project's .gitignore to keep it out of the base for everyone, or \
             re-include it there with a `!` pattern (a directory with `!dir/`) to store it, or \
             remove your own rule"
                .to_owned(),
        ),
        ComposeProblem::NotSelected { transitive, .. } => Some(if *transitive {
            "remove the selected unit that requires it, or select it with `assemblr add` first"
                .to_owned()
        } else {
            "assemblr.jsonc lists the units this project selects".to_owned()
        }),
        ComposeProblem::InvalidRequirement(_) => Some(
            "write a SemVer requirement such as ^1.2.0, ~1.2, or =1.2.3 after the unit: \
             @owner/name@^1.2.0"
                .to_owned(),
        ),
        ComposeProblem::CannotEdit(_) => {
            Some("edit assemblr.jsonc by hand and run `assemblr compose`".to_owned())
        }
        ComposeProblem::Reserved(_) => Some(
            "a unit cannot deliver Assemblr's own files; the unit's author must move or remove \
             them"
                .to_owned(),
        ),
        ComposeProblem::Conflicts(_) => Some(
            "for each file, make the project's content the composed one or restore the \
             composed content, or change the declaration; conflicts between text content are \
             left in the files to resolve, but not together with conflicts of other kinds"
                .to_owned(),
        ),
        ComposeProblem::Pending(_) => Some(
            "resolve the conflict markers in those files and run `assemblr compose --continue`, \
             or run `assemblr compose --abort` to restore the project"
                .to_owned(),
        ),
        ComposeProblem::Unresolved(_) => Some(
            "edit each file to the content you want, with no <<<<<<< current or >>>>>>> incoming \
             line left, then run `assemblr compose --continue` again"
                .to_owned(),
        ),
        ComposeProblem::NothingPending | ComposeProblem::NotPending => Some(
            "`assemblr compose` completes or reverts an interrupted composition, and composes the \
             declaration"
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
    }
    }));
    if !advice.is_empty() {
        hint.push_str("; ");
        hint.push_str(&advice.join("; "));
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
        BuildProblem::Merge {
            unit,
            failure,
            crossed,
            ..
        } => {
            let mut advice = merge_advice(failure.as_ref(), Audience::Consumer);
            if let Some((amender, base)) = crossed.first() {
                advice.push_str("; ");
                advice.push_str(&cross_version_advice(
                    unit,
                    amender,
                    base,
                    &CrossVersionReason::NoMerge,
                ));
            }
            advice
        }
        BuildProblem::Composition(error) => composition_advice(error, Audience::Consumer)?,
        BuildProblem::CrossVersion {
            unit,
            amender,
            base,
            reason,
            ..
        } => cross_version_advice(unit, amender, base, reason),
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
