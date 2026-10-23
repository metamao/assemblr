//! `assemblr unit`: authoring operations on the unit repository in the
//! current directory.
//!
//! The result goes to stdout. Diagnostics, and changes to the lockfile, go to
//! stderr.

use super::advice::{
    Audience, composition_advice, cross_version_advice, distinct, merge_problem_advice,
};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use assemblr::adapters::{
    AmendFailure, AmendProblem, AmendState, BuildFailure, BuildProblem, Built, CheckFailure,
    CheckProblem, FileOutcome, GitOutcome, IndexOutcome, InitFailure, Initialized, LockfileOutcome,
    ManifestFileError, PublishFailure, PublishProblem, PublishState, Published, RecoveryError,
    Registry, TagProblem, UpdateFailure, UpdateProblem, UpdateState, Updated, amend_unit,
    build_unit, check_unit, init_unit, publish_unit, update_unit,
};
use assemblr::domain::{
    ContributionError, Excluded, IntegrityChange, LockChange, LockError, RegistryError,
    ResolutionError, UnitIdentity,
};
use clap::{Args, Subcommand};

use super::diagnostic::{Diagnosis, count, fail, fail_with};

#[derive(Debug, Subcommand)]
pub(super) enum UnitCommand {
    /// Create a new unit in its own directory
    ///
    /// Creates the directory, a Git repository, `manifest.jsonc`, the
    /// manifest's JSON Schema, and `.gitignore`. Without a directory, the
    /// unit goes in a new directory named after it: `@acme/web` goes in
    /// `web/`. Give `.` to set up the unit in the current directory, for
    /// example in a repository you already have. Running it again only
    /// adds what is missing.
    #[command(after_long_help = "Examples:\n  \
        assemblr unit init @acme/web\n  \
        assemblr unit init @acme/web . --description \"Web server setup\"")]
    Init(InitArgs),
    /// Build the unit and its dependencies into build/
    ///
    /// Run it in the unit's directory. The first build creates
    /// `assemblr-lock.json`, which records the versions of the
    /// dependencies.
    Build,
    /// Check the unit for build and publication problems
    ///
    /// Writes nothing.
    Check,
    /// Publish the committed unit as the version in its manifest
    ///
    /// Tags the commit at `HEAD` as `v<version>` and indexes the version in
    /// the local registry when the repository is registered.
    Publish,
    /// Copy a dependency's file to amendments/ to change it
    ///
    /// The copy is declared in `manifest.jsonc`, and the build delivers it
    /// in place of the dependency's file.
    #[command(after_long_help = "Examples:\n  \
        assemblr unit amend package.json\n  \
        assemblr unit amend config/app.json --unit @acme/base")]
    Amend(AmendArgs),
    /// Update dependencies to the newest versions allowed
    ///
    /// Updates `assemblr-lock.json` within the requirements in
    /// `manifest.jsonc`, and carries the unit's amendments over to the new
    /// versions.
    #[command(after_long_help = "Examples:\n  \
        assemblr unit update\n  \
        assemblr unit update @acme/base")]
    Update(UpdateArgs),
}

#[derive(Debug, Args)]
pub(super) struct InitArgs {
    /// The unit, like `@acme/web`
    #[arg(value_name = "UNIT", value_parser = UnitIdentity::parse)]
    identity: UnitIdentity,
    /// The unit's directory, created if missing [default: the unit's name]
    directory: Option<PathBuf>,
    /// The manifest's description [default: "The <UNIT> unit."]
    #[arg(long, value_name = "TEXT")]
    description: Option<String>,
}

#[derive(Debug, Args)]
pub(super) struct UpdateArgs {
    /// The dependencies to update, like `@acme/base`, direct or not
    /// [default: every dependency]
    #[arg(value_name = "UNIT", value_parser = UnitIdentity::parse)]
    units: Vec<UnitIdentity>,
}

