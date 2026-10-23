//! `unit amend`: an editable copy of a dependency's file, created under
//! `amendments/` and declared in the manifest (see
//! `docs/domain/authoring-workflow.md` and `docs/adr/0010-unit-amend.md`).
//!
//! The command reads the unit as `unit build` does and resolves its
//! dependencies under the unit lockfile, which must exist and already
//! record that resolution: `unit amend` never writes the lockfile. It then
//! searches the `src/` trees of the direct dependencies at their locked
//! versions for the file the author named, and, for exactly one match,
//! copies the file to `amendments/<path>` and adds the declaration to
//! `manifest.jsonc`. Everything is decided before anything is written.
//!
//! The copy is written first, created new so that an existing file is never
//! overwritten, and the manifest is replaced afterwards through a temporary
//! file and a rename. If the manifest cannot be written, the copy is removed
//! again. An interruption between the two steps leaves a copy without a
//! declaration, which `unit build` reports, and which running the same
//! command again completes when the copy is still the provider's file.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use super::exclusive::BuildLock;
use super::{
    BuildLockError, BuildProblem, MANIFEST, ManifestFileError, development_values, layer_sources,
    read_working_manifest_text, resolve_graph, without_amendments,
};
use crate::adapters::git::{FILE_LIMIT, GitError, GitRepository, ReadSrcError, UnitDirectory};
use crate::adapters::lockfile::{self, LockfileError};
use crate::adapters::manifest::{EditError, declare_amendment, read_manifest};
use crate::adapters::registry::{Registry, StorageError};
use crate::domain::{
    AmendmentDeclaration, LockChange, LockError, RelativePath, RelativePathError, ResolutionError,
    Root, UnitContent, UnitIdentity, Version, compose, delivered_path,
};

/// Where the edited manifest is written before it replaces `manifest.jsonc`.
/// The name is reserved beside the manifest.
pub(super) const MANIFEST_NEW: &str = ".assemblr-manifest.jsonc.new";

