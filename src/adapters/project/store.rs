//! The consumer project's state directory, `.assemblr/`: the stored
//! reconciliation base and the journal of a composition being applied
//! (see `docs/adr/0016-consumer-compose.md`).
//!
//! The base is stored content-addressed so that it travels with the
//! project in version control without clutter: each file's bytes once under
//! `objects/`, by their SHA-256, and the canonical listing of a base under
//! `bases/`, by the base's identity. Nothing in the store depends on the
//! file system's modes or on a checkout's line-ending rules: modes are in
//! the listing, and the directory's own `.gitattributes` marks everything
//! binary, so that Git never converts the stored bytes. Verification reads
//! the listing and every blob back and compares them with the identities
//! the lock records.
//!
//! The journal lists the files a composition is about to create. It is
//! written after the store and before the project's files, and removed
//! after the lock, so that its presence means an interrupted composition:
//! the next one reverts the files it lists whose content is still what was
//! planned, and starts over.

use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::domain::{BaseIdentity, BlobId, ListingError, RelativePath, Snapshot};

/// The state directory, at the project's top level.
pub(crate) const STATE_DIR: &str = ".assemblr";
/// The attributes file that keeps Git from converting stored bytes.
const ATTRIBUTES: &str = ".gitattributes";
/// What it says.
const ATTRIBUTES_TEXT: &str = "* -text\n";
/// The blobs, by content identity.
const OBJECTS: &str = "objects";
/// The listings, by base identity.
const BASES: &str = "bases";
/// The journal of a composition being applied.
const JOURNAL: &str = "journal.json";
/// Where the journal is written before it is put in place.
const JOURNAL_NEW: &str = "journal.json.new";
/// The version of the journal's format.
const JOURNAL_FORMAT: u64 = 1;
/// The largest listing or journal read, in bytes.
const TEXT_LIMIT: u64 = 64 * 1024 * 1024;

/// The store under `root/.assemblr`.
pub(super) struct Store {
    dir: PathBuf,
}

impl Store {
    pub(super) fn new(root: &Path) -> Self {
        Self {
            dir: root.join(STATE_DIR),
        }
    }

    fn blob_path(&self, blob: &BlobId) -> PathBuf {
        let (prefix, rest) = blob.as_str().split_at(2);
        self.dir.join(OBJECTS).join(prefix).join(rest)
    }

    fn listing_path(&self, base: &BaseIdentity) -> PathBuf {
        self.dir.join(BASES).join(base.digits())
    }

    /// The path of the listing of `base`, relative to the project's top
    /// level, as the project's version control sees it.
    pub(super) fn listing_relative(&self, base: &BaseIdentity) -> String {
        format!("{STATE_DIR}/{BASES}/{}", base.digits())
    }

