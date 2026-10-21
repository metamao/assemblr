//! A unit's lock state: the exact published version of every unit its
//! development resolves to, and the content each version was resolved to.
//!
//! The lock is what `unit build` resolves against. A locked version is kept
//! while it satisfies every requirement, so a version published later never
//! changes it (see `docs/domain/authoring-workflow.md`). The lock also
//! records each version's provenance, so a locked version whose published
//! content is no longer the content it was locked to is detected rather than
//! used.
//!
//! A published version's lockfile also identifies the bases of its
//! amendments: the entry for an amended provider records the base version
//! with the commit and tree it was published with. [`published_amendments`]
//! checks that a published version's declarations, copies, and lockfile
//! agree, and [`verify_bases`] checks each base against the composer's own
//! registry record (see `docs/domain/amendments-and-composition.md`).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use super::identity::UnitIdentity;
use super::manifest::Manifest;
use super::path::RelativePath;
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

/// Checks that `resolution` keeps every amended unit of `manifest` at its
/// declared base, which the lock state `previous` records.
///
/// The lockfile entry for an amended unit is the base's identity, and a
/// build never changes it: changing the base is an explicit `unit update`
/// (see `docs/domain/authoring-workflow.md`). Every amendment whose provider
/// resolves to another version is reported, sorted by path, and the error
/// tells apart the causes: the resolution moved away from the locked base,
/// the declaration no longer names the locked version, or the unit is not
/// locked at all.
pub fn amendment_bases(
    manifest: &Manifest,
    previous: &UnitLock,
    resolution: &Resolution,
) -> Vec<AmendmentBaseError> {
    let mut errors = Vec::new();
    for (path, declaration) in manifest.amendments() {
        let unit = declaration.unit();
        let base = declaration.base();
        let Some(resolved) = resolution.graph().units().get(unit) else {
            continue;
        };
        let resolved = resolved.version();
        if resolved == base {
            continue;
        }
        let error = match previous.units.get(unit) {
            Some(locked) if locked.version == *base => AmendmentBaseError::ResolutionChanged {
                path: path.clone(),
                unit: unit.clone(),
                base: base.clone(),
                resolved: resolved.clone(),
            },
            Some(locked) => AmendmentBaseError::DeclarationChanged {
                path: path.clone(),
                unit: unit.clone(),
                base: base.clone(),
                locked: locked.version.clone(),
                resolved: resolved.clone(),
            },
            None => AmendmentBaseError::NotLocked {
                path: path.clone(),
                unit: unit.clone(),
                base: base.clone(),
                resolved: resolved.clone(),
            },
        };
        errors.push(error);
    }
    errors
}

/// Why a build cannot keep an amendment's provider at the declared base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AmendmentBaseError {
    /// The lock agrees with the declaration, but the resolution would move
    /// the provider to another version, for example after a range was
    /// edited. Reconciling the amendment is `unit update`'s work.
    ResolutionChanged {
        /// The amended path.
        path: RelativePath,
        /// The provider.
        unit: UnitIdentity,
        /// The declared and locked base.
        base: Version,
        /// The version the resolution would lock instead.
        resolved: Version,
    },
    /// The declaration names a version other than the locked one, which the
    /// resolution keeps: the declaration was edited by hand.
    DeclarationChanged {
        /// The amended path.
        path: RelativePath,
        /// The provider.
        unit: UnitIdentity,
        /// The declared base.
        base: Version,
        /// The locked version.
        locked: Version,
        /// The resolved version.
        resolved: Version,
    },
    /// The provider is not locked, and the resolution picks a version other
    /// than the declared base.
    NotLocked {
        /// The amended path.
        path: RelativePath,
        /// The provider.
        unit: UnitIdentity,
        /// The declared base.
        base: Version,
        /// The resolved version.
        resolved: Version,
    },
}

