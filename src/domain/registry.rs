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
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use super::{Provenance, UnitIdentity, Version};

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredUnit {
    source: SourceReference,
    versions: BTreeMap<Version, Provenance>,
}

impl RegisteredUnit {
    /// A unit registered from `source`, with its indexed versions.
    pub fn new(source: SourceReference, versions: BTreeMap<Version, Provenance>) -> Self {
        Self { source, versions }
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

/// The validated index of registered units.
///
/// A source is registered for one identity, or for each unit of a catalog,
/// and no unit has two versions of equal SemVer precedence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistryIndex {
    units: BTreeMap<UnitIdentity, RegisteredUnit>,
}

impl RegistryIndex {
    /// An index with no registered units.
    pub fn new() -> Self {
        Self::default()
    }

    /// An index of `units`, checked against the index's invariants. Every
    /// broken invariant is reported.
    pub fn from_units(
        units: BTreeMap<UnitIdentity, RegisteredUnit>,
    ) -> Result<Self, Vec<InvalidIndex>> {
        let index = Self { units };
        let errors = index.broken_invariants();
        if errors.is_empty() {
            Ok(index)
        } else {
            Err(errors)
        }
    }

    fn broken_invariants(&self) -> Vec<InvalidIndex> {
        let mut errors = Vec::new();
        for (identity, unit) in &self.units {
            let ambiguous = ambiguous_versions(unit.versions.keys());
            if !ambiguous.is_empty() {
                errors.push(InvalidIndex::Ambiguous {
                    identity: identity.clone(),
                    versions: ambiguous,
                });
            }
        }
        errors
    }

    /// The registered units, ordered by identity.
    pub fn units(&self) -> &BTreeMap<UnitIdentity, RegisteredUnit> {
        &self.units
    }

    /// The identities `source` is registered for, in order: one for a
    /// unit's repository, every unit's for a catalog. Empty when it is not
    /// registered.
    pub fn identities_of(&self, source: &SourceReference) -> Vec<&UnitIdentity> {
        self.units
            .iter()
            .filter(|(_, unit)| &unit.source == source)
            .map(|(identity, _)| identity)
            .collect()
    }

