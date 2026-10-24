//! Consumer projects: `assemblr init` and `assemblr compose` (see
//! `docs/domain/consumer-projects.md`, `docs/adr/0016-consumer-compose.md`,
//! and `docs/adr/0017-consumer-reconciliation.md`).
//!
//! `init` creates `assemblr.jsonc`, the editable declaration of the
//! project's explicit selections and their options, and nothing else: no
//! Git repository, no files, no commands run. `compose` resolves the
//! declared units from the local registry under the project's lock,
//! composes and renders their files as a unit build does with the project
//! as the root, and reconciles the project with them three-way: the
//! stored reconciliation base, what the project holds now, and the
//! incoming composition decide each path, so that the consumer's edits
//! stay and the composition's changes arrive, and a path both sides
//! changed is merged when its format allows it and refused otherwise.
//! Everything is decided before anything is written, the writes are
//! journaled with the previous content of what they replace so that an
//! interrupted composition is completed or reverted by the next one, and
//! a composition that would change nothing changes nothing.

pub(crate) mod declaration;
mod ignore;
mod lines;
mod pending;
mod store;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub use declaration::DeclarationChange;
use ignore::IgnoreError;
pub use pending::{Continued, abort_compose, continue_compose};
pub use store::StoreError;
use store::{Journal, JournalEntry, Pending, STATE_DIR, Store, write_new};

use super::build::{BuildProblem, render_files, resolve_graph};
use super::git::MANIFEST_LIMIT;
use super::lockfile::{self, LOCKFILE, Lockfile, LockfileError, serialize_lockfile};
use super::manifest::{ReadManifestError, read_project};
use super::merge::{ReconcileFailure, reconcile_json};
use super::registry::Registry;
use super::scaffold::PROJECT_AGENTS;
use crate::domain::{
    Action, BaseIdentity, BlobId, Conflict, Contributor, Current, Disagreement, LockChange,
    MergeFormat, Mergeability, Project, ReconciliationState, RegistryIndex, RelativePath, Root,
    Snapshot, SnapshotFile, UnitFile, UnitIdentity, UnitLock, Version, VersionRequirement, compose,
    is_text, reconcile, update_lock,
};

/// The project's declaration, at its top level.
pub const DECLARATION: &str = "assemblr.jsonc";
/// A unit's manifest, which a consumer project's directory must not hold.
const MANIFEST: &str = "manifest.jsonc";
/// Where a new lockfile is written before it replaces the old one.
const LOCK_STAGING: &str = ".assemblr-lock.json.new";
/// The text `init` writes.
const INITIAL_DECLARATION: &str = "{\n  \"dependencies\": {},\n  \"options\": {}\n}\n";

/// The project's instructions for coding agents, which `init` writes when
/// missing.
const AGENTS: &str = "AGENTS.md";

/// Initializes a consumer project in `directory`, which is created when it
/// does not exist. Besides the declaration, it writes `AGENTS.md`, pointing
/// a coding agent to `assemblr guide`, when nothing is at that path yet;
/// it is written first, so that a project that exists always had its
/// chance at one.
pub fn init_project(directory: &Path) -> Result<ProjectInitialized, InitProjectFailure> {
    let directory = std::path::absolute(directory).map_err(|error| InitProjectFailure::Io {
        path: directory.to_owned(),
        error,
    })?;
    let exists = match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.is_dir() => true,
        Ok(_) => return Err(InitProjectFailure::NotADirectory(directory)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(InitProjectFailure::Io {
                path: directory,
                error,
            });
        }
    };
    if directory.join(MANIFEST).exists() {
        return Err(InitProjectFailure::UnitRepository(directory));
    }
    let declaration = directory.join(DECLARATION);
    match fs::symlink_metadata(&declaration) {
        Ok(metadata) if metadata.is_file() => {
            // Never overwritten: a readable declaration means the project
            // is initialized already.
            let text = read_text(&declaration).map_err(|error| InitProjectFailure::Io {
                path: declaration.clone(),
                error,
            })?;
            return match read_project(&text) {
                Ok(_) => Ok(ProjectInitialized {
                    directory,
                    created: false,
                    directory_created: false,
                    agents_created: false,
                }),
                Err(error) => Err(InitProjectFailure::InvalidDeclaration {
                    path: declaration,
                    error,
                }),
            };
        }
        Ok(_) => return Err(InitProjectFailure::NotAFile(declaration)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(InitProjectFailure::Io {
                path: declaration,
                error,
            });
        }
    }
    if !exists {
        fs::create_dir_all(&directory).map_err(|error| InitProjectFailure::Io {
            path: directory.clone(),
            error,
        })?;
    }
    let agents = directory.join(AGENTS);
    let agents_created = match fs::symlink_metadata(&agents) {
        Ok(_) => false,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Err(error) = write_new(&agents, PROJECT_AGENTS.as_bytes()) {
                // Only a file this call created is removed.
                if error.kind() != io::ErrorKind::AlreadyExists {
                    let _ = fs::remove_file(&agents);
                }
                return Err(InitProjectFailure::Io {
                    path: agents,
                    error,
                });
            }
            true
        }
        Err(error) => {
            return Err(InitProjectFailure::Io {
                path: agents,
                error,
            });
        }
    };
    if let Err(error) = write_new(&declaration, INITIAL_DECLARATION.as_bytes()) {
        if agents_created {
            let _ = fs::remove_file(&agents);
        }
        return Err(InitProjectFailure::Io {
            path: declaration,
            error,
        });
    }
    Ok(ProjectInitialized {
        directory,
        created: true,
        directory_created: !exists,
        agents_created,
    })
}

/// An initialized project.
#[derive(Debug)]
pub struct ProjectInitialized {
    directory: PathBuf,
    created: bool,
    directory_created: bool,
    agents_created: bool,
}

impl ProjectInitialized {
    /// The project's directory, absolute.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Whether the declaration was created, or found in place.
    pub fn created(&self) -> bool {
        self.created
    }

    /// Whether the directory was created.
    pub fn directory_created(&self) -> bool {
        self.directory_created
    }

    /// Whether `AGENTS.md` was created; `false` when something was there.
    pub fn agents_created(&self) -> bool {
        self.agents_created
    }
}

/// Why `init` failed. Nothing was written.
#[derive(Debug)]
pub enum InitProjectFailure {
    /// A path could not be examined or written.
    Io {
        /// The path.
        path: PathBuf,
        /// Why.
        error: io::Error,
    },
    /// The path exists and is not a directory.
    NotADirectory(PathBuf),
    /// The directory holds a unit's manifest.
    UnitRepository(PathBuf),
    /// `assemblr.jsonc` is a directory, a symbolic link, or another
    /// special file.
    NotAFile(PathBuf),
    /// `assemblr.jsonc` exists and is not a valid declaration.
    InvalidDeclaration {
        /// The declaration.
        path: PathBuf,
        /// Why.
        error: ReadManifestError,
    },
}

impl fmt::Display for InitProjectFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, error } => write!(f, "could not use {}: {error}", path.display()),
            Self::NotADirectory(path) => {
                write!(f, "{} exists and is not a directory", path.display())
            }
            Self::UnitRepository(path) => write!(
                f,
                "{} holds `manifest.jsonc`, so it is a unit's repository, not a consumer \
                 project; a unit is initialized with `assemblr unit init`",
                path.display()
            ),
            Self::NotAFile(path) => write!(f, "{} is not a regular file", path.display()),
            Self::InvalidDeclaration { path, error } => write!(
                f,
                "{} exists and is not a valid declaration, so it was left as it is: {error}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for InitProjectFailure {}

/// Composes the project whose top level is `root`, resolving its units
/// from `registry`.
pub fn compose_project(root: &Path, registry: &Registry) -> Result<Composed, ComposeFailure> {
    compose_with(root, registry, None, None, &mut |_| Ok(()))
}

/// Moves the locked versions of `units`, or of every locked unit when none
/// is named, to the newest versions the declared requirements allow, then
/// reconciles the project as [`compose_project`] does. The declaration is
/// never edited. The other locked versions are kept unless a requirement of
/// the new ones forces them to move. A unit that is not locked is refused,
/// and so is a project that was never composed.
pub fn update_units(
    root: &Path,
    registry: &Registry,
    units: &[UnitIdentity],
) -> Result<Composed, ComposeFailure> {
    compose_with(root, registry, None, Some(units), &mut |_| Ok(()))
}

/// Selects `identity` in the declaration of the project whose top level is
/// `root`, then composes it as [`compose_project`] does. Without a
/// `requirement` the unit is recorded with a caret requirement on the
/// version the resolution picks; a unit already selected keeps its own.
/// Each of `options`, a parameter name and the text of its value, is set
/// in the unit's `options` entry, read by the parameter's type: the text
/// itself for a string parameter, and otherwise the JSON value it reads
/// as, such as `8080` or `true`, or a string when it is not JSON. A
/// composition that is refused leaves the declaration as it was.
pub fn add_unit(
    root: &Path,
    registry: &Registry,
    identity: &UnitIdentity,
    requirement: Option<&str>,
    options: &[(&str, &str)],
) -> Result<Composed, ComposeFailure> {
    let edit = DeclarationEdit::Add {
        identity: identity.clone(),
        requirement: requirement.map(str::to_owned),
        options: options
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
    };
    compose_with(root, registry, Some(&edit), None, &mut |_| Ok(()))
}

/// Removes the selection of `identity`, and its `options` entry, from the
/// declaration of the project whose top level is `root`, then composes it
/// as [`compose_project`] does.
pub fn remove_unit(
    root: &Path,
    registry: &Registry,
    identity: &UnitIdentity,
) -> Result<Composed, ComposeFailure> {
    let edit = DeclarationEdit::Remove {
        identity: identity.clone(),
    };
    compose_with(root, registry, Some(&edit), None, &mut |_| Ok(()))
}

/// What `add` and `remove` change in the declaration.
enum DeclarationEdit {
    Add {
        identity: UnitIdentity,
        requirement: Option<String>,
        options: Vec<(String, String)>,
    },
    Remove {
        identity: UnitIdentity,
    },
}

/// The declaration an edit produced, planned and not written.
struct PlannedDeclaration {
    project: Project,
    text: String,
    change: DeclarationChange,
}

/// The steps of writing, so that tests can make each one fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Store,
    Previous,
    Journal,
    Files,
    Declaration,
    Pending,
    Lock,
    Finish,
}

