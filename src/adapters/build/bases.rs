//! The stored bases of a catalog's amendments (see
//! `docs/domain/amendments-and-composition.md` and
//! `docs/adr/0040-stored-amendment-bases.md`).
//!
//! The units of a catalog change together, so an amendment of a sibling's
//! file has no earlier published version to take its base from. The base
//! of every amendment of a unit in a catalog is therefore stored in the
//! catalog, under `.assemblr/` at its top level, in the content-addressed
//! format of a consumer project's store: each base's bytes once under
//! `objects/`, and one listing for the whole catalog under `bases/`, whose
//! paths are `<unit directory>/<amended path>`. `amendment-bases.json`
//! names the listing by its identity, which every read verifies, with
//! every blob it names (**Provisional (D-58)**).

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::amend::composed_bases;
use super::catalog::in_unit;
use super::{BuildProblem, Siblings, resolve_graph_in};
use crate::adapters::layered::{has_markers, rebase_json, rebase_text_on_stored};
use crate::adapters::project::{STATE_DIR, Store, StoreError, write_new};
use crate::domain::{
    BaseIdentity, MergeFormat, RegistryIndex, RelativePath, Snapshot, SnapshotFile, UnitIdentity,
    UnitLock, is_text,
};

/// The record that names the listing, in the state directory.
const RECORD: &str = "amendment-bases.json";
/// Where the record is written before it is put in place.
const RECORD_NEW: &str = "amendment-bases.json.new";
/// The record's format. Every change to its members takes a new number,
/// and a reader refuses a number or a member it does not know (ADR 0027).
const RECORD_FORMAT: u64 = 1;
const RECORD_MEMBERS: [&str; 2] = ["format", "base"];
/// The largest record read, in bytes.
const RECORD_LIMIT: u64 = 64 * 1024;

/// The stored bases of a catalog's amendments, by
/// `<unit directory>/<amended path>`.
#[derive(Debug, Clone, Default)]
pub(super) struct StoredBases {
    /// The identity of the listing, when the catalog has one.
    identity: Option<BaseIdentity>,
    bases: BTreeMap<RelativePath, Vec<u8>>,
}

impl StoredBases {
    /// Reads and verifies the stored bases of the catalog whose top level
    /// is `root`: none when it has no record.
    pub(super) fn read(root: &Path) -> Result<Self, BaseStoreError> {
        let path = record_path(root);
        let bytes = match read_record(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(BaseStoreError::Io { path, error }),
        };
        let identity = parse_record(&bytes).map_err(|detail| BaseStoreError::Record {
            path: path.clone(),
            detail,
        })?;
        let store = Store::new(root);
        let snapshot = store
            .verify_base(&identity, &Default::default())
            .map_err(BaseStoreError::Store)?;
        let mut bases = BTreeMap::new();
        for (key, file) in snapshot.files() {
            let bytes = store
                .read_blob(file.blob())
                .map_err(BaseStoreError::Store)?;
            bases.insert(key.clone(), bytes);
        }
        Ok(Self {
            identity: Some(identity),
            bases,
        })
    }

    /// The stored base of the amendment of `path` by the unit in
    /// `units/<directory>`.
    pub(super) fn get(&self, directory: &str, path: &RelativePath) -> Option<&[u8]> {
        self.bases.get(&key(directory, path)?).map(Vec::as_slice)
    }

    /// Every stored base, by key.
    pub(super) fn all(&self) -> &BTreeMap<RelativePath, Vec<u8>> {
        &self.bases
    }

