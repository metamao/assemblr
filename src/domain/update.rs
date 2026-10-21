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
//! An amendment whose provider moves is reconciled on the source files
//! ([`reconcile_amendments`]). Until the merge engine is decided, only the
//! case that needs no rule is supported: when the provider's file is the
//! same bytes at the old base and at the new version, the copy carries over
//! and its declared base follows the lock. Every other case refuses the
//! update. The update also refuses a resulting graph in which a published
//! amendment's provider does not resolve to its base
//! ([`published_amenders_at_base`]), because no build could compose it.

use std::collections::BTreeMap;
use std::fmt;

use super::composition::FileTree;
use super::identity::UnitIdentity;
use super::lock::{LockError, LockedUnit, UnitLock};
use super::manifest::Manifest;
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

/// The resolved version of each unit of `graph`, by identity.
fn versions(graph: &ResolvedGraph) -> BTreeMap<&UnitIdentity, &Version> {
    graph
        .units()
        .iter()
        .map(|(identity, unit)| (identity, unit.version()))
        .collect()
}

/// Reconciles the amendments of `manifest` with `graph`, the resolution of
/// an update, given `previous`, the lock before it, and `files`, the `src/`
/// files of each provider at its old base and at its new version.
///
/// An amendment whose provider still resolves to the declared base is
/// untouched. One whose provider moves carries over when the provider's
/// file at the amended path has the same bytes at both versions: the result
/// maps its path to the new base. Every other case is an error, sorted by
/// path: a declared base that is not the locked version, which the author
/// edited by hand; a path the provider does not have at the base or no
/// longer has at the new version; a file that differs between the two
/// versions, which needs the merge engine; and a version whose files
/// `files` does not hold.
pub fn reconcile_amendments(
    manifest: &Manifest,
    previous: &UnitLock,
    graph: &ResolvedGraph,
    files: &BTreeMap<(UnitIdentity, Version), FileTree>,
) -> Result<BTreeMap<RelativePath, Version>, Vec<ReconcileError>> {
    let resolved = versions(graph);
    let mut carried = BTreeMap::new();
    let mut errors = Vec::new();
    for (path, declaration) in manifest.amendments() {
        let unit = declaration.unit();
        let base = declaration.base();
        let Some(&new) = resolved.get(unit) else {
            continue;
        };
        if new == base {
            continue;
        }
        let moved = |cause| ReconcileError {
            path: path.clone(),
            unit: unit.clone(),
            base: base.clone(),
            resolved: new.clone(),
            cause,
        };
        if let Some(locked) = previous
            .units()
            .get(unit)
            .filter(|locked| locked.version() != base)
        {
            errors.push(moved(ReconcileCause::DeclarationChanged {
                locked: locked.version().clone(),
            }));
            continue;
        }
        let tree_at = |version: &Version| files.get(&(unit.clone(), version.clone()));
        let (Some(old_tree), Some(new_tree)) = (tree_at(base), tree_at(new)) else {
            let missing = if tree_at(base).is_none() { base } else { new };
            errors.push(moved(ReconcileCause::Unavailable(missing.clone())));
            continue;
        };
        match (old_tree.get(path), new_tree.get(path)) {
            (None, _) => errors.push(moved(ReconcileCause::NotProvidedAtBase)),
            (Some(_), None) => errors.push(moved(ReconcileCause::NoLongerProvided)),
            (Some(old_file), Some(new_file)) if old_file.contents() == new_file.contents() => {
                carried.insert(path.clone(), new.clone());
            }
            (Some(_), Some(_)) => errors.push(moved(ReconcileCause::Changed)),
        }
    }
    if errors.is_empty() {
        Ok(carried)
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
    /// The provider's files at this version were not available to the
    /// update.
    Unavailable(Version),
    /// The provider does not have the path at the declared base.
    NotProvidedAtBase,
    /// The provider no longer has the path at the new version.
    NoLongerProvided,
    /// The provider's file differs between the two versions, so carrying
    /// the amendment over needs reconciliation, which is not supported yet.
    Changed,
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
            ReconcileCause::Unavailable(version) => write!(
                f,
                "amendments/{path} is based on {unit} {base}, which the update would move to \
                 {resolved}, but the files of {unit} {version} are not available"
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
                "amendments/{path} is based on {unit} {base}, and src/{path} differs in {unit} \
                 {resolved}, which the update would select; reconciling an amendment with a \
                 changed base is not supported yet"
            ),
        }
    }
}

