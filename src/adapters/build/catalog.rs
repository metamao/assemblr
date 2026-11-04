//! `unit build` and `unit check` in a catalog: a repository with
//! `catalog.jsonc` at its top level and one unit in each directory of
//! `units/` (see `docs/domain/units.md` and
//! `docs/adr/0039-catalog-repositories.md`).
//!
//! A catalog's units are read from its working tree and resolved together,
//! each sibling at the catalog's version with the content of the working
//! tree, never from the registry. The whole catalog resolves to one lock,
//! which is its one lockfile, at its top level, for what its units depend
//! on in other repositories (**Provisional (D-53)**). Each unit is then
//! validated as a unit outside a catalog is, under that lock, and built
//! into its own `units/<name>/build/`. In a unit's directory, the command
//! builds or checks that unit; at the top level, every unit.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::bases::{self, Drift, RebasedCopy, StoredBases};
use super::check::{CheckFailure, CheckProblem, Checked, publication_checks};
use super::exclusive::BuildLock;
use super::{
    BuildFailure, BuildProblem, Built, Dependencies, Inputs, Joint, LockfileOutcome, MANIFEST,
    ManifestFileError, PublishedContent, Sibling, Siblings, Validated, output, read_working_text,
    validate, write_pending,
};
use crate::adapters::git::{CATALOG, GitRepository, ReadSrcError, UNITS};
use crate::adapters::lockfile;
use crate::adapters::manifest::{InCatalog, read_catalog, read_catalog_manifest};
use crate::adapters::registry::Registry;
use crate::domain::{
    Catalog, FileTree, LockChange, LockedUnit, Manifest, ObjectId, Provenance, RegistryIndex,
    RelativePath, UnitFile, UnitIdentity, UnitLock, VersionRequirement, resolve, update_lock,
};

/// Where a new `catalog.jsonc` is written before it replaces the old one.
const CATALOG_NEW: &str = ".assemblr-catalog.jsonc.new";

/// Where a unit command runs, as far as catalogs go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    /// Not in a catalog: the directory is a unit repository's top level, or
    /// is not a unit's at all.
    Unit,
    /// A catalog's top level, which has `catalog.jsonc`.
    Catalog,
    /// The directory of a unit in a catalog, `units/<directory>`.
    CatalogUnit {
        /// The catalog's top level.
        catalog: PathBuf,
        /// The unit's directory, under `units/`.
        directory: String,
    },
}

/// Where `directory` is: a catalog's top level when it has
/// `catalog.jsonc`, a unit of a catalog when it is a directory of
/// `units/` beside one, or neither.
pub fn locate_unit(directory: &Path) -> Location {
    let exists = |path: &Path| fs::symlink_metadata(path).is_ok();
    if exists(&directory.join(CATALOG)) {
        return Location::Catalog;
    }
    let Ok(absolute) = std::path::absolute(directory) else {
        return Location::Unit;
    };
    let (Some(name), Some(units)) = (absolute.file_name(), absolute.parent()) else {
        return Location::Unit;
    };
    let (Some(name), Some(catalog)) = (name.to_str(), units.parent()) else {
        return Location::Unit;
    };
    if units.file_name() != Some(UNITS.as_ref()) || !exists(&catalog.join(CATALOG)) {
        return Location::Unit;
    }
    Location::CatalogUnit {
        catalog: catalog.to_owned(),
        directory: name.to_owned(),
    }
}