/// Amends `file` of one of the direct dependencies of the unit whose
/// repository's top level is `root`, or of `unit` alone when it is given.
///
/// `file` is a filename, searched recursively, or, when it contains a `/`,
/// a path in the provider's `src/`. Either matches a template by its source
/// name or by its delivered name, without `.assemblr.tmpl`; the copy always
/// keeps the source name.
pub fn amend_unit(
    root: &Path,
    registry: &Registry,
    file: &str,
    unit: Option<&UnitIdentity>,
) -> Result<Amended, AmendFailure> {
    let manifest_path = root.join(MANIFEST);
    if let Err(error) = fs::symlink_metadata(&manifest_path) {
        if error.kind() == io::ErrorKind::NotFound {
            return Err(AmendFailure::Rejected(vec![AmendProblem::Manifest(
                ManifestFileError::Missing {
                    path: manifest_path,
                },
            )]));
        }
    }
    let rejected = |problem| AmendFailure::Rejected(vec![problem]);
    let repository =
        GitRepository::open(root).map_err(|error| rejected(AmendProblem::Repository(error)))?;
    // The manifest and the lockfile are read, and the manifest written,
    // under the lock that serializes builds, so a build sees either the
    // manifest before this amendment or the one after it.
    let git_dir = repository
        .git_directory()
        .map_err(|error| rejected(AmendProblem::Repository(error)))?;
    let _guard =
        BuildLock::acquire(&git_dir).map_err(|error| rejected(AmendProblem::BuildLock(error)))?;
    match super::update::interrupted(&git_dir) {
        Ok(None) => {}
        Ok(Some(journal)) => return Err(rejected(AmendProblem::UpdateInterrupted { journal })),
        Err(error) => return Err(rejected(AmendProblem::GitDirectory(error))),
    }

    let mut problems = Vec::new();
    let wanted = match Wanted::parse(file) {
        Ok(wanted) => Some(wanted),
        Err(error) => {
            problems.push(AmendProblem::InvalidFile {
                file: file.to_owned(),
                error,
            });
            None
        }
    };
    let text = read_working_manifest_text(root)
        .map_err(|error| problems.push(AmendProblem::Manifest(error)))
        .ok();
    let manifest = text.as_ref().and_then(|text| {
        read_manifest(text)
            .map_err(|error| {
                problems.push(AmendProblem::Manifest(ManifestFileError::Invalid {
                    path: manifest_path.clone(),
                    error,
                }))
            })
            .ok()
    });
    let src = match repository.read_working_src() {
        Ok(src) => Some(src),
        Err(ReadSrcError::Git(error)) => {
            problems.push(AmendProblem::Repository(error));
            None
        }
        Err(error) => {
            problems.push(AmendProblem::Src(error));
            None
        }
    };
    let previous = match lockfile::read(root) {
        Ok(Some(previous)) => Some(previous),
        Ok(None) => {
            problems.push(AmendProblem::NoLockfile);
            None
        }
        Err(error) => {
            problems.push(AmendProblem::Lockfile(error));
            None
        }
    };
    let index = registry
        .index()
        .map_err(|error| problems.push(AmendProblem::Registry(error)))
        .ok();
    let (Some(wanted), Some(text), Some(manifest), Some(src), Some(previous), Some(index)) =
        (wanted, text, manifest, src, previous, index)
    else {
        return Err(AmendFailure::Rejected(problems));
    };
    if !problems.is_empty() {
        return Err(AmendFailure::Rejected(problems));
    }

    // The dependencies, as the lockfile records them, with every locked
    // version's content. A resolution that would change the lockfile is a
    // build's work.
    let mut found_problems = Vec::new();
    let resolved = resolve_graph(
        Some(manifest.name()),
        manifest.dependencies(),
        &previous,
        &index,
        &mut found_problems,
    );
    let Some(resolved) = resolved else {
        return Err(AmendFailure::Rejected(
            found_problems
                .into_iter()
                .map(AmendProblem::Build)
                .collect(),
        ));
    };
    if !found_problems.is_empty() {
        return Err(AmendFailure::Rejected(
            found_problems
                .into_iter()
                .map(AmendProblem::Build)
                .collect(),
        ));
    }
    if !resolved.resolution.changes().is_empty() {
        return Err(rejected(AmendProblem::LockfileStale {
            changes: resolved.resolution.changes().clone(),
        }));
    }
    let lock = &resolved.lock;
    if let Some(unit) = unit
        && !lock.units().contains_key(unit)
    {
        return Err(rejected(AmendProblem::NotADependency(unit.clone())));
    }
    // Every unit in the graph is searched, at its locked version: a file's
    // provider need not be a direct dependency.
    let searched: Vec<&UnitIdentity> = lock
        .units()
        .keys()
        .filter(|identity| unit.is_none_or(|unit| unit == *identity))
        .collect();
    let mut matches = Vec::new();
    for identity in &searched {
        let (Some(content), Some(locked)) = (
            resolved.contents.get(*identity),
            lock.units().get(*identity),
        ) else {
            continue;
        };
        for path in content.files.keys().filter(|path| wanted.matches(path)) {
            matches.push(Match {
                unit: (*identity).clone(),
                version: locked.version().clone(),
                path: path.clone(),
            });
        }
    }
    let found = match matches.as_slice() {
        [] => {
            return Err(rejected(AmendProblem::NoMatch {
                file: file.to_owned(),
                searched: searched.into_iter().cloned().collect(),
            }));
        }
        [found] => found.clone(),
        _ => {
            return Err(rejected(AmendProblem::Ambiguous {
                file: file.to_owned(),
                matches,
            }));
        }
    };
    let declaration = AmendmentDeclaration::new(found.unit.clone(), found.version.clone());

    if let Some(existing) = manifest.amendments().get(&found.path) {
        problems.push(AmendProblem::AlreadyDeclared {
            path: found.path.clone(),
            unit: existing.unit().clone(),
            base: existing.base().clone(),
        });
    }
    // A file the unit provides itself collides with the provider's in the
    // composition; that is reported as what it is, before composing.
    if src.contains_key(&found.path) {
        problems.push(AmendProblem::OwnSrc {
            path: found.path.clone(),
        });
    }
    if !problems.is_empty() {
        return Err(AmendFailure::Rejected(problems));
    }
    // The base: the file as the unit's dependencies compose it, which is
    // the file as it stands in `build/` before this amendment. The sources
    // are layered, so a template keeps its expressions. The unit's own
    // amendments are no part of it, and are left out of the composition:
    // one of them may be a copy an interrupted run left undeclared, or one
    // with conflict markers the author has not resolved yet.
    let (development, _) = development_values(&manifest);
    let units = resolved.units();
    let bare = without_amendments(&manifest);
    let no_copies = crate::domain::FileTree::new();
    let composition = compose(
        resolved.resolution.graph(),
        Root::Unit(UnitContent::new(&bare, &src, &no_copies)),
        &units,
        &development,
    );
    let composition = match composition {
        Ok(composition) => composition,
        Err(errors) => {
            problems.extend(
                errors
                    .into_iter()
                    .map(|error| AmendProblem::Build(BuildProblem::Composition(error))),
            );
            return Err(AmendFailure::Rejected(problems));
        }
    };
    let delivered = delivered_path(&found.path)
        .ok()
        .flatten()
        .unwrap_or_else(|| found.path.clone());
    let mut found_problems = Vec::new();
    let base = layer_sources(
        &composition,
        &delivered,
        lock,
        Some(manifest.name()),
        &resolved.dependencies,
        &mut found_problems,
    );
    problems.extend(found_problems.into_iter().map(AmendProblem::Build));
    let Some(base) = base else {
        return Err(AmendFailure::Rejected(problems));
    };
    // The locked units whose amendments the copy is composed from, with
    // their versions.
    let below: Vec<(UnitIdentity, Version)> = composition
        .files()
        .get(&delivered)
        .map(|composed| {
            composed
                .amendments()
                .iter()
                .filter_map(|amendment| {
                    let locked = lock.units().get(amendment.amender())?;
                    Some((amendment.amender().clone(), locked.version().clone()))
                })
                .collect()
        })
        .unwrap_or_default();
    let markers = crate::adapters::layered::has_markers(&base);
    let copy_path = root
        .join(UnitDirectory::Amendments.as_str())
        .join(found.path.as_str());
    // Every directory between the top level and the copy must be a real
    // directory, or not exist yet: a symbolic link would take the copy out
    // of the unit. The directories that do not exist are created, and
    // removed again if the amendment is not completed.
    let missing = match real_directories(root, &found.path) {
        Ok(missing) => missing,
        Err(DirectoryProblem::NotADirectory(path)) => {
            problems.push(AmendProblem::NotADirectory { path });
            Vec::new()
        }
        Err(DirectoryProblem::Unreadable(path, error)) => {
            problems.push(AmendProblem::Unreadable { path, error });
            Vec::new()
        }
    };
    let keep = match existing_copy(&copy_path, &base) {
        Ok(keep) => keep,
        Err(problem) => {
            problems.push(problem(found.path.clone()));
            false
        }
    };
    let staged = root.join(MANIFEST_NEW);
    if fs::symlink_metadata(&staged).is_ok() {
        problems.push(AmendProblem::StagingOccupied {
            path: staged.clone(),
        });
    }
    if !problems.is_empty() {
        return Err(AmendFailure::Rejected(problems));
    }
    let edited = declare_amendment(&text, &manifest, &found.path, &declaration)
        .map_err(|error| rejected(AmendProblem::ManifestEdit(error)))?;

    let created = if keep {
        None
    } else {
        if let Err(error) = write_copy(&copy_path, &base, &missing) {
            // The copy was never completed: nothing of it stays.
            let _ = fs::remove_file(&copy_path);
            remove_directories(&missing);
            return Err(AmendFailure::Write(WriteError {
                path: copy_path,
                error,
                state: AmendState::Unchanged,
            }));
        }
        Some(copy_path.as_path())
    };
    if let Err(error) = write_manifest(root, &staged, edited.as_bytes()) {
        let _ = fs::remove_file(&staged);
        let state = match created.map(fs::remove_file) {
            None | Some(Ok(())) => {
                remove_directories(&missing);
                AmendState::Unchanged
            }
            Some(Err(_)) => AmendState::CopyWithoutDeclaration,
        };
        return Err(AmendFailure::Write(WriteError {
            path: root.join(MANIFEST),
            error,
            state,
        }));
    }
    Ok(Amended {
        found,
        kept_existing: keep,
        below,
        markers,
    })
}