    /// Writes `snapshot`'s listing and the blobs of `files`, the bytes of
    /// each path to store, creating the directory and its attributes when
    /// they do not exist. A blob or listing already stored is left as it
    /// is when its bytes are the expected ones, and is an error otherwise.
    pub(super) fn write_base(
        &self,
        snapshot: &Snapshot,
        files: &[(&RelativePath, &[u8])],
    ) -> Result<(), StoreError> {
        let io = |path: &Path, error: io::Error| StoreError::Io {
            path: path.to_owned(),
            error,
        };
        for dir in [
            self.dir.clone(),
            self.dir.join(OBJECTS),
            self.dir.join(BASES),
        ] {
            match fs::symlink_metadata(&dir) {
                Ok(metadata) if metadata.is_dir() => {}
                Ok(_) => return Err(StoreError::NotADirectory(dir)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    fs::create_dir(&dir).map_err(|error| io(&dir, error))?;
                }
                Err(error) => return Err(io(&dir, error)),
            }
        }
        let attributes = self.dir.join(ATTRIBUTES);
        match fs::symlink_metadata(&attributes) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                write_new(&attributes, ATTRIBUTES_TEXT.as_bytes())
                    .map_err(|error| io(&attributes, error))?;
            }
            Err(error) => return Err(io(&attributes, error)),
            Ok(_) => {}
        }
        for (path, bytes) in files {
            let Some(file) = snapshot.files().get(*path) else {
                continue;
            };
            let blob = self.blob_path(file.blob());
            if let Some(parent) = blob.parent() {
                match fs::symlink_metadata(parent) {
                    Ok(metadata) if metadata.is_dir() => {}
                    Ok(_) => return Err(StoreError::NotADirectory(parent.to_owned())),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        fs::create_dir(parent).map_err(|error| io(parent, error))?;
                    }
                    Err(error) => return Err(io(parent, error)),
                }
            }
            self.write_object(&blob, bytes)?;
        }
        let listing = self.listing_path(&snapshot.identity());
        self.write_object(&listing, snapshot.listing().as_bytes())
    }

    /// Writes a content-addressed file, or checks an existing one.
    fn write_object(&self, path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
        let io = |error: io::Error| StoreError::Io {
            path: path.to_owned(),
            error,
        };
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() => {
                let existing = fs::read(path).map_err(io)?;
                if existing == bytes {
                    Ok(())
                } else {
                    Err(StoreError::Corrupt {
                        path: path.to_owned(),
                        detail: "its bytes are not the content it is named after".to_owned(),
                    })
                }
            }
            Ok(_) => Err(StoreError::Corrupt {
                path: path.to_owned(),
                detail: "it is not a regular file".to_owned(),
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                write_new(path, bytes).map_err(io)
            }
            Err(error) => Err(io(error)),
        }
    }

    /// Reads the base `identity` back and verifies it: the listing's bytes
    /// hash to the identity, and every blob of a path not in `unbased` is
    /// stored with the bytes its identity names.
    pub(super) fn verify_base(
        &self,
        identity: &BaseIdentity,
        unbased: &BTreeSet<RelativePath>,
    ) -> Result<Snapshot, StoreError> {
        let listing_path = self.listing_path(identity);
        let text = read_bounded(&listing_path, TEXT_LIMIT).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => StoreError::Missing(listing_path.clone()),
            _ => StoreError::Io {
                path: listing_path.clone(),
                error,
            },
        })?;
        let text = String::from_utf8(text).map_err(|_| StoreError::Corrupt {
            path: listing_path.clone(),
            detail: "it is not UTF-8 text".to_owned(),
        })?;
        if !identity.verifies(&text) {
            return Err(StoreError::Corrupt {
                path: listing_path,
                detail: "its bytes do not hash to the identity the lockfile records".to_owned(),
            });
        }
        let snapshot = Snapshot::parse_listing(&text).map_err(|error| StoreError::Listing {
            path: listing_path.clone(),
            error,
        })?;
        for (path, file) in snapshot.files() {
            if unbased.contains(path) {
                continue;
            }
            let blob_path = self.blob_path(file.blob());
            let bytes = read_bounded(&blob_path, u64::MAX).map_err(|error| match error.kind() {
                io::ErrorKind::NotFound => StoreError::Missing(blob_path.clone()),
                _ => StoreError::Io {
                    path: blob_path.clone(),
                    error,
                },
            })?;
            if BlobId::of(&bytes) != *file.blob() {
                return Err(StoreError::Corrupt {
                    path: blob_path,
                    detail: format!("its bytes are not the content of {path} the listing names"),
                });
            }
        }
        Ok(snapshot)
    }

    /// The journal, if a composition was interrupted.
    pub(super) fn journal(&self) -> Result<Option<Journal>, StoreError> {
        let path = self.dir.join(JOURNAL);
        let bytes = match read_bounded(&path, TEXT_LIMIT) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(StoreError::Io { path, error }),
        };
        Journal::parse(&bytes)
            .map(Some)
            .map_err(|detail| StoreError::Corrupt { path, detail })
    }

    /// Records `journal`, durably, before the project's files are written.
    pub(super) fn begin(&self, journal: &Journal) -> Result<(), StoreError> {
        let staged = self.dir.join(JOURNAL_NEW);
        let path = self.dir.join(JOURNAL);
        let io = |path: &Path, error: io::Error| StoreError::Io {
            path: path.to_owned(),
            error,
        };
        let bytes = journal.serialize().map_err(|error| io(&staged, error))?;
        let _ = fs::remove_file(&staged);
        write_new(&staged, &bytes).map_err(|error| io(&staged, error))?;
        fs::rename(&staged, &path).map_err(|error| io(&path, error))?;
        sync_dir(&self.dir);
        Ok(())
    }

    /// Removes the journal: the composition is complete.
    pub(super) fn finish(&self) -> Result<(), StoreError> {
        let path = self.dir.join(JOURNAL);
        fs::remove_file(&path).map_err(|error| StoreError::Io { path, error })
    }
}

