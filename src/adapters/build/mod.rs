//! `unit build`: the unit's working tree, composed with the published
//! versions of its dependencies, written to `build/` (see
//! `docs/domain/authoring-workflow.md` and `docs/adr/0008-unit-build.md`),
//! and `unit amend`, which shares its reading of the unit and its
//! dependencies (see [`amend`]).
//!
//! A build is decided completely before anything is written. It reads the
//! unit's `manifest.jsonc`, `src/`, and `amendments/` from its working tree,
//! resolves its dependencies from the registry index under the unit
//! lockfile, reads each resolved version's content from Git at the commit
//! the index records, and composes the result. A resolved version that
//! declares amendments is read with its `amendments/` and its lockfile, and
//! each base the lockfile identifies is verified against the registry index
//! before composition (see `docs/domain/amendments-and-composition.md`).
//! Every problem found along the way is reported, and then nothing is
//! written. Only a build without problems is published, together with its
//! lockfile (see [`output`]).

mod add;
mod amend;
mod check;
mod exclusive;
mod layering;
pub use layering::UnresolvedFile;
pub(crate) use layering::{layer_sources, render_files};
mod output;
mod publish;
mod update;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub use add::{AddFailure, AddProblem, AddedParameter, DependencyAdded, add_dependency};
pub use amend::{AmendFailure, AmendProblem, AmendState, Amended, Match, WriteError, amend_unit};
pub use check::{CheckFailure, CheckProblem, Checked, check_unit};
use exclusive::BuildLock;
pub use exclusive::BuildLockError;
pub use output::{OutputProblem, PublishError, State as PublishState};
pub use publish::{IndexOutcome, PublishFailure, PublishProblem, Published, publish_unit};
pub use update::{
    Carried, FileState, Recovered, RecoveryError, UpdateFailure, UpdateProblem, UpdateState,
    Updated, WriteError as UpdateWriteError, update_unit,
};

use super::git::{GitError, GitRepository, InvalidVersion, MANIFEST_LIMIT, ReadSrcError};
use super::lockfile::{self, LockfileError, PublishedLockfileError};
use super::manifest::{ReadManifestError, read_manifest};
use super::registry::{Registry, StorageError};
use super::render::RenderError;
use crate::domain::{
    AmendmentBaseError, AvailableVersions, BaseIntegrityError, CompositionError, ContributionError,
    FileTree, LockChange, LockError, Manifest, ObjectId, ParameterName, PublishedAmendmentError,
    RelativePath, Requirer, ResolutionError, Root, TEMPLATE_SUFFIX, UnitContent, UnitFile,
    UnitIdentity, UnitLock, ValueKind, Version, amendment_bases, compose, contribution_problems,
    published_amendments, relock, resolve, verify_bases,
};

/// The unit's manifest, at the repository's top level.
const MANIFEST: &str = "manifest.jsonc";

/// Builds the unit whose repository's top level is `root`, resolving its
/// dependencies from `registry`.
pub fn build_unit(root: &Path, registry: &Registry) -> Result<Built, BuildFailure> {
    let mut opened = open(root, registry)?;
    let manifest = opened.manifest.clone();
    let validated = validate_opened(&mut opened, &manifest)?;
    let text = opened.text.clone();
    publish_validated(root, &opened, &text, &manifest, validated)
}

/// A unit read from its working tree for a build, under the lock that
/// serializes builds of the unit (see [`open`]).
struct Opened {
    guard: BuildLock,
    /// The text of the working tree's `manifest.jsonc`.
    text: String,
    manifest: Manifest,
    /// The `src/` files, or `None` when they could not be read, which
    /// `problems` reports.
    src: Option<FileTree>,
    /// The amendment copies, or `None` as for `src`.
    amendments: Option<FileTree>,
    /// The lockfile, when there is one.
    previous: Option<UnitLock>,
    index: crate::domain::RegistryIndex,
    /// The problems found so far, which reject the build once everything
    /// else is validated.
    problems: Vec<BuildProblem>,
}

/// Reads everything a build of the unit at `root` starts from, and takes
/// the build lock, which the result holds. A unit whose manifest, lockfile,
/// or registry index cannot be read is rejected here, with every problem
/// found; other problems are kept for the validation to report together
/// with its own.
fn open(root: &Path, registry: &Registry) -> Result<Opened, BuildFailure> {
    let manifest_path = root.join(MANIFEST);
    if let Err(error) = fs::symlink_metadata(&manifest_path) {
        if error.kind() == io::ErrorKind::NotFound {
            // Not a unit's directory, so nothing else here is worth reporting.
            return Err(BuildFailure::Rejected(vec![BuildProblem::Manifest(
                ManifestFileError::Missing {
                    path: manifest_path,
                },
            )]));
        }
    }
    let mut problems = Vec::new();
    let repository = match GitRepository::open(root) {
        Ok(repository) => repository,
        Err(error) => {
            problems.push(BuildProblem::Repository(error));
            if let Err(error) = read_working_manifest(root) {
                problems.push(BuildProblem::Manifest(error));
            }
            return Err(BuildFailure::Rejected(problems));
        }
    };
    // Everything from here on reads or writes what another build of this
    // unit may change: its manifest and files, its lockfile, `build/`, and
    // what an interrupted build left. Builds of the unit therefore run one
    // at a time, from here until the build is published.
    let git_dir = repository
        .git_directory()
        .map_err(|error| BuildFailure::Rejected(vec![BuildProblem::Repository(error)]))?;
    let guard = BuildLock::acquire(&git_dir)
        .map_err(|error| BuildFailure::Rejected(vec![BuildProblem::BuildLock(error)]))?;
    // An interrupted update leaves the manifest and the lockfile in a state
    // only `unit update` can complete.
    if let Some(journal) = update::interrupted(&git_dir)
        .map_err(|error| BuildFailure::Rejected(vec![BuildProblem::GitDirectory(error)]))?
    {
        return Err(BuildFailure::Rejected(vec![
            BuildProblem::UpdateInterrupted { journal },
        ]));
    }

    let text = read_working_manifest_text(root)
        .map_err(|error| problems.push(BuildProblem::Manifest(error)))
        .ok();
    let manifest = text.as_deref().and_then(|text| {
        read_manifest(text)
            .map_err(|error| {
                problems.push(BuildProblem::Manifest(ManifestFileError::Invalid {
                    path: manifest_path.clone(),
                    error,
                }));
            })
            .ok()
    });
    let src = match repository.read_working_src() {
        Ok(src) => Some(src),
        Err(ReadSrcError::Git(error)) => {
            problems.push(BuildProblem::Repository(error));
            None
        }
        Err(error) => {
            problems.push(BuildProblem::Src(error));
            None
        }
    };
    let amendments = match repository.read_working_amendments() {
        Ok(amendments) => Some(amendments),
        Err(ReadSrcError::Git(error)) => {
            problems.push(BuildProblem::Repository(error));
            None
        }
        Err(error) => {
            problems.push(BuildProblem::Amendments(error));
            None
        }
    };
    // If the record cannot be read, nothing at the reserved paths is taken
    // for a leftover, so nothing there is deleted.
    let interrupted = guard.interrupted().unwrap_or(false);
    problems.extend(
        output::check(root, interrupted)
            .into_iter()
            .map(BuildProblem::Output),
    );
    let previous = lockfile::read(root)
        .map_err(|error| problems.push(BuildProblem::Lockfile(error)))
        .ok();
    let index = registry
        .index()
        .map_err(|error| problems.push(BuildProblem::Registry(error)))
        .ok();

    // Dependencies, as far as the unit's own inputs allow.
    let (Some(text), Some(manifest), Some(previous), Some(index)) =
        (text, manifest, previous, index)
    else {
        return Err(BuildFailure::Rejected(problems));
    };
    Ok(Opened {
        guard,
        text,
        manifest,
        src,
        amendments,
        previous,
        index,
        problems,
    })
}

