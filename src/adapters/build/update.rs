//! `unit update`: an explicit change of the dependencies' locked versions,
//! with the unit's amendments carried to the providers' new versions (see
//! `docs/domain/authoring-workflow.md` and `docs/adr/0012-unit-update.md`).
//!
//! The command reads the unit as `unit build` does and resolves its
//! dependencies with the lockfile minus the units the author updates, all
//! of them when none is named. What changed, and whether every kept version
//! is still published with the content it was locked to, is computed
//! against the whole previous lockfile. Each of the unit's amendments is
//! then reconciled with its layered base: the base as the dependencies
//! compose it before the update, the copy, and the base after it are
//! merged three ways. A change below that the copy did not touch flows
//! through; a change to a value or to lines the copy changed too is left
//! in the copy between `current` and `incoming` markers, and reported. The
//! declaration's `base` follows the lock. A conflict between sibling
//! amenders of a file the unit does not amend yet is handed to the unit as
//! a build does, as a marked copy declared in the manifest. An amendment
//! whose base cannot be merged, and a published amendment whose provider
//! would not resolve to its base, refuse the update before anything is
//! written.
//!
//! An update replaces the copies it rebases or creates, `manifest.jsonc`,
//! and `assemblr-lock.json`, and must never leave them half updated. Before the
//! first is touched, a **journal** in the repository's Git directory records
//! both files' contents before and after the update; then each file is
//! replaced through a temporary file and a rename, and the journal is
//! removed last. While the journal exists, builds and amendments refuse to
//! run, and the next `unit update` first completes the interrupted one from
//! the journal: a file that still has its "before" content is replaced, one
//! that already has its "after" content is left, and one with any other
//! content stops the completion, since someone changed it meanwhile.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::amend::{self, MANIFEST_NEW};
use super::exclusive::{BuildLock, BuildLockError};
use super::output::LOCK_STAGING;
use super::{
    BuildProblem, Dependencies, MANIFEST, ManifestFileError, PendingFile, Resolved,
    development_values, layer_sources, pending_files, read_working_manifest_text, render_files,
    resolve_graph, without_amendments,
};
use crate::adapters::git::UnitDirectory;
use crate::adapters::git::{GitError, GitRepository, ReadSrcError};
use crate::adapters::layered::{has_markers, rebase_json, rebase_text};
use crate::adapters::lockfile::{self, LOCKFILE, LockfileError, serialize};
use crate::adapters::manifest::{EditError, declare_amendment, read_manifest, rebase_amendments};
use crate::adapters::registry::{Registry, StorageError};
use crate::domain::{
    AmendmentDeclaration, Composition, FileTree, LockChange, LockError, Manifest, MergeFormat,
    ReconcileCause, ReconcileError, RelativePath, ResolutionError, Root, UnitContent, UnitIdentity,
    UnitLock, Version, compose, delivered_path, is_text, resolve, update_lock,
};

/// The journal of an update that has started writing, in the Git directory.
pub(super) const JOURNAL: &str = "assemblr-update.json";
/// Where the journal is written before it is put in place.
const JOURNAL_NEW: &str = "assemblr-update.json.new";
/// The version of the journal's format.
const JOURNAL_FORMAT: u64 = 1;
/// The largest journal read: both files, twice, with room to spare.
const JOURNAL_LIMIT: u64 = 64 * 1024 * 1024;

/// Updates the locked versions of `units`, or of every dependency when
/// `units` is empty, of the unit whose repository's top level is `root`,
/// resolving from `registry`.
pub fn update_unit(
    root: &Path,
    registry: &Registry,
    units: &[UnitIdentity],
) -> Result<Updated, UpdateFailure> {
    update_with(root, registry, units, &mut |_| Ok(()))
}

/// The steps of writing, so that tests can make each one fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Journal,
    StageCopy,
    ReplaceCopy,
    StageManifest,
    ReplaceManifest,
    StageLock,
    ReplaceLock,
    RemoveJournal,
}

