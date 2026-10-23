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
mod output;
mod publish;
mod update;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub use add::{
    AddFailure, AddProblem, AddedParameter, DependencyAdded, DependencyChange, add_dependency,
};
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
use super::render::{Context, RenderError, parameters_referenced, render};
use crate::domain::{
    AmendmentBaseError, AvailableVersions, BaseIntegrityError, CompositionError, ContributionError,
    FileTree, LockChange, LockError, Manifest, MergeFormat, ObjectId, ParameterName,
    ParameterValues, PublishedAmendmentError, RelativePath, Requirer, ResolutionError, Root,
    TEMPLATE_SUFFIX, UnitContent, UnitFile, UnitIdentity, UnitLock, ValueKind, Version,
    amendment_bases, compose, contribution_problems, is_text, published_amendments, relock,
    resolve, verify_bases,
};

/// The unit's manifest, at the repository's top level.
const MANIFEST: &str = "manifest.jsonc";

/// Builds the unit whose repository's top level is `root`, resolving its
/// dependencies from `registry`.
pub fn build_unit(root: &Path, registry: &Registry) -> Result<Built, BuildFailure> {
    let mut opened = open(root, registry)?;
    let manifest = opened.manifest.clone();
    let validated = validate_opened(&mut opened, &manifest)?;
    publish_validated(root, &opened, &manifest, validated)
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
    manifest: &Manifest,
    validated: Validated,
) -> Result<Built, BuildFailure> {
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
                verify_bases(identity, locked.version(), manifest, published, index)
                    .into_iter()
                    .map(BuildProblem::BaseIntegrity),
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
    // A required parameter is built with its example: development values
    // are the author's, for this unit only, and a consumer never sees them.
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
    let development: BTreeMap<UnitIdentity, crate::domain::OptionValues> = if development.is_empty()
    {
        BTreeMap::new()
    } else {
        BTreeMap::from([(manifest.name().clone(), development)])
    };
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
    validation.result = Some(Validated {
        lock: resolved.lock,
        changes: resolved.resolution.changes().clone(),
        files: rendered.files,
        notes: rendered.notes.into_iter().map(|(_, note)| note).collect(),
    });
    validation
}

/// The rendered files of a composition.
pub(crate) struct Rendered {
    pub(crate) files: Vec<(RelativePath, UnitFile)>,
    /// What the composition reports without failing, each with the
    /// delivered path it concerns.
    pub(crate) notes: Vec<(RelativePath, String)>,
    /// The amendments applied to another version of their provider than
    /// the one they were taken from, each with its delivered path.
    pub(crate) crossed: Vec<(RelativePath, String)>,
}