/// Builds every unit of the catalog whose top level is `root`, each into
/// its `units/<name>/build/`, with the catalog's one lockfile, resolving
/// what they depend on in other repositories from `registry`.
///
/// Nothing is written unless every unit builds. The builds are then
/// written one unit at a time, the first with the lockfile; if one fails,
/// the failure lists the units already written.
pub fn build_catalog(root: &Path, registry: &Registry) -> Result<Vec<Built>, BuildFailure> {
    let mut opened = open(root, registry).map_err(BuildFailure::Rejected)?;
    let mut problems = std::mem::take(&mut opened.problems);
    let Some(joint) = opened.joint.take() else {
        return Err(BuildFailure::Rejected(problems));
    };
    let joint = &joint;
    let scope: BTreeSet<UnitIdentity> = opened.units.keys().cloned().collect();
    let rebase = rebase(root, &mut opened, joint, &scope, &mut problems)?;
    let interrupted = opened.guard.interrupted().unwrap_or(false);
    let mut validated = Vec::new();
    for (index, (identity, unit)) in opened.units.iter().enumerate() {
        let lock_root = (index == 0).then_some(root);
        problems.extend(
            output::check(&unit.path, lock_root, interrupted)
                .into_iter()
                .map(|problem| in_unit(&unit.directory, BuildProblem::Output(problem))),
        );
        let mut found = Vec::new();
        let result = validate_unit(unit, &opened.siblings, joint, &opened.index, &mut found);
        problems.extend(
            found
                .into_iter()
                .map(|problem| in_unit(&unit.directory, problem)),
        );
        match result {
            Some(result) if !result.pending.is_empty() => {
                problems.push(BuildProblem::CatalogPending {
                    directory: unit.directory.clone(),
                    paths: result
                        .pending
                        .iter()
                        .map(|file| file.path.clone())
                        .collect(),
                });
            }
            Some(result) => validated.push((identity, unit, result)),
            None => {}
        }
    }
    if !problems.is_empty() {
        return Err(BuildFailure::Rejected(problems));
    }
    let mut rebased = apply_rebase(root, rebase)?;
    let lockfile = lockfile_outcome(opened.previous.as_ref(), &joint.lock);
    let mut built = Vec::new();
    for (index, (identity, unit, validated)) in validated.into_iter().enumerate() {
        let first = index == 0;
        let outcome = if first {
            lockfile
        } else {
            LockfileOutcome::Unchanged
        };
        let (own, rest) = rebased
            .into_iter()
            .partition(|copy: &RebasedCopy| copy.unit() == identity);
        rebased = rest;
        match write(root, identity, unit, validated, outcome, own, &opened.guard) {
            Ok(unit) => built.push(unit),
            Err(error) => {
                return Err(BuildFailure::PartlyBuilt {
                    built,
                    directory: unit.directory.clone(),
                    error,
                });
            }
        }
    }
    if let Some(first) = built.first_mut() {
        first.declared = declare_in_catalog(root, &opened);
    }
    Ok(built)
}

/// Sets the `assemblr` field of the catalog's `catalog.jsonc` after a
/// build, as a unit's build does in its manifest.
fn declare_in_catalog(root: &Path, opened: &Opened) -> Option<super::Declared> {
    let text = opened.catalog_text.as_deref()?;
    super::declare_required(
        &root.join(CATALOG),
        text,
        &root.join(CATALOG_NEW),
        |edited, version| {
            let expected = read_catalog(text).map(|read| read.with_assemblr(Some(version.clone())));
            expected.is_ok() && read_catalog(edited).ok() == expected.ok()
        },
    )
}

/// Builds the unit in `units/<directory>` of the catalog whose top level
/// is `root` into its `build/`, with the catalog's one lockfile. A conflict
/// between amenders of a file it does not amend creates the pending state
/// in its directory, as for a unit outside a catalog.
pub(super) fn build_in_catalog(
    root: &Path,
    directory: &str,
    registry: &Registry,
) -> Result<Built, BuildFailure> {
    let mut opened = open(root, registry).map_err(BuildFailure::Rejected)?;
    let mut problems = std::mem::take(&mut opened.problems);
    let identity = opened
        .units
        .iter()
        .find(|(_, unit)| unit.directory == directory)
        .map(|(identity, _)| identity.clone());
    let (Some(joint), Some(identity)) = (opened.joint.take(), identity) else {
        if problems.is_empty() {
            problems.push(not_a_unit(root, directory));
        }
        return Err(BuildFailure::Rejected(problems));
    };
    let joint = &joint;
    // The copies rebased are those of the unit and of the units of the
    // catalog below it, which its build composes.
    let scope = bases::closure(&identity, &opened.siblings);
    let rebase = rebase(root, &mut opened, joint, &scope, &mut problems)?;
    let Some(unit) = opened.units.get(&identity) else {
        return Err(BuildFailure::Rejected(problems));
    };
    let identity = &identity;
    let interrupted = opened.guard.interrupted().unwrap_or(false);
    problems.extend(
        output::check(&unit.path, Some(root), interrupted)
            .into_iter()
            .map(BuildProblem::Output),
    );
    let validated = validate_unit(unit, &opened.siblings, joint, &opened.index, &mut problems);
    let validated = match validated {
        Some(validated) if problems.is_empty() => validated,
        _ => return Err(BuildFailure::Rejected(problems)),
    };
    if !validated.pending.is_empty() {
        // The copies the pending state creates are new amendments, whose
        // bases are stored with the rebase, before them: a base stored for
        // a copy the pending state then fails to write is unused.
        let (mut drift, stored) = match rebase {
            Some(rebase) => rebase,
            None => {
                let stored = StoredBases::read(root).map_err(|error| {
                    BuildFailure::Rejected(vec![BuildProblem::BaseStore(error)])
                })?;
                let drift = Drift {
                    copies: Vec::new(),
                    bases: stored.all().clone(),
                };
                (drift, stored)
            }
        };
        for pending in &validated.pending {
            if let Some(key) = bases::key(directory, &pending.path) {
                drift.bases.insert(key, pending.copy.clone());
            }
        }
        bases::apply(root, &drift, &stored).map_err(BuildFailure::RebaseWrite)?;
        let catalog = opened
            .catalog
            .as_ref()
            .ok_or_else(|| BuildFailure::Rejected(vec![not_a_unit(root, directory)]))?;
        let in_catalog = InCatalog::new(catalog, directory, &opened.directories);
        return Err(write_pending(
            &unit.path,
            root,
            opened.previous.as_ref(),
            &unit.text,
            &unit.manifest,
            validated,
            Some(&in_catalog),
        ));
    }
    let rebased = apply_rebase(root, rebase)?;
    let outcome = lockfile_outcome(opened.previous.as_ref(), &joint.lock);
    let mut built = write(
        root,
        identity,
        unit,
        validated,
        outcome,
        rebased,
        &opened.guard,
    )
    .map_err(BuildFailure::Publish)?;
    built.declared = declare_in_catalog(root, &opened);
    Ok(built)
}