    /// Stores `bases` in place of these, in the catalog whose top level is
    /// `root`: their blobs and listing, then the record, through a new file
    /// and a rename, then the old listing is removed with the blobs only it
    /// names. Writing the same bases again writes nothing. A failure before
    /// the record is replaced leaves these bases in effect, with unused
    /// blobs at most.
    pub(super) fn replace(
        &self,
        root: &Path,
        bases: &BTreeMap<RelativePath, Vec<u8>>,
    ) -> Result<(), BaseStoreError> {
        let snapshot = Snapshot::new(
            bases
                .iter()
                .map(|(key, bytes)| (key.clone(), SnapshotFile::new(bytes, false)))
                .collect(),
        );
        let identity = snapshot.identity();
        if self.identity.as_ref() == Some(&identity) {
            return Ok(());
        }
        let store = Store::new(root);
        let path = record_path(root);
        // No base left: the record goes, and the listing with it.
        if bases.is_empty() {
            if let Some(previous) = &self.identity {
                fs::remove_file(&path).map_err(|error| BaseStoreError::Io {
                    path: path.clone(),
                    error,
                })?;
                store.discard_base(previous, None);
            }
            return Ok(());
        }
        let files: Vec<(&RelativePath, &[u8])> = bases
            .iter()
            .map(|(key, bytes)| (key, bytes.as_slice()))
            .collect();
        store
            .write_base(&snapshot, &files)
            .map_err(BaseStoreError::Store)?;
        let staged = root.join(STATE_DIR).join(RECORD_NEW);
        let record = serde_json::to_vec_pretty(&json!({
            "format": RECORD_FORMAT,
            "base": identity.as_str(),
        }))
        .map_err(|error| BaseStoreError::Io {
            path: staged.clone(),
            error: error.into(),
        })?;
        let _ = fs::remove_file(&staged);
        write_new(&staged, &[record.as_slice(), b"\n"].concat())
            .and_then(|()| fs::rename(&staged, &path))
            .map_err(|error| {
                let _ = fs::remove_file(&staged);
                BaseStoreError::Io {
                    path: path.clone(),
                    error,
                }
            })?;
        if let Some(previous) = &self.identity {
            store.discard_base(previous, Some(&identity));
        }
        Ok(())
    }
}

/// What the stored bases of a catalog's amendments say about the copies:
/// the copies to rebase, and the bases to store.
#[derive(Debug, Default)]
pub(super) struct Drift {
    /// Each copy whose base changed, rebased onto the base now, in the
    /// order of the units, each after the units it depends on.
    pub(super) copies: Vec<RebasedCopy>,
    /// The bases to store: the stored ones, with each rebased copy's new
    /// base, without the bases of amendments the catalog no longer
    /// declares.
    pub(super) bases: BTreeMap<RelativePath, Vec<u8>>,
}

impl Drift {
    /// Whether a rebase left conflicts in a copy.
    pub(super) fn conflicts(&self) -> bool {
        self.copies.iter().any(|copy| !copy.conflicts.is_empty())
    }
}

/// A copy under a unit's `amendments/` rebased onto its base now.
#[derive(Debug, Clone)]
pub struct RebasedCopy {
    pub(super) unit: UnitIdentity,
    pub(super) path: RelativePath,
    pub(super) provider: UnitIdentity,
    /// The copy before the rebase.
    pub(super) previous: Vec<u8>,
    /// The copy rebased, each conflict between markers.
    pub(super) copy: Vec<u8>,
    /// Each conflict left marked, as the merge reports it.
    pub(super) conflicts: Vec<String>,
}

impl RebasedCopy {
    /// The unit whose copy it is.
    pub fn unit(&self) -> &UnitIdentity {
        &self.unit
    }

    /// The copy's path under the unit's `amendments/`.
    pub fn path(&self) -> &RelativePath {
        &self.path
    }

    /// The unit that provides the amended file.
    pub fn provider(&self) -> &UnitIdentity {
        &self.provider
    }

    /// Each conflict left between markers in the copy.
    pub fn conflicts(&self) -> &[String] {
        &self.conflicts
    }
}