impl std::error::Error for ReconcileError {}

/// Checks that in `graph` every published amendment's provider resolves to
/// the declared base, where `manifests` holds the manifest of each resolved
/// unit at its resolved version. A published amendment can be composed only
/// at its base, so an update that leaves one elsewhere would leave the unit
/// unbuildable. Errors are sorted by amender and then path.
pub fn published_amenders_at_base(
    graph: &ResolvedGraph,
    manifests: &BTreeMap<(UnitIdentity, Version), Manifest>,
) -> Vec<PublishedAmenderError> {
    let resolved = versions(graph);
    let mut errors = Vec::new();
    for (amender, version) in &resolved {
        let Some(manifest) = manifests.get(&((*amender).clone(), (*version).clone())) else {
            continue;
        };
        for (path, declaration) in manifest.amendments() {
            let provider = declaration.unit();
            let Some(&selected) = resolved.get(provider) else {
                continue;
            };
            if selected != declaration.base() {
                errors.push(PublishedAmenderError {
                    amender: (*amender).clone(),
                    version: (*version).clone(),
                    path: path.clone(),
                    provider: provider.clone(),
                    base: declaration.base().clone(),
                    resolved: selected.clone(),
                });
            }
        }
    }
    errors
}

/// A published amendment whose provider the update would resolve to a
/// version other than its base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedAmenderError {
    amender: UnitIdentity,
    version: Version,
    path: RelativePath,
    provider: UnitIdentity,
    base: Version,
    resolved: Version,
}

impl PublishedAmenderError {
    /// The published unit that declares the amendment.
    pub fn amender(&self) -> &UnitIdentity {
        &self.amender
    }

    /// Its resolved version.
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

    /// The declared base.
    pub fn base(&self) -> &Version {
        &self.base
    }

    /// The version the update would select for the provider.
    pub fn resolved(&self) -> &Version {
        &self.resolved
    }
}

impl fmt::Display for PublishedAmenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} amends {} of {} at {}, but the update would select {} {}; applying a \
             published amendment to another version of its provider is not supported yet",
            self.amender,
            self.version,
            self.path,
            self.provider,
            self.base,
            self.provider,
            self.resolved
        )
    }
}