/// Opens the declaration and takes the project's lock. Compositions of
/// one project run one at a time: the declaration, which every composition
/// reads and none replaces, carries the lock. The handle is writable where
/// the file system allows: `add` and `remove` write the declaration
/// through it, and so does the recovery of one that was interrupted, since
/// a rename would leave the lock on a file that is no longer the
/// declaration.
fn lock_declaration(root: &Path) -> Result<fs::File, ComposeProblem> {
    let declaration_path = root.join(DECLARATION);
    if fs::symlink_metadata(&declaration_path).is_err() {
        return Err(ComposeProblem::NoDeclaration(declaration_path));
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(&declaration_path)
        .or_else(|_| OpenOptions::new().read(true).open(&declaration_path))
        .and_then(|file| fs4::FileExt::lock(&file).map(|()| file))
        .map_err(|error| ComposeProblem::Io {
            path: declaration_path,
            error,
        })
}

fn compose_with(
    root: &Path,
    registry: &Registry,
    edit: Option<&DeclarationEdit>,
    update: Option<&[UnitIdentity]>,
    fault: &mut dyn FnMut(Step) -> io::Result<()>,
) -> Result<Composed, ComposeFailure> {
    let rejected = |problem| ComposeFailure::Rejected {
        problems: vec![problem],
        recovered: None,
    };
    let declaration_path = root.join(DECLARATION);
    // A unit's directory is told apart first: the author is in the wrong
    // flow, not in the wrong directory.
    if root.join(MANIFEST).exists() {
        return Err(rejected(ComposeProblem::UnitRepository(root.to_owned())));
    }
    if fs::symlink_metadata(&declaration_path).is_err() {
        return Err(rejected(ComposeProblem::NoDeclaration(declaration_path)));
    }
    let guard = lock_declaration(root).map_err(rejected)?;
    // `add` and `remove` write the declaration, so a read-only one is
    // refused before anything else happens, not after the files are.
    if edit.is_some()
        && fs::metadata(&declaration_path).is_ok_and(|metadata| metadata.permissions().readonly())
    {
        return Err(rejected(ComposeProblem::CannotEdit(format!(
            "{DECLARATION} is read-only"
        ))));
    }
    let store = Store::new(root);
    let mut problems = Vec::new();
    let mut recovered = None;
    match store.journal() {
        Ok(None) => {}
        Ok(Some(journal)) => {
            // A composition waiting for the consumer's resolution is not an
            // interrupted one: reverting it would discard the resolution.
            if let Some(pending) = journal
                .pending
                .as_ref()
                .filter(|_| !pending::lock_records(root, &journal))
            {
                return Err(rejected(ComposeProblem::Pending(pending.files.clone())));
            }
            match recover(root, &store, &journal, &guard, &[]) {
                Ok(summary) => recovered = Some(summary),
                Err(error) => return Err(rejected(ComposeProblem::Store(error))),
            }
        }
        Err(error) => return Err(rejected(ComposeProblem::Store(error))),
    }
    let rejected = |problem| ComposeFailure::Rejected {
        problems: vec![problem],
        recovered: recovered.clone(),
    };
    let rejected_all = |problems| ComposeFailure::Rejected {
        problems,
        recovered: recovered.clone(),
    };
    let declaration_text = read_handle(&guard)
        .map_err(|error| ComposeProblem::Io {
            path: declaration_path.clone(),
            error,
        })
        .map_err(|problem| problems.push(problem))
        .ok();
    let project = declaration_text.as_deref().and_then(|text| {
        read_project(text)
            .map_err(|error| {
                problems.push(ComposeProblem::InvalidDeclaration {
                    path: declaration_path.clone(),
                    error,
                });
            })
            .ok()
    });
    let previous = match lockfile::read_lockfile(root) {
        Ok(None) => Some(None),
        Ok(Some((_, Lockfile { project: None, .. }))) => {
            problems.push(ComposeProblem::ForeignLockfile(root.join(LOCKFILE)));
            None
        }
        Ok(Some((
            _,
            Lockfile {
                lock,
                project: Some(state),
            },
        ))) => Some(Some((lock, state))),
        Err(error) => {
            problems.push(ComposeProblem::Lockfile(error));
            None
        }
    };
    let index = registry
        .index()
        .map_err(|error| {
            problems.push(ComposeProblem::Build(Box::new(BuildProblem::Registry(
                error,
            ))))
        })
        .ok();
    let (Some(project), Some(previous), Some(index)) = (project, previous, index) else {
        return Err(rejected_all(problems));
    };
    // The stored base is verified before anything else happens to a
    // project that was composed before. A first composition has an empty
    // base: every file is new.
    let base = match &previous {
        Some((_, state)) => match store.verify_base(state.base(), state.unbased()) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                problems.push(ComposeProblem::Base(error));
                return Err(rejected_all(problems));
            }
        },
        None => Snapshot::default(),
    };
    let previously_unbased: BTreeSet<RelativePath> = previous
        .as_ref()
        .map(|(_, state)| state.unbased().clone())
        .unwrap_or_default();
    let previous_lock = previous
        .as_ref()
        .map(|(lock, _)| lock.clone())
        .unwrap_or_default();
    // `add` and `remove`: the composition runs from the edited declaration,
    // which is written only after the composition is planned and
    // validated, so a refused composition leaves the declaration as it was.
    let mut planned_declaration: Option<PlannedDeclaration> = None;
    let project = match (edit, declaration_text.as_deref()) {
        (Some(edit), Some(text)) => match plan_declaration(edit, text, &previous_lock, &index) {
            Ok(planned) => {
                let edited = planned.project.clone();
                planned_declaration = Some(planned);
                edited
            }
            Err(found) => {
                problems.extend(found);
                return Err(rejected_all(problems));
            }
        },
        _ => project,
    };
    let declaration_changes = planned_declaration
        .as_ref()
        .is_some_and(|planned| Some(planned.text.as_str()) != declaration_text.as_deref());
    // An update resolves without the locked versions of the units it
    // moves, so that the others keep precedence (see `unit update`).
    let resolution_lock = match update {
        None => previous_lock.clone(),
        Some(named) => {
            if previous.is_none() {
                problems.push(ComposeProblem::NothingLocked);
                return Err(rejected_all(problems));
            }
            for unit in named {
                if !previous_lock.units().contains_key(unit) {
                    problems.push(ComposeProblem::NotLocked {
                        unit: unit.clone(),
                        locked: previous_lock.units().keys().cloned().collect(),
                    });
                }
            }
            if !problems.is_empty() {
                return Err(rejected_all(problems));
            }
            if named.is_empty() {
                UnitLock::default()
            } else {
                UnitLock::new(
                    previous_lock
                        .units()
                        .iter()
                        .filter(|(identity, _)| !named.contains(identity))
                        .map(|(identity, locked)| (identity.clone(), locked.clone()))
                        .collect(),
                )
            }
        }
    };
    let mut found = Vec::new();
    let Some(resolved) = resolve_graph(
        None,
        project.dependencies(),
        &resolution_lock,
        &index,
        &mut found,
    ) else {
        problems.extend(
            found
                .into_iter()
                .map(|problem| ComposeProblem::Build(Box::new(problem))),
        );
        return Err(rejected_all(problems));
    };
    // What changed is told against the whole previous lock, and a kept
    // version must still be published with the content it was locked to,
    // whatever lock the resolver was given.
    let lock_changes = match update_lock(&previous_lock, resolved.resolution.graph(), &index) {
        Ok((_, changes)) => changes,
        Err(errors) => {
            found.extend(errors.into_iter().map(BuildProblem::Lock));
            problems.extend(
                found
                    .into_iter()
                    .map(|problem| ComposeProblem::Build(Box::new(problem))),
            );
            return Err(rejected_all(problems));
        }
    };
    let units = resolved.units();
    let composition = match compose(
        resolved.resolution.graph(),
        Root::Project,
        &units,
        project.options(),
    ) {
        Ok(composition) => composition,
        Err(errors) => {
            found.extend(errors.into_iter().map(BuildProblem::Composition));
            problems.extend(
                found
                    .into_iter()
                    .map(|problem| ComposeProblem::Build(Box::new(problem))),
            );
            return Err(rejected_all(problems));
        }
    };
    let rendered = render_files(
        &composition,
        &resolved.lock,
        None,
        &resolved.dependencies,
        &mut found,
    );
    problems.extend(
        found
            .into_iter()
            .map(|problem| ComposeProblem::Build(Box::new(problem))),
    );
    let Some(rendered) = rendered.filter(|_| problems.is_empty()) else {
        return Err(rejected_all(problems));
    };
    let mut files: BTreeMap<RelativePath, UnitFile> = rendered.files.into_iter().collect();
    // A conflict between sibling amenders that no unit above both resolves
    // is the consumer's to resolve (owner decision on #101): the file, each
    // conflict between markers, is the composition's, so the markers reach
    // the project like any incoming change and the composition waits.
    let mut unit_conflicts: Vec<(RelativePath, String)> = Vec::new();
    for unresolved in rendered.unresolved {
        let report = unresolved.to_string();
        files.insert(
            unresolved.delivered.clone(),
            UnitFile::new(unresolved.bytes, unresolved.executable),
        );
        unit_conflicts.push((unresolved.delivered, report));
    }
    // What the merge reports about a file, its author's to act on, is told
    // to the consumer when this composition writes the file; the others
    // are kept for `--verbose`, with the amendments applied across
    // provider versions.
    let file_notes = rendered.notes;
    let crossed: Vec<String> = rendered.crossed.into_iter().map(|(_, line)| line).collect();
    let mut notes = Vec::new();

    let reserved: Vec<RelativePath> = files
        .keys()
        .filter(|path| is_reserved(path))
        .cloned()
        .collect();
    if !reserved.is_empty() {
        problems.push(ComposeProblem::Reserved(reserved));
    }
    let snapshot = Snapshot::new(
        files
            .iter()
            .map(|(path, file)| {
                (
                    path.clone(),
                    SnapshotFile::new(file.contents(), file.is_executable()),
                )
            })
            .collect(),
    );
    let identity = snapshot.identity();

    // A project composed before: nothing to do when the composition is the
    // one materialized, whatever the consumer did to its files since.
    if let Some((_, state)) = &previous {
        if !problems.is_empty() {
            return Err(rejected_all(problems));
        }
        // The lock is rewritten when the resolution changed, whether or not
        // a file did: it records the versions and provenance the project was
        // composed from.
        if *state.base() == identity && !declaration_changes && resolved.lock == previous_lock {
            return Ok(Composed {
                units: resolved.lock.units().len(),
                files: 0,
                changes: BTreeMap::new(),
                paths: BTreeMap::new(),
                base: identity,
                unbased: BTreeSet::new(),
                notes,
                unwritten_notes: file_notes.into_iter().map(|(_, note)| note).collect(),
                crossed,
                recovered: recovered.clone(),
                unchanged: true,
                first: false,
                declaration: planned_declaration.map(|planned| planned.change),
                pending: BTreeSet::new(),
                consumer_conflicts: BTreeSet::new(),
                unit_conflicts: Vec::new(),
            });
        }
    }

    // What the project holds at the base's paths, and what to do at every
    // path of the base and the incoming composition.
    let current = match read_current(root, &base, &snapshot) {
        Ok(current) => current,
        Err(problem) => {
            problems.push(problem);
            return Err(rejected_all(problems));
        }
    };
    let compared: BTreeMap<RelativePath, Current> = current
        .iter()
        .map(|(path, file)| (path.clone(), file.compared.clone()))
        .collect();
    let mergeable = |path: &RelativePath| {
        if previously_unbased.contains(path) {
            Mergeability::BaseNotStored
        } else if MergeFormat::of_project_path(path) == MergeFormat::Unsupported {
            Mergeability::Format
        } else {
            Mergeability::Mergeable
        }
    };
    let reconciliation = reconcile(&base, &compared, &snapshot, &mergeable);
    let mut conflicts: Vec<FileConflict> = reconciliation
        .disagreements()
        .iter()
        .map(|(path, disagreement)| FileConflict {
            path: path.clone(),
            kind: FileConflictKind::Disagreement(disagreement.clone()),
        })
        .collect();
    let mut changes: BTreeMap<RelativePath, Change> = BTreeMap::new();
    let mut created = Vec::new();
    for (path, action) in reconciliation.actions() {
        let incoming = files.get(path);
        let held = current.get(path);
        match (action, incoming, held) {
            (Action::Create, Some(file), _) => {
                created.push(path.clone());
                changes.insert(path.clone(), Change::Create(file.clone()));
            }
            (Action::Write { executable }, Some(file), Some(held)) => {
                let mut bytes = file.contents().to_vec();
                if held.crlf {
                    bytes = with_crlf(&bytes);
                    notes.push(format!(
                        "{path} has CRLF line endings in the project, so the composed content \
                         was written with CRLF line endings too"
                    ));
                }
                changes.insert(
                    path.clone(),
                    Change::Modify {
                        file: UnitFile::new(bytes, *executable),
                        previous: held.bytes.clone(),
                        previous_executable: held.executable,
                        merged: false,
                    },
                );
            }
            (Action::Mode { executable }, _, Some(held)) => {
                changes.insert(
                    path.clone(),
                    Change::Modify {
                        file: UnitFile::new(held.bytes.clone(), *executable),
                        previous: held.bytes.clone(),
                        previous_executable: held.executable,
                        merged: false,
                    },
                );
            }
            (Action::Merge { executable }, Some(file), Some(held)) => {
                match merge_file(&store, &base, path, held, file) {
                    Ok((bytes, not_carried)) => {
                        notes.extend(not_carried.into_iter().map(|what| {
                            format!(
                                "{path}: {what} in the composition is not in your file; the \
                                 merge carries members and values, not comments or order"
                            )
                        }));
                        changes.insert(
                            path.clone(),
                            Change::Modify {
                                file: UnitFile::new(bytes, *executable),
                                previous: held.bytes.clone(),
                                previous_executable: held.executable,
                                merged: true,
                            },
                        );
                    }
                    Err(kind) => conflicts.push(FileConflict {
                        path: path.clone(),
                        kind,
                    }),
                }
            }
            (Action::Remove, _, Some(held)) => {
                changes.insert(
                    path.clone(),
                    Change::Remove {
                        previous: held.bytes.clone(),
                        previous_executable: held.executable,
                    },
                );
            }
            (Action::Keep, _, None) if base.files().contains_key(path) => {
                notes.push(format!(
                    "{path} was deleted in the project and the composition does not change it; \
                     it stays deleted"
                ));
            }
            _ => {}
        }
    }
    // A conflict between the project's text and the composition's is left
    // in the file, marked, for the consumer to resolve; a composition
    // with any other kind of conflict is refused whole.
    let mut marked: BTreeSet<RelativePath> = BTreeSet::new();
    let marks: Vec<Option<Change>> = conflicts
        .iter()
        .map(|conflict| pending::marked_change(&store, &base, &current, &files, conflict))
        .collect();
    if marks.iter().all(Option::is_some) {
        for (conflict, change) in conflicts.drain(..).zip(marks) {
            if let Some(change) = change {
                marked.insert(conflict.path.clone());
                changes.insert(conflict.path, change);
            }
        }
    }
    conflicts.sort_by(|a, b| a.path.cmp(&b.path));
    if !conflicts.is_empty() {
        problems.push(ComposeProblem::Conflicts(conflicts));
    }
    // A file the units leave marked waits for the consumer as long as the
    // project holds the markers: when this composition writes them, and
    // when the project holds them already, as a clone of a waiting project
    // does, where the reconciliation has nothing to write. A conflict the
    // consumer resolved before, which the composition keeps, is not
    // theirs to resolve again.
    let mut consumer_conflicts = marked.clone();
    unit_conflicts.retain(|(path, _)| {
        let written = matches!(
            changes.get(path),
            Some(Change::Create(_) | Change::Modify { .. })
        );
        let held = current
            .get(path)
            .is_some_and(|held| super::layered::has_markers(&held.bytes));
        if written || held {
            marked.insert(path.clone());
        }
        written || held
    });

    // A created path where the project already holds a file of its own,
    // which Assemblr did not write, is a conflict for the consumer to
    // resolve like any other (owner decision on OC-44): both files, whole,
    // between markers, with the project's executable bit, which the
    // consumer set. A file that already holds the composed content, or that
    // content with CRLF line endings, is kept as it is, mode included. What
    // cannot be marked, a binary file or something other than a file, is
    // refused as existing content below.
    created.retain(|path| {
        let Some(incoming) = files.get(path) else {
            return true;
        };
        let (bytes, executable) = match existing_file(root, path, incoming.is_executable()) {
            Ok(Some(held)) => held,
            Ok(None) => return true,
            Err(problem) => {
                problems.push(problem);
                return false;
            }
        };
        if bytes == incoming.contents()
            || without_crlf(&bytes).as_deref() == Some(incoming.contents())
        {
            changes.remove(path);
            notes.push(format!(
                "{path} already held the composed content, so it was kept as it is"
            ));
            return false;
        }
        match pending::marked_existing(&bytes, executable, incoming) {
            Some(change) => {
                changes.insert(path.clone(), change);
                marked.insert(path.clone());
                consumer_conflicts.insert(path.clone());
                false
            }
            None => true,
        }
    });

    // Nothing else of a created file may exist yet.
    let (existing, mut directories) = (existing_paths(root, created.iter()), Vec::new());
    match existing {
        Ok(found) if found.is_empty() => {}
        Ok(found) => problems.push(ComposeProblem::Existing(found)),
        Err(problem) => problems.push(problem),
    }
    // Which delivered files the project's version control ignores: their
    // base is not stored, since the store is tracked. Only the project's
    // own rules, its `.gitignore` files, decide that, because the lock
    // records it and every clone must lock the same way. The rules are read
    // at every composition: a file the project ignored since is no longer
    // stored, and one it no longer ignores is stored from now on.
    let path_texts: Vec<&str> = files.keys().map(RelativePath::as_str).collect();
    let mut probe = path_texts.clone();
    let directories_of_store = store.directories();
    probe.extend(directories_of_store.iter().map(String::as_str));
    let ignores = match ignore::ignored(root, &probe) {
        Ok(ignores) => ignores,
        Err(error) => {
            problems.push(ComposeProblem::Ignore(error));
            ignore::Ignores::default()
        }
    };
    // The consumer's own rules (`.git/info/exclude`, the global excludes)
    // keep a file out of version control on their machine only. Such a
    // file is not unbased for everyone, so storing its content would put
    // into the tracked base what the consumer chose to keep out, and
    // leaving it unstored would lock this project differently here than
    // elsewhere. A file the base already stores stays stored: that was
    // settled before, and nothing new enters the store.
    let already_stored =
        |path: &RelativePath| base.files().contains_key(path) && !previously_unbased.contains(path);
    let locally_ignored: Vec<(RelativePath, String)> = files
        .keys()
        .filter(|path| !ignores.is_project(path))
        .filter(|path| !already_stored(path))
        .filter_map(|path| {
            ignores.local(path).map(|rule| {
                (
                    path.clone(),
                    format!("pattern {:?} in {}", rule.pattern(), rule.source()),
                )
            })
        })
        .collect();
    if !locally_ignored.is_empty() {
        problems.push(ComposeProblem::LocallyIgnored(locally_ignored));
    }
    if !problems.is_empty() {
        return Err(rejected_all(problems));
    }
    let unbased: BTreeSet<RelativePath> = files
        .keys()
        .filter(|path| ignores.is_project(path))
        .cloned()
        .collect();
    for path in &unbased {
        if previously_unbased.contains(path) {
            continue;
        }
        notes.push(format!(
            "{path} is ignored by the project's .gitignore, so its reconciliation base is not \
             stored; a later composition cannot reconcile edits to it"
        ));
    }
    for path in files.keys() {
        if previously_unbased.contains(path) && !unbased.contains(path) {
            notes.push(format!(
                "{path} is no longer ignored by the project's .gitignore, so its composed content \
                 is stored in the tracked base from now on"
            ));
        }
    }
    if let Some(directory) = directories_of_store
        .iter()
        .find(|directory| ignores.any(directory))
    {
        notes.push(format!(
            "{directory} is ignored by the project's version control, so the reconciliation base will \
             not travel with the project; a clone cannot reconcile later compositions"
        ));
    }
    let state = ReconciliationState::new(
        identity.clone(),
        files.keys().cloned().collect(),
        unbased.clone(),
    );
    let lock_bytes = serialize_lockfile(&resolved.lock, Some(&state)).map_err(|error| {
        rejected(ComposeProblem::Io {
            path: root.join(LOCKFILE),
            error,
        })
    })?;
    for path in &created {
        let mut segments: Vec<&str> = path.segments().collect();
        segments.pop();
        let mut directory = String::new();
        for segment in segments {
            if !directory.is_empty() {
                directory.push('/');
            }
            directory.push_str(segment);
            if let Ok(directory) = RelativePath::parse(&directory) {
                if !directories.contains(&directory) && !root.join(directory.as_str()).exists() {
                    directories.push(directory);
                }
            }
        }
    }
    directories.sort();
    directories.dedup();

    // Writing (see `apply`).
    let declaration_used = planned_declaration
        .as_ref()
        .map(|planned| planned.text.as_str())
        .or(declaration_text.as_deref())
        .unwrap_or_default();
    let application = Application {
        files: &files,
        changes: &changes,
        snapshot: &snapshot,
        unbased: &unbased,
        lock_bytes: &lock_bytes,
        directories: &directories,
        marked: &marked,
        declaration_used: BlobId::of(declaration_used.as_bytes()),
        declaration: planned_declaration
            .as_ref()
            .filter(|_| declaration_changes)
            .zip(declaration_text.as_deref())
            .map(|(planned, previous)| DeclarationWrite {
                guard: &guard,
                text: planned.text.as_bytes(),
                previous: previous.as_bytes(),
            }),
    };
    let restored = apply(root, &store, &application, fault).map_err(|failure| match failure {
        ComposeFailure::Rejected { problems, .. } => rejected_all(problems),
        write => write,
    })?;
    for name in restored {
        notes.push(format!(
            "{STATE_DIR}/{name} had been edited and was restored; it keeps version control from \
             {}",
            if name == ".gitattributes" {
                "converting the stored base"
            } else {
                "committing the journal and the previous content of a composition in progress"
            }
        ));
    }
    drop(guard);
    let mut paths: BTreeMap<RelativePath, FileChange> = changes
        .iter()
        .map(|(path, change)| {
            let change = match change {
                Change::Create(_) | Change::Modify { .. } if marked.contains(path) => {
                    FileChange::Conflicted
                }
                Change::Create(_) => FileChange::Created,
                Change::Modify { merged: true, .. } => FileChange::Merged,
                Change::Modify { .. } => FileChange::Modified,
                Change::Remove { .. } => FileChange::Removed,
            };
            (path.clone(), change)
        })
        .collect();
    for path in &marked {
        paths.entry(path.clone()).or_insert(FileChange::Conflicted);
    }
    let (written, unwritten): (Vec<_>, Vec<_>) = file_notes
        .into_iter()
        .partition(|(path, _)| changes.contains_key(path));
    let notes = written
        .into_iter()
        .map(|(_, note)| note)
        .chain(notes)
        .collect();
    Ok(Composed {
        units: resolved.lock.units().len(),
        files: files.len(),
        changes: lock_changes,
        paths,
        base: identity,
        unbased,
        notes,
        unwritten_notes: unwritten.into_iter().map(|(_, note)| note).collect(),
        crossed,
        recovered: recovered.clone(),
        unchanged: false,
        first: previous.is_none(),
        declaration: planned_declaration.map(|planned| planned.change),
        pending: marked,
        consumer_conflicts,
        unit_conflicts,
    })
}