/// Validates `manifest`, the working tree's or an edited one, with the
/// files and the lock `opened` read, as a build does. Every problem, those
/// found while opening included, rejects the build.
fn validate_opened(opened: &mut Opened, manifest: &Manifest) -> Result<Validated, BuildFailure> {
    let mut problems = Vec::new();
    let previous_lock = opened.previous.clone().unwrap_or_default();
    let inputs = Inputs {
        manifest,
        src: opened.src.as_ref(),
        amendments: opened.amendments.as_ref(),
        previous: &previous_lock,
    };
    // A build composes an ineffective unit as readily as any other: the
    // effective-contribution check gates publication (OC-11).
    let validated = validate(&inputs, &opened.index, &mut problems).result;
    let validated = validated.filter(|_| problems.is_empty() && opened.problems.is_empty());
    match validated {
        Some(validated) => Ok(validated),
        None => {
            let mut all = std::mem::take(&mut opened.problems);
            all.append(&mut problems);
            Err(BuildFailure::Rejected(all))
        }
    }
}

/// Writes `validated`, the build of `manifest`, as `build/` and the lockfile.
fn publish_validated(
    root: &Path,
    opened: &Opened,
    text: &str,
    manifest: &Manifest,
    validated: Validated,
) -> Result<Built, BuildFailure> {
    if !validated.pending.is_empty() {
        return Err(write_pending(root, opened, text, manifest, validated));
    }
    let lock = validated.lock;
    let lockfile = match &opened.previous {
        None => LockfileOutcome::Created,
        Some(previous) if *previous == lock => LockfileOutcome::Unchanged,
        Some(_) => LockfileOutcome::Updated,
    };
    let files: Vec<(&RelativePath, &UnitFile)> = validated
        .files
        .iter()
        .map(|(path, file)| (path, file))
        .collect();
    let new_lock = (lockfile != LockfileOutcome::Unchanged).then_some(&lock);
    let published =
        output::publish(root, &files, new_lock, &opened.guard).map_err(BuildFailure::Publish)?;
    Ok(Built {
        unit: manifest.name().clone(),
        version: manifest.version().clone(),
        files: files.len(),
        dependencies: lock.units().len(),
        lockfile,
        changes: validated.changes,
        notes: validated.notes,
        leftover: published.leftover,
    })
}

/// Writes the pending state: each marked copy under `amendments/`, its
/// declaration in the manifest, and the lockfile, since the resolution is
/// sound and the conflict is a matter of content. `build/` is not touched.
/// The copies are created first, new files only, then the manifest is
/// replaced through its staging file, then the lockfile through its own.
fn write_pending(
    root: &Path,
    opened: &Opened,
    text: &str,
    manifest: &Manifest,
    validated: Validated,
) -> BuildFailure {
    let staged = root.join(amend::MANIFEST_NEW);
    let lock_staging = root.join(output::LOCK_STAGING);
    let mut problems = Vec::new();
    for path in [&staged, &lock_staging] {
        if fs::symlink_metadata(path).is_ok() {
            problems.push(BuildProblem::StagingOccupied { path: path.clone() });
        }
    }
    let mut edited = text.to_owned();
    let mut declared = manifest.clone();
    let mut placements = Vec::new();
    for pending in &validated.pending {
        let declaration = crate::domain::AmendmentDeclaration::new(
            pending.provider.clone(),
            pending.base.clone(),
        );
        match crate::adapters::manifest::declare_amendment(
            &edited,
            &declared,
            &pending.source,
            &declaration,
        ) {
            Ok(text) => edited = text,
            Err(error) => {
                problems.push(BuildProblem::ManifestEdit(error));
                continue;
            }
        }
        if let Some(next) = declared.with_amendment(pending.source.clone(), declaration) {
            declared = next;
        }
        placements.push(pending);
    }
    // The directories the copies need, each once: two copies can share a
    // new one.
    let mut missing = Vec::new();
    match amend::real_directories_of(root, placements.iter().map(|pending| &pending.source)) {
        Ok(directories) => missing = directories,
        Err(amend::DirectoryProblem::NotADirectory(path)) => {
            problems.push(BuildProblem::NotADirectory { path });
        }
        Err(amend::DirectoryProblem::Unreadable(path, error)) => {
            problems.push(BuildProblem::Unreadable { path, error });
        }
    }
    if !problems.is_empty() {
        return BuildFailure::Rejected(problems);
    }
    let lock_bytes = match lockfile::serialize(&validated.lock) {
        Ok(bytes) => bytes,
        Err(error) => {
            return BuildFailure::PendingWrite {
                path: root.join(lockfile::LOCKFILE),
                error,
                written: Vec::new(),
            };
        }
    };
    for directory in &missing {
        if let Err(error) = fs::create_dir(directory) {
            amend::remove_directories(&missing);
            return BuildFailure::PendingWrite {
                path: directory.clone(),
                error,
                written: Vec::new(),
            };
        }
    }
    let mut written = Vec::new();
    for pending in &placements {
        let copy_path = root
            .join(super::git::UnitDirectory::Amendments.as_str())
            .join(pending.source.as_str());
        if let Err(error) = amend::write_copy(&copy_path, &pending.copy, &[]) {
            let _ = fs::remove_file(&copy_path);
            if written.is_empty() {
                amend::remove_directories(&missing);
            }
            return BuildFailure::PendingWrite {
                path: copy_path,
                error,
                written,
            };
        }
        written.push(pending.source.clone());
    }
    if let Err(error) = amend::write_manifest(root, &staged, edited.as_bytes()) {
        let _ = fs::remove_file(&staged);
        return BuildFailure::PendingWrite {
            path: root.join(MANIFEST),
            error,
            written,
        };
    }
    written.push(
        RelativePath::parse(MANIFEST).unwrap_or_else(|_| validated.pending[0].source.clone()),
    );
    let lockfile_outcome = match &opened.previous {
        None => LockfileOutcome::Created,
        Some(previous) if *previous == validated.lock => LockfileOutcome::Unchanged,
        Some(_) => LockfileOutcome::Updated,
    };
    if lockfile_outcome != LockfileOutcome::Unchanged {
        let replaced = fs::write(&lock_staging, &lock_bytes)
            .and_then(|()| fs::rename(&lock_staging, root.join(lockfile::LOCKFILE)));
        if let Err(error) = replaced {
            let _ = fs::remove_file(&lock_staging);
            return BuildFailure::PendingWrite {
                path: root.join(lockfile::LOCKFILE),
                error,
                written,
            };
        }
    }
    BuildFailure::Pending(Pending {
        files: validated.pending,
        lockfile: lockfile_outcome,
    })
}

