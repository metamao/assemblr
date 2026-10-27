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
/// Every source is registered for one identity only, and no unit has two
/// versions of equal SemVer precedence.
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
        }
        for (source, identities) in identities {
            if identities.len() > 1 {
                errors.push(InvalidIndex::SharedSource {
                    source: source.clone(),
                    identities,
                });
            }
        }
        errors
    }

    /// The registered units, ordered by identity.
    pub fn units(&self) -> &BTreeMap<UnitIdentity, RegisteredUnit> {
        &self.units
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
        if self.identity_of(&source).is_some() {
            return Ok(Change::unchanged(self));
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
        })
    }

    /// Removes the registration of `identity`. `None` if it is not
    /// registered.
    pub fn remove(&self, identity: &UnitIdentity) -> Option<Removed> {
        let mut index = self.clone();
        let unit = index.units.remove(identity)?;
        Some(Removed { index, unit })
    }
}

/// A removed registration.
#[derive(Debug, Clone, PartialEq)]
pub struct Removed {
    index: RegistryIndex,
    unit: RegisteredUnit,
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
        Change, IntegrityChange, ObservedTag, ObservedVersion, RegisteredUnit, RegistryError,
        RegistryIndex, SourceReference, declared_identity,
    };
    use crate::domain::{ObjectId, Provenance, UnitIdentity, Version};

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

        assert_eq!(errors.len(), 2, "{errors:?}");
    }
}