/// A file of the project, as read for reconciliation.
struct CurrentFile {
    /// Its bytes, as they are.
    bytes: Vec<u8>,
    /// Its executable bit, or the base's where the file system has none.
    executable: bool,
    /// What it is compared as.
    compared: Current,
    /// Whether it is the base's or the incoming content with CRLF line
    /// endings, which a checkout can produce: it counts as unchanged, and
    /// what is written to it keeps its line endings.
    crlf: bool,
}

/// Reads what the project holds at every path of `base`. The incoming
/// composition's other paths are new, and what may exist there is checked
/// as existing content.
fn read_current(
    root: &Path,
    base: &Snapshot,
    incoming: &Snapshot,
) -> Result<BTreeMap<RelativePath, CurrentFile>, ComposeProblem> {
    let mut current = BTreeMap::new();
    for (path, file) in base.files() {
        let target = root.join(path.as_str());
        let other = CurrentFile {
            bytes: Vec::new(),
            executable: false,
            compared: Current::Other,
            crlf: false,
        };
        // The directories on the way must be real ones: a symbolic link
        // among them would take a read, and later a write, out of the
        // project.
        let mut prefix = root.to_owned();
        let mut segments: Vec<&str> = path.segments().collect();
        segments.pop();
        let mut through_link = false;
        for segment in segments {
            prefix.push(segment);
            match fs::symlink_metadata(&prefix) {
                Ok(metadata) if metadata.is_dir() => {}
                Ok(_) => {
                    through_link = true;
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => break,
                Err(error) => {
                    return Err(ComposeProblem::Io {
                        path: prefix,
                        error,
                    });
                }
            }
        }
        if through_link {
            current.insert(path.clone(), other);
            continue;
        }
        let metadata = match fs::symlink_metadata(&target) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(ComposeProblem::Io {
                    path: target,
                    error,
                });
            }
        };
        if !metadata.is_file() {
            current.insert(path.clone(), other);
            continue;
        }
        let bytes = fs::read(&target).map_err(|error| ComposeProblem::Io {
            path: target.clone(),
            error,
        })?;
        let executable = executable_bit(&metadata, file.is_executable());
        let known = |blob: &BlobId| {
            *file.blob() == *blob
                || incoming
                    .files()
                    .get(path)
                    .is_some_and(|incoming| incoming.blob() == blob)
        };
        let raw = BlobId::of(&bytes);
        let (blob, crlf) = if known(&raw) {
            (raw, false)
        } else {
            match without_crlf(&bytes).map(|text| BlobId::of(&text)) {
                Some(normalized) if known(&normalized) => (normalized, true),
                _ => (raw, false),
            }
        };
        current.insert(
            path.clone(),
            CurrentFile {
                bytes,
                executable,
                compared: Current::File(SnapshotFile::from_blob(blob, executable)),
                crlf,
            },
        );
    }
    Ok(current)
}