#[derive(Debug, Args)]
pub(super) struct AmendArgs {
    /// The file to amend: a file name, or a path in the dependency's src/;
    /// a template matches with or without `.assemblr.tmpl`
    file: String,
    /// Look only in this direct dependency, like `@acme/base`
    #[arg(long, value_name = "UNIT", value_parser = UnitIdentity::parse)]
    unit: Option<UnitIdentity>,
}

pub(super) fn run(command: UnitCommand) -> ExitCode {
    match command {
        UnitCommand::Init(args) => init(&args),
        UnitCommand::Build => build(),
        UnitCommand::Check => check(),
        UnitCommand::Publish => publish(),
        UnitCommand::Amend(args) => amend(&args),
        UnitCommand::Update(args) => update(&args),
    }
}

fn init(args: &InitArgs) -> ExitCode {
    match init_unit(
        args.directory.as_deref(),
        &args.identity,
        args.description.as_deref(),
    ) {
        Ok(initialized) => {
            println!("{}", describe_initialized(&initialized));
            let enter = match &args.directory {
                Some(directory) if directory.as_os_str() == "." => String::new(),
                Some(directory) => format!("`cd {}`, then ", directory.display()),
                None => format!("`cd {}`, then ", args.identity.name()),
            };
            println!(
                "Next: {enter}add files under src/, declare dependencies in manifest.jsonc, and \
                 run `assemblr unit build`."
            );
            ExitCode::SUCCESS
        }
        Err(failure) => fail_with(&failure.to_string(), &init_diagnosis(&failure, args)),
    }
}

/// One line saying what `unit init` created, replaced, or found in place.
fn describe_initialized(initialized: &Initialized) -> String {
    let mut created = Vec::new();
    let mut replaced = Vec::new();
    let mut kept = Vec::new();
    if initialized.directory_created() {
        created.push("the directory");
    }
    let nested;
    match initialized.git() {
        GitOutcome::Initialized => created.push("a Git repository"),
        GitOutcome::Nested { within } => {
            nested = format!(
                "a Git repository (nested in the repository at {})",
                within.display()
            );
            created.push(&nested);
        }
        GitOutcome::Adopted => kept.push("the Git repository"),
    }
    for (name, outcome, refreshed) in [
        ("manifest.jsonc", initialized.manifest(), "manifest.jsonc"),
        (
            "manifest.schema.json",
            initialized.schema(),
            "manifest.schema.json (refreshed)",
        ),
        (
            ".gitignore",
            initialized.gitignore(),
            ".gitignore (/build/ added)",
        ),
    ] {
        match outcome {
            FileOutcome::Created => created.push(name),
            FileOutcome::Replaced => replaced.push(refreshed),
            FileOutcome::Kept => kept.push(name),
        }
    }
    let list = |items: &[&str]| items.join(", ");
    let mut parts = Vec::new();
    if !created.is_empty() {
        parts.push(format!("created {}", list(&created)));
    }
    if !replaced.is_empty() {
        parts.push(format!("replaced {}", list(&replaced)));
    }
    let fresh = initialized.manifest() == FileOutcome::Created;
    if !kept.is_empty() {
        parts.push(format!(
            "{} {}",
            if fresh { "kept" } else { "already in place:" },
            list(&kept)
        ));
    }
    format!(
        "{} {} in {}: {}.",
        if fresh {
            "Initialized"
        } else {
            "Completed the scaffold of"
        },
        initialized.identity(),
        initialized.directory().display(),
        parts.join("; ")
    )
}