/// What a composition is about to write in the project: the files, each
/// with the identity of its planned content, and the directories it will
/// create, outermost first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Journal {
    pub(super) files: Vec<(RelativePath, BlobId)>,
    pub(super) directories: Vec<RelativePath>,
}

impl Journal {
    fn serialize(&self) -> io::Result<Vec<u8>> {
        let files: Vec<Value> = self
            .files
            .iter()
            .map(|(path, blob)| json!({ "path": path.as_str(), "blob": blob.as_str() }))
            .collect();
        let directories: Vec<Value> = self
            .directories
            .iter()
            .map(|path| Value::String(path.as_str().to_owned()))
            .collect();
        let mut bytes = serde_json::to_vec_pretty(&json!({
            "format": JOURNAL_FORMAT,
            "files": files,
            "directories": directories,
        }))?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let value: Value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        let root = value
            .as_object()
            .ok_or_else(|| "the journal is not a JSON object".to_owned())?;
        match root.get("format").and_then(Value::as_u64) {
            Some(JOURNAL_FORMAT) => {}
            Some(other) => {
                return Err(format!(
                    "it has format {other}, which this version of Assemblr does not read"
                ));
            }
            None => return Err("it has no format number".to_owned()),
        }
        let files = root
            .get("files")
            .and_then(Value::as_array)
            .ok_or_else(|| "it has no `files` list".to_owned())?
            .iter()
            .map(|entry| {
                let path = entry
                    .get("path")
                    .and_then(Value::as_str)
                    .and_then(|text| RelativePath::parse(text).ok());
                let blob = entry
                    .get("blob")
                    .and_then(Value::as_str)
                    .and_then(BlobId::parse);
                match (path, blob) {
                    (Some(path), Some(blob)) => Ok((path, blob)),
                    _ => Err("a `files` entry has no valid `path` and `blob`".to_owned()),
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let directories = root
            .get("directories")
            .and_then(Value::as_array)
            .ok_or_else(|| "it has no `directories` list".to_owned())?
            .iter()
            .map(|entry| {
                entry
                    .as_str()
                    .and_then(|text| RelativePath::parse(text).ok())
                    .ok_or_else(|| "a `directories` entry is not a valid path".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { files, directories })
    }
}

/// Creates `path` new with `bytes`, flushed.
pub(super) fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Flushes a directory's entries where the platform can.
pub(super) fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    let _ = File::open(dir).and_then(|dir| dir.sync_all());
    #[cfg(not(unix))]
    let _ = dir;
}

/// Reads a regular file, at most `limit` bytes.
fn read_bounded(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::other("larger than Assemblr reads"));
    }
    Ok(bytes)
}

/// Why the store could not be used.
#[derive(Debug)]
pub enum StoreError {
    /// A file or directory could not be read or written.
    Io {
        /// The path.
        path: PathBuf,
        /// Why.
        error: io::Error,
    },
    /// A path of the store is not a directory.
    NotADirectory(PathBuf),
    /// A stored file the lock refers to does not exist.
    Missing(PathBuf),
    /// A stored file is not what its name says.
    Corrupt {
        /// The file.
        path: PathBuf,
        /// What is wrong.
        detail: String,
    },
    /// A listing cannot be read.
    Listing {
        /// The listing.
        path: PathBuf,
        /// Why.
        error: ListingError,
    },
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, error } => write!(f, "could not use {}: {error}", path.display()),
            Self::NotADirectory(path) => write!(f, "{} is not a directory", path.display()),
            Self::Missing(path) => write!(
                f,
                "{} does not exist; the reconciliation base the lockfile records is not stored",
                path.display()
            ),
            Self::Corrupt { path, detail } => write!(
                f,
                "{} does not verify: {detail}; the stored reconciliation base cannot be trusted",
                path.display()
            ),
            Self::Listing { path, error } => {
                write!(f, "{} is not a base listing: {error}", path.display())
            }
        }
    }
}

impl std::error::Error for StoreError {}