#[cfg(unix)]
fn executable_bit(metadata: &fs::Metadata, _base: bool) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

/// Where the file system has no executable bit, the file's mode is the
/// base's: the consumer cannot have changed it.
#[cfg(not(unix))]
fn executable_bit(_metadata: &fs::Metadata, base: bool) -> bool {
    base
}

/// `bytes` with every CRLF line ending as LF, when `bytes` is text whose
/// line endings are all CRLF; `None` otherwise.
fn without_crlf(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.contains(&0) || !bytes.contains(&b'\r') {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => {
                out.push(b'\n');
                index += 2;
            }
            b'\r' | b'\n' => return None,
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    Some(out)
}

/// `bytes` with every LF line ending as CRLF, when `bytes` has no CR.
fn with_crlf(bytes: &[u8]) -> Vec<u8> {
    if bytes.contains(&b'\r') {
        return bytes.to_vec();
    }
    let mut out = Vec::with_capacity(bytes.len() + bytes.len() / 16);
    for &byte in bytes {
        if byte == b'\n' {
            out.push(b'\r');
        }
        out.push(byte);
    }
    out
}

/// Merges the incoming change to `path` into the project's file.
fn merge_file(
    store: &Store,
    base: &Snapshot,
    path: &RelativePath,
    held: &CurrentFile,
    incoming: &UnitFile,
) -> Result<(Vec<u8>, Vec<String>), FileConflictKind> {
    let Some(base_file) = base.files().get(path) else {
        return Err(FileConflictKind::Unmergeable("it has no base".to_owned()));
    };
    let base_bytes = store
        .read_blob(base_file.blob())
        .map_err(|error| FileConflictKind::Unmergeable(error.to_string()))?;
    // Text is UTF-8 without a NUL: a small binary file is valid UTF-8 and
    // must not be merged by line.
    let text = |bytes: &[u8], what: &str| {
        if !is_text(bytes) {
            return Err(FileConflictKind::Unmergeable(format!(
                "{what} is not UTF-8 text"
            )));
        }
        Ok(String::from_utf8_lossy(bytes).into_owned())
    };
    let base_text = text(&base_bytes, "its base")?;
    let current_text = text(&held.bytes, "the project's file")?;
    let incoming_text = text(incoming.contents(), "the composed file")?;
    if MergeFormat::of_project_path(path) == MergeFormat::Text {
        return lines::reconcile(&base_text, &current_text, &incoming_text)
            .map(|merged| (merged.into_bytes(), Vec::new()))
            .map_err(|places| FileConflictKind::Lines { places });
    }
    match reconcile_json(&base_text, &current_text, &incoming_text) {
        Ok(reconciled) => Ok((reconciled.text.into_bytes(), reconciled.notes)),
        Err(ReconcileFailure::Conflicts(conflicts)) => Err(FileConflictKind::Values(conflicts)),
        Err(other) => Err(FileConflictKind::Unmergeable(other.to_string())),
    }
}

/// What a composition does to one file of the project.
enum Change {
    /// Creates it.
    Create(UnitFile),
    /// Replaces its content or its mode; `merged` when the content is the
    /// merge of the project's and the composition's changes.
    Modify {
        file: UnitFile,
        previous: Vec<u8>,
        previous_executable: bool,
        merged: bool,
    },
    /// Removes it.
    Remove {
        previous: Vec<u8>,
        previous_executable: bool,
    },
}

/// What a composition writes into the project.
struct Application<'a> {
    /// The composed files, the next reconciliation base.
    files: &'a BTreeMap<RelativePath, UnitFile>,
    /// What changes in the project, by path.
    changes: &'a BTreeMap<RelativePath, Change>,
    snapshot: &'a Snapshot,
    unbased: &'a BTreeSet<RelativePath>,
    lock_bytes: &'a [u8],
    /// The directories to create, outermost first.
    directories: &'a [RelativePath],
    /// The declaration `add` or `remove` rewrites, when it changes.
    declaration: Option<DeclarationWrite<'a>>,
    /// The files that hold conflict markers. When there are any, the
    /// composition is written except for its lock, which waits for
    /// `compose --continue`.
    marked: &'a BTreeSet<RelativePath>,
    /// The identity of the declaration the composition was planned from,
    /// as the project holds it once the composition is written.
    declaration_used: BlobId,
}

