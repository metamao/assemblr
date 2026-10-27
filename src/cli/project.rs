//! `assemblr init`, `compose`, `add`, and `remove`: consumer projects.
//!
//! The result goes to stdout. Diagnostics, and what a composition could
//! not carry or store, go to stderr.

use super::advice::{
    Audience, composition_advice, cross_version_advice, distinct, merge_problem_advice,
};
use super::diagnostic::{Diagnosis, count, fail, fail_with};
use super::placeholders;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use assemblr::adapters::{
    BuildProblem, ComposeFailure, ComposeProblem, ComposeRecovery, Composed, DeclarationChange,
    FileChange, GitErrorKind, InitProjectFailure, ProjectStatus, PublishedProblem, Registry,
    StoreError, WriteStep, abort_compose, add_unit, compose_project, continue_compose,
    init_project, project_status, remove_unit, update_units,
};
use assemblr::domain::{LockChange, LockError, PlaceholderReport, ResolutionError, UnitIdentity};
use clap::Args;

#[derive(Debug, Args)]
pub(super) struct InitArgs {
    /// The project's directory, created if missing [default: .]
    directory: Option<PathBuf>,
}

pub(super) fn init(args: &InitArgs) -> ExitCode {
    let directory = args.directory.clone().unwrap_or_else(|| PathBuf::from("."));
    match init_project(&directory) {
        Ok(initialized) if initialized.created() => {
            let created = match (
                initialized.directory_created(),
                initialized.agents_created(),
            ) {
                (true, true) => "the directory, assemblr.jsonc, and AGENTS.md",
                (true, false) => "the directory and assemblr.jsonc",
                (false, true) => "assemblr.jsonc and AGENTS.md",
                (false, false) => "assemblr.jsonc",
            };
            let kept = if initialized.agents_created() {
                ""
            } else {
                "; AGENTS.md was already there, so it was kept"
            };
            println!(
                "Initialized a project in {}: created {created}{kept}.",
                initialized.directory().display(),
            );
            println!(
                "Next: add a unit with `assemblr add @owner/name`; `assemblr registry list` shows \
                 the units you can add."
            );
            ExitCode::SUCCESS
        }
        Ok(initialized) => {
            println!(
                "{} is already a project: assemblr.jsonc is in place; nothing changed.",
                initialized.directory().display()
            );
            ExitCode::SUCCESS
        }
        Err(failure) => {
            let diagnosis = Diagnosis::note("nothing was changed");
            let diagnosis = match &failure {
                InitProjectFailure::UnitRepository(_, "catalog.jsonc") => {
                    diagnosis.advise("run `assemblr init` in a directory outside the catalog")
                }
                InitProjectFailure::UnitRepository(..) => diagnosis
                    .advise("run `assemblr init` in a directory that is not a unit's repository"),
                _ => diagnosis,
            };
            fail_with(&failure.to_string(), &diagnosis)
        }
    }
}

/// A unit on the command line, `@owner/name`, with the text after a second
/// `@` when there is one: a version requirement or an exact version.
#[derive(Debug, Clone)]
pub(super) struct UnitArgument {
    identity: UnitIdentity,
    version: Option<String>,
}

impl UnitArgument {
    pub(super) fn identity(&self) -> &UnitIdentity {
        &self.identity
    }

    pub(super) fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }
}

/// Parses `@owner/name`, optionally followed by `@` and a version.
pub(super) fn unit_argument(argument: &str) -> Result<UnitArgument, String> {
    let (identity, version) = match argument.get(1..).and_then(|rest| rest.find('@')) {
        Some(at) => (&argument[..=at], Some(argument[at + 2..].to_owned())),
        None => (argument, None),
    };
    if version.as_deref() == Some("") {
        return Err(format!("nothing follows the last `@` in `{argument}`"));
    }
    let identity = identity
        .parse::<UnitIdentity>()
        .map_err(|error| error.to_string())?;
    Ok(UnitArgument { identity, version })
}