/// Checks that `amendments/` and the directories of `path` under it are
/// real directories where they exist, and returns the ones that do not
/// exist yet, outermost first.
pub(super) fn real_directories(
    root: &Path,
    path: &RelativePath,
) -> Result<Vec<PathBuf>, DirectoryProblem> {
    let mut segments: Vec<&str> = path.segments().collect();
    segments.pop();
    let mut directory = root.join(UnitDirectory::Amendments.as_str());
    let mut missing = Vec::new();
    for segment in std::iter::once(None).chain(segments.into_iter().map(Some)) {
        if let Some(segment) = segment {
            directory.push(segment);
        }
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Err(DirectoryProblem::NotADirectory(directory)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(directory.clone());
            }
            Err(error) => return Err(DirectoryProblem::Unreadable(directory, error)),
        }
    }
    Ok(missing)
}

/// Why a directory of the copy's path cannot be written into.
pub(super) enum DirectoryProblem {
    NotADirectory(PathBuf),
    Unreadable(PathBuf, io::Error),
}

/// Removes the directories `created`, innermost first, as far as they are
/// empty. Best effort: a directory that cannot be removed is harmless.
pub(super) fn remove_directories(created: &[PathBuf]) {
    for directory in created.iter().rev() {
        let _ = fs::remove_dir(directory);
    }
}