/// Reads the working tree's `manifest.jsonc`.
fn read_working_manifest(root: &Path) -> Result<Manifest, ManifestFileError> {
    let text = read_working_manifest_text(root)?;
    read_manifest(&text).map_err(|error| ManifestFileError::Invalid {
        path: root.join(MANIFEST),
        error,
    })
}

/// Reads the text of the working tree's `manifest.jsonc`.
fn read_working_manifest_text(root: &Path) -> Result<String, ManifestFileError> {
    let path = root.join(MANIFEST);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(ManifestFileError::Missing { path });
        }
        Err(error) => return Err(ManifestFileError::Io { path, error }),
    };
    if !metadata.is_file() {
        return Err(ManifestFileError::NotAFile { path });
    }
    let mut bytes = Vec::new();
    fs::File::open(&path)
        .and_then(|file| file.take(MANIFEST_LIMIT + 1).read_to_end(&mut bytes))
        .map_err(|error| ManifestFileError::Io {
            path: path.clone(),
            error,
        })?;
    if bytes.len() as u64 > MANIFEST_LIMIT {
        return Err(ManifestFileError::TooLarge {
            path,
            limit: MANIFEST_LIMIT,
        });
    }
    String::from_utf8(bytes).map_err(|_| ManifestFileError::NotUtf8 { path })
}

/// What a unit is validated from: its manifest, its `src/` files, its
/// amendment copies, and the lock it is resolved under. A build and a check
/// read them from the working tree, a publication from the commit at
/// `HEAD`.
pub(super) struct Inputs<'a> {
    pub(super) manifest: &'a Manifest,
    /// The `src/` files, or `None` when they could not be read, which is
    /// reported separately; the dependencies are still validated.
    pub(super) src: Option<&'a FileTree>,
    /// The amendment copies, or `None` as for `src`.
    pub(super) amendments: Option<&'a FileTree>,
    /// The previous lock, empty when there is no lockfile.
    pub(super) previous: &'a UnitLock,
}

/// What validating a unit found.
pub(super) struct Validation {
    /// The problems with the unit's contribution, which gate publication
    /// and never a build (OC-11). Empty when resolution did not finish.
    pub(super) contribution: Vec<ContributionError>,
    /// The validated unit, when every validation passed.
    pub(super) result: Option<Validated>,
}

/// A unit that passed every validation a build runs.
pub(super) struct Validated {
    /// The lock the resolution produces.
    pub(super) lock: UnitLock,
    /// How that lock differs from the previous one.
    pub(super) changes: BTreeMap<UnitIdentity, LockChange>,
    /// The composed files, rendered, by delivered path.
    pub(super) files: Vec<(RelativePath, UnitFile)>,
    /// What the composition reports without failing: parts of amendment
    /// copies the merge could not carry, each naming the file.
    pub(super) notes: Vec<String>,
    /// Files the unit does not amend whose sibling amenders conflict:
    /// the build creates the marked copies instead of `build/` (the
    /// pending state, see `docs/domain/authoring-workflow.md`).
    pub(super) pending: Vec<PendingFile>,
}

/// A copy the pending state creates: the composition of a file with the
/// conflict between its sibling amenders marked, for the unit to resolve.
#[derive(Debug, Clone)]
pub struct PendingFile {
    /// The path in the provider's `src/`, and the copy's under
    /// `amendments/`.
    source: RelativePath,
    /// The provider.
    provider: UnitIdentity,
    /// Its locked version, the copy's base.
    base: Version,
    /// The amenders whose contributions conflict.
    amenders: Vec<UnitIdentity>,
    /// Each conflict, as the merge reports it.
    conflicts: Vec<String>,
    /// The marked copy.
    copy: Vec<u8>,
}

impl PendingFile {
    /// The copy's path under `amendments/`.
    pub fn path(&self) -> &RelativePath {
        &self.source
    }

    /// The provider of the amended file.
    pub fn provider(&self) -> &UnitIdentity {
        &self.provider
    }

    /// The provider's version the copy is based on.
    pub fn base(&self) -> &Version {
        &self.base
    }

    /// The amenders whose contributions conflict, by identity.
    pub fn amenders(&self) -> &[UnitIdentity] {
        &self.amenders
    }

    /// Each conflict, as the merge reports it.
    pub fn conflicts(&self) -> &[String] {
        &self.conflicts
    }
}

/// The values a unit is built with: a required parameter is built with its
/// example, for this unit only, and a consumer never sees them. The
/// parameters without a default or an example are given too.
pub(super) fn development_values(
    manifest: &Manifest,
) -> (
    BTreeMap<UnitIdentity, crate::domain::OptionValues>,
    Vec<ParameterName>,
) {
    let mut development = crate::domain::OptionValues::new();
    let mut parameters: Vec<ParameterName> = Vec::new();
    for (name, definition) in manifest.params() {
        if definition.default().is_some() {
            continue;
        }
        match definition.example() {
            Some(example) => {
                development.insert(name.clone(), example.clone());
            }
            None => parameters.push(name.clone()),
        }
    }
    let development = if development.is_empty() {
        BTreeMap::new()
    } else {
        BTreeMap::from([(manifest.name().clone(), development)])
    };
    (development, parameters)
}

/// The pending copy for each file of `unresolved` that `manifest`, the
/// unit being built, does not amend: the sources layered, with the
/// conflict marked, for the unit to resolve. The files it amends are given
/// back: its copy resolved what it saw, and the rest is the caller's to
/// report.
pub(super) fn pending_files(
    composition: &crate::domain::Composition<'_>,
    unresolved: Vec<UnresolvedFile>,
    lock: &UnitLock,
    manifest: &Manifest,
    dependencies: &Dependencies,
    problems: &mut Vec<BuildProblem>,
) -> (Vec<PendingFile>, Vec<UnresolvedFile>) {
    let mut pending = Vec::new();
    let mut kept = Vec::new();
    for unresolved in unresolved {
        let Some(locked) = lock.units().get(&unresolved.provider) else {
            kept.push(unresolved);
            continue;
        };
        if manifest.amendments().contains_key(&unresolved.source) {
            kept.push(unresolved);
            continue;
        }
        let copy = layer_sources(
            composition,
            &unresolved.delivered,
            lock,
            Some(manifest.name()),
            dependencies,
            problems,
        );
        let Some(copy) = copy else {
            continue;
        };
        pending.push(PendingFile {
            source: unresolved.source.clone(),
            provider: unresolved.provider.clone(),
            base: locked.version().clone(),
            amenders: unresolved.amenders.clone(),
            conflicts: unresolved
                .conflicts
                .iter()
                .map(ToString::to_string)
                .collect(),
            copy,
        });
    }
    (pending, kept)
}

/// `manifest` without its amendments: the root of a composition that
/// gives the bases of the unit's own amendments, which are no part of
/// them, and that is not disturbed by a copy an interrupted run left
/// undeclared or by one that still holds conflict markers.
pub(super) fn without_amendments(manifest: &Manifest) -> Manifest {
    Manifest::new(
        manifest.name().clone(),
        manifest.version().clone(),
        manifest.description().clone(),
        manifest.dependencies().clone(),
        manifest.options().clone(),
        manifest.params().clone(),
        BTreeMap::new(),
    )
    .unwrap_or_else(|_| manifest.clone())
}

