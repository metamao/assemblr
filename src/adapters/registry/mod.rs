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
pub(crate) use reference::catalog_reference;
use reference::is_remote_reference;
pub use reference::{ReferenceError, local_reference, source_reference};
use sha2::{Digest, Sha256};
pub use store::StorageError;
use store::Store;

use super::catalog::{CatalogProblem, PublishedCatalog, is_catalog, read_published_catalog};
use super::git::{
    FetchError, GitError, GitRepository, HeadManifestError, InvalidVersion, ReadFileError,
    ReadSrcError, VersionTag, fetch, fetch_objects,
};
use super::lockfile::{PublishedLockfileError, read_published};
use crate::domain::{
    CatalogIdentity, Change, Excluded, ObjectId, ObservedTag, ObservedVersion,
    PublishedAmendmentError, RegisteredUnit, RegistryError, RegistryIndex, RemovedCatalog,
    SourceReference, UnitIdentity, Unremovable, Version, catalog_tags, declared_identity,
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
    /// fetched first: that is how the content of a unit only catalogs
    /// manage is retrieved, when a build or a composition first needs it.
    /// The repository returned fetches a version's file contents the first
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

    /// Registers the unit repository or catalog `path` names: a local path,
    /// or a remote reference, which is fetched first.
    ///
    /// A unit's valid published versions are indexed, after their `src/` is
    /// read and validated, and invalid version tags are excluded. With no
    /// valid version, the unit's identity comes from the manifest committed
    /// at `HEAD`. A repository whose `HEAD` has a `registry.jsonc` and no
    /// manifest is a catalog, registered as [`add_catalog`] describes. A
    /// source already registered by the same reference is left as it is.
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
        let catalog = is_catalog(&repository)
            .map_err(|problem| RegistryFailure::catalog(&source, problem))?;
        if catalog {
            return self.add_catalog(source, &repository, &current);
        }
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

    /// Registers the catalog in `repository`, from `source`.
    ///
    /// The catalog's `registry.jsonc` and `registry-index.json` are read as
    /// committed at `HEAD`, and must agree. A unit only the catalog declares
    /// is indexed with the versions its index lists, from that metadata
    /// alone: its repository is fetched when a build or a composition first
    /// needs its content. Any problem rejects the registration.
    ///
    /// [`add_catalog`]: Self::add
    fn add_catalog(
        &self,
        source: SourceReference,
        repository: &GitRepository,
        current: &RegistryIndex,
    ) -> Result<Added, RegistryFailure> {
        let published = read_published_catalog(repository)
            .map_err(|problem| RegistryFailure::catalog(&source, problem))?;
        // Decided once before the lock, as `add` does, and again under it.
        let apply = |index: &RegistryIndex| {
            index
                .apply_catalog(
                    published.identity().clone(),
                    published.registered(source.clone()),
                    Self::observe_catalog(index, &published),
                )
                .map_err(RegistryFailure::invalid)
        };
        apply(current)?;
        let lock = self.store.lock()?;
        let latest = self.index()?;
        if let Some(already) = already_registered(&latest, &source) {
            return Ok(already);
        }
        let change = apply(&latest)?;
        let units = published
            .units()
            .keys()
            .map(|identity| {
                let versions = change
                    .index()
                    .units()
                    .get(identity)
                    .map(|unit| unit.versions().keys().cloned().collect())
                    .unwrap_or_default();
                (identity.clone(), versions)
            })
            .collect();
        let (index, excluded) = change.into_parts();
        self.store.replace(&lock, &index)?;
        Ok(Added::CatalogRegistered {
            catalog: published.identity().clone(),
            source,
            units,
            excluded,
        })
    }

    /// The versions `published` lists for each unit it declares, as the
    /// observations [`RegistryIndex::apply_catalog`] takes. The units'
    /// repositories are not read: their content is retrieved, and checked
    /// against these versions, when a build or a composition first needs it.
    fn observe_catalog(
        index: &RegistryIndex,
        published: &PublishedCatalog,
    ) -> BTreeMap<UnitIdentity, Vec<ObservedTag<TagProblem>>> {
        published
            .units()
            .iter()
            .map(|(identity, listed)| {
                let indexed = index
                    .units()
                    .get(identity)
                    .filter(|unit| unit.source() == listed.source());
                (
                    identity.clone(),
                    catalog_tags(identity, &listed.provenances(), indexed),
                )
            })
            .collect()
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
        if !unit.is_direct() {
            return Ok(Indexed::Managed {
                catalogs: index.listing(&identity),
            });
        }
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

    /// Removes the direct registration of `identity`, and returns it with
    /// the catalogs that still declare the unit, which keep it registered.
    /// The repository is not touched, and need not exist any more. A unit
    /// only catalogs manage is not removed by itself.
    pub fn remove(&self, identity: &UnitIdentity) -> Result<Removed, RegistryFailure> {
        let unremovable = |error| match error {
            Unremovable::NotRegistered => RegistryFailure::NotRegistered(identity.clone()),
            Unremovable::Managed(catalogs) => RegistryFailure::Managed {
                identity: identity.clone(),
                catalogs,
            },
        };
        self.index()?.remove(identity).map_err(unremovable)?;
        let lock = self.store.lock()?;
        let removed = self.index()?.remove(identity).map_err(unremovable)?;
        self.store.replace(&lock, removed.index())?;
        Ok(Removed {
            unit: removed.unit().clone(),
            kept_by: removed.kept_by().to_vec(),
        })
    }

    /// Removes the catalog `identity`, and every unit only it manages. The
    /// catalog's repository is not touched.
    pub fn remove_catalog(
        &self,
        identity: &CatalogIdentity,
    ) -> Result<RemovedCatalog, RegistryFailure> {
        let not_registered = || RegistryFailure::CatalogNotRegistered(identity.clone());
        self.index()?
            .remove_catalog(identity)
            .ok_or_else(not_registered)?;
        let lock = self.store.lock()?;
        let removed = self
            .index()?
            .remove_catalog(identity)
            .ok_or_else(not_registered)?;
        self.store.replace(&lock, removed.index())?;
        Ok(removed)
    }

    /// Reads every registered repository and catalog again, fetching each
    /// remote one first, and indexes the versions they have published since.
    ///
    /// Units registered directly are read first. Each catalog is then read
    /// as committed at its `HEAD`, as [`add`](Self::add) reads one: units it
    /// now declares are registered, units it no longer declares leave the
    /// registry unless something else keeps them, and the versions its index
    /// lists are indexed. The repositories of units only catalogs manage are
    /// not fetched. A source that cannot be fetched or read, or any
    /// conflict in any unit or catalog, rejects the whole synchronization,
    /// and every problem is reported.
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
            if !unit.is_direct() {
                continue;
            }
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
        let mut next = match index.sync(observed) {
            Ok(change) => Synchronizing::from(change),
            Err(errors) => {
                problems.extend(errors.into_iter().map(Problem::Invalid));
                Synchronizing::from(Change::unchanged_from(&index))
            }
        };
        for catalog in index.catalogs().values() {
            let source = catalog.source();
            if !self.fetch_once(source, &mut fetched, &mut problems) {
                continue;
            }
            let published = self
                .open_source(source)
                .map_err(CatalogProblem::Unreadable)
                .and_then(|repository| read_published_catalog(&repository));
            let published = match published {
                Ok(published) => published,
                Err(problem) => {
                    problems.push(Problem::Catalog {
                        catalog: source.clone(),
                        problem,
                    });
                    continue;
                }
            };
            let observed = Self::observe_catalog(&next.index, &published);
            match next.index.apply_catalog(
                published.identity().clone(),
                published.registered(source.clone()),
                observed,
            ) {
                Ok(change) => next.merge(change),
                Err(errors) => problems.extend(errors.into_iter().map(Problem::Invalid)),
            }
        }
        if !problems.is_empty() {
            return Err(RegistryFailure::Rejected(problems));
        }
        let Synchronizing {
            index: synced,
            mut added,
            mut excluded,
            removed,
        } = next;
        // A unit removed after versions were indexed for it in the same
        // synchronization is reported only as removed.
        added.retain(|identity, _| !removed.contains(identity));
        excluded.retain(|excluded| !removed.contains(excluded.identity()));
        if synced != index {
            self.store.replace(&lock, &synced)?;
        }
        Ok(Synced {
            units: synced.units().len(),
            catalogs: synced.catalogs().len(),
            added,
            excluded,
            removed,
        })
    }
}