    /// Registers `source` for `identities`, with the version tags read from
    /// it: the identities its valid tags publish (see
    /// [`declared_identities`]), or those committed at `HEAD` when no tag
    /// is valid.
    ///
    /// Each unit's valid versions are indexed, and invalid tags excluded.
    /// The registration is rejected if another source is registered for one
    /// of `identities`, or if two valid versions of a unit have equal
    /// precedence. Registering a source that is already registered changes
    /// nothing.
    pub fn add<R>(
        &self,
        source: SourceReference,
        identities: &BTreeSet<UnitIdentity>,
        tags: Vec<ObservedTag<R>>,
    ) -> Result<Change<R>, Vec<RegistryError<R>>> {
        if !self.identities_of(&source).is_empty() {
            return Ok(Change::unchanged(self));
        }
        let mut errors = Vec::new();
        for identity in identities {
            if let Some(registered) = self.units.get(identity) {
                errors.push(RegistryError::SourceConflict {
                    identity: identity.clone(),
                    registered: registered.source.clone(),
                    given: source.clone(),
                });
            }
        }
        let mut versions: BTreeMap<UnitIdentity, BTreeMap<Version, Provenance>> = identities
            .iter()
            .map(|identity| (identity.clone(), BTreeMap::new()))
            .collect();
        let mut excluded = Vec::new();
        for tag in tags {
            match tag.outcome {
                Ok(found) => {
                    for found in found {
                        match versions.get_mut(&found.identity) {
                            Some(versions) => {
                                versions.insert(tag.version.clone(), found.provenance);
                            }
                            None => errors.push(RegistryError::UnexpectedIdentity {
                                identity: identities
                                    .first()
                                    .cloned()
                                    .unwrap_or_else(|| found.identity.clone()),
                                source: source.clone(),
                                version: tag.version.clone(),
                                declared: found.identity,
                            }),
                        }
                    }
                }
                Err(reason) => excluded.push(Excluded {
                    subject: subject(&source, identities, tag.catalog),
                    tag: tag.name,
                    version: tag.version,
                    reason,
                }),
            }
        }
        for (identity, versions) in &versions {
            let ambiguous = ambiguous_versions(versions.keys());
            if !ambiguous.is_empty() {
                errors.push(RegistryError::AmbiguousVersions {
                    identity: identity.clone(),
                    source: source.clone(),
                    versions: ambiguous,
                });
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        excluded.sort_by(Excluded::order);
        let mut index = self.clone();
        let mut added = BTreeMap::new();
        for (identity, versions) in versions {
            added.insert(identity.clone(), versions.keys().cloned().collect());
            index
                .units
                .insert(identity, RegisteredUnit::new(source.clone(), versions));
        }
        Ok(Change {
            index,
            added,
            excluded,
        })
    }

    /// Synchronizes the registered sources with the version tags read again
    /// from them. Sources with no entry in `observed` are kept as they are.
    ///
    /// New valid versions are indexed and new invalid tags excluded. A unit
    /// a catalog's new version publishes for the first time is registered
    /// from the catalog. An indexed version whose tag has disappeared,
    /// moved, become invalid, or no longer publishes the unit is an
    /// integrity conflict. Any conflict, in any source, rejects the whole
    /// synchronization, and every one is reported.
    pub fn sync<R: Clone>(
        &self,
        observed: BTreeMap<SourceReference, Vec<ObservedTag<R>>>,
    ) -> Result<Change<R>, Vec<RegistryError<R>>> {
        let mut errors = Vec::new();
        let mut index = self.clone();
        let mut added: BTreeMap<UnitIdentity, Vec<Version>> = BTreeMap::new();
        let mut excluded = Vec::new();
        for (source, tags) in observed {
            let registered: BTreeSet<UnitIdentity> =
                self.identities_of(&source).into_iter().cloned().collect();
            if registered.is_empty() {
                continue;
            }
            let catalog = registered.len() > 1 || tags.iter().any(|tag| tag.catalog);
            let tags: BTreeMap<Version, ObservedTag<R>> = tags
                .into_iter()
                .map(|tag| (tag.version.clone(), tag))
                .collect();
            for identity in &registered {
                let Some(unit) = self.units.get(identity) else {
                    continue;
                };
                for (version, recorded) in &unit.versions {
                    let change = match tags.get(version).map(|tag| &tag.outcome) {
                        None => IntegrityChange::TagRemoved,
                        Some(Err(reason)) => IntegrityChange::BecameInvalid(reason.clone()),
                        Some(Ok(found)) => {
                            match found.iter().find(|found| &found.identity == identity) {
                                Some(found) if found.provenance == *recorded => continue,
                                Some(found) => IntegrityChange::TagMoved {
                                    recorded: recorded.clone(),
                                    found: found.provenance.clone(),
                                },
                                None => IntegrityChange::UnitMissing,
                            }
                        }
                    };
                    errors.push(RegistryError::Integrity {
                        identity: identity.clone(),
                        source: source.clone(),
                        version: version.clone(),
                        change,
                    });
                }
            }
            let indexed = |version: &Version| {
                registered.iter().any(|identity| {
                    self.units
                        .get(identity)
                        .is_some_and(|unit| unit.versions.contains_key(version))
                })
            };
            for (version, tag) in tags {
                let found = match tag.outcome {
                    Ok(found) => found,
                    Err(_) if indexed(&version) => continue,
                    Err(reason) => {
                        excluded.push(Excluded {
                            subject: subject(&source, &registered, catalog),
                            tag: tag.name,
                            version,
                            reason,
                        });
                        continue;
                    }
                };
                for found in found {
                    match index.units.get_mut(&found.identity) {
                        Some(unit) if unit.source == source => {
                            if unit.versions.contains_key(&version) {
                                continue;
                            }
                            unit.versions.insert(version.clone(), found.provenance);
                        }
                        Some(unit) => {
                            errors.push(RegistryError::SourceConflict {
                                identity: found.identity,
                                registered: unit.source.clone(),
                                given: source.clone(),
                            });
                            continue;
                        }
                        None if catalog => {
                            index.units.insert(
                                found.identity.clone(),
                                RegisteredUnit::new(
                                    source.clone(),
                                    BTreeMap::from([(version.clone(), found.provenance)]),
                                ),
                            );
                        }
                        None => {
                            errors.push(RegistryError::UnexpectedIdentity {
                                identity: registered
                                    .first()
                                    .cloned()
                                    .unwrap_or_else(|| found.identity.clone()),
                                source: source.clone(),
                                version: version.clone(),
                                declared: found.identity,
                            });
                            continue;
                        }
                    }
                    added
                        .entry(found.identity)
                        .or_default()
                        .push(version.clone());
                }
            }
        }
        for (identity, unit) in &index.units {
            let ambiguous = ambiguous_versions(unit.versions.keys());
            if !ambiguous.is_empty() && self.units.get(identity) != Some(unit) {
                errors.push(RegistryError::AmbiguousVersions {
                    identity: identity.clone(),
                    source: unit.source.clone(),
                    versions: ambiguous,
                });
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        for versions in added.values_mut() {
            versions.sort();
        }
        excluded.sort_by(Excluded::order);
        Ok(Change {
            index,
            added,
            excluded,
        })
    }

    /// Removes the registration of `identity`, and of every other unit
    /// registered from its source: a catalog's units are registered and
    /// removed together (**Provisional (D-54)**). `None` if it is not
    /// registered.
    pub fn remove(&self, identity: &UnitIdentity) -> Option<Removed> {
        let source = self.units.get(identity)?.source.clone();
        let mut index = self.clone();
        let mut units = BTreeMap::new();
        index.units.retain(|identity, unit| {
            if unit.source == source {
                units.insert(identity.clone(), unit.clone());
                false
            } else {
                true
            }
        });
        Some(Removed {
            index,
            source,
            units,
        })
    }
}

/// What an invalid tag of `source` is reported for: its one unit, or the
/// catalog, whose tags publish all its units at once.
fn subject(
    source: &SourceReference,
    identities: &BTreeSet<UnitIdentity>,
    catalog: bool,
) -> Subject {
    match identities.first() {
        Some(identity) if identities.len() == 1 && !catalog => Subject::Unit(identity.clone()),
        _ => Subject::Catalog(source.clone()),
    }
}

/// A removed registration.
#[derive(Debug, Clone, PartialEq)]
pub struct Removed {
    index: RegistryIndex,
    source: SourceReference,
    units: BTreeMap<UnitIdentity, RegisteredUnit>,
}

impl Removed {
    /// The index without the registration.
    pub fn index(&self) -> &RegistryIndex {
        &self.index
    }

    /// The source whose units were removed.
    pub fn source(&self) -> &SourceReference {
        &self.source
    }

    /// The registrations as they were, by identity: one, or every unit of a
    /// catalog.
    pub fn units(&self) -> &BTreeMap<UnitIdentity, RegisteredUnit> {
        &self.units
    }
}

/// The identities the valid tags of `source` publish: its one unit's, or
/// every unit any version of a catalog publishes. Empty if no tag is valid.
/// Tags that each publish one unit but declare different identities reject
/// the source: a unit's repository publishes one unit.
pub fn declared_identities<R>(
    source: &SourceReference,
    tags: &[ObservedTag<R>],
) -> Result<BTreeSet<UnitIdentity>, Vec<RegistryError<R>>> {
    let mut identities = BTreeSet::new();
    let mut single: BTreeMap<UnitIdentity, Vec<Version>> = BTreeMap::new();
    for tag in tags {
        let Ok(found) = &tag.outcome else {
            continue;
        };
        for found in found {
            identities.insert(found.identity.clone());
            if !tag.catalog {
                single
                    .entry(found.identity.clone())
                    .or_default()
                    .push(tag.version.clone());
            }
        }
    }
    if single.len() > 1 {
        for versions in single.values_mut() {
            versions.sort();
        }
        return Err(vec![RegistryError::ConflictingIdentities {
            source: source.clone(),
            identities: single,
        }]);
    }
    Ok(identities)
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
    outcome: Result<Vec<ObservedVersion>, R>,
    /// Whether the tag is a catalog's, which publishes all its units.
    catalog: bool,
}

impl<R> ObservedTag<R> {
    /// The tag `name` of a unit's repository, which names `version`, and
    /// what reading it found.
    pub fn new(name: String, version: Version, outcome: Result<ObservedVersion, R>) -> Self {
        Self {
            name,
            version,
            outcome: outcome.map(|found| vec![found]),
            catalog: false,
        }
    }

    /// The tag `name` of a catalog, which names `version`, and what reading
    /// it found: the version of every unit in the catalog.
    pub fn of_catalog(
        name: String,
        version: Version,
        outcome: Result<Vec<ObservedVersion>, R>,
    ) -> Self {
        Self {
            name,
            version,
            outcome,
            catalog: true,
        }
    }

    /// The tag's version.
    pub fn version(&self) -> &Version {
        &self.version
    }

    /// The valid versions it publishes, one for each unit, or why the tag
    /// does not publish any.
    pub fn outcome(&self) -> Result<&[ObservedVersion], &R> {
        self.outcome.as_deref()
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

    /// The index after the change, and the excluded tags.
    pub fn into_parts(self) -> (RegistryIndex, Vec<Excluded<R>>) {
        (self.index, self.excluded)
    }
}

/// A version tag excluded from the index because it is invalid.
#[derive(Debug, Clone, PartialEq)]
pub struct Excluded<R> {
    subject: Subject,
    tag: String,
    version: Version,
    reason: R,
}

impl<R> Excluded<R> {
    fn order(a: &Self, b: &Self) -> Ordering {
        (&a.subject, &a.version, &a.tag).cmp(&(&b.subject, &b.version, &b.tag))
    }

    /// The unit the tag belongs to, or the catalog.
    pub fn subject(&self) -> &Subject {
        &self.subject
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

/// What a tag belongs to: a unit's repository, or a catalog, whose tags
/// publish every unit in it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Subject {
    /// The unit.
    Unit(UnitIdentity),
    /// The catalog registered from this source.
    Catalog(SourceReference),
}

impl fmt::Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unit(identity) => identity.fmt(f),
            Self::Catalog(source) => write!(f, "the catalog {source}"),
        }
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
    /// The tag, a catalog's, no longer publishes the unit.
    UnitMissing,
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
                    IntegrityChange::UnitMissing => write!(
                        f,
                        "tag v{version} no longer publishes it; a published version never \
                         changes"
                    ),
                }
            }
        }
    }
}

impl<R: fmt::Debug + fmt::Display> std::error::Error for RegistryError<R> {}

/// Why a stored index breaks the index's invariants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidIndex {
    /// A unit has versions of equal precedence.
    Ambiguous {
        /// The unit.
        identity: UnitIdentity,
        /// The versions of equal precedence.
        versions: Vec<Version>,
    },
}