/// What the author asked for: a filename to search for, or a path.
enum Wanted {
    /// A filename, matched against the last segment of every path.
    Name(String),
    /// A path in the provider's `src/`.
    Path(RelativePath),
}

impl Wanted {
    fn parse(file: &str) -> Result<Self, RelativePathError> {
        let path = RelativePath::parse(file)?;
        Ok(if file.contains('/') {
            Self::Path(path)
        } else {
            Self::Name(file.to_owned())
        })
    }

    /// Whether `path`, a provider's source path, is the file asked for: by
    /// its source name or path, or by its delivered one for a template.
    fn matches(&self, path: &RelativePath) -> bool {
        let delivered = delivered_path(path).ok().flatten();
        match self {
            Self::Name(name) => {
                let last = |path: &RelativePath| path.segments().last().map(str::to_owned);
                last(path).as_deref() == Some(name)
                    || delivered.as_ref().and_then(last).as_deref() == Some(name)
            }
            Self::Path(wanted) => path == wanted || delivered.as_ref() == Some(wanted),
        }
    }
}

/// Whether a copy at `path` already holds `base`, so that an interrupted
/// `unit amend` is completed by declaring it. Anything else at the path is
/// a problem: a copy with other content is never overwritten.
fn existing_copy(path: &Path, base: &[u8]) -> Result<bool, fn(RelativePath) -> AmendProblem> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(|path| AmendProblem::CopyNotAFile { path }),
    };
    if !metadata.is_file() {
        return Err(|path| AmendProblem::CopyNotAFile { path });
    }
    let mut contents = Vec::new();
    let read =
        fs::File::open(path).and_then(|file| file.take(FILE_LIMIT + 1).read_to_end(&mut contents));
    match read {
        Ok(_) if contents == base => Ok(true),
        _ => Err(|path| AmendProblem::CopyExists { path }),
    }
}

/// Creates the copy, new, in the directories `missing`, which it creates
/// one at a time and never through a link, and flushes it.
pub(super) fn write_copy(path: &Path, contents: &[u8], missing: &[PathBuf]) -> io::Result<()> {
    for directory in missing {
        fs::create_dir(directory)?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

/// Replaces `manifest.jsonc` with `contents` through the new file `staged`.
pub(super) fn write_manifest(root: &Path, staged: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(staged)?;
    file.write_all(contents)?;
    file.sync_all()?;
    drop(file);
    fs::rename(staged, root.join(MANIFEST))
}

/// A file found in a provider's `src/` at its locked version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    unit: UnitIdentity,
    version: Version,
    path: RelativePath,
}

impl Match {
    /// The provider.
    pub fn unit(&self) -> &UnitIdentity {
        &self.unit
    }

    /// The provider's locked version, the amendment's base.
    pub fn version(&self) -> &Version {
        &self.version
    }

    /// The file's path in the provider's `src/`, which is the copy's path
    /// under `amendments/`.
    pub fn path(&self) -> &RelativePath {
        &self.path
    }
}

impl fmt::Display for Match {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "src/{} of {} {}", self.path, self.unit, self.version)
    }
}