/// Rebases the copies of the units of `scope` whose stored base is not the
/// file below them now (see `bases`), in memory: `opened` holds the rebased
/// copies afterwards. When a rebase leaves conflicts, every rebased copy is
/// written with the new bases, and the build stops there. Otherwise the
/// rebase is given back, to be written once the whole build is validated;
/// `None` when there is nothing to write, or when `problems`, which a
/// missing or unusable stored base joins, already reject the build.
fn rebase(
    root: &Path,
    opened: &mut Opened,
    joint: &JointLock,
    scope: &BTreeSet<UnitIdentity>,
    problems: &mut Vec<BuildProblem>,
) -> Result<Option<(Drift, StoredBases)>, BuildFailure> {
    if !problems.is_empty() {
        return Ok(None);
    }
    let declares = opened
        .siblings
        .units
        .values()
        .any(|sibling| !sibling.manifest.amendments().is_empty());
    let stored = match StoredBases::read(root) {
        Ok(stored) => stored,
        Err(error) => {
            // A store nothing uses is no problem of the build.
            if declares {
                problems.push(BuildProblem::BaseStore(error));
            }
            return Ok(None);
        }
    };
    let mut found = Vec::new();
    let drift = bases::drift(
        scope,
        &mut opened.siblings,
        &joint.lock,
        &opened.index,
        &stored,
        true,
        &mut found,
    );
    if !found.is_empty() {
        problems.extend(found);
        return Ok(None);
    }
    for copy in &drift.copies {
        let Some(amendments) = opened
            .units
            .get_mut(copy.unit())
            .and_then(|unit| unit.amendments.as_mut())
        else {
            continue;
        };
        let executable = amendments
            .get(copy.path())
            .is_some_and(UnitFile::is_executable);
        amendments.insert(
            copy.path().clone(),
            UnitFile::new(copy.copy.clone(), executable),
        );
    }
    if drift.conflicts() {
        bases::apply(root, &drift, &stored).map_err(BuildFailure::RebaseWrite)?;
        return Err(BuildFailure::Rebased(drift.copies));
    }
    if drift.copies.is_empty() && drift.bases == *stored.all() {
        return Ok(None);
    }
    Ok(Some((drift, stored)))
}

/// The problems of the stored bases of the amendments of the units of
/// `scope`: a base that cannot be read, is missing, or is not the file
/// below the copy now.
pub(super) fn stale_bases(
    root: &Path,
    scope: &BTreeSet<UnitIdentity>,
    siblings: &mut Siblings,
    joint: &JointLock,
    index: &RegistryIndex,
) -> Vec<BuildProblem> {
    let declares = scope.iter().any(|identity| {
        siblings
            .units
            .get(identity)
            .is_some_and(|sibling| !sibling.manifest.amendments().is_empty())
    });
    if !declares {
        return Vec::new();
    }
    let stored = match StoredBases::read(root) {
        Ok(stored) => stored,
        Err(error) => return vec![BuildProblem::BaseStore(error)],
    };
    let mut problems = Vec::new();
    bases::drift(
        scope,
        siblings,
        &joint.lock,
        index,
        &stored,
        false,
        &mut problems,
    );
    problems
}

