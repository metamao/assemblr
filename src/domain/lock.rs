//! A unit's lock state: the exact published version of every unit its
//! development resolves to, and the content each version was resolved to.
//!
//! The lock is what `unit build` resolves against. A locked version is kept
//! while it satisfies every requirement, so a version published later never
//! changes it (see `docs/domain/authoring-workflow.md`). The lock also
//! records each version's provenance, so a locked version whose published
//! content is no longer the content it was locked to is detected rather than
//! used.

use std::collections::BTreeMap;
use std::fmt;

use super::identity::UnitIdentity;
use super::provenance::Provenance;
use super::registry::RegistryIndex;
use super::resolution::{Lock, Resolution};
use super::version::Version;

/// The lock state of a unit repository: each resolved unit, direct and
/// transitive, by identity.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnitLock {
    units: BTreeMap<UnitIdentity, LockedUnit>,
}

impl UnitLock {
    /// A lock of exactly these units.
    pub fn new(units: BTreeMap<UnitIdentity, LockedUnit>) -> Self {
        Self { units }
    }

    /// The locked units, by identity.
    pub fn units(&self) -> &BTreeMap<UnitIdentity, LockedUnit> {
        &self.units
    }

    /// The locked version of each identity, for [`resolve`](super::resolve).
    pub fn versions(&self) -> Lock {
        self.units
            .iter()
            .map(|(identity, unit)| (identity.clone(), unit.version.clone()))
            .collect()
    }
}

/// One locked unit: its exact version and the content it was locked to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedUnit {
    version: Version,
    provenance: Provenance,
}

impl LockedUnit {
    /// `version`, published with the content `provenance` identifies.
    pub fn new(version: Version, provenance: Provenance) -> Self {
        Self {
            version,
            provenance,
        }
    }

    /// The exact version.
    pub fn version(&self) -> &Version {
        &self.version
    }

    /// The commit and tree the version was published with when it was
    /// locked.
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
}

/// The lock after `resolution`, which resolved against `previous`, with each
/// resolved version's provenance taken from `index`.
///
/// A locked version that `resolution` keeps must still be published with
/// the content it was locked to. Every one that is not is reported, sorted
/// by identity.
pub fn relock(
    previous: &UnitLock,
    resolution: &Resolution,
    index: &RegistryIndex,
) -> Result<UnitLock, Vec<LockError>> {
    let mut units = BTreeMap::new();
    let mut errors = Vec::new();
    for (identity, resolved) in resolution.graph().units() {
        let version = resolved.version();
        let Some(published) = index
            .units()
            .get(identity)
            .and_then(|unit| unit.versions().get(version))
        else {
            errors.push(LockError::NotIndexed {
                unit: identity.clone(),
                version: version.clone(),
            });
            continue;
        };
        let kept = !resolution.changes().contains_key(identity);
        if let Some(locked) = previous.units.get(identity).filter(|_| kept) {
            if locked.provenance != *published {
                errors.push(LockError::ContentChanged {
                    unit: identity.clone(),
                    version: version.clone(),
                    locked: locked.provenance.clone(),
                    published: published.clone(),
                });
                continue;
            }
        }
        units.insert(
            identity.clone(),
            LockedUnit::new(version.clone(), published.clone()),
        );
    }
    if errors.is_empty() {
        Ok(UnitLock { units })
    } else {
        Err(errors)
    }
}

/// A problem with the lock that a resolution produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockError {
    /// A locked version is still published, but with other content than it
    /// was locked to: its tag was moved and the source registered again, or
    /// the lock comes from a source with other history.
    ContentChanged {
        /// The locked unit.
        unit: UnitIdentity,
        /// Its locked version.
        version: Version,
        /// What it was locked to.
        locked: Provenance,
        /// What the registry records for it now.
        published: Provenance,
    },
    /// A resolved version is not in the registry index. Resolution only
    /// selects indexed versions, so this is a defect in the caller.
    NotIndexed {
        /// The resolved unit.
        unit: UnitIdentity,
        /// Its resolved version.
        version: Version,
    },
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ContentChanged {
                unit,
                version,
                locked,
                published,
            } => write!(
                f,
                "{unit} {version} is locked to commit {} (tree {}), but the registry now \
                 records commit {} (tree {}) for it; a published version's content must not \
                 change",
                locked.commit(),
                locked.tree(),
                published.commit(),
                published.tree()
            ),
            Self::NotIndexed { unit, version } => write!(
                f,
                "internal error: {unit} {version} was resolved but is not in the registry index"
            ),
        }
    }
}

