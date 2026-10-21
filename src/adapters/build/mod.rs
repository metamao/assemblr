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

mod amend;
mod check;
mod exclusive;
mod output;
mod publish;
mod update;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

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
use super::render::{Context, RenderError, render};
use crate::domain::{
    AmendmentBaseError, AvailableVersions, BaseIntegrityError, CompositionError, ContributionError,
    FileTree, LockChange, LockError, Manifest, ObjectId, ParameterName, ParameterValues,
    PublishedAmendmentError, RelativePath, Requirer, ResolutionError, Root, TEMPLATE_SUFFIX,
    UnitContent, UnitFile, UnitIdentity, UnitLock, Version, amendment_bases, compose,
    contribution_problems, published_amendments, relock, resolve, verify_bases,
};

/// The unit's manifest, at the repository's top level.
const MANIFEST: &str = "manifest.jsonc";

/// Builds the unit whose repository's top level is `root`, resolving its
/// dependencies from `registry`.
pub fn build_unit(root: &Path, registry: &Registry) -> Result<Built, BuildFailure> {
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

    let manifest = match read_working_manifest(root) {
        Ok(manifest) => Some(manifest),
        Err(error) => {
            problems.push(BuildProblem::Manifest(error));
            None
        }
    };
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
    let (Some(manifest), Some(previous), Some(index)) = (&manifest, previous, index) else {
        return Err(BuildFailure::Rejected(problems));
    };
    let previous_lock = previous.clone().unwrap_or_default();
    let inputs = Inputs {
        manifest,
        src: src.as_ref(),
        amendments: amendments.as_ref(),
        previous: &previous_lock,
    };
    // A build composes an ineffective unit as readily as any other: the
    // effective-contribution check gates publication (OC-11).
    let validated = validate(&inputs, &index, &mut problems).result;
    let Some(validated) = validated.filter(|_| problems.is_empty()) else {
        return Err(BuildFailure::Rejected(problems));
    };
    let lock = validated.lock;

    let lockfile = match &previous {
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
        output::publish(root, &files, new_lock, &guard).map_err(BuildFailure::Publish)?;
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
    let parameters: Vec<ParameterName> = manifest
        .params()
        .iter()
        .filter(|(_, definition)| definition.default().is_none())
        .map(|(name, _)| name.clone())
        .collect();
    if !parameters.is_empty() {
        problems.push(BuildProblem::DevelopmentValues {
            unit: manifest.name().clone(),
            parameters,
        });
    }
    let dependencies = match Dependencies::read(index, manifest) {
        Ok(dependencies) => dependencies,
        Err(found) => {
            problems.extend(found);
            return validation;
        }
    };
    let resolution = match resolve(
        Some(manifest.name()),
        manifest.dependencies(),
        &dependencies.available,
        &previous.versions(),
    ) {
        Ok(resolution) => resolution,
        Err(errors) => {
            problems.extend(errors.into_iter().map(BuildProblem::Resolution));
            return validation;
        }
    };
    let lock = match relock(previous, &resolution, index) {
        Ok(lock) => lock,
        Err(errors) => {
            problems.extend(errors.into_iter().map(BuildProblem::Lock));
            return validation;
        }
    };
    // A build never moves an amended unit away from its base. The cause is
    // told apart here, with the previous lock; the composition below would
    // only see that the base and the resolved version differ.
    let moved: BTreeSet<RelativePath> = amendment_bases(manifest, previous, &resolution)
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
    let contents = match dependencies.read_content(&lock, lock.units().keys()) {
        Ok(contents) => contents,
        Err(found) => {
            problems.extend(found);
            return validation;
        }
    };

    // Each published amender's bases are verified against the index here,
    // whatever else the composition finds: a disagreement is an integrity
    // conflict in its own right.
    let mut units = BTreeMap::new();
    for (identity, locked) in lock.units() {
        let key = (identity.clone(), locked.version().clone());
        let (Some(manifest), Some(content)) =
            (dependencies.manifests.get(&key), contents.get(identity))
        else {
            continue;
        };
        if let Some(published) = &content.lock {
            problems.extend(
                verify_bases(identity, locked.version(), manifest, published, index)
                    .into_iter()
                    .map(BuildProblem::BaseIntegrity),
            );
        }
        units.insert(
            identity.clone(),
            UnitContent::new(manifest, &content.files, &content.amendments),
        );
    }
    let (Some(src), Some(amendments)) = (src, amendments) else {
        return validation;
    };
    let providers: BTreeMap<&UnitIdentity, (&Version, &FileTree)> = lock
        .units()
        .iter()
        .filter_map(|(identity, locked)| {
            let content = contents.get(identity)?;
            Some((identity, (locked.version(), &content.files)))
        })
        .collect();
    validation.contribution = contribution_problems(manifest, src, amendments, &providers);
    let root_content = UnitContent::new(manifest, src, amendments);
    let composition = compose(
        resolution.graph(),
        Root::Unit(root_content),
        &units,
        &BTreeMap::new(),
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

    // Templates are rendered with the parameter values of the unit that
    // provides each one, plus the amending unit's values as `own` for a
    // copy. One copy takes the provider's file's place; several are
    // combined on the rendered content (see `super::merge`). Every
    // template's problems are reported, by delivered path and then
    // position.
    let no_values = ParameterValues::new();
    let values_of = |unit: &UnitIdentity| values_of(&composition, &no_values, unit);
    let mut files: Vec<(RelativePath, UnitFile)> = Vec::new();
    let mut notes = Vec::new();
    let mut failed = false;
    for (path, composed) in composition.files() {
        let executable = composed.is_executable();
        let provider = composed.unit();
        let published_version = |amender: &UnitIdentity| {
            (amender != manifest.name())
                .then(|| lock.units().get(amender))
                .flatten()
                .map(|locked| locked.version().clone())
        };
        // Renders one side: the provider's file, or an amender's copy.
        let mut side = |bytes: &[u8], amender: Option<&UnitIdentity>| -> Option<Vec<u8>> {
            if !composed.is_template() {
                return Some(bytes.to_vec());
            }
            let template = |error| BuildProblem::Template {
                unit: provider.clone(),
                path: composed.source().clone(),
                amended_by: amender.cloned(),
                published: amender.and_then(published_version),
                error,
            };
            let Ok(text) = std::str::from_utf8(bytes) else {
                problems.push(template(TemplateError::NotUtf8));
                return None;
            };
            let values = values_of(provider);
            let context = match amender {
                Some(amender) => Context::amended(values, values_of(amender)),
                None => Context::new(values),
            };
            match render(text, &context) {
                Ok(rendered) => Some(rendered.into_bytes()),
                Err(errors) => {
                    problems.extend(errors.into_iter().map(TemplateError::Render).map(template));
                    None
                }
            }
        };
        let delivered = match composed.amendments() {
            [] => side(composed.contents(), None),
            [(amender, copy)] => side(copy.contents(), Some(amender)),
            amendments => {
                let base = side(composed.contents(), None);
                let copies: Vec<Option<(&UnitIdentity, Vec<u8>)>> = amendments
                    .iter()
                    .map(|(amender, copy)| {
                        side(copy.contents(), Some(amender)).map(|text| (*amender, text))
                    })
                    .collect();
                match (base, copies.into_iter().collect::<Option<Vec<_>>>()) {
                    (Some(base), Some(copies)) => {
                        let merge = || BuildProblem::Merge {
                            unit: provider.clone(),
                            version: lock.units().get(provider).map_or_else(
                                || manifest.version().clone(),
                                |locked| locked.version().clone(),
                            ),
                            path: composed.source().clone(),
                            amenders: amendments
                                .iter()
                                .map(|(amender, _)| (*amender).clone())
                                .collect(),
                            failure: None,
                        };
                        let sides: Option<(&str, Vec<(&UnitIdentity, &str)>)> = utf8(&base)
                            .and_then(|base| {
                                copies
                                    .iter()
                                    .map(|(amender, copy)| utf8(copy).map(|copy| (*amender, copy)))
                                    .collect::<Option<Vec<_>>>()
                                    .map(|copies| (base, copies))
                            });
                        match sides {
                            None => {
                                problems.push(merge());
                                None
                            }
                            Some((base, copies)) => match super::merge::merge_json(base, &copies) {
                                Ok(merged) => {
                                    notes.extend(merged.notes().iter().map(|note| {
                                        format!("src/{} of {provider}: {note}", composed.source())
                                    }));
                                    Some(merged.into_text().into_bytes())
                                }
                                Err(failure) => {
                                    let mut problem = merge();
                                    if let BuildProblem::Merge { failure: slot, .. } = &mut problem
                                    {
                                        *slot = Some(failure);
                                    }
                                    problems.push(problem);
                                    None
                                }
                            },
                        }
                    }
                    _ => None,
                }
            }
        };
        match delivered {
            Some(bytes) => files.push((path.clone(), UnitFile::new(bytes, executable))),
            None => failed = true,
        }
    }
    if failed {
        return validation;
    }
    validation.result = Some(Validated {
        lock,
        changes: resolution.changes().clone(),
        files,
        notes,
    });
    validation
}

/// The effective values of `unit` in `composition`, or none.
fn values_of<'c>(
    composition: &'c crate::domain::Composition<'_>,
    none: &'c ParameterValues,
    unit: &UnitIdentity,
) -> &'c ParameterValues {
    composition.values().get(unit).unwrap_or(none)
}

