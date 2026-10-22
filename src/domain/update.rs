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
//! ([`reconcile_amendments`]). When the provider's file is the same bytes at
//! the old base and at the new version, the copy carries over untouched and
//! its declared base follows the lock. When it differs, a text or JSON file
//! is rebased: the amender's changes against the old base are merged by
//! line into the file at the new version, on the source templates, so that
//! the copy keeps its template expressions. Insertions the provider and the
//! amendment make at one position are both kept, the provider's first, as
//! composition does when it applies an amendment across versions. A
//! collision refuses the update and says where. Other formats, and files that are not text, are
//! carried over only when the provider's file is unchanged. A published
//! amendment whose provider resolves to another version than its base is
//! not a problem: composition applies it to the version resolved.

use std::collections::BTreeMap;
use std::fmt;

use super::composition::{FileTree, UnitFile};
use super::identity::UnitIdentity;
use super::lock::{LockError, LockedUnit, UnitLock};
use super::manifest::Manifest;
use super::merge::MergeFormat;
use super::path::RelativePath;
use super::registry::RegistryIndex;
use super::resolution::{LockChange, ResolvedGraph};
use super::text_merge::{InsertTie, describe_change, describe_lines, is_text, merge_text};
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
/// an update, given `previous`, the lock before it, `files`, the `src/`
/// files of each provider at its old base and at its new version, and
/// `copies`, the unit's own copies under `amendments/`.
///
/// An amendment whose provider still resolves to the declared base is
/// untouched. One whose provider moves is carried to the new version, its
/// declared base following the lock: unchanged when the provider's file has
/// the same bytes at both versions, and otherwise rebased, when the file is
/// text outside the structured formats without a merge. The rebase merges
/// the changes the copy makes against the old base into the new version's
/// file, line by line, neither side taking precedence; the result is the
/// new copy. Every other case is an error, sorted by path: a declared base
/// that is not the locked version, which the author edited by hand; a path
/// the provider does not have at the base or no longer has at the new
/// version; a file that differs and cannot be merged; lines the copy and the
/// provider both changed; and a version whose files `files` does not hold.
pub fn reconcile_amendments(
    manifest: &Manifest,
    previous: &UnitLock,
    graph: &ResolvedGraph,
    files: &BTreeMap<(UnitIdentity, Version), FileTree>,
    copies: &BTreeMap<RelativePath, UnitFile>,
) -> Result<BTreeMap<RelativePath, Reconciled>, Vec<ReconcileError>> {
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
        let (old_file, new_file) = match (old_tree.get(path), new_tree.get(path)) {
            (None, _) => {
                errors.push(moved(ReconcileCause::NotProvidedAtBase));
                continue;
            }
            (Some(_), None) => {
                errors.push(moved(ReconcileCause::NoLongerProvided));
                continue;
            }
            (Some(old_file), Some(new_file)) => (old_file, new_file),
        };
        if old_file.contents() == new_file.contents() {
            carried.insert(
                path.clone(),
                Reconciled {
                    base: new.clone(),
                    copy: None,
                },
            );
            continue;
        }
        // The file changed: rebase the copy on the source text.
        let delivered = super::composition::delivered_path(path).ok().flatten();
        let format = MergeFormat::of_path(delivered.as_ref().unwrap_or(path));
        let copy = copies.get(path);
        let (Some(copy), MergeFormat::Json | MergeFormat::Text) = (copy, format) else {
            errors.push(moved(ReconcileCause::Changed));
            continue;
        };
        let (Ok(old_text), Ok(new_text), Ok(copy_text)) = (
            std::str::from_utf8(old_file.contents()),
            std::str::from_utf8(new_file.contents()),
            std::str::from_utf8(copy.contents()),
        ) else {
            errors.push(moved(ReconcileCause::Changed));
            continue;
        };
        if ![old_file.contents(), new_file.contents(), copy.contents()]
            .into_iter()
            .all(is_text)
        {
            errors.push(moved(ReconcileCause::Changed));
            continue;
        }
        let merge = merge_text(
            old_text,
            &[(Side::Copy, copy_text), (Side::Provider, new_text)],
            InsertTie::Chain,
        );
        match merge.text() {
            Some(text) => {
                carried.insert(
                    path.clone(),
                    Reconciled {
                        base: new.clone(),
                        copy: Some(text.into_bytes()),
                    },
                );
            }
            None => errors.push(moved(ReconcileCause::Collides(
                merge
                    .conflicts()
                    .map(|conflict| {
                        let side = |wanted: Side| {
                            conflict
                                .sides
                                .iter()
                                .find(|(side, _)| *side == wanted)
                                .map(|(_, lines)| describe_change(&conflict.base, lines))
                                .unwrap_or_default()
                        };
                        format!(
                            "{}: the provider {}; the amendment {}",
                            describe_lines(&conflict.base_lines),
                            side(Side::Provider),
                            side(Side::Copy)
                        )
                    })
                    .collect(),
            ))),
        }
    }
    if errors.is_empty() {
        Ok(carried)
    } else {
        Err(errors)
    }
}

