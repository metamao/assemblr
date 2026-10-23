//! The local registry's index of unit repositories and their published
//! versions, and the rules that keep it valid.
//!
//! Reading sources and storing the index belong to adapters. This module
//! decides what a registration or a synchronization makes of what was read
//! (see `docs/domain/registries-and-discovery.md`):
//!
//! - a newly found invalid version tag is excluded and reported, and the
//!   operation continues;
//! - a source conflict, disagreeing identities, versions of equal precedence,
//!   or a change to a version already indexed rejects the whole operation,
//!   and the index stays as it was.
//!
//! The reason a tag is invalid is the caller's type `R`, so adapters keep
//! their own typed diagnostics.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;

use super::{CatalogIdentity, ObjectId, Provenance, UnitIdentity, Version};

/// How a registered source is referred to. References are compared exactly,
/// as stored: two different references never denote the same source, even
/// when they lead to the same repository.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceReference(String);

impl SourceReference {
    /// A reference, exactly as given.
    pub fn new(reference: impl Into<String>) -> Self {
        Self(reference.into())
    }

    /// The reference as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourceReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A registered unit repository and the published versions indexed from it,
/// each with the provenance first recorded for it.
///
/// A unit is registered directly, by `registry add`, or managed by the
/// catalogs that declare it, or both. A unit only catalogs manage is indexed
/// with the versions their indexes list, and leaves the registry when no
/// registered catalog declares it any more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredUnit {
    source: SourceReference,
    versions: BTreeMap<Version, Provenance>,
    direct: bool,
}

impl RegisteredUnit {
    /// A unit registered directly from `source`, with its indexed versions.
    pub fn new(source: SourceReference, versions: BTreeMap<Version, Provenance>) -> Self {
        Self {
            source,
            versions,
            direct: true,
        }
    }

    /// A unit from `source` that only catalogs manage.
    pub fn managed(source: SourceReference, versions: BTreeMap<Version, Provenance>) -> Self {
        Self {
            source,
            versions,
            direct: false,
        }
    }

    /// Whether the unit was registered directly, rather than only through
    /// catalogs.
    pub fn is_direct(&self) -> bool {
        self.direct
    }

    /// Where the unit is registered from.
    pub fn source(&self) -> &SourceReference {
        &self.source
    }

    /// Its indexed published versions, in SemVer order.
    pub fn versions(&self) -> &BTreeMap<Version, Provenance> {
        &self.versions
    }
}

/// A registered catalog: where it is registered from, the catalog commit its
/// inventory was read at, and the repository of each unit it declares, with
/// relative references already resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredCatalog {
    source: SourceReference,
    commit: ObjectId,
    units: BTreeMap<UnitIdentity, SourceReference>,
}

impl RegisteredCatalog {
    /// A catalog from `source`, read at `commit`, declaring `units`.
    pub fn new(
        source: SourceReference,
        commit: ObjectId,
        units: BTreeMap<UnitIdentity, SourceReference>,
    ) -> Self {
        Self {
            source,
            commit,
            units,
        }
    }

    /// Where the catalog is registered from.
    pub fn source(&self) -> &SourceReference {
        &self.source
    }

    /// The catalog commit the index was built from.
    pub fn commit(&self) -> &ObjectId {
        &self.commit
    }

    /// Each declared unit and its repository, ordered by identity.
    pub fn units(&self) -> &BTreeMap<UnitIdentity, SourceReference> {
        &self.units
    }
}

/// The validated index of registered units and catalogs.
///
/// Every source is registered for one identity only, no unit has two
/// versions of equal SemVer precedence, every unit a catalog declares is
/// registered from the repository the catalog gives, and every unit that is
/// not registered directly is declared by a catalog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistryIndex {
    units: BTreeMap<UnitIdentity, RegisteredUnit>,
    catalogs: BTreeMap<CatalogIdentity, RegisteredCatalog>,
}

impl RegistryIndex {
    /// An index with no registered units.
    pub fn new() -> Self {
        Self::default()
    }

    /// An index of `units` and no catalogs, checked as
    /// [`from_parts`](Self::from_parts) does.
    pub fn from_units(
        units: BTreeMap<UnitIdentity, RegisteredUnit>,
    ) -> Result<Self, Vec<InvalidIndex>> {
        Self::from_parts(units, BTreeMap::new())
    }

    /// An index of `units` and `catalogs`, checked against the index's
    /// invariants. Every broken invariant is reported.
    pub fn from_parts(
        units: BTreeMap<UnitIdentity, RegisteredUnit>,
        catalogs: BTreeMap<CatalogIdentity, RegisteredCatalog>,
    ) -> Result<Self, Vec<InvalidIndex>> {
        let index = Self { units, catalogs };
        let errors = index.broken_invariants();
        if errors.is_empty() {
            Ok(index)
        } else {
            Err(errors)
        }
    }

    fn broken_invariants(&self) -> Vec<InvalidIndex> {
        let mut errors = Vec::new();
        let mut identities: BTreeMap<&SourceReference, Vec<UnitIdentity>> = BTreeMap::new();
        for (identity, unit) in &self.units {
            identities
                .entry(&unit.source)
                .or_default()
                .push(identity.clone());
            let ambiguous = ambiguous_versions(unit.versions.keys());
            if !ambiguous.is_empty() {
                errors.push(InvalidIndex::Ambiguous {
                    identity: identity.clone(),
                    versions: ambiguous,
                });
            }
            if !unit.direct && self.listing(identity).is_empty() {
                errors.push(InvalidIndex::Unmanaged {
                    identity: identity.clone(),
                });
            }
        }
        for (source, identities) in identities {
            if identities.len() > 1 {
                errors.push(InvalidIndex::SharedSource {
                    source: source.clone(),
                    identities,
                });
            }
        }
        let mut catalog_sources: BTreeMap<&SourceReference, Vec<CatalogIdentity>> = BTreeMap::new();
        for (catalog, registered) in &self.catalogs {
            catalog_sources
                .entry(&registered.source)
                .or_default()
                .push(catalog.clone());
            for (identity, source) in &registered.units {
                if self.units.get(identity).map(RegisteredUnit::source) != Some(source) {
                    errors.push(InvalidIndex::Undeclared {
                        catalog: catalog.clone(),
                        identity: identity.clone(),
                        source: source.clone(),
                    });
                }
            }
        }
        for (source, catalogs) in catalog_sources {
            if catalogs.len() > 1 {
                errors.push(InvalidIndex::SharedCatalogSource {
                    source: source.clone(),
                    catalogs,
                });
            }
        }
        errors
    }

    /// The registered units, ordered by identity.
    pub fn units(&self) -> &BTreeMap<UnitIdentity, RegisteredUnit> {
        &self.units
    }