/// A created amendment.
#[derive(Debug)]
pub struct Amended {
    found: Match,
    kept_existing: bool,
    below: Vec<(UnitIdentity, Version)>,
    markers: bool,
}

impl Amended {
    /// The amended file.
    pub fn file(&self) -> &Match {
        &self.found
    }

    /// Whether the copy already existed, unmodified, and was only declared:
    /// an earlier run was interrupted after writing it.
    pub fn kept_existing(&self) -> bool {
        self.kept_existing
    }

    /// The locked dependencies, with their versions, whose published
    /// amendments of the same file the copy is composed from, by identity.
    pub fn below(&self) -> &[(UnitIdentity, Version)] {
        &self.below
    }

    /// Whether the copy holds conflict markers: amendments below the unit
    /// conflict, and the copy resolves the conflict once the markers are
    /// replaced.
    pub fn has_markers(&self) -> bool {
        self.markers
    }
}

/// Why `unit amend` failed.
#[derive(Debug)]
pub enum AmendFailure {
    /// The command was rejected before anything was written. Every problem
    /// found is listed, and nothing changed.
    Rejected(Vec<AmendProblem>),
    /// Writing failed. The error says what remains.
    Write(WriteError),
}

impl fmt::Display for AmendFailure {
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
            Self::Write(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for AmendFailure {}

/// A write that failed, and what it left.
#[derive(Debug)]
pub struct WriteError {
    path: PathBuf,
    error: io::Error,
    state: AmendState,
}

impl WriteError {
    /// What remains.
    pub fn state(&self) -> AmendState {
        self.state
    }
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "could not write {}: {}", self.path.display(), self.error)
    }
}

impl std::error::Error for WriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// What a failed `unit amend` left behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmendState {
    /// Nothing: the manifest and `amendments/` are as they were.
    Unchanged,
    /// The copy, which the manifest does not declare. Running the command
    /// again completes the amendment.
    CopyWithoutDeclaration,
}

/// A problem that rejects `unit amend`.
#[derive(Debug)]
pub enum AmendProblem {
    /// The working tree's `manifest.jsonc` cannot be used.
    Manifest(ManifestFileError),
    /// The unit's repository or its working tree could not be read.
    Repository(GitError),
    /// The lock that keeps builds and amendments of the unit from running at
    /// the same time could not be taken.
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
    /// The lockfile cannot be read.
    Lockfile(LockfileError),
    /// There is no lockfile, so the dependencies' locked versions are
    /// unknown.
    NoLockfile,
    /// The lockfile does not record the current resolution.
    LockfileStale {
        /// How the resolution differs from it, by identity.
        changes: BTreeMap<UnitIdentity, LockChange>,
    },
    /// The registry index cannot be read.
    Registry(StorageError),
    /// A dependency could not be read as a build reads it.
    Build(BuildProblem),
    /// The dependencies cannot be resolved.
    Resolution(ResolutionError),
    /// The resolution contradicts the lockfile.
    Lock(LockError),
    /// `--unit` names an identity that is not a locked dependency, direct
    /// or transitive.
    NotADependency(UnitIdentity),
    /// The file argument is not a valid filename or path.
    InvalidFile {
        /// The argument.
        file: String,
        /// Why.
        error: RelativePathError,
    },
    /// No dependency provides the file.
    NoMatch {
        /// The argument.
        file: String,
        /// The dependencies searched, sorted.
        searched: Vec<UnitIdentity>,
    },
    /// Several files match, so the file must be named by its path, or the
    /// dependency given.
    Ambiguous {
        /// The argument.
        file: String,
        /// Every match, by dependency and then path.
        matches: Vec<Match>,
    },
    /// The manifest already declares an amendment of the path.
    AlreadyDeclared {
        /// The path.
        path: RelativePath,
        /// The declared provider.
        unit: UnitIdentity,
        /// The declared base.
        base: Version,
    },
    /// A file at `amendments/<path>` exists with other content than the
    /// provider's file, and is never overwritten.
    CopyExists {
        /// The path.
        path: RelativePath,
    },
    /// The entry at `amendments/<path>` is not a regular file.
    CopyNotAFile {
        /// The path.
        path: RelativePath,
    },
    /// `amendments/`, or a directory of the copy's path under it, exists
    /// and is not a real directory, for example a symbolic link, which
    /// could take the copy out of the unit.
    NotADirectory {
        /// The entry.
        path: PathBuf,
    },
    /// Something exists at the reserved path the edited manifest is staged
    /// at.
    StagingOccupied {
        /// The path.
        path: PathBuf,
    },
    /// A directory of the copy's path could not be examined.
    Unreadable {
        /// The entry.
        path: PathBuf,
        /// Why.
        error: io::Error,
    },
    /// The unit's own `src/` holds the path, which would collide with the
    /// amended file.
    OwnSrc {
        /// The path.
        path: RelativePath,
    },
    /// The manifest cannot be edited safely.
    ManifestEdit(EditError),
}