/// Writes `rebase`, the rebase of a validated build, and gives back the
/// copies rebased.
fn apply_rebase(
    root: &Path,
    rebase: Option<(Drift, StoredBases)>,
) -> Result<Vec<RebasedCopy>, BuildFailure> {
    let Some((drift, stored)) = rebase else {
        return Ok(Vec::new());
    };
    bases::apply(root, &drift, &stored).map_err(BuildFailure::RebaseWrite)?;
    Ok(drift.copies)
}

/// Checks every unit of the catalog whose top level is `root`, as
/// `unit check` checks a unit, under the catalog's one lockfile.
pub fn check_catalog(root: &Path, registry: &Registry) -> Result<Vec<Checked>, CheckFailure> {
    check(root, None, registry)
}

/// Checks the unit in `units/<directory>` of the catalog whose top level
/// is `root`.
pub(super) fn check_in_catalog(
    root: &Path,
    directory: &str,
    registry: &Registry,
) -> Result<Checked, CheckFailure> {
    check(root, Some(directory), registry).map(|mut checked| checked.remove(0))
}

/// Checks the unit in `units/<only>`, or every unit when `only` is `None`.
/// The lockfile's problems are reported once, for the whole catalog; when
/// every unit is checked, each other problem names its unit.
fn check(
    root: &Path,
    only: Option<&str>,
    registry: &Registry,
) -> Result<Vec<Checked>, CheckFailure> {
    let mut opened = open(root, registry).map_err(|problems| {
        CheckFailure::Rejected(problems.into_iter().map(CheckProblem::Build).collect())
    })?;
    let mut problems: Vec<CheckProblem> = std::mem::take(&mut opened.problems)
        .into_iter()
        .map(CheckProblem::Build)
        .collect();
    let units: Vec<(&UnitIdentity, &WorkingUnit)> = opened
        .units
        .iter()
        .filter(|(_, unit)| only.is_none_or(|only| unit.directory == only))
        .collect();
    if let Some(only) = only {
        if units.is_empty() && problems.is_empty() {
            problems.push(CheckProblem::Build(not_a_unit(root, only)));
        }
    }
    let Some(joint) = &opened.joint else {
        return Err(CheckFailure::Rejected(problems));
    };
    // Every copy checked is on the file below it now: a stale base is the
    // next build's to rebase.
    let scope: BTreeSet<UnitIdentity> = units
        .iter()
        .map(|(identity, _)| (*identity).clone())
        .collect();
    problems.extend(
        stale_bases(root, &scope, &mut opened.siblings, joint, &opened.index)
            .into_iter()
            .map(CheckProblem::Build),
    );
    let mut checked = Vec::new();
    let mut lockfile_reported = false;
    for (identity, unit) in units {
        let mut found = Vec::new();
        let (Some(src), Some(amendments)) = (&unit.src, &unit.amendments) else {
            continue;
        };
        let outcome = publication_checks(
            &Inputs {
                manifest: &unit.manifest,
                src: Some(src),
                amendments: Some(amendments),
                previous: &joint.lock,
                catalog: Some(&Joint {
                    siblings: &opened.siblings,
                    lock: &joint.lock,
                    changes: &joint.changes,
                }),
            },
            opened.previous.is_some(),
            &opened.index,
            &mut found,
        );
        let mut lockfile = false;
        for problem in found {
            if matches!(
                problem,
                CheckProblem::NoLockfile | CheckProblem::LockfileStale { .. }
            ) {
                if !lockfile_reported {
                    problems.push(problem);
                }
                lockfile = true;
            } else if only.is_some() {
                problems.push(problem);
            } else {
                problems.push(match problem {
                    CheckProblem::Build(problem) => {
                        CheckProblem::Build(in_unit(&unit.directory, problem))
                    }
                    problem => CheckProblem::InUnit {
                        directory: unit.directory.clone(),
                        problem: Box::new(problem),
                    },
                });
            }
        }
        lockfile_reported = lockfile_reported || lockfile;
        if let Some(outcome) = outcome {
            checked.push(Checked {
                unit: identity.clone(),
                version: unit.manifest.version().clone(),
                files: outcome.files,
                dependencies: outcome.dependencies,
                notes: outcome.notes,
            });
        }
    }
    if problems.is_empty() {
        Ok(checked)
    } else {
        Err(CheckFailure::Rejected(problems))
    }
}