/// What a failed initialization left, and what the author can do.
fn init_diagnosis(failure: &InitFailure, args: &InitArgs) -> Diagnosis {
    match failure {
        InitFailure::Write { done, .. } | InitFailure::Init { done, .. } => {
            let mut state = Vec::new();
            if done.directory_created() {
                state.push("the directory was created");
            }
            // On `Init`, the repository is what failed; `done.git()` holds
            // the plan, not an outcome.
            if matches!(failure, InitFailure::Write { .. }) && *done.git() != GitOutcome::Adopted {
                state.push("the Git repository was initialized");
            }
            for (name, outcome) in [
                ("manifest.jsonc", done.manifest()),
                ("manifest.schema.json", done.schema()),
                (".gitignore", done.gitignore()),
            ] {
                if outcome != FileOutcome::Kept {
                    state.push(name);
                }
            }
            let note = if state.is_empty() {
                "nothing was written".to_owned()
            } else {
                format!("written so far: {}", state.join(", "))
            };
            let diagnosis = Diagnosis::note(note);
            let diagnosis = if matches!(failure, InitFailure::Init { .. }) {
                diagnosis.advise("Assemblr needs Git installed and on the PATH")
            } else {
                diagnosis
            };
            diagnosis.advise("fix the problem and run the same command again to finish")
        }
        InitFailure::Inside { top_level, .. } => {
            let diagnosis = Diagnosis::note("nothing was changed").advise(format!(
                "initialize the unit in a directory outside {}",
                top_level.display()
            ));
            if args.directory.is_none() {
                // Run again at a unit's top level, the derived directory
                // lands inside it: `.` is what completes the scaffold.
                diagnosis.advise(format!(
                    "to complete the scaffold of the unit here, run `assemblr unit init {} .`",
                    args.identity
                ))
            } else {
                diagnosis
            }
        }
        InitFailure::OtherIdentity { declared, .. } => Diagnosis::note("nothing was changed")
            .advise(format!(
                "to work on {declared}, run `assemblr unit init {declared} .`; for a new unit, \
                 choose another directory"
            )),
        InitFailure::NotEmpty(directory) => Diagnosis::note("nothing was changed").advise(format!(
            "give a new directory, like `assemblr unit init {identity} {name}-unit`, or name \
                 the directory to use it anyway: `assemblr unit init {identity} {name}`",
            identity = args.identity,
            name = directory.display()
        )),
        InitFailure::ReservedName(_) => Diagnosis::note("nothing was changed").advise(format!(
            "give the unit a directory: `assemblr unit init {} <DIRECTORY>`",
            args.identity
        )),
        InitFailure::Git(_) => Diagnosis::note("nothing was changed")
            .advise("Assemblr needs Git installed and on the PATH"),
        _ => Diagnosis::note("nothing was changed"),
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
    let (root, registry) = match locate() {
        Ok(located) => located,
        Err(code) => return code,
    };
    match amend_unit(&root, &registry, &args.file, args.unit.as_ref()) {
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
            if !amended.shared_with().is_empty() {
                let others: Vec<String> = amended
                    .shared_with()
                    .iter()
                    .map(|(unit, version)| format!("{unit} {version}"))
                    .collect();
                println!(
                    "The file is also amended by {}; the build combines the copies, and changes \
                     to the same value conflict.",
                    others.join(", ")
                );
            }
            ExitCode::SUCCESS
        }
        Err(failure) => fail_with(&failure.to_string(), &amend_diagnosis(&failure, &root)),
    }
}

/// What a failed amendment left, and what the author can do.
fn amend_diagnosis(failure: &AmendFailure, root: &Path) -> Diagnosis {
    let problems = match failure {
        AmendFailure::Write(error) => {
            return match error.state() {
                AmendState::Unchanged => Diagnosis::note("nothing was changed")
                    .advise("fix the problem and run the command again"),
                AmendState::CopyWithoutDeclaration => {
                    Diagnosis::note("the copy was created but not declared in manifest.jsonc")
                        .advise("run the same command again to declare it")
                }
            };
        }
        AmendFailure::Rejected(problems) => problems,
    };
    let advice = problems.iter().find_map(|problem| match problem {
        AmendProblem::Manifest(ManifestFileError::Missing { .. }) => Some(format!(
            "{} is not a unit repository's top level; run `assemblr unit amend` in the \
             directory that holds `manifest.jsonc`",
            root.display()
        )),
        AmendProblem::NoLockfile | AmendProblem::LockfileStale { .. } => {
            Some("run `assemblr unit build` first, which writes the lockfile".to_owned())
        }
        AmendProblem::UpdateInterrupted { .. } => {
            Some("run `assemblr unit update` to complete the interrupted update".to_owned())
        }
        AmendProblem::Build(problem) => advice(problem, root),
        AmendProblem::Resolution(error) => advice(&BuildProblem::Resolution(error.clone()), root),
        _ => None,
    });
    Diagnosis::note("nothing was changed").advise_if(advice)
}