impl fmt::Display for AmendmentBaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResolutionChanged {
                path,
                unit,
                base,
                resolved,
            } => write!(
                f,
                "amendments/{path} is based on {unit} {base}, the locked version, but the \
                 dependencies now resolve {unit} to {resolved}; a build never changes the locked \
                 version of an amended unit, and reconciling the amendment to another version \
                 (`unit update`) is not supported yet, so restore a requirement that {base} \
                 satisfies"
            ),
            Self::DeclarationChanged {
                path,
                unit,
                base,
                locked,
                resolved,
            } => write!(
                f,
                "amendments/{path} declares {unit} {base} as its base, but the lockfile locks \
                 {unit} {locked} and the dependencies resolve it to {resolved}; the declared base \
                 was edited by hand, so revert it to {locked}"
            ),
            Self::NotLocked {
                path,
                unit,
                base,
                resolved,
            } => write!(
                f,
                "amendments/{path} is based on {unit} {base}, but {unit} is not locked and the \
                 dependencies resolve it to {resolved}; restore the lockfile that locks {unit} \
                 to {base}"
            ),
        }
    }
}

impl std::error::Error for AmendmentBaseError {}

/// Checks that a published version's amendment declarations in `manifest`,
/// its copies under `amendments/` at `copies`, and its lockfile `lock`
/// agree: every copy is declared, every declaration has a copy, and, when
/// anything is declared, the lockfile exists and locks each amended provider
/// at the declared base. These are the properties a version shows on its
/// own; whether a base's content is the one the composer's registry records
/// is [`verify_bases`]. Every problem is reported, sorted by path.
pub fn published_amendments<'a>(
    manifest: &Manifest,
    copies: impl IntoIterator<Item = &'a RelativePath>,
    lock: Option<&UnitLock>,
) -> Vec<PublishedAmendmentError> {
    let declared = manifest.amendments();
    let copies: BTreeSet<&RelativePath> = copies.into_iter().collect();
    let mut errors: Vec<PublishedAmendmentError> = copies
        .iter()
        .filter(|path| !declared.contains_key(*path))
        .map(|path| PublishedAmendmentError::Undeclared {
            path: (*path).clone(),
        })
        .collect();
    for (path, declaration) in declared {
        let unit = declaration.unit();
        let base = declaration.base();
        if !copies.contains(path) {
            errors.push(PublishedAmendmentError::MissingCopy {
                path: path.clone(),
                unit: unit.clone(),
            });
        }
        match lock.and_then(|lock| lock.units.get(unit)) {
            Some(locked) if locked.version == *base => {}
            Some(locked) => errors.push(PublishedAmendmentError::BaseNotLocked {
                path: path.clone(),
                unit: unit.clone(),
                base: base.clone(),
                locked: locked.version.clone(),
            }),
            None if lock.is_some() => errors.push(PublishedAmendmentError::NotLocked {
                path: path.clone(),
                unit: unit.clone(),
                base: base.clone(),
            }),
            None => {}
        }
    }
    if lock.is_none() && !declared.is_empty() {
        errors.push(PublishedAmendmentError::NoLockfile);
    }
    errors.sort_by_key(|error| match error {
        PublishedAmendmentError::NoLockfile => None,
        PublishedAmendmentError::Undeclared { path }
        | PublishedAmendmentError::MissingCopy { path, .. }
        | PublishedAmendmentError::NotLocked { path, .. }
        | PublishedAmendmentError::BaseNotLocked { path, .. } => Some(path.clone()),
    });
    errors
}

/// Why a published version's amendments are not consistent with its
/// manifest and lockfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishedAmendmentError {
    /// A copy under `amendments/` that the manifest does not declare.
    Undeclared {
        /// The copy's path.
        path: RelativePath,
    },
    /// A declaration whose copy does not exist.
    MissingCopy {
        /// The amended path.
        path: RelativePath,
        /// The provider the declaration names.
        unit: UnitIdentity,
    },
    /// The version declares amendments but carries no lockfile, so its
    /// bases are not identified.
    NoLockfile,
    /// The lockfile has no entry for an amended provider.
    NotLocked {
        /// The amended path.
        path: RelativePath,
        /// The provider.
        unit: UnitIdentity,
        /// The declared base.
        base: Version,
    },
    /// The lockfile locks an amended provider at a version other than the
    /// declared base.
    BaseNotLocked {
        /// The amended path.
        path: RelativePath,
        /// The provider.
        unit: UnitIdentity,
        /// The declared base.
        base: Version,
        /// The locked version.
        locked: Version,
    },
}

