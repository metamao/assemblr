//! The managed local registry of unit repositories.
//!
//! The registry keeps a validated index of registered unit repositories and
//! their published versions, in the user's data directory (see
//! `docs/adr/0007-local-registry-storage.md`). Sources are read through
//! [`GitRepository`], which never touches a working tree, and the rules for
//! what enters the index are the domain's ([`RegistryIndex`]). A remote
//! source is read from a bare copy beside the index, which only `add` and
//! `sync` fetch into (see `docs/adr/0029-remote-sources.md`).
//!
//! Every change is decided completely before the index is written, and a
//! rejected change leaves the index as it was.

mod info;
mod reference;
mod store;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) use self::observe as observe_tags;
pub use info::{Found, InfoFailure, Search, UnitInfo};
use reference::is_remote_reference;
pub use reference::{ReferenceError, local_reference, source_reference};
use sha2::{Digest, Sha256};
pub use store::StorageError;
use store::Store;

use super::git::{
    FetchError, GitError, GitRepository, HeadManifestError, InvalidVersion, ReadFileError,
    ReadSrcError, VersionTag, fetch, fetch_objects,
};
use super::lockfile::{PublishedLockfileError, read_published};
use crate::domain::{
    Excluded, ObjectId, ObservedTag, ObservedVersion, PublishedAmendmentError, RegisteredUnit,
    RegistryError, RegistryIndex, SourceReference, UnitIdentity, Version, declared_identity,
    published_amendments,
};

/// The environment variable that places the registry under another
/// directory, so tests and isolated environments never touch the user's
/// registry.
const DATA_DIR_VARIABLE: &str = "ASSEMBLR_DATA_DIR";

/// The user's local registry.
#[derive(Debug, Clone)]
pub struct Registry {
    store: Store,
    /// Where the bare copies of remote sources are kept.
    copies: PathBuf,
}

impl Registry {
    /// The registry of the current user: under `ASSEMBLR_DATA_DIR` when it is
    /// set, otherwise in the platform's local data directory. Nothing is
    /// created until the registry is first changed.
    pub fn locate() -> Result<Self, StorageError> {
        let root = match std::env::var_os(DATA_DIR_VARIABLE) {
            Some(dir) if !dir.is_empty() => {
                let dir = PathBuf::from(dir);
                if !dir.is_absolute() {
                    return Err(StorageError::RelativeLocation(dir));
                }
                dir
            }
            _ => dirs::data_local_dir()
                .ok_or(StorageError::NoLocation)?
                .join("assemblr"),
        };
        Ok(Self {
            store: Store::new(root.join("registry")),
            copies: root.join("sources"),
        })
    }

    /// Opens the repository registered from `source`: the local repository
    /// itself, or the copy of a remote one. Nothing is fetched.
    pub(crate) fn open_source(&self, source: &SourceReference) -> Result<GitRepository, GitError> {
        if !is_remote_reference(source) {
            return GitRepository::open(Path::new(source.as_str()));
        }
        let copy = self.copy_of(source);
        if !copy.is_dir() {
            return Err(GitError::not_a_repository(
                copy,
                format!(
                    "Assemblr has no copy of {source}; `assemblr registry sync` fetches it again"
                ),
            ));
        }
        GitRepository::open(&copy)
    }

    /// Opens the repository registered from `source` in the user's registry
    /// to read the content of `commits`, which the index records for it.
    /// Callers that hold only an index use this: there is one registry per
    /// user, so it is the one the index came from.
    ///
    /// A remote source whose copy is missing or lacks one of `commits` is
    /// fetched first. The repository returned fetches a version's file contents the first
    /// time they are read, since a remote copy holds only trees and
    /// metadata files (ADR 0037). The index is never changed, and the
    /// caller checks that each commit still has the tree the index records.
    pub(crate) fn open_registered(
        source: &SourceReference,
        commits: &[&ObjectId],
    ) -> Result<GitRepository, GitError> {
        if !is_remote_reference(source) {
            return GitRepository::open(Path::new(source.as_str()));
        }
        let registry = Self::locate().map_err(|error| {
            GitError::not_a_repository(
                PathBuf::from(source.as_str()),
                format!("the registry that holds its copy cannot be found: {error}"),
            )
        })?;
        let present = registry.copy_of(source).is_dir()
            && registry.open_source(source)?.has_commits(commits)?;
        if !present {
            registry.fetch_source(source).map_err(|error| {
                GitError::unretrievable(PathBuf::from(source.as_str()), error.to_string())
            })?;
        }
        let repository = registry.open_source(source)?;
        if !repository.has_commits(commits)? {
            return Err(GitError::unretrievable(
                PathBuf::from(source.as_str()),
                "it no longer has a commit that was indexed, so a published version was \
                 rewritten"
                    .to_owned(),
            ));
        }
        // The copy holds trees but not file contents (ADR 0037): a version's
        // files are fetched the first time they are read.
        Ok(repository.retrieving_from(source.as_str()))
    }

