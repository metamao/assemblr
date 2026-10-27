//! Reading catalogs: repositories with `catalog.jsonc` at their root and one
//! unit in each directory of `units/`, published together at the catalog's
//! version (see `docs/domain/units.md`).
//!
//! A commit is a catalog when its root has `catalog.jsonc`. There, a unit's
//! manifest, files, and tree are those of its directory, `units/<name>`, and
//! its manifest is read by the catalog's rules (see
//! [`read_catalog_manifest`]). The lockfile stays at the root, one for the
//! whole catalog.

use std::collections::{BTreeMap, BTreeSet};

use super::command::Objects;
use super::object::{EntryKind, TreeEntry};
use super::{
    GitError, GitRepository, InvalidVersion, MANIFEST, MANIFEST_LIMIT, PublishedVersion,
    ReadSrcError,
};
use crate::adapters::manifest::{InCatalog, read_catalog, read_catalog_manifest};
use crate::domain::{Catalog, ObjectId, Provenance, UnitIdentity, Version};

/// A sibling of a unit in a catalog: its identity, the catalog's version,
/// and the commit with the sibling's directory's tree.
pub(crate) type Sibling = (UnitIdentity, Version, Provenance);

/// A catalog's file at the root of the repository.
pub(crate) const CATALOG: &str = "catalog.jsonc";
/// The directory that holds a catalog's units, one in each directory.
pub(crate) const UNITS: &str = "units";

/// What a version tag publishes: one unit, or every unit of a catalog.
#[derive(Debug, Clone, PartialEq)]
pub enum TagContent {
    /// The repository's one unit.
    Unit(PublishedVersion),
    /// A catalog's units.
    Catalog(PublishedCatalog),
}

impl TagContent {
    /// The units published, in the order of their directories for a
    /// catalog.
    pub fn units(&self) -> &[PublishedVersion] {
        match self {
            Self::Unit(unit) => std::slice::from_ref(unit),
            Self::Catalog(catalog) => &catalog.units,
        }
    }

    /// The catalog, when the tag publishes one.
    pub fn catalog(&self) -> Option<&Catalog> {
        match self {
            Self::Unit(_) => None,
            Self::Catalog(catalog) => Some(&catalog.catalog),
        }
    }
}

/// A catalog at a commit, with every unit in it valid.
#[derive(Debug, Clone, PartialEq)]
pub struct PublishedCatalog {
    catalog: Catalog,
    units: Vec<PublishedVersion>,
}

impl PublishedCatalog {
    /// The catalog's `catalog.jsonc`.
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Its units, in the order of their directories, each with the
    /// commit's provenance and its directory's tree.
    pub fn units(&self) -> &[PublishedVersion] {
        &self.units
    }
}

/// The directory a unit is read from at a commit: its tree and entries, and
/// the catalog when the commit is one, with the names of its unit
/// directories.
pub(super) struct UnitRoot {
    pub(super) tree: ObjectId,
    pub(super) entries: Vec<TreeEntry>,
    pub(super) catalog: Option<(Catalog, BTreeSet<String>)>,
}

/// A catalog read from a root listing: `catalog.jsonc`, and the entries of
/// `units/` by name, each a directory.
struct CatalogAt {
    catalog: Catalog,
    units: BTreeMap<String, ObjectId>,
}

impl CatalogAt {
    fn directories(&self) -> BTreeSet<String> {
        self.units.keys().cloned().collect()
    }
}

impl GitRepository {
    /// The directory the unit is read from at `commit`: the root, or, when
    /// the commit is a catalog, the unit's directory, which must exist.
    pub(super) fn unit_root(
        &self,
        objects: &mut Objects<'_>,
        commit: &ObjectId,
    ) -> Result<Result<UnitRoot, InvalidVersion>, GitError> {
        let tree = match self.commit_tree(objects, commit)? {
            Ok(tree) => tree,
            Err(too_large) => return Ok(Err(InvalidVersion::ObjectTooLarge(too_large))),
        };
        let root = match self.read_tree(objects, &tree)? {
            Ok(root) => root,
            Err(too_large) => return Ok(Err(InvalidVersion::ObjectTooLarge(too_large))),
        };
        let at = match self.catalog_at(objects, &root)? {
            None => {
                return Ok(Ok(UnitRoot {
                    tree,
                    entries: root,
                    catalog: None,
                }));
            }
            Some(Err(invalid)) => return Ok(Err(invalid)),
            Some(Ok(at)) => at,
        };
        let directory = self.unit.clone().unwrap_or_default();
        let Some(id) = at.units.get(&directory) else {
            return Ok(Err(InvalidVersion::NotInCatalog { directory }));
        };
        let entries = match self.read_tree(objects, id)? {
            Ok(entries) => entries,
            Err(too_large) => return Ok(Err(InvalidVersion::ObjectTooLarge(too_large))),
        };
        let directories = at.directories();
        Ok(Ok(UnitRoot {
            tree: id.clone(),
            entries,
            catalog: Some((at.catalog, directories)),
        }))
    }

