//! What `assemblr status` reports about a consumer project, read-only and
//! offline: where its composition stands, its units, the files edited since
//! the last composition, and which units deliver each file (see
//! `docs/domain/consumer-projects.md`, **Provisional (D-46)**).
//!
//! Nothing is written and nothing is locked: the report is what the
//! project, its state directory, and the local registry hold when it is
//! read. What composing would do is planned exactly as `compose` plans it,
//! from the lock in effect: the lockfile, or the one a composition waiting
//! for its conflicts keeps.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::lockfile::{self, LOCKFILE, Lockfile};
use super::store::Store;
use super::{
    ComposeProblem, DECLARATION, MANIFEST, Origin, Planned, plan, read_current, read_text,
    snapshot_of,
};
use crate::adapters::build::{BuildProblem, newest_allowed};
use crate::adapters::manifest::read_project;
use crate::adapters::registry::Registry;
use crate::domain::{
    Current, ReconciliationState, RelativePath, Snapshot, UnitIdentity, Version, VersionRequirement,
};

/// Reports on the consumer project at `root` against `registry`'s index.
/// What keeps the project's own state from being read is an error: a
/// directory that is not a project, an unreadable declaration or
/// lockfile, or a journal that cannot be read. What keeps a composition
/// from being planned is reported in the status instead, with everything
/// else that could be read.
pub fn project_status(root: &Path, registry: &Registry) -> Result<ProjectStatus, ComposeProblem> {
    if root.join(MANIFEST).exists() {
        return Err(ComposeProblem::UnitRepository(root.to_owned()));
    }
    let declaration_path = root.join(DECLARATION);
    if std::fs::symlink_metadata(&declaration_path).is_err() {
        return Err(ComposeProblem::NoDeclaration(declaration_path));
    }
    let declaration = read_text(&declaration_path).map_err(|error| ComposeProblem::Io {
        path: declaration_path.clone(),
        error,
    })?;
    let lockfile = match lockfile::read_lockfile(root).map_err(ComposeProblem::Lockfile)? {
        None => None,
        Some((_, Lockfile { project: None, .. })) => {
            return Err(ComposeProblem::ForeignLockfile(root.join(LOCKFILE)));
        }
        Some((
            _,
            Lockfile {
                lock,
                project: Some(state),
            },
        )) => Some((lock, state)),
    };
    let store = Store::new(root);
    let journal = store.journal().map_err(ComposeProblem::Store)?;
    let completed = journal
        .as_ref()
        .is_some_and(|journal| super::pending::lock_records(root, journal));
    let mut problems = Vec::new();

    // The state, and the lock in effect: a composition waiting for its
    // conflicts keeps the lock it will write.
    let (state, effective) = match journal.filter(|_| !completed) {
        None => match &lockfile {
            None => (CompositionState::Uncomposed, None),
            Some(locked) => (CompositionState::Composed, Some(locked.clone())),
        },
        Some(journal) => match &journal.pending {
            None => (CompositionState::Interrupted, lockfile.clone()),
            Some(pending) => {
                let mut files = Vec::new();
                for path in &pending.files {
                    files.push(MarkedFile {
                        path: path.clone(),
                        marked: super::pending::still_marked(root, path)?,
                    });
                }
                let kept = store
                    .read_previous(&pending.lock)
                    .map_err(ComposeProblem::Store)?;
                let effective = match lockfile::parse(&kept) {
                    Ok(Lockfile {
                        lock,
                        project: Some(state),
                    }) => Some((lock, state)),
                    _ => {
                        return Err(ComposeProblem::Store(super::StoreError::Corrupt {
                            path: root.join(super::STATE_DIR),
                            detail: "the lock kept for the pending composition is not a \
                                     project's"
                                .to_owned(),
                        }));
                    }
                };
                (CompositionState::Pending(files), effective)
            }
        },
    };
    let lock = effective
        .as_ref()
        .map(|(lock, _)| lock.clone())
        .unwrap_or_default();

    let project = read_project(&declaration)
        .map_err(|error| {
            problems.push(ComposeProblem::InvalidDeclaration {
                path: declaration_path.clone(),
                error,
            });
        })
        .ok();
    let index = registry
        .index()
        .map_err(|error| {
            problems.push(ComposeProblem::Build(Box::new(BuildProblem::Registry(
                error,
            ))));
        })
        .ok();
    let planned = match (&project, &index) {
        (Some(project), Some(index)) => match plan(project, &lock, &lock, index) {
            Ok(planned) => Some(planned),
            Err(found) => {
                problems.extend(found);
                None
            }
        },
        _ => None,
    };
    // An update can be told only of a composition that can be planned: a
    // problem that keeps it from resolving is reported once, above.
    let updates = match (&project, &index, &planned) {
        (Some(project), Some(index), Some(_)) => {
            match newest_allowed(project.dependencies(), index) {
                Ok(versions) => versions,
                Err(found) => {
                    problems.extend(
                        found
                            .into_iter()
                            .map(|problem| ComposeProblem::Build(Box::new(problem))),
                    );
                    BTreeMap::new()
                }
            }
        }
        _ => BTreeMap::new(),
    };

    let up_to_date = match (&state, &effective, &planned) {
        (CompositionState::Composed, Some((_, reconciliation)), Some(planned)) => Some(
            snapshot_of(&planned.files).identity() == *reconciliation.base()
                && planned.resolved.lock == lock,
        ),
        _ => None,
    };
    let edited = match (&state, &effective) {
        (CompositionState::Composed, Some((_, reconciliation))) => {
            match edits(root, &store, reconciliation) {
                Ok(edited) => edited,
                Err(problem) => {
                    problems.push(problem);
                    Vec::new()
                }
            }
        }
        _ => Vec::new(),
    };

    // The units: every locked one, and every selection not locked yet.
    let requirements: BTreeMap<UnitIdentity, VersionRequirement> = project
        .as_ref()
        .map(|project| project.dependencies().clone())
        .unwrap_or_default();
    let required_by = planned.as_ref().map(dependents).unwrap_or_default();
    let identities: BTreeSet<&UnitIdentity> =
        lock.units().keys().chain(requirements.keys()).collect();
    let units = identities
        .into_iter()
        .map(|identity| {
            let version = lock
                .units()
                .get(identity)
                .map(|locked| locked.version().clone());
            let update = updates
                .get(identity)
                .filter(|_| version.is_some())
                .filter(|allowed| Some(*allowed) != version.as_ref())
                .cloned();
            let newest = index
                .as_ref()
                .and_then(|index| newest_stable(index, identity))
                .filter(|newest| {
                    version.as_ref().is_none_or(|version| newest > version)
                        && updates.get(identity).is_none_or(|allowed| newest > allowed)
                });
            UnitStatus {
                identity: identity.clone(),
                version,
                requirement: requirements.get(identity).cloned(),
                required_by: required_by.get(identity).cloned().unwrap_or_default(),
                update,
                newest,
            }
        })
        .collect();
    Ok(ProjectStatus {
        state,
        up_to_date,
        units,
        edited,
        origins: planned.map(|planned| planned.origins),
        problems,
    })
}

