//! The managed local registry of unit repositories.
//!
//! The registry keeps a validated index of registered local unit
//! repositories and their published versions, in the user's data directory
//! (see `docs/adr/0007-local-registry-storage.md`). Sources are read through
//! [`GitRepository`], which never touches a working tree, and the rules for
//! what enters the index are the domain's ([`RegistryIndex`]).
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
pub use reference::{ReferenceError, local_reference};
pub use store::StorageError;
use store::Store;

use super::git::{
    GitError, GitRepository, HeadManifestError, InvalidVersion, ReadFileError, ReadSrcError,
    VersionTag,
};
use super::lockfile::{PublishedLockfileError, read_published};
use crate::domain::{
    Excluded, ObservedTag, ObservedVersion, PublishedAmendmentError, RegisteredUnit, RegistryError,
    RegistryIndex, SourceReference, UnitIdentity, Version, declared_identity, published_amendments,
};

/// The environment variable that places the registry under another
/// directory, so tests and isolated environments never touch the user's
/// registry.
const DATA_DIR_VARIABLE: &str = "ASSEMBLR_DATA_DIR";

/// The user's local registry.
#[derive(Debug, Clone)]
pub struct Registry {
    store: Store,
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
        })
    }

    /// The index. An empty one if nothing has been registered.
    pub fn index(&self) -> Result<RegistryIndex, StorageError> {
        Ok(self.store.load()?.unwrap_or_default())
    }

    /// Registers the local unit repository at `path`.
    ///
    /// Its valid published versions are indexed, after their `src/` is
    /// read and validated, and invalid version tags are excluded. With no
    /// valid version, the unit's identity comes from the manifest committed
    /// at `HEAD`. A repository already registered by the same reference is
    /// left as it is.
    pub fn add(&self, path: &Path) -> Result<Added, RegistryFailure> {
        let source = local_reference(path)?;
        let current = self.index()?;
        if let Some(identity) = current.identity_of(&source) {
            return Ok(Added::AlreadyRegistered {
                identity: identity.clone(),
                source,
            });
        }
        let unreadable = |error| RegistryFailure::unreadable(&source, error);
        let repository = GitRepository::open(Path::new(source.as_str())).map_err(unreadable)?;
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
        if let Some(identity) = latest.identity_of(&source) {
            return Ok(Added::AlreadyRegistered {
                identity: identity.clone(),
                source,
            });
        }
        let change = latest
            .add(source.clone(), identity.clone(), tags)
            .map_err(RegistryFailure::invalid)?;
        let versions = change.added().get(&identity).cloned().unwrap_or_default();
        let (index, excluded) = change.into_parts();
        self.store.replace(&lock, &index)?;
        Ok(Added::Registered {
            identity,
            source,
            versions,
            excluded,
        })
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
        let tags = GitRepository::open(Path::new(source.as_str()))
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
        if self.index()?.remove(identity).is_none() {
            return Err(not_registered());
        }
        let lock = self.store.lock()?;
        let (index, removed) = self.index()?.remove(identity).ok_or_else(not_registered)?;
        self.store.replace(&lock, &index)?;
        Ok(removed)
    }

    /// Reads every registered repository again, and indexes the versions
    /// they have published since.
    ///
    /// A source that cannot be read, or any conflict in any unit, rejects
    /// the whole synchronization, and every problem is reported.
    pub fn sync(&self) -> Result<Synced, RegistryFailure> {
        if self.store.load()?.is_none() {
            return Ok(Synced::default());
        }
        let lock = self.store.lock()?;
        let index = self.index()?;
        let mut problems = Vec::new();
        let mut observed = BTreeMap::new();
        for (identity, unit) in index.units() {
            let tags = GitRepository::open(Path::new(unit.source().as_str()))
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
            Ok(change) if problems.is_empty() => change,
            Ok(_) => return Err(RegistryFailure::Rejected(problems)),
            Err(errors) => {
                problems.extend(errors.into_iter().map(Problem::Invalid));
                return Err(RegistryFailure::Rejected(problems));
            }
        };
        let units = index.units().len();
        let added = change.added().clone();
        let (next, excluded) = change.into_parts();
        if next != index {
            self.store.replace(&lock, &next)?;
        }
        Ok(Synced {
            units,
            added,
            excluded,
        })
    }
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
        match repository.read_src(provenance.commit()) {
            Ok(_) => {}
            Err(ReadSrcError::Git(error)) => return Err(error),
            Err(error) => return Ok(Err(TagProblem::Src(Arc::new(error)))),
        }
        let copies = match repository.read_amendments(provenance.commit()) {
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
        let problems = published_amendments(manifest, copies.keys(), lock.as_ref());
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