fn update(args: &UpdateArgs) -> ExitCode {
    let units = &args.units;
    let (root, registry) = match locate() {
        Ok(located) => located,
        Err(code) => return code,
    };
    match update_unit(&root, &registry, units) {
        Ok(updated) => {
            report_recovered(&updated);
            if !updated.written() {
                println!(
                    "Nothing to update: every dependency is at the newest version its \
                     requirements allow."
                );
                return ExitCode::SUCCESS;
            }
            let changes: Vec<String> = updated
                .changes()
                .iter()
                .map(|(identity, change)| describe(identity, change))
                .collect();
            let mut result = format!("Updated assemblr-lock.json: {}.", changes.join(", "));
            if changes.is_empty() {
                result = "Updated manifest.jsonc.".to_owned();
            }
            if !updated.carried().is_empty() {
                let carried: Vec<String> = updated
                    .carried()
                    .iter()
                    .map(|(path, carried)| {
                        format!(
                            "amendments/{path} from {} {} to {}",
                            carried.unit(),
                            carried.from(),
                            carried.to()
                        )
                    })
                    .collect();
                result.push_str(&format!(
                    " Carried over, in manifest.jsonc: {}.",
                    carried.join(", ")
                ));
                let rebased: Vec<String> = updated
                    .carried()
                    .iter()
                    .filter(|(_, carried)| carried.rebased())
                    .map(|(path, _)| format!("amendments/{path}"))
                    .collect();
                if !rebased.is_empty() {
                    result.push_str(&format!(
                        " Rewritten to follow the provider's changes: {}.",
                        rebased.join(", ")
                    ));
                }
            }
            println!("{result}");
            ExitCode::SUCCESS
        }
        Err(failure) => fail_with(&failure.to_string(), &update_diagnosis(&failure, &root)),
    }
}

/// Reports on stderr that an interrupted update was completed first.
fn report_recovered(updated: &Updated) {
    let Some(recovered) = updated.recovered() else {
        return;
    };
    let replaced = match (recovered.manifest(), recovered.lockfile()) {
        (true, true) => "manifest.jsonc and assemblr-lock.json were replaced",
        (false, true) => "assemblr-lock.json was replaced",
        _ => "both files were already in place",
    };
    let copies = match recovered.copies() {
        0 => String::new(),
        copies => format!(
            "; {} under amendments/",
            count(copies, "copy was replaced", "copies were replaced")
        ),
    };
    eprintln!("note: completed an update that was interrupted earlier: {replaced}{copies}");
}