impl fmt::Display for InvalidIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ambiguous { identity, versions } => write!(
                f,
                "{identity} has versions of equal precedence: {}",
                list(versions)
            ),
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
    use std::collections::{BTreeMap, BTreeSet};

    use super::{
        Change, IntegrityChange, ObservedTag, ObservedVersion, RegisteredUnit, RegistryError,
        RegistryIndex, SourceReference, declared_identities,
    };
    use crate::domain::{ObjectId, Provenance, UnitIdentity, Version};

    fn id(text: &str) -> UnitIdentity {
        UnitIdentity::parse(text).unwrap()
    }

    fn ids(texts: &[&str]) -> BTreeSet<UnitIdentity> {
        texts.iter().map(|text| id(text)).collect()
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
                &ids(&["@acme/node"]),
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
                &ids(&["@acme/node"]),
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
                &ids(&["@acme/node"]),
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
                &ids(&["@acme/node"]),
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
                &ids(&["@acme/node"]),
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
            declared_identities(
                &reference,
                &[invalid("0.1.0", "bad"), valid("@acme/node", "1.0.0", 'a')]
            ),
            Ok(ids(&["@acme/node"]))
        );
        assert_eq!(
            declared_identities(&reference, &[invalid("0.1.0", "bad")]),
            Ok(BTreeSet::new())
        );

        let error = declared_identities(
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
                &ids(&["@acme/node"]),
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
                source("/units/node"),
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
                source("/units/node"),
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
                source("/units/node"),
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
                    source("/units/node"),
                    vec![
                        valid("@acme/node", "1.0.0", 'a'),
                        valid("@acme/node", "2.0.0", 'c'),
                    ],
                ),
                (source("/units/web"), vec![]),
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
                source("/units/node"),
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
                (source("/units/node"), tags()),
                (source("/units/web"), vec![invalid("1.0.0", "three")]),
            ]))
            .unwrap();

        for rotation in 1..5 {
            let mut shuffled = tags();
            shuffled.rotate_left(rotation);
            shuffled.swap(0, 3);
            let change = before
                .sync(BTreeMap::from([
                    (source("/units/node"), shuffled),
                    (source("/units/web"), vec![invalid("1.0.0", "three")]),
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

        assert_eq!(removed.source(), &source("/units/node"));
        assert_eq!(removed.index(), &index(&[("@acme/web", "/units/web", &[])]));
        assert_eq!(before.remove(&id("@acme/missing")), None);
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

        // A catalog registers several units from one source.
        assert_eq!(errors.len(), 1, "{errors:?}");
    }
}
