//! `assemblr unit`: authoring operations on the unit repository in the
//! current directory.
//!
//! The result goes to stdout. Diagnostics, and changes to the lockfile, go to
//! stderr.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use assemblr::adapters::{
    AmendFailure, AmendProblem, AmendState, BuildFailure, BuildProblem, Built, CheckFailure,
    CheckProblem, FileOutcome, GitOutcome, IndexOutcome, InitFailure, Initialized, LockfileOutcome,
    ManifestFileError, MergeFailure, PublishFailure, PublishProblem, PublishState, Published,
    RecoveryError, Registry, TagProblem, UpdateFailure, UpdateProblem, UpdateState, Updated,
    amend_unit, build_unit, check_unit, init_unit, publish_unit, update_unit,
};
use assemblr::domain::{
    ContributionError, Excluded, IntegrityChange, LockChange, LockError, RegistryError,
    ResolutionError, UnitIdentity,
};
use clap::{Args, Subcommand};

#[derive(Debug, Subcommand)]
pub(super) enum UnitCommand {
    /// Create a unit's scaffold: its Git repository, `manifest.jsonc`, the
    /// manifest's JSON Schema, and `.gitignore`.
    Init(InitArgs),
    /// Compose the unit in the current directory with its published
    /// dependencies into `build/`.
    Build,
    /// Run every validation a build runs, plus the checks that gate
    /// publication, without writing anything.
    Check,
    /// Tag the committed unit as the version its manifest declares, and
    /// index it in the local registry when the repository is registered.
    Publish,
    /// Copy a dependency's file to `amendments/` for editing, and declare
    /// the amendment in `manifest.jsonc`.
    Amend(AmendArgs),
    /// Move dependencies to the newest published versions their declared
    /// ranges allow, in `assemblr-lock.json`, carrying the unit's
    /// amendments over.
    Update(UpdateArgs),
}

#[derive(Debug, Args)]
pub(super) struct InitArgs {
    /// The unit's identity, such as `@owner/unit`.
    #[arg(value_parser = UnitIdentity::parse)]
    identity: UnitIdentity,
    /// The unit's directory, created if it does not exist. Defaults to the
    /// current directory.
    directory: Option<PathBuf>,
    /// The manifest's description. Defaults to a placeholder that names the
    /// unit.
    #[arg(long, value_name = "TEXT")]
    description: Option<String>,
}

#[derive(Debug, Args)]
pub(super) struct UpdateArgs {
    /// The dependencies to update, such as `@owner/unit`, direct or
    /// transitive. Without any, every dependency is updated.
    #[arg(value_name = "IDENTITY")]
    units: Vec<String>,
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
        UnitCommand::Init(args) => init(&args),
        UnitCommand::Build => build(),
        UnitCommand::Check => check(),
        UnitCommand::Publish => publish(),
        UnitCommand::Amend(args) => amend(&args),
        UnitCommand::Update(args) => update(&args),
    }
}