/// The units that require each unit in the planned resolution, in identity
/// order.
fn dependents(planned: &Planned) -> BTreeMap<UnitIdentity, Vec<UnitIdentity>> {
    let mut required_by: BTreeMap<UnitIdentity, Vec<UnitIdentity>> = BTreeMap::new();
    for (identity, unit) in planned.resolved.resolution.graph().units() {
        for dependency in unit.dependencies().keys() {
            required_by
                .entry(dependency.clone())
                .or_default()
                .push(identity.clone());
        }
    }
    required_by
}

/// The newest version of `identity` the index lists that is not a
/// prerelease.
fn newest_stable(index: &crate::domain::RegistryIndex, identity: &UnitIdentity) -> Option<Version> {
    index
        .units()
        .get(identity)?
        .versions()
        .keys()
        .filter(|version| !version.is_prerelease())
        .max()
        .cloned()
}

/// The managed files that differ from the stored base, in path order. A
/// file whose base is not stored is compared by the identity the listing
/// records, which needs no stored content.
fn edits(
    root: &Path,
    store: &Store,
    reconciliation: &ReconciliationState,
) -> Result<Vec<(RelativePath, FileEdit)>, ComposeProblem> {
    let base = store
        .verify_base(reconciliation.base(), reconciliation.unbased())
        .map_err(ComposeProblem::Base)?;
    let current = read_current(root, &base, &Snapshot::default())?;
    let mut edited = Vec::new();
    for (path, file) in base.files() {
        let edit = match current.get(path).map(|held| &held.compared) {
            None => FileEdit::Deleted,
            Some(Current::Other) => FileEdit::Replaced,
            Some(Current::File(held)) if held == file => continue,
            Some(Current::File(held)) if held.blob() == file.blob() => FileEdit::Mode,
            Some(Current::File(_)) => FileEdit::Modified,
        };
        edited.push((path.clone(), edit));
    }
    Ok(edited)
}