    /// Where the copy of the remote `source` is kept: a directory named by
    /// the SHA-256 of the reference, which is compared exactly, so two
    /// spellings of one URL get two copies.
    fn copy_of(&self, source: &SourceReference) -> PathBuf {
        let digest = Sha256::digest(source.as_str().as_bytes());
        let name: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        self.copies.join(name)
    }

    /// Fetches the remote `source` into its copy: its tags, commits, and
    /// trees, and the metadata files registration reads at each version tag
    /// and at `HEAD`, but no other file contents (ADR 0037). A local source
    /// is read in place and needs nothing.
    pub(crate) fn fetch_source(&self, source: &SourceReference) -> Result<(), FetchError> {
        if !is_remote_reference(source) {
            return Ok(());
        }
        let copy = self.copy_of(source);
        fetch(source.as_str(), &copy)?;
        let missing = GitRepository::open(&copy)
            .and_then(|repository| repository.missing_metadata())
            .map_err(|error| FetchError::unreadable(source.as_str(), error))?;
        fetch_objects(source.as_str(), &copy, &missing)
    }

    /// The index. An empty one if nothing has been registered.
    pub fn index(&self) -> Result<RegistryIndex, StorageError> {
        Ok(self.store.load()?.unwrap_or_default())
    }

    /// Registers the unit repository `path` names: a local path, or a remote
    /// reference, which is fetched first.
    ///
    /// A unit's valid published versions are indexed, after their `src/` and
    /// `amendments/` trees are validated, and invalid version tags are
    /// excluded. With no valid version, the unit's identity comes from the
    /// manifest committed at `HEAD`. A source already registered by the
    /// same reference is left as it is.
    pub fn add(&self, path: &Path) -> Result<Added, RegistryFailure> {
        let source = source_reference(path)?;
        let current = self.index()?;
        if let Some(already) = already_registered(&current, &source) {
            return Ok(already);
        }
        self.fetch_source(&source).map_err(|error| {
            RegistryFailure::Rejected(vec![Problem::Unfetchable {
                source: source.clone(),
                error,
            }])
        })?;
        let unreadable = |error| RegistryFailure::unreadable(&source, error);
        let repository = self.open_source(&source).map_err(unreadable)?;
        let tags = observe(&repository, None).map_err(unreadable)?;
        let identity = match declared_identity(&source, &tags) {
            Ok(Some(identity)) => identity,
            Ok(None) => match repository.head_manifest() {
                Ok(manifest) => manifest.name().clone(),
                Err(HeadManifestError::Git(error)) => return Err(unreadable(error)),
                Err(error) => {
                    return Err(RegistryFailure::Rejected(vec![Problem::NoIdentity {
                        source,
                        error,
                    }]));
                }
            },
            Err(errors) => return Err(RegistryFailure::invalid(errors)),
        };
        // Decided once before the lock, so that a rejected registration
        // creates no registry state, and again under it, against the index
        // as it is then.
        current
            .add(source.clone(), identity.clone(), tags.clone())
            .map_err(RegistryFailure::invalid)?;
        let lock = self.store.lock()?;
        let latest = self.index()?;
        if let Some(already) = already_registered(&latest, &source) {
            return Ok(already);
        }
        let change = latest
            .add(source.clone(), identity.clone(), tags)
            .map_err(RegistryFailure::invalid)?;
        let versions = change
            .index()
            .units()
            .get(&identity)
            .map(|unit| unit.versions().keys().cloned().collect())
            .unwrap_or_default();
        let (index, excluded) = change.into_parts();
        self.store.replace(&lock, &index)?;
        Ok(Added::Registered {
            identity,
            source,
            versions,
            excluded,
        })
    }