/// What a failed update left, and what the author can do.
fn update_diagnosis(failure: &UpdateFailure, root: &Path) -> Diagnosis {
    let problems = match failure {
        UpdateFailure::Write(error) => {
            return match error.state() {
                UpdateState::Unchanged => Diagnosis::note("nothing was changed")
                    .advise("fix the problem and run the command again"),
                UpdateState::Interrupted {
                    copies_replaced,
                    manifest_replaced,
                    lockfile_replaced,
                } => {
                    let replaced = match (manifest_replaced, lockfile_replaced) {
                        (true, true) => "manifest.jsonc and assemblr-lock.json were replaced",
                        (true, false) => "manifest.jsonc was replaced, assemblr-lock.json was not",
                        _ => "neither manifest.jsonc nor assemblr-lock.json was replaced",
                    };
                    Diagnosis::note(format!(
                        "{replaced}; {} replaced; the update is recorded",
                        count(
                            copies_replaced,
                            "amendment copy was",
                            "amendment copies were"
                        )
                    ))
                    .advise("run `assemblr unit update` again to finish it")
                }
            };
        }
        UpdateFailure::Rejected(problems) => problems,
    };
    // An interrupted update that could not be completed stops everything
    // else: the files are left for the author, with the record.
    if let Some(UpdateProblem::Recovery(error)) = problems
        .iter()
        .find(|problem| matches!(problem, UpdateProblem::Recovery(_)))
    {
        return match error {
            RecoveryError::Write { .. } => {
                Diagnosis::note("the record of the interrupted update remains")
                    .advise("fix the problem and run `assemblr unit update` again to finish it")
            }
            _ => Diagnosis::note(
                "nothing was changed now; the record of the interrupted update is kept",
            )
            .advise("only `assemblr unit update` finishes it"),
        };
    }
    let advice = problems.iter().find_map(|problem| match problem {
        UpdateProblem::Manifest(ManifestFileError::Missing { .. }) => Some(format!(
            "{} is not a unit repository's top level; run `assemblr unit update` in the \
             directory that holds `manifest.jsonc`",
            root.display()
        )),
        UpdateProblem::NoLockfile => {
            Some("run `assemblr unit build` first, which creates the lockfile".to_owned())
        }
        UpdateProblem::Lockfile(_) => {
            Some(format!("fix assemblr-lock.json; {DELETING_THE_LOCKFILE}"))
        }
        UpdateProblem::Build(problem) => advice(problem, root),
        UpdateProblem::Resolution(error) => advice(&BuildProblem::Resolution(error.clone()), root),
        UpdateProblem::Lock(error) => advice(&BuildProblem::Lock(error.clone()), root),
        UpdateProblem::Reconcile(_) => Some(
            "keep the locked version, or resolve what is reported: remove the amendment's \
             declaration and its copy, update, and amend the file again at the new version"
                .to_owned(),
        ),
        UpdateProblem::RebasedNotJson { .. } => Some(
            "remove the amendment's declaration and its copy, update, and amend the file again \
             at the new version"
                .to_owned(),
        ),
        _ => None,
    });
    Diagnosis::note("nothing was changed").advise_if(advice)
}

fn build() -> ExitCode {
    let (root, registry) = match locate() {
        Ok(located) => located,
        Err(code) => return code,
    };
    match build_unit(&root, &registry) {
        Ok(built) => {
            report_lockfile(&built);
            warn_notes(built.notes());
            if let Some((path, error)) = built.leftover() {
                eprintln!(
                    "warning: could not delete the previous build at {}: {error}; the next \
                     build deletes it",
                    path.display()
                );
            }
            println!(
                "Built {} {} into build/: {}, with {}.",
                built.unit(),
                built.version(),
                count(built.files(), "file", "files"),
                count(built.dependencies(), "dependency", "dependencies")
            );
            ExitCode::SUCCESS
        }
        Err(failure) => fail_with(&failure.to_string(), &diagnosis(&failure, &root)),
    }
}

fn check() -> ExitCode {
    let (root, registry) = match locate() {
        Ok(located) => located,
        Err(code) => return code,
    };
    match check_unit(&root, &registry) {
        Ok(checked) => {
            warn_notes(checked.notes());
            println!(
                "Checked {} {}: no problems; a build delivers {}, with {}.",
                checked.unit(),
                checked.version(),
                count(checked.files(), "file", "files"),
                count(checked.dependencies(), "dependency", "dependencies")
            );
            ExitCode::SUCCESS
        }
        Err(failure) => {
            let CheckFailure::Rejected(problems) = &failure;
            let diagnosis = Diagnosis::note("nothing was changed").advise_if(
                problems
                    .iter()
                    .find_map(|problem| check_advice(problem, &root, "check")),
            );
            fail_with(&failure.to_string(), &diagnosis)
        }
    }
}