/// Compares the stored base of every amendment of the units of `scope`
/// with its base now: the file as the units below the amender compose it,
/// under `lock`, with `siblings` as the catalog's units. With `rebase`, a
/// copy whose base changed is rebased onto the base now, and `siblings`
/// holds the rebased copy afterwards, so that the units above compose it;
/// without, the change is a problem, a stale base. A missing base is a
/// problem either way. The units are taken each after the units of the
/// catalog it depends on.
pub(super) fn drift(
    scope: &std::collections::BTreeSet<UnitIdentity>,
    siblings: &mut Siblings,
    lock: &UnitLock,
    index: &RegistryIndex,
    stored: &StoredBases,
    rebase: bool,
    problems: &mut Vec<BuildProblem>,
) -> Drift {
    let mut found = Drift {
        copies: Vec::new(),
        bases: stored.all().clone(),
    };
    // The bases of amendments no unit of the catalog declares any more
    // go: an interrupted `unit amend`, or an amendment removed since.
    found.bases.retain(|key, _| {
        split(key).is_some_and(|(directory, path)| {
            siblings.units.iter().any(|(identity, sibling)| {
                identity.name() == directory && sibling.manifest.amendments().contains_key(&path)
            })
        })
    });
    // A file whose rebase conflicts is not rebased in the units above,
    // whose base holds the marked copy until the author resolves it.
    let mut conflicted: std::collections::BTreeSet<(UnitIdentity, RelativePath)> =
        std::collections::BTreeSet::new();
    for identity in in_dependency_order(scope, siblings) {
        let Some(sibling) = siblings.units.get(&identity) else {
            continue;
        };
        let manifest = &sibling.manifest;
        if manifest.amendments().is_empty() {
            continue;
        }
        let directory = identity.name().to_owned();
        let mut unit_problems = Vec::new();
        let current = resolve_graph_in(
            Some(&identity),
            manifest.dependencies(),
            lock,
            index,
            siblings,
            &mut unit_problems,
        )
        .and_then(|resolved| {
            composed_bases(
                manifest,
                &sibling.content.files,
                manifest.amendments().keys(),
                &resolved,
                &mut unit_problems,
            )
        });
        problems.extend(
            unit_problems
                .into_iter()
                .map(|problem| in_unit(&directory, problem)),
        );
        let Some(current) = current else {
            continue;
        };
        let mut rebased = Vec::new();
        for (path, declaration) in manifest.amendments() {
            let provider = declaration.unit();
            // A missing copy, a copy with markers, and a file the provider
            // does not have are the build's to report.
            let (Some(copy), Some(now)) = (sibling.content.amendments.get(path), current.get(path))
            else {
                continue;
            };
            let copy = copy.contents();
            if has_markers(copy) {
                continue;
            }
            let Some(base) = stored.get(&directory, path) else {
                problems.push(in_unit(
                    &directory,
                    BuildProblem::BaseMissing {
                        path: path.clone(),
                        provider: provider.clone(),
                    },
                ));
                continue;
            };
            if base == now.bytes.as_slice()
                || conflicted.contains(&(provider.clone(), path.clone()))
            {
                continue;
            }
            if !rebase {
                problems.push(in_unit(
                    &directory,
                    BuildProblem::BaseStale {
                        path: path.clone(),
                        provider: provider.clone(),
                    },
                ));
                continue;
            }
            match rebase_copy(path, base, copy, &now.bytes) {
                Ok((bytes, conflicts)) => {
                    if !conflicts.is_empty() {
                        conflicted.insert((provider.clone(), path.clone()));
                    }
                    if let Some(key) = key(&directory, path) {
                        found.bases.insert(key, now.bytes.clone());
                    }
                    rebased.push(RebasedCopy {
                        unit: identity.clone(),
                        path: path.clone(),
                        provider: provider.clone(),
                        previous: copy.to_vec(),
                        copy: bytes,
                        conflicts,
                    });
                }
                Err(reason) => problems.push(in_unit(
                    &directory,
                    BuildProblem::BaseUnmergeable {
                        path: path.clone(),
                        provider: provider.clone(),
                        reason,
                    },
                )),
            }
        }
        // The units above compose the rebased copies.
        if let Some(sibling) = siblings.units.get_mut(&identity) {
            for copy in &rebased {
                let executable = sibling
                    .content
                    .amendments
                    .get(&copy.path)
                    .is_some_and(crate::domain::UnitFile::is_executable);
                sibling.content.amendments.insert(
                    copy.path.clone(),
                    crate::domain::UnitFile::new(copy.copy.clone(), executable),
                );
            }
        }
        found.copies.extend(rebased);
    }
    found
}