/// The resolved, verified dependencies of a root: a unit, or a consumer
/// project when `name` is `None`.
pub(crate) struct Resolved {
    pub(crate) resolution: crate::domain::Resolution,
    pub(crate) lock: UnitLock,
    pub(crate) dependencies: Dependencies,
    contents: BTreeMap<UnitIdentity, PublishedContent>,
}

impl Resolved {
    /// The content of every locked unit, for composition, after each
    /// published amender's bases were verified against the index.
    pub(crate) fn units(&self) -> BTreeMap<UnitIdentity, UnitContent<'_>> {
        let mut units = BTreeMap::new();
        for (identity, locked) in self.lock.units() {
            let key = (identity.clone(), locked.version().clone());
            let (Some(manifest), Some(content)) = (
                self.dependencies.manifests.get(&key),
                self.contents.get(identity),
            ) else {
                continue;
            };
            units.insert(
                identity.clone(),
                UnitContent::new(manifest, &content.files, &content.amendments),
            );
        }
        units
    }

    /// The locked version and `src/` files of each locked unit.
    pub(super) fn providers(&self) -> BTreeMap<&UnitIdentity, (&Version, &FileTree)> {
        self.lock
            .units()
            .iter()
            .filter_map(|(identity, locked)| {
                let content = self.contents.get(identity)?;
                Some((identity, (locked.version(), &content.files)))
            })
            .collect()
    }
}

/// Resolves `requirements` from `index` under `previous`, reads every
/// locked version's content, and verifies each published amender's bases.
/// Every problem is added to `problems`, and the result is given only when
/// resolution and reading succeeded; base problems are reported alongside
/// a result, for the composition to report with its own.
pub(crate) fn resolve_graph(
    name: Option<&UnitIdentity>,
    requirements: &crate::domain::Dependencies,
    previous: &UnitLock,
    index: &crate::domain::RegistryIndex,
    problems: &mut Vec<BuildProblem>,
) -> Option<Resolved> {
    let dependencies = match Dependencies::read_for(index, name, requirements) {
        Ok(dependencies) => dependencies,
        Err(found) => {
            problems.extend(found);
            return None;
        }
    };
    let resolution = match resolve(
        name,
        requirements,
        &dependencies.available,
        &previous.versions(),
    ) {
        Ok(resolution) => resolution,
        Err(errors) => {
            problems.extend(errors.into_iter().map(BuildProblem::Resolution));
            return None;
        }
    };
    let lock = match relock(previous, &resolution, index) {
        Ok(lock) => lock,
        Err(errors) => {
            problems.extend(errors.into_iter().map(BuildProblem::Lock));
            return None;
        }
    };
    let contents = match dependencies.read_content(&lock, lock.units().keys()) {
        Ok(contents) => contents,
        Err(found) => {
            problems.extend(found);
            return None;
        }
    };
    // Each published amender's bases are verified against the index here,
    // whatever else the composition finds: a disagreement is an integrity
    // conflict in its own right.
    for (identity, locked) in lock.units() {
        let key = (identity.clone(), locked.version().clone());
        let (Some(manifest), Some(content)) =
            (dependencies.manifests.get(&key), contents.get(identity))
        else {
            continue;
        };
        if let Some(published) = &content.lock {
            problems.extend(
                verify_bases(
                    identity,
                    locked.version(),
                    manifest,
                    published,
                    index,
                    |unit, version| dependencies.manifests.get(&(unit.clone(), version.clone())),
                )
                .into_iter()
                .map(|error| BuildProblem::BaseIntegrity(Box::new(error))),
            );
        }
    }
    Some(Resolved {
        resolution,
        lock,
        dependencies,
        contents,
    })
}

/// Runs every validation that gates a build on `inputs` (see
/// `docs/domain/authoring-workflow.md`): development values, resolution
/// under the lock against `index`, the lock's integrity, the amendment
/// bases, the published content of every locked version, the bases of
/// published amenders, composition, and rendering. Every problem found is
/// added to `problems`, and the result is given only when none was.
pub(super) fn validate(
    inputs: &Inputs<'_>,
    index: &crate::domain::RegistryIndex,
    problems: &mut Vec<BuildProblem>,
) -> Validation {
    let Inputs {
        manifest,
        src,
        amendments,
        previous,
    } = *inputs;
    let mut validation = Validation {
        contribution: Vec::new(),
        result: None,
    };
    let (development, parameters) = development_values(manifest);
    if !parameters.is_empty() {
        problems.push(BuildProblem::DevelopmentValues {
            unit: manifest.name().clone(),
            parameters,
        });
    }
    let Some(resolved) = resolve_graph(
        Some(manifest.name()),
        manifest.dependencies(),
        previous,
        index,
        problems,
    ) else {
        return validation;
    };
    // A build never moves an amended unit away from its base. The cause is
    // told apart here, with the previous lock; the composition below would
    // only see that the base and the resolved version differ.
    let moved: BTreeSet<RelativePath> = amendment_bases(manifest, previous, &resolved.resolution)
        .into_iter()
        .map(|error| {
            let path = match &error {
                AmendmentBaseError::ResolutionChanged { path, .. }
                | AmendmentBaseError::DeclarationChanged { path, .. }
                | AmendmentBaseError::NotLocked { path, .. } => path.clone(),
            };
            problems.push(BuildProblem::AmendmentBase(error));
            path
        })
        .collect();
    let (Some(src), Some(amendments)) = (src, amendments) else {
        return validation;
    };
    // A copy that still holds a conflict marker is not a copy yet: the
    // author resolves it first, and nothing is composed from it.
    let marked: Vec<RelativePath> = amendments
        .iter()
        .filter(|(_, copy)| super::layered::has_markers(copy.contents()))
        .map(|(path, _)| path.clone())
        .collect();
    if !marked.is_empty() {
        problems.push(BuildProblem::Markers { paths: marked });
        return validation;
    }
    validation.contribution =
        contribution_problems(manifest, src, amendments, &resolved.providers());
    let units = resolved.units();
    let root_content = UnitContent::new(manifest, src, amendments);
    let composition = compose(
        resolved.resolution.graph(),
        Root::Unit(root_content),
        &units,
        &development,
    );
    let composition = match composition {
        Ok(composition) => composition,
        Err(errors) => {
            problems.extend(
                errors
                    .into_iter()
                    // Already reported, from the manifest alone, or with
                    // the cause, from the lock.
                    .filter(|error| match error {
                        CompositionError::MissingValue { unit, .. } => unit != manifest.name(),
                        CompositionError::AmendmentBase {
                            amender: Requirer::Root,
                            path,
                            ..
                        } => !moved.contains(path),
                        _ => true,
                    })
                    .map(BuildProblem::Composition),
            );
            return validation;
        }
    };
    let Some(rendered) = render_files(
        &composition,
        &resolved.lock,
        Some(manifest.name()),
        &resolved.dependencies,
        problems,
    ) else {
        return validation;
    };
    // A conflict between siblings in a file the unit does not amend is
    // the unit's to resolve: the pending state gives it the marked copy
    // of the sources. In a file it amends, its copy resolved what it saw
    // and the rest is reported.
    let (pending, kept) = pending_files(
        &composition,
        rendered.unresolved,
        &resolved.lock,
        manifest,
        &resolved.dependencies,
        problems,
    );
    problems.extend(kept.into_iter().map(BuildProblem::Unresolved));
    if !problems.is_empty() {
        return validation;
    }
    validation.result = Some(Validated {
        lock: resolved.lock,
        changes: resolved.resolution.changes().clone(),
        files: rendered.files,
        notes: rendered.notes.into_iter().map(|(_, note)| note).collect(),
        pending,
    });
    validation
}