impl fmt::Display for AmendProblem {
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
            Self::Lockfile(error) => error.fmt(f),
            Self::NoLockfile => f.write_str(
                "there is no assemblr-lock.json, so the dependencies' locked versions, which an \
                 amendment is based on, are unknown",
            ),
            Self::LockfileStale { changes } => {
                let changes: Vec<String> = changes
                    .iter()
                    .map(|(identity, change)| match change {
                        LockChange::Added(version) => format!("{identity} {version} (new)"),
                        LockChange::Changed { from, to } => format!("{identity} {from} -> {to}"),
                        LockChange::Retired(version) => {
                            format!("{identity} {version} (no longer required)")
                        }
                    })
                    .collect();
                write!(
                    f,
                    "assemblr-lock.json does not record the current resolution: {}; an amendment \
                     is based on a locked version, and only `unit build` writes the lockfile",
                    changes.join(", ")
                )
            }
            Self::Registry(error) => error.fmt(f),
            Self::Build(problem) => problem.fmt(f),
            Self::Resolution(error) => error.fmt(f),
            Self::Lock(error) => error.fmt(f),
            Self::NotADependency(unit) => write!(
                f,
                "{unit} is not a dependency of the unit, directly or through its dependencies, \
                 so none of its files can be amended here"
            ),
            Self::InvalidFile { file, error } => {
                write!(
                    f,
                    "{file:?} is not a file name or a path in `src/`: {error}"
                )
            }
            Self::NoMatch { file, searched } => {
                if searched.is_empty() {
                    return write!(
                        f,
                        "no file matches {file:?}: the unit declares no dependencies to amend"
                    );
                }
                let units: Vec<String> = searched.iter().map(ToString::to_string).collect();
                write!(
                    f,
                    "no file matches {file:?} in the `src/` of {}, at the locked version",
                    units.join(", ")
                )?;
                if file.contains('/') {
                    f.write_str("; a path is matched exactly, with or without `.assemblr.tmpl`")
                } else {
                    f.write_str("; a name is matched against every file's name")
                }
            }
            Self::Ambiguous { file, matches } => {
                write!(f, "{file:?} matches {} files: ", matches.len())?;
                for (index, found) in matches.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    found.fmt(f)?;
                }
                f.write_str("; name the file by its path, or the dependency with `--unit`")
            }
            Self::AlreadyDeclared { path, unit, base } => write!(
                f,
                "the manifest already declares an amendment of {path} from {unit} {base}"
            ),
            Self::CopyExists { path } => write!(
                f,
                "amendments/{path} already exists with content other than the file's composed \
                 base, and an existing amendment is never overwritten"
            ),
            Self::CopyNotAFile { path } => {
                write!(f, "amendments/{path} exists and is not a regular file")
            }
            Self::NotADirectory { path } => write!(
                f,
                "{} exists and is not a directory; a symbolic link or a file there could take \
                 the copy outside the unit",
                path.display()
            ),
            Self::Unreadable { path, error } => {
                write!(f, "could not read {}: {error}", path.display())
            }
            Self::StagingOccupied { path } => write!(
                f,
                "{} exists; the name is reserved for the edited manifest before it replaces \
                 `manifest.jsonc`, so remove it if an earlier `unit amend` left it, and \
                 otherwise move it away",
                path.display()
            ),
            Self::OwnSrc { path } => write!(
                f,
                "src/{path} of the unit itself would collide with the amended file; a unit \
                 amends a dependency's file or provides its own at a path, never both"
            ),
            Self::ManifestEdit(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for AmendProblem {}