    /// The entries of the unit's directory at `commit`, as [`unit_root`]
    /// finds it, for reading its files.
    ///
    /// [`unit_root`]: Self::unit_root
    pub(super) fn unit_entries(
        &self,
        objects: &mut Objects<'_>,
        commit: &ObjectId,
    ) -> Result<Vec<TreeEntry>, ReadSrcError> {
        // The commit's kind is checked as for the root's entries.
        self.root_entries(objects, commit)?;
        match self.unit_root(objects, commit)? {
            Ok(unit) => Ok(unit.entries),
            Err(InvalidVersion::ObjectTooLarge(too_large)) => {
                Err(ReadSrcError::ObjectTooLarge(too_large))
            }
            Err(invalid) => Err(ReadSrcError::Unit(invalid)),
        }
    }

    /// Reads the catalog at `commit` and every unit in it, or `None` when
    /// the commit is not a catalog. A unit that is not valid makes the
    /// whole catalog invalid: its units are published together.
    pub(super) fn read_catalog_version(
        &self,
        objects: &mut Objects<'_>,
        commit: &ObjectId,
    ) -> Result<Option<Result<PublishedCatalog, InvalidVersion>>, GitError> {
        let tree = match self.commit_tree(objects, commit)? {
            Ok(tree) => tree,
            Err(too_large) => return Ok(Some(Err(InvalidVersion::ObjectTooLarge(too_large)))),
        };
        let root = match self.read_tree(objects, &tree)? {
            Ok(root) => root,
            Err(too_large) => return Ok(Some(Err(InvalidVersion::ObjectTooLarge(too_large)))),
        };
        let at = match self.catalog_at(objects, &root)? {
            None => return Ok(None),
            Some(Err(invalid)) => return Ok(Some(Err(invalid))),
            Some(Ok(at)) => at,
        };
        let directories = at.directories();
        let mut units = Vec::new();
        for (directory, id) in &at.units {
            let invalid = |problem| InvalidVersion::InvalidUnit {
                directory: directory.clone(),
                problem: Box::new(problem),
            };
            let entries = match self.read_tree(objects, id)? {
                Ok(entries) => entries,
                Err(too_large) => {
                    return Ok(Some(Err(invalid(InvalidVersion::ObjectTooLarge(
                        too_large,
                    )))));
                }
            };
            let in_catalog = InCatalog::new(&at.catalog, directory, &directories);
            let manifest = match self.manifest_in(objects, entries, |text| {
                read_catalog_manifest(text, &in_catalog)
            })? {
                Ok(manifest) => manifest,
                Err(problem) => return Ok(Some(Err(invalid(problem)))),
            };
            units.push(PublishedVersion {
                manifest,
                provenance: Provenance::new(commit.clone(), id.clone()),
            });
        }
        Ok(Some(Ok(PublishedCatalog {
            catalog: at.catalog,
            units,
        })))
    }

    /// Reads the catalog committed at `commit` and every unit in it, as a
    /// tag of the commit would publish them, or `None` when the commit is
    /// not a catalog.
    pub(crate) fn read_catalog_at(
        &self,
        commit: &ObjectId,
    ) -> Result<Option<Result<PublishedCatalog, InvalidVersion>>, GitError> {
        let mut objects = self.objects()?;
        let read = self.read_catalog_version(&mut objects, commit)?;
        objects.finish()?;
        Ok(read)
    }

    /// The other units of the catalog at `commit`, each at the catalog's
    /// version with the commit and its directory's tree, or `None` when
    /// the commit is not a valid catalog. Only trees and `catalog.jsonc`
    /// are read. A catalog's lockfile locks what its units depend on in
    /// other repositories; their siblings are these.
    pub(crate) fn siblings_at(&self, commit: &ObjectId) -> Result<Option<Vec<Sibling>>, GitError> {
        let mut objects = self.objects()?;
        let Ok(tree) = self.commit_tree(&mut objects, commit)? else {
            return Ok(None);
        };
        let Ok(root) = self.read_tree(&mut objects, &tree)? else {
            return Ok(None);
        };
        let Some(Ok(at)) = self.catalog_at(&mut objects, &root)? else {
            return Ok(None);
        };
        objects.finish()?;
        let mut siblings = Vec::new();
        for (directory, id) in &at.units {
            if self.unit.as_deref() == Some(directory.as_str()) {
                continue;
            }
            let Ok(identity) = at.catalog.unit_identity(directory) else {
                return Ok(None);
            };
            siblings.push((
                identity,
                at.catalog.version().clone(),
                Provenance::new(commit.clone(), id.clone()),
            ));
        }
        Ok(Some(siblings))
    }