/// Advice for a problem a check or a publication found, for the command
/// `command`.
fn check_advice(problem: &CheckProblem, root: &Path, command: &str) -> Option<String> {
    Some(match problem {
        CheckProblem::Build(BuildProblem::Manifest(ManifestFileError::Missing { .. })) => format!(
            "{} is not a unit repository's top level; run `assemblr unit {command}` in the \
             directory that holds `manifest.jsonc`",
            root.display()
        ),
        CheckProblem::Build(problem) => return advice(problem, root),
        CheckProblem::NoLockfile | CheckProblem::LockfileStale { .. } if command == "publish" => {
            "run `assemblr unit build`, which writes the lockfile, then commit it and publish \
             again"
                .to_owned()
        }
        CheckProblem::NoLockfile | CheckProblem::LockfileStale { .. } => {
            "run `assemblr unit build`, which writes the lockfile, and check the result".to_owned()
        }
        CheckProblem::Contribution(ContributionError::NoContribution) => {
            "add files under `src/`, or declare the dependencies the unit composes".to_owned()
        }
        CheckProblem::Contribution(ContributionError::RedundantAmendment { .. }) => {
            return None;
        }
    })
}

fn publish() -> ExitCode {
    let (root, registry) = match locate() {
        Ok(located) => located,
        Err(code) => return code,
    };
    match publish_unit(&root, &registry) {
        Ok(published) => {
            report_published(&published);
            ExitCode::SUCCESS
        }
        Err(PublishFailure::IndexNotUpdated { published, error }) => {
            report_published(&published);
            fail(
                &format!("the registry index was not updated: {error}"),
                Some(
                    "the published tag stands; run `assemblr registry sync` to index the \
                     version once the problem is fixed",
                ),
            )
        }
        Err(failure) => fail_with(&failure.to_string(), &publish_diagnosis(&failure, &root)),
    }
}

/// Reports on stderr what a composition could not carry.
fn warn_notes(notes: &[String]) {
    for note in notes {
        eprintln!("warning: {note}");
    }
}

/// Reports a publication: the tag on stdout, and the registry's part.
fn report_published(published: &Published) {
    warn_notes(published.notes());
    let commit = published.provenance().commit().as_str();
    let short = commit.get(..12).unwrap_or(commit);
    if published.already_published() {
        println!(
            "{} {} is already published: tag {} points at commit {short}.",
            published.unit(),
            published.version(),
            published.tag()
        );
    } else {
        println!(
            "Published {} {} as tag {} at commit {short}.",
            published.unit(),
            published.version(),
            published.tag()
        );
    }
    match published.index() {
        IndexOutcome::Indexed { excluded } => {
            warn_excluded(excluded);
            println!("Indexed it in the local registry.");
        }
        IndexOutcome::AlreadyIndexed { excluded } => {
            warn_excluded(excluded);
            println!("The local registry already indexes it.");
        }
        IndexOutcome::RegisteredElsewhere { source } => {
            println!(
                "The local registry registers {} from {source}, not from this repository, so the \
                 version becomes discoverable when `assemblr registry sync` reads that source, \
                 for example after a push to it.",
                published.unit()
            );
        }
        IndexOutcome::Managed { catalogs } => {
            let catalogs: Vec<String> = catalogs.iter().map(ToString::to_string).collect();
            println!(
                "The local registry has {} through {}, whose index lists its versions, so the \
                 version becomes discoverable once `assemblr catalog refresh` adds it there, \
                 the catalog is pushed, and `assemblr registry sync` reads it.",
                published.unit(),
                catalogs.join(", ")
            );
        }
        IndexOutcome::NotRegistered => {
            println!(
                "The local registry does not register {}, so the version is not discoverable \
                 until `assemblr registry add {}` registers this repository.",
                published.unit(),
                root_display()
            );
        }
    }
}

/// The current directory as the author would name it on the command line.
fn root_display() -> String {
    std::env::current_dir().map_or_else(|_| ".".to_owned(), |dir| dir.display().to_string())
}

/// Explains on stderr why each excluded tag is invalid.
fn warn_excluded(excluded: &[Excluded<TagProblem>]) {
    for excluded in excluded {
        eprintln!(
            "warning: {}: excluded tag {}, which is not indexed as a published version: {}",
            excluded.identity(),
            excluded.tag(),
            excluded.reason()
        );
    }
}