/// Renders every file of `composition`: templates with the parameter
/// values of the unit that provides each one, plus the amending unit's
/// values as `own` for a copy. One copy takes the provider's file's place;
/// several are combined on the rendered content (see `super::merge`).
/// Every problem is reported, by delivered path and then position, and the
/// files are given only when there was none. `built` is the unit being
/// built, whose amendments are not published versions.
pub(crate) fn render_files(
    composition: &crate::domain::Composition<'_>,
    lock: &UnitLock,
    built: Option<&UnitIdentity>,
    dependencies: &Dependencies,
    problems: &mut Vec<BuildProblem>,
) -> Option<Rendered> {
    let no_values = ParameterValues::new();
    let values_of = |unit: &UnitIdentity| values_of(composition, &no_values, unit);
    let mut files: Vec<(RelativePath, UnitFile)> = Vec::new();
    let mut notes = Vec::new();
    let mut crossed = Vec::new();
    let mut failed = false;
    // Problems are collected here, since the rendering closure below holds
    // on to the sink while the rest of the loop reports into it too.
    let sink: std::cell::RefCell<Vec<BuildProblem>> = std::cell::RefCell::new(Vec::new());
    // The `src/` trees of the versions amendments were taken from, read
    // when a composition resolves a provider to another version.
    let history: std::cell::RefCell<BTreeMap<(UnitIdentity, Version), FileTree>> =
        std::cell::RefCell::new(BTreeMap::new());
    for (path, composed) in composition.files() {
        let executable = composed.is_executable();
        let provider = composed.unit();
        let resolved_version = lock.units().get(provider).map(|locked| locked.version());
        let published_version = |amender: &UnitIdentity| {
            (Some(amender) != built)
                .then(|| lock.units().get(amender))
                .flatten()
                .map(|locked| locked.version().clone())
        };
        // Renders one side: the provider's file, or an amender's copy.
        // `quiet` renders without reporting: the base of a single copy is
        // rendered only to compare, and a base that does not render is
        // not that build's problem.
        let quiet = std::cell::Cell::new(false);
        let side = |bytes: &[u8], amender: Option<&UnitIdentity>| -> Option<Vec<u8>> {
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
                if !quiet.get() {
                    sink.borrow_mut().push(template(TemplateError::NotUtf8));
                }
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
                    if !quiet.get() {
                        sink.borrow_mut()
                            .extend(errors.into_iter().map(TemplateError::Render).map(template));
                    }
                    None
                }
            }
        };
        // The amendments taken from another version of the provider than
        // the one resolved, with the provider's file as it was there.
        // Amendments whose file is the same bytes at both versions are
        // applied as if taken from the resolved one.
        let mut crossing: BTreeMap<&UnitIdentity, (Version, Vec<u8>)> = BTreeMap::new();
        let mut unreadable = false;
        for amendment in composed.amendments() {
            let base = amendment.base();
            let Some(resolved) = resolved_version.filter(|resolved| *resolved != base) else {
                continue;
            };
            crossed.push((
                path.clone(),
                format!(
                    "{path}: the amendment of {} was taken from {provider} {base} and is applied \
                     to {provider} {resolved}",
                    amendment.amender()
                ),
            ));
            let cross = |reason| BuildProblem::CrossVersion {
                unit: provider.clone(),
                path: composed.source().clone(),
                amender: amendment.amender().clone(),
                published: published_version(amendment.amender()),
                base: base.clone(),
                resolved: resolved.clone(),
                reason,
            };
            let key = (provider.clone(), base.clone());
            if !history.borrow().contains_key(&key) {
                match dependencies.read_src_at(provider, base) {
                    Some(Ok(tree)) => {
                        history.borrow_mut().insert(key.clone(), tree);
                    }
                    Some(Err(problem)) => {
                        sink.borrow_mut().push(problem);
                        unreadable = true;
                        continue;
                    }
                    None => {
                        sink.borrow_mut()
                            .push(cross(CrossVersionReason::BaseUnavailable));
                        unreadable = true;
                        continue;
                    }
                }
            }
            let tree = history.borrow();
            match tree
                .get(&key)
                .and_then(|tree| tree.get(composed.source()))
                .map(|file| file.contents().to_vec())
            {
                Some(bytes) if bytes == composed.contents() => {}
                Some(bytes) => {
                    crossing.insert(amendment.amender(), (base.clone(), bytes));
                }
                None => {
                    sink.borrow_mut()
                        .push(cross(CrossVersionReason::BaseUnavailable));
                    unreadable = true;
                }
            }
        }
        if unreadable {
            failed = true;
            continue;
        }
        let delivered = match composed.amendments() {
            [] => side(composed.contents(), None),
            [only] if crossing.is_empty() => {
                let (amender, copy) = (only.amender(), only.copy());
                let delivered = side(copy.contents(), Some(amender));
                // The unit's own copy takes the file's place whole, but a
                // second amender would make a merge of it, which carries
                // members and values only: say so to its author now.
                if Some(amender) == built && MergeFormat::of_path(path) == MergeFormat::Json {
                    quiet.set(true);
                    let base = side(composed.contents(), None);
                    quiet.set(false);
                    if let (Some(base), Some(copy)) = (
                        base.as_deref().and_then(utf8),
                        delivered.as_deref().and_then(utf8),
                    ) {
                        for note in super::merge::unshared_notes(base, amender, copy) {
                            let super::merge::MergeNote::NotCarried { what, .. } = note else {
                                continue;
                            };
                            notes.push((
                                path.clone(),
                                format!(
                                    "amendments/{} of {amender}: if another unit amends src/{} \
                                     of {provider} too, {what} of this copy is not carried; the \
                                     merge carries members and values, not comments or order",
                                    composed.source(),
                                    composed.source()
                                ),
                            ));
                        }
                    }
                }
                delivered
            }
            amendments => {
                let merge = || BuildProblem::Merge {
                    unit: provider.clone(),
                    // An amended file's provider is a locked dependency:
                    // the root is never amended.
                    version: resolved_version.cloned(),
                    path: composed.source().clone(),
                    // By identity, as the conflicts name them.
                    amenders: amendments
                        .iter()
                        .map(|amendment| amendment.amender().clone())
                        .collect::<std::collections::BTreeSet<_>>()
                        .into_iter()
                        .collect(),
                    crossed: crossing
                        .iter()
                        .map(|(amender, (base, _))| ((*amender).clone(), base.clone()))
                        .collect(),
                    failure: None,
                };
                // A merge across versions is made on text, in a format
                // with a merge; anything else is applied only to the file
                // it was taken from.
                let cross_problem = |amender: &UnitIdentity, reason| {
                    let (base, _) = &crossing[amender];
                    BuildProblem::CrossVersion {
                        unit: provider.clone(),
                        path: composed.source().clone(),
                        amender: amender.clone(),
                        published: published_version(amender),
                        base: base.clone(),
                        resolved: resolved_version.cloned().unwrap_or_else(|| base.clone()),
                        reason,
                    }
                };
                let mut refused = false;
                for amender in crossing.keys() {
                    let not_text = !is_text(composed.contents())
                        || !is_text(&crossing[amender].1)
                        || amendments
                            .iter()
                            .any(|amendment| !is_text(amendment.copy().contents()));
                    if MergeFormat::of_path(path) == MergeFormat::Unsupported || not_text {
                        sink.borrow_mut()
                            .push(cross_problem(amender, CrossVersionReason::NoMerge));
                        refused = true;
                    }
                }
                // A parameter a template names must mean the same at the
                // resolved version as at the base, or the contribution
                // cannot be told from the provider's own change.
                if !refused && composed.is_template() {
                    for amendment in amendments {
                        let Some((base_version, base_source)) = crossing.get(amendment.amender())
                        else {
                            continue;
                        };
                        let mut referenced = utf8(base_source)
                            .map(parameters_referenced)
                            .unwrap_or_default();
                        referenced.extend(
                            utf8(amendment.copy().contents())
                                .map(parameters_referenced)
                                .unwrap_or_default(),
                        );
                        let at = |version: &Version| {
                            dependencies
                                .manifests
                                .get(&(provider.clone(), version.clone()))
                                .map(|manifest| manifest.params())
                        };
                        let (Some(then), Some(now)) =
                            (at(base_version), resolved_version.and_then(at))
                        else {
                            continue;
                        };
                        for name in referenced {
                            let kind =
                                |params: &BTreeMap<_, crate::domain::ParameterDefinition>| {
                                    params
                                        .get(&name)
                                        .map(|definition| definition.parameter_type().value_kind())
                                };
                            if kind(now).is_none() || kind(then) != kind(now) {
                                sink.borrow_mut().push(cross_problem(
                                    amendment.amender(),
                                    CrossVersionReason::Parameter {
                                        name: name.clone(),
                                        base: kind(then),
                                        resolved: kind(now),
                                    },
                                ));
                                refused = true;
                            }
                        }
                    }
                }
                if refused {
                    None
                } else {
                    let base = side(composed.contents(), None);
                    let copies: Vec<Option<RenderedCopy<'_>>> = amendments
                        .iter()
                        .map(|amendment| {
                            let copy =
                                side(amendment.copy().contents(), Some(amendment.amender()))?;
                            let amender_base = match (&base, crossing.get(amendment.amender())) {
                                (_, Some((_, source))) => side(source, None)?,
                                (Some(base), None) => base.clone(),
                                (None, None) => return None,
                            };
                            Some((amendment.amender(), amender_base, copy))
                        })
                        .collect();
                    match (base, copies.into_iter().collect::<Option<Vec<_>>>()) {
                        (Some(base), Some(copies)) => {
                            let sides: Option<(&str, Vec<CopyText<'_>>)> =
                                utf8(&base).and_then(|provider_text| {
                                    copies
                                        .iter()
                                        .map(|(amender, amender_base, copy)| {
                                            Some((*amender, utf8(amender_base)?, utf8(copy)?))
                                        })
                                        .collect::<Option<Vec<_>>>()
                                        .map(|copies| (provider_text, copies))
                                });
                            match sides {
                                None => {
                                    sink.borrow_mut().push(merge());
                                    None
                                }
                                Some((provider_text, copies)) => {
                                    match match MergeFormat::of_path(path) {
                                        MergeFormat::Json => Some(super::merge::merge_json_from(
                                            provider_text,
                                            &copies,
                                        )),
                                        MergeFormat::Text => {
                                            Some(super::merge::merge_text_files_from(
                                                provider,
                                                provider_text,
                                                &copies,
                                            ))
                                        }
                                        // The composition refuses these
                                        // before it gets here.
                                        MergeFormat::Unsupported => None,
                                    } {
                                        Some(Ok(merged)) => {
                                            notes.extend(merged.notes().iter().map(|note| {
                                                (
                                                    path.clone(),
                                                    format!(
                                                        "src/{} of {provider}: {note}",
                                                        composed.source()
                                                    ),
                                                )
                                            }));
                                            Some(merged.into_text().into_bytes())
                                        }
                                        Some(Err(failure)) => {
                                            let mut problem = merge();
                                            if let BuildProblem::Merge { failure: slot, .. } =
                                                &mut problem
                                            {
                                                *slot = Some(failure);
                                            }
                                            sink.borrow_mut().push(problem);
                                            None
                                        }
                                        None => {
                                            sink.borrow_mut().push(merge());
                                            None
                                        }
                                    }
                                }
                            }
                        }
                        _ => None,
                    }
                }
            }
        };
        match delivered {
            Some(bytes) => files.push((path.clone(), UnitFile::new(bytes, executable))),
            None => failed = true,
        }
    }
    problems.extend(sink.into_inner());
    if failed {
        return None;
    }
    Some(Rendered {
        files,
        notes,
        crossed,
    })
}

/// An amender, the provider's file as it was where the copy was taken from,
/// and the copy, all rendered.
type RenderedCopy<'a> = (&'a UnitIdentity, Vec<u8>, Vec<u8>);

/// [`RenderedCopy`] as text.
type CopyText<'a> = (&'a UnitIdentity, &'a str, &'a str);

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
    BaseIntegrity(BaseIntegrityError),
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