    /// The catalog a root listing describes, or `None` when it has no
    /// `catalog.jsonc`.
    fn catalog_at(
        &self,
        objects: &mut Objects<'_>,
        root: &[TreeEntry],
    ) -> Result<Option<Result<CatalogAt, InvalidVersion>>, GitError> {
        let Some(entry) = root.iter().find(|entry| entry.name == CATALOG.as_bytes()) else {
            return Ok(None);
        };
        if root.iter().any(|entry| entry.name == MANIFEST) {
            return Ok(Some(Err(InvalidVersion::CatalogAndManifest)));
        }
        if !matches!(entry.kind, EntryKind::File { .. }) {
            return Ok(Some(Err(InvalidVersion::CatalogNotAFile)));
        }
        let blob = self.require(objects, entry.id.as_str(), MANIFEST_LIMIT)?;
        let Some(bytes) = blob.contents else {
            return Ok(Some(Err(InvalidVersion::CatalogTooLarge {
                size: blob.header.size,
                limit: MANIFEST_LIMIT,
            })));
        };
        let Ok(text) = String::from_utf8(bytes) else {
            return Ok(Some(Err(InvalidVersion::CatalogNotUtf8)));
        };
        let catalog = match read_catalog(&text) {
            Ok(catalog) => catalog,
            Err(error) => return Ok(Some(Err(InvalidVersion::InvalidCatalog(error)))),
        };
        let Some(units) = root.iter().find(|entry| entry.name == UNITS.as_bytes()) else {
            return Ok(Some(Err(InvalidVersion::NoUnits)));
        };
        if units.kind != EntryKind::Directory {
            return Ok(Some(Err(InvalidVersion::UnitsNotADirectory)));
        }
        let listing = match self.read_tree(objects, &units.id)? {
            Ok(listing) => listing,
            Err(too_large) => return Ok(Some(Err(InvalidVersion::ObjectTooLarge(too_large)))),
        };
        let mut directories = BTreeMap::new();
        for entry in listing {
            let name = String::from_utf8_lossy(&entry.name).into_owned();
            if entry.kind != EntryKind::Directory || std::str::from_utf8(&entry.name).is_err() {
                return Ok(Some(Err(InvalidVersion::NotAUnitDirectory { name })));
            }
            directories.insert(name, entry.id);
        }
        if directories.is_empty() {
            return Ok(Some(Err(InvalidVersion::NoUnits)));
        }
        Ok(Some(Ok(CatalogAt {
            catalog,
            units: directories,
        })))
    }

    /// The blobs registration reads in the catalog a root listing
    /// describes: `catalog.jsonc` and each unit's `manifest.jsonc`. Nothing
    /// when the root is not a catalog's. Entries that cannot be read are
    /// skipped: reading the version reports them.
    pub(super) fn catalog_metadata(
        &self,
        objects: &mut Objects<'_>,
        root: &[TreeEntry],
    ) -> Result<Vec<ObjectId>, GitError> {
        let mut blobs = Vec::new();
        let Some(catalog) = root.iter().find(|entry| entry.name == CATALOG.as_bytes()) else {
            return Ok(blobs);
        };
        if matches!(catalog.kind, EntryKind::File { .. }) {
            blobs.push(catalog.id.clone());
        }
        let Some(units) = root
            .iter()
            .find(|entry| entry.name == UNITS.as_bytes() && entry.kind == EntryKind::Directory)
        else {
            return Ok(blobs);
        };
        let Ok(listing) = self.read_tree(objects, &units.id)? else {
            return Ok(blobs);
        };
        for unit in listing
            .iter()
            .filter(|entry| entry.kind == EntryKind::Directory)
        {
            let Ok(entries) = self.read_tree(objects, &unit.id)? else {
                continue;
            };
            blobs.extend(
                entries
                    .into_iter()
                    .filter(|entry| {
                        entry.name == MANIFEST && matches!(entry.kind, EntryKind::File { .. })
                    })
                    .map(|entry| entry.id),
            );
        }
        Ok(blobs)
    }
}
