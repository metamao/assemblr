//! `unit update`: the lock after an explicit update, and what the update
//! does with the unit's amendments (see `docs/domain/authoring-workflow.md`).
//!
//! An update resolves with the lock minus the units the author updates, so
//! the resolver keeps every other locked version that still satisfies the
//! requirements. What changed is then computed against the **complete**
//! previous lock, and a kept version must still be published with the
//! content it was locked to, whether or not the resolver was told about it
//! ([`update_lock`]).
//!
//! The unit's amendments are reconciled by the composer against their
//! layered bases (see `docs/domain/amendments-and-composition.md`): the
//! base before the update, the copy, and the base after it are merged
//! three ways, and what cannot be merged is left in the copy between
//! conflict markers. [`ReconcileError`] names the cases where no merge is
//! attempted, so that the update is refused before anything is written.
//! A published amendment whose provider resolves to another version than
//! its base is not a problem: composition applies it to the version
//! resolved.

use std::collections::BTreeMap;
use std::fmt;

use super::identity::UnitIdentity;
use super::lock::{LockError, LockedUnit, UnitLock};
use super::path::RelativePath;
use super::registry::RegistryIndex;
use super::resolution::{LockChange, ResolvedGraph};
use super::version::Version;

/// The lock that records `graph`, and how it differs from `previous`, with
/// each resolved version's provenance taken from `index`.
///
/// A unit resolved to the version `previous` locks is kept, and must still
/// be published with the content it was locked to; every one that is not
/// is reported, sorted by identity. The changes are computed against the
/// whole of `previous`, whatever lock the resolver was given.
pub fn update_lock(
    previous: &UnitLock,
    graph: &ResolvedGraph,
    index: &RegistryIndex,
) -> Result<(UnitLock, BTreeMap<UnitIdentity, LockChange>), Vec<LockError>> {
    let mut units = BTreeMap::new();
    let mut changes = BTreeMap::new();
    let mut errors = Vec::new();
    for (identity, resolved) in graph.units() {
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
        match previous.units().get(identity) {
            Some(locked) if locked.version() == version => {
                if locked.provenance() != published {
                    errors.push(LockError::ContentChanged {
                        unit: identity.clone(),
                        version: version.clone(),
                        locked: locked.provenance().clone(),
                        published: published.clone(),
                    });
                    continue;
                }
            }
            Some(locked) => {
                changes.insert(
                    identity.clone(),
                    LockChange::Changed {
                        from: locked.version().clone(),
                        to: version.clone(),
                    },
                );
            }
            None => {
                changes.insert(identity.clone(), LockChange::Added(version.clone()));
            }
        }
        units.insert(
            identity.clone(),
            LockedUnit::new(version.clone(), published.clone()),
        );
    }
    for (identity, locked) in previous.units() {
        if !graph.units().contains_key(identity) {
            changes.insert(
                identity.clone(),
                LockChange::Retired(locked.version().clone()),
            );
        }
    }
    if errors.is_empty() {
        Ok((UnitLock::new(units), changes))
    } else {
        Err(errors)
    }
}

/// An amendment that an update cannot carry to the provider's new version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileError {
    path: RelativePath,
    unit: UnitIdentity,
    base: Version,
    resolved: Version,
    cause: ReconcileCause,
}

impl ReconcileError {
    /// An error for the amendment of `path`, of `unit`, based on `base`,
    /// which the update would move to `resolved`.
    pub(crate) fn new(
        path: RelativePath,
        unit: UnitIdentity,
        base: Version,
        resolved: Version,
        cause: ReconcileCause,
    ) -> Self {
        Self {
            path,
            unit,
            base,
            resolved,
            cause,
        }
    }

    /// The amended path.
    pub fn path(&self) -> &RelativePath {
        &self.path
    }

    /// The provider.
    pub fn unit(&self) -> &UnitIdentity {
        &self.unit
    }

    /// The declared base.
    pub fn base(&self) -> &Version {
        &self.base
    }

    /// The version the update selects.
    pub fn resolved(&self) -> &Version {
        &self.resolved
    }

    /// Why the amendment cannot be carried over.
    pub fn cause(&self) -> &ReconcileCause {
        &self.cause
    }
}

/// Why an update cannot carry an amendment to the provider's new version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileCause {
    /// The declaration names a version other than the locked one: it was
    /// edited by hand, so the amendment's real base is unknown.
    DeclarationChanged {
        /// The locked version.
        locked: Version,
    },
    /// The provider does not have the path at the declared base.
    NotProvidedAtBase,
    /// The provider no longer has the path at the new version.
    NoLongerProvided,
    /// The base changes with the update, and its format has no merge, or
    /// it or the copy is not text.
    Changed,
    /// The base changes with the update, and the merge of the copy onto
    /// the new base failed for the reason given.
    Unmergeable(String),
}

impl fmt::Display for ReconcileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            path,
            unit,
            base,
            resolved,
            cause,
        } = self;
        match cause {
            ReconcileCause::DeclarationChanged { locked } => write!(
                f,
                "amendments/{path} declares {unit} {base} as its base, but the lockfile locks \
                 {unit} {locked}; revert the declaration to {locked} before updating {unit}"
            ),
            ReconcileCause::NotProvidedAtBase => write!(
                f,
                "amendments/{path} is based on {unit} {base}, which the update would move to \
                 {resolved}, but {unit} {base} has no src/{path}"
            ),
            ReconcileCause::NoLongerProvided => write!(
                f,
                "amendments/{path} is based on {unit} {base}, but {unit} {resolved}, which the \
                 update would select, no longer has src/{path}; reconciling an amendment whose \
                 file was removed is not supported yet"
            ),
            ReconcileCause::Changed => write!(
                f,
                "amendments/{path} is based on {unit} {base}, and the file it amends changes \
                 with the update to {unit} {resolved}; the file is XML, or it or the copy is \
                 not UTF-8 text, so the copy follows it only when it is unchanged"
            ),
            ReconcileCause::Unmergeable(detail) => write!(
                f,
                "amendments/{path} is based on {unit} {base}, and the file it amends changes \
                 with the update to {unit} {resolved}, but the copy cannot be moved onto the \
                 new file: {detail}"
            ),
        }
    }
}