fn update_with(
    root: &Path,
    registry: &Registry,
    units: &[UnitIdentity],
    fault: &mut dyn FnMut(Step) -> io::Result<()>,
) -> Result<Updated, UpdateFailure> {
    let manifest_path = root.join(MANIFEST);
    if let Err(error) = fs::symlink_metadata(&manifest_path) {
        if error.kind() == io::ErrorKind::NotFound {
            return Err(UpdateFailure::Rejected(vec![UpdateProblem::Manifest(
                ManifestFileError::Missing {
                    path: manifest_path,
                },
            )]));
        }
    }
    let rejected = |problem| UpdateFailure::Rejected(vec![problem]);
    let repository =
        GitRepository::open(root).map_err(|error| rejected(UpdateProblem::Repository(error)))?;
    // The manifest and the lockfile are read and written under the lock
    // that serializes builds, so a build sees either the files before this
    // update or the files after it.
    let git_dir = repository
        .git_directory()
        .map_err(|error| rejected(UpdateProblem::Repository(error)))?;
    let _guard =
        BuildLock::acquire(&git_dir).map_err(|error| rejected(UpdateProblem::BuildLock(error)))?;
    let recovered =
        recover(root, &git_dir).map_err(|error| rejected(UpdateProblem::Recovery(error)))?;

    let mut problems = Vec::new();
    let text = read_working_manifest_text(root)
        .map_err(|error| problems.push(UpdateProblem::Manifest(error)))
        .ok();
    let manifest = text.as_ref().and_then(|text| {
        read_manifest(text)
            .map_err(|error| {
                problems.push(UpdateProblem::Manifest(ManifestFileError::Invalid {
                    path: manifest_path.clone(),
                    error,
                }))
            })
            .ok()
    });
    let previous = match lockfile::read_with_text(root) {
        Ok(Some(previous)) => Some(previous),
        Ok(None) => {
            problems.push(UpdateProblem::NoLockfile);
            None
        }
        Err(error) => {
            problems.push(UpdateProblem::Lockfile(error));
            None
        }
    };
    let index = registry
        .index()
        .map_err(|error| problems.push(UpdateProblem::Registry(error)))
        .ok();
    let (Some(text), Some(manifest), Some((lock_text, previous)), Some(index)) =
        (text, manifest, previous, index)
    else {
        return Err(UpdateFailure::Rejected(problems));
    };
    let dependencies = Dependencies::read(&index, &manifest).map_err(|found| {
        UpdateFailure::Rejected(found.into_iter().map(UpdateProblem::Build).collect())
    })?;

    // A named unit must be locked, or declared by the unit or by a locked
    // version: anything else is not a dependency of this unit.
    let selected: BTreeSet<&UnitIdentity> = units.iter().collect();
    let mut known: BTreeSet<&UnitIdentity> = previous.units().keys().collect();
    known.extend(manifest.dependencies().keys());
    for (identity, locked) in previous.units() {
        let key = (identity.clone(), locked.version().clone());
        if let Some(published) = dependencies.manifests.get(&key) {
            known.extend(published.dependencies().keys());
        }
    }
    for unit in &selected {
        if !known.contains(unit) {
            problems.push(UpdateProblem::NotADependency((*unit).clone()));
        }
    }
    if !problems.is_empty() {
        return Err(UpdateFailure::Rejected(problems));
    }

    // Resolution disregards the lock for the updated units, and keeps
    // precedence for the others.
    let mut lock = previous.versions();
    if selected.is_empty() {
        lock.clear();
    } else {
        lock.retain(|identity, _| !selected.contains(identity));
    }
    let resolution = resolve(
        Some(manifest.name()),
        manifest.dependencies(),
        &dependencies.available,
        &lock,
    )
    .map_err(|errors| {
        UpdateFailure::Rejected(errors.into_iter().map(UpdateProblem::Resolution).collect())
    })?;
    let graph = resolution.graph();
    let (new_lock, changes) = update_lock(&previous, graph, &index).map_err(|errors| {
        UpdateFailure::Rejected(errors.into_iter().map(UpdateProblem::Lock).collect())
    })?;

    // The unit's own copies, which a rebase reads and rewrites.
    let copies = if manifest.amendments().is_empty() {
        FileTree::new()
    } else {
        match repository.read_working_amendments() {
            Ok(copies) => copies,
            Err(error) => {
                problems.push(UpdateProblem::Copies(error));
                FileTree::new()
            }
        }
    };
    let src = match repository.read_working_src() {
        Ok(src) => src,
        Err(ReadSrcError::Git(error)) => {
            problems.push(UpdateProblem::Repository(error));
            FileTree::new()
        }
        Err(error) => {
            problems.push(UpdateProblem::Build(BuildProblem::Src(error)));
            FileTree::new()
        }
    };
    if !problems.is_empty() {
        return Err(UpdateFailure::Rejected(problems));
    }
    // The dependencies before the update and after it, each read and
    // verified as a build reads them, for the layered bases.
    let mut found = Vec::new();
    let before = resolve_graph(
        Some(manifest.name()),
        manifest.dependencies(),
        &previous,
        &index,
        &mut found,
    );
    let after = resolve_graph(
        Some(manifest.name()),
        manifest.dependencies(),
        &new_lock,
        &index,
        &mut found,
    );
    problems.extend(found.into_iter().map(UpdateProblem::Build));
    let (Some(before), Some(after)) = (before, after) else {
        return Err(UpdateFailure::Rejected(problems));
    };
    let (development, _) = development_values(&manifest);
    let bare = without_amendments(&manifest);
    let no_copies = FileTree::new();
    let (units_before, units_after) = (before.units(), after.units());
    let compose_world = |world: &Resolved, units| {
        compose(
            world.resolution.graph(),
            Root::Unit(UnitContent::new(&bare, &src, &no_copies)),
            units,
            &development,
        )
    };
    let composed_after = compose_world(&after, &units_after);
    let mut reconciled: BTreeMap<RelativePath, Reconciled> = BTreeMap::new();
    if !manifest.amendments().is_empty() {
        let composed_before = compose_world(&before, &units_before);
        match (&composed_before, &composed_after) {
            (Ok(composed_before), Ok(composed_after)) => {
                let worlds = (
                    World {
                        resolved: &before,
                        composition: composed_before,
                        lock: &previous,
                    },
                    World {
                        resolved: &after,
                        composition: composed_after,
                        lock: &new_lock,
                    },
                );
                match reconcile(&manifest, &previous, &worlds, &copies) {
                    Ok(carried) => reconciled = carried,
                    Err(errors) => problems.extend(errors),
                }
            }
            (Err(errors), _) | (_, Err(errors)) => problems.extend(
                errors
                    .iter()
                    .cloned()
                    .map(|error| UpdateProblem::Build(BuildProblem::Composition(error))),
            ),
        }
    }
    // A conflict between siblings in a file the unit does not amend, after
    // the update, is the unit's to resolve: it gets the marked copy as a
    // build would give it. What else the composition after the update
    // finds is the next build's to report.
    let mut pending = Vec::new();
    if let Ok(composed_after) = &composed_after {
        let mut sink = Vec::new();
        if let Some(rendered) = render_files(
            composed_after,
            &new_lock,
            Some(manifest.name()),
            &after.dependencies,
            &mut sink,
        ) {
            pending = pending_files(
                composed_after,
                rendered.unresolved,
                &new_lock,
                &manifest,
                &after.dependencies,
                &mut sink,
            )
            .0;
        }
    }
    if !problems.is_empty() {
        return Err(UpdateFailure::Rejected(problems));
    }
    let rebased_copies: BTreeMap<RelativePath, Vec<u8>> = reconciled
        .iter()
        .filter_map(|(path, rebased)| Some((path.clone(), rebased.copy.clone()?)))
        .collect();
    let carried: BTreeMap<RelativePath, Carried> = reconciled
        .into_iter()
        .filter_map(|(path, rebased)| {
            let declaration = manifest.amendments().get(&path)?;
            Some((
                path,
                Carried {
                    unit: declaration.unit().clone(),
                    from: declaration.base().clone(),
                    to: rebased.base,
                    rebased: rebased.copy.is_some(),
                    markers: rebased.copy.as_deref().is_some_and(has_markers),
                    conflicts: rebased.conflicts,
                },
            ))
        })
        .collect();
    let mut updated = Updated {
        recovered,
        changes,
        carried,
        pending,
        written: false,
    };
    if updated.changes.is_empty() && updated.carried.is_empty() && updated.pending.is_empty() {
        return Ok(updated);
    }

    // The plan: every file's new contents, decided before anything is
    // written. The manifest follows the carried bases and declares the
    // pending copies.
    let bases: BTreeMap<RelativePath, Version> = updated
        .carried
        .iter()
        .filter(|(_, carried)| carried.from != carried.to)
        .map(|(path, carried)| (path.clone(), carried.to.clone()))
        .collect();
    let mut edited = text.clone();
    let mut declared = manifest.clone();
    if !bases.is_empty() {
        edited = rebase_amendments(&edited, &declared, &bases)
            .map_err(|error| rejected(UpdateProblem::ManifestEdit(error)))?;
        declared = declared.with_bases(&bases).unwrap_or(declared);
    }
    for pending in &updated.pending {
        let declaration = AmendmentDeclaration::new(pending.provider.clone(), pending.base.clone());
        edited = declare_amendment(&edited, &declared, &pending.source, &declaration)
            .map_err(|error| rejected(UpdateProblem::ManifestEdit(error)))?;
        if let Some(next) = declared.with_amendment(pending.source.clone(), declaration) {
            declared = next;
        }
    }
    let edited = (edited != text).then_some(edited);
    let lock_after = serialize(&new_lock).map_err(|error| {
        UpdateFailure::Write(WriteError {
            operation: "serialize",
            path: root.join(LOCKFILE),
            error,
            state: UpdateState::Unchanged,
        })
    })?;
    let mut copy_plans = Vec::new();
    for (path, after) in rebased_copies {
        let before = copies
            .get(&path)
            .map(|file| file.contents().to_vec())
            .unwrap_or_default();
        copy_plans.push((path, Planned { before, after }));
    }
    // A pending copy is new: nothing exists at its path, and the
    // directories on the way are real directories, created here when they
    // do not exist and removed again if the update is not written.
    let mut created = Vec::new();
    match amend::real_directories_of(root, updated.pending.iter().map(|pending| &pending.source)) {
        Ok(missing) => created = missing,
        Err(amend::DirectoryProblem::NotADirectory(path)) => {
            problems.push(UpdateProblem::Build(BuildProblem::NotADirectory { path }));
        }
        Err(amend::DirectoryProblem::Unreadable(path, error)) => {
            problems.push(UpdateProblem::Build(BuildProblem::Unreadable {
                path,
                error,
            }));
        }
    }
    for pending in &updated.pending {
        let target = copy_target(root, &pending.source);
        if fs::symlink_metadata(&target).is_ok() {
            problems.push(UpdateProblem::StagingOccupied { path: target });
            continue;
        }
        copy_plans.push((
            pending.source.clone(),
            Planned {
                before: Vec::new(),
                after: pending.copy.clone(),
            },
        ));
    }
    let staging_names = [MANIFEST_NEW.to_owned(), LOCK_STAGING.to_owned()];
    let mut staged: Vec<PathBuf> = staging_names.iter().map(|name| root.join(name)).collect();
    staged.extend(copy_plans.iter().map(|(path, _)| copy_staging(root, path)));
    for path in staged {
        if fs::symlink_metadata(&path).is_ok() {
            problems.push(UpdateProblem::StagingOccupied { path });
        }
    }
    if !problems.is_empty() {
        return Err(UpdateFailure::Rejected(problems));
    }
    for directory in &created {
        if let Err(error) = fs::create_dir(directory) {
            amend::remove_directories(&created);
            return Err(UpdateFailure::Write(WriteError {
                operation: "create",
                path: directory.clone(),
                error,
                state: UpdateState::Unchanged,
            }));
        }
    }
    let journal = Journal {
        copies: copy_plans,
        manifest: edited.map(|after| Planned {
            before: text.into_bytes(),
            after: after.into_bytes(),
        }),
        lockfile: Planned {
            before: lock_text,
            after: lock_after,
        },
    };
    if let Err(error) = commit(root, &git_dir, &journal, fault) {
        if error.state == UpdateState::Unchanged {
            amend::remove_directories(&created);
        }
        return Err(UpdateFailure::Write(error));
    }
    updated.written = true;
    Ok(updated)
}