/// The published versions a resolution may choose from, read from the
/// registered repositories.
pub(crate) struct Dependencies {
    available: AvailableVersions,
    /// Each available version's manifest.
    manifests: BTreeMap<(UnitIdentity, Version), Manifest>,
    /// Each identity's repository, by identity.
    repositories: BTreeMap<UnitIdentity, GitRepository>,
    /// Each available version's commit.
    commits: BTreeMap<(UnitIdentity, Version), ObjectId>,
}

impl Dependencies {
    /// The manifest of `identity` at `version`, when that version was read.
    pub(crate) fn manifest(&self, identity: &UnitIdentity, version: &Version) -> Option<&Manifest> {
        self.manifests.get(&(identity.clone(), version.clone()))
    }

    /// Reads every published version of every identity the unit can reach
    /// through some version's dependencies, starting from its own, at the
    /// commits `index` records. Identities without a registered source are
    /// left for resolution to report. Every repository that cannot be read
    /// and every version whose content no longer matches the index is
    /// reported, sorted by unit and version.
    fn read(
        index: &crate::domain::RegistryIndex,
        root: &Manifest,
    ) -> Result<Self, Vec<BuildProblem>> {
        Self::read_for(index, Some(root.name()), root.dependencies())
    }

    /// [`read`](Self::read) for a root that is a consumer project (`name`
    /// is `None`) or a unit, with its direct `requirements`.
    pub(super) fn read_for(
        index: &crate::domain::RegistryIndex,
        name: Option<&UnitIdentity>,
        requirements: &crate::domain::Dependencies,
    ) -> Result<Self, Vec<BuildProblem>> {
        let mut dependencies = Self {
            available: AvailableVersions::new(),
            manifests: BTreeMap::new(),
            repositories: BTreeMap::new(),
            commits: BTreeMap::new(),
        };
        let mut problems = Vec::new();
        let mut pending: BTreeSet<UnitIdentity> = requirements.keys().cloned().collect();
        let mut seen = BTreeSet::new();
        while let Some(identity) = pending.pop_first() {
            if !seen.insert(identity.clone()) || Some(&identity) == name {
                continue;
            }
            let Some(unit) = index.units().get(&identity) else {
                continue;
            };
            let unreadable = |error| BuildProblem::Source {
                unit: identity.clone(),
                error,
            };
            let commits: Vec<&ObjectId> = unit
                .versions()
                .values()
                .map(|provenance| provenance.commit())
                .collect();
            let repository = match Registry::open_registered(unit.source(), &commits) {
                Ok(repository) => repository,
                Err(error) => {
                    problems.push(unreadable(error));
                    continue;
                }
            };
            let outcomes = match repository.read_published(&commits) {
                Ok(outcomes) => outcomes,
                Err(error) => {
                    problems.push(unreadable(error));
                    continue;
                }
            };
            let mut versions = BTreeMap::new();
            for ((version, provenance), outcome) in unit.versions().iter().zip(outcomes) {
                let problem = |problem| BuildProblem::Published {
                    unit: identity.clone(),
                    version: version.clone(),
                    problem,
                };
                let published = match outcome {
                    Ok(published) => published,
                    Err(invalid) => {
                        problems.push(problem(PublishedProblem::Invalid(invalid)));
                        continue;
                    }
                };
                let manifest = published.manifest();
                if manifest.name() != &identity
                    || manifest.version() != version
                    || published.provenance() != provenance
                {
                    problems.push(problem(PublishedProblem::NotIndexed));
                    continue;
                }
                pending.extend(manifest.dependencies().keys().cloned());
                versions.insert(version.clone(), manifest.dependencies().clone());
                let key = (identity.clone(), version.clone());
                dependencies
                    .commits
                    .insert(key.clone(), provenance.commit().clone());
                dependencies.manifests.insert(key, manifest.clone());
            }
            dependencies.available.insert(identity.clone(), versions);
            dependencies.repositories.insert(identity, repository);
        }
        if problems.is_empty() {
            Ok(dependencies)
        } else {
            Err(problems)
        }
    }

    /// Reads the `src/` files of `identity` at `version`, or `None` when
    /// the index does not record that version.
    fn read_src_at(
        &self,
        identity: &UnitIdentity,
        version: &Version,
    ) -> Option<Result<FileTree, BuildProblem>> {
        let key = (identity.clone(), version.clone());
        let (repository, commit) = (self.repositories.get(identity)?, self.commits.get(&key)?);
        Some(repository.read_src(commit).map_err(|error| match error {
            ReadSrcError::Git(error) => BuildProblem::Source {
                unit: identity.clone(),
                error,
            },
            error => BuildProblem::Published {
                unit: identity.clone(),
                version: version.clone(),
                problem: PublishedProblem::Src(error),
            },
        }))
    }

    /// Reads the `amendments/` copies of `identity` at `version`, or
    /// `None` when the index does not record that version.
    fn read_amendments_at(
        &self,
        identity: &UnitIdentity,
        version: &Version,
    ) -> Option<Result<FileTree, BuildProblem>> {
        let key = (identity.clone(), version.clone());
        let (repository, commit) = (self.repositories.get(identity)?, self.commits.get(&key)?);
        Some(
            repository
                .read_amendments(commit)
                .map_err(|error| match error {
                    ReadSrcError::Git(error) => BuildProblem::Source {
                        unit: identity.clone(),
                        error,
                    },
                    error => BuildProblem::Published {
                        unit: identity.clone(),
                        version: version.clone(),
                        problem: PublishedProblem::Amendments(error),
                    },
                }),
        )
    }

    /// Reads the lockfile of `identity` at `version`, or `None` when the
    /// index does not record that version. A version without a lockfile
    /// reads as `Ok(None)`.
    fn read_lock_at(
        &self,
        identity: &UnitIdentity,
        version: &Version,
    ) -> Option<Result<Option<UnitLock>, BuildProblem>> {
        let key = (identity.clone(), version.clone());
        let (repository, commit) = (self.repositories.get(identity)?, self.commits.get(&key)?);
        Some(
            lockfile::read_published(repository, commit).map_err(|error| match error {
                PublishedLockfileError::Read(super::git::ReadFileError::Git(error)) => {
                    BuildProblem::Source {
                        unit: identity.clone(),
                        error,
                    }
                }
                error => BuildProblem::Published {
                    unit: identity.clone(),
                    version: version.clone(),
                    problem: PublishedProblem::Lockfile(error),
                },
            }),
        )
    }

