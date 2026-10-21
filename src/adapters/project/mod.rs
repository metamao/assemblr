//! Consumer projects: `assemblr init` and the first `assemblr compose`
//! (see `docs/domain/consumer-projects.md` and
//! `docs/adr/0016-consumer-compose.md`).
//!
//! `init` creates `assemblr.jsonc`, the editable declaration of the
//! project's explicit selections and their options, and nothing else: no
//! Git repository, no files, no commands run. `compose` resolves the
//! declared units from the local registry under the project's lock,
//! composes and renders their files as a unit build does with the project
//! as the root, and materializes them into the project together with the
//! lock, which records the resolved versions and the reconciliation
//! state, and the stored reconciliation base. Everything is decided before
//! anything is written, the writes are journaled so that an interrupted
//! composition is reverted by the next one, and a second composition that
//! would change nothing changes nothing. Evolving a composition that is
//! already materialized is not supported yet and is refused explicitly.

mod ignore;
mod store;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use ignore::IgnoreError;
pub use store::StoreError;
use store::{Journal, STATE_DIR, Store, write_new};

use super::build::{BuildProblem, render_files, resolve_graph};
use super::git::MANIFEST_LIMIT;
use super::lockfile::{self, LOCKFILE, Lockfile, LockfileError, serialize_lockfile};
use super::manifest::{ReadManifestError, read_project};
use super::registry::Registry;
use crate::domain::{
    BaseIdentity, LockChange, ReconciliationState, RelativePath, Root, Snapshot, SnapshotFile,
    UnitFile, UnitIdentity, compose,
};

/// The project's declaration, at its top level.
pub const DECLARATION: &str = "assemblr.jsonc";
/// A unit's manifest, which a consumer project's directory must not hold.
const MANIFEST: &str = "manifest.jsonc";
/// Where a new lockfile is written before it replaces the old one.
const LOCK_STAGING: &str = ".assemblr-lock.json.new";
/// The text `init` writes.
const INITIAL_DECLARATION: &str = "{\n  \"dependencies\": {},\n  \"options\": {}\n}\n";

/// Initializes a consumer project in `directory`, which is created when it
/// does not exist.
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
    write_new(&declaration, INITIAL_DECLARATION.as_bytes()).map_err(|error| {
        InitProjectFailure::Io {
            path: declaration.clone(),
            error,
        }
    })?;
    Ok(ProjectInitialized {
        directory,
        created: true,
        directory_created: !exists,
    })
}

/// An initialized project.
#[derive(Debug)]
pub struct ProjectInitialized {
    directory: PathBuf,
    created: bool,
    directory_created: bool,
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
    compose_with(root, registry, &mut |_| Ok(()))
}

/// The steps of writing, so that tests can make each one fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Store,
    Journal,
    Files,
    Lock,
    Finish,
}