impl std::error::Error for LockError {}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{LockError, LockedUnit, UnitLock, relock};
    use crate::domain::{
        AvailableVersions, Dependencies, ObjectId, Provenance, RegisteredUnit, RegistryIndex,
        SourceReference, UnitIdentity, Version, resolve,
    };

    fn identity(input: &str) -> UnitIdentity {
        input.parse().unwrap()
    }

    fn version(input: &str) -> Version {
        input.parse().unwrap()
    }

    /// A provenance whose commit and tree are made of `digit`.
    fn provenance(commit: char, tree: char) -> Provenance {
        Provenance::new(
            ObjectId::parse(&commit.to_string().repeat(40)).unwrap(),
            ObjectId::parse(&tree.to_string().repeat(40)).unwrap(),
        )
    }

    /// An index publishing `@acme/node` 1.0.0 and 1.1.0 with no
    /// dependencies, 1.0.0 with the content `published`.
    fn index(published: &Provenance) -> RegistryIndex {
        let versions = BTreeMap::from([
            (version("1.0.0"), published.clone()),
            (version("1.1.0"), provenance('3', '4')),
        ]);
        RegistryIndex::from_units(BTreeMap::from([(
            identity("@acme/node"),
            RegisteredUnit::new(SourceReference::new("/units/node"), versions),
        )]))
        .unwrap()
    }

    fn available(index: &RegistryIndex) -> AvailableVersions {
        index
            .units()
            .iter()
            .map(|(identity, unit)| {
                let versions = unit
                    .versions()
                    .keys()
                    .map(|version| (version.clone(), Dependencies::new()))
                    .collect();
                (identity.clone(), versions)
            })
            .collect()
    }

    fn requirements(range: &str) -> Dependencies {
        Dependencies::from([(identity("@acme/node"), range.parse().unwrap())])
    }

    fn lock(entries: &[(&str, &str, Provenance)]) -> UnitLock {
        UnitLock::new(
            entries
                .iter()
                .map(|(name, locked, provenance)| {
                    (
                        identity(name),
                        LockedUnit::new(version(locked), provenance.clone()),
                    )
                })
                .collect(),
        )
    }

    fn relocked(
        previous: &UnitLock,
        range: &str,
        index: &RegistryIndex,
    ) -> Result<UnitLock, Vec<LockError>> {
        let resolution = resolve(
            None,
            &requirements(range),
            &available(index),
            &previous.versions(),
        )
        .unwrap();
        relock(previous, &resolution, index)
    }

    #[test]
    fn a_new_lock_records_each_resolved_version_with_its_provenance() {
        let index = index(&provenance('1', '2'));
        let locked = relocked(&UnitLock::default(), "^1", &index).unwrap();
        assert_eq!(
            locked,
            lock(&[("@acme/node", "1.1.0", provenance('3', '4'))])
        );
    }

    #[test]
    fn a_kept_version_keeps_its_content() {
        let index = index(&provenance('1', '2'));
        let previous = lock(&[("@acme/node", "1.0.0", provenance('1', '2'))]);
        assert_eq!(relocked(&previous, "^1", &index).unwrap(), previous);
    }

    #[test]
    fn a_kept_version_whose_published_content_changed_is_rejected() {
        let index = index(&provenance('5', '6'));
        let previous = lock(&[("@acme/node", "1.0.0", provenance('1', '2'))]);
        let errors = relocked(&previous, "^1", &index).unwrap_err();
        assert_eq!(
            errors,
            [LockError::ContentChanged {
                unit: identity("@acme/node"),
                version: version("1.0.0"),
                locked: provenance('1', '2'),
                published: provenance('5', '6'),
            }]
        );
        let message = errors[0].to_string();
        assert!(message.contains(&"1".repeat(40)), "{message}");
        assert!(message.contains(&"5".repeat(40)), "{message}");
    }

    #[test]
    fn a_replaced_version_is_not_compared_with_its_old_content() {
        // The range no longer allows the locked version, so its old content
        // does not matter.
        let index = index(&provenance('5', '6'));
        let previous = lock(&[("@acme/node", "1.0.0", provenance('1', '2'))]);
        assert_eq!(
            relocked(&previous, "^1.1", &index).unwrap(),
            lock(&[("@acme/node", "1.1.0", provenance('3', '4'))])
        );
    }

    #[test]
    fn a_unit_no_longer_required_leaves_the_lock() {
        let index = index(&provenance('1', '2'));
        let previous = lock(&[
            ("@acme/node", "1.0.0", provenance('1', '2')),
            ("@acme/old", "2.0.0", provenance('7', '8')),
        ]);
        assert_eq!(
            relocked(&previous, "^1", &index).unwrap(),
            lock(&[("@acme/node", "1.0.0", provenance('1', '2'))])
        );
    }
}