/// What a refused publication left, and what the author can do.
fn publish_diagnosis(failure: &PublishFailure, root: &Path) -> Diagnosis {
    let problems = match failure {
        PublishFailure::Tag(_) => {
            return Diagnosis::note("no tag was created and nothing was changed").advise(
                "a tag needs your Git identity (`user.name` and `user.email` in the Git \
                 configuration) and, if signing is configured, a working signing setup",
            );
        }
        PublishFailure::IndexNotUpdated { .. } => {
            return Diagnosis::note("the published tag stands")
                .advise("run `assemblr registry sync` to index the version");
        }
        PublishFailure::Rejected(problems) => problems,
    };
    let advice = problems.iter().find_map(|problem| match problem {
        PublishProblem::Check(problem) => check_advice(problem, root, "publish"),
        PublishProblem::NoCommit | PublishProblem::Uncommitted { .. } => Some(
            "commit the unit, build/ excepted, and run `assemblr unit publish` again".to_owned(),
        ),
        PublishProblem::CommittedManifest(_) | PublishProblem::CommittedLockfile(_) => {
            Some("fix the file, commit, and run `assemblr unit publish` again".to_owned())
        }
        PublishProblem::TagElsewhere { .. } | PublishProblem::EqualPrecedence { .. } => Some(
            "published versions never change: raise the version in manifest.jsonc, build, \
             commit, and publish again"
                .to_owned(),
        ),
        PublishProblem::RegisteredAs { registered, .. } => Some(format!(
            "restore the name in manifest.jsonc, or run `assemblr registry remove {registered}` \
             and register the repository again under its new identity"
        )),
        PublishProblem::Registry(errors) => errors.iter().find_map(|error| match error {
            RegistryError::Integrity { change, .. } => Some(match change {
                IntegrityChange::TagRemoved | IntegrityChange::TagMoved { .. } => {
                    "published versions never change: restore the tag where it was".to_owned()
                }
                IntegrityChange::BecameInvalid(_) => {
                    "published versions never change: restore the tag where it was".to_owned()
                }
            }),
            _ => None,
        }),
        PublishProblem::Reference(_) => None,
    });
    Diagnosis::note("no tag was created and nothing was changed").advise_if(advice)
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

/// What a failed build left, and what the author can do about it.
fn diagnosis(failure: &BuildFailure, root: &Path) -> Diagnosis {
    let problems = match failure {
        BuildFailure::Publish(error) => {
            return match error.state() {
                PublishState::Unchanged => Diagnosis::note("nothing was changed")
                    .advise("fix the problem and run `assemblr unit build` again"),
                PublishState::BuildRemoved | PublishState::LockfileWithoutBuild => {
                    Diagnosis::note("there is no build/ now")
                        .advise("run `assemblr unit build` again")
                }
            };
        }
        BuildFailure::Rejected(problems) => problems,
    };
    let advice = distinct(problems.iter().filter_map(|problem| advice(problem, root)));
    let diagnosis = Diagnosis::note("build/ and assemblr-lock.json were not changed");
    if advice.is_empty() {
        diagnosis
    } else {
        diagnosis.advise(advice.join("; "))
    }
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
        BuildProblem::UpdateInterrupted { .. } => {
            "run `assemblr unit update` to complete the interrupted update".to_owned()
        }
        BuildProblem::Lock(LockError::ContentChanged { .. }) => format!(
            "published versions never change: restore the tag in the unit's repository; \
             {DELETING_THE_LOCKFILE}"
        ),
        // The message already says what to do.
        BuildProblem::AmendmentBase(_) => return None,
        BuildProblem::BaseIntegrity(_) => "published versions never change: either the \
                                           provider's tag was moved and its source registered \
                                           again, or the amender was published against other \
                                           content; run `assemblr registry sync` to check the \
                                           registered repositories"
            .to_owned(),
        BuildProblem::Merge {
            unit,
            failure,
            crossed,
            ..
        } => merge_problem_advice(unit, failure.as_ref(), crossed, Audience::Author),
        BuildProblem::Composition(error) => composition_advice(error, Audience::Author)?,
        BuildProblem::CrossVersion {
            unit,
            amender,
            base,
            reason,
            ..
        } => cross_version_advice(unit, amender, base, reason, Audience::Author),
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