/// The dependencies on one side of the update, composed.
struct World<'w> {
    resolved: &'w Resolved,
    composition: &'w Composition<'w>,
    lock: &'w UnitLock,
}

/// An amendment carried across the update.
struct Reconciled {
    /// The new declared base: the provider's version after the update.
    base: Version,
    /// The rebased copy, when its base changed with the update; `None`
    /// when the copy is untouched.
    copy: Option<Vec<u8>>,
    /// Each conflict left marked in the copy, as the merge reports it.
    conflicts: Vec<String>,
}

/// Reconciles each amendment of `manifest` with its layered base before
/// and after the update, given `previous`, the lock before it, `worlds`,
/// the dependencies composed on each side, and `copies`, the unit's own
/// copies. An amendment whose base is the same on both sides is carried
/// over as it is, its declared base following the lock. One whose base
/// changes is rebased: the copy, the old base, and the new base are merged
/// three ways, and what both the copy and the update change is left
/// between markers. Every other case is an error, sorted by path: a
/// declared base that is not the locked version, which the author edited
/// by hand; a path the provider does not have at the base or no longer has
/// after the update; and a base that changes and cannot be merged.
fn reconcile(
    manifest: &Manifest,
    previous: &UnitLock,
    worlds: &(World<'_>, World<'_>),
    copies: &FileTree,
) -> Result<BTreeMap<RelativePath, Reconciled>, Vec<UpdateProblem>> {
    let (before, after) = worlds;
    let mut carried = BTreeMap::new();
    let mut problems = Vec::new();
    for (path, declaration) in manifest.amendments() {
        let unit = declaration.unit();
        let base = declaration.base();
        // A provider that leaves the graph is the next build's to report.
        let Some(new) = after.lock.units().get(unit).map(|locked| locked.version()) else {
            continue;
        };
        let moved = |cause| {
            UpdateProblem::Reconcile(ReconcileError::new(
                path.clone(),
                unit.clone(),
                base.clone(),
                new.clone(),
                cause,
            ))
        };
        if let Some(locked) = previous
            .units()
            .get(unit)
            .filter(|locked| locked.version() != base)
        {
            problems.push(moved(ReconcileCause::DeclarationChanged {
                locked: locked.version().clone(),
            }));
            continue;
        }
        let delivered = delivered_path(path)
            .ok()
            .flatten()
            .unwrap_or_else(|| path.clone());
        let mut found = Vec::new();
        let base_of = |world: &World<'_>, found: &mut Vec<BuildProblem>| {
            let composed = world.composition.files().get(&delivered)?;
            if composed.unit() != unit || composed.source() != path {
                return None;
            }
            layer_sources(
                world.composition,
                &delivered,
                world.lock,
                Some(manifest.name()),
                &world.resolved.dependencies,
                found,
            )
        };
        let old_base = base_of(before, &mut found);
        let new_base = base_of(after, &mut found);
        if !found.is_empty() {
            problems.extend(found.into_iter().map(UpdateProblem::Build));
            continue;
        }
        let (old_base, new_base) = match (old_base, new_base) {
            (None, _) => {
                problems.push(moved(ReconcileCause::NotProvidedAtBase));
                continue;
            }
            (Some(_), None) => {
                problems.push(moved(ReconcileCause::NoLongerProvided));
                continue;
            }
            (Some(old_base), Some(new_base)) => (old_base, new_base),
        };
        let untouched = Reconciled {
            base: new.clone(),
            copy: None,
            conflicts: Vec::new(),
        };
        if old_base == new_base {
            if new != base {
                carried.insert(path.clone(), untouched);
            }
            continue;
        }
        // A copy that does not exist is the next build's to report; the
        // base follows the lock meanwhile.
        let Some(copy) = copies.get(path) else {
            if new != base {
                carried.insert(path.clone(), untouched);
            }
            continue;
        };
        // A template's sources are rebased by line, whatever its format.
        let format = match MergeFormat::of_path(&delivered) {
            MergeFormat::Json if delivered != *path => MergeFormat::Text,
            format => format,
        };
        let texts = (
            std::str::from_utf8(&old_base),
            std::str::from_utf8(copy.contents()),
            std::str::from_utf8(&new_base),
        );
        let all_text = [old_base.as_slice(), copy.contents(), new_base.as_slice()]
            .into_iter()
            .all(is_text);
        let (Ok(old_text), Ok(copy_text), Ok(new_text)) = texts else {
            problems.push(moved(ReconcileCause::Changed));
            continue;
        };
        let rebased = match format {
            MergeFormat::Json => match rebase_json(old_text, copy_text, new_text) {
                Ok(rebased) => rebased,
                Err(failure) => {
                    problems.push(moved(ReconcileCause::Unmergeable(failure.to_string())));
                    continue;
                }
            },
            MergeFormat::Text if all_text => rebase_text(old_text, copy_text, new_text),
            MergeFormat::Text | MergeFormat::Unsupported => {
                problems.push(moved(ReconcileCause::Changed));
                continue;
            }
        };
        carried.insert(
            path.clone(),
            Reconciled {
                base: new.clone(),
                copy: Some(rebased.text.into_bytes()),
                conflicts: rebased.conflicts,
            },
        );
    }
    if problems.is_empty() {
        Ok(carried)
    } else {
        Err(problems)
    }
}