    /// Fetches `source` unless this operation fetched it already, and
    /// whether it is fetched. A source that fails is reported in `problems`
    /// once.
    fn fetch_once(
        &self,
        source: &SourceReference,
        fetched: &mut Fetched,
        problems: &mut Vec<Problem>,
    ) -> bool {
        if fetched.done.contains(source) {
            return true;
        }
        if fetched.failed.contains(source) {
            return false;
        }
        match self.fetch_source(source) {
            Ok(()) => {
                fetched.done.insert(source.clone());
                true
            }
            Err(error) => {
                fetched.failed.insert(source.clone());
                problems.push(Problem::Unfetchable {
                    source: source.clone(),
                    error,
                });
                false
            }
        }
    }

    /// Reads the repository registered from `source` again, and indexes the
    /// versions it has published since, as [`sync`](Self::sync) would for
    /// that one source: the same validation and integrity rules apply, and
    /// a conflict in any of its tags rejects the update. The other
    /// registered units are not read. `unit publish` indexes the version it
    /// has just tagged this way.
    pub fn index_source(&self, source: &SourceReference) -> Result<Indexed, RegistryFailure> {
        let lock = self.store.lock()?;
        let index = self.index()?;
        let Some((identity, unit)) = index
            .units()
            .iter()
            .find(|(_, unit)| unit.source() == source)
        else {
            return Ok(Indexed::NotRegistered);
        };
        let identity = identity.clone();
        let tags = self
            .open_source(source)
            .and_then(|repository| observe(&repository, Some(unit)))
            .map_err(|error| RegistryFailure::unreadable(source, error))?;
        let change = index
            .sync(BTreeMap::from([(identity.clone(), tags)]))
            .map_err(RegistryFailure::invalid)?;
        let added = change.added().get(&identity).cloned().unwrap_or_default();
        let (next, excluded) = change.into_parts();
        if next != index {
            self.store.replace(&lock, &next)?;
        }
        Ok(Indexed::Indexed {
            identity,
            added,
            excluded,
        })
    }

    /// Removes the registration of `identity`, and returns it. The
    /// repository is not touched, and need not exist any more.
    pub fn remove(&self, identity: &UnitIdentity) -> Result<RegisteredUnit, RegistryFailure> {
        let not_registered = || RegistryFailure::NotRegistered(identity.clone());
        self.index()?.remove(identity).ok_or_else(not_registered)?;
        let lock = self.store.lock()?;
        let removed = self.index()?.remove(identity).ok_or_else(not_registered)?;
        self.store.replace(&lock, removed.index())?;
        Ok(removed.unit().clone())
    }

    /// Reads every registered repository again, fetching each remote one
    /// first, and indexes the versions they have published since. A source
    /// that cannot be fetched or read, or any conflict in any unit, rejects
    /// the whole synchronization, and every problem is reported.
    pub fn sync(&self) -> Result<Synced, RegistryFailure> {
        if self.store.load()?.is_none() {
            return Ok(Synced::default());
        }
        let lock = self.store.lock()?;
        let index = self.index()?;
        let mut problems = Vec::new();
        let mut fetched = Fetched::default();
        let mut observed = BTreeMap::new();
        for (identity, unit) in index.units() {
            if !self.fetch_once(unit.source(), &mut fetched, &mut problems) {
                continue;
            }
            let tags = self
                .open_source(unit.source())
                .and_then(|repository| observe(&repository, Some(unit)));
            match tags {
                Ok(tags) => {
                    observed.insert(identity.clone(), tags);
                }
                Err(error) => problems.push(Problem::Unreadable {
                    source: unit.source().clone(),
                    error,
                }),
            }
        }
        let change = match index.sync(observed) {
            Ok(change) => Some(change),
            Err(errors) => {
                problems.extend(errors.into_iter().map(Problem::Invalid));
                None
            }
        };
        let Some(change) = change.filter(|_| problems.is_empty()) else {
            return Err(RegistryFailure::Rejected(problems));
        };
        let added = change.added().clone();
        let (synced, excluded) = change.into_parts();
        if synced != index {
            self.store.replace(&lock, &synced)?;
        }
        Ok(Synced {
            units: synced.units().len(),
            added,
            excluded,
        })
    }
}

/// The sources one operation has fetched, so each is fetched once.
#[derive(Debug, Default)]
struct Fetched {
    done: std::collections::BTreeSet<SourceReference>,
    failed: std::collections::BTreeSet<SourceReference>,
}