impl std::error::Error for ReconcileError {}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "tests fail on a bad fixture"
    )]

    use std::collections::BTreeMap;

    use super::super::provenance::{ObjectId, Provenance};
    use super::super::registry::{RegisteredUnit, SourceReference};
    use super::super::resolution::{AvailableVersions, Dependencies, resolve};
    use super::*;

    fn identity(input: &str) -> UnitIdentity {
        input.parse().unwrap()
    }

    fn version(input: &str) -> Version {
        input.parse().unwrap()
    }

    fn provenance(commit: char, tree: char) -> Provenance {
        Provenance::new(
            ObjectId::parse(&commit.to_string().repeat(40)).unwrap(),
            ObjectId::parse(&tree.to_string().repeat(40)).unwrap(),
        )
    }

    /// `@acme/node` 1.0.0, 1.1.0, and 2.0.0, each with its own provenance,
    /// and `@acme/amender` 0.1.0 and 0.2.0, which depend on `@acme/node`.
    fn index() -> RegistryIndex {
        let versions = |entries: &[(&str, Provenance)]| {
            entries
                .iter()
                .map(|(v, p)| (version(v), p.clone()))
                .collect()
        };
        RegistryIndex::from_units(BTreeMap::from([
            (
                identity("@acme/node"),
                RegisteredUnit::new(
                    SourceReference::new("/units/node"),
                    versions(&[
                        ("1.0.0", provenance('1', 'a')),
                        ("1.1.0", provenance('2', 'b')),
                        ("2.0.0", provenance('3', 'c')),
                    ]),
                ),
            ),
            (
                identity("@acme/amender"),
                RegisteredUnit::new(
                    SourceReference::new("/units/amender"),
                    versions(&[
                        ("0.1.0", provenance('5', 'e')),
                        ("0.2.0", provenance('6', 'f')),
                    ]),
                ),
            ),
        ]))
        .unwrap()
    }

    /// Every indexed version; the amender's versions depend on `@acme/node`
    /// 1.x.
    fn available(index: &RegistryIndex) -> AvailableVersions {
        index
            .units()
            .iter()
            .map(|(identity, unit)| {
                let dependencies = if *identity == self::identity("@acme/amender") {
                    requirements("^1")
                } else {
                    Dependencies::new()
                };
                (
                    identity.clone(),
                    unit.versions()
                        .keys()
                        .map(|version| (version.clone(), dependencies.clone()))
                        .collect(),
                )
            })
            .collect()
    }

    fn requirements(range: &str) -> Dependencies {
        BTreeMap::from([(identity("@acme/node"), range.parse().unwrap())])
    }

    fn lock(entries: &[(&str, &str, Provenance)]) -> UnitLock {
        UnitLock::new(
            entries
                .iter()
                .map(|(unit, version, provenance)| {
                    (
                        identity(unit),
                        LockedUnit::new(self::version(version), provenance.clone()),
                    )
                })
                .collect(),
        )
    }

    fn graph(range: &str, lock: &UnitLock) -> ResolvedGraph {
        graph_of(&requirements(range), lock)
    }

    fn graph_of(requirements: &Dependencies, lock: &UnitLock) -> ResolvedGraph {
        let index = index();
        resolve(None, requirements, &available(&index), &lock.versions())
            .unwrap()
            .graph()
            .clone()
    }

    #[test]
    fn an_unlocked_unit_that_resolves_to_its_locked_version_is_kept_and_checked() {
        let previous = lock(&[("@acme/node", "1.1.0", provenance('2', 'b'))]);
        let graph = graph("^1", &UnitLock::default());

        let (lock, changes) = update_lock(&previous, &graph, &index()).unwrap();

        assert_eq!(lock, previous);
        assert!(changes.is_empty());
    }

    #[test]
    fn a_kept_version_whose_content_changed_is_rejected_even_when_unlocked() {
        let previous = lock(&[("@acme/node", "1.1.0", provenance('9', 'b'))]);
        let graph = graph("^1", &UnitLock::default());

        let errors = update_lock(&previous, &graph, &index()).unwrap_err();

        assert_eq!(
            errors,
            [LockError::ContentChanged {
                unit: identity("@acme/node"),
                version: version("1.1.0"),
                locked: provenance('9', 'b'),
                published: provenance('2', 'b'),
            }]
        );
    }

    #[test]
    fn changes_are_computed_against_the_whole_previous_lock() {
        let previous = lock(&[
            ("@acme/node", "1.0.0", provenance('1', 'a')),
            ("@acme/gone", "0.1.0", provenance('5', 'e')),
        ]);
        let graph = graph("^1", &UnitLock::default());

        let (lock, changes) = update_lock(&previous, &graph, &index()).unwrap();

        assert_eq!(
            lock,
            self::lock(&[("@acme/node", "1.1.0", provenance('2', 'b'))])
        );
        assert_eq!(
            changes,
            BTreeMap::from([
                (
                    identity("@acme/node"),
                    LockChange::Changed {
                        from: version("1.0.0"),
                        to: version("1.1.0"),
                    }
                ),
                (
                    identity("@acme/gone"),
                    LockChange::Retired(version("0.1.0"))
                ),
            ])
        );
    }
}