/// Parses `@owner/name` alone, for the commands that take no version.
fn unit_only(argument: &str) -> Result<UnitIdentity, String> {
    let parsed = unit_argument(argument)?;
    match parsed.version {
        None => Ok(parsed.identity),
        Some(_) => Err(format!(
            "`{argument}` includes a version; give the unit only, like `{}`, and change a \
             requirement with `assemblr add {}@<requirement>`",
            parsed.identity, parsed.identity
        )),
    }
}

#[derive(Debug, Args)]
pub(super) struct AddArgs {
    /// The unit, like `@acme/web` or `@acme/web@^1.2`
    #[arg(value_name = "UNIT", value_parser = unit_argument)]
    unit: UnitArgument,
    /// Also show warnings about unchanged files and cross-version amendments
    #[arg(long)]
    verbose: bool,
}

#[derive(Debug, Args)]
pub(super) struct RemoveArgs {
    /// The unit to remove, like `@acme/web`
    #[arg(value_name = "UNIT", value_parser = unit_only)]
    unit: UnitIdentity,
    /// Also show warnings about unchanged files and cross-version amendments
    #[arg(long)]
    verbose: bool,
}

pub(super) fn add(args: &AddArgs) -> ExitCode {
    let identity = args.unit.identity();
    run_with(Some(identity), None, args.verbose, |root, registry| {
        add_unit(root, registry, identity, args.unit.version())
    })
}

pub(super) fn remove(args: &RemoveArgs) -> ExitCode {
    run_with(Some(&args.unit), None, args.verbose, |root, registry| {
        remove_unit(root, registry, &args.unit)
    })
}

#[derive(Debug, Args)]
pub(super) struct UpdateArgs {
    /// The units to update, like `@acme/web` [default: every unit]
    #[arg(value_name = "UNIT", value_parser = unit_only)]
    units: Vec<UnitIdentity>,
    /// Also show warnings about unchanged files and cross-version amendments
    #[arg(long)]
    verbose: bool,
}

pub(super) fn update(args: &UpdateArgs) -> ExitCode {
    run_with(None, Some(&args.units), args.verbose, |root, registry| {
        update_units(root, registry, &args.units)
    })
}

#[derive(Debug, Args)]
pub(super) struct ComposeArgs {
    /// Finish a composition after you resolve its conflicts
    #[arg(long = "continue", conflicts_with = "abort")]
    resume: bool,
    /// Undo a composition that left conflicts, assemblr.jsonc included
    #[arg(long)]
    abort: bool,
    /// Also show warnings about unchanged files and cross-version amendments
    #[arg(long, conflicts_with_all = ["resume", "abort"])]
    verbose: bool,
}

pub(super) fn compose(args: &ComposeArgs) -> ExitCode {
    if args.resume {
        return settle(|root| {
            continue_compose(root).map(|continued| report_continued(root, &continued))
        });
    }
    if args.abort {
        return settle(|root| abort_compose(root).map(|recovery| report_aborted(&recovery)));
    }
    run_with(None, None, args.verbose, compose_project)
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
        Err(failure) => fail_with(&failure.to_string(), &diagnosis(&failure, &root)),
    }
}

fn report_continued(root: &Path, continued: &assemblr::adapters::Continued) {
    if continued.completed() {
        println!("The composition was already finished; removed its journal.");
    } else {
        println!(
            "Resolved {} and finished the composition; assemblr-lock.json is updated.",
            count(continued.files().len(), "file", "files")
        );
        for path in continued.files() {
            println!("  resolved {path}");
        }
    }
    // The placeholders were hidden while the composition waited. The
    // report is informational, so a registry that cannot be read only
    // leaves it out; `assemblr status` says why.
    let status = Registry::locate()
        .ok()
        .and_then(|registry| project_status(root, &registry).ok());
    if let Some(report) = status.as_ref().and_then(ProjectStatus::placeholders) {
        report_placeholders(report);
    }
}