/// The sources one operation has fetched, so each is fetched once.
#[derive(Debug, Default)]
struct Fetched {
    done: std::collections::BTreeSet<SourceReference>,
    failed: std::collections::BTreeSet<SourceReference>,
}

/// A synchronization's changes so far.
struct Synchronizing {
    index: RegistryIndex,
    added: BTreeMap<UnitIdentity, Vec<Version>>,
    excluded: Vec<Excluded<TagProblem>>,
    removed: Vec<UnitIdentity>,
}

impl From<Change<TagProblem>> for Synchronizing {
    fn from(change: Change<TagProblem>) -> Self {
        let added = change.added().clone();
        let removed = change.removed().to_vec();
        let (index, excluded) = change.into_parts();
        Self {
            index,
            added,
            excluded,
            removed,
        }
    }
}

impl Synchronizing {
    fn merge(&mut self, change: Change<TagProblem>) {
        let Self {
            index,
            added,
            excluded,
            removed,
        } = Self::from(change);
        self.index = index;
        for (identity, mut versions) in added {
            let entry = self.added.entry(identity).or_default();
            entry.append(&mut versions);
            entry.sort();
            entry.dedup();
        }
        self.excluded.extend(excluded);
        self.excluded
            .sort_by(|a, b| (a.identity(), a.tag()).cmp(&(b.identity(), b.tag())));
        for identity in removed {
            if !self.removed.contains(&identity) {
                self.removed.push(identity);
            }
        }
        self.removed.sort();
    }
}