    /// The registered catalogs, ordered by identity.
    pub fn catalogs(&self) -> &BTreeMap<CatalogIdentity, RegisteredCatalog> {
        &self.catalogs
    }

    /// The identity the catalog `source` is registered for, if it is
    /// registered.
    pub fn catalog_of(&self, source: &SourceReference) -> Option<&CatalogIdentity> {
        self.catalogs
            .iter()
            .find(|(_, catalog)| &catalog.source == source)
            .map(|(identity, _)| identity)
    }

    /// The registered catalogs that declare `identity`, in order.
    pub fn listing(&self, identity: &UnitIdentity) -> Vec<CatalogIdentity> {
        self.catalogs
            .iter()
            .filter(|(_, catalog)| catalog.units.contains_key(identity))
            .map(|(catalog, _)| catalog.clone())
            .collect()
    }

    /// The identity `source` is registered for, if it is registered.
    pub fn identity_of(&self, source: &SourceReference) -> Option<&UnitIdentity> {
        self.units
            .iter()
            .find(|(_, unit)| &unit.source == source)
            .map(|(identity, _)| identity)
    }

    /// Registers `source` as `identity`, with the version tags read from it.
    ///
    /// Valid versions are indexed and invalid ones excluded. The registration
    /// is rejected if another source is registered for `identity`, if a
    /// valid version declares another identity, or if two valid versions
    /// have equal precedence. Registering a source that is already
    /// registered changes nothing.
    pub fn add<R>(
        &self,
        source: SourceReference,
        identity: UnitIdentity,
        tags: Vec<ObservedTag<R>>,
    ) -> Result<Change<R>, Vec<RegistryError<R>>> {
        if let Some(registered) = self.identity_of(&source) {
            if self.units[registered].direct {
                return Ok(Change::unchanged(self));
            }
            // A unit catalogs manage, now registered directly as well: every
            // version it publishes is indexed from now on.
            let registered = registered.clone();
            let mut index = self.clone();
            if let Some(unit) = index.units.get_mut(&registered) {
                unit.direct = true;
            }
            return index.sync(BTreeMap::from([(registered, tags)]));
        }
        let mut errors = Vec::new();
        if let Some(registered) = self.units.get(&identity) {
            errors.push(RegistryError::SourceConflict {
                identity: identity.clone(),
                registered: registered.source.clone(),
                given: source.clone(),
            });
        }
        let mut versions = BTreeMap::new();
        let mut excluded = Vec::new();
        for tag in tags {
            match tag.outcome {
                Ok(found) if found.identity == identity => {
                    versions.insert(tag.version, found.provenance);
                }
                Ok(found) => errors.push(RegistryError::UnexpectedIdentity {
                    identity: identity.clone(),
                    source: source.clone(),
                    version: tag.version,
                    declared: found.identity,
                }),
                Err(reason) => excluded.push(Excluded {
                    identity: identity.clone(),
                    tag: tag.name,
                    version: tag.version,
                    reason,
                }),
            }
        }
        let ambiguous = ambiguous_versions(versions.keys());
        if !ambiguous.is_empty() {
            errors.push(RegistryError::AmbiguousVersions {
                identity: identity.clone(),
                source: source.clone(),
                versions: ambiguous,
            });
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        excluded.sort_by(Excluded::order);
        let mut index = self.clone();
        let added = versions.keys().cloned().collect();
        index
            .units
            .insert(identity.clone(), RegisteredUnit::new(source, versions));
        Ok(Change {
            index,
            added: BTreeMap::from([(identity, added)]),
            excluded,
            removed: Vec::new(),
        })
    }

    /// Synchronizes the registered units with the version tags read again
    /// from their sources. Units with no entry in `observed` are kept as
    /// they are.
    ///
    /// New valid versions are indexed and new invalid ones excluded. An
    /// indexed version whose tag has disappeared, moved, or become invalid
    /// is an integrity conflict. Any conflict, in any unit, rejects the whole
    /// synchronization, and every one is reported.
    pub fn sync<R>(
        &self,
        observed: BTreeMap<UnitIdentity, Vec<ObservedTag<R>>>,
    ) -> Result<Change<R>, Vec<RegistryError<R>>> {
        let mut errors = Vec::new();
        let mut index = self.clone();
        let mut added = BTreeMap::new();
        let mut excluded = Vec::new();
        for (identity, tags) in observed {
            let Some(unit) = index.units.get_mut(&identity) else {
                continue;
            };
            let mut tags: BTreeMap<Version, ObservedTag<R>> = tags
                .into_iter()
                .map(|tag| (tag.version.clone(), tag))
                .collect();
            let integrity = |version: &Version, change| RegistryError::Integrity {
                identity: identity.clone(),
                source: unit.source.clone(),
                version: version.clone(),
                change,
            };
            for (version, recorded) in &unit.versions {
                let change = match tags.remove(version).map(|tag| tag.outcome) {
                    None => IntegrityChange::TagRemoved,
                    Some(Err(reason)) => IntegrityChange::BecameInvalid(reason),
                    Some(Ok(found)) if found.provenance == *recorded => continue,
                    Some(Ok(found)) => IntegrityChange::TagMoved {
                        recorded: recorded.clone(),
                        found: found.provenance,
                    },
                };
                errors.push(integrity(version, change));
            }
            let mut new_versions = Vec::new();
            for (version, tag) in tags {
                match tag.outcome {
                    Ok(found) if found.identity == identity => {
                        unit.versions.insert(version.clone(), found.provenance);
                        new_versions.push(version);
                    }
                    Ok(found) => errors.push(RegistryError::UnexpectedIdentity {
                        identity: identity.clone(),
                        source: unit.source.clone(),
                        version,
                        declared: found.identity,
                    }),
                    Err(reason) => excluded.push(Excluded {
                        identity: identity.clone(),
                        tag: tag.name,
                        version,
                        reason,
                    }),
                }
            }
            let ambiguous = ambiguous_versions(unit.versions.keys());
            if !ambiguous.is_empty() {
                errors.push(RegistryError::AmbiguousVersions {
                    identity: identity.clone(),
                    source: unit.source.clone(),
                    versions: ambiguous,
                });
            }
            if !new_versions.is_empty() {
                added.insert(identity, new_versions);
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        excluded.sort_by(Excluded::order);
        Ok(Change {
            index,
            added,
            excluded,
            removed: Vec::new(),
        })
    }

    /// Registers the catalog `identity`, or synchronizes it if it is
    /// registered, as `catalog` declares it now, with the versions its index
    /// lists for each unit it declares, as [`catalog_tags`] gives them.
    ///
    /// A declared unit that is not registered yet is registered as managed
    /// by the catalog. A managed one is synchronized with the listed
    /// versions. One registered directly keeps the versions read from its
    /// own tags, and a listed version indexed at another commit is an
    /// integrity conflict. One registered from another repository
    /// is a source conflict. A unit the catalog no longer declares leaves
    /// the registry unless it is registered directly or another catalog
    /// declares it. The catalog's source cannot change identity, and the
    /// identity cannot change source. Every conflict is reported, and any
    /// one rejects the whole change.
    pub fn apply_catalog<R>(
        &self,
        identity: CatalogIdentity,
        catalog: RegisteredCatalog,
        mut observed: BTreeMap<UnitIdentity, Vec<ObservedTag<R>>>,
    ) -> Result<Change<R>, Vec<RegistryError<R>>> {
        let mut errors = Vec::new();
        if let Some(registered) = self.catalogs.get(&identity) {
            if registered.source != catalog.source {
                errors.push(RegistryError::CatalogConflict {
                    catalog: identity.clone(),
                    registered: registered.source.clone(),
                    given: catalog.source.clone(),
                });
            }
        }
        if let Some(registered) = self.catalog_of(&catalog.source) {
            if *registered != identity {
                errors.push(RegistryError::CatalogRenamed {
                    source: catalog.source.clone(),
                    registered: registered.clone(),
                    declared: identity.clone(),
                });
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        let mut index = self.clone();
        let previous = index.catalogs.insert(identity.clone(), catalog.clone());
        let mut added = BTreeMap::new();
        let mut excluded = Vec::new();
        for (unit, source) in &catalog.units {
            let tags = observed.remove(unit).unwrap_or_default();
            let change = match index.units.get(unit) {
                Some(registered) if registered.source != *source => {
                    errors.push(RegistryError::SourceConflict {
                        identity: unit.clone(),
                        registered: registered.source.clone(),
                        given: source.clone(),
                    });
                    continue;
                }
                // A unit registered directly is indexed from its own tags.
                // What the catalog lists must agree with what is indexed.
                Some(registered) if registered.direct => {
                    for tag in &tags {
                        let (Ok(found), Some(recorded)) =
                            (&tag.outcome, registered.versions.get(&tag.version))
                        else {
                            continue;
                        };
                        if &found.provenance != recorded {
                            errors.push(RegistryError::Integrity {
                                identity: unit.clone(),
                                source: source.clone(),
                                version: tag.version.clone(),
                                change: IntegrityChange::TagMoved {
                                    recorded: recorded.clone(),
                                    found: found.provenance.clone(),
                                },
                            });
                        }
                    }
                    continue;
                }
                Some(_) => index.sync(BTreeMap::from([(unit.clone(), tags)])),
                // The repository is registered as another unit.
                None if index.identity_of(source).is_some() => {
                    errors.push(RegistryError::Inconsistent(InvalidIndex::Undeclared {
                        catalog: identity.clone(),
                        identity: unit.clone(),
                        source: source.clone(),
                    }));
                    continue;
                }
                None => index
                    .add(source.clone(), unit.clone(), tags)
                    .map(|mut change| {
                        if let Some(registered) = change.index.units.get_mut(unit) {
                            registered.direct = false;
                        }
                        change
                    }),
            };
            match change {
                Ok(change) => {
                    index = change.index;
                    added.extend(change.added);
                    excluded.extend(change.excluded);
                }
                Err(mut found) => errors.append(&mut found),
            }
        }
        let mut removed = Vec::new();
        if let Some(previous) = previous {
            for unit in previous.units.keys() {
                if !catalog.units.contains_key(unit) && index.prune(unit) {
                    removed.push(unit.clone());
                }
            }
        }
        if errors.is_empty() {
            errors.extend(
                index
                    .broken_invariants()
                    .into_iter()
                    .map(RegistryError::Inconsistent),
            );
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        excluded.sort_by(Excluded::order);
        Ok(Change {
            index,
            added,
            excluded,
            removed,
        })
    }

    /// Removes `unit` if it is not registered directly and no catalog
    /// declares it. Whether it was removed.
    fn prune(&mut self, unit: &UnitIdentity) -> bool {
        let unmanaged = self
            .units
            .get(unit)
            .is_some_and(|registered| !registered.direct)
            && self.listing(unit).is_empty();
        if unmanaged {
            self.units.remove(unit);
        }
        unmanaged
    }

    /// Removes the direct registration of `identity`.
    ///
    /// A unit no catalog declares leaves the index. One that catalogs
    /// declare stays, managed by them, and they are returned. A unit only
    /// catalogs manage cannot be removed by itself: it leaves with its
    /// catalogs, or when they no longer declare it.
    pub fn remove(&self, identity: &UnitIdentity) -> Result<Removed, Unremovable> {
        let unit = self.units.get(identity).ok_or(Unremovable::NotRegistered)?;
        let catalogs = self.listing(identity);
        if !unit.direct {
            return Err(Unremovable::Managed(catalogs));
        }
        let mut index = self.clone();
        if catalogs.is_empty() {
            index.units.remove(identity);
        } else if let Some(unit) = index.units.get_mut(identity) {
            unit.direct = false;
        }
        Ok(Removed {
            index,
            unit: unit.clone(),
            kept_by: catalogs,
        })
    }

    /// Removes the catalog `identity`, and with it every unit only it
    /// manages. `None` if it is not registered.
    pub fn remove_catalog(&self, identity: &CatalogIdentity) -> Option<RemovedCatalog> {
        let mut index = self.clone();
        let catalog = index.catalogs.remove(identity)?;
        let removed = catalog
            .units
            .keys()
            .filter(|unit| index.prune(unit))
            .cloned()
            .collect();
        Some(RemovedCatalog {
            index,
            catalog,
            removed,
        })
    }
}

/// The versions a catalog's index lists for `identity`, as tags observed
/// from the catalog rather than from the unit's repository, which is not
/// read. Versions already indexed and no longer listed keep their recorded
/// provenance: a catalog that has not been refreshed since never removes a
/// version, while one that lists a version at another commit is an
/// integrity conflict.
pub fn catalog_tags<R>(
    identity: &UnitIdentity,
    listed: &BTreeMap<Version, Provenance>,
    indexed: Option<&RegisteredUnit>,
) -> Vec<ObservedTag<R>> {
    let tag = |version: &Version, provenance: &Provenance| {
        ObservedTag::new(
            format!("v{version}"),
            version.clone(),
            Ok(ObservedVersion::new(identity.clone(), provenance.clone())),
        )
    };
    let mut tags: Vec<ObservedTag<R>> = listed
        .iter()
        .map(|(version, provenance)| tag(version, provenance))
        .collect();
    if let Some(unit) = indexed {
        tags.extend(
            unit.versions
                .iter()
                .filter(|(version, _)| !listed.contains_key(*version))
                .map(|(version, provenance)| tag(version, provenance)),
        );
    }
    tags
}

/// A removed direct registration.
#[derive(Debug, Clone, PartialEq)]
pub struct Removed {
    index: RegistryIndex,
    unit: RegisteredUnit,
    kept_by: Vec<CatalogIdentity>,
}

impl Removed {
    /// The index without the registration.
    pub fn index(&self) -> &RegistryIndex {
        &self.index
    }

    /// The registration as it was.
    pub fn unit(&self) -> &RegisteredUnit {
        &self.unit
    }

    /// The catalogs that still declare the unit, which keep it registered.
    pub fn kept_by(&self) -> &[CatalogIdentity] {
        &self.kept_by
    }
}

/// Why a unit's registration cannot be removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unremovable {
    /// The unit is not registered.
    NotRegistered,
    /// Only these catalogs manage the unit.
    Managed(Vec<CatalogIdentity>),
}

/// A removed catalog.
#[derive(Debug, Clone, PartialEq)]
pub struct RemovedCatalog {
    index: RegistryIndex,
    catalog: RegisteredCatalog,
    removed: Vec<UnitIdentity>,
}

impl RemovedCatalog {
    /// The index without the catalog.
    pub fn index(&self) -> &RegistryIndex {
        &self.index
    }

    /// The catalog as it was registered.
    pub fn catalog(&self) -> &RegisteredCatalog {
        &self.catalog
    }

    /// The units that left the registry with it, in order.
    pub fn removed(&self) -> &[UnitIdentity] {
        &self.removed
    }
}

/// The identity the valid tags of `source` declare, or `None` if no tag is
/// valid. Tags that declare different identities reject the source.
pub fn declared_identity<R>(
    source: &SourceReference,
    tags: &[ObservedTag<R>],
) -> Result<Option<UnitIdentity>, Vec<RegistryError<R>>> {
    let mut identities: BTreeMap<UnitIdentity, Vec<Version>> = BTreeMap::new();
    for tag in tags {
        if let Ok(found) = &tag.outcome {
            identities
                .entry(found.identity.clone())
                .or_default()
                .push(tag.version.clone());
        }
    }
    if identities.len() > 1 {
        for versions in identities.values_mut() {
            versions.sort();
        }
        return Err(vec![RegistryError::ConflictingIdentities {
            source: source.clone(),
            identities,
        }]);
    }
    Ok(identities.into_keys().next())
}

/// The versions in groups of two or more with equal SemVer precedence, such
/// as `1.0.0+a` and `1.0.0+b`, in order.
fn ambiguous_versions<'a>(versions: impl Iterator<Item = &'a Version>) -> Vec<Version> {
    // `Version` orders by precedence first and build metadata last, so
    // versions of equal precedence are adjacent.
    let ordered: Vec<&Version> = versions.collect();
    let equal = |a: &Version, b: Option<&&Version>| {
        b.is_some_and(|b| a.cmp_precedence(b) == Ordering::Equal)
    };
    ordered
        .iter()
        .enumerate()
        .filter(|(index, version)| {
            equal(
                version,
                index.checked_sub(1).and_then(|before| ordered.get(before)),
            ) || equal(version, ordered.get(index + 1))
        })
        .map(|(_, version)| (*version).clone())
        .collect()
}

/// A version tag read from a source, and whether it publishes a valid
/// version. `R` says why it does not.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservedTag<R> {
    name: String,
    version: Version,
    outcome: Result<ObservedVersion, R>,
}

impl<R> ObservedTag<R> {
    /// The tag `name`, which names `version`, and what reading it found.
    pub fn new(name: String, version: Version, outcome: Result<ObservedVersion, R>) -> Self {
        Self {
            name,
            version,
            outcome,
        }
    }

    /// The tag's version.
    pub fn version(&self) -> &Version {
        &self.version
    }

    /// The valid version, or why the tag does not publish one.
    pub fn outcome(&self) -> Result<&ObservedVersion, &R> {
        self.outcome.as_ref()
    }
}

/// A valid published version, as read from its tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedVersion {
    identity: UnitIdentity,
    provenance: Provenance,
}

impl ObservedVersion {
    /// A version whose manifest declares `identity`, read from `provenance`.
    pub fn new(identity: UnitIdentity, provenance: Provenance) -> Self {
        Self {
            identity,
            provenance,
        }
    }

    /// The identity its manifest declares.
    pub fn identity(&self) -> &UnitIdentity {
        &self.identity
    }

    /// Where it was read from.
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
}

/// An accepted registration or synchronization.
#[derive(Debug, Clone, PartialEq)]
pub struct Change<R> {
    index: RegistryIndex,
    added: BTreeMap<UnitIdentity, Vec<Version>>,
    excluded: Vec<Excluded<R>>,
    removed: Vec<UnitIdentity>,
}

impl<R> Change<R> {
    /// No change to `index`.
    pub fn unchanged_from(index: &RegistryIndex) -> Self {
        Self::unchanged(index)
    }

    fn unchanged(index: &RegistryIndex) -> Self {
        Self {
            index: index.clone(),
            added: BTreeMap::new(),
            excluded: Vec::new(),
            removed: Vec::new(),
        }
    }

    /// The index after the change.
    pub fn index(&self) -> &RegistryIndex {
        &self.index
    }

    /// The newly indexed versions of each unit, in order. A newly
    /// registered unit appears even when it has no versions.
    pub fn added(&self) -> &BTreeMap<UnitIdentity, Vec<Version>> {
        &self.added
    }

    /// The version tags excluded as invalid, ordered by identity, version,
    /// and tag.
    pub fn excluded(&self) -> &[Excluded<R>] {
        &self.excluded
    }

    /// The units a catalog no longer declares that left the registry, in
    /// order.
    pub fn removed(&self) -> &[UnitIdentity] {
        &self.removed
    }

    /// The index after the change, and the excluded tags.
    pub fn into_parts(self) -> (RegistryIndex, Vec<Excluded<R>>) {
        (self.index, self.excluded)
    }
}

/// A version tag excluded from the index because it is invalid.
#[derive(Debug, Clone, PartialEq)]
pub struct Excluded<R> {
    identity: UnitIdentity,
    tag: String,
    version: Version,
    reason: R,
}

impl<R> Excluded<R> {
    fn order(a: &Self, b: &Self) -> Ordering {
        (&a.identity, &a.version, &a.tag).cmp(&(&b.identity, &b.version, &b.tag))
    }

    /// The unit the tag belongs to.
    pub fn identity(&self) -> &UnitIdentity {
        &self.identity
    }

    /// The tag's name.
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// Why the tag publishes no valid version.
    pub fn reason(&self) -> &R {
        &self.reason
    }
}

/// Why a registration or a synchronization is rejected.
#[derive(Debug, Clone, PartialEq)]
pub enum RegistryError<R> {
    /// A different source is already registered for the identity.
    SourceConflict {
        /// The identity both sources declare.
        identity: UnitIdentity,
        /// The source registered for it.
        registered: SourceReference,
        /// The source being registered.
        given: SourceReference,
    },
    /// A source's valid versions declare different identities.
    ConflictingIdentities {
        /// The source.
        source: SourceReference,
        /// Each identity declared, with the versions that declare it.
        identities: BTreeMap<UnitIdentity, Vec<Version>>,
    },
    /// A valid version declares an identity other than its source's.
    UnexpectedIdentity {
        /// The identity the source is registered for.
        identity: UnitIdentity,
        /// The source.
        source: SourceReference,
        /// The version.
        version: Version,
        /// The identity it declares.
        declared: UnitIdentity,
    },
    /// Versions of equal SemVer precedence, which differ only in build
    /// metadata, so neither can be preferred.
    AmbiguousVersions {
        /// The unit.
        identity: UnitIdentity,
        /// Its source.
        source: SourceReference,
        /// The versions of equal precedence.
        versions: Vec<Version>,
    },
    /// A version already indexed no longer matches its source.
    Integrity {
        /// The unit.
        identity: UnitIdentity,
        /// Its source.
        source: SourceReference,
        /// The indexed version.
        version: Version,
        /// What changed.
        change: IntegrityChange<R>,
    },
    /// A different source is already registered for the catalog identity.
    CatalogConflict {
        /// The catalog identity.
        catalog: CatalogIdentity,
        /// The source registered for it.
        registered: SourceReference,
        /// The source being registered.
        given: SourceReference,
    },
    /// A registered catalog now declares another identity.
    CatalogRenamed {
        /// The catalog's source.
        source: SourceReference,
        /// The identity it is registered for.
        registered: CatalogIdentity,
        /// The identity it declares now.
        declared: CatalogIdentity,
    },
    /// The change would break an invariant of the index.
    Inconsistent(InvalidIndex),
}

/// How an indexed version's tag changed.
#[derive(Debug, Clone, PartialEq)]
pub enum IntegrityChange<R> {
    /// The tag no longer exists.
    TagRemoved,
    /// The tag points at a different commit.
    TagMoved {
        /// The provenance indexed.
        recorded: Provenance,
        /// The provenance found now.
        found: Provenance,
    },
    /// The tag no longer publishes a valid version, for this reason.
    BecameInvalid(R),
}

impl<R: fmt::Display> fmt::Display for RegistryError<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SourceConflict {
                identity,
                registered,
                given,
            } => write!(
                f,
                "source conflict: {given} declares {identity}, \
                 which is already registered from {registered}"
            ),
            Self::ConflictingIdentities { source, identities } => {
                write!(
                    f,
                    "the published versions of {source} declare different identities:"
                )?;
                for (identity, versions) in identities {
                    write!(f, " {identity} ({})", list(versions))?;
                }
                Ok(())
            }
            Self::UnexpectedIdentity {
                identity,
                source,
                version,
                declared,
            } => write!(
                f,
                "{source} is registered as {identity}, \
                 but its version {version} declares {declared}"
            ),
            Self::AmbiguousVersions {
                identity,
                source,
                versions,
            } => write!(
                f,
                "the published versions {} of {identity} ({source}) have equal SemVer \
                 precedence: they differ only in build metadata, so neither can be preferred",
                list(versions)
            ),
            Self::Integrity {
                identity,
                source,
                version,
                change,
            } => {
                write!(
                    f,
                    "integrity conflict: {identity} {version} is indexed from {source}, but "
                )?;
                match change {
                    IntegrityChange::TagRemoved => write!(f, "tag v{version} no longer exists"),
                    IntegrityChange::TagMoved { recorded, found } => write!(
                        f,
                        "tag v{version} moved from commit {} (tree {}) to commit {} (tree {}); \
                         a published version never changes",
                        recorded.commit(),
                        recorded.tree(),
                        found.commit(),
                        found.tree()
                    ),
                    IntegrityChange::BecameInvalid(reason) => {
                        write!(f, "tag v{version} is no longer valid: {reason}")
                    }
                }
            }
            Self::CatalogConflict {
                catalog,
                registered,
                given,
            } => write!(
                f,
                "source conflict: {given} declares the catalog {catalog}, \
                 which is already registered from {registered}"
            ),
            Self::CatalogRenamed {
                source,
                registered,
                declared,
            } => write!(
                f,
                "the catalog {source} is registered as {registered}, but it now declares \
                 {declared}; a registered catalog keeps its identity"
            ),
            Self::Inconsistent(invalid) => invalid.fmt(f),
        }
    }
}