    /// Reads the content of the locked versions of `identities`: the
    /// `src/` files of each, and, for a version whose manifest declares
    /// amendments, its `amendments/` copies and its lockfile, which must
    /// identify each base.
    fn read_content<'a>(
        &self,
        lock: &UnitLock,
        identities: impl IntoIterator<Item = &'a UnitIdentity>,
    ) -> Result<BTreeMap<UnitIdentity, PublishedContent>, Vec<BuildProblem>> {
        let mut contents = BTreeMap::new();
        let mut problems = Vec::new();
        for identity in identities {
            let Some(locked) = lock.units().get(identity) else {
                continue;
            };
            let key = (identity.clone(), locked.version().clone());
            let (Some(repository), Some(commit), Some(manifest)) = (
                self.repositories.get(identity),
                self.commits.get(&key),
                self.manifests.get(&key),
            ) else {
                continue;
            };
            let source = |error| BuildProblem::Source {
                unit: identity.clone(),
                error,
            };
            let published = |problem| BuildProblem::Published {
                unit: identity.clone(),
                version: locked.version().clone(),
                problem,
            };
            let mut content = PublishedContent::default();
            match repository.read_src(commit) {
                Ok(files) => content.files = files,
                Err(ReadSrcError::Git(error)) => problems.push(source(error)),
                Err(error) => problems.push(published(PublishedProblem::Src(error))),
            }
            if manifest.amendments().is_empty() {
                contents.insert(identity.clone(), content);
                continue;
            }
            match repository.read_amendments(commit) {
                Ok(copies) => content.amendments = copies,
                Err(ReadSrcError::Git(error)) => problems.push(source(error)),
                Err(error) => problems.push(published(PublishedProblem::Amendments(error))),
            }
            match lockfile::read_published(repository, commit) {
                Ok(lock) => content.lock = lock,
                Err(PublishedLockfileError::Read(super::git::ReadFileError::Git(error))) => {
                    problems.push(source(error));
                }
                Err(error) => problems.push(published(PublishedProblem::Lockfile(error))),
            }
            // The copies are checked by the composition; the lockfile must
            // identify every base before any base can be verified.
            let bases: Vec<PublishedAmendmentError> =
                published_amendments(manifest, content.amendments.keys(), content.lock.as_ref())
                    .into_iter()
                    .filter(|problem| {
                        matches!(
                            problem,
                            PublishedAmendmentError::NoLockfile
                                | PublishedAmendmentError::NotLocked { .. }
                                | PublishedAmendmentError::BaseNotLocked { .. }
                        )
                    })
                    .collect();
            if !bases.is_empty() {
                problems.push(published(PublishedProblem::Bases(bases)));
            }
            contents.insert(identity.clone(), content);
        }
        if problems.is_empty() {
            Ok(contents)
        } else {
            Err(problems)
        }
    }
}

/// A published version's content, as the build composes it.
#[derive(Debug, Default)]
struct PublishedContent {
    /// Its `src/` files.
    files: FileTree,
    /// Its `amendments/` copies; empty unless its manifest declares
    /// amendments.
    amendments: FileTree,
    /// Its lockfile, read only when its manifest declares amendments.
    lock: Option<UnitLock>,
}

/// A successful build.
#[derive(Debug)]
pub struct Built {
    unit: UnitIdentity,
    version: Version,
    files: usize,
    dependencies: usize,
    lockfile: LockfileOutcome,
    changes: BTreeMap<UnitIdentity, LockChange>,
    notes: Vec<String>,
    leftover: Option<(PathBuf, io::Error)>,
}

impl Built {
    /// The unit built.
    pub fn unit(&self) -> &UnitIdentity {
        &self.unit
    }

    /// Its version, as its working tree's manifest declares.
    pub fn version(&self) -> &Version {
        &self.version
    }

    /// How many files `build/` holds.
    pub fn files(&self) -> usize {
        self.files
    }

    /// How many dependencies, direct and transitive, were composed.
    pub fn dependencies(&self) -> usize {
        self.dependencies
    }

    /// What happened to the lockfile.
    pub fn lockfile(&self) -> LockfileOutcome {
        self.lockfile
    }

    /// How the resolution differs from the previous lockfile, by identity.
    pub fn changes(&self) -> &BTreeMap<UnitIdentity, LockChange> {
        &self.changes
    }

    /// What the composition reports without failing: parts of amendment
    /// copies the merge could not carry, each naming the file.
    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// The previous build, if it was replaced but could not be deleted, and
    /// why. The next build deletes it.
    pub fn leftover(&self) -> Option<(&Path, &io::Error)> {
        self.leftover
            .as_ref()
            .map(|(path, error)| (path.as_path(), error))
    }
}

/// What a build did to the lockfile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockfileOutcome {
    /// There was none, and it was created.
    Created,
    /// It was rewritten, because the resolution changed.
    Updated,
    /// It already recorded the resolution, and was not touched.
    Unchanged,
}

/// Why a build failed.
#[derive(Debug)]
pub enum BuildFailure {
    /// The build was rejected before anything was written. Every problem
    /// found is listed. `build/`, the lockfile, and the unit's files are
    /// unchanged.
    Rejected(Vec<BuildProblem>),
    /// Writing the build failed. The error says what remains.
    Publish(PublishError),
    /// The build met a conflict between sibling amenders in files the unit
    /// does not amend, and created the marked copies for it to resolve:
    /// the pending state. The manifest declares them and the lockfile is
    /// written; `build/` is untouched.
    Pending(Pending),
    /// Writing the pending state failed. `written` lists the files that
    /// were written before it did, by path under `amendments/`, with
    /// `manifest.jsonc` last when it was replaced.
    PendingWrite {
        /// The file that could not be written.
        path: PathBuf,
        /// Why.
        error: io::Error,
        /// What was written.
        written: Vec<RelativePath>,
    },
}

/// The pending state a build created.
#[derive(Debug)]
pub struct Pending {
    files: Vec<PendingFile>,
    lockfile: LockfileOutcome,
}

impl Pending {
    /// The copies created, in path order.
    pub fn files(&self) -> &[PendingFile] {
        &self.files
    }

    /// What happened to the lockfile.
    pub fn lockfile(&self) -> LockfileOutcome {
        self.lockfile
    }
}

impl fmt::Display for BuildFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(problems) => {
                for (index, problem) in problems.iter().enumerate() {
                    if index > 0 {
                        f.write_str("\n")?;
                    }
                    problem.fmt(f)?;
                }
                Ok(())
            }
            Self::Publish(error) => error.fmt(f),
            Self::Pending(pending) => {
                for (index, file) in pending.files.iter().enumerate() {
                    if index > 0 {
                        f.write_str("\n")?;
                    }
                    let amenders: Vec<String> =
                        file.amenders.iter().map(ToString::to_string).collect();
                    write!(
                        f,
                        "amendments/{}: the amendments of {} of src/{} of {} {} conflict, and \
                         the copy holds each conflict between markers:",
                        file.source,
                        amenders.join(" and "),
                        file.source,
                        file.provider,
                        file.base
                    )?;
                    for conflict in &file.conflicts {
                        write!(f, "\n  {conflict}")?;
                    }
                }
                Ok(())
            }
            Self::PendingWrite { path, error, .. } => {
                write!(f, "cannot write {}: {error}", path.display())
            }
        }
    }
}