/// The outcome for `source` when it is registered already.
fn already_registered(index: &RegistryIndex, source: &SourceReference) -> Option<Added> {
    index
        .identity_of(source)
        .map(|identity| Added::AlreadyRegistered {
            identity: identity.clone(),
            source: source.clone(),
        })
}

/// Reads the version tags of `repository`. The `src/` and `amendments/` of
/// every valid version not indexed yet for `unit` are read too, so that a
/// version is indexed only when its content is a valid published unit. A
/// version with amendments must also carry a lockfile that identifies each
/// base (see [`published_amendments`]); whether a base's content is the one
/// this registry records is checked by the build that composes it.
pub(crate) fn observe(
    repository: &GitRepository,
    unit: Option<&RegisteredUnit>,
) -> Result<Vec<ObservedTag<TagProblem>>, GitError> {
    let mut observed = Vec::new();
    for tag in repository.published_versions()? {
        let outcome = observe_tag(repository, unit, &tag)?;
        observed.push(ObservedTag::new(
            tag.name().to_owned(),
            tag.version().clone(),
            outcome,
        ));
    }
    Ok(observed)
}

fn observe_tag(
    repository: &GitRepository,
    unit: Option<&RegisteredUnit>,
    tag: &VersionTag,
) -> Result<Result<ObservedVersion, TagProblem>, GitError> {
    let published = match tag.outcome() {
        Ok(published) => published,
        Err(invalid) => return Ok(Err(TagProblem::Version(invalid.clone()))),
    };
    let provenance = published.provenance();
    // An indexed version keeps the content first recorded for it, which was
    // validated then. If its tag moved, that is a conflict whatever the new
    // content is.
    let indexed = unit.is_some_and(|unit| unit.versions().contains_key(tag.version()));
    if !indexed {
        // Only trees are checked here, since a remote copy holds no other
        // file contents (ADR 0037). The files' sizes are checked by the same
        // readers when a build or a composition first reads them.
        match repository.check_src(provenance.commit()) {
            Ok(()) => {}
            Err(ReadSrcError::Git(error)) => return Err(error),
            Err(error) => return Ok(Err(TagProblem::Src(Arc::new(error)))),
        }
        let copies = match repository.amendment_paths(provenance.commit()) {
            Ok(copies) => copies,
            Err(ReadSrcError::Git(error)) => return Err(error),
            Err(error) => return Ok(Err(TagProblem::Amendments(Arc::new(error)))),
        };
        let manifest = published.manifest();
        let lock = if manifest.amendments().is_empty() {
            None
        } else {
            match read_published(repository, provenance.commit()) {
                Ok(lock) => lock,
                Err(PublishedLockfileError::Read(ReadFileError::Git(error))) => return Err(error),
                Err(error) => return Ok(Err(TagProblem::Lockfile(Arc::new(error)))),
            }
        };
        let problems = published_amendments(manifest, &copies, lock.as_ref());
        if !problems.is_empty() {
            return Ok(Err(TagProblem::InvalidAmendments(problems)));
        }
    }
    Ok(Ok(ObservedVersion::new(
        published.manifest().name().clone(),
        provenance.clone(),
    )))
}

/// The outcome of a registration.
#[derive(Debug)]
pub enum Added {
    /// The repository was registered.
    Registered {
        /// The unit's identity.
        identity: UnitIdentity,
        /// Its reference.
        source: SourceReference,
        /// Its indexed versions, in order.
        versions: Vec<Version>,
        /// The version tags excluded as invalid.
        excluded: Vec<Excluded<TagProblem>>,
    },
    /// The same reference was already registered, and nothing changed.
    AlreadyRegistered {
        /// The identity it is registered for.
        identity: UnitIdentity,
        /// The reference.
        source: SourceReference,
    },
}

/// The outcome of indexing one registered source again.
#[derive(Debug)]
pub enum Indexed {
    /// The source is registered, and its versions are indexed.
    Indexed {
        /// The identity it is registered for.
        identity: UnitIdentity,
        /// The newly indexed versions, in order.
        added: Vec<Version>,
        /// The version tags excluded as invalid.
        excluded: Vec<Excluded<TagProblem>>,
    },
    /// No unit is registered from the source, so nothing changed.
    NotRegistered,
}