/// The declaration's new text, written through the handle that holds its
/// lock.
#[derive(Clone, Copy)]
struct DeclarationWrite<'a> {
    guard: &'a fs::File,
    text: &'a [u8],
    previous: &'a [u8],
}

/// Writes `application` into the project at `root`: the store first,
/// content-addressed, so a leftover is harmless; then the previous content
/// of the files that change, then the journal; then the directories and
/// the file changes; then the lock; then the journal's removal. Returns
/// the store's own files that had to be restored. A failure before the
/// journal leaves the project untouched; after it, the journal records the
/// composition for [`recover`].
fn apply(
    root: &Path,
    store: &Store,
    application: &Application<'_>,
    fault: &mut dyn FnMut(Step) -> io::Result<()>,
) -> Result<Vec<&'static str>, ComposeFailure> {
    let Application {
        files,
        changes,
        snapshot,
        unbased,
        lock_bytes,
        directories,
        declaration,
        marked,
        ref declaration_used,
    } = *application;
    let rejected = |problem| ComposeFailure::Rejected {
        problems: vec![problem],
        recovered: None,
    };
    let write_failed = |step: WriteStep, error: io::Error| ComposeFailure::Write { step, error };
    let stored: Vec<(&RelativePath, &[u8])> = files
        .iter()
        .filter(|(path, _)| !unbased.contains(*path))
        .map(|(path, file)| (path, file.contents()))
        .collect();
    if let Err(error) = fault(Step::Store) {
        return Err(write_failed(WriteStep::Store, error));
    }
    store
        .write_base(snapshot, &stored)
        .map_err(|error| rejected(ComposeProblem::Store(error)))?;
    if let Err(error) = fault(Step::Previous) {
        return Err(write_failed(WriteStep::Previous, error));
    }
    let mut entries = Vec::new();
    for (path, change) in changes {
        let entry = match change {
            Change::Create(file) => JournalEntry::Create {
                blob: BlobId::of(file.contents()),
            },
            Change::Modify {
                file,
                previous,
                previous_executable,
                ..
            } => JournalEntry::Modify {
                blob: BlobId::of(file.contents()),
                executable: file.is_executable(),
                previous: store
                    .save_previous(previous)
                    .map_err(|error| rejected(ComposeProblem::Store(error)))?,
                previous_executable: *previous_executable,
            },
            Change::Remove {
                previous,
                previous_executable,
            } => JournalEntry::Remove {
                previous: store
                    .save_previous(previous)
                    .map_err(|error| rejected(ComposeProblem::Store(error)))?,
                previous_executable: *previous_executable,
            },
        };
        entries.push((path.clone(), entry));
    }
    if let Some(declaration) = &declaration {
        entries.push((
            declaration_path(),
            JournalEntry::Modify {
                blob: BlobId::of(declaration.text),
                executable: false,
                previous: store
                    .save_previous(declaration.previous)
                    .map_err(|error| rejected(ComposeProblem::Store(error)))?,
                previous_executable: false,
            },
        ));
    }
    let journal = Journal {
        base: snapshot.identity(),
        files: entries,
        directories: directories.to_vec(),
        pending: None,
    };
    if let Err(error) = fault(Step::Journal) {
        return Err(write_failed(WriteStep::Journal, error));
    }
    store
        .begin(&journal)
        .map_err(|error| rejected(ComposeProblem::Store(error)))?;
    for directory in directories {
        let path = root.join(directory.as_str());
        if let Err(error) = fs::create_dir(&path) {
            return Err(write_failed(WriteStep::File(path), error));
        }
    }
    // Creations and replacements first, removals last: a removal empties
    // and removes directories, which a file created later may need.
    let removal = |change: &Change| matches!(change, Change::Remove { .. });
    let ordered = changes
        .iter()
        .filter(|(_, change)| !removal(change))
        .chain(changes.iter().filter(|(_, change)| removal(change)));
    for (path, change) in ordered {
        let target = root.join(path.as_str());
        let done = fault(Step::Files).and_then(|()| match change {
            Change::Create(file) | Change::Modify { file, .. } => put_file(&target, file),
            Change::Remove { .. } => {
                fs::remove_file(&target)?;
                prune_directories(root, path);
                Ok(())
            }
        });
        if let Err(error) = done {
            return Err(write_failed(WriteStep::File(target), error));
        }
    }
    // The declaration is the intent the lock then records, so it is
    // written before the lock: an interruption between them reverts to the
    // previous declaration with the previous files.
    if let Some(declaration) = &declaration {
        let written = fault(Step::Declaration)
            .and_then(|()| write_in_place(declaration.guard, declaration.text));
        if let Err(error) = written {
            return Err(write_failed(WriteStep::Declaration, error));
        }
    }
    if !marked.is_empty() {
        // Everything is written but the lock. The journal now says so, and
        // keeps the lock `--continue` writes: the next composition waits
        // for the consumer instead of reverting.
        let other = |error: StoreError| write_failed(WriteStep::Pending, io::Error::other(error));
        let kept = store.save_previous(lock_bytes).map_err(other)?;
        let waiting = Journal {
            pending: Some(Pending {
                lock: kept,
                files: marked.iter().cloned().collect(),
                declaration: Some(declaration_used.clone()),
            }),
            ..journal
        };
        if let Err(error) = fault(Step::Pending) {
            return Err(write_failed(WriteStep::Pending, error));
        }
        store.begin(&waiting).map_err(other)?;
        return Ok(store.restored());
    }
    write_lock(root, lock_bytes, fault)?;
    let finished = fault(Step::Finish).and_then(|()| {
        store.finish().map_err(|error| match error {
            StoreError::Io { error, .. } => error,
            other => io::Error::other(other.to_string()),
        })
    });
    if let Err(error) = finished {
        return Err(write_failed(WriteStep::Finish, error));
    }
    Ok(store.restored())
}

/// Replaces the lockfile with `lock_bytes`, staged beside it and renamed
/// into place.
fn write_lock(
    root: &Path,
    lock_bytes: &[u8],
    fault: &mut dyn FnMut(Step) -> io::Result<()>,
) -> Result<(), ComposeFailure> {
    let staged = root.join(LOCK_STAGING);
    let lock_path = root.join(LOCKFILE);
    let locked = fault(Step::Lock)
        .and_then(|()| {
            let _ = fs::remove_file(&staged);
            write_new(&staged, lock_bytes)
        })
        .and_then(|()| fs::rename(&staged, &lock_path));
    if let Err(error) = locked {
        let _ = fs::remove_file(&staged);
        return Err(ComposeFailure::Write {
            step: WriteStep::Lock,
            error,
        });
    }
    store::sync_dir(root);
    Ok(())
}