/// A consumer project's status.
#[derive(Debug)]
pub struct ProjectStatus {
    state: CompositionState,
    up_to_date: Option<bool>,
    units: Vec<UnitStatus>,
    edited: Vec<(RelativePath, FileEdit)>,
    origins: Option<BTreeMap<RelativePath, Origin>>,
    problems: Vec<ComposeProblem>,
}

impl ProjectStatus {
    /// Where the project's composition stands.
    pub fn state(&self) -> &CompositionState {
        &self.state
    }

    /// For a composed project whose composition can be planned, whether
    /// composing now would change nothing: neither a file nor the lock.
    pub fn up_to_date(&self) -> Option<bool> {
        self.up_to_date
    }

    /// Every unit locked or selected, in identity order.
    pub fn units(&self) -> &[UnitStatus] {
        &self.units
    }

    /// For a composed project, the files that differ from what the last
    /// composition wrote, in path order. CRLF line endings on the composed
    /// content are not an edit.
    pub fn edited(&self) -> &[(RelativePath, FileEdit)] {
        &self.edited
    }

    /// The units that deliver each file of the composition the declaration
    /// and the lock in effect plan, when it can be planned.
    pub fn origins(&self) -> Option<&BTreeMap<RelativePath, Origin>> {
        self.origins.as_ref()
    }

    /// What kept a composition from being planned or the files from being
    /// compared, in the order found.
    pub fn problems(&self) -> &[ComposeProblem] {
        &self.problems
    }
}

/// Where a project's composition stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompositionState {
    /// The project was never composed.
    Uncomposed,
    /// The last composition completed.
    Composed,
    /// A composition is written except for its lock, which waits for the
    /// consumer to resolve the conflicts marked in these files.
    Pending(Vec<MarkedFile>),
    /// A composition was interrupted; the next one completes or reverts it.
    Interrupted,
}

/// A file a pending composition marked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkedFile {
    path: RelativePath,
    marked: bool,
}

impl MarkedFile {
    /// The file.
    pub fn path(&self) -> &RelativePath {
        &self.path
    }

    /// Whether it still holds conflict markers, or is no longer a regular
    /// file.
    pub fn marked(&self) -> bool {
        self.marked
    }
}

/// How a file differs from what the last composition wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileEdit {
    /// Its content changed.
    Modified,
    /// Only its executable bit changed.
    Mode,
    /// It is gone.
    Deleted,
    /// Something other than a regular file is in its place.
    Replaced,
}

impl FileEdit {
    /// The word that names it, in the human and the machine output.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Modified => "modified",
            Self::Mode => "mode",
            Self::Deleted => "deleted",
            Self::Replaced => "replaced",
        }
    }
}

/// One unit of a project's status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitStatus {
    identity: UnitIdentity,
    version: Option<Version>,
    requirement: Option<VersionRequirement>,
    required_by: Vec<UnitIdentity>,
    update: Option<Version>,
    newest: Option<Version>,
}

impl UnitStatus {
    /// The unit.
    pub fn identity(&self) -> &UnitIdentity {
        &self.identity
    }

    /// The locked version, or `None` for a selection not composed yet.
    pub fn version(&self) -> Option<&Version> {
        self.version.as_ref()
    }

    /// The declaration's requirement, for an explicit selection.
    pub fn requirement(&self) -> Option<&VersionRequirement> {
        self.requirement.as_ref()
    }

    /// The units whose locked versions require it, in identity order.
    pub fn required_by(&self) -> &[UnitIdentity] {
        &self.required_by
    }

    /// The version `assemblr update` would lock, when it is another one.
    pub fn update(&self) -> Option<&Version> {
        self.update.as_ref()
    }

    /// The newest stable version the registry lists, when it is newer than
    /// both the locked version and the update: one the requirements do not
    /// allow.
    pub fn newest(&self) -> Option<&Version> {
        self.newest.as_ref()
    }
}