/// Applies `journal`: records it, replaces the manifest if the journal
/// plans it, replaces the lockfile, and removes the journal. A failure
/// after the journal is recorded leaves it, so that the next `unit update`
/// completes the update, and the error says what was replaced.
fn commit(
    root: &Path,
    git_dir: &Path,
    journal: &Journal,
    fault: &mut dyn FnMut(Step) -> io::Result<()>,
) -> Result<(), WriteError> {
    let journal_path = git_dir.join(JOURNAL);
    let staged_journal = git_dir.join(JOURNAL_NEW);
    let written = fault(Step::Journal)
        .and_then(|()| journal.serialize())
        .and_then(|bytes| write_synced(&staged_journal, &bytes, false))
        .and_then(|()| fs::rename(&staged_journal, &journal_path));
    if let Err(error) = written {
        let _ = fs::remove_file(&staged_journal);
        return Err(WriteError {
            operation: "record the update in",
            path: journal_path,
            error,
            state: UpdateState::Unchanged,
        });
    }
    let mut state = UpdateState::Interrupted {
        copies_replaced: 0,
        manifest_replaced: false,
        lockfile_replaced: false,
    };
    // The copies go first, then the manifest, then the lockfile.
    for (index, (path, planned)) in journal.copies.iter().enumerate() {
        let staged = copy_staging(root, path);
        let target = copy_target(root, path);
        if let Err(error) = fault(Step::StageCopy)
            .and_then(|()| write_synced(&staged, &planned.after, true))
            .and_then(|()| fault(Step::ReplaceCopy))
            .and_then(|()| fs::rename(&staged, &target))
        {
            let _ = fs::remove_file(&staged);
            return Err(WriteError {
                operation: "replace",
                path: target,
                error,
                state,
            });
        }
        state = UpdateState::Interrupted {
            copies_replaced: index + 1,
            manifest_replaced: false,
            lockfile_replaced: false,
        };
    }
    if let Some(planned) = &journal.manifest {
        let staged = root.join(MANIFEST_NEW);
        if let Err(error) = fault(Step::StageManifest)
            .and_then(|()| write_synced(&staged, &planned.after, true))
            .and_then(|()| fault(Step::ReplaceManifest))
            .and_then(|()| fs::rename(&staged, root.join(MANIFEST)))
        {
            let _ = fs::remove_file(&staged);
            return Err(WriteError {
                operation: "replace",
                path: root.join(MANIFEST),
                error,
                state,
            });
        }
        state = UpdateState::Interrupted {
            copies_replaced: journal.copies.len(),
            manifest_replaced: true,
            lockfile_replaced: false,
        };
    }
    let staged = root.join(LOCK_STAGING);
    if let Err(error) = fault(Step::StageLock)
        .and_then(|()| write_synced(&staged, &journal.lockfile.after, true))
        .and_then(|()| fault(Step::ReplaceLock))
        .and_then(|()| fs::rename(&staged, root.join(LOCKFILE)))
    {
        let _ = fs::remove_file(&staged);
        return Err(WriteError {
            operation: "replace",
            path: root.join(LOCKFILE),
            error,
            state,
        });
    }
    sync_directory(root);
    if let Err(error) = fault(Step::RemoveJournal).and_then(|()| fs::remove_file(&journal_path)) {
        return Err(WriteError {
            operation: "remove the record of the update at",
            path: journal_path,
            error,
            state: UpdateState::Interrupted {
                copies_replaced: journal.copies.len(),
                manifest_replaced: journal.manifest.is_some(),
                lockfile_replaced: true,
            },
        });
    }
    Ok(())
}

/// Where the copy at `amendments/<path>` is staged before it replaces the
/// file: beside it, under a reserved name.
fn copy_staging(root: &Path, path: &RelativePath) -> PathBuf {
    let target = copy_target(root, path);
    let mut name = target
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".assemblr-update-new");
    target.with_file_name(name)
}

/// The copy at `amendments/<path>`.
fn copy_target(root: &Path, path: &RelativePath) -> PathBuf {
    root.join(UnitDirectory::Amendments.as_str())
        .join(path.as_str())
}

/// Writes `bytes` to `path`, flushed to disk. With `new`, the file must not
/// exist, so that nothing is truncated.
fn write_synced(path: &Path, bytes: &[u8], new: bool) -> io::Result<()> {
    let mut file = if new {
        OpenOptions::new().write(true).create_new(true).open(path)?
    } else {
        File::create(path)?
    };
    file.write_all(bytes)?;
    file.sync_all()
}

/// Makes the renames in `directory` durable, best effort: they have already
/// happened.
fn sync_directory(directory: &Path) {
    #[cfg(unix)]
    let _ = File::open(directory).and_then(|dir| dir.sync_all());
    #[cfg(not(unix))]
    let _ = directory;
}