impl fmt::Display for PublishedAmendmentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Undeclared { path } => {
                write!(f, "amendments/{path} is not declared in its manifest")
            }
            Self::MissingCopy { path, unit } => write!(
                f,
                "its manifest declares an amendment of {path} from {unit}, but it has no \
                 amendments/{path}"
            ),
            Self::NoLockfile => f.write_str(
                "it declares amendments but has no assemblr-lock.json, which must identify each \
                 amendment's base",
            ),
            Self::NotLocked { path, unit, base } => write!(
                f,
                "amendments/{path} is based on {unit} {base}, but its assemblr-lock.json does not \
                 lock {unit}"
            ),
            Self::BaseNotLocked {
                path,
                unit,
                base,
                locked,
            } => write!(
                f,
                "amendments/{path} is based on {unit} {base}, but its assemblr-lock.json locks \
                 {unit} {locked}"
            ),
        }
    }
}

impl std::error::Error for PublishedAmendmentError {}

/// Verifies the bases of the amendments that `manifest`, the manifest of
/// the published version `version` of `amender`, declares: the entry of the
/// amender's published lockfile `lock` for each provider at the declared
/// base must identify the same commit and tree as `index` records for that
/// provider and version. A disagreement is an integrity conflict. A base
/// the index does not record is not reported: the provider then resolves
/// to another version, which the composition reports. Errors are sorted by
/// path.
pub fn verify_bases(
    amender: &UnitIdentity,
    version: &Version,
    manifest: &Manifest,
    lock: &UnitLock,
    index: &RegistryIndex,
) -> Vec<BaseIntegrityError> {
    let mut errors = Vec::new();
    for (path, declaration) in manifest.amendments() {
        let provider = declaration.unit();
        let base = declaration.base();
        let Some(recorded) = index
            .units()
            .get(provider)
            .and_then(|unit| unit.versions().get(base))
        else {
            continue;
        };
        let Some(locked) = lock
            .units
            .get(provider)
            .filter(|locked| locked.version == *base)
        else {
            continue;
        };
        if locked.provenance != *recorded {
            errors.push(BaseIntegrityError {
                amender: amender.clone(),
                version: version.clone(),
                path: path.clone(),
                provider: provider.clone(),
                base: base.clone(),
                published: locked.provenance.clone(),
                recorded: recorded.clone(),
            });
        }
    }
    errors
}

/// An amendment whose base, as the amending unit's published lockfile
/// identifies it, is not the content the registry records for that version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseIntegrityError {
    amender: UnitIdentity,
    version: Version,
    path: RelativePath,
    provider: UnitIdentity,
    base: Version,
    published: Provenance,
    recorded: Provenance,
}

impl BaseIntegrityError {
    /// The amending unit.
    pub fn amender(&self) -> &UnitIdentity {
        &self.amender
    }

    /// Its published version.
    pub fn version(&self) -> &Version {
        &self.version
    }

    /// The amended path.
    pub fn path(&self) -> &RelativePath {
        &self.path
    }

    /// The provider.
    pub fn provider(&self) -> &UnitIdentity {
        &self.provider
    }

    /// The declared base version.
    pub fn base(&self) -> &Version {
        &self.base
    }

    /// What the amender's lockfile identifies the base as.
    pub fn published(&self) -> &Provenance {
        &self.published
    }

    /// What the registry records for the provider at the base version.
    pub fn recorded(&self) -> &Provenance {
        &self.recorded
    }
}

impl fmt::Display for BaseIntegrityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "integrity conflict: {} {} amends {} of {} {}, whose content its lockfile identifies \
             as commit {} (tree {}), but the registry records commit {} (tree {}) for {} {}; a \
             published version's content must not change, and an amendment is applied only to \
             its verified base",
            self.amender,
            self.version,
            self.path,
            self.provider,
            self.base,
            self.published.commit(),
            self.published.tree(),
            self.recorded.commit(),
            self.recorded.tree(),
            self.provider,
            self.base
        )
    }
}