/// The problem for `directory`, under `units/`, which holds no unit the
/// catalog reads.
fn not_a_unit(root: &Path, directory: &str) -> BuildProblem {
    BuildProblem::Manifest(ManifestFileError::Missing {
        path: root.join(UNITS).join(directory).join(MANIFEST),
    })
}

/// `problem`, found in the unit in `units/<directory>`.
pub(super) fn in_unit(directory: &str, problem: BuildProblem) -> BuildProblem {
    BuildProblem::InUnit {
        directory: directory.to_owned(),
        problem: Box::new(problem),
    }
}

/// What happens to the lockfile when `lock` replaces `previous`.
fn lockfile_outcome(previous: Option<&UnitLock>, lock: &UnitLock) -> LockfileOutcome {
    match previous {
        None => LockfileOutcome::Created,
        Some(previous) if previous == lock => LockfileOutcome::Unchanged,
        Some(_) => LockfileOutcome::Updated,
    }
}

/// Validates `unit` with its siblings under the catalog's lock, as a build
/// does.
fn validate_unit(
    unit: &WorkingUnit,
    siblings: &Siblings,
    joint: &JointLock,
    index: &RegistryIndex,
    problems: &mut Vec<BuildProblem>,
) -> Option<Validated> {
    let catalog = Joint {
        siblings,
        lock: &joint.lock,
        changes: &joint.changes,
    };
    let inputs = Inputs {
        manifest: &unit.manifest,
        src: unit.src.as_ref(),
        amendments: unit.amendments.as_ref(),
        previous: &joint.lock,
        catalog: Some(&catalog),
    };
    let mut found = Vec::new();
    let validated = validate(&inputs, index, &mut found).result;
    let clean = found.is_empty();
    problems.extend(found);
    validated.filter(|_| clean)
}

/// Writes the build of `unit` into its `build/`, and the lockfile at the
/// catalog's top level when `lockfile` says it changes.
fn write(
    root: &Path,
    identity: &UnitIdentity,
    unit: &WorkingUnit,
    validated: Validated,
    lockfile: LockfileOutcome,
    rebased: Vec<RebasedCopy>,
    guard: &BuildLock,
) -> Result<Built, super::PublishError> {
    let files: Vec<(&RelativePath, &crate::domain::UnitFile)> = validated
        .files
        .iter()
        .map(|(path, file)| (path, file))
        .collect();
    let new_lock = (lockfile != LockfileOutcome::Unchanged).then_some(&validated.lock);
    let published = output::publish(&unit.path, root, &files, new_lock, guard)?;
    Ok(Built {
        unit: identity.clone(),
        version: unit.manifest.version().clone(),
        directory: Some(unit.directory.clone()),
        files: files.len(),
        dependencies: validated.dependencies,
        lockfile,
        changes: if lockfile == LockfileOutcome::Unchanged {
            BTreeMap::new()
        } else {
            validated.changes
        },
        notes: validated.notes,
        placeholders: validated.placeholders,
        unresolved: validated.unresolved,
        leftover: published.leftover,
        rebased,
        declared: None,
    })
}

/// A catalog's working tree, read for a build or a check under the lock
/// that serializes builds of the repository.
pub(super) struct Opened {
    pub(super) guard: BuildLock,
    /// The catalog's `catalog.jsonc`, when it could be read.
    pub(super) catalog: Option<Catalog>,
    /// Its text, when it could be read.
    pub(super) catalog_text: Option<String>,
    /// Its unit directories, under `units/`.
    pub(super) directories: BTreeSet<String>,
    /// Every unit whose manifest could be read, by identity.
    pub(super) units: BTreeMap<UnitIdentity, WorkingUnit>,
    /// The units whose files could be read too, as siblings.
    pub(super) siblings: Siblings,
    /// The lockfile, when there is one.
    pub(super) previous: Option<UnitLock>,
    pub(super) index: RegistryIndex,
    /// The lock the whole catalog resolves to, when every unit could be
    /// read and resolved.
    pub(super) joint: Option<JointLock>,
    /// The problems found, which reject the build or check.
    pub(super) problems: Vec<BuildProblem>,
}

