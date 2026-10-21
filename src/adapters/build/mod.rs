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
//! the index records, and composes the result. Every problem found along the
//! way is reported, and then nothing is written. Only a build without
//! problems is published, together with its lockfile (see [`output`]).

mod amend;
mod exclusive;
mod lockfile;
mod output;

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub use amend::{AmendFailure, AmendProblem, AmendState, Amended, Match, WriteError, amend_unit};
use exclusive::BuildLock;
pub use exclusive::BuildLockError;
pub use lockfile::LockfileError;
pub use output::{OutputProblem, PublishError, State as PublishState};

use super::git::{GitError, GitRepository, InvalidVersion, MANIFEST_LIMIT, ReadSrcError};
use super::manifest::{ReadManifestError, read_manifest};
use super::registry::{Registry, StorageError};
use super::render::{Context, RenderError, render};
use crate::domain::{
    AmendmentBaseError, AvailableVersions, CompositionError, FileTree, LockChange, LockError,
    Manifest, ObjectId, ParameterName, ParameterValues, RelativePath, ResolutionError, Root,
    TEMPLATE_SUFFIX, UnitContent, UnitFile, UnitIdentity, UnitLock, Version, amendment_bases,
    compose, relock, resolve,
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
    let guard = match repository.git_directory() {
        Ok(git_dir) => BuildLock::acquire(&git_dir)
            .map_err(|error| BuildFailure::Rejected(vec![BuildProblem::BuildLock(error)]))?,
        Err(error) => {
            return Err(BuildFailure::Rejected(vec![BuildProblem::Repository(
                error,
            )]));
        }
    };

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
    if let Some(manifest) = &manifest {
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
    }
    let index = registry
        .index()
        .map_err(|error| problems.push(BuildProblem::Registry(error)))
        .ok();

    // Dependencies, as far as the unit's own inputs allow.
    let (Some(manifest), Some(previous), Some(index)) = (&manifest, previous, index) else {
        return Err(BuildFailure::Rejected(problems));
    };
    let previous_lock = previous.clone().unwrap_or_default();
    let dependencies = match Dependencies::read(&index, manifest) {
        Ok(dependencies) => dependencies,
        Err(found) => {
            problems.extend(found);
            return Err(BuildFailure::Rejected(problems));
        }
    };
    let resolution = match resolve(
        Some(manifest.name()),
        manifest.dependencies(),
        &dependencies.available,
        &previous_lock.versions(),
    ) {
        Ok(resolution) => resolution,
        Err(errors) => {
            problems.extend(errors.into_iter().map(BuildProblem::Resolution));
            return Err(BuildFailure::Rejected(problems));
        }
    };
    let lock = match relock(&previous_lock, &resolution, &index) {
        Ok(lock) => lock,
        Err(errors) => {
            problems.extend(errors.into_iter().map(BuildProblem::Lock));
            return Err(BuildFailure::Rejected(problems));
        }
    };
    // A build never moves an amended unit away from its base. The cause is
    // told apart here, with the previous lock; the composition below would
    // only see that the base and the resolved version differ.
    let moved: BTreeSet<RelativePath> = amendment_bases(manifest, &previous_lock, &resolution)
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
    let trees = match dependencies.read_src(&lock, lock.units().keys()) {
        Ok(trees) => trees,
        Err(found) => {
            problems.extend(found);
            return Err(BuildFailure::Rejected(problems));
        }
    };
    let (Some(src), Some(amendments)) = (src, amendments) else {
        return Err(BuildFailure::Rejected(problems));
    };

    let no_amendments = FileTree::new();
    let mut units = BTreeMap::new();
    for (identity, locked) in lock.units() {
        let key = (identity.clone(), locked.version().clone());
        if let (Some(manifest), Some(files)) =
            (dependencies.manifests.get(&key), trees.get(identity))
        {
            units.insert(
                identity.clone(),
                UnitContent::new(manifest, files, &no_amendments),
            );
        }
    }
    let root_content = UnitContent::new(manifest, &src, &amendments);
    let composition = compose(
        resolution.graph(),
        Root::Unit(root_content),
        &units,
        &BTreeMap::new(),
    );
    let composition = match composition {
        Ok(composition) => Some(composition),
        Err(errors) => {
            problems.extend(
                errors
                    .into_iter()
                    // Already reported, from the manifest alone, or with
                    // the cause, from the lock.
                    .filter(|error| match error {
                        CompositionError::MissingValue { unit, .. } => unit != manifest.name(),
                        CompositionError::AmendmentBase { path, .. } => !moved.contains(path),
                        _ => true,
                    })
                    .map(BuildProblem::Composition),
            );
            None
        }
    };
    let Some(composition) = composition.filter(|_| problems.is_empty()) else {
        return Err(BuildFailure::Rejected(problems));
    };

    let lockfile = match &previous {
        None => LockfileOutcome::Created,
        Some(previous) if *previous == lock => LockfileOutcome::Unchanged,
        Some(_) => LockfileOutcome::Updated,
    };

    // Templates are rendered with the parameter values of the unit that
    // provides each one, plus the amending unit's values as `own` for an
    // amended one. Every template's problems are reported, by delivered path
    // and then position, and nothing is written when there is any.
    let no_values = ParameterValues::new();
    let mut files: Vec<(&RelativePath, Cow<'_, [u8]>, bool)> = Vec::new();
    for (path, composed) in composition.files() {
        let executable = composed.is_executable();
        if !composed.is_template() {
            files.push((path, Cow::Borrowed(composed.contents()), executable));
            continue;
        }
        let template = |error| BuildProblem::Template {
            unit: composed.unit().clone(),
            path: composed.source().clone(),
            amended_by: composed.amended_by().cloned(),
            error,
        };
        let Ok(text) = std::str::from_utf8(composed.contents()) else {
            problems.push(template(TemplateError::NotUtf8));
            continue;
        };
        let values_of = |unit| composition.values().get(unit).unwrap_or(&no_values);
        let values = values_of(composed.unit());
        let context = match composed.amended_by() {
            Some(amender) => Context::amended(values, values_of(amender)),
            None => Context::new(values),
        };
        match render(text, &context) {
            Ok(rendered) => files.push((path, Cow::Owned(rendered.into_bytes()), executable)),
            Err(errors) => {
                problems.extend(errors.into_iter().map(TemplateError::Render).map(template))
            }
        }
    }
    if !problems.is_empty() {
        return Err(BuildFailure::Rejected(problems));
    }
    let files: Vec<(&RelativePath, UnitFile)> = files
        .into_iter()
        .map(|(path, contents, executable)| {
            (path, UnitFile::new(contents.into_owned(), executable))
        })
        .collect();
    let files: Vec<(&RelativePath, &UnitFile)> =
        files.iter().map(|(path, file)| (*path, file)).collect();
    let new_lock = (lockfile != LockfileOutcome::Unchanged).then_some(&lock);
    let published =
        output::publish(root, &files, new_lock, &guard).map_err(BuildFailure::Publish)?;
    Ok(Built {
        unit: manifest.name().clone(),
        version: manifest.version().clone(),
        files: files.len(),
        dependencies: lock.units().len(),
        lockfile,
        changes: resolution.changes().clone(),
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

    /// Reads the `src/` files of the locked versions of `identities`.
    fn read_src<'a>(
        &self,
        lock: &UnitLock,
        identities: impl IntoIterator<Item = &'a UnitIdentity>,
    ) -> Result<BTreeMap<UnitIdentity, FileTree>, Vec<BuildProblem>> {
        let mut trees = BTreeMap::new();
        let mut problems = Vec::new();
        for identity in identities {
            let Some(locked) = lock.units().get(identity) else {
                continue;
            };
            let key = (identity.clone(), locked.version().clone());
            let (Some(repository), Some(commit)) =
                (self.repositories.get(identity), self.commits.get(&key))
            else {
                continue;
            };
            match repository.read_src(commit) {
                Ok(files) => {
                    trees.insert(identity.clone(), files);
                }
                Err(ReadSrcError::Git(error)) => problems.push(BuildProblem::Source {
                    unit: identity.clone(),
                    error,
                }),
                Err(error) => problems.push(BuildProblem::Published {
                    unit: identity.clone(),
                    version: locked.version().clone(),
                    problem: PublishedProblem::Src(error),
                }),
            }
        }
        if problems.is_empty() {
            Ok(trees)
        } else {
            Err(problems)
        }
    }
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
    /// The unit and its dependencies cannot be composed.
    Composition(CompositionError),
    /// A template cannot be rendered.
    Template {
        /// The unit whose `src/` provides the template.
        unit: UnitIdentity,
        /// The template's path in that `src/`.
        path: RelativePath,
        /// The unit whose copy at `amendments/<path>` is rendered instead
        /// of the provider's file, if the template is amended.
        amended_by: Option<UnitIdentity>,
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
            Self::Src(error) => error.fmt(f),
            Self::Amendments(error) => error.fmt(f),
            Self::Output(problem) => problem.fmt(f),
            Self::Lockfile(error) => error.fmt(f),
            Self::DevelopmentValues { unit, parameters } => {
                let names: Vec<String> =
                    parameters.iter().map(|name| format!("`{name}`")).collect();
                write!(
                    f,
                    "{unit} declares required parameters without defaults: {}; a build needs \
                     development values for them, and supplying development values is not \
                     supported yet",
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
            Self::Composition(error) => error.fmt(f),
            Self::Template {
                unit,
                path,
                amended_by,
                error,
            } => {
                match amended_by {
                    Some(amender) => write!(
                        f,
                        "amendments/{path} of {amender} (a copy of template src/{path} of {unit})"
                    )?,
                    None => write!(f, "template src/{path} of {unit}")?,
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
