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
mod bases;
mod catalog;
mod check;
mod exclusive;
mod layering;
pub use layering::UnresolvedFile;
pub(crate) use layering::{layer_file, layer_files};
mod output;
mod publish;
mod update;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub use add::{AddFailure, AddProblem, DependencyAdded, add_dependency};
pub use amend::{AmendFailure, AmendProblem, AmendState, Amended, Match, WriteError, amend_unit};
pub use bases::{BaseStoreError, RebaseWriteError, RebasedCopy};
pub use catalog::{Location, build_catalog, check_catalog, locate_unit};
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
use crate::domain::{
    AmendmentBaseError, AvailableVersions, BaseIntegrityError, CompositionError, ContributionError,
    FileTree, LockChange, LockError, Manifest, ObjectId, PlaceholderError, PlaceholderReport,
    PublishedAmendmentError, RelativePath, Requirer, ResolutionError, Root, UnitContent, UnitFile,
    UnitIdentity, UnitLock, UnresolvedTokens, Version, amendment_bases, check_placeholders,
    compose, contribution_problems, published_amendments, relock, resolve, verify_bases,
};

/// The unit's manifest, at the repository's top level.
const MANIFEST: &str = "manifest.jsonc";

/// Builds the unit whose repository's top level is `root`, resolving its
/// dependencies from `registry`.
///
/// In the directory of a unit in a catalog, the unit is built with its
/// siblings from the catalog's working tree, under the catalog's one
/// lockfile (see [`build_catalog`]).
pub fn build_unit(root: &Path, registry: &Registry) -> Result<Built, BuildFailure> {
    if let Location::CatalogUnit { catalog, directory } = locate_unit(root) {
        return catalog::build_in_catalog(&catalog, &directory, registry);
    }
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
        output::check(root, Some(root), interrupted)
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
        catalog: None,
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
        return Err(write_pending(
            root,
            root,
            opened.previous.as_ref(),
            text,
            manifest,
            validated,
            None,
        ));
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
    let published = output::publish(root, root, &files, new_lock, &opened.guard)
        .map_err(BuildFailure::Publish)?;
    Ok(Built {
        unit: manifest.name().clone(),
        version: manifest.version().clone(),
        directory: None,
        files: files.len(),
        dependencies: validated.dependencies,
        lockfile,
        changes: validated.changes,
        notes: validated.notes,
        placeholders: validated.placeholders,
        unresolved: validated.unresolved,
        leftover: published.leftover,
        rebased: Vec::new(),
    })
}