/// The lock a catalog's units resolve to together, without the units
/// themselves, and how it differs from the lockfile.
pub(super) struct JointLock {
    pub(super) lock: UnitLock,
    pub(super) changes: BTreeMap<UnitIdentity, LockChange>,
}

/// A unit of a catalog, read from the working tree.
pub(super) struct WorkingUnit {
    /// Its directory, under `units/`.
    pub(super) directory: String,
    /// The directory's path.
    pub(super) path: PathBuf,
    /// The text of its `manifest.jsonc`.
    pub(super) text: String,
    pub(super) manifest: Manifest,
    /// Its `src/` files, or `None` when they could not be read.
    pub(super) src: Option<FileTree>,
    /// Its amendment copies, or `None` as for `src`.
    pub(super) amendments: Option<FileTree>,
}

/// Reads the catalog whose top level is `root` from its working tree, and
/// resolves its units together. Only a catalog whose repository cannot be
/// opened or locked is rejected here; every other problem is kept for the
/// caller to report with its own.
pub(super) fn open(root: &Path, registry: &Registry) -> Result<Opened, Vec<BuildProblem>> {
    let rejected = |problem| vec![problem];
    let repository =
        GitRepository::open(root).map_err(|error| rejected(BuildProblem::Repository(error)))?;
    let git_dir = repository
        .git_directory()
        .map_err(|error| rejected(BuildProblem::Repository(error)))?;
    let guard =
        BuildLock::acquire(&git_dir).map_err(|error| rejected(BuildProblem::BuildLock(error)))?;
    match super::update::interrupted(&git_dir) {
        Ok(None) => {}
        Ok(Some(journal)) => return Err(rejected(BuildProblem::UpdateInterrupted { journal })),
        Err(error) => return Err(rejected(BuildProblem::GitDirectory(error))),
    }

    let mut problems = Vec::new();
    let manifest = root.join(MANIFEST);
    if fs::symlink_metadata(&manifest).is_ok() {
        problems.push(BuildProblem::CatalogAndManifest { path: manifest });
    }
    let catalog_path = root.join(CATALOG);
    let catalog_text = read_working_text(catalog_path.clone())
        .map_err(|error| problems.push(BuildProblem::Manifest(error)))
        .ok();
    let catalog = catalog_text.as_deref().and_then(|text| {
        read_catalog(text)
            .map_err(|error| {
                problems.push(BuildProblem::Manifest(ManifestFileError::Invalid {
                    path: catalog_path.clone(),
                    error,
                }));
            })
            .ok()
    });
    let entries = repository
        .working_units()
        .map_err(|error| problems.push(BuildProblem::Repository(error)))
        .unwrap_or_default();
    let units_path = root.join(UNITS);
    // `units/` itself is a real directory: a link would read units, and
    // write their builds, outside the repository.
    if let Ok(metadata) = fs::symlink_metadata(&units_path) {
        if !metadata.is_dir() {
            return Err(vec![BuildProblem::UnitsNotADirectory { path: units_path }]);
        }
    }
    let mut directories = BTreeSet::new();
    for (name, directory) in entries {
        if directory {
            directories.insert(name);
        } else {
            problems.push(BuildProblem::NotAUnitDirectory {
                path: units_path.join(name),
            });
        }
    }
    if directories.is_empty() {
        problems.push(BuildProblem::NoUnits {
            path: units_path.clone(),
        });
    }

    let mut units = BTreeMap::new();
    if let Some(catalog) = &catalog {
        for directory in &directories {
            let path = units_path.join(directory);
            let manifest_path = path.join(MANIFEST);
            let Some(text) = read_working_text(manifest_path.clone())
                .map_err(|error| problems.push(BuildProblem::Manifest(error)))
                .ok()
            else {
                continue;
            };
            let in_catalog = InCatalog::new(catalog, directory, &directories);
            let manifest = match read_catalog_manifest(&text, &in_catalog) {
                Ok(manifest) => manifest,
                Err(error) => {
                    problems.push(BuildProblem::Manifest(ManifestFileError::Invalid {
                        path: manifest_path,
                        error,
                    }));
                    continue;
                }
            };
            let unit_repository = repository.clone().in_unit_directory(directory);
            let mut read = |files: Result<FileTree, ReadSrcError>,
                            kind: fn(ReadSrcError) -> BuildProblem| {
                match files {
                    Ok(files) => Some(files),
                    Err(ReadSrcError::Git(error)) => {
                        problems.push(BuildProblem::Repository(error));
                        None
                    }
                    Err(error) => {
                        problems.push(in_unit(directory, kind(error)));
                        None
                    }
                }
            };
            let src = read(unit_repository.read_working_src(), BuildProblem::Src);
            let amendments = read(
                unit_repository.read_working_amendments(),
                BuildProblem::Amendments,
            );
            units.insert(
                manifest.name().clone(),
                WorkingUnit {
                    directory: directory.clone(),
                    path,
                    text,
                    manifest,
                    src,
                    amendments,
                },
            );
        }
    }
    let previous = lockfile::read(root)
        .map_err(|error| problems.push(BuildProblem::Lockfile(error)))
        .ok();
    let index = registry
        .index()
        .map_err(|error| problems.push(BuildProblem::Registry(error)))
        .ok();

    let mut siblings = Siblings::default();
    for (identity, unit) in &units {
        let (Some(src), Some(amendments)) = (&unit.src, &unit.amendments) else {
            continue;
        };
        siblings.units.insert(
            identity.clone(),
            Sibling {
                manifest: unit.manifest.clone(),
                content: PublishedContent {
                    files: src.clone(),
                    amendments: amendments.clone(),
                    lock: None,
                },
                provenance: working_provenance(),
            },
        );
    }
    let complete = problems.is_empty() && siblings.units.len() == directories.len();
    let (Some(previous), Some(index)) = (previous, index) else {
        return Err(problems);
    };
    let joint = if complete {
        match joint_lock(&siblings, previous.as_ref(), &index) {
            Ok(joint) => Some(joint),
            Err(found) => {
                problems.extend(found);
                None
            }
        }
    } else {
        None
    };
    // A sibling's amendments have their bases in the catalog's lock, as
    // they will when it is published.
    if let Some(joint) = &joint {
        for sibling in siblings.units.values_mut() {
            if !sibling.manifest.amendments().is_empty() {
                sibling.content.lock = Some(joint.lock.clone());
            }
        }
    }
    Ok(Opened {
        guard,
        catalog,
        catalog_text,
        directories,
        units,
        siblings,
        previous,
        index,
        joint,
        problems,
    })
}