/// The journal of an update, if one was interrupted: `Some(path)`.
pub(super) fn interrupted(git_dir: &Path) -> io::Result<Option<PathBuf>> {
    let path = git_dir.join(JOURNAL);
    match fs::symlink_metadata(&path) {
        Ok(_) => Ok(Some(path)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Completes the update whose journal is in `git_dir`, if there is one,
/// and returns what that replaced.
fn recover(root: &Path, git_dir: &Path) -> Result<Option<Recovered>, RecoveryError> {
    let journal_path = git_dir.join(JOURNAL);
    let unreadable = |error| RecoveryError::Unreadable {
        path: journal_path.clone(),
        error,
    };
    let journal = match fs::symlink_metadata(&journal_path) {
        Ok(metadata) if metadata.is_file() => {
            Journal::read(&journal_path).map_err(|detail| RecoveryError::Invalid {
                path: journal_path.clone(),
                detail,
            })?
        }
        Ok(_) => {
            return Err(RecoveryError::Invalid {
                path: journal_path,
                detail: "it is not a regular file".to_owned(),
            });
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(unreadable(error)),
    };
    let state_of = |path: &Path, planned: &Planned| -> Result<FileState, RecoveryError> {
        let current = match fs::read(path) {
            Ok(current) => current,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                return Err(RecoveryError::Unreadable {
                    path: path.to_owned(),
                    error,
                });
            }
        };
        Ok(if current == planned.after {
            FileState::After
        } else if current == planned.before {
            FileState::Before
        } else {
            FileState::Modified
        })
    };
    // The update replaces the copies, then the manifest, then the lockfile,
    // so the files it can have left are some that hold the planned content
    // followed by some that still hold the old. A file whose planned content
    // is its current content reads as already replaced and is consistent
    // anywhere.
    let mut copy_states = Vec::new();
    for (path, planned) in &journal.copies {
        copy_states.push((path.clone(), state_of(&copy_target(root, path), planned)?));
    }
    let manifest = journal
        .manifest
        .as_ref()
        .map(|planned| state_of(&root.join(MANIFEST), planned))
        .transpose()?;
    let lockfile = state_of(&root.join(LOCKFILE), &journal.lockfile)?;
    let unchanged: Vec<bool> = journal
        .copies
        .iter()
        .map(|(_, planned)| planned)
        .chain(journal.manifest.as_ref())
        .chain(std::iter::once(&journal.lockfile))
        .map(|planned| planned.before == planned.after)
        .collect();
    let states: Vec<FileState> = copy_states
        .iter()
        .map(|(_, state)| *state)
        .chain(manifest)
        .chain(std::iter::once(lockfile))
        .collect();
    let mut seen_before = false;
    let mut consistent = true;
    for (state, unchanged) in states.iter().zip(&unchanged) {
        match state {
            FileState::Modified => consistent = false,
            FileState::Before => seen_before = true,
            FileState::After if seen_before && !unchanged => consistent = false,
            FileState::After => {}
        }
    }
    if !consistent {
        return Err(RecoveryError::Inconsistent {
            journal: journal_path,
            copies: copy_states,
            manifest,
            lockfile,
        });
    }
    let replace_copies: Vec<bool> = copy_states
        .iter()
        .map(|(_, state)| *state == FileState::Before)
        .collect();
    let replace_manifest = manifest == Some(FileState::Before);
    let replace_lock = lockfile == FileState::Before;
    let write = |operation, path: &Path, error| RecoveryError::Write {
        operation,
        path: path.to_owned(),
        error,
    };
    // Whatever the interrupted update staged is its own, and is replaced.
    let replace = |staged: &Path, target: &Path, bytes: &[u8]| -> Result<(), RecoveryError> {
        match fs::symlink_metadata(staged) {
            Ok(metadata) if metadata.is_file() => {
                fs::remove_file(staged).map_err(|error| write("delete", staged, error))?;
            }
            Ok(_) => {
                return Err(write(
                    "replace",
                    staged,
                    io::Error::other("it is not a regular file, so it is left as it is"),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(write("inspect", staged, error)),
        }
        write_synced(staged, bytes, true).map_err(|error| write("write", staged, error))?;
        fs::rename(staged, target).map_err(|error| write("replace", target, error))
    };
    for ((path, planned), replace_it) in journal.copies.iter().zip(&replace_copies) {
        if *replace_it {
            replace(
                &copy_staging(root, path),
                &copy_target(root, path),
                &planned.after,
            )?;
        }
    }
    if replace_manifest {
        if let Some(planned) = &journal.manifest {
            replace(
                &root.join(MANIFEST_NEW),
                &root.join(MANIFEST),
                &planned.after,
            )?;
        }
    }
    if replace_lock {
        replace(
            &root.join(LOCK_STAGING),
            &root.join(LOCKFILE),
            &journal.lockfile.after,
        )?;
    }
    sync_directory(root);
    let leftovers = [root.join(MANIFEST_NEW), root.join(LOCK_STAGING)]
        .into_iter()
        .chain(
            journal
                .copies
                .iter()
                .map(|(path, _)| copy_staging(root, path)),
        );
    for staged in leftovers {
        match fs::symlink_metadata(&staged) {
            Ok(metadata) if metadata.is_file() => {
                fs::remove_file(&staged).map_err(|error| write("delete", &staged, error))?;
            }
            _ => {}
        }
    }
    fs::remove_file(&journal_path).map_err(|error| write("remove", &journal_path, error))?;
    Ok(Some(Recovered {
        copies: replace_copies.iter().filter(|replaced| **replaced).count(),
        manifest: replace_manifest,
        lockfile: replace_lock,
    }))
}

/// What an update writes: both files' contents before and after it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Journal {
    /// The copies under `amendments/` the update rewrites, by path.
    copies: Vec<(RelativePath, Planned)>,
    /// The manifest, when the update changes it.
    manifest: Option<Planned>,
    lockfile: Planned,
}

/// A file's contents before and after the update.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Planned {
    before: Vec<u8>,
    after: Vec<u8>,
}

impl Journal {
    fn serialize(&self) -> io::Result<Vec<u8>> {
        let planned = |planned: &Planned| -> io::Result<Value> {
            let text = |bytes: &[u8]| {
                String::from_utf8(bytes.to_vec())
                    .map_err(|_| io::Error::other("the file is not UTF-8"))
            };
            Ok(json!({ "before": text(&planned.before)?, "after": text(&planned.after)? }))
        };
        let manifest = match &self.manifest {
            Some(manifest) => planned(manifest)?,
            None => Value::Null,
        };
        let mut copies = Map::new();
        for (path, copy) in &self.copies {
            copies.insert(path.to_string(), planned(copy)?);
        }
        let mut document = json!({
            "format": JOURNAL_FORMAT,
            "manifest": manifest,
            "lockfile": planned(&self.lockfile)?,
        });
        // Absent when there are none, so that the journal of an update
        // that rewrites no copy is the journal of earlier versions.
        if !copies.is_empty() {
            document["copies"] = Value::Object(copies);
        }
        let mut bytes = serde_json::to_vec_pretty(&document)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    /// Reads the journal at `path`, or describes what is wrong with it.
    fn read(path: &Path) -> Result<Self, String> {
        let mut bytes = Vec::new();
        File::open(path)
            .and_then(|file| {
                use std::io::Read;
                file.take(JOURNAL_LIMIT + 1).read_to_end(&mut bytes)
            })
            .map_err(|error| error.to_string())?;
        if bytes.len() as u64 > JOURNAL_LIMIT {
            return Err(format!(
                "it is larger than the {JOURNAL_LIMIT} bytes Assemblr reads"
            ));
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        let root = object(
            &value,
            "the journal",
            &["format", "manifest", "lockfile", "copies"],
        )?;
        match root.get("format").and_then(Value::as_u64) {
            Some(JOURNAL_FORMAT) => {}
            Some(other) => {
                return Err(format!(
                    "it has format {other}, which this version of Assemblr does not read"
                ));
            }
            None => return Err("it has no format number".to_owned()),
        }
        let planned = |value: &Value, context: &str| -> Result<Planned, String> {
            let object = object(value, context, &["before", "after"])?;
            let text = |name: &str| {
                object
                    .get(name)
                    .and_then(Value::as_str)
                    .map(|text| text.as_bytes().to_vec())
                    .ok_or_else(|| format!("{context} has no `{name}` string"))
            };
            Ok(Planned {
                before: text("before")?,
                after: text("after")?,
            })
        };
        let manifest = match root.get("manifest") {
            None | Some(Value::Null) => None,
            Some(value) => Some(planned(value, "`manifest`")?),
        };
        let lockfile = root
            .get("lockfile")
            .ok_or_else(|| "it has no `lockfile`".to_owned())
            .and_then(|value| planned(value, "`lockfile`"))?;
        let mut copies = Vec::new();
        if let Some(value) = root.get("copies") {
            let members = value
                .as_object()
                .ok_or_else(|| "`copies` is not a JSON object".to_owned())?;
            for (name, value) in members {
                let path = RelativePath::parse(name)
                    .map_err(|error| format!("`copies` names {name:?}: {error}"))?;
                copies.push((path, planned(value, &format!("`copies` entry {name:?}"))?));
            }
            copies.sort_by(|left, right| left.0.cmp(&right.0));
        }
        Ok(Self {
            copies,
            manifest,
            lockfile,
        })
    }
}

/// `value` as an object with no keys beyond `fields`.
fn object<'a>(
    value: &'a Value,
    context: &str,
    fields: &[&str],
) -> Result<&'a Map<String, Value>, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{context} is not a JSON object"))?;
    if let Some(key) = object.keys().find(|key| !fields.contains(&key.as_str())) {
        return Err(format!("{context} has an unexpected `{key}`"));
    }
    Ok(object)
}

/// A successful update.
#[derive(Debug)]
pub struct Updated {
    recovered: Option<Recovered>,
    changes: BTreeMap<UnitIdentity, LockChange>,
    carried: BTreeMap<RelativePath, Carried>,
    pending: Vec<PendingFile>,
    written: bool,
}

impl Updated {
    /// The update interrupted earlier that this one completed first, if
    /// there was one.
    pub fn recovered(&self) -> Option<&Recovered> {
        self.recovered.as_ref()
    }

    /// How the lockfile changed, by identity: empty when nothing changed.
    pub fn changes(&self) -> &BTreeMap<UnitIdentity, LockChange> {
        &self.changes
    }

    /// The amendments carried across the update, by path: those whose
    /// provider moved, and those whose base changed below.
    pub fn carried(&self) -> &BTreeMap<RelativePath, Carried> {
        &self.carried
    }

    /// The copies the update created for the unit to resolve a conflict
    /// between sibling amenders, in path order.
    pub fn pending(&self) -> &[PendingFile] {
        &self.pending
    }

    /// Whether any copy written holds conflict markers: one the update
    /// created, or one it rebased with a conflict.
    pub fn has_markers(&self) -> bool {
        !self.pending.is_empty() || self.carried.values().any(|carried| carried.markers)
    }

    /// Whether the lockfile, and the manifest and the copies when there
    /// was something to carry or to create, were written. False when there
    /// was nothing to update.
    pub fn written(&self) -> bool {
        self.written
    }
}

/// An amendment carried across the update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Carried {
    unit: UnitIdentity,
    from: Version,
    to: Version,
    rebased: bool,
    markers: bool,
    conflicts: Vec<String>,
}

impl Carried {
    /// The provider.
    pub fn unit(&self) -> &UnitIdentity {
        &self.unit
    }

    /// The base before the update.
    pub fn from(&self) -> &Version {
        &self.from
    }

    /// The base after it.
    pub fn to(&self) -> &Version {
        &self.to
    }

    /// Whether the copy was rewritten to follow its base's changes, and
    /// not only carried over unchanged.
    pub fn rebased(&self) -> bool {
        self.rebased
    }

    /// Whether the rewritten copy holds conflict markers: those of the
    /// conflicts below, or markers that flowed in from the base.
    pub fn has_markers(&self) -> bool {
        self.markers
    }

    /// Each conflict between the copy and the update left marked in the
    /// copy, as the merge reports it.
    pub fn conflicts(&self) -> &[String] {
        &self.conflicts
    }
}

/// What completing an interrupted update replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recovered {
    copies: usize,
    manifest: bool,
    lockfile: bool,
}

impl Recovered {
    /// How many copies under `amendments/` were replaced now.
    pub fn copies(&self) -> usize {
        self.copies
    }

    /// Whether `manifest.jsonc` was replaced now.
    pub fn manifest(&self) -> bool {
        self.manifest
    }

    /// Whether `assemblr-lock.json` was replaced now.
    pub fn lockfile(&self) -> bool {
        self.lockfile
    }
}

/// Why `unit update` failed.
#[derive(Debug)]
pub enum UpdateFailure {
    /// The update was rejected before anything was written.
    Rejected(Vec<UpdateProblem>),
    /// A write failed.
    Write(WriteError),
}

impl fmt::Display for UpdateFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(problems) => {
                for (i, problem) in problems.iter().enumerate() {
                    if i > 0 {
                        f.write_str("\n")?;
                    }
                    problem.fmt(f)?;
                }
                Ok(())
            }
            Self::Write(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for UpdateFailure {}

/// A write that failed, and what it left.
#[derive(Debug)]
pub struct WriteError {
    operation: &'static str,
    path: PathBuf,
    error: io::Error,
    state: UpdateState,
}

impl WriteError {
    /// What remains.
    pub fn state(&self) -> UpdateState {
        self.state
    }
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "could not {} {}: {}",
            self.operation,
            self.path.display(),
            self.error
        )
    }
}

impl std::error::Error for WriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// What a failed update left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateState {
    /// Nothing: the manifest and the lockfile are as they were.
    Unchanged,
    /// The update is recorded, and the next `unit update` completes it.
    Interrupted {
        /// How many copies under `amendments/` already hold the updated
        /// content, in path order.
        copies_replaced: usize,
        /// Whether `manifest.jsonc` already holds the updated content.
        manifest_replaced: bool,
        /// Whether `assemblr-lock.json` already holds the updated content.
        lockfile_replaced: bool,
    },
}