/// Recovers from the interrupted composition `journal` records. The lock
/// is written after every file, so a lock that records the journal's base
/// means the composition was complete except for the journal's removal,
/// and it is completed: only the journal goes. Otherwise the composition
/// is reverted, file by file: a created file whose content is still the
/// planned one is removed, a replaced one is restored from its previous
/// content, and a removed one is put back; a file with other content is
/// left, since it is the consumer's now. The directories the composition
/// created are removed when empty, innermost first, and the journal last.
/// The `resolving` files are the exception: they are restored whatever
/// they hold, since an abort gives up the resolution.
fn recover(
    root: &Path,
    store: &Store,
    journal: &Journal,
    guard: &fs::File,
    resolving: &[RelativePath],
) -> Result<ComposeRecovery, StoreError> {
    if let Ok(Some((
        _,
        Lockfile {
            project: Some(state),
            ..
        },
    ))) = lockfile::read_lockfile(root)
    {
        if *state.base() == journal.base {
            let _ = fs::remove_file(root.join(LOCK_STAGING));
            store.finish()?;
            return Ok(ComposeRecovery::Completed);
        }
    }
    let mut removed = Vec::new();
    let mut restored = Vec::new();
    let mut kept = Vec::new();
    for (path, entry) in &journal.files {
        let target = root.join(path.as_str());
        let _ = fs::remove_file(staging_path(&target));
        // The declaration is read and written through the handle that
        // holds its lock, never replaced.
        let is_declaration = *path == declaration_path();
        let held = if is_declaration {
            read_bytes(guard)
                .map(Held::File)
                .map_err(|error| StoreError::Io {
                    path: target.clone(),
                    error,
                })?
        } else {
            held_content(&target)?
        };
        let put_back = |blob: &BlobId, executable: bool| -> Result<(), StoreError> {
            let bytes = store.read_previous(blob)?;
            if is_declaration {
                return write_in_place(guard, &bytes).map_err(|error| StoreError::Io {
                    path: target.clone(),
                    error,
                });
            }
            if let Some(parent) = target.parent() {
                let _ = fs::create_dir_all(parent);
            }
            put_file(&target, &UnitFile::new(bytes, executable)).map_err(|error| StoreError::Io {
                path: target.clone(),
                error,
            })
        };
        match entry {
            JournalEntry::Create { blob } => match held {
                // A created file the consumer was resolving holds their
                // edits, which an abort gives up with the rest.
                Held::File(bytes) if BlobId::of(&bytes) == *blob || resolving.contains(path) => {
                    fs::remove_file(&target).map_err(|error| StoreError::Io {
                        path: target.clone(),
                        error,
                    })?;
                    removed.push(path.clone());
                }
                Held::Absent => {}
                Held::File(_) | Held::Other => kept.push(path.clone()),
            },
            JournalEntry::Modify {
                blob,
                previous,
                previous_executable,
                ..
            } => match held {
                // A file the consumer was resolving holds their edits, not
                // the planned content, and an abort restores it anyway.
                Held::File(_) | Held::Absent if resolving.contains(path) => {
                    put_back(previous, *previous_executable)?;
                    restored.push(path.clone());
                }
                Held::File(bytes) if BlobId::of(&bytes) == *blob => {
                    put_back(previous, *previous_executable)?;
                    restored.push(path.clone());
                }
                Held::File(bytes) if BlobId::of(&bytes) == *previous => {}
                Held::File(_) | Held::Other | Held::Absent => kept.push(path.clone()),
            },
            JournalEntry::Remove {
                previous,
                previous_executable,
            } => match held {
                Held::Absent => {
                    put_back(previous, *previous_executable)?;
                    restored.push(path.clone());
                }
                Held::File(bytes) if BlobId::of(&bytes) == *previous => {}
                Held::File(_) | Held::Other => kept.push(path.clone()),
            },
        }
    }
    for directory in journal.directories.iter().rev() {
        let _ = fs::remove_dir(root.join(directory.as_str()));
    }
    let _ = fs::remove_file(root.join(LOCK_STAGING));
    store.finish()?;
    Ok(ComposeRecovery::Reverted {
        removed,
        restored,
        kept,
    })
}

/// What a path holds during recovery.
enum Held {
    Absent,
    File(Vec<u8>),
    Other,
}

fn held_content(target: &Path) -> Result<Held, StoreError> {
    match fs::symlink_metadata(target) {
        Ok(metadata) if metadata.is_file() => {
            fs::read(target)
                .map(Held::File)
                .map_err(|error| StoreError::Io {
                    path: target.to_owned(),
                    error,
                })
        }
        Ok(_) => Ok(Held::Other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Held::Absent),
        Err(error) => Err(StoreError::Io {
            path: target.to_owned(),
            error,
        }),
    }
}

/// The delivered paths, or their directories, that already exist in the
/// project, sorted.
fn existing_paths<'a>(
    root: &Path,
    paths: impl Iterator<Item = &'a RelativePath>,
) -> Result<Vec<RelativePath>, ComposeProblem> {
    let mut existing = BTreeSet::new();
    let mut checked: BTreeSet<String> = BTreeSet::new();
    for path in paths {
        let segments: Vec<&str> = path.segments().collect();
        let mut prefix = String::new();
        for (index, segment) in segments.iter().enumerate() {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(segment);
            let last = index + 1 == segments.len();
            if !last && !checked.insert(prefix.clone()) {
                continue;
            }
            let target = root.join(&prefix);
            match fs::symlink_metadata(&target) {
                // A real directory on the way is fine; anything else at a
                // delivered path or on the way to one is not.
                Ok(metadata)
                    if !last && metadata.is_dir() && !metadata.file_type().is_symlink() => {}
                Ok(_) => {
                    if let Ok(found) = RelativePath::parse(&prefix) {
                        existing.insert(found);
                    }
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => break,
                Err(error) => {
                    return Err(ComposeProblem::Io {
                        path: target,
                        error,
                    });
                }
            }
        }
    }
    Ok(existing.into_iter().collect())
}

/// Whether `path` is one Assemblr keeps for itself, or lies in Git's own
/// directory at any depth, which a composition must never write into: a
/// `.git` there is the project's or a nested repository's.
fn is_reserved(path: &RelativePath) -> bool {
    let text = path.as_str();
    text == DECLARATION
        || text == LOCKFILE
        || text == STATE_DIR
        || text.starts_with(&format!("{STATE_DIR}/"))
        || path
            .segments()
            .any(|segment| segment.eq_ignore_ascii_case(".git"))
}

/// The project's own file at `path`, a path the composition creates: its
/// bytes and executable bit, when the path holds a regular file reached
/// through real directories. `None` when nothing is there, or when what is
/// there is not such a file, which [`existing_paths`] reports.
fn existing_file(
    root: &Path,
    path: &RelativePath,
    incoming_executable: bool,
) -> Result<Option<(Vec<u8>, bool)>, ComposeProblem> {
    let io_problem = |path: PathBuf, error| ComposeProblem::Io { path, error };
    let mut target = root.to_owned();
    let segments: Vec<&str> = path.segments().collect();
    for (index, segment) in segments.iter().enumerate() {
        target.push(segment);
        let metadata = match fs::symlink_metadata(&target) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_problem(target, error)),
        };
        if index + 1 < segments.len() {
            if !metadata.is_dir() {
                return Ok(None);
            }
        } else if metadata.is_file() {
            let bytes = fs::read(&target).map_err(|error| io_problem(target.clone(), error))?;
            return Ok(Some((
                bytes,
                executable_bit(&metadata, incoming_executable),
            )));
        }
    }
    Ok(None)
}

/// Creates `target` new with `file`'s bytes and executable bit, flushed.
fn write_file(target: &Path, file: &UnitFile) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if file.is_executable() { 0o755 } else { 0o644 });
    }
    let mut handle = options.open(target)?;
    handle.write_all(file.contents())?;
    handle.sync_all()
}

/// Where a file is written before it is put in place.
fn staging_path(target: &Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map(ToOwned::to_owned)
        .unwrap_or_default();
    name.push(".assemblr-new");
    target.with_file_name(name)
}

/// Puts `file` at `target`, whether or not something is there: it is
/// written next to it and renamed into place, so that `target` holds
/// either what it held or the whole of `file`, never part of it.
fn put_file(target: &Path, file: &UnitFile) -> io::Result<()> {
    let staged = staging_path(target);
    let _ = fs::remove_file(&staged);
    write_file(&staged, file)?;
    fs::rename(&staged, target)
}

/// Removes the directories of `path` that are empty, innermost first,
/// never the project's root.
fn prune_directories(root: &Path, path: &RelativePath) {
    let mut segments: Vec<&str> = path.segments().collect();
    segments.pop();
    while !segments.is_empty() {
        if fs::remove_dir(root.join(segments.join("/"))).is_err() {
            break;
        }
        segments.pop();
    }
}

/// Reads a text file, bounded like a manifest.
fn read_text(path: &Path) -> io::Result<String> {
    read_handle(&fs::File::open(path)?)
}