/// The units of `scope`, each after the units of the catalog it depends
/// on, and otherwise in identity order.
fn in_dependency_order(
    scope: &std::collections::BTreeSet<UnitIdentity>,
    siblings: &Siblings,
) -> Vec<UnitIdentity> {
    fn visit(
        identity: &UnitIdentity,
        siblings: &Siblings,
        seen: &mut std::collections::BTreeSet<UnitIdentity>,
        order: &mut Vec<UnitIdentity>,
    ) {
        if !seen.insert(identity.clone()) {
            return;
        }
        if let Some(sibling) = siblings.units.get(identity) {
            for dependency in sibling.manifest.dependencies().keys() {
                if siblings.contains(dependency) {
                    visit(dependency, siblings, seen, order);
                }
            }
        }
        order.push(identity.clone());
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut order = Vec::new();
    for identity in scope {
        visit(identity, siblings, &mut seen, &mut order);
    }
    order.retain(|identity| scope.contains(identity));
    order
}

/// The units of the catalog `identity` composes: itself and the units of
/// the catalog it depends on, directly or not.
pub(super) fn closure(
    identity: &UnitIdentity,
    siblings: &Siblings,
) -> std::collections::BTreeSet<UnitIdentity> {
    let mut found = std::collections::BTreeSet::new();
    let mut pending = vec![identity.clone()];
    while let Some(next) = pending.pop() {
        if !found.insert(next.clone()) {
            continue;
        }
        if let Some(sibling) = siblings.units.get(&next) {
            pending.extend(
                sibling
                    .manifest
                    .dependencies()
                    .keys()
                    .filter(|dependency| siblings.contains(dependency))
                    .cloned(),
            );
        }
    }
    found
}

/// Rebases `copy`, taken on `base`, onto `now`: the changes from `base` to
/// `now` are merged into the copy, and the lines or values both changed
/// differently are left between markers and reported. `Err` says why the
/// copy cannot be merged.
fn rebase_copy(
    path: &RelativePath,
    base: &[u8],
    copy: &[u8],
    now: &[u8],
) -> Result<(Vec<u8>, Vec<String>), String> {
    let texts = (
        std::str::from_utf8(base),
        std::str::from_utf8(copy),
        std::str::from_utf8(now),
    );
    let all_text = [base, copy, now].into_iter().all(is_text);
    let (Ok(base), Ok(copy), Ok(now)) = texts else {
        return Err("the file is not text, and only text is merged".to_owned());
    };
    let rebased = match MergeFormat::of_path(path) {
        MergeFormat::Json => rebase_json(base, copy, now).map_err(|failure| failure.to_string())?,
        MergeFormat::Text if all_text => rebase_text_on_stored(base, copy, now),
        MergeFormat::Text => return Err("the file is not text, and only text is merged".to_owned()),
        MergeFormat::Unsupported => {
            return Err("Assemblr has no merge for the file's format".to_owned());
        }
    };
    Ok((rebased.text.into_bytes(), rebased.conflicts))
}

/// Writes `drift` into the catalog whose top level is `root`: each rebased
/// copy, through a new file beside it and a rename, then the bases, whose
/// record is replaced last. When a write fails, the copies already
/// replaced are put back, so that the copies and the stored bases stay as
/// they were, and the error says whether that succeeded.
pub(super) fn apply(
    root: &Path,
    drift: &Drift,
    stored: &StoredBases,
) -> Result<(), RebaseWriteError> {
    let target = |copy: &RebasedCopy| {
        root.join(crate::adapters::git::UNITS)
            .join(copy.unit.name())
            .join(crate::adapters::git::UnitDirectory::Amendments.as_str())
            .join(copy.path.as_str())
    };
    let staging = |path: &Path| {
        let mut name = path
            .file_name()
            .map(|name| name.to_os_string())
            .unwrap_or_default();
        name.push(".assemblr-rebase-new");
        path.with_file_name(name)
    };
    for copy in &drift.copies {
        let staged = staging(&target(copy));
        if fs::symlink_metadata(&staged).is_ok() {
            return Err(RebaseWriteError {
                path: staged,
                error: "the name is reserved for a rebased copy before it replaces the copy; \
                        remove it if an earlier build left it, and otherwise move it away"
                    .to_owned(),
                unrestored: Vec::new(),
            });
        }
    }
    let replace = |path: &Path, bytes: &[u8]| {
        let staged = staging(path);
        // The new file takes the copy's mode, so that its executable bit
        // stays.
        let written = fs::metadata(path).and_then(|metadata| {
            write_new(&staged, bytes)?;
            fs::set_permissions(&staged, metadata.permissions())?;
            fs::rename(&staged, path)
        });
        if written.is_err() {
            let _ = fs::remove_file(&staged);
        }
        written
    };
    let restore = |replaced: &[&RebasedCopy]| -> Vec<PathBuf> {
        replaced
            .iter()
            .filter_map(|copy| {
                let path = target(copy);
                replace(&path, &copy.previous).err().map(|_| path)
            })
            .collect()
    };
    let mut replaced: Vec<&RebasedCopy> = Vec::new();
    for copy in &drift.copies {
        let path = target(copy);
        if let Err(error) = replace(&path, &copy.copy) {
            return Err(RebaseWriteError {
                path,
                error: error.to_string(),
                unrestored: restore(&replaced),
            });
        }
        replaced.push(copy);
    }
    stored
        .replace(root, &drift.bases)
        .map_err(|error| RebaseWriteError {
            path: record_path(root),
            error: error.to_string(),
            unrestored: restore(&replaced),
        })
}

/// Writing a rebase failed.
#[derive(Debug)]
pub struct RebaseWriteError {
    path: PathBuf,
    error: String,
    /// The copies that were rebased and could not be put back.
    unrestored: Vec<PathBuf>,
}

impl fmt::Display for RebaseWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "could not write {}: {}", self.path.display(), self.error)?;
        if self.unrestored.is_empty() {
            f.write_str("; the copies and their stored bases are as they were")
        } else {
            let paths: Vec<String> = self
                .unrestored
                .iter()
                .map(|path| path.display().to_string())
                .collect();
            write!(
                f,
                "; {} are rebased and could not be put back, while their stored bases are not; \
                 the next build rebases them again",
                paths.join(", ")
            )
        }
    }
}