/// A problem that rejects `unit update`.
#[derive(Debug)]
pub enum UpdateProblem {
    /// The working tree's `manifest.jsonc` cannot be used.
    Manifest(ManifestFileError),
    /// The unit's repository could not be read.
    Repository(GitError),
    /// The lock that keeps builds, amendments, and updates of the unit from
    /// running at the same time could not be taken.
    BuildLock(BuildLockError),
    /// An update interrupted earlier could not be completed.
    Recovery(RecoveryError),
    /// The lockfile cannot be read.
    Lockfile(LockfileError),
    /// There is no lockfile, so there are no locked versions to update.
    NoLockfile,
    /// The registry index cannot be read.
    Registry(StorageError),
    /// A dependency could not be read as a build reads it.
    Build(BuildProblem),
    /// A named unit is neither locked nor declared as a dependency.
    NotADependency(UnitIdentity),
    /// The dependencies cannot be resolved.
    Resolution(ResolutionError),
    /// A kept version contradicts the lockfile.
    Lock(LockError),
    /// An amendment of the unit cannot be carried to the provider's new
    /// version.
    Reconcile(ReconcileError),
    /// The manifest cannot be edited safely.
    ManifestEdit(EditError),
    /// The unit's copies under `amendments/` cannot be read.
    Copies(crate::adapters::git::ReadSrcError),
    /// Something exists at a reserved path the update writes to.
    StagingOccupied {
        /// The path.
        path: PathBuf,
    },
}

impl fmt::Display for UpdateProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest(error) => error.fmt(f),
            Self::Repository(error) => error.fmt(f),
            Self::BuildLock(error) => error.fmt(f),
            Self::Recovery(error) => error.fmt(f),
            Self::Lockfile(error) => error.fmt(f),
            Self::NoLockfile => f.write_str(
                "there is no assemblr-lock.json, so there are no locked versions to update",
            ),
            Self::Registry(error) => error.fmt(f),
            Self::Build(problem) => problem.fmt(f),
            Self::NotADependency(unit) => write!(
                f,
                "{unit} is not a dependency of this unit: it is neither locked nor declared by \
                 the unit or by a locked version"
            ),
            Self::Resolution(error) => error.fmt(f),
            Self::Lock(error) => error.fmt(f),
            Self::Reconcile(error) => error.fmt(f),
            Self::ManifestEdit(error) => error.fmt(f),
            Self::Copies(error) => write!(f, "the unit's amendments cannot be read: {error}"),
            Self::StagingOccupied { path } => write!(
                f,
                "{} exists, and an update writes there; move it away first",
                path.display()
            ),
        }
    }
}

impl std::error::Error for UpdateProblem {}

/// Why an interrupted update could not be completed.
#[derive(Debug)]
pub enum RecoveryError {
    /// The journal or one of the files cannot be read.
    Unreadable {
        /// The file.
        path: PathBuf,
        /// Why.
        error: io::Error,
    },
    /// The journal is not one Assemblr wrote.
    Invalid {
        /// The journal.
        path: PathBuf,
        /// What is wrong.
        detail: String,
    },
    /// The files are not in a state the interrupted update could have left,
    /// so someone changed them since.
    Inconsistent {
        /// The journal.
        journal: PathBuf,
        /// The state of each copy the update planned to replace.
        copies: Vec<(RelativePath, FileState)>,
        /// The manifest's state, when the update planned to replace it.
        manifest: Option<FileState>,
        /// The lockfile's state.
        lockfile: FileState,
    },
    /// Replacing a file failed. The journal remains.
    Write {
        /// What failed.
        operation: &'static str,
        /// The file.
        path: PathBuf,
        /// Why.
        error: io::Error,
    },
}