impl<R: fmt::Debug + fmt::Display> std::error::Error for RegistryError<R> {}

/// Why a stored index breaks the index's invariants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidIndex {
    /// One source is registered for several identities.
    SharedSource {
        /// The source.
        source: SourceReference,
        /// The identities.
        identities: Vec<UnitIdentity>,
    },
    /// A unit has versions of equal precedence.
    Ambiguous {
        /// The unit.
        identity: UnitIdentity,
        /// The versions of equal precedence.
        versions: Vec<Version>,
    },
    /// A unit is neither registered directly nor declared by a catalog.
    Unmanaged {
        /// The unit.
        identity: UnitIdentity,
    },
    /// A catalog declares a unit that is not registered from the repository
    /// it gives.
    Undeclared {
        /// The catalog.
        catalog: CatalogIdentity,
        /// The unit.
        identity: UnitIdentity,
        /// The repository the catalog gives.
        source: SourceReference,
    },
    /// One source is registered for several catalogs.
    SharedCatalogSource {
        /// The source.
        source: SourceReference,
        /// The catalogs.
        catalogs: Vec<CatalogIdentity>,
    },
}

impl fmt::Display for InvalidIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SharedSource { source, identities } => {
                write!(f, "{source} is registered for several identities:")?;
                for identity in identities {
                    write!(f, " {identity}")?;
                }
                Ok(())
            }
            Self::Ambiguous { identity, versions } => write!(
                f,
                "{identity} has versions of equal precedence: {}",
                list(versions)
            ),
            Self::Unmanaged { identity } => write!(
                f,
                "{identity} is neither registered directly nor declared by a registered catalog"
            ),
            Self::Undeclared {
                catalog,
                identity,
                source,
            } => write!(
                f,
                "the catalog {catalog} declares {identity} from {source}, \
                 but it is not registered from there"
            ),
            Self::SharedCatalogSource { source, catalogs } => {
                write!(f, "{source} is registered for several catalogs:")?;
                for catalog in catalogs {
                    write!(f, " {catalog}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for InvalidIndex {}

fn list(versions: &[Version]) -> String {
    versions
        .iter()
        .map(Version::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        Change, IntegrityChange, ObservedTag, ObservedVersion, RegisteredCatalog, RegisteredUnit,
        RegistryError, RegistryIndex, SourceReference, Unremovable, catalog_tags,
        declared_identity,
    };
    use crate::domain::{CatalogIdentity, ObjectId, Provenance, UnitIdentity, Version};

    fn id(text: &str) -> UnitIdentity {
        UnitIdentity::parse(text).unwrap()
    }

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    fn source(text: &str) -> SourceReference {
        SourceReference::new(text)
    }

    /// A provenance whose commit and tree are made from `seed`.
    fn provenance(seed: char) -> Provenance {
        let commit = ObjectId::parse(&seed.to_string().repeat(40)).unwrap();
        let tree = ObjectId::parse(&format!("{}{seed}", "e".repeat(39))).unwrap();
        Provenance::new(commit, tree)
    }

    fn valid(identity: &str, text: &str, seed: char) -> ObservedTag<&'static str> {
        ObservedTag::new(
            format!("v{text}"),
            version(text),
            Ok(ObservedVersion::new(id(identity), provenance(seed))),
        )
    }

    fn invalid(text: &str, reason: &'static str) -> ObservedTag<&'static str> {
        ObservedTag::new(format!("v{text}"), version(text), Err(reason))
    }

    /// Units by identity, reference, and versions with their provenance seeds.
    type Units<'a> = &'a [(&'a str, &'a str, &'a [(&'a str, char)])];

    fn index(units: Units<'_>) -> RegistryIndex {
        RegistryIndex::from_units(
            units
                .iter()
                .map(|(identity, reference, versions)| {
                    let versions = versions
                        .iter()
                        .map(|(text, seed)| (version(text), provenance(*seed)))
                        .collect();
                    (
                        id(identity),
                        RegisteredUnit::new(source(reference), versions),
                    )
                })
                .collect(),
        )
        .unwrap()
    }

    fn versions(change: &Change<&str>, identity: &str) -> Vec<String> {
        change.index().units()[&id(identity)]
            .versions()
            .keys()
            .map(Version::to_string)
            .collect()
    }

    #[test]
    fn registration_indexes_valid_versions_and_excludes_invalid_tags() {
        let change = RegistryIndex::new()
            .add(
                source("/units/node"),
                id("@acme/node"),
                vec![
                    valid("@acme/node", "1.1.0", 'b'),
                    invalid("1.2.0", "the manifest declares version 1.3.0"),
                    valid("@acme/node", "1.0.0", 'a'),
                ],
            )
            .unwrap();

        assert_eq!(versions(&change, "@acme/node"), ["1.0.0", "1.1.0"]);
        assert_eq!(
            change.index().units()[&id("@acme/node")].versions()[&version("1.0.0")],
            provenance('a')
        );
        assert_eq!(change.excluded().len(), 1);
        assert_eq!(change.excluded()[0].tag(), "v1.2.0");
        assert_eq!(
            *change.excluded()[0].reason(),
            "the manifest declares version 1.3.0"
        );
        assert_eq!(
            change.added()[&id("@acme/node")],
            [version("1.0.0"), version("1.1.0")]
        );
    }

    #[test]
    fn a_unit_with_no_valid_version_is_registered_without_versions() {
        let change = RegistryIndex::new()
            .add(
                source("/units/node"),
                id("@acme/node"),
                vec![invalid("1.0.0", "no manifest")],
            )
            .unwrap();

        assert!(versions(&change, "@acme/node").is_empty());
        assert_eq!(change.excluded().len(), 1);
    }

    #[test]
    fn registering_a_registered_source_changes_nothing() {
        let before = index(&[("@acme/node", "/units/node", &[("1.0.0", 'a')])]);

        let change = before
            .add(
                source("/units/node"),
                id("@acme/node"),
                vec![valid("@acme/node", "2.0.0", 'b')],
            )
            .unwrap();

        assert_eq!(*change.index(), before);
        assert!(change.added().is_empty());
    }

    #[test]
    fn a_second_source_for_a_registered_identity_is_a_conflict() {
        let before = index(&[("@acme/node", "/units/node", &[])]);

        let errors = before
            .add(
                source("/elsewhere/node"),
                id("@acme/node"),
                vec![valid("@acme/node", "1.0.0", 'a')],
            )
            .unwrap_err();

        assert_eq!(
            errors,
            [RegistryError::SourceConflict {
                identity: id("@acme/node"),
                registered: source("/units/node"),
                given: source("/elsewhere/node"),
            }]
        );
    }

    #[test]
    fn versions_of_equal_precedence_reject_the_registration() {
        let errors = RegistryIndex::new()
            .add(
                source("/units/node"),
                id("@acme/node"),
                vec![
                    valid("@acme/node", "1.0.0+b", 'b'),
                    valid("@acme/node", "1.0.0+a", 'a'),
                    valid("@acme/node", "1.1.0", 'c'),
                ],
            )
            .unwrap_err();

        assert_eq!(
            errors,
            [RegistryError::AmbiguousVersions {
                identity: id("@acme/node"),
                source: source("/units/node"),
                versions: vec![version("1.0.0+a"), version("1.0.0+b")],
            }]
        );
    }

    #[test]
    fn the_identity_comes_from_the_valid_versions_which_must_agree() {
        let reference = source("/units/node");
        assert_eq!(
            declared_identity(
                &reference,
                &[invalid("0.1.0", "bad"), valid("@acme/node", "1.0.0", 'a')]
            ),
            Ok(Some(id("@acme/node")))
        );
        assert_eq!(
            declared_identity(&reference, &[invalid("0.1.0", "bad")]),
            Ok(None)
        );

        let error = declared_identity(
            &reference,
            &[
                valid("@acme/node", "2.0.0", 'c'),
                valid("@acme/old", "1.0.0", 'a'),
                valid("@acme/node", "1.5.0", 'b'),
            ],
        )
        .unwrap_err();
        assert_eq!(
            error,
            [RegistryError::ConflictingIdentities {
                source: reference,
                identities: BTreeMap::from([
                    (id("@acme/node"), vec![version("1.5.0"), version("2.0.0")]),
                    (id("@acme/old"), vec![version("1.0.0")]),
                ]),
            }]
        );
    }

    #[test]
    fn a_version_declaring_another_identity_rejects_the_registration() {
        let errors = RegistryIndex::new()
            .add(
                source("/units/node"),
                id("@acme/node"),
                vec![valid("@acme/other", "1.0.0", 'a')],
            )
            .unwrap_err();

        assert!(matches!(
            &errors[..],
            [RegistryError::UnexpectedIdentity { declared, .. }] if *declared == id("@acme/other")
        ));
    }

    #[test]
    fn synchronization_indexes_new_versions_and_excludes_new_invalid_tags() {
        let before = index(&[("@acme/node", "/units/node", &[("1.0.0", 'a')])]);

        let change = before
            .sync(BTreeMap::from([(
                id("@acme/node"),
                vec![
                    valid("@acme/node", "1.0.0", 'a'),
                    invalid("1.1.0", "no manifest"),
                    valid("@acme/node", "1.2.0", 'c'),
                ],
            )]))
            .unwrap();

        assert_eq!(versions(&change, "@acme/node"), ["1.0.0", "1.2.0"]);
        assert_eq!(change.added()[&id("@acme/node")], [version("1.2.0")]);
        assert_eq!(change.excluded()[0].tag(), "v1.1.0");
    }

    #[test]
    fn synchronization_without_changes_keeps_the_index() {
        let before = index(&[("@acme/node", "/units/node", &[("1.0.0", 'a')])]);

        let change = before
            .sync(BTreeMap::from([(
                id("@acme/node"),
                vec![valid("@acme/node", "1.0.0", 'a')],
            )]))
            .unwrap();

        assert_eq!(*change.index(), before);
        assert!(change.added().is_empty());
    }

    #[test]
    fn an_indexed_tag_that_disappears_moves_or_becomes_invalid_is_an_integrity_conflict() {
        let before = index(&[(
            "@acme/node",
            "/units/node",
            &[("1.0.0", 'a'), ("1.1.0", 'b'), ("1.2.0", 'c')],
        )]);

        let errors = before
            .sync(BTreeMap::from([(
                id("@acme/node"),
                vec![
                    valid("@acme/node", "1.1.0", 'd'),
                    invalid("1.2.0", "no manifest"),
                ],
            )]))
            .unwrap_err();

        let changes: Vec<_> = errors
            .iter()
            .map(|error| match error {
                RegistryError::Integrity {
                    version, change, ..
                } => (version.to_string(), change.clone()),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(
            changes,
            [
                ("1.0.0".to_owned(), IntegrityChange::TagRemoved),
                (
                    "1.1.0".to_owned(),
                    IntegrityChange::TagMoved {
                        recorded: provenance('b'),
                        found: provenance('d'),
                    }
                ),
                (
                    "1.2.0".to_owned(),
                    IntegrityChange::BecameInvalid("no manifest")
                ),
            ]
        );
    }

    #[test]
    fn a_conflict_in_one_unit_rejects_the_whole_synchronization() {
        let before = index(&[
            ("@acme/node", "/units/node", &[("1.0.0", 'a')]),
            ("@acme/web", "/units/web", &[("1.0.0", 'b')]),
        ]);

        let errors = before
            .sync(BTreeMap::from([
                (
                    id("@acme/node"),
                    vec![
                        valid("@acme/node", "1.0.0", 'a'),
                        valid("@acme/node", "2.0.0", 'c'),
                    ],
                ),
                (id("@acme/web"), vec![]),
            ]))
            .unwrap_err();

        assert!(matches!(
            &errors[..],
            [RegistryError::Integrity { identity, change: IntegrityChange::TagRemoved, .. }]
                if *identity == id("@acme/web")
        ));
    }

    #[test]
    fn synchronization_rejects_new_versions_of_another_identity_or_equal_precedence() {
        let before = index(&[("@acme/node", "/units/node", &[("1.0.0+a", 'a')])]);

        let errors = before
            .sync(BTreeMap::from([(
                id("@acme/node"),
                vec![
                    valid("@acme/node", "1.0.0+a", 'a'),
                    valid("@acme/node", "1.0.0+b", 'b'),
                    valid("@acme/renamed", "2.0.0", 'c'),
                ],
            )]))
            .unwrap_err();

        assert!(matches!(
            &errors[..],
            [
                RegistryError::UnexpectedIdentity { .. },
                RegistryError::AmbiguousVersions { versions, .. },
            ] if *versions == [version("1.0.0+a"), version("1.0.0+b")]
        ));
    }

    #[test]
    fn synchronization_does_not_depend_on_the_order_tags_are_read_in() {
        let before = index(&[
            ("@acme/node", "/units/node", &[("1.0.0", 'a')]),
            ("@acme/web", "/units/web", &[]),
        ]);
        let tags = || {
            vec![
                valid("@acme/node", "1.0.0", 'a'),
                invalid("1.1.0", "one"),
                valid("@acme/node", "1.2.0", 'c'),
                invalid("0.9.0", "two"),
                valid("@acme/node", "2.0.0-beta.1", 'd'),
            ]
        };
        let expected = before
            .sync(BTreeMap::from([
                (id("@acme/node"), tags()),
                (id("@acme/web"), vec![invalid("1.0.0", "three")]),
            ]))
            .unwrap();

        for rotation in 1..5 {
            let mut shuffled = tags();
            shuffled.rotate_left(rotation);
            shuffled.swap(0, 3);
            let change = before
                .sync(BTreeMap::from([
                    (id("@acme/node"), shuffled),
                    (id("@acme/web"), vec![invalid("1.0.0", "three")]),
                ]))
                .unwrap();
            assert_eq!(change, expected);
        }
    }

    #[test]
    fn removal_drops_only_the_registration() {
        let before = index(&[
            ("@acme/node", "/units/node", &[("1.0.0", 'a')]),
            ("@acme/web", "/units/web", &[]),
        ]);

        let removed = before.remove(&id("@acme/node")).unwrap();

        assert_eq!(removed.unit().source(), &source("/units/node"));
        assert!(removed.kept_by().is_empty());
        assert_eq!(removed.index(), &index(&[("@acme/web", "/units/web", &[])]));
        assert_eq!(
            before.remove(&id("@acme/missing")),
            Err(Unremovable::NotRegistered)
        );
    }

    fn none() -> Vec<ObservedTag<&'static str>> {
        Vec::new()
    }

    fn catalog(seed: char, units: &[(&str, &str)]) -> RegisteredCatalog {
        RegisteredCatalog::new(
            source("https://example.com/catalog.git"),
            ObjectId::parse(&seed.to_string().repeat(40)).unwrap(),
            units
                .iter()
                .map(|(identity, reference)| (id(identity), source(reference)))
                .collect(),
        )
    }

    fn web() -> CatalogIdentity {
        CatalogIdentity::parse("@acme/web").unwrap()
    }

    #[test]
    fn a_catalog_registers_the_units_it_declares_as_managed() {
        let change = RegistryIndex::new()
            .apply_catalog(
                web(),
                catalog(
                    'c',
                    &[("@acme/node", "/units/node"), ("@acme/api", "/units/api")],
                ),
                BTreeMap::from([
                    (id("@acme/node"), vec![valid("@acme/node", "1.0.0", 'a')]),
                    (id("@acme/api"), none()),
                ]),
            )
            .unwrap();

        let index = change.index();
        assert_eq!(index.catalogs().len(), 1);
        let node = &index.units()[&id("@acme/node")];
        assert!(!node.is_direct());
        assert_eq!(node.versions().len(), 1);
        assert_eq!(index.listing(&id("@acme/api")), vec![web()]);
        assert_eq!(change.added()[&id("@acme/api")], Vec::<Version>::new());
        // A managed unit cannot be removed by itself.
        assert_eq!(
            index.remove(&id("@acme/node")),
            Err(Unremovable::Managed(vec![web()]))
        );
    }

    #[test]
    fn a_unit_the_catalog_drops_leaves_unless_registered_directly() {
        let start = index(&[("@acme/api", "/units/api", &[])]);
        let first = start
            .apply_catalog(
                web(),
                catalog(
                    'c',
                    &[("@acme/node", "/units/node"), ("@acme/api", "/units/api")],
                ),
                BTreeMap::from([(id("@acme/node"), none()), (id("@acme/api"), none())]),
            )
            .unwrap();
        assert!(first.index().units()[&id("@acme/api")].is_direct());

        let second = first
            .index()
            .apply_catalog(
                web(),
                catalog('d', &[]),
                BTreeMap::from([(id("@acme/x"), none())]),
            )
            .unwrap();

        assert_eq!(second.removed(), &[id("@acme/node")]);
        assert_eq!(second.index().units().len(), 1);
        assert!(second.index().units().contains_key(&id("@acme/api")));
        // Removing the catalog removes what only it manages.
        let removed = first.index().remove_catalog(&web()).unwrap();
        assert_eq!(removed.removed(), &[id("@acme/node")]);
        // Removing the direct registration of a declared unit keeps it.
        let kept = first.index().remove(&id("@acme/api")).unwrap();
        assert_eq!(kept.kept_by(), &[web()]);
        assert!(!kept.index().units()[&id("@acme/api")].is_direct());
    }

    #[test]
    fn a_catalog_conflicts_with_another_source_for_a_unit_or_itself() {
        let start = index(&[("@acme/node", "/units/node", &[])]);
        let errors = start
            .apply_catalog(
                web(),
                catalog('c', &[("@acme/node", "/elsewhere/node")]),
                BTreeMap::from([(id("@acme/node"), none())]),
            )
            .unwrap_err();
        assert_eq!(
            errors,
            vec![RegistryError::SourceConflict {
                identity: id("@acme/node"),
                registered: source("/units/node"),
                given: source("/elsewhere/node"),
            }]
        );

        let registered = RegistryIndex::new()
            .apply_catalog(
                web(),
                catalog('c', &[]),
                BTreeMap::from([(id("@acme/x"), none())]),
            )
            .unwrap();
        let mut moved = catalog('c', &[]);
        moved = RegisteredCatalog::new(
            source("/elsewhere/catalog"),
            moved.commit().clone(),
            BTreeMap::new(),
        );
        let errors = registered
            .index()
            .apply_catalog(web(), moved, BTreeMap::<_, Vec<ObservedTag<&str>>>::new())
            .unwrap_err();
        assert!(matches!(errors[0], RegistryError::CatalogConflict { .. }));
        let renamed = registered
            .index()
            .apply_catalog(
                CatalogIdentity::parse("@acme/other").unwrap(),
                catalog('c', &[]),
                BTreeMap::<_, Vec<ObservedTag<&str>>>::new(),
            )
            .unwrap_err();
        assert!(matches!(renamed[0], RegistryError::CatalogRenamed { .. }));
    }

    #[test]
    fn catalog_tags_keep_indexed_versions_the_catalog_no_longer_lists() {
        let node = id("@acme/node");
        let listed = BTreeMap::from([(version("1.1.0"), provenance('b'))]);
        let indexed = RegisteredUnit::managed(
            source("/units/node"),
            BTreeMap::from([(version("1.0.0"), provenance('a'))]),
        );

        let tags: Vec<ObservedTag<&str>> = catalog_tags(&node, &listed, Some(&indexed));
        let versions: Vec<_> = tags.iter().map(|tag| tag.version().to_string()).collect();
        assert_eq!(versions, ["1.1.0", "1.0.0"]);

        // A listed version at another commit than the indexed one conflicts.
        let start = RegistryIndex::new()
            .apply_catalog(
                web(),
                catalog('c', &[("@acme/node", "/units/node")]),
                BTreeMap::from([(id("@acme/node"), vec![valid("@acme/node", "1.0.0", 'a')])]),
            )
            .unwrap();
        let moved = BTreeMap::from([(version("1.0.0"), provenance('f'))]);
        let errors = start
            .index()
            .apply_catalog(
                web(),
                catalog('d', &[("@acme/node", "/units/node")]),
                BTreeMap::from([(
                    id("@acme/node"),
                    catalog_tags::<&str>(&node, &moved, start.index().units().get(&node)),
                )]),
            )
            .unwrap_err();
        assert!(matches!(errors[0], RegistryError::Integrity { .. }));
    }

    #[test]
    fn a_catalog_cannot_move_a_version_of_a_unit_registered_directly() {
        let start = index(&[("@acme/node", "/units/node", &[("1.0.0", 'a')])]);
        let errors = start
            .apply_catalog(
                web(),
                catalog('c', &[("@acme/node", "/units/node")]),
                BTreeMap::from([(id("@acme/node"), vec![valid("@acme/node", "1.0.0", 'f')])]),
            )
            .unwrap_err();
        assert!(matches!(errors[0], RegistryError::Integrity { .. }));
        // Versions it lists that are not indexed change nothing.
        let change = start
            .apply_catalog(
                web(),
                catalog('c', &[("@acme/node", "/units/node")]),
                BTreeMap::from([(id("@acme/node"), vec![valid("@acme/node", "2.0.0", 'b')])]),
            )
            .unwrap();
        assert_eq!(
            change.index().units()[&id("@acme/node")].versions().len(),
            1
        );
    }

    #[test]
    fn a_stored_index_must_keep_the_invariants() {
        let units = BTreeMap::from([
            (
                id("@acme/node"),
                RegisteredUnit::new(
                    source("/units/shared"),
                    BTreeMap::from([
                        (version("1.0.0+a"), provenance('a')),
                        (version("1.0.0+b"), provenance('b')),
                    ]),
                ),
            ),
            (
                id("@acme/web"),
                RegisteredUnit::new(source("/units/shared"), BTreeMap::new()),
            ),
        ]);

        let errors = RegistryIndex::from_units(units).unwrap_err();

        assert_eq!(errors.len(), 2, "{errors:?}");
    }
}