/// Writes the pending state: each marked copy under `amendments/`, its
/// declaration in the manifest, and the lockfile, since the resolution is
/// sound and the conflict is a matter of content. `build/` is not touched.
/// The copies are created first, new files only, then the manifest is
/// replaced through its staging file, then the lockfile, in `lock_root`,
/// through its own. `previous` is the lockfile's content before. In a
/// catalog, `in_catalog` says where the manifest is read.
fn write_pending(
    root: &Path,
    lock_root: &Path,
    previous: Option<&UnitLock>,
    text: &str,
    manifest: &Manifest,
    validated: Validated,
    in_catalog: Option<&crate::adapters::manifest::InCatalog<'_>>,
) -> BuildFailure {
    let staged = root.join(amend::MANIFEST_NEW);
    let lock_staging = lock_root.join(output::LOCK_STAGING);
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
        match crate::adapters::manifest::declare_amendment_in(
            &edited,
            &declared,
            &pending.path,
            &declaration,
            in_catalog,
        ) {
            Ok(text) => edited = text,
            Err(error) => {
                problems.push(BuildProblem::ManifestEdit(error));
                continue;
            }
        }
        if let Some(next) = declared.with_amendment(pending.path.clone(), declaration) {
            declared = next;
        }
        placements.push(pending);
    }
    // The directories the copies need, each once: two copies can share a
    // new one.
    let mut missing = Vec::new();
    match amend::real_directories_of(root, placements.iter().map(|pending| &pending.path)) {
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
                path: lock_root.join(lockfile::LOCKFILE),
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
            .join(pending.path.as_str());
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
        written.push(pending.path.clone());
    }
    if let Err(error) = amend::write_manifest(root, &staged, edited.as_bytes()) {
        let _ = fs::remove_file(&staged);
        return BuildFailure::PendingWrite {
            path: root.join(MANIFEST),
            error,
            written,
        };
    }
    written
        .push(RelativePath::parse(MANIFEST).unwrap_or_else(|_| validated.pending[0].path.clone()));
    let lockfile_outcome = match previous {
        None => LockfileOutcome::Created,
        Some(previous) if *previous == validated.lock => LockfileOutcome::Unchanged,
        Some(_) => LockfileOutcome::Updated,
    };
    if lockfile_outcome != LockfileOutcome::Unchanged {
        let replaced = fs::write(&lock_staging, &lock_bytes)
            .and_then(|()| fs::rename(&lock_staging, lock_root.join(lockfile::LOCKFILE)));
        if let Err(error) = replaced {
            let _ = fs::remove_file(&lock_staging);
            return BuildFailure::PendingWrite {
                path: lock_root.join(lockfile::LOCKFILE),
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
    read_working_text(root.join(MANIFEST))
}

/// Reads the text of a manifest, or of a catalog's `catalog.jsonc`, at
/// `path` in the working tree.
fn read_working_text(path: PathBuf) -> Result<String, ManifestFileError> {
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
    /// The previous lock, empty when there is no lockfile. For a unit in a
    /// catalog, the lock the whole catalog resolves to.
    pub(super) previous: &'a UnitLock,
    /// The unit's catalog, when it is in one.
    pub(super) catalog: Option<&'a Joint<'a>>,
}

/// What a unit in a catalog is validated with besides its own inputs: its
/// siblings, and the lock the whole catalog resolves to, which is the one
/// lockfile of the catalog, with how it differs from that lockfile.
pub(super) struct Joint<'a> {
    pub(super) siblings: &'a Siblings,
    pub(super) lock: &'a UnitLock,
    pub(super) changes: &'a BTreeMap<UnitIdentity, LockChange>,
}

/// What validating a unit found.
pub(super) struct Validation {
    /// The problems with the unit's contribution, which gate publication
    /// and never a build (OC-11). Empty when resolution did not finish.
    pub(super) contribution: Vec<ContributionError>,
    /// The tokens in the unit's own files that look like placeholders and
    /// that neither it nor its graph declares or ignores: a warning for a
    /// build, a problem for a publication. Empty when resolution did not
    /// finish.
    pub(super) unresolved: UnresolvedTokens,
    /// The validated unit, when every validation passed.
    pub(super) result: Option<Validated>,
}

/// A unit that passed every validation a build runs.
pub(super) struct Validated {
    /// The lock the resolution produces: the lockfile's new content. For a
    /// unit in a catalog, the whole catalog's.
    pub(super) lock: UnitLock,
    /// How many units, direct and transitive, the unit was composed with.
    pub(super) dependencies: usize,
    /// How that lock differs from the previous one.
    pub(super) changes: BTreeMap<UnitIdentity, LockChange>,
    /// The composed files, by path.
    pub(super) files: Vec<(RelativePath, UnitFile)>,
    /// What the composition reports without failing: parts of amendment
    /// copies the merge could not carry, each naming the file.
    pub(super) notes: Vec<String>,
    /// Files the unit does not amend whose sibling amenders conflict:
    /// the build creates the marked copies instead of `build/` (the
    /// pending state, see `docs/domain/authoring-workflow.md`).
    pub(super) pending: Vec<PendingFile>,
    /// The declared placeholders the composed files hold.
    pub(super) placeholders: PlaceholderReport,
    /// As [`Validation::unresolved`].
    pub(super) unresolved: UnresolvedTokens,
}

/// A copy the pending state creates: the composition of a file with the
/// conflict between its sibling amenders marked, for the unit to resolve.
#[derive(Debug, Clone)]
pub struct PendingFile {
    /// The path in the provider's `src/`, and the copy's under
    /// `amendments/`.
    path: RelativePath,
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
        &self.path
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
        if manifest.amendments().contains_key(&unresolved.path) {
            kept.push(unresolved);
            continue;
        }
        let copy = layer_file(
            composition,
            &unresolved.path,
            lock,
            Some(manifest.name()),
            dependencies,
            problems,
        );
        let Some(copy) = copy else {
            continue;
        };
        pending.push(PendingFile {
            path: unresolved.path.clone(),
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

/// The resolved, verified dependencies of a root: a unit, or a consumer
/// project when `name` is `None`.
pub(crate) struct Resolved {
    pub(crate) resolution: crate::domain::Resolution,
    pub(crate) lock: UnitLock,
    pub(crate) dependencies: Dependencies,
    contents: BTreeMap<UnitIdentity, PublishedContent>,
}

impl Resolved {
    /// The manifest of `identity` at its locked version.
    pub(crate) fn manifest_of(&self, identity: &UnitIdentity) -> Option<&Manifest> {
        let locked = self.lock.units().get(identity)?;
        self.dependencies.manifest(identity, locked.version())
    }

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
    resolve_graph_in(
        name,
        requirements,
        previous,
        index,
        &Siblings::default(),
        problems,
    )
}

/// [`resolve_graph`], with the units of `siblings` resolved from it at its
/// version and locked as it records them.
fn resolve_graph_in(
    name: Option<&UnitIdentity>,
    requirements: &crate::domain::Dependencies,
    previous: &UnitLock,
    index: &crate::domain::RegistryIndex,
    siblings: &Siblings,
    problems: &mut Vec<BuildProblem>,
) -> Option<Resolved> {
    let index = &siblings.index(index);
    let dependencies = match Dependencies::read_in(index, name, requirements, siblings) {
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

/// The versions a resolution of `requirements` against `index` selects
/// with no lock to prefer: the newest each requirement allows, as an
/// update of every unit would lock them. Only the manifests are read, not
/// the units' content.
pub(crate) fn newest_allowed(
    requirements: &crate::domain::Dependencies,
    index: &crate::domain::RegistryIndex,
) -> Result<BTreeMap<UnitIdentity, Version>, Vec<BuildProblem>> {
    let dependencies = Dependencies::read_for(index, None, requirements)?;
    let resolution = resolve(
        None,
        requirements,
        &dependencies.available,
        &BTreeMap::new(),
    )
    .map_err(|errors| {
        errors
            .into_iter()
            .map(BuildProblem::Resolution)
            .collect::<Vec<_>>()
    })?;
    Ok(resolution
        .graph()
        .units()
        .iter()
        .map(|(identity, unit)| (identity.clone(), unit.version().clone()))
        .collect())
}

/// Runs every validation that gates a build on `inputs` (see
/// `docs/domain/authoring-workflow.md`): resolution under the lock against
/// `index`, the lock's integrity, the amendment bases, the published
/// content of every locked version, the bases of published amenders, the
/// unit's placeholders, composition, and layering. Every problem found is
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
        catalog,
    } = *inputs;
    let alone = Siblings::default();
    let mut validation = Validation {
        contribution: Vec::new(),
        unresolved: UnresolvedTokens::new(),
        result: None,
    };
    let Some(resolved) = resolve_graph_in(
        Some(manifest.name()),
        manifest.dependencies(),
        previous,
        index,
        catalog.map_or(&alone, |catalog| catalog.siblings),
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
            problems.push(match error {
                // In a catalog, `previous` is the catalog's new joint lock,
                // not the lockfile, and `unit update` does not run there:
                // the declared base moves by hand, and the build rebases
                // the copy from its stored base.
                AmendmentBaseError::DeclarationChanged {
                    path,
                    unit,
                    base,
                    resolved,
                    ..
                } if catalog.is_some() => BuildProblem::CatalogBase {
                    path,
                    unit,
                    base,
                    resolved,
                },
                error => BuildProblem::AmendmentBase(error),
            });
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
    let graph = resolved
        .lock
        .units()
        .keys()
        .filter_map(|identity| resolved.manifest_of(identity));
    let placeholders = check_placeholders(manifest, src, amendments, graph);
    problems.extend(
        placeholders
            .problems
            .into_iter()
            .map(BuildProblem::Placeholder),
    );
    validation.unresolved = placeholders.unresolved;
    let units = resolved.units();
    let root_content = UnitContent::new(manifest, src, amendments);
    let composition = compose(
        resolved.resolution.graph(),
        Root::Unit(root_content),
        &units,
    );
    let composition = match composition {
        Ok(composition) => composition,
        Err(errors) => {
            problems.extend(
                errors
                    .into_iter()
                    // Already reported, with the cause, from the lock.
                    .filter(|error| match error {
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
    let Some(layered) = layer_files(
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
        layered.unresolved,
        &resolved.lock,
        manifest,
        &resolved.dependencies,
        problems,
    );
    problems.extend(kept.into_iter().map(BuildProblem::Unresolved));
    if !problems.is_empty() {
        return validation;
    }
    let placeholders = PlaceholderReport::find(layered.files.iter().filter_map(|(path, file)| {
        composition
            .files()
            .get(path)
            .map(|composed| (path, file.contents(), composed.placeholders()))
    }));
    let (lock, changes) = match catalog {
        Some(catalog) => (catalog.lock.clone(), catalog.changes.clone()),
        None => (resolved.lock.clone(), resolved.resolution.changes().clone()),
    };
    validation.result = Some(Validated {
        dependencies: resolved.lock.units().len(),
        lock,
        changes,
        files: layered.files,
        notes: layered.notes.into_iter().map(|(_, note)| note).collect(),
        pending,
        placeholders,
        unresolved: validation.unresolved.clone(),
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
    /// The siblings of a unit in a catalog, by identity: their one version
    /// and its content, which is not read from the registry.
    siblings: BTreeMap<UnitIdentity, (Version, PublishedContent)>,
}

impl Dependencies {
    /// The manifest of `identity` at `version`, when that version was read.
    pub(crate) fn manifest(&self, identity: &UnitIdentity, version: &Version) -> Option<&Manifest> {
        self.manifests.get(&(identity.clone(), version.clone()))
    }

    /// Whether `identity` is a sibling of the unit in its catalog, whose
    /// content is the catalog's own, not a published version's.
    pub(super) fn is_sibling(&self, identity: &UnitIdentity) -> bool {
        self.siblings.contains_key(identity)
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
        Self::read_in(index, name, requirements, &Siblings::default())
    }

    /// [`read_for`](Self::read_for), with the units of `siblings` taken
    /// from it, at its version, instead of from the registry.
    fn read_in(
        index: &crate::domain::RegistryIndex,
        name: Option<&UnitIdentity>,
        requirements: &crate::domain::Dependencies,
        siblings: &Siblings,
    ) -> Result<Self, Vec<BuildProblem>> {
        let mut dependencies = Self {
            available: AvailableVersions::new(),
            manifests: BTreeMap::new(),
            repositories: BTreeMap::new(),
            commits: BTreeMap::new(),
            siblings: BTreeMap::new(),
        };
        let mut problems = Vec::new();
        let mut pending: BTreeSet<UnitIdentity> = requirements.keys().cloned().collect();
        let mut seen = BTreeSet::new();
        while let Some(identity) = pending.pop_first() {
            if !seen.insert(identity.clone()) || Some(&identity) == name {
                continue;
            }
            if let Some(sibling) = siblings.units.get(&identity) {
                let version = sibling.manifest.version().clone();
                let requirements = sibling.manifest.dependencies();
                pending.extend(requirements.keys().cloned());
                dependencies.available.insert(
                    identity.clone(),
                    BTreeMap::from([(version.clone(), requirements.clone())]),
                );
                dependencies.manifests.insert(
                    (identity.clone(), version.clone()),
                    sibling.manifest.clone(),
                );
                dependencies
                    .siblings
                    .insert(identity, (version, sibling.content.clone()));
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
                Ok(repository) => repository.for_unit(&identity),
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
        if let Some(content) = self.sibling(identity, version) {
            return Some(Ok(content.files.clone()));
        }
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
        if let Some(content) = self.sibling(identity, version) {
            return Some(Ok(content.amendments.clone()));
        }
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
        if let Some(content) = self.sibling(identity, version) {
            return Some(Ok(content.lock.clone()));
        }
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

    /// The content of `identity` at `version`, when it is a sibling.
    fn sibling(&self, identity: &UnitIdentity, version: &Version) -> Option<&PublishedContent> {
        self.siblings
            .get(identity)
            .filter(|(sibling, _)| sibling == version)
            .map(|(_, content)| content)
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
            if let Some(content) = self.sibling(identity, locked.version()) {
                contents.insert(identity.clone(), content.clone());
                continue;
            }
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
#[derive(Debug, Clone, Default)]
pub(super) struct PublishedContent {
    /// Its `src/` files.
    pub(super) files: FileTree,
    /// Its `amendments/` copies; empty unless its manifest declares
    /// amendments.
    pub(super) amendments: FileTree,
    /// Its lockfile, read only when its manifest declares amendments.
    pub(super) lock: Option<UnitLock>,
}

/// The units of a catalog, as a build of one of them resolves its
/// siblings: each at the catalog's version, with the content of the
/// catalog's working tree or of the commit being published, never read from
/// the registry (see `docs/domain/units.md`). Empty outside a catalog.
#[derive(Default)]
pub(super) struct Siblings {
    pub(super) units: BTreeMap<UnitIdentity, Sibling>,
}

/// A unit of a catalog, as its siblings compose it.
pub(super) struct Sibling {
    pub(super) manifest: Manifest,
    /// Its files, and, when it declares amendments, the catalog's lock,
    /// which identifies their bases.
    pub(super) content: PublishedContent,
    /// What the lock records for it, while it resolves with its siblings.
    pub(super) provenance: crate::domain::Provenance,
}

impl Siblings {
    /// `index`, with every sibling registered at its one version in place
    /// of any registration it has: a catalog's units resolve and lock
    /// together, from the catalog itself.
    fn index(&self, index: &crate::domain::RegistryIndex) -> crate::domain::RegistryIndex {
        if self.units.is_empty() {
            return index.clone();
        }
        let mut units = index.units().clone();
        for (identity, sibling) in &self.units {
            units.insert(
                identity.clone(),
                crate::domain::RegisteredUnit::new(
                    crate::domain::SourceReference::new("the catalog being built"),
                    BTreeMap::from([(
                        sibling.manifest.version().clone(),
                        sibling.provenance.clone(),
                    )]),
                ),
            );
        }
        // One version for each sibling breaks no invariant. Were one
        // broken, the siblings would be reported as not indexed.
        crate::domain::RegistryIndex::from_units(units).unwrap_or_else(|_| index.clone())
    }

    /// Whether `identity` is a sibling.
    pub(super) fn contains(&self, identity: &UnitIdentity) -> bool {
        self.units.contains_key(identity)
    }
}

/// A successful build.
#[derive(Debug)]
pub struct Built {
    unit: UnitIdentity,
    version: Version,
    directory: Option<String>,
    files: usize,
    dependencies: usize,
    lockfile: LockfileOutcome,
    changes: BTreeMap<UnitIdentity, LockChange>,
    notes: Vec<String>,
    placeholders: PlaceholderReport,
    unresolved: UnresolvedTokens,
    leftover: Option<(PathBuf, io::Error)>,
    /// In a catalog, the copies rebased onto their bases now first.
    rebased: Vec<RebasedCopy>,
}

impl Built {
    /// In a catalog, the copies under `amendments/` that the build rebased
    /// first, because the file below them changed since their stored base.
    pub fn rebased(&self) -> &[RebasedCopy] {
        &self.rebased
    }

    /// The unit built.
    pub fn unit(&self) -> &UnitIdentity {
        &self.unit
    }

    /// Its version, as its working tree's manifest declares.
    pub fn version(&self) -> &Version {
        &self.version
    }

    /// Its directory, under `units/`, when it is in a catalog: its build is
    /// in `units/<directory>/build/`.
    pub fn directory(&self) -> Option<&str> {
        self.directory.as_deref()
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

    /// The declared placeholders `build/` holds, for the consumer to
    /// replace.
    pub fn placeholders(&self) -> &PlaceholderReport {
        &self.placeholders
    }

    /// The tokens in the unit's own files that look like placeholders and
    /// are not declared or ignored, which `unit publish` refuses.
    pub fn unresolved(&self) -> &UnresolvedTokens {
        &self.unresolved
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
    /// Building every unit of a catalog wrote the builds of `built`, then
    /// writing the next one failed, and the rest were not written. The
    /// error says what remains of that one.
    PartlyBuilt {
        /// The units built, in order; the first wrote the lockfile.
        built: Vec<Built>,
        /// The directory, under `units/`, of the unit whose build failed.
        directory: String,
        /// Why.
        error: PublishError,
    },
    /// In a catalog, the file below some copies changed since their stored
    /// base, and rebasing them left conflicts: every rebased copy is
    /// written, those with conflicts between markers, with the new bases,
    /// and nothing is built.
    Rebased(Vec<RebasedCopy>),
    /// Writing the rebased copies of a catalog, or their bases, failed.
    RebaseWrite(RebaseWriteError),
    /// In a catalog, the build wrote the rebased copies `rebased` and their
    /// new bases, then ended in `failure` while writing the rest.
    AfterRebase {
        /// The copies rebased and written.
        rebased: Vec<RebasedCopy>,
        /// What happened next.
        failure: Box<BuildFailure>,
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
                        file.path,
                        amenders.join(" and "),
                        file.path,
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
            Self::PartlyBuilt {
                directory, error, ..
            } => write!(f, "units/{directory}: {error}"),
            Self::Rebased(copies) => {
                let mut first = true;
                for copy in copies.iter().filter(|copy| !copy.conflicts.is_empty()) {
                    if !first {
                        f.write_str("\n")?;
                    }
                    first = false;
                    write!(
                        f,
                        "units/{}/amendments/{}: src/{} of {}, as the units below compose \
                         it, changed since the copy's stored base, in what the copy changes \
                         too, and the copy holds each conflict between markers:",
                        copy.unit.name(),
                        copy.path,
                        copy.path,
                        copy.provider
                    )?;
                    for conflict in &copy.conflicts {
                        write!(f, "\n  {conflict}")?;
                    }
                }
                Ok(())
            }
            Self::RebaseWrite(error) => error.fmt(f),
            Self::AfterRebase { failure, .. } => failure.fmt(f),
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
    /// The unit's placeholder declarations do not suit its files or its
    /// resolved graph.
    Placeholder(PlaceholderError),
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
    /// In a catalog, an amendment declares a base other than the version
    /// the catalog's units now resolve its provider to.
    CatalogBase {
        /// The amended path.
        path: RelativePath,
        /// The provider.
        unit: UnitIdentity,
        /// The declared base.
        base: Version,
        /// The version the catalog's units resolve the provider to.
        resolved: Version,
    },
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
        /// Why, or `None` when a side is not UTF-8 text.
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
    /// A problem with one unit of a catalog, where several are built or
    /// checked together.
    InUnit {
        /// The unit's directory, under `units/`.
        directory: String,
        /// The problem.
        problem: Box<BuildProblem>,
    },
    /// The directory has both `catalog.jsonc` and `manifest.jsonc`: a
    /// repository is a catalog or one unit, not both.
    CatalogAndManifest {
        /// The manifest.
        path: PathBuf,
    },
    /// An entry of a catalog's `units/` is not a directory.
    NotAUnitDirectory {
        /// The entry.
        path: PathBuf,
    },
    /// A catalog's `units/` is a symbolic link or not a directory.
    UnitsNotADirectory {
        /// Its `units/`.
        path: PathBuf,
    },
    /// A catalog has no unit directory under `units/`.
    NoUnits {
        /// Its `units/`.
        path: PathBuf,
    },
    /// Building every unit of a catalog met a conflict between amenders of
    /// a file that one unit does not amend: the unit's own build creates
    /// the marked copies for it to resolve.
    CatalogPending {
        /// The unit's directory, under `units/`.
        directory: String,
        /// The copies its build would create, by path under `amendments/`.
        paths: Vec<RelativePath>,
    },
    /// The stored bases of a catalog's amendments cannot be used.
    BaseStore(BaseStoreError),
    /// A catalog stores no base for an amendment of one of its units.
    BaseMissing {
        /// The amended path.
        path: RelativePath,
        /// The unit that provides the file.
        provider: UnitIdentity,
    },
    /// The file below a copy in a catalog changed since its stored base:
    /// a build rebases the copy, a check or a publication refuses it.
    BaseStale {
        /// The amended path.
        path: RelativePath,
        /// The unit that provides the file.
        provider: UnitIdentity,
    },
    /// The file below a copy in a catalog changed since its stored base,
    /// and the copy cannot be merged with the change.
    BaseUnmergeable {
        /// The amended path.
        path: RelativePath,
        /// The unit that provides the file.
        provider: UnitIdentity,
        /// Why.
        reason: String,
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
            Self::Placeholder(error) => error.fmt(f),
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
            Self::CatalogBase {
                path,
                unit,
                base,
                resolved,
            } => write!(
                f,
                "amendments/{path} declares {unit} {base} as its base, but the catalog's units \
                 now resolve {unit} to {resolved}; declare {resolved} as the base in \
                 manifest.jsonc and run `assemblr unit build`, which rebases the copy from its \
                 stored base, or restore a requirement that {base} satisfies"
            ),
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
                    None => f.write_str("a side is not UTF-8 text"),
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
                }
            }
            Self::InUnit { directory, problem } => write!(f, "units/{directory}: {problem}"),
            Self::CatalogAndManifest { path } => write!(
                f,
                "{} is beside catalog.jsonc: a repository is a catalog, with its units in \
                 units/, or one unit, with its manifest at the top level, not both",
                path.display()
            ),
            Self::NotAUnitDirectory { path } => write!(
                f,
                "{} is not a directory: each entry of a catalog's units/ is a unit's directory",
                path.display()
            ),
            Self::UnitsNotADirectory { path } => write!(
                f,
                "{} is not a directory: a catalog's units are directories in its own units/, \
                 never behind a symbolic link",
                path.display()
            ),
            Self::NoUnits { path } => write!(
                f,
                "the catalog has no units: {} holds no unit directory",
                path.display()
            ),
            Self::CatalogPending { directory, paths } => {
                let paths: Vec<String> = paths
                    .iter()
                    .map(|path| format!("amendments/{path}"))
                    .collect();
                write!(
                    f,
                    "units/{directory}: amenders of files it does not amend conflict, and its \
                     build creates {} with each conflict marked",
                    paths.join(", ")
                )
            }
            Self::BaseStore(error) => error.fmt(f),
            Self::BaseMissing { path, provider } => write!(
                f,
                "amendments/{path}, an amendment of src/{path} of {provider}, has no stored \
                 base in the catalog's .assemblr/, so the changes the copy makes are unknown"
            ),
            Self::BaseStale { path, provider } => write!(
                f,
                "src/{path} of {provider}, as the units below compose it, changed since \
                 amendments/{path} was taken, and the copy is not rebased onto the change yet"
            ),
            Self::BaseUnmergeable {
                path,
                provider,
                reason,
            } => write!(
                f,
                "src/{path} of {provider}, as the units below compose it, changed since \
                 amendments/{path} was taken, and the change cannot be merged into the copy: \
                 {reason}"
            ),
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