/// The two sides of a rebase. The provider orders first, so that
/// insertions both make at one position keep the provider's lines before
/// the amendment's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Side {
    Provider,
    Copy,
}

/// An amendment carried to its provider's new version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reconciled {
    /// The new declared base: the version the update selects.
    pub base: Version,
    /// The rebased copy, when the provider's file changed and the copy had
    /// to follow it; `None` when the copy is untouched.
    pub copy: Option<Vec<u8>>,
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
    /// The provider's file differs between the two versions, and its format
    /// has no merge, or it or the copy is not text.
    Changed,
    /// The copy and the provider changed the same lines of the file between
    /// the two versions, described one collision each.
    Collides(Vec<String>),
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
                 {resolved}, which the update would select; the file is YAML, TOML, XML, or \
                 INI, or it or the copy is not UTF-8 text, so the copy follows it only when it \
                 is unchanged"
            ),
            ReconcileCause::Collides(places) => {
                write!(
                    f,
                    "amendments/{path} is based on {unit} {base}, and {unit} {resolved}, which \
                     the update would select, changes lines of src/{path} that the amendment \
                     changes too (lines of {base}):"
                )?;
                for place in places {
                    write!(f, "\n    {place}")?;
                }
                Ok(())
            }
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
    ) -> Result<BTreeMap<RelativePath, Reconciled>, Vec<ReconcileError>> {
        reconcile_with(manifest, files, &BTreeMap::new())
    }

    fn reconcile_with(
        manifest: &Manifest,
        files: &BTreeMap<(UnitIdentity, Version), FileTree>,
        copies: &BTreeMap<RelativePath, UnitFile>,
    ) -> Result<BTreeMap<RelativePath, Reconciled>, Vec<ReconcileError>> {
        let previous = lock(&[("@acme/node", "1.0.0", provenance('1', 'a'))]);
        let graph = graph("^1", &UnitLock::default());
        reconcile_amendments(manifest, &previous, &graph, files, copies)
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
            BTreeMap::from([(
                path("config/app.yaml"),
                Reconciled {
                    base: version("1.1.0"),
                    copy: None
                }
            )])
        );
    }

    #[test]
    fn an_amendment_whose_provider_stays_is_untouched() {
        let previous = lock(&[("@acme/node", "1.1.0", provenance('2', 'b'))]);
        let graph = graph("^1", &previous);

        let carried = reconcile_amendments(
            &amending("1.1.0"),
            &previous,
            &graph,
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
        .unwrap();

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

    fn amending_path(file: &str, base: &str) -> Manifest {
        Manifest::new(
            identity("@acme/app"),
            version("0.1.0"),
            Description::new("An app.").unwrap(),
            requirements("^1"),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::from([(
                path(file),
                AmendmentDeclaration::new(identity("@acme/node"), version(base)),
            )]),
        )
        .unwrap()
    }

    fn copy_of(file: &str, text: &str) -> BTreeMap<RelativePath, UnitFile> {
        BTreeMap::from([(path(file), UnitFile::new(text.as_bytes().to_vec(), false))])
    }

    const NOTES_OLD: &str = "# Notes\n\nOne.\n\nTwo.\n\nThree.\n";

    #[test]
    fn a_changed_text_file_is_rebased_onto_the_new_version() {
        // The provider rewords the title; the copy adds a section at the end.
        let new = "# Team notes\n\nOne.\n\nTwo.\n\nThree.\n";
        let copy = format!("{NOTES_OLD}\n## Mine\n\nExtra.\n");
        let files = files(&[("NOTES.md", NOTES_OLD)], &[("NOTES.md", new)]);

        let rebased = reconcile_with(
            &amending_path("NOTES.md", "1.0.0"),
            &files,
            &copy_of("NOTES.md", &copy),
        )
        .unwrap();

        let reconciled = &rebased[&path("NOTES.md")];
        assert_eq!(reconciled.base, version("1.1.0"));
        assert_eq!(
            String::from_utf8(reconciled.copy.clone().unwrap()).unwrap(),
            "# Team notes\n\nOne.\n\nTwo.\n\nThree.\n\n## Mine\n\nExtra.\n"
        );
    }

    #[test]
    fn insertions_the_provider_and_the_copy_make_at_one_place_are_both_kept_provider_first() {
        // Both add a section at the end of the file: the provider in its new
        // version, the copy against the old one.
        let new = format!("{NOTES_OLD}\n## Provider\n\nTheirs.\n");
        let copy = format!("{NOTES_OLD}\n## Mine\n\nExtra.\n");
        let files = files(&[("NOTES.md", NOTES_OLD)], &[("NOTES.md", &new)]);

        let rebased = reconcile_with(
            &amending_path("NOTES.md", "1.0.0"),
            &files,
            &copy_of("NOTES.md", &copy),
        )
        .unwrap();

        assert_eq!(
            String::from_utf8(rebased[&path("NOTES.md")].copy.clone().unwrap()).unwrap(),
            format!("{NOTES_OLD}\n## Provider\n\nTheirs.\n\n## Mine\n\nExtra.\n")
        );
    }

    #[test]
    fn lines_both_the_copy_and_the_provider_changed_are_a_collision_that_names_them() {
        let new = NOTES_OLD.replace("Two.", "Two, provider.");
        let copy = NOTES_OLD.replace("Two.", "Two, mine.");
        let files = files(&[("NOTES.md", NOTES_OLD)], &[("NOTES.md", &new)]);

        let errors = reconcile_with(
            &amending_path("NOTES.md", "1.0.0"),
            &files,
            &copy_of("NOTES.md", &copy),
        )
        .unwrap_err();

        assert_eq!(errors.len(), 1);
        let ReconcileCause::Collides(places) = errors[0].cause() else {
            panic!("{:?}", errors[0]);
        };
        assert_eq!(
            places,
            &["line 5: the provider sets \"Two, provider.\"; the amendment sets \"Two, mine.\""]
        );
        let message = errors[0].to_string();
        assert!(
            message.contains("changes lines of src/NOTES.md that the amendment changes too"),
            "{message}"
        );
    }

    #[test]
    fn a_changed_file_without_a_merge_or_that_is_not_text_is_not_rebased() {
        let new = NOTES_OLD.replace("Two.", "Two, provider.");
        // YAML has no merge.
        let files_yaml = files(
            &[("config/app.yaml", "a: 1\n")],
            &[("config/app.yaml", "a: 2\n")],
        );
        let errors = reconcile_with(
            &amending("1.0.0"),
            &files_yaml,
            &copy_of("config/app.yaml", "a: 1\nb: 2\n"),
        )
        .unwrap_err();
        assert_eq!(errors[0].cause(), &ReconcileCause::Changed);
        // A copy that is not text.
        let files_text = files(&[("NOTES.md", NOTES_OLD)], &[("NOTES.md", &new)]);
        let mut binary = copy_of("NOTES.md", NOTES_OLD);
        binary.insert(path("NOTES.md"), UnitFile::new(b"a\0b".to_vec(), false));
        let errors =
            reconcile_with(&amending_path("NOTES.md", "1.0.0"), &files_text, &binary).unwrap_err();
        assert_eq!(errors[0].cause(), &ReconcileCause::Changed);
    }
}