fn compose_with(
    root: &Path,
    registry: &Registry,
    fault: &mut dyn FnMut(Step) -> io::Result<()>,
) -> Result<Composed, ComposeFailure> {
    let rejected = |problem| ComposeFailure::Rejected {
        problems: vec![problem],
        reverted: None,
    };
    let declaration_path = root.join(DECLARATION);
    if fs::symlink_metadata(&declaration_path).is_err() {
        return Err(rejected(ComposeProblem::NoDeclaration(declaration_path)));
    }
    if root.join(MANIFEST).exists() {
        return Err(rejected(ComposeProblem::UnitRepository(root.to_owned())));
    }
    // Compositions of one project run one at a time: the declaration,
    // which every composition reads and none writes, carries the lock.
    let guard = OpenOptions::new()
        .read(true)
        .open(&declaration_path)
        .and_then(|file| fs4::FileExt::lock(&file).map(|()| file))
        .map_err(|error| {
            rejected(ComposeProblem::Io {
                path: declaration_path.clone(),
                error,
            })
        })?;
    let store = Store::new(root);
    let mut problems = Vec::new();
    let mut reverted = None;
    match store.journal() {
        Ok(None) => {}
        Ok(Some(journal)) => match revert(root, &store, &journal) {
            Ok(summary) => reverted = Some(summary),
            Err(error) => return Err(rejected(ComposeProblem::Store(error))),
        },
        Err(error) => return Err(rejected(ComposeProblem::Store(error))),
    }
    let rejected = |problem| ComposeFailure::Rejected {
        problems: vec![problem],
        reverted: reverted.clone(),
    };
    let rejected_all = |problems| ComposeFailure::Rejected {
        problems,
        reverted: reverted.clone(),
    };
    let project = read_text(&declaration_path)
        .map_err(|error| ComposeProblem::Io {
            path: declaration_path.clone(),
            error,
        })
        .and_then(|text| {
            read_project(&text).map_err(|error| ComposeProblem::InvalidDeclaration {
                path: declaration_path.clone(),
                error,
            })
        })
        .map_err(|problem| problems.push(problem))
        .ok();
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
    // project that was composed before.
    if let Some((_, state)) = &previous {
        if let Err(error) = store.verify_base(state.base(), state.unbased()) {
            problems.push(ComposeProblem::Base(error));
            return Err(rejected_all(problems));
        }
    }
    let previous_lock = previous
        .as_ref()
        .map(|(lock, _)| lock.clone())
        .unwrap_or_default();
    let mut found = Vec::new();
    let Some(resolved) = resolve_graph(
        None,
        project.dependencies(),
        &previous_lock,
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
    let rendered = render_files(&composition, &resolved.lock, None, &mut found);
    problems.extend(
        found
            .into_iter()
            .map(|problem| ComposeProblem::Build(Box::new(problem))),
    );
    let Some(rendered) = rendered.filter(|_| problems.is_empty()) else {
        return Err(rejected_all(problems));
    };
    let files: BTreeMap<RelativePath, UnitFile> = rendered.files.into_iter().collect();
    let mut notes = rendered.notes;

    // The paths Assemblr keeps for itself.
    let reserved: Vec<RelativePath> = files
        .keys()
        .filter(|path| {
            let text = path.as_str();
            text == DECLARATION
                || text == LOCKFILE
                || text == STATE_DIR
                || text.starts_with(&format!("{STATE_DIR}/"))
        })
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
    // one materialized; otherwise reconciling it is item 14's work.
    if let Some((_, state)) = &previous {
        if !problems.is_empty() {
            return Err(rejected_all(problems));
        }
        if *state.base() == identity {
            return Ok(Composed {
                units: resolved.lock.units().len(),
                files: 0,
                changes: BTreeMap::new(),
                base: identity,
                unbased: BTreeSet::new(),
                notes,
                reverted,
                unchanged: true,
            });
        }
        let changed = state
            .managed()
            .symmetric_difference(&files.keys().cloned().collect())
            .count();
        problems.push(ComposeProblem::Changed {
            base: state.base().clone(),
            incoming: identity,
            paths: changed,
        });
        return Err(rejected_all(problems));
    }

    // A first composition: nothing of it may exist yet.
    let (existing, mut directories) = (existing_paths(root, files.keys()), Vec::new());
    match existing {
        Ok(found) if found.is_empty() => {}
        Ok(found) => problems.push(ComposeProblem::Existing(found)),
        Err(problem) => problems.push(problem),
    }
    // Which delivered files the project's version control ignores: their
    // base is not stored, since the store is tracked.
    let path_texts: Vec<&str> = files.keys().map(RelativePath::as_str).collect();
    let mut probe = path_texts.clone();
    let directories_of_store = store.directories();
    probe.extend(directories_of_store.iter().map(String::as_str));
    let ignored = match ignore::ignored(root, &probe) {
        Ok(ignored) => ignored,
        Err(error) => {
            problems.push(ComposeProblem::Ignore(error));
            BTreeSet::new()
        }
    };
    if !problems.is_empty() {
        return Err(rejected_all(problems));
    }
    let unbased: BTreeSet<RelativePath> = files
        .keys()
        .filter(|path| ignore::is_ignored(&ignored, path))
        .cloned()
        .collect();
    for path in &unbased {
        notes.push(format!(
            "{path} is ignored by the project's version control, so its reconciliation base is \
             not stored; a later composition cannot reconcile edits to it"
        ));
    }
    if let Some(directory) = directories_of_store
        .iter()
        .find(|directory| ignored.contains(*directory))
    {
        notes.push(format!(
            "{directory} is ignored by the project's version control, so the reconciliation \
             base will not travel with the project; a clone cannot reconcile later compositions"
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
    for path in files.keys() {
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

    // Writing. The store first: content-addressed, so a leftover is
    // harmless. Then the journal, the files, the lock, and the journal's
    // removal.
    let stored: Vec<(&RelativePath, &[u8])> = files
        .iter()
        .filter(|(path, _)| !unbased.contains(*path))
        .map(|(path, file)| (path, file.contents()))
        .collect();
    let write_failed = |step: Step, error: io::Error, path: PathBuf| ComposeFailure::Write {
        step: match step {
            Step::Store => WriteStep::Store,
            Step::Journal => WriteStep::Journal,
            Step::Files => WriteStep::File(path),
            Step::Lock => WriteStep::Lock,
            Step::Finish => WriteStep::Finish,
        },
        error,
    };
    if let Err(error) = fault(Step::Store) {
        return Err(write_failed(Step::Store, error, PathBuf::new()));
    }
    store
        .write_base(&snapshot, &stored)
        .map_err(|error| rejected(ComposeProblem::Store(error)))?;
    if store.restored_attributes() {
        notes.push(format!(
            "{STATE_DIR}/.gitattributes had been edited and was restored; it keeps version \
             control from converting the stored base"
        ));
    }
    let journal = Journal {
        files: files
            .keys()
            .map(|path| (path.clone(), snapshot.files()[path].blob().clone()))
            .collect(),
        directories: directories.clone(),
    };
    if let Err(error) = fault(Step::Journal) {
        return Err(write_failed(Step::Journal, error, PathBuf::new()));
    }
    store
        .begin(&journal)
        .map_err(|error| rejected(ComposeProblem::Store(error)))?;
    for directory in &directories {
        let path = root.join(directory.as_str());
        if let Err(error) = fs::create_dir(&path) {
            return Err(write_failed(Step::Files, error, path));
        }
    }
    for (path, file) in &files {
        let target = root.join(path.as_str());
        let written = fault(Step::Files).and_then(|()| write_file(&target, file));
        if let Err(error) = written {
            return Err(write_failed(Step::Files, error, target));
        }
    }
    let staged = root.join(LOCK_STAGING);
    let lock_path = root.join(LOCKFILE);
    let locked = fault(Step::Lock)
        .and_then(|()| {
            let _ = fs::remove_file(&staged);
            write_new(&staged, &lock_bytes)
        })
        .and_then(|()| fs::rename(&staged, &lock_path));
    if let Err(error) = locked {
        let _ = fs::remove_file(&staged);
        return Err(write_failed(Step::Lock, error, lock_path));
    }
    store::sync_dir(root);
    let finished = fault(Step::Finish).and_then(|()| {
        store.finish().map_err(|error| match error {
            StoreError::Io { error, .. } => error,
            other => io::Error::other(other.to_string()),
        })
    });
    if let Err(error) = finished {
        return Err(write_failed(Step::Finish, error, PathBuf::new()));
    }
    drop(guard);
    Ok(Composed {
        units: resolved.lock.units().len(),
        files: files.len(),
        changes: resolved.resolution.changes().clone(),
        base: identity,
        unbased,
        notes,
        reverted: reverted.clone(),
        unchanged: false,
    })
}

/// Reverts what the interrupted composition `journal` records: each file
/// whose content is still the planned one is removed, the directories the
/// composition created are removed when empty, innermost first, and the
/// journal last. A file with other content is left: it is the consumer's
/// now.
fn revert(root: &Path, store: &Store, journal: &Journal) -> Result<Reverted, StoreError> {
    let mut removed = Vec::new();
    let mut kept = Vec::new();
    for (path, blob) in &journal.files {
        let target = root.join(path.as_str());
        match fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.is_file() => {
                let bytes = fs::read(&target).map_err(|error| StoreError::Io {
                    path: target.clone(),
                    error,
                })?;
                if crate::domain::BlobId::of(&bytes) == *blob {
                    fs::remove_file(&target).map_err(|error| StoreError::Io {
                        path: target.clone(),
                        error,
                    })?;
                    removed.push(path.clone());
                } else {
                    kept.push(path.clone());
                }
            }
            Ok(_) => kept.push(path.clone()),
            Err(_) => {}
        }
    }
    for directory in journal.directories.iter().rev() {
        let _ = fs::remove_dir(root.join(directory.as_str()));
    }
    let _ = fs::remove_file(root.join(LOCK_STAGING));
    store.finish()?;
    Ok(Reverted { removed, kept })
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

/// Reads a text file, bounded like a manifest.
fn read_text(path: &Path) -> io::Result<String> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MANIFEST_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MANIFEST_LIMIT {
        return Err(io::Error::other(format!(
            "it is larger than the {MANIFEST_LIMIT} bytes Assemblr reads"
        )));
    }
    String::from_utf8(bytes).map_err(|_| io::Error::other("it is not UTF-8 text"))
}

/// A composed project.
#[derive(Debug)]
pub struct Composed {
    units: usize,
    files: usize,
    changes: BTreeMap<UnitIdentity, LockChange>,
    base: BaseIdentity,
    unbased: BTreeSet<RelativePath>,
    notes: Vec<String>,
    reverted: Option<Reverted>,
    unchanged: bool,
}

impl Composed {
    /// How many units, direct and transitive, are composed.
    pub fn units(&self) -> usize {
        self.units
    }

    /// How many files were written.
    pub fn files(&self) -> usize {
        self.files
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

    /// What was reported without failing.
    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// The interrupted composition that was reverted first, if any.
    pub fn reverted(&self) -> Option<&Reverted> {
        self.reverted.as_ref()
    }

    /// Whether the project already held this composition, so that nothing
    /// was written.
    pub fn unchanged(&self) -> bool {
        self.unchanged
    }
}

/// What reverting an interrupted composition did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reverted {
    removed: Vec<RelativePath>,
    kept: Vec<RelativePath>,
}

impl Reverted {
    /// The files removed, still holding what the interrupted composition
    /// wrote.
    pub fn removed(&self) -> &[RelativePath] {
        &self.removed
    }

    /// The files kept, because their content is no longer what was written.
    pub fn kept(&self) -> &[RelativePath] {
        &self.kept
    }
}

/// Why a composition failed.
#[derive(Debug)]
pub enum ComposeFailure {
    /// The composition was rejected before anything was written. Every
    /// problem found is listed.
    Rejected {
        /// The problems.
        problems: Vec<ComposeProblem>,
        /// The interrupted composition that was reverted first, if any.
        reverted: Option<Reverted>,
    },
    /// Writing failed. The journal remains, and the next composition
    /// reverts what was written.
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
    /// Writing the journal; nothing of the project was written.
    Journal,
    /// Writing a project file; the journal records the composition.
    File(PathBuf),
    /// Writing the lockfile; the files are written, the journal remains.
    Lock,
    /// Removing the journal; the composition is complete.
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
                WriteStep::Journal => write!(f, "could not write the journal: {error}"),
                WriteStep::File(path) => write!(f, "could not write {}: {error}", path.display()),
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
    /// exist.
    Existing(Vec<RelativePath>),
    /// The project's ignore rules could not be read.
    Ignore(IgnoreError),
    /// The project was composed before, and the composition changed.
    Changed {
        /// The materialized base.
        base: BaseIdentity,
        /// The composition now declared.
        incoming: BaseIdentity,
        /// How many paths are only in one of them.
        paths: usize,
    },
}

impl fmt::Display for ComposeProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDeclaration(path) => write!(f, "{} does not exist", path.display()),
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
                    "the composition would write {} path(s) that already hold content Assemblr did \
                     not materialize:",
                    paths.len()
                )?;
                for path in paths {
                    write!(f, "\n  {path}")?;
                }
                Ok(())
            }
            Self::Ignore(error) => error.fmt(f),
            Self::Changed {
                base,
                incoming,
                paths,
            } => write!(
                f,
                "the project holds the composition {base}, and the declaration now composes \
                 {incoming} ({paths} path(s) added or removed); reconciling a project that was \
                 composed before is not supported yet"
            ),
        }
    }
}

impl std::error::Error for ComposeProblem {}