/// What the lock records for a unit read from the working tree while it
/// resolves with its siblings. No lockfile records it: a catalog's
/// lockfile holds what its units depend on in other repositories only.
fn working_provenance() -> Provenance {
    let Some(none) = ObjectId::parse(&"0".repeat(40)) else {
        unreachable!("forty hexadecimal digits are an object id");
    };
    Provenance::new(none.clone(), none)
}

/// The lock the units of `siblings` resolve to together under `previous`,
/// from `index`, without them, since they are the catalog's own, and how
/// it differs from `previous`.
pub(super) fn joint_lock(
    siblings: &Siblings,
    previous: Option<&UnitLock>,
    index: &RegistryIndex,
) -> Result<JointLock, Vec<BuildProblem>> {
    let previous = previous.cloned().unwrap_or_default();
    let index = &siblings.index(index);
    let requirements: crate::domain::Dependencies = siblings
        .units
        .iter()
        .map(|(identity, sibling)| {
            (
                identity.clone(),
                VersionRequirement::exactly(sibling.manifest.version()),
            )
        })
        .collect();
    let dependencies = Dependencies::read_in(index, None, &requirements, siblings)?;
    let resolution = resolve(
        None,
        &requirements,
        &dependencies.available,
        &previous.versions(),
    )
    .map_err(|errors| {
        errors
            .into_iter()
            .map(BuildProblem::Resolution)
            .collect::<Vec<_>>()
    })?;
    let (lock, changes) = update_lock(&previous, resolution.graph(), index).map_err(|errors| {
        errors
            .into_iter()
            .map(BuildProblem::Lock)
            .collect::<Vec<_>>()
    })?;
    let units: BTreeMap<UnitIdentity, LockedUnit> = lock
        .units()
        .iter()
        .filter(|(identity, _)| !siblings.contains(identity))
        .map(|(identity, locked)| (identity.clone(), locked.clone()))
        .collect();
    let changes = changes
        .into_iter()
        .filter(|(identity, _)| !siblings.contains(identity))
        .collect();
    Ok(JointLock {
        lock: UnitLock::new(units),
        changes,
    })
}