impl std::error::Error for PublishedAmenderError {}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "tests fail on a bad fixture"
    )]

    use std::collections::BTreeMap;

    use super::super::composition::UnitFile;
    use super::super::manifest::AmendmentDeclaration;
    use super::super::parameter::Description;
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

    fn path(input: &str) -> RelativePath {
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

    fn amending(base: &str) -> Manifest {
        Manifest::new(
            identity("@acme/app"),
            version("0.1.0"),
            Description::new("An app.").unwrap(),
            requirements("^1"),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::from([(
                path("config/app.yaml"),
                AmendmentDeclaration::new(identity("@acme/node"), version(base)),
            )]),
        )
        .unwrap()
    }

    fn tree(files: &[(&str, &str)]) -> FileTree {
        files
            .iter()
            .map(|(path, contents)| {
                (
                    self::path(path),
                    UnitFile::new(contents.as_bytes().to_vec(), false),
                )
            })
            .collect()
    }

    fn files(
        old: &[(&str, &str)],
        new: &[(&str, &str)],
    ) -> BTreeMap<(UnitIdentity, Version), FileTree> {
        BTreeMap::from([
            ((identity("@acme/node"), version("1.0.0")), tree(old)),
            ((identity("@acme/node"), version("1.1.0")), tree(new)),
        ])
    }

    fn reconcile(
        manifest: &Manifest,
        files: &BTreeMap<(UnitIdentity, Version), FileTree>,
    ) -> Result<BTreeMap<RelativePath, Version>, Vec<ReconcileError>> {
        let previous = lock(&[("@acme/node", "1.0.0", provenance('1', 'a'))]);
        let graph = graph("^1", &UnitLock::default());
        reconcile_amendments(manifest, &previous, &graph, files)
    }

    #[test]
    fn an_unchanged_base_file_carries_the_amendment_over() {
        let files = files(
            &[("config/app.yaml", "port: 80\n"), ("other", "1.0.0")],
            &[("config/app.yaml", "port: 80\n"), ("other", "1.1.0")],
        );

        let carried = reconcile(&amending("1.0.0"), &files).unwrap();

        assert_eq!(
            carried,
            BTreeMap::from([(path("config/app.yaml"), version("1.1.0"))])
        );
    }

    #[test]
    fn an_amendment_whose_provider_stays_is_untouched() {
        let previous = lock(&[("@acme/node", "1.1.0", provenance('2', 'b'))]);
        let graph = graph("^1", &previous);

        let carried =
            reconcile_amendments(&amending("1.1.0"), &previous, &graph, &BTreeMap::new()).unwrap();

        assert!(carried.is_empty());
    }

    #[test]
    fn each_cause_that_refuses_a_carry_over_is_told_apart() {
        let cause = |manifest: &Manifest, files| {
            let errors = reconcile(manifest, &files).unwrap_err();
            assert_eq!(errors.len(), 1, "{errors:?}");
            let error = errors.into_iter().next().unwrap();
            assert_eq!(error.path(), &path("config/app.yaml"));
            assert_eq!(error.unit(), &identity("@acme/node"));
            assert_eq!(error.resolved(), &version("1.1.0"));
            error.cause().clone()
        };
        let same = [("config/app.yaml", "port: 80\n")];

        assert_eq!(
            cause(
                &amending("1.0.0"),
                files(&same, &[("config/app.yaml", "port: 8080\n")])
            ),
            ReconcileCause::Changed
        );
        assert_eq!(
            cause(&amending("1.0.0"), files(&same, &[])),
            ReconcileCause::NoLongerProvided
        );
        assert_eq!(
            cause(&amending("1.0.0"), files(&[], &same)),
            ReconcileCause::NotProvidedAtBase
        );
        assert_eq!(
            cause(&amending("1.0.0"), BTreeMap::new()),
            ReconcileCause::Unavailable(version("1.0.0"))
        );
        assert_eq!(
            cause(&amending("2.0.0"), files(&same, &same)),
            ReconcileCause::DeclarationChanged {
                locked: version("1.0.0")
            }
        );
    }

    fn amender_manifest(base: &str) -> Manifest {
        Manifest::new(
            identity("@acme/amender"),
            version("0.1.0"),
            Description::new("An amender.").unwrap(),
            requirements("^1"),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::from([(
                path("config/app.yaml"),
                AmendmentDeclaration::new(identity("@acme/node"), version(base)),
            )]),
        )
        .unwrap()
    }

    /// The amender's published manifests: 0.1.0 amends `@acme/node` at
    /// 1.0.0, and 0.2.0 at 1.1.0.
    fn amender_manifests() -> BTreeMap<(UnitIdentity, Version), Manifest> {
        BTreeMap::from([
            (
                (identity("@acme/amender"), version("0.1.0")),
                amender_manifest("1.0.0"),
            ),
            (
                (identity("@acme/amender"), version("0.2.0")),
                amender_manifest("1.1.0"),
            ),
        ])
    }

    #[test]
    fn a_published_amender_is_checked_against_the_new_graph() {
        let requirements = Dependencies::from([
            (identity("@acme/node"), "^1".parse().unwrap()),
            (identity("@acme/amender"), "^0".parse().unwrap()),
        ]);
        let manifests = amender_manifests();

        // Updating both: the amender's 0.2.0 targets node 1.1.0.
        let graph = graph_of(&requirements, &UnitLock::default());
        assert!(published_amenders_at_base(&graph, &manifests).is_empty());

        // Updating node alone leaves the amender at 0.1.0, whose base is
        // 1.0.0.
        let kept = lock(&[("@acme/amender", "0.1.0", provenance('5', 'e'))]);
        let graph = graph_of(&requirements, &kept);
        let errors = published_amenders_at_base(&graph, &manifests);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].amender(), &identity("@acme/amender"));
        assert_eq!(errors[0].version(), &version("0.1.0"));
        assert_eq!(errors[0].path(), &path("config/app.yaml"));
        assert_eq!(errors[0].provider(), &identity("@acme/node"));
        assert_eq!(errors[0].base(), &version("1.0.0"));
        assert_eq!(errors[0].resolved(), &version("1.1.0"));
    }
}