impl std::error::Error for RebaseWriteError {}

/// The key of the amendment of `path` by the unit in `units/<directory>`.
pub(super) fn key(directory: &str, path: &RelativePath) -> Option<RelativePath> {
    RelativePath::parse(&format!("{directory}/{path}")).ok()
}

/// The unit directory and the amended path of `key`.
pub(super) fn split(key: &RelativePath) -> Option<(&str, RelativePath)> {
    let (directory, path) = key.as_str().split_once('/')?;
    Some((directory, RelativePath::parse(path).ok()?))
}

fn record_path(root: &Path) -> PathBuf {
    root.join(STATE_DIR).join(RECORD)
}

/// Reads the record, a regular file of at most [`RECORD_LIMIT`] bytes.
fn read_record(path: &Path) -> io::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(RECORD_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > RECORD_LIMIT {
        return Err(io::Error::other("larger than Assemblr reads"));
    }
    Ok(bytes)
}

/// The listing's identity the record names, or what is wrong with it.
fn parse_record(bytes: &[u8]) -> Result<BaseIdentity, String> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|error| format!("it is not JSON: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "it is not a JSON object".to_owned())?;
    if let Some(unknown) = object
        .keys()
        .find(|key| !RECORD_MEMBERS.contains(&key.as_str()))
    {
        return Err(format!(
            "it has the member `{unknown}`, which this version of Assemblr does not know"
        ));
    }
    match object.get("format").and_then(Value::as_u64) {
        Some(RECORD_FORMAT) => {}
        Some(format) => {
            return Err(format!(
                "its format is {format}, which this version of Assemblr does not know"
            ));
        }
        None => return Err("it has no `format` number".to_owned()),
    }
    object
        .get("base")
        .and_then(Value::as_str)
        .and_then(BaseIdentity::parse)
        .ok_or_else(|| "its `base` is not a base identity".to_owned())
}