impl std::error::Error for BuildFailure {}

/// A problem that rejects a build.
#[derive(Debug)]
pub enum BuildProblem {
    /// The working tree's `manifest.jsonc` cannot be used.
    Manifest(ManifestFileError),
    /// The unit's repository or its working tree could not be read.
    Repository(GitError),
    /// The lock that keeps builds of the unit from running at the same time
    /// could not be taken.
    BuildLock(BuildLockError),
    /// The repository's Git directory could not be examined for the record
    /// of an interrupted update.
    GitDirectory(io::Error),
    /// An update of the unit was interrupted, and only `unit update`
    /// completes it.
    UpdateInterrupted {
        /// The record of the update.
        journal: PathBuf,
    },
    /// The working tree's `src/` holds something a unit file cannot be.
    Src(ReadSrcError),
    /// The working tree's `amendments/` holds something a unit file cannot
    /// be.
    Amendments(ReadSrcError),
    /// A path the build would replace is not what Assemblr writes there.
    Output(OutputProblem),
    /// The lockfile cannot be read.
    Lockfile(LockfileError),
    /// The unit being built has required parameters with neither a default
    /// nor an example, so it has no value to be built with.
    DevelopmentValues {
        /// The unit.
        unit: UnitIdentity,
        /// The parameters, sorted.
        parameters: Vec<ParameterName>,
    },
    /// The registry index cannot be read.
    Registry(StorageError),
    /// A dependency's registered repository could not be read.
    Source {
        /// The dependency.
        unit: UnitIdentity,
        /// Why.
        error: GitError,
    },
    /// A dependency's published version, at the commit the index records,
    /// is not the valid version the index recorded.
    Published {
        /// The dependency.
        unit: UnitIdentity,
        /// The version.
        version: Version,
        /// What is wrong.
        problem: PublishedProblem,
    },
    /// The dependencies cannot be resolved.
    Resolution(ResolutionError),
    /// The resolution contradicts the lockfile.
    Lock(LockError),
    /// The resolution would move an amended unit away from its base.
    AmendmentBase(AmendmentBaseError),
    /// A published amender's lockfile identifies a base as other content
    /// than the registry records for that version.
    BaseIntegrity(Box<BaseIntegrityError>),
    /// The unit and its dependencies cannot be composed.
    Composition(CompositionError),
    /// Several amendments of one provider file cannot be combined.
    Merge {
        /// The provider.
        unit: UnitIdentity,
        /// Its resolved version, when it is locked.
        version: Option<Version>,
        /// The amended path in its `src/`.
        path: RelativePath,
        /// The amenders, by identity.
        amenders: Vec<UnitIdentity>,
        /// The amenders whose copy was taken from another version of the
        /// provider than the one resolved, with that version.
        crossed: Vec<(UnitIdentity, Version)>,
        /// Why, or `None` when a rendered side is not UTF-8 text.
        failure: Option<super::merge::MergeFailure>,
    },
    /// Copies under `amendments/` that still hold conflict markers.
    Markers {
        /// The copies' paths.
        paths: Vec<RelativePath>,
    },
    /// A conflict between sibling amenders of a file that no unit above
    /// both resolves.
    Unresolved(UnresolvedFile),
    /// A staging file the pending state writes through already exists.
    StagingOccupied {
        /// The staging file.
        path: PathBuf,
    },
    /// The manifest cannot be edited safely to declare a pending copy.
    ManifestEdit(super::manifest::EditError),
    /// A directory on a pending copy's path under `amendments/` is not a
    /// real directory.
    NotADirectory {
        /// The path.
        path: PathBuf,
    },
    /// A directory on a pending copy's path cannot be read.
    Unreadable {
        /// The path.
        path: PathBuf,
        /// Why.
        error: io::Error,
    },
    /// An amendment taken from another version of its provider than the one
    /// resolved cannot be applied to it.
    CrossVersion {
        /// The provider.
        unit: UnitIdentity,
        /// The amended path in its `src/`.
        path: RelativePath,
        /// The amender.
        amender: UnitIdentity,
        /// The amender's published version, when it is not the unit being
        /// built.
        published: Option<Version>,
        /// The version the copy was taken from.
        base: Version,
        /// The version the provider resolved to.
        resolved: Version,
        /// Why it cannot be applied.
        reason: CrossVersionReason,
    },
    /// A template cannot be rendered.
    Template {
        /// The unit whose `src/` provides the template.
        unit: UnitIdentity,
        /// The template's path in that `src/`.
        path: RelativePath,
        /// The unit whose copy at `amendments/<path>` is rendered instead
        /// of the provider's file, if the template is amended.
        amended_by: Option<UnitIdentity>,
        /// That unit's published version, when it is not the unit being
        /// built.
        published: Option<Version>,
        /// What is wrong.
        error: TemplateError,
    },
}