fn report_aborted(recovery: &ComposeRecovery) {
    match recovery {
        ComposeRecovery::Completed => println!(
            "The composition was already finished, so there is nothing to abort; removed its \
             journal."
        ),
        ComposeRecovery::Reverted {
            removed,
            restored,
            kept,
        } => {
            let declaration = restored
                .iter()
                .any(|path| path.as_str() == assemblr::adapters::DECLARATION);
            println!(
                "Aborted the composition: removed {} it created and restored {} it changed or \
                 removed{}.",
                count(removed.len(), "file", "files"),
                restored.len(),
                if declaration {
                    ", assemblr.jsonc included"
                } else {
                    ""
                }
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
/// it. `unit` is the unit `add` or `remove` was given; `named` holds the
/// units `update` was asked to update, which the report tells apart from the
/// ones that moved with them; and `verbose`
/// says whether `--verbose` was given.
fn run_with(
    unit: Option<&UnitIdentity>,
    named: Option<&[UnitIdentity]>,
    verbose: bool,
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
            report(&composed, named, verbose);
            if composed.pending().is_empty() {
                ExitCode::SUCCESS
            } else {
                let between_units = !composed.unit_conflicts().is_empty();
                let with_yours = !composed.consumer_conflicts().is_empty();
                let what = match (between_units, with_yours) {
                    (false, _) => "between your changes and the composition",
                    (true, false) => "between units",
                    (true, true) => "between units, and between your changes and the composition",
                };
                fail(
                    &format!(
                        "{} conflicts {what}",
                        count(composed.pending().len(), "file holds", "files hold"),
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
            let mut diagnosis = diagnosis(&failure, &root);
            if named.is_some_and(<[UnitIdentity]>::is_empty) && crosses_versions(&failure) {
                diagnosis = diagnosis.advise(
                    "`assemblr update` with the units to move updates only those, and the \
                     units they do not require to move stay where they are",
                );
            }
            fail_with(&failure.to_string(), &diagnosis)
        }
    }
}

/// Whether `failure` holds an amendment that cannot be applied to the
/// provider version resolved, which an update of every unit runs into
/// when only one provider moves too far.
fn crosses_versions(failure: &ComposeFailure) -> bool {
    let ComposeFailure::Rejected { problems, .. } = failure else {
        return false;
    };
    problems.iter().any(|problem| match problem {
        ComposeProblem::Build(problem) => match problem.as_ref() {
            BuildProblem::CrossVersion { .. } => true,
            BuildProblem::Merge { crossed, .. } => !crossed.is_empty(),
            _ => false,
        },
        _ => false,
    })
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
        DeclarationChange::Removed => println!("Removed {unit} from assemblr.jsonc."),
    }
}

fn report(composed: &Composed, named: Option<&[UnitIdentity]>, verbose: bool) {
    if let Some(recovered) = composed.recovered() {
        report_recovered(recovered);
    }
    for note in composed.notes() {
        eprintln!("warning: {note}");
    }
    if verbose {
        for note in composed.unwritten_notes() {
            eprintln!("warning: {note}");
        }
        for line in composed.crossed() {
            eprintln!("note: {line}");
        }
        eprintln!("note: the stored base is {}", composed.base());
    }
    if !composed.pending().is_empty() {
        report_pending(composed);
        return;
    }
    summarize(composed, named);
    // The placeholders are hidden while the composition waits, as `status`
    // hides the edits then.
    if let Some(report) = composed.placeholders() {
        report_placeholders(report);
    }
}

/// The placeholders that remain, after a composition's summary.
fn report_placeholders(report: &PlaceholderReport) {
    if report.is_empty() {
        return;
    }
    println!();
    println!("{}", placeholders::heading(report));
    for line in placeholders::body(report) {
        println!("{line}");
    }
    eprintln!();
    eprintln!("hint: {}", placeholders::HINT);
}

/// The result of a composition that does not wait for the consumer.
fn summarize(composed: &Composed, named: Option<&[UnitIdentity]>) {
    // A declaration that changed without changing the composition, as when
    // a unit that is in the composition already is selected.
    let declaration_only = composed.declaration().is_some()
        && composed.paths().is_empty()
        && composed.changes().is_empty()
        && !composed.first();
    if composed.unchanged() && named.is_some() {
        println!(
            "Nothing to update: every {} is at the newest version its requirements allow.",
            if named.is_some_and(|named| named.len() == 1) {
                "named unit"
            } else {
                "unit"
            },
        );
        return;
    }
    if composed.unchanged() || declaration_only {
        println!("Nothing to compose: the project already matches assemblr.jsonc.");
        return;
    }
    let changes: Vec<String> = composed
        .changes()
        .iter()
        .map(|(identity, change)| describe(identity, change))
        .collect();
    if let Some(named) = named {
        println!("Updated: {}.", changes.join(", "));
        // A unit that moved without being named moved because the versions
        // chosen for the named ones require it.
        let forced: Vec<String> = composed
            .changes()
            .iter()
            .filter(|(_, change)| !matches!(change, LockChange::Retired(_)))
            .map(|(identity, _)| identity)
            .filter(|identity| !named.is_empty() && !named.contains(identity))
            .map(ToString::to_string)
            .collect();
        if !forced.is_empty() {
            eprintln!(
                "note: {} moved although not named: the versions selected for {} require it; to \
                 keep {} where {}, do not update {}",
                forced.join(", "),
                if named.len() == 1 {
                    "the named unit"
                } else {
                    "the named units"
                },
                if forced.len() == 1 { "it" } else { "them" },
                if forced.len() == 1 {
                    "it is"
                } else {
                    "they are"
                },
                if named.len() == 1 {
                    "that unit"
                } else {
                    "those units"
                },
            );
        }
    }
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
            "Composed {} into {}.",
            count(composed.units(), "unit", "units"),
            count(composed.files(), "file", "files")
        );
        return;
    }
    let changed = |change: FileChange| {
        composed
            .paths()
            .values()
            .filter(|found| **found == change)
            .count()
    };
    let changed: Vec<String> = [
        (FileChange::Created, "created"),
        (FileChange::Modified, "modified"),
        (FileChange::Merged, "merged"),
        (FileChange::Removed, "removed"),
    ]
    .into_iter()
    .map(|(change, word)| (changed(change), word))
    .filter(|(found, _)| *found > 0)
    .map(|(found, word)| format!("{found} {word}"))
    .collect();
    println!(
        "Composed {} into {}: {}.",
        count(composed.units(), "unit", "units"),
        count(composed.files(), "file", "files"),
        if changed.is_empty() {
            "no file changed".to_owned()
        } else {
            changed.join(", ")
        }
    );
    for (path, change) in composed.paths() {
        println!("  {} {path}", describe_change(*change));
    }
}

/// A composition that waits for the consumer: the lock is not written yet.
fn report_pending(composed: &Composed) {
    println!(
        "Composed {} into {}, but {} conflicts; the rest is applied, and the lock is written \
         when you finish:",
        count(composed.units(), "unit", "units"),
        count(composed.files(), "file", "files"),
        count(composed.pending().len(), "file has", "files have"),
    );
    for (path, change) in composed.paths() {
        println!("  {} {path}", describe_change(*change));
    }
    for (_, conflict) in composed.unit_conflicts() {
        println!("{conflict}");
    }
    for conflict in composed.consumer_conflicts().values() {
        println!("{conflict}");
    }
    if !composed.consumer_conflicts().is_empty() {
        println!(
            "A conflict with your changes shows your content, the last composition's (empty for \
             a file Assemblr did not write), and the new one, between <<<<<<< current, ||||||| \
             base, =======, and >>>>>>> incoming lines."
        );
    }
    if !composed.unit_conflicts().is_empty() {
        println!(
            "A conflict between units shows each unit's content between <<<<<<< and >>>>>>> \
             lines that name the units, or, in a JSON file, as a \"<<<<<<< conflict\" object \
             naming each unit's value; replace it with the content you want."
        );
    }
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
            "note: reverted a composition that was interrupted earlier: removed {} it created, \
             restored {} it changed or removed{}",
            count(removed.len(), "file", "files"),
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

/// What a failure left, and what the user can do about it.
fn diagnosis(failure: &ComposeFailure, root: &Path) -> Diagnosis {
    let problems = match failure {
        ComposeFailure::Write { step, .. } => {
            return match step {
                WriteStep::Store | WriteStep::Previous | WriteStep::Journal => {
                    Diagnosis::note("the project's files were not changed")
                        .advise("fix the problem and run `assemblr compose` again")
                }
                WriteStep::File(_)
                | WriteStep::Declaration
                | WriteStep::Pending
                | WriteStep::Lock => {
                    Diagnosis::note("the composition is recorded in .assemblr/journal.json").advise(
                        "run `assemblr compose`: it reverts what was changed, assemblr.jsonc \
                         included, and starts over",
                    )
                }
                WriteStep::Finish => {
                    Diagnosis::note("the composition is complete except for its journal")
                        .advise("the next `assemblr compose` removes the journal")
                }
            };
        }
        ComposeFailure::Rejected { problems, .. } => problems,
    };
    let advice = distinct(
        problems
            .iter()
            .filter_map(|problem| problem_advice(problem, root)),
    );
    let mut diagnosis = Diagnosis::note("nothing was changed");
    if !advice.is_empty() {
        diagnosis = diagnosis.advise(advice.join("; "));
    }
    diagnosis
}

/// What to do about `problem`, met in the project at `root`, when there is
/// advice to give.
pub(super) fn problem_advice(problem: &ComposeProblem, root: &Path) -> Option<String> {
    match problem {
        ComposeProblem::UnitRepository(_, "catalog.jsonc") => Some(
            "unit commands run here, for the catalog's units; a project is created in another \
             directory with `assemblr init`"
                .to_owned(),
        ),
        ComposeProblem::UnitRepository(..) => Some(
            "unit commands run here, and a dependency is added with `assemblr unit add <UNIT>`; \
             a project is created in another directory with `assemblr init`"
                .to_owned(),
        ),
        ComposeProblem::NoDeclaration(_) => Some(match enclosing_project(root) {
            Some(project) => format!(
                "the project is in {}; run the command there",
                project.display()
            ),
            None => "run `assemblr init` to create a project here, or change to the project's \
                     directory"
                .to_owned(),
        }),
        ComposeProblem::Existing(_) => Some(
            "a binary file, a directory, or a link cannot hold conflict markers; move it away \
             and compose again"
                .to_owned(),
        ),
        ComposeProblem::LocallyIgnored(_) => Some(
            "a file must be tracked or ignored the same way on every machine: add it to the \
             project's .gitignore, re-include it there with a `!` pattern (`!dir/` for a \
             directory), or remove your own ignore rule"
                .to_owned(),
        ),
        ComposeProblem::NotSelected { transitive, .. } => Some(if *transitive {
            "remove the unit that requires it, or add it with `assemblr add` first".to_owned()
        } else {
            "assemblr.jsonc lists the units this project selects".to_owned()
        }),
        ComposeProblem::NothingLocked => Some(
            "run `assemblr compose` first: a project is updated from the versions it holds"
                .to_owned(),
        ),
        ComposeProblem::NotLocked { locked, .. } => Some(if locked.is_empty() {
            "no unit is locked yet; run `assemblr compose` first".to_owned()
        } else {
            format!(
                "the units you can update are: {}",
                locked
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }),
        ComposeProblem::InvalidRequirement(_) => Some(
            "write a SemVer requirement after the unit, like @owner/name@^1.2.0, @owner/name@~1.2, \
             or @owner/name@=1.2.3"
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
            "for each file, restore the composed content or change the declaration; only \
             conflicts between text files are left in the files to resolve, and only when no \
             other kind of conflict is found"
                .to_owned(),
        ),
        ComposeProblem::Pending(_) => Some(
            "resolve the conflict markers in those files and run `assemblr compose --continue`, \
             or run `assemblr compose --abort` to restore the project"
                .to_owned(),
        ),
        ComposeProblem::Unresolved(_) => Some(
            "edit each file to the content you want, remove every <<<<<<<, ||||||| base, and \
             >>>>>>> line and every \"<<<<<<< conflict\" object, and run `assemblr compose \
             --continue` again"
                .to_owned(),
        ),
        ComposeProblem::DeclarationChanged => Some(
            "put assemblr.jsonc back as the composition left it and run `assemblr compose \
             --continue` again, or run `assemblr compose --abort`, which gives up the \
             resolution and restores assemblr.jsonc where `add` or `remove` rewrote it, and \
             compose from the declaration you want afterwards"
                .to_owned(),
        ),
        ComposeProblem::NothingPending | ComposeProblem::NotPending => Some(
            "`--continue` and `--abort` only apply to a composition that left conflicts; run \
             `assemblr compose` to compose the project"
                .to_owned(),
        ),
        ComposeProblem::Base(StoreError::Missing(_)) => Some(
            "restore .assemblr/ from version control; a project cannot be reconciled without its \
             stored base"
                .to_owned(),
        ),
        ComposeProblem::Base(_) => Some(
            "restore .assemblr/ from version control; a stored base that does not verify is never \
             used"
                .to_owned(),
        ),
        ComposeProblem::Store(StoreError::Journal { .. }) => Some(
            "the journal may come from a newer Assemblr: finish the composition with that version \
             (`assemblr compose`, or `--continue` or `--abort` when it waits)"
                .to_owned(),
        ),
        ComposeProblem::ForeignLockfile(_) => Some(
            "remove or move the unit's lockfile; `assemblr compose` writes the project's own"
                .to_owned(),
        ),
        ComposeProblem::Build(problem) => build_advice(problem),
        _ => None,
    }
}

/// The nearest directory above `root` that holds a project's declaration.
fn enclosing_project(root: &Path) -> Option<&Path> {
    root.ancestors()
        .skip(1)
        .find(|directory| directory.join("assemblr.jsonc").is_file())
}

/// Advice for the problems a unit build also reports.
fn build_advice(problem: &BuildProblem) -> Option<String> {
    Some(match problem {
        BuildProblem::Lock(LockError::ContentChanged { .. }) => {
            "published versions never change: either its tag was moved, which restoring the tag \
             in the unit's repository repairs, or the lockfile was built against content never \
             published as that version"
                .to_owned()
        }
        BuildProblem::Resolution(ResolutionError::Unavailable { .. }) => {
            "run `assemblr registry sync` to index newly published versions".to_owned()
        }
        BuildProblem::Resolution(ResolutionError::Missing { .. }) => {
            "register the unit's repository with `assemblr registry add <path>`".to_owned()
        }
        BuildProblem::Source { error, .. }
            if matches!(error.kind(), GitErrorKind::Unretrievable(_)) =>
        {
            format!(
                "check that `git ls-remote {}` works with your Git credentials",
                error.repository().display()
            )
        }
        BuildProblem::Source { .. } => "if a registered repository was moved or deleted, \
                                        `assemblr registry remove <identity>` removes its \
                                        registration"
            .to_owned(),
        // Content is checked when a version is first read (ADR 0037), and a
        // version once indexed stays indexed, so synchronizing cannot help.
        BuildProblem::Published {
            unit,
            problem: PublishedProblem::Src(_) | PublishedProblem::Amendments(_),
            ..
        } => format!(
            "this version cannot be used: select another version of {unit}, or ask its author \
             to publish a fixed one"
        ),
        BuildProblem::Published { .. } => {
            "run `assemblr registry sync` to check the registered repositories again".to_owned()
        }
        BuildProblem::Merge {
            unit,
            failure,
            crossed,
            ..
        } => merge_problem_advice(unit, failure.as_ref(), crossed, Audience::Consumer),
        BuildProblem::Composition(error) => composition_advice(error, Audience::Consumer)?,
        BuildProblem::CrossVersion {
            unit,
            amender,
            base,
            reason,
            ..
        } => cross_version_advice(unit, amender, base, reason, Audience::Consumer),
        _ => return None,
    })
}