impl std::error::Error for BaseIntegrityError {}

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

    use super::{
        AmendmentBaseError, BaseIntegrityError, LockError, LockedUnit, PublishedAmendmentError,
        UnitLock, amendment_bases, published_amendments, relock, verify_bases,
    };
    use crate::domain::{
        AmendmentDeclaration, AvailableVersions, Dependencies, Description, Manifest, ObjectId,
        Provenance, RegisteredUnit, RegistryIndex, RelativePath, SourceReference, UnitIdentity,
        Version, resolve,
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

    /// `@acme/app`, depending on `@acme/node` with `range` and amending its
    /// `node.txt` at `base`.
    fn amending(range: &str, base: &str) -> Manifest {
        Manifest::new(
            identity("@acme/app"),
            version("0.1.0"),
            Description::new("An app.").unwrap(),
            requirements(range),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::from([(
                "node.txt".parse().unwrap(),
                AmendmentDeclaration::new(identity("@acme/node"), version(base)),
            )]),
        )
        .unwrap()
    }

    fn bases(manifest: &Manifest, previous: &UnitLock) -> Vec<AmendmentBaseError> {
        let index = index(&provenance('1', '2'));
        let resolution = resolve(
            Some(manifest.name()),
            manifest.dependencies(),
            &available(&index),
            &previous.versions(),
        )
        .unwrap();
        amendment_bases(manifest, previous, &resolution)
    }

    #[test]
    fn an_amendment_at_the_locked_version_passes() {
        let previous = lock(&[("@acme/node", "1.0.0", provenance('1', '2'))]);
        assert_eq!(bases(&amending("^1", "1.0.0"), &previous), []);
        // Without a lock, a resolution that picks the base is fine too.
        assert_eq!(
            bases(&amending("=1.0.0", "1.0.0"), &UnitLock::default()),
            []
        );
    }

    #[test]
    fn each_cause_of_a_moved_base_is_told_apart() {
        let path = || "node.txt".parse().unwrap();
        // The range no longer allows the locked base.
        let previous = lock(&[("@acme/node", "1.0.0", provenance('1', '2'))]);
        let errors = bases(&amending("^1.1", "1.0.0"), &previous);
        assert_eq!(
            errors,
            [AmendmentBaseError::ResolutionChanged {
                path: path(),
                unit: identity("@acme/node"),
                base: version("1.0.0"),
                resolved: version("1.1.0"),
            }]
        );
        assert!(errors[0].to_string().contains("not supported yet"));

        // The declaration was edited; the lock still holds.
        let errors = bases(&amending("^1", "1.1.0"), &previous);
        assert_eq!(
            errors,
            [AmendmentBaseError::DeclarationChanged {
                path: path(),
                unit: identity("@acme/node"),
                base: version("1.1.0"),
                locked: version("1.0.0"),
                resolved: version("1.0.0"),
            }]
        );
        assert!(errors[0].to_string().contains("revert it to 1.0.0"));

        // No lock at all, and the resolution prefers the newest version.
        let errors = bases(&amending("^1", "1.0.0"), &UnitLock::default());
        assert_eq!(
            errors,
            [AmendmentBaseError::NotLocked {
                path: path(),
                unit: identity("@acme/node"),
                base: version("1.0.0"),
                resolved: version("1.1.0"),
            }]
        );
        assert!(errors[0].to_string().contains("restore the lockfile"));
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

    fn path(text: &str) -> RelativePath {
        RelativePath::parse(text).unwrap()
    }

    /// `@acme/app` 0.1.0 depending on `@acme/node` and amending `paths` of
    /// it at base 1.0.0.
    fn amender(paths: &[&str]) -> Manifest {
        let amendments = paths
            .iter()
            .map(|amended| {
                (
                    path(amended),
                    AmendmentDeclaration::new(identity("@acme/node"), version("1.0.0")),
                )
            })
            .collect();
        Manifest::new(
            identity("@acme/app"),
            version("0.1.0"),
            Description::new("An app.").unwrap(),
            requirements("^1"),
            BTreeMap::new(),
            BTreeMap::new(),
            amendments,
        )
        .unwrap()
    }

    #[test]
    fn consistent_published_amendments_have_no_problem() {
        let manifest = amender(&["bin/run", "package.json.assemblr.tmpl"]);
        let lock = lock(&[("@acme/node", "1.0.0", provenance('1', '2'))]);
        let copies = [path("package.json.assemblr.tmpl"), path("bin/run")];
        assert_eq!(
            published_amendments(&manifest, copies.iter(), Some(&lock)),
            []
        );
        // Without declarations or copies, no lockfile is needed.
        assert_eq!(published_amendments(&amender(&[]), [].iter(), None), []);
    }

    #[test]
    fn every_inconsistency_of_published_amendments_is_reported_by_path() {
        let manifest = amender(&["bin/run", "missing.txt", "other.txt"]);
        let lock = lock(&[("@acme/node", "1.1.0", provenance('3', '4'))]);
        let copies = [path("stray.txt"), path("bin/run"), path("other.txt")];
        let node = identity("@acme/node");
        let not_locked_at_base = |amended: &str| PublishedAmendmentError::BaseNotLocked {
            path: path(amended),
            unit: node.clone(),
            base: version("1.0.0"),
            locked: version("1.1.0"),
        };
        assert_eq!(
            published_amendments(&manifest, copies.iter(), Some(&lock)),
            [
                not_locked_at_base("bin/run"),
                PublishedAmendmentError::MissingCopy {
                    path: path("missing.txt"),
                    unit: node.clone(),
                },
                not_locked_at_base("missing.txt"),
                not_locked_at_base("other.txt"),
                PublishedAmendmentError::Undeclared {
                    path: path("stray.txt"),
                },
            ]
        );

        // A lockfile without the provider, and no lockfile at all.
        let unrelated = self::lock(&[("@acme/other", "1.0.0", provenance('1', '2'))]);
        let manifest = amender(&["bin/run"]);
        assert_eq!(
            published_amendments(&manifest, [path("bin/run")].iter(), Some(&unrelated)),
            [PublishedAmendmentError::NotLocked {
                path: path("bin/run"),
                unit: node.clone(),
                base: version("1.0.0"),
            }]
        );
        let errors = published_amendments(&manifest, [path("bin/run")].iter(), None);
        assert_eq!(errors, [PublishedAmendmentError::NoLockfile]);
        assert!(
            errors[0].to_string().contains("has no assemblr-lock.json"),
            "{}",
            errors[0]
        );
    }

    #[test]
    fn a_base_is_verified_against_the_registry_record() {
        let index = index(&provenance('1', '2'));
        let manifest = amender(&["bin/run", "package.json.assemblr.tmpl"]);
        let agreeing = lock(&[("@acme/node", "1.0.0", provenance('1', '2'))]);
        assert_eq!(
            verify_bases(
                &identity("@acme/app"),
                &version("0.1.0"),
                &manifest,
                &agreeing,
                &index
            ),
            []
        );

        let differing = lock(&[("@acme/node", "1.0.0", provenance('5', '6'))]);
        let errors = verify_bases(
            &identity("@acme/app"),
            &version("0.1.0"),
            &manifest,
            &differing,
            &index,
        );
        let expected = |amended: &str| BaseIntegrityError {
            amender: identity("@acme/app"),
            version: version("0.1.0"),
            path: path(amended),
            provider: identity("@acme/node"),
            base: version("1.0.0"),
            published: provenance('5', '6'),
            recorded: provenance('1', '2'),
        };
        assert_eq!(
            errors,
            [expected("bin/run"), expected("package.json.assemblr.tmpl")]
        );
        let message = errors[0].to_string();
        for part in [
            "integrity conflict",
            "@acme/app 0.1.0 amends bin/run of @acme/node 1.0.0",
            "commit 5555",
            "records commit 1111",
        ] {
            assert!(message.contains(part), "{message}");
        }

        // A base the index does not record, or that the lockfile does not
        // lock at the base, is not an integrity conflict here.
        let other = RegistryIndex::from_units(BTreeMap::new()).unwrap();
        assert_eq!(
            verify_bases(
                &identity("@acme/app"),
                &version("0.1.0"),
                &manifest,
                &differing,
                &other
            ),
            []
        );
        let elsewhere = lock(&[("@acme/node", "1.1.0", provenance('5', '6'))]);
        assert_eq!(
            verify_bases(
                &identity("@acme/app"),
                &version("0.1.0"),
                &manifest,
                &elsewhere,
                &index
            ),
            []
        );
    }
}