/// Why the stored bases of a catalog's amendments cannot be used.
#[derive(Debug)]
pub enum BaseStoreError {
    /// The record could not be read or written.
    Io {
        /// The path.
        path: PathBuf,
        /// Why.
        error: io::Error,
    },
    /// The record is not one this version reads.
    Record {
        /// The record.
        path: PathBuf,
        /// What is wrong.
        detail: String,
    },
    /// The listing or a blob it names is missing, or does not verify.
    Store(StoreError),
}

impl fmt::Display for BaseStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let lost = "the stored bases of the catalog's amendments cannot be used";
        match self {
            Self::Io { path, error } => write!(f, "could not use {}: {error}", path.display()),
            Self::Record { path, detail } => {
                write!(f, "{} cannot be read: {detail}; {lost}", path.display())
            }
            Self::Store(StoreError::Missing(path)) => write!(
                f,
                "{} does not exist, and {RECORD} names it; {lost}",
                path.display()
            ),
            // The consumer's store says what its lockfile records; here the
            // record names the listing.
            Self::Store(StoreError::Corrupt { path, detail }) => write!(
                f,
                "{} does not verify: {}; {lost}",
                path.display(),
                detail.replace("the lockfile records", &format!("{RECORD} records"))
            ),
            Self::Store(StoreError::Listing { path, error }) => {
                write!(f, "{} cannot be read: {error}; {lost}", path.display())
            }
            Self::Store(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for BaseStoreError {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "a failed fixture fails the test")]

    use super::*;

    fn path(text: &str) -> RelativePath {
        RelativePath::parse(text).unwrap()
    }

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("assemblr-bases-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn stored_bases_read_back_and_replace_their_predecessor() {
        let root = temp("replace");
        let none = StoredBases::read(&root).unwrap();
        assert!(none.all().is_empty());
        let first = BTreeMap::from([(path("b/a.txt"), b"one\n".to_vec())]);
        none.replace(&root, &first).unwrap();
        let read = StoredBases::read(&root).unwrap();
        assert_eq!(read.get("b", &path("a.txt")), Some(&b"one\n"[..]));
        let second = BTreeMap::from([(path("b/a.txt"), b"two\n".to_vec())]);
        read.replace(&root, &second).unwrap();
        let read = StoredBases::read(&root).unwrap();
        assert_eq!(read.get("b", &path("a.txt")), Some(&b"two\n"[..]));
        // The first listing and its blob are gone.
        let listings = fs::read_dir(root.join(".assemblr/bases")).unwrap().count();
        assert_eq!(listings, 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_corrupt_or_unknown_record_is_an_explicit_failure() {
        let root = temp("corrupt");
        let bases = BTreeMap::from([(path("b/a.txt"), b"one\n".to_vec())]);
        StoredBases::default().replace(&root, &bases).unwrap();
        let objects = root.join(".assemblr/objects");
        let prefix = fs::read_dir(&objects)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let blob = fs::read_dir(&prefix)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        fs::write(&blob, b"tampered\n").unwrap();
        let error = StoredBases::read(&root).unwrap_err().to_string();
        assert!(error.contains("does not verify"), "{error}");
        fs::remove_file(&blob).unwrap();
        let error = StoredBases::read(&root).unwrap_err().to_string();
        assert!(error.contains("does not exist"), "{error}");
        fs::write(
            root.join(".assemblr").join(RECORD),
            r#"{"format": 2, "base": "x"}"#,
        )
        .unwrap();
        let error = StoredBases::read(&root).unwrap_err().to_string();
        assert!(error.contains("its format is 2"), "{error}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn keys_join_and_split_a_directory_and_a_path() {
        let key = key("web", &path("config/app.json")).unwrap();
        assert_eq!(key.as_str(), "web/config/app.json");
        let (directory, amended) = split(&key).unwrap();
        assert_eq!((directory, amended.as_str()), ("web", "config/app.json"));
    }
}