/// The outcome of a synchronization.
#[derive(Debug, Default)]
pub struct Synced {
    units: usize,
    added: BTreeMap<UnitIdentity, Vec<Version>>,
    excluded: Vec<Excluded<TagProblem>>,
}

impl Synced {
    /// How many units were synchronized.
    pub fn units(&self) -> usize {
        self.units
    }

    /// The newly indexed versions of each unit.
    pub fn added(&self) -> &BTreeMap<UnitIdentity, Vec<Version>> {
        &self.added
    }

    /// The version tags excluded as invalid.
    pub fn excluded(&self) -> &[Excluded<TagProblem>] {
        &self.excluded
    }
}

/// Why a version tag publishes no valid version.
#[derive(Debug, Clone)]
pub enum TagProblem {
    /// The tag or its manifest is invalid.
    Version(InvalidVersion),
    /// Its `src/` cannot be read as unit files. Never a failure to read
    /// the repository, which fails the source instead.
    Src(Arc<ReadSrcError>),
    /// Its `amendments/` cannot be read as unit files. Never a failure to
    /// read the repository, which fails the source instead.
    Amendments(Arc<ReadSrcError>),
    /// It declares amendments, but its lockfile, which must identify each
    /// base, cannot be read.
    Lockfile(Arc<PublishedLockfileError>),
    /// Its amendment declarations, copies, and lockfile do not agree.
    InvalidAmendments(Vec<PublishedAmendmentError>),
}

impl fmt::Display for TagProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Version(invalid) => invalid.fmt(f),
            Self::Src(error) => write!(f, "invalid `src/`: {error}"),
            Self::Amendments(error) => write!(f, "invalid `amendments/`: {error}"),
            Self::Lockfile(error) => write!(f, "it declares amendments, but {error}"),
            Self::InvalidAmendments(problems) => {
                f.write_str("its amendments are invalid: ")?;
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

/// Why a registry operation failed. The index is unchanged.
#[derive(Debug)]
pub enum RegistryFailure {
    /// The registry's files could not be used.
    Storage(StorageError),
    /// The path cannot be a registry reference.
    Reference(ReferenceError),
    /// The identity is not registered.
    NotRegistered(UnitIdentity),
    /// The operation was rejected. Every problem found is listed.
    Rejected(Vec<Problem>),
}

impl RegistryFailure {
    fn unreadable(source: &SourceReference, error: GitError) -> Self {
        Self::Rejected(vec![Problem::Unreadable {
            source: source.clone(),
            error,
        }])
    }

    fn invalid(errors: Vec<RegistryError<TagProblem>>) -> Self {
        Self::Rejected(errors.into_iter().map(Problem::Invalid).collect())
    }
}

impl From<StorageError> for RegistryFailure {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<ReferenceError> for RegistryFailure {
    fn from(error: ReferenceError) -> Self {
        Self::Reference(error)
    }
}

impl fmt::Display for RegistryFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(error) => error.fmt(f),
            Self::Reference(error) => error.fmt(f),
            Self::NotRegistered(identity) => write!(f, "{identity} is not registered"),
            Self::Rejected(problems) => {
                for (index, problem) in problems.iter().enumerate() {
                    if index > 0 {
                        f.write_str("\n")?;
                    }
                    problem.fmt(f)?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for RegistryFailure {}

/// A problem that rejects a registry operation.
#[derive(Debug)]
pub enum Problem {
    /// A remote source could not be fetched.
    Unfetchable {
        /// The source.
        source: SourceReference,
        /// Why.
        error: FetchError,
    },
    /// A source could not be read as a Git repository.
    Unreadable {
        /// The source.
        source: SourceReference,
        /// Why.
        error: GitError,
    },
    /// A source has no valid published version, and no valid manifest
    /// committed at `HEAD` to take its identity from.
    NoIdentity {
        /// The source.
        source: SourceReference,
        /// Why the committed manifest cannot be used.
        error: HeadManifestError,
    },
    /// What was read breaks the registry's rules.
    Invalid(RegistryError<TagProblem>),
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unfetchable { error, .. } => error.fmt(f),
            Self::Unreadable { error, .. } => error.fmt(f),
            Self::NoIdentity { source, error } => write!(
                f,
                "{source} has no valid published version, so its identity is taken from \
                 the manifest committed at `HEAD`, but {error}"
            ),
            Self::Invalid(error) => error.fmt(f),
        }
    }
}