fn init(args: &InitArgs) -> ExitCode {
    let directory = args.directory.clone().unwrap_or_else(|| PathBuf::from("."));
    match init_unit(&directory, &args.identity, args.description.as_deref()) {
        Ok(initialized) => {
            println!("{}", describe_initialized(&initialized));
            println!(
                "Next: add files under src/, declare dependencies in manifest.jsonc, and run \
                 `assemblr unit build`."
            );
            ExitCode::SUCCESS
        }
        Err(failure) => fail(&failure.to_string(), Some(&init_hint(&failure))),
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

/// What remains after a failed initialization, and what the author can do.
fn init_hint(failure: &InitFailure) -> String {
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
            let written = if state.is_empty() {
                "nothing was written".to_owned()
            } else {
                format!("written so far: {}", state.join(", "))
            };
            let git = if matches!(failure, InitFailure::Init { .. }) {
                "; Assemblr needs Git installed and on the PATH"
            } else {
                ""
            };
            format!(
                "{written}{git}; fix the problem and run the same command again, which \
                 completes the scaffold"
            )
        }
        InitFailure::Inside { top_level, .. } => format!(
            "initialize the unit in a directory outside {}",
            top_level.display()
        ),
        InitFailure::OtherIdentity { declared, .. } => format!(
            "to work on {declared}, run `assemblr unit init {declared}`; for a new unit, choose \
             another directory"
        ),
        InitFailure::Git(_) => {
            "nothing was changed; Assemblr needs Git installed and on the PATH".to_owned()
        }
        _ => "nothing was changed".to_owned(),
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
        AmendProblem::UpdateInterrupted { .. } => {
            Some("run `assemblr unit update` to complete the interrupted update".to_owned())
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

fn update(args: &UpdateArgs) -> ExitCode {
    let mut units = Vec::new();
    for unit in &args.units {
        match UnitIdentity::parse(unit) {
            Ok(unit) => units.push(unit),
            Err(error) => return fail(&format!("{unit}: {error}"), None),
        }
    }
    let (root, registry) = match locate() {
        Ok(located) => located,
        Err(code) => return code,
    };
    match update_unit(&root, &registry, &units) {
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
            }
            println!("{result}");
            ExitCode::SUCCESS
        }
        Err(failure) => {
            let hint = update_hint(&failure, &root);
            fail(&failure.to_string(), hint.as_deref())
        }
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
    eprintln!("note: completed an update that was interrupted earlier: {replaced}");
}

/// What remains after a failed update, and what the author can do.
fn update_hint(failure: &UpdateFailure, root: &Path) -> Option<String> {
    let problems = match failure {
        UpdateFailure::Write(error) => {
            return Some(match error.state() {
                UpdateState::Unchanged => {
                    "nothing was changed; fix the problem and run the command again".to_owned()
                }
                UpdateState::Interrupted {
                    manifest_replaced,
                    lockfile_replaced,
                } => {
                    let replaced = match (manifest_replaced, lockfile_replaced) {
                        (true, true) => "manifest.jsonc and assemblr-lock.json were replaced",
                        (true, false) => "manifest.jsonc was replaced, assemblr-lock.json was not",
                        _ => "neither manifest.jsonc nor assemblr-lock.json was replaced",
                    };
                    format!(
                        "{replaced}; the update is recorded, and running `assemblr unit update` \
                         again completes it"
                    )
                }
            });
        }
        UpdateFailure::Rejected(problems) => problems,
    };
    // An interrupted update that could not be completed stops everything
    // else: the files are left for the author, with the record.
    if let Some(UpdateProblem::Recovery(error)) = problems
        .iter()
        .find(|problem| matches!(problem, UpdateProblem::Recovery(_)))
    {
        return Some(match error {
            RecoveryError::Write { .. } => "the record of the interrupted update remains; fix \
                                            the problem and run `assemblr unit update` again \
                                            to complete it"
                .to_owned(),
            _ => "nothing was changed now; the record of the interrupted update is kept, and \
                  only `assemblr unit update` completes it"
                .to_owned(),
        });
    }
    let mut hint = "nothing was changed".to_owned();
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
        UpdateProblem::PublishedAmender(_) => Some(
            "update the amender together with its provider, to a version that amends the \
             provider at the selected version, or leave both as they are"
                .to_owned(),
        ),
        UpdateProblem::Reconcile(_) => Some(
            "keep the locked version, or narrow the declared range to the versions whose file \
             is unchanged; reconciling a changed base is not supported yet"
                .to_owned(),
        ),
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
            warn_notes(built.notes());
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

fn check() -> ExitCode {
    let (root, registry) = match locate() {
        Ok(located) => located,
        Err(code) => return code,
    };
    match check_unit(&root, &registry) {
        Ok(checked) => {
            warn_notes(checked.notes());
            println!(
                "Checked {} {}: no problems; a build delivers {} file(s), with {} dependenc{}.",
                checked.unit(),
                checked.version(),
                checked.files(),
                checked.dependencies(),
                if checked.dependencies() == 1 {
                    "y"
                } else {
                    "ies"
                }
            );
            ExitCode::SUCCESS
        }
        Err(failure) => {
            let CheckFailure::Rejected(problems) = &failure;
            let mut hint = "nothing was changed".to_owned();
            if let Some(advice) = problems
                .iter()
                .find_map(|problem| check_advice(problem, &root, "check"))
            {
                hint.push_str("; ");
                hint.push_str(&advice);
            }
            fail(&failure.to_string(), Some(&hint))
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
        Err(failure) => {
            let hint = publish_hint(&failure, &root);
            fail(&failure.to_string(), Some(&hint))
        }
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

/// What remains after a refused publication, and what the author can do.
fn publish_hint(failure: &PublishFailure, root: &Path) -> String {
    let problems = match failure {
        PublishFailure::Tag(_) => {
            return "no tag was created and nothing was changed; a tag needs the author's Git \
                    identity (`user.name` and `user.email` in the Git configuration) and, if \
                    signing is configured, a working signing setup"
                .to_owned();
        }
        PublishFailure::IndexNotUpdated { .. } => {
            return "the published tag stands; run `assemblr registry sync` to index the version"
                .to_owned();
        }
        PublishFailure::Rejected(problems) => problems,
    };
    let mut hint =
        "the publication was rejected; no tag was created and nothing was changed".to_owned();
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
    if let Some(advice) = advice {
        hint.push_str("; ");
        hint.push_str(&advice);
    }
    hint
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
        BuildProblem::UpdateInterrupted { .. } => {
            "run `assemblr unit update` to complete the interrupted update".to_owned()
        }
        BuildProblem::Lock(LockError::ContentChanged { .. }) => format!(
            "published versions never change: restore the tag in the unit's repository; \
             {DELETING_THE_LOCKFILE}"
        ),
        BuildProblem::AmendmentBase(_) => {
            "a build never changes the locked version of an amended unit".to_owned()
        }
        BuildProblem::BaseIntegrity(_) => "published versions never change: either the \
                                           provider's tag was moved and its source registered \
                                           again, or the amender was published against other \
                                           content; run `assemblr registry sync` to check the \
                                           registered repositories"
            .to_owned(),
        BuildProblem::Merge { failure, .. } => match failure {
            Some(MergeFailure::Unreadable { .. }) | None => {
                "every side of a merge must be a JSON document: fix the copy, or the \
                 provider's file, so that it reads as JSON"
                    .to_owned()
            }
            Some(MergeFailure::Edit(_) | MergeFailure::Projection { .. }) => {
                "the provider's file has a layout the merge could not edit safely; nothing was \
                 written, and reformatting that file conventionally works around it"
                    .to_owned()
            }
            Some(MergeFailure::Conflicts(_)) => "the amendments of one file must agree: change \
                                                 the copies so that no two amenders set one \
                                                 value differently, or expose the value as a \
                                                 parameter of the provider"
                .to_owned(),
        },
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