impl fmt::Display for BuildProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest(error) => error.fmt(f),
            Self::Repository(error) => error.fmt(f),
            Self::BuildLock(error) => error.fmt(f),
            Self::GitDirectory(error) => write!(
                f,
                "could not examine the repository's Git directory for an interrupted update: \
                 {error}"
            ),
            Self::UpdateInterrupted { journal } => write!(
                f,
                "an update of this unit was interrupted and not completed, as {} records; run \
                 `assemblr unit update` to complete it first",
                journal.display()
            ),
            Self::Src(error) => error.fmt(f),
            Self::Amendments(error) => error.fmt(f),
            Self::Output(problem) => problem.fmt(f),
            Self::Lockfile(error) => error.fmt(f),
            Self::DevelopmentValues { unit, parameters } => {
                let names: Vec<String> =
                    parameters.iter().map(|name| format!("`{name}`")).collect();
                write!(
                    f,
                    "{unit} declares required parameters without defaults or examples: {}; \
                     building it needs a value for each, and an `example` on the parameter \
                     supplies one",
                    names.join(", ")
                )
            }
            Self::Registry(error) => error.fmt(f),
            Self::Source { unit, error } => {
                write!(f, "could not read the registered source of {unit}: {error}")
            }
            Self::Published {
                unit,
                version,
                problem,
            } => write!(
                f,
                "{unit} {version}, as the registry index records it: {problem}"
            ),
            Self::Resolution(error) => error.fmt(f),
            Self::Lock(error) => error.fmt(f),
            Self::AmendmentBase(error) => error.fmt(f),
            Self::BaseIntegrity(error) => error.fmt(f),
            Self::Composition(error) => error.fmt(f),
            Self::Merge {
                unit,
                version,
                path,
                amenders,
                crossed,
                failure,
            } => {
                let amenders: Vec<String> = amenders.iter().map(ToString::to_string).collect();
                let version = version
                    .as_ref()
                    .map_or_else(String::new, |version| format!(" {version}"));
                write!(
                    f,
                    "src/{path} of {unit}{version} is amended by {}, and the amendments cannot \
                     be combined: ",
                    amenders.join(", ")
                )?;
                match failure {
                    Some(failure) => failure.fmt(f),
                    None => f.write_str("a rendered side is not UTF-8 text"),
                }?;
                for (amender, base) in crossed {
                    write!(
                        f,
                        "\n  the amendment of {amender} was taken from {unit} {base}, and is \
                         applied to the version resolved"
                    )?;
                }
                Ok(())
            }
            Self::Markers { paths } => {
                let paths: Vec<String> = paths
                    .iter()
                    .map(|path| format!("amendments/{path}"))
                    .collect();
                write!(
                    f,
                    "{} still {} conflict markers; the build composes nothing until they are \
                     resolved",
                    paths.join(", "),
                    if paths.len() == 1 { "holds" } else { "hold" }
                )
            }
            Self::Unresolved(unresolved) => unresolved.fmt(f),
            Self::StagingOccupied { path } => write!(
                f,
                "{} exists; it is reserved for the build's staging, so nothing was written",
                path.display()
            ),
            Self::ManifestEdit(error) => write!(
                f,
                "the manifest cannot be edited safely to declare the copy: {error}"
            ),
            Self::NotADirectory { path } => write!(
                f,
                "{} is not a directory, and the copy would be written beneath it",
                path.display()
            ),
            Self::Unreadable { path, error } => {
                write!(f, "cannot read {}: {error}", path.display())
            }
            Self::CrossVersion {
                unit,
                path,
                amender,
                published,
                base,
                resolved,
                reason,
            } => {
                let amender = match published {
                    Some(version) => format!("{amender} {version}"),
                    None => amender.to_string(),
                };
                write!(
                    f,
                    "the amendment of src/{path} by {amender} was taken from {unit} {base}, and \
                     cannot be applied to {unit} {resolved}: "
                )?;
                match reason {
                    CrossVersionReason::BaseUnavailable => write!(
                        f,
                        "the file as it was in {unit} {base} cannot be read from the registry"
                    ),
                    CrossVersionReason::NoMerge => f.write_str(
                        "the file differs between the two versions, and its format has no merge \
                         (it is applied across versions only to a file that is unchanged), or a \
                         copy or version of it is not UTF-8 text",
                    ),
                    CrossVersionReason::Parameter {
                        name,
                        base: then,
                        resolved: now,
                    } => {
                        let kind = |kind: &Option<ValueKind>| {
                            kind.as_ref().map_or_else(
                                || "not declared".to_owned(),
                                |kind| format!("declared as {kind}"),
                            )
                        };
                        write!(
                            f,
                            "it names parameter `{name}`, which is {} in {unit} {base} and {} in \
                             {unit} {resolved}, so the amendment's contribution cannot be \
                             told from the provider's own change",
                            kind(then),
                            kind(now)
                        )
                    }
                }
            }
            Self::Template {
                unit,
                path,
                amended_by,
                published,
                error,
            } => {
                match (amended_by, published) {
                    (Some(amender), Some(version)) => write!(
                        f,
                        "amendments/{path} of {amender} {version} (a copy of template src/{path} \
                         of {unit})"
                    )?,
                    (Some(amender), None) => write!(
                        f,
                        "amendments/{path} of {amender} (a copy of template src/{path} of {unit})"
                    )?,
                    (None, _) => write!(f, "template src/{path} of {unit}")?,
                }
                match error {
                    TemplateError::NotUtf8 => write!(
                        f,
                        " is not UTF-8 text; a template is rendered as text, and a file that is \
                         not text must not end in `{TEMPLATE_SUFFIX}`"
                    ),
                    TemplateError::Render(error) => write!(f, ", {error}"),
                }
            }
        }
    }
}

impl std::error::Error for BuildProblem {}

/// Why an amendment cannot be applied to another version of its provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrossVersionReason {
    /// The provider's file as it was in the base version cannot be read.
    BaseUnavailable,
    /// The file differs and has no merge, or is not text.
    NoMerge,
    /// A parameter a template names is not declared at the resolved
    /// version, or is declared as another kind of value.
    Parameter {
        /// The parameter.
        name: ParameterName,
        /// Its kind at the base version, when declared.
        base: Option<ValueKind>,
        /// Its kind at the resolved version, when declared.
        resolved: Option<ValueKind>,
    },
}

/// Why a template cannot be rendered.
#[derive(Debug)]
pub enum TemplateError {
    /// Its source is not UTF-8 text.
    NotUtf8,
    /// A construct in it cannot be rendered.
    Render(RenderError),
}

/// Why a published version cannot be used as the index records it.
#[derive(Debug)]
pub enum PublishedProblem {
    /// The recorded commit has no valid manifest.
    Invalid(InvalidVersion),
    /// The recorded commit's manifest or tree is not the one indexed.
    NotIndexed,
    /// Its `src/` cannot be read as unit files.
    Src(ReadSrcError),
    /// Its `amendments/` cannot be read as unit files.
    Amendments(ReadSrcError),
    /// It declares amendments, but its lockfile cannot be read.
    Lockfile(PublishedLockfileError),
    /// Its lockfile does not identify the base of each amendment.
    Bases(Vec<PublishedAmendmentError>),
}

impl fmt::Display for PublishedProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(invalid) => invalid.fmt(f),
            Self::NotIndexed => f.write_str(
                "the recorded commit does not hold the identity, version, or content that was \
                 indexed",
            ),
            Self::Src(error) => write!(f, "invalid `src/`: {error}"),
            Self::Amendments(error) => write!(f, "invalid `amendments/`: {error}"),
            Self::Lockfile(error) => write!(f, "it declares amendments, but {error}"),
            Self::Bases(problems) => {
                f.write_str("its lockfile does not identify the base of each amendment: ")?;
                for (index, problem) in problems.iter().enumerate() {
                    if index > 0 {
                        f.write_str("; ")?;
                    }
                    problem.fmt(f)?;
                }
                Ok(())
            }
        }
    }
}

/// Why the working tree's `manifest.jsonc` cannot be used.
#[derive(Debug)]
pub enum ManifestFileError {
    /// There is none: the directory is not a unit repository's top level.
    Missing {
        /// Where it was looked for.
        path: PathBuf,
    },
    /// It is a directory, a symbolic link, or another special file.
    NotAFile {
        /// The manifest.
        path: PathBuf,
    },
    /// It could not be read.
    Io {
        /// The manifest.
        path: PathBuf,
        /// Why.
        error: io::Error,
    },
    /// It is larger than Assemblr reads.
    TooLarge {
        /// The manifest.
        path: PathBuf,
        /// The largest size read, in bytes.
        limit: u64,
    },
    /// It is not UTF-8 text.
    NotUtf8 {
        /// The manifest.
        path: PathBuf,
    },
    /// It is not a valid manifest.
    Invalid {
        /// The manifest.
        path: PathBuf,
        /// Why.
        error: ReadManifestError,
    },
}

impl fmt::Display for ManifestFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { path } => write!(f, "{} does not exist", path.display()),
            Self::NotAFile { path } => write!(f, "{} is not a regular file", path.display()),
            Self::Io { path, error } => write!(f, "could not read {}: {error}", path.display()),
            Self::TooLarge { path, limit } => write!(
                f,
                "{} is larger than the {limit} bytes Assemblr reads",
                path.display()
            ),
            Self::NotUtf8 { path } => write!(f, "{} is not UTF-8 text", path.display()),
            Self::Invalid { path, error } => write!(f, "{}: {error}", path.display()),
        }
    }
}

impl std::error::Error for ManifestFileError {}