/// The outcome for `source` when it is registered already, as a unit
/// registered directly or as a catalog.
fn already_registered(index: &RegistryIndex, source: &SourceReference) -> Option<Added> {
    if let Some(catalog) = index.catalog_of(source) {
        return Some(Added::CatalogAlreadyRegistered {
            catalog: catalog.clone(),
            source: source.clone(),
        });
    }
    let identity = index.identity_of(source)?;
    index.units()[identity]
        .is_direct()
        .then(|| Added::AlreadyRegistered {
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
    /// The catalog was registered.
    CatalogRegistered {
        /// The catalog's identity.
        catalog: CatalogIdentity,
        /// Its reference.
        source: SourceReference,
        /// Each unit it declares, with its indexed versions in order.
        units: BTreeMap<UnitIdentity, Vec<Version>>,
        /// The version tags excluded as invalid.
        excluded: Vec<Excluded<TagProblem>>,
    },
    /// The same reference was already registered as a catalog, and nothing
    /// changed.
    CatalogAlreadyRegistered {
        /// The catalog's identity.
        catalog: CatalogIdentity,
        /// The reference.
        source: SourceReference,
    },
}

/// A removed direct registration.
#[derive(Debug)]
pub struct Removed {
    unit: RegisteredUnit,
    kept_by: Vec<CatalogIdentity>,
}

impl Removed {
    /// The registration as it was.
    pub fn unit(&self) -> &RegisteredUnit {
        &self.unit
    }

    /// The catalogs that still declare the unit, which keep it registered.
    pub fn kept_by(&self) -> &[CatalogIdentity] {
        &self.kept_by
    }
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
    /// Only catalogs manage the unit, so its versions are those their
    /// indexes list, and nothing changed.
    Managed {
        /// The catalogs that declare it.
        catalogs: Vec<CatalogIdentity>,
    },
}

/// The outcome of a synchronization.
#[derive(Debug, Default)]
pub struct Synced {
    units: usize,
    catalogs: usize,
    added: BTreeMap<UnitIdentity, Vec<Version>>,
    excluded: Vec<Excluded<TagProblem>>,
    removed: Vec<UnitIdentity>,
}

impl Synced {
    /// How many units were synchronized.
    pub fn units(&self) -> usize {
        self.units
    }

    /// How many catalogs were synchronized.
    pub fn catalogs(&self) -> usize {
        self.catalogs
    }

    /// The newly indexed versions of each unit.
    pub fn added(&self) -> &BTreeMap<UnitIdentity, Vec<Version>> {
        &self.added
    }

    /// The units that left the registry because no catalog declares them
    /// any more, in order.
    pub fn removed(&self) -> &[UnitIdentity] {
        &self.removed
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
    /// No catalog is registered for the identity.
    CatalogNotRegistered(CatalogIdentity),
    /// Only catalogs manage the unit, so it cannot be removed by itself.
    Managed {
        /// The unit.
        identity: UnitIdentity,
        /// The catalogs that declare it.
        catalogs: Vec<CatalogIdentity>,
    },
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

    fn catalog(catalog: &SourceReference, problem: CatalogProblem) -> Self {
        Self::Rejected(vec![Problem::Catalog {
            catalog: catalog.clone(),
            problem,
        }])
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
            Self::CatalogNotRegistered(identity) => {
                write!(f, "no catalog {identity} is registered")
            }
            Self::Managed { identity, catalogs } => {
                write!(f, "{identity} is registered through the catalog")?;
                if catalogs.len() > 1 {
                    f.write_str("s")?;
                }
                for (index, catalog) in catalogs.iter().enumerate() {
                    f.write_str(if index == 0 { " " } else { ", " })?;
                    catalog.fmt(f)?;
                }
                f.write_str(", not by itself")
            }
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
    /// A catalog cannot be registered or synchronized.
    Catalog {
        /// The catalog's source.
        catalog: SourceReference,
        /// What is wrong.
        problem: CatalogProblem,
    },
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
            Self::Catalog { catalog, problem } => write!(f, "catalog {catalog}: {problem}"),
        }
    }
}