/// Reads an open text file, bounded like a manifest. The declaration is
/// read through the handle that holds its lock: on Windows the lock is
/// mandatory, and another handle could not read the file.
fn read_handle(file: &fs::File) -> io::Result<String> {
    let mut bytes = Vec::new();
    file.take(MANIFEST_LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MANIFEST_LIMIT {
        return Err(io::Error::other(format!(
            "it is larger than the {MANIFEST_LIMIT} bytes Assemblr reads"
        )));
    }
    String::from_utf8(bytes).map_err(|_| io::Error::other("it is not UTF-8 text"))
}

/// All of an open file's bytes, bounded like a manifest.
fn read_bytes(file: &fs::File) -> io::Result<Vec<u8>> {
    let mut file = file;
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.take(MANIFEST_LIMIT + 1).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Replaces an open file's content through the handle itself. A rename
/// would put another file where the declaration was and leave the lock on
/// one that is no longer it; the journal holds the previous content, so
/// that an interruption does not lose it.
fn write_in_place(file: &fs::File, bytes: &[u8]) -> io::Result<()> {
    let mut file = file;
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// The declaration's path as the journal names it.
fn declaration_path() -> RelativePath {
    // A constant that is a valid path by construction.
    #[allow(clippy::expect_used, reason = "DECLARATION is a valid relative path")]
    RelativePath::parse(DECLARATION).expect("the declaration's name is a valid relative path")
}

/// Plans what `add` or `remove` does to the declaration `text`: the edited
/// text, the project it declares, and what changed. For an `add` without a
/// requirement the resolution says which version to record.
fn plan_declaration(
    edit: &DeclarationEdit,
    text: &str,
    previous_lock: &UnitLock,
    index: &RegistryIndex,
) -> Result<PlannedDeclaration, Vec<ComposeProblem>> {
    let cannot = |detail: String| vec![ComposeProblem::CannotEdit(detail)];
    let (edited, change) = match edit {
        DeclarationEdit::Add {
            identity,
            requirement,
            options,
        } => {
            let requirement = match requirement {
                // `add <unit>@1.2.3` records `^1.2.3`, as `npm install`
                // does: the file always spells its operator.
                Some(requirement) if Version::parse(requirement).is_ok() => {
                    format!("^{requirement}")
                }
                Some(requirement) => {
                    VersionRequirement::parse(requirement).map_err(|error| {
                        vec![ComposeProblem::InvalidRequirement(error.to_string())]
                    })?;
                    requirement.clone()
                }
                None => match declaration::selected_requirement(text, identity) {
                    // Already selected: its own requirement stays.
                    Some(selected) => selected,
                    None => probe_requirement(text, identity, previous_lock, index)?,
                },
            };
            let (edited, change) = declaration::add(text, identity, &requirement).map_err(
                |failure| match failure {
                    declaration::EditFailure::Cannot(detail) => cannot(detail),
                    declaration::EditFailure::NotSelected => {
                        cannot("the unit is not selected".to_owned())
                    }
                },
            )?;
            let types = if options.is_empty() {
                BTreeMap::new()
            } else {
                parameter_types(&edited, identity, previous_lock, index)
            };
            let values: Vec<(String, crate::domain::Value)> = options
                .iter()
                .map(|(name, raw)| {
                    (
                        name.clone(),
                        declaration::option_value(raw, types.get(name.as_str())),
                    )
                })
                .collect();
            let edited =
                declaration::set_options(&edited, identity, &values).map_err(|failure| {
                    match failure {
                        declaration::EditFailure::Cannot(detail) => cannot(detail),
                        declaration::EditFailure::NotSelected => {
                            cannot("the unit is not selected".to_owned())
                        }
                    }
                })?;
            (edited, change)
        }
        DeclarationEdit::Remove { identity } => {
            declaration::remove(text, identity).map_err(|failure| match failure {
                declaration::EditFailure::NotSelected => vec![ComposeProblem::NotSelected {
                    identity: identity.clone(),
                    transitive: previous_lock.units().contains_key(identity),
                }],
                declaration::EditFailure::Cannot(detail) => cannot(detail),
            })?
        }
    };
    let project = read_project(&edited)
        .map_err(|error| cannot(format!("the edited declaration is not valid: {error}")))?;
    Ok(PlannedDeclaration {
        project,
        text: edited,
        change,
    })
}

/// The type of each parameter `identity` declares at the version the
/// declaration `text` resolves it to, so that `--option` values are read
/// by type. Empty when the declaration does not resolve: the composition
/// reports why.
fn parameter_types(
    text: &str,
    identity: &UnitIdentity,
    previous_lock: &UnitLock,
    index: &RegistryIndex,
) -> BTreeMap<String, crate::domain::ParameterType> {
    let Ok(project) = read_project(text) else {
        return BTreeMap::new();
    };
    let mut ignored = Vec::new();
    let Some(resolved) = resolve_graph(
        None,
        project.dependencies(),
        previous_lock,
        index,
        &mut ignored,
    ) else {
        return BTreeMap::new();
    };
    let Some(locked) = resolved.lock.units().get(identity) else {
        return BTreeMap::new();
    };
    resolved
        .dependencies
        .manifest(identity, locked.version())
        .map(|manifest| {
            manifest
                .params()
                .iter()
                .map(|(name, definition)| {
                    (
                        name.as_str().to_owned(),
                        definition.parameter_type().clone(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The requirement an `add` without one records: a caret requirement on
/// the version the resolution picks for the unit when it is selected with
/// any stable version, which is the one already locked when the unit is
/// present transitively.
fn probe_requirement(
    text: &str,
    identity: &UnitIdentity,
    previous_lock: &UnitLock,
    index: &RegistryIndex,
) -> Result<String, Vec<ComposeProblem>> {
    let cannot = |detail: String| vec![ComposeProblem::CannotEdit(detail)];
    let (probe_text, _) =
        declaration::add(text, identity, "*").map_err(|failure| match failure {
            declaration::EditFailure::Cannot(detail) => cannot(detail),
            declaration::EditFailure::NotSelected => cannot("the unit is not selected".to_owned()),
        })?;
    let probe = read_project(&probe_text)
        .map_err(|error| cannot(format!("the edited declaration is not valid: {error}")))?;
    let mut found = Vec::new();
    let Some(resolved) =
        resolve_graph(None, probe.dependencies(), previous_lock, index, &mut found)
    else {
        return Err(found
            .into_iter()
            .map(|problem| ComposeProblem::Build(Box::new(problem)))
            .collect());
    };
    match resolved.lock.units().get(identity) {
        Some(locked) => Ok(format!("^{}", locked.version())),
        None => Err(cannot(format!(
            "the resolution did not select {identity}, so there is no version to record"
        ))),
    }
}

/// A composed project.
#[derive(Debug)]
pub struct Composed {
    units: usize,
    files: usize,
    changes: BTreeMap<UnitIdentity, LockChange>,
    paths: BTreeMap<RelativePath, FileChange>,
    base: BaseIdentity,
    unbased: BTreeSet<RelativePath>,
    notes: Vec<String>,
    unwritten_notes: Vec<String>,
    crossed: Vec<String>,
    recovered: Option<ComposeRecovery>,
    unchanged: bool,
    first: bool,
    declaration: Option<DeclarationChange>,
    pending: BTreeSet<RelativePath>,
    consumer_conflicts: BTreeSet<RelativePath>,
    unit_conflicts: Vec<(RelativePath, String)>,
}

impl Composed {
    /// The conflicts between units that no unit above them resolves and
    /// that the project holds between markers, each with the delivered
    /// path and the report of what the units contend. Every such path is
    /// pending; a conflict the consumer resolved earlier, which this
    /// composition keeps, is not listed.
    pub fn unit_conflicts(&self) -> &[(RelativePath, String)] {
        &self.unit_conflicts
    }

    /// The pending files that hold a conflict between the consumer's
    /// changes and the composition, between `current`, `base`, and
    /// `incoming` markers. A file can hold both kinds.
    pub fn consumer_conflicts(&self) -> &BTreeSet<RelativePath> {
        &self.consumer_conflicts
    }

    /// How many units, direct and transitive, are composed.
    pub fn units(&self) -> usize {
        self.units
    }

    /// How many files the composition holds.
    pub fn files(&self) -> usize {
        self.files
    }

    /// What happened to each path the composition changed in the project.
    pub fn paths(&self) -> &BTreeMap<RelativePath, FileChange> {
        &self.paths
    }

    /// How the lock changed, by identity.
    pub fn changes(&self) -> &BTreeMap<UnitIdentity, LockChange> {
        &self.changes
    }

    /// The identity of the materialized composition, the reconciliation
    /// base of the next one.
    pub fn base(&self) -> &BaseIdentity {
        &self.base
    }

    /// The written files whose base is not stored, because the project's
    /// version control ignores them.
    pub fn unbased(&self) -> &BTreeSet<RelativePath> {
        &self.unbased
    }

    /// What was reported without failing. What the merge reports about a
    /// file is here only when this composition wrote the file.
    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// What the merge reports about the files this composition did not
    /// write, the same every time the composition is the same.
    pub fn unwritten_notes(&self) -> &[String] {
        &self.unwritten_notes
    }

    /// The amendments applied to another version of their provider than the
    /// one they were taken from, one line each, by delivered path.
    pub fn crossed(&self) -> &[String] {
        &self.crossed
    }

    /// The interrupted composition that was recovered from first, if any.
    pub fn recovered(&self) -> Option<&ComposeRecovery> {
        self.recovered.as_ref()
    }

    /// What `add` or `remove` did to the declaration, or `None` for a
    /// plain composition.
    pub fn declaration(&self) -> Option<&DeclarationChange> {
        self.declaration.as_ref()
    }

    /// Whether the project already held this composition, so that nothing
    /// was written.
    pub fn unchanged(&self) -> bool {
        self.unchanged
    }

    /// Whether this was the project's first composition, with no base to
    /// reconcile against.
    pub fn first(&self) -> bool {
        self.first
    }

    /// The files that hold conflict markers. When there are any, the
    /// composition waits for `compose --continue` or `--abort`, and the
    /// lock is not written yet.
    pub fn pending(&self) -> &BTreeSet<RelativePath> {
        &self.pending
    }
}

/// What a composition did to a path of the project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChange {
    /// Created it.
    Created,
    /// Replaced its content, or its mode, with the composition's.
    Modified,
    /// Merged the composition's changes into the project's edits.
    Merged,
    /// Removed it.
    Removed,
    /// Left its conflict between the project's content and the
    /// composition's marked in the file, for the consumer to resolve.
    Conflicted,
}

/// What recovering from an interrupted composition did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposeRecovery {
    /// The composition had written its files and its lock; only the
    /// journal was left, and it was removed.
    Completed,
    /// The composition was reverted.
    Reverted {
        /// The files removed, still holding what the interrupted
        /// composition wrote.
        removed: Vec<RelativePath>,
        /// The files restored to the content they had before.
        restored: Vec<RelativePath>,
        /// The files kept, because their content is neither what was
        /// written nor what was there before.
        kept: Vec<RelativePath>,
    },
}

/// Why a composition failed.
#[derive(Debug)]
pub enum ComposeFailure {
    /// The composition was rejected before anything was written. Every
    /// problem found is listed.
    Rejected {
        /// The problems.
        problems: Vec<ComposeProblem>,
        /// The interrupted composition that was recovered from first, if
        /// any.
        recovered: Option<ComposeRecovery>,
    },
    /// Writing failed. After the journal, it remains, and the next
    /// composition completes or reverts what was written.
    Write {
        /// What failed.
        step: WriteStep,
        /// Why.
        error: io::Error,
    },
}

/// Where writing failed.
#[derive(Debug)]
pub enum WriteStep {
    /// Writing the reconciliation base to the store; nothing else was
    /// written.
    Store,
    /// Keeping the previous content of the files that change; nothing of
    /// the project was written.
    Previous,
    /// Writing the journal; nothing of the project was written.
    Journal,
    /// Writing a project file; the journal records the composition.
    File(PathBuf),
    /// Writing the declaration, which `add` and `remove` change after the
    /// files; the journal records the composition.
    Declaration,
    /// Recording that the composition waits for the consumer's resolution;
    /// the files are written, the journal remains.
    Pending,
    /// Writing the lockfile; the files are written, the journal remains.
    Lock,
    /// Removing the journal; the composition is complete, and the next
    /// composition removes the journal.
    Finish,
}

impl fmt::Display for ComposeFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected { problems, .. } => {
                for (index, problem) in problems.iter().enumerate() {
                    if index > 0 {
                        f.write_str("\n")?;
                    }
                    problem.fmt(f)?;
                }
                Ok(())
            }
            Self::Write { step, error } => match step {
                WriteStep::Store => write!(f, "could not store the reconciliation base: {error}"),
                WriteStep::Previous => write!(
                    f,
                    "could not keep the previous content of the files that change: {error}"
                ),
                WriteStep::Journal => write!(f, "could not write the journal: {error}"),
                WriteStep::File(path) => write!(f, "could not write {}: {error}", path.display()),
                WriteStep::Declaration => write!(f, "could not write {DECLARATION}: {error}"),
                WriteStep::Pending => write!(
                    f,
                    "could not record the composition as waiting for conflict resolution: {error}"
                ),
                WriteStep::Lock => write!(f, "could not write {LOCKFILE}: {error}"),
                WriteStep::Finish => write!(f, "could not remove the journal: {error}"),
            },
        }
    }
}

impl std::error::Error for ComposeFailure {}

/// A problem that rejects a composition.
#[derive(Debug)]
pub enum ComposeProblem {
    /// There is no `assemblr.jsonc`: the directory is not a project's top
    /// level.
    NoDeclaration(PathBuf),
    /// The directory holds a unit's manifest.
    UnitRepository(PathBuf),
    /// A file could not be read or written.
    Io {
        /// The file.
        path: PathBuf,
        /// Why.
        error: io::Error,
    },
    /// `assemblr.jsonc` is not a valid declaration.
    InvalidDeclaration {
        /// The declaration.
        path: PathBuf,
        /// Why.
        error: ReadManifestError,
    },
    /// The lockfile cannot be read.
    Lockfile(LockfileError),
    /// The lockfile is a unit's, with no reconciliation state.
    ForeignLockfile(PathBuf),
    /// The stored reconciliation base does not verify.
    Base(StoreError),
    /// The store could not be used.
    Store(StoreError),
    /// A problem a unit build reports too, with the same wording.
    Build(Box<BuildProblem>),
    /// Delivered paths Assemblr reserves for its own files.
    Reserved(Vec<RelativePath>),
    /// Delivered paths, or directories on the way to them, that already
    /// hold something that cannot be marked as a conflict: a binary file, a
    /// directory, or a link.
    Existing(Vec<RelativePath>),
    /// The project's ignore rules could not be read.
    Ignore(IgnoreError),
    /// `update` found no lock: the project was never composed.
    NothingLocked,
    /// `update` named a unit that is not locked.
    NotLocked {
        /// The unit.
        unit: UnitIdentity,
        /// The locked units.
        locked: Vec<UnitIdentity>,
    },
    /// `remove` named a unit that is not an explicit selection.
    NotSelected {
        /// The unit.
        identity: UnitIdentity,
        /// Whether it is in the composition anyway, because a selected
        /// unit requires it.
        transitive: bool,
    },
    /// The requirement `add` was given is not a valid SemVer requirement.
    InvalidRequirement(String),
    /// The declaration could not be edited safely.
    CannotEdit(String),
    /// Delivered files that only the consumer's own Git rules ignore, and
    /// that the base would store for the first time.
    LocallyIgnored(Vec<(RelativePath, String)>),
    /// The project and the declared composition changed the same files
    /// in ways that need the consumer.
    Conflicts(Vec<FileConflict>),
    /// A composition waits for the consumer to resolve the conflicts
    /// marked in these files.
    Pending(Vec<RelativePath>),
    /// `compose --continue` found the declaration changed since the
    /// waiting composition was planned from it.
    DeclarationChanged,
    /// `compose --continue` or `--abort` found no composition waiting.
    NothingPending,
    /// `compose --continue` found an interrupted composition, not one
    /// waiting for the consumer.
    NotPending,
    /// `compose --continue` found files that are not resolved, with why.
    Unresolved(Vec<(RelativePath, String)>),
}

/// A file the consumer must settle.
#[derive(Debug)]
pub struct FileConflict {
    path: RelativePath,
    kind: FileConflictKind,
}

impl FileConflict {
    /// The file.
    pub fn path(&self) -> &RelativePath {
        &self.path
    }

    /// What is in conflict.
    pub fn kind(&self) -> &FileConflictKind {
        &self.kind
    }
}

/// What is in conflict at a file.
#[derive(Debug)]
pub enum FileConflictKind {
    /// The sides disagree, as the reconciliation decides.
    Disagreement(Disagreement),
    /// The project and the composition change the same values of a
    /// structured file.
    Values(Vec<Conflict<Contributor>>),
    /// The project and the composition change the same lines of a text
    /// file, in this many places.
    Lines {
        /// How many places conflict.
        places: usize,
    },
    /// Both sides changed the file, and the merge could not be done.
    Unmergeable(String),
}

impl fmt::Display for FileConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.path)?;
        match &self.kind {
            FileConflictKind::Disagreement(Disagreement::BothChanged(Mergeability::Format)) => f
                .write_str(
                    "the project and the composition both changed it, and its format has no merge",
                ),
            FileConflictKind::Disagreement(Disagreement::BothChanged(
                Mergeability::BaseNotStored,
            )) => f.write_str(
                "the project and the composition both changed it, and its base is not stored \
                 (the file was ignored by version control when it was composed), so the changes \
                 cannot be told apart",
            ),
            FileConflictKind::Disagreement(Disagreement::BothChanged(Mergeability::Mergeable)) => {
                f.write_str("the project and the composition both changed it")
            }
            FileConflictKind::Disagreement(Disagreement::RemovedButEdited) => {
                f.write_str("the composition no longer delivers it, and the project edited it")
            }
            FileConflictKind::Disagreement(Disagreement::DeletedButChanged) => {
                f.write_str("the project deleted it, and the composition changes it")
            }
            FileConflictKind::Disagreement(Disagreement::NotAFile) => {
                f.write_str("it is not a regular file in the project")
            }
            FileConflictKind::Lines { places } => write!(
                f,
                "the project and the composition change the same lines, in {places} place(s)"
            ),
            FileConflictKind::Values(conflicts) => {
                f.write_str("the project and the composition change the same values:")?;
                for conflict in conflicts {
                    write!(f, "\n    {conflict}")?;
                }
                Ok(())
            }
            FileConflictKind::Unmergeable(detail) => write!(
                f,
                "the project and the composition both changed it, and the changes could not be \
                 merged: {detail}"
            ),
        }
    }
}

impl fmt::Display for ComposeProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDeclaration(path) => match path.parent() {
                Some(directory) => write!(f, "no {DECLARATION} in {}", directory.display()),
                None => write!(f, "{} does not exist", path.display()),
            },
            Self::UnitRepository(path) => write!(
                f,
                "{} holds `manifest.jsonc`, so it is a unit's repository, not a consumer project",
                path.display()
            ),
            Self::Io { path, error } => write!(f, "could not use {}: {error}", path.display()),
            Self::InvalidDeclaration { path, error } => write!(f, "{}: {error}", path.display()),
            Self::Lockfile(error) => error.fmt(f),
            Self::ForeignLockfile(path) => write!(
                f,
                "{} is a unit's lockfile, without a project's reconciliation state; a consumer \
                 project's lockfile is written by `assemblr compose`",
                path.display()
            ),
            Self::Base(error) | Self::Store(error) => error.fmt(f),
            Self::Build(problem) => problem.fmt(f),
            Self::Reserved(paths) => {
                write!(
                    f,
                    "the composition delivers {} path(s) Assemblr reserves for its own files:",
                    paths.len()
                )?;
                for path in paths {
                    write!(f, "\n  {path}")?;
                }
                Ok(())
            }
            Self::Existing(paths) => {
                write!(
                    f,
                    "the composition would write {} path(s) that already hold something Assemblr did \
                     not write and cannot mark as a conflict:",
                    paths.len()
                )?;
                for path in paths {
                    write!(f, "\n  {path}")?;
                }
                Ok(())
            }
            Self::Ignore(error) => error.fmt(f),
            Self::NothingLocked => f.write_str(
                "this project was never composed, so there are no locked versions to update",
            ),
            Self::NotLocked { unit, .. } => write!(
                f,
                "{unit} is not part of the composition this project holds, so it has no locked \
                 version to update"
            ),
            Self::NotSelected {
                identity,
                transitive,
            } => {
                write!(f, "{identity} is not an explicit selection of this project")?;
                if *transitive {
                    write!(
                        f,
                        "; it is in the composition only because a selected unit requires it, and \
                         leaves with that unit"
                    )?;
                }
                Ok(())
            }
            Self::InvalidRequirement(error) => {
                write!(f, "the version requirement is not valid: {error}")
            }
            Self::CannotEdit(detail) => {
                write!(f, "{DECLARATION} could not be edited safely: {detail}")
            }
            Self::LocallyIgnored(paths) => {
                write!(
                    f,
                    "the composition would store the content of {} file(s) that only your own Git \
                     rules ignore, not the project's .gitignore, in the tracked base:",
                    paths.len()
                )?;
                for (path, rule) in paths {
                    write!(
                        f,
                        "
  {path} ({rule})"
                    )?;
                }
                Ok(())
            }
            Self::Conflicts(conflicts) => {
                write!(
                    f,
                    "the project and the declared composition disagree at {} path(s); only a \
                     conflict between text content can be left in the file for you to resolve, \
                     and a composition with any other conflict is refused whole:",
                    conflicts.len()
                )?;
                for conflict in conflicts {
                    write!(f, "\n  {conflict}")?;
                }
                Ok(())
            }
            Self::Pending(paths) => {
                write!(
                    f,
                    "a composition is waiting for you to resolve its conflicts in {} file(s):",
                    paths.len()
                )?;
                for path in paths {
                    write!(f, "\n  {path}")?;
                }
                Ok(())
            }
            Self::DeclarationChanged => f.write_str(
                "assemblr.jsonc changed since the composition that waits for your resolution was \
                 planned from it, so the lock it would write no longer records what the \
                 declaration says",
            ),
            Self::NothingPending => {
                f.write_str("no composition is waiting for conflict resolution")
            }
            Self::NotPending => f.write_str(
                "no composition is waiting for conflict resolution; an interrupted one is recorded \
                 in the journal",
            ),
            Self::Unresolved(files) => {
                write!(f, "{} file(s) are not resolved yet:", files.len())?;
                for (path, why) in files {
                    write!(f, "\n  {path}: {why}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ComposeProblem {}

#[cfg(test)]
mod tests;