/// `bytes` as text, if they are UTF-8.
fn utf8(bytes: &[u8]) -> Option<&str> {
    std::str::from_utf8(bytes).ok()
}

/// The published versions a resolution may choose from, read from the
/// registered repositories.
struct Dependencies {
    available: AvailableVersions,
    /// Each available version's manifest.
    manifests: BTreeMap<(UnitIdentity, Version), Manifest>,
    /// Each identity's repository, by identity.
    repositories: BTreeMap<UnitIdentity, GitRepository>,
    /// Each available version's commit.
    commits: BTreeMap<(UnitIdentity, Version), ObjectId>,
}

impl Dependencies {
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
        let mut dependencies = Self {
            available: AvailableVersions::new(),
            manifests: BTreeMap::new(),
            repositories: BTreeMap::new(),
            commits: BTreeMap::new(),
        };
        let mut problems = Vec::new();
        let mut pending: BTreeSet<UnitIdentity> = root.dependencies().keys().cloned().collect();
        let mut seen = BTreeSet::new();
        while let Some(identity) = pending.pop_first() {
            if !seen.insert(identity.clone()) || identity == *root.name() {
                continue;
            }
            let Some(unit) = index.units().get(&identity) else {
                continue;
            };
            let unreadable = |error| BuildProblem::Source {
                unit: identity.clone(),
                error,
            };
            let repository = match GitRepository::open(Path::new(unit.source().as_str())) {
                Ok(repository) => repository,
                Err(error) => {
                    problems.push(unreadable(error));
                    continue;
                }
            };
            let commits: Vec<&ObjectId> = unit
                .versions()
                .values()
                .map(|provenance| provenance.commit())
                .collect();
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
    /// The unit being built has required parameters without defaults, and
    /// supplying development values is not supported yet.
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
    BaseIntegrity(BaseIntegrityError),
    /// The unit and its dependencies cannot be composed.
    Composition(CompositionError),
    /// Several amendments of one provider file cannot be combined.
    Merge {
        /// The provider.
        unit: UnitIdentity,
        /// Its resolved version.
        version: Version,
        /// The amended path in its `src/`.
        path: RelativePath,
        /// The amenders, in requirer order.
        amenders: Vec<UnitIdentity>,
        /// Why, or `None` when a rendered side is not UTF-8 text.
        failure: Option<super::merge::MergeFailure>,
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
                    "{unit} declares required parameters without defaults: {}; composing it \
                     needs development values for them, and supplying development values is \
                     not supported yet",
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
                failure,
            } => {
                let amenders: Vec<String> = amenders.iter().map(ToString::to_string).collect();
                write!(
                    f,
                    "src/{path} of {unit} {version} is amended by {}, and the amendments cannot \
                     be combined: ",
                    amenders.join(", ")
                )?;
                match failure {
                    Some(failure) => failure.fmt(f),
                    None => f.write_str("a rendered side is not UTF-8 text, which a JSON file is"),
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