/// How a file compares with what an interrupted update recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    /// As before the update.
    Before,
    /// As the update planned to leave it.
    After,
    /// Neither.
    Modified,
}

impl fmt::Display for FileState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Before => "as before the update",
            Self::After => "as the update planned",
            Self::Modified => "neither as before the update nor as it planned",
        })
    }
}

impl fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { path, error } => write!(
                f,
                "an update was interrupted, and could not be completed: could not read {}: \
                 {error}",
                path.display()
            ),
            Self::Invalid { path, detail } => write!(
                f,
                "an update was interrupted, and its record at {} cannot be used: {detail}",
                path.display()
            ),
            Self::Inconsistent {
                journal,
                copies,
                manifest,
                lockfile,
            } => {
                write!(f, "an update was interrupted, and could not be completed: ")?;
                for (path, state) in copies {
                    write!(f, "amendments/{path} is {state}, and ")?;
                }
                if let Some(manifest) = manifest {
                    write!(f, "manifest.jsonc is {manifest}, and ")?;
                }
                write!(
                    f,
                    "assemblr-lock.json is {lockfile}; a file changed since the update started \
                     is never overwritten. Restore the file to the content the record at {} \
                     holds under `before` or `after`, then run `assemblr unit update` again; \
                     or delete the record to abandon the update, leaving the files as they are",
                    journal.display()
                )
            }
            Self::Write {
                operation,
                path,
                error,
            } => write!(
                f,
                "an update was interrupted, and completing it failed: could not {operation} {}: \
                 {error}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for RecoveryError {}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "tests fail on a bad fixture"
    )]

    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};

    use super::{
        FileState, JOURNAL, Journal, Planned, RecoveryError, Step, UpdateState, commit, recover,
    };
    use crate::adapters::build::amend::MANIFEST_NEW;
    use crate::adapters::build::output::LOCK_STAGING;

    /// An empty directory for one test, with a Git directory beside it.
    fn scratch(test: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir()
            .join(format!("assemblr-update-{}", std::process::id()))
            .join(test);
        if dir.exists() {
            fs::remove_dir_all(&dir).unwrap();
        }
        let root = dir.join("unit");
        let git_dir = dir.join("git");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(root.join("manifest.jsonc"), "before manifest").unwrap();
        fs::write(root.join("assemblr-lock.json"), "before lock").unwrap();
        (root, git_dir)
    }

    fn journal(manifest: bool) -> Journal {
        Journal {
            copies: Vec::new(),
            manifest: manifest.then(|| Planned {
                before: b"before manifest".to_vec(),
                after: b"after manifest".to_vec(),
            }),
            lockfile: Planned {
                before: b"before lock".to_vec(),
                after: b"after lock".to_vec(),
            },
        }
    }

    fn contents(root: &Path) -> (String, String) {
        (
            fs::read_to_string(root.join("manifest.jsonc")).unwrap(),
            fs::read_to_string(root.join("assemblr-lock.json")).unwrap(),
        )
    }

    fn commit_failing_at(root: &Path, git_dir: &Path, failing: Step) -> UpdateState {
        let error = commit(root, git_dir, &journal(true), &mut |step| {
            if step == failing {
                Err(io::Error::other(format!("injected failure at {step:?}")))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        error.state()
    }

    #[test]
    fn a_journal_round_trips() {
        for manifest in [true, false] {
            let journal = journal(manifest);
            let (root, _) = scratch(&format!("round-trip-{manifest}"));
            let path = root.join(JOURNAL);
            fs::write(&path, journal.serialize().unwrap()).unwrap();
            assert_eq!(Journal::read(&path).unwrap(), journal);
        }
    }

    #[test]
    fn an_invalid_journal_is_described() {
        let (root, _) = scratch("invalid-journal");
        let path = root.join(JOURNAL);
        for (text, detail) in [
            ("{", "EOF"),
            (
                r#"{"format": 2, "lockfile": {"before": "", "after": ""}}"#,
                "format 2",
            ),
            (r#"{"lockfile": {"before": "", "after": ""}}"#, "no format"),
            (r#"{"format": 1}"#, "no `lockfile`"),
            (r#"{"format": 1, "lockfile": {"before": ""}}"#, "no `after`"),
            (
                r#"{"format": 1, "lockfile": {"before": "", "after": ""}, "extra": 1}"#,
                "unexpected `extra`",
            ),
            (
                r#"{"format": 1, "lockfile": {"before": "", "after": ""},
                    "copies": {"a.md": {"before": "", "after": "", "mode": 1}}}"#,
                "unexpected `mode`",
            ),
        ] {
            fs::write(&path, text).unwrap();
            let error = Journal::read(&path).unwrap_err();
            assert!(error.contains(detail), "{text}: {error}");
        }
    }

    #[test]
    fn a_failure_before_the_journal_changes_nothing() {
        let (root, git_dir) = scratch("fail-journal");
        assert_eq!(
            commit_failing_at(&root, &git_dir, Step::Journal),
            UpdateState::Unchanged
        );
        assert!(!git_dir.join(JOURNAL).exists());
        assert!(!git_dir.join("assemblr-update.json.new").exists());
        assert_eq!(
            contents(&root),
            ("before manifest".to_owned(), "before lock".to_owned())
        );
        assert!(recover(&root, &git_dir).unwrap().is_none());
    }

    #[test]
    fn a_failure_at_each_later_step_is_completed_by_recovery() {
        for (step, manifest_replaced, lockfile_replaced) in [
            (Step::StageManifest, false, false),
            (Step::ReplaceManifest, false, false),
            (Step::StageLock, true, false),
            (Step::ReplaceLock, true, false),
            (Step::RemoveJournal, true, true),
        ] {
            let (root, git_dir) = scratch(&format!("fail-{step:?}"));
            assert_eq!(
                commit_failing_at(&root, &git_dir, step),
                UpdateState::Interrupted {
                    copies_replaced: 0,
                    manifest_replaced,
                    lockfile_replaced,
                },
                "{step:?}"
            );
            assert!(git_dir.join(JOURNAL).exists(), "{step:?}");
            let (manifest, lock) = contents(&root);
            assert_eq!(
                manifest,
                if manifest_replaced {
                    "after manifest"
                } else {
                    "before manifest"
                },
                "{step:?}"
            );
            assert_eq!(
                lock,
                if lockfile_replaced {
                    "after lock"
                } else {
                    "before lock"
                },
                "{step:?}"
            );
            assert!(!root.join(MANIFEST_NEW).exists(), "{step:?}");
            assert!(!root.join(LOCK_STAGING).exists(), "{step:?}");

            let recovered = recover(&root, &git_dir).unwrap().unwrap();
            assert_eq!(recovered.manifest(), !manifest_replaced, "{step:?}");
            assert_eq!(recovered.lockfile(), !lockfile_replaced, "{step:?}");
            assert_eq!(
                contents(&root),
                ("after manifest".to_owned(), "after lock".to_owned()),
                "{step:?}"
            );
            assert!(!git_dir.join(JOURNAL).exists(), "{step:?}");
            assert!(recover(&root, &git_dir).unwrap().is_none(), "{step:?}");
        }
    }

    #[test]
    fn an_update_without_a_manifest_change_is_completed_too() {
        let (root, git_dir) = scratch("lock-only");
        fs::write(git_dir.join(JOURNAL), journal(false).serialize().unwrap()).unwrap();
        // Stale staging from the interruption is the update's own.
        fs::write(root.join(LOCK_STAGING), "half").unwrap();

        let recovered = recover(&root, &git_dir).unwrap().unwrap();

        assert!(!recovered.manifest());
        assert!(recovered.lockfile());
        assert_eq!(
            contents(&root),
            ("before manifest".to_owned(), "after lock".to_owned())
        );
        assert!(!root.join(LOCK_STAGING).exists());
        assert!(!git_dir.join(JOURNAL).exists());
    }

    #[test]
    fn a_plan_that_leaves_the_lockfile_as_it_is_is_completed() {
        let (root, git_dir) = scratch("lock-unchanged");
        let mut journal = journal(true);
        journal.lockfile.after = journal.lockfile.before.clone();
        fs::write(git_dir.join(JOURNAL), journal.serialize().unwrap()).unwrap();

        let recovered = recover(&root, &git_dir).unwrap().unwrap();

        assert!(recovered.manifest());
        assert!(!recovered.lockfile());
        assert_eq!(
            contents(&root),
            ("after manifest".to_owned(), "before lock".to_owned())
        );
        assert!(!git_dir.join(JOURNAL).exists());
    }

    #[test]
    fn a_file_changed_since_the_update_started_is_never_overwritten() {
        for (manifest, lock, expected_manifest, expected_lock) in [
            (
                "edited",
                "before lock",
                Some(FileState::Modified),
                FileState::Before,
            ),
            (
                "before manifest",
                "after lock",
                Some(FileState::Before),
                FileState::After,
            ),
            (
                "after manifest",
                "tampered",
                Some(FileState::After),
                FileState::Modified,
            ),
        ] {
            let (root, git_dir) = scratch(&format!("inconsistent-{manifest}-{lock}"));
            fs::write(root.join("manifest.jsonc"), manifest).unwrap();
            fs::write(root.join("assemblr-lock.json"), lock).unwrap();
            fs::write(git_dir.join(JOURNAL), journal(true).serialize().unwrap()).unwrap();

            let error = recover(&root, &git_dir).unwrap_err();

            match error {
                RecoveryError::Inconsistent {
                    manifest: found_manifest,
                    lockfile,
                    ..
                } => {
                    assert_eq!(found_manifest, expected_manifest, "{manifest} / {lock}");
                    assert_eq!(lockfile, expected_lock, "{manifest} / {lock}");
                }
                other => panic!("{other:?}"),
            }
            assert_eq!(contents(&root), (manifest.to_owned(), lock.to_owned()));
            assert!(git_dir.join(JOURNAL).exists());
        }
    }

    #[test]
    fn an_unusable_journal_is_reported_and_kept() {
        let (root, git_dir) = scratch("unusable-journal");
        fs::write(git_dir.join(JOURNAL), "{").unwrap();
        let error = recover(&root, &git_dir).unwrap_err();
        assert!(matches!(error, RecoveryError::Invalid { .. }), "{error:?}");
        assert!(git_dir.join(JOURNAL).exists());
        assert_eq!(
            contents(&root),
            ("before manifest".to_owned(), "before lock".to_owned())
        );
    }

    fn journal_with_copy() -> Journal {
        Journal {
            copies: vec![(
                crate::domain::RelativePath::parse("NOTES.md").unwrap(),
                Planned {
                    before: b"before copy".to_vec(),
                    after: b"after copy".to_vec(),
                },
            )],
            ..journal(true)
        }
    }

    fn scratch_with_copy(test: &str) -> (PathBuf, PathBuf) {
        let (root, git_dir) = scratch(test);
        fs::create_dir_all(root.join("amendments")).unwrap();
        fs::write(root.join("amendments/NOTES.md"), "before copy").unwrap();
        (root, git_dir)
    }

    #[test]
    fn a_failure_at_each_step_of_an_update_that_rewrites_a_copy_is_completed_by_recovery() {
        for failing in [
            Step::Journal,
            Step::StageCopy,
            Step::ReplaceCopy,
            Step::StageManifest,
            Step::ReplaceManifest,
            Step::StageLock,
            Step::ReplaceLock,
            Step::RemoveJournal,
        ] {
            let (root, git_dir) = scratch_with_copy(&format!("copy-{failing:?}"));
            let result = commit(&root, &git_dir, &journal_with_copy(), &mut |step| {
                if step == failing {
                    Err(io::Error::other("injected"))
                } else {
                    Ok(())
                }
            });
            assert!(result.is_err(), "{failing:?}");
            if failing == Step::Journal {
                // Nothing was recorded or written.
                assert!(!git_dir.join(JOURNAL).exists());
                assert_eq!(
                    fs::read_to_string(root.join("amendments/NOTES.md")).unwrap(),
                    "before copy"
                );
                continue;
            }
            assert!(git_dir.join(JOURNAL).exists(), "{failing:?}");
            let recovered = recover(&root, &git_dir).unwrap();
            assert!(recovered.is_some(), "{failing:?}");
            assert!(!git_dir.join(JOURNAL).exists(), "{failing:?}");
            assert_eq!(
                fs::read_to_string(root.join("amendments/NOTES.md")).unwrap(),
                "after copy",
                "{failing:?}"
            );
            assert_eq!(
                contents(&root),
                ("after manifest".to_owned(), "after lock".to_owned()),
                "{failing:?}"
            );
            // Nothing staged is left behind.
            let leftovers: Vec<_> = fs::read_dir(root.join("amendments"))
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            assert_eq!(leftovers.len(), 1, "{failing:?}: {leftovers:?}");
        }
    }

    #[test]
    fn a_copy_edited_since_the_update_started_stops_its_recovery() {
        let (root, git_dir) = scratch_with_copy("copy-edited");
        let result = commit(&root, &git_dir, &journal_with_copy(), &mut |step| {
            if step == Step::StageManifest {
                Err(io::Error::other("injected"))
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        // The copy was replaced; then the author edited it.
        fs::write(root.join("amendments/NOTES.md"), "edited meanwhile").unwrap();
        let error = recover(&root, &git_dir).unwrap_err();
        assert!(
            matches!(&error, RecoveryError::Inconsistent { copies, .. }
                if copies[0].1 == FileState::Modified),
            "{error}"
        );
        assert!(
            error
                .to_string()
                .contains("amendments/NOTES.md is neither as before"),
            "{error}"
        );
        assert!(git_dir.join(JOURNAL).exists());
        assert_eq!(
            fs::read_to_string(root.join("amendments/NOTES.md")).unwrap(),
            "edited meanwhile"
        );
    }

    #[test]
    fn the_journal_of_an_update_that_rewrites_no_copy_has_no_copies_member() {
        let bytes = journal(true).serialize().unwrap();
        assert!(!String::from_utf8(bytes).unwrap().contains("copies"));
        let with = journal_with_copy().serialize().unwrap();
        let dir = std::env::temp_dir().join(format!("assemblr-journal-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("journal.json");
        fs::write(&path, with).unwrap();
        assert_eq!(Journal::read(&path).unwrap(), journal_with_copy());
    }
}
