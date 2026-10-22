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
//! The journal lists what a composition is about to do to each file:
//! create it, replace it, or remove it, with the identities of the planned
//! and the previous content. It is written after the store and before the
//! project's files, and removed after the lock, so that its presence means
//! an interrupted composition: the next one completes it when the lock is
//! in place and reverts it otherwise. The previous bytes of the files a
//! composition replaces or removes wait under `previous/` until it
//! completes. A journal with a `pending` record belongs to a composition
//! that is written except for its lock, which waits, kept under
//! `previous/`, for the consumer to resolve the conflicts marked in some
//! files (`compose --continue` or `--abort`). The journal and `previous/`
//! are the project's transient
//! state, which the directory's own `.gitignore` keeps out of version
//! control: the consumer's edits never enter the tracked store.

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
/// The ignore file that keeps the transient state out of version control.
const IGNORE: &str = ".gitignore";
/// What it says.
const IGNORE_TEXT: &str = "/journal.json\n/journal.json.new\n/previous/\n";
/// The previous content of the files a composition replaces or removes,
/// by content identity, until it completes.
const PREVIOUS: &str = "previous";
/// The blobs, by content identity.
const OBJECTS: &str = "objects";
/// The listings, by base identity.
const BASES: &str = "bases";
/// The journal of a composition being applied.
const JOURNAL: &str = "journal.json";
/// Where the journal is written before it is put in place.
const JOURNAL_NEW: &str = "journal.json.new";
/// The version of the journal's format this version writes. Every change to
/// the journal's members takes a new number, and a reader refuses a number
/// or a member it does not know (ADR 0027).
const JOURNAL_FORMAT: u64 = 2;
/// The formats this version reads. Format 1 has the same members: its
/// journals were written before the number was raised, by versions that
/// ignored members they did not know, so a format-2 journal is one they
/// refuse instead of misreading its `pending` record.
const JOURNAL_FORMATS: [u64; 2] = [1, 2];
/// The members of a journal, of a `files` entry, and of `pending`.
const JOURNAL_MEMBERS: [&str; 5] = ["format", "base", "files", "directories", "pending"];
const ENTRY_MEMBERS: [&str; 6] = [
    "path",
    "action",
    "blob",
    "executable",
    "previous",
    "previous_executable",
];
const PENDING_MEMBERS: [&str; 2] = ["lock", "files"];
/// The largest listing or journal read, in bytes.
const TEXT_LIMIT: u64 = 64 * 1024 * 1024;

/// The store under `root/.assemblr`.
pub(super) struct Store {
    dir: PathBuf,
    /// The store's own files that writing the base restored, because
    /// they had been edited.
    restored: std::cell::RefCell<Vec<&'static str>>,
}

impl Store {
    pub(super) fn new(root: &Path) -> Self {
        Self {
            dir: root.join(STATE_DIR),
            restored: std::cell::RefCell::new(Vec::new()),
        }
    }

    /// The names of the store's own files, `.gitattributes` and
    /// `.gitignore`, that [`write_base`](Self::write_base) restored
    /// because they had been edited.
    pub(super) fn restored(&self) -> Vec<&'static str> {
        self.restored.borrow().clone()
    }

    /// The store's directories as `git check-ignore` must see them, with a
    /// trailing slash, since a rule such as `.assemblr/objects/` matches
    /// the directory only when it is named as one.
    pub(super) fn directories(&self) -> [String; 3] {
        [
            format!("{STATE_DIR}/"),
            format!("{STATE_DIR}/{OBJECTS}/"),
            format!("{STATE_DIR}/{BASES}/"),
        ]
    }

    fn blob_path(&self, blob: &BlobId) -> PathBuf {
        let (prefix, rest) = blob.as_str().split_at(2);
        self.dir.join(OBJECTS).join(prefix).join(rest)
    }

    fn listing_path(&self, base: &BaseIdentity) -> PathBuf {
        self.dir.join(BASES).join(base.digits())
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
        // The attributes and ignore files are Assemblr's own: when one was
        // edited, it is restored, since without them a checkout could
        // convert the store or commit the transient state.
        for (name, text) in [(ATTRIBUTES, ATTRIBUTES_TEXT), (IGNORE, IGNORE_TEXT)] {
            let path = self.dir.join(name);
            match fs::symlink_metadata(&path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    write_new(&path, text.as_bytes()).map_err(|error| io(&path, error))?;
                }
                Err(error) => return Err(io(&path, error)),
                Ok(metadata) if metadata.is_file() => {
                    let current = fs::read(&path).map_err(|error| io(&path, error))?;
                    if current != text.as_bytes() {
                        let staged = self.dir.join(format!("{name}.new"));
                        let _ = fs::remove_file(&staged);
                        write_new(&staged, text.as_bytes())
                            .and_then(|()| fs::rename(&staged, &path))
                            .map_err(|error| io(&path, error))?;
                        self.restored.borrow_mut().push(name);
                    }
                }
                Ok(_) => return Err(StoreError::NotADirectory(path)),
            }
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

    /// The stored bytes of `blob`.
    pub(super) fn read_blob(&self, blob: &BlobId) -> Result<Vec<u8>, StoreError> {
        let path = self.blob_path(blob);
        let bytes = read_bounded(&path, u64::MAX).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => StoreError::Missing(path.clone()),
            _ => StoreError::Io {
                path: path.clone(),
                error,
            },
        })?;
        if BlobId::of(&bytes) == *blob {
            Ok(bytes)
        } else {
            Err(StoreError::Corrupt {
                path,
                detail: "its bytes are not the content it is named after".to_owned(),
            })
        }
    }

    fn previous_path(&self, blob: &BlobId) -> PathBuf {
        self.dir.join(PREVIOUS).join(blob.as_str())
    }

    /// Keeps `bytes`, the previous content of a file the composition is
    /// about to replace or remove, until the composition completes.
    pub(super) fn save_previous(&self, bytes: &[u8]) -> Result<BlobId, StoreError> {
        let dir = self.dir.join(PREVIOUS);
        match fs::symlink_metadata(&dir) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Err(StoreError::NotADirectory(dir)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&dir).map_err(|error| StoreError::Io {
                    path: dir.clone(),
                    error,
                })?;
            }
            Err(error) => return Err(StoreError::Io { path: dir, error }),
        }
        let blob = BlobId::of(bytes);
        let path = self.previous_path(&blob);
        if fs::symlink_metadata(&path).is_ok() {
            let _ = fs::remove_file(&path);
        }
        write_new(&path, bytes).map_err(|error| StoreError::Io { path, error })?;
        Ok(blob)
    }

    /// The previous content `blob` that [`save_previous`](Self::save_previous)
    /// kept.
    pub(super) fn read_previous(&self, blob: &BlobId) -> Result<Vec<u8>, StoreError> {
        let path = self.previous_path(blob);
        let bytes = read_bounded(&path, u64::MAX).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => StoreError::Missing(path.clone()),
            _ => StoreError::Io {
                path: path.clone(),
                error,
            },
        })?;
        if BlobId::of(&bytes) == *blob {
            Ok(bytes)
        } else {
            Err(StoreError::Corrupt {
                path,
                detail: "its bytes are not the content it is named after".to_owned(),
            })
        }
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
            .map_err(|detail| StoreError::Journal { path, detail })
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

    /// Removes the journal and the previous content it refers to: the
    /// composition is complete, or reverted.
    pub(super) fn finish(&self) -> Result<(), StoreError> {
        let path = self.dir.join(JOURNAL);
        fs::remove_file(&path).map_err(|error| StoreError::Io { path, error })?;
        let _ = fs::remove_dir_all(self.dir.join(PREVIOUS));
        Ok(())
    }
}

/// What a composition is about to do in the project: each file with its
/// planned and previous content, and the directories it will create,
/// outermost first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Journal {
    /// The identity of the composition being applied: the lock, written
    /// after every file, records the same identity, which tells a
    /// completed composition from an interrupted one.
    pub(super) base: BaseIdentity,
    pub(super) files: Vec<(RelativePath, JournalEntry)>,
    pub(super) directories: Vec<RelativePath>,
    /// Set once every change is written but the lock: the composition
    /// waits for the consumer to settle the conflicts it left in files.
    pub(super) pending: Option<Pending>,
}

/// A composition that is written except for its lock, waiting for the
/// consumer to resolve the conflicts marked in some files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Pending {
    /// The lockfile `--continue` writes, kept under `previous/`.
    pub(super) lock: BlobId,
    /// The files that hold conflict markers, in path order.
    pub(super) files: Vec<RelativePath>,
}

/// What the composition does to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum JournalEntry {
    /// Creates it with this content.
    Create { blob: BlobId },
    /// Replaces its content, or its mode, keeping the previous bytes.
    Modify {
        blob: BlobId,
        executable: bool,
        previous: BlobId,
        previous_executable: bool,
    },
    /// Removes it, keeping the previous bytes.
    Remove {
        previous: BlobId,
        previous_executable: bool,
    },
}

impl Journal {
    fn serialize(&self) -> io::Result<Vec<u8>> {
        let files: Vec<Value> = self
            .files
            .iter()
            .map(|(path, entry)| match entry {
                JournalEntry::Create { blob } => {
                    json!({ "path": path.as_str(), "action": "create", "blob": blob.as_str() })
                }
                JournalEntry::Modify {
                    blob,
                    executable,
                    previous,
                    previous_executable,
                } => json!({
                    "path": path.as_str(),
                    "action": "modify",
                    "blob": blob.as_str(),
                    "executable": executable,
                    "previous": previous.as_str(),
                    "previous_executable": previous_executable,
                }),
                JournalEntry::Remove {
                    previous,
                    previous_executable,
                } => json!({
                    "path": path.as_str(),
                    "action": "remove",
                    "previous": previous.as_str(),
                    "previous_executable": previous_executable,
                }),
            })
            .collect();
        let directories: Vec<Value> = self
            .directories
            .iter()
            .map(|path| Value::String(path.as_str().to_owned()))
            .collect();
        let mut document = json!({
            "format": JOURNAL_FORMAT,
            "base": self.base.as_str(),
            "files": files,
            "directories": directories,
        });
        if let (Some(pending), Some(object)) = (&self.pending, document.as_object_mut()) {
            object.insert(
                "pending".to_owned(),
                json!({
                    "lock": pending.lock.as_str(),
                    "files": pending.files.iter().map(RelativePath::as_str).collect::<Vec<_>>(),
                }),
            );
        }
        let mut bytes = serde_json::to_vec_pretty(&document)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let value: Value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        let root = value
            .as_object()
            .ok_or_else(|| "the journal is not a JSON object".to_owned())?;
        match root.get("format").and_then(Value::as_u64) {
            Some(format) if JOURNAL_FORMATS.contains(&format) => {}
            Some(other) => {
                return Err(format!(
                    "it has format {other}, which this version of Assemblr does not read"
                ));
            }
            None => return Err("it has no format number".to_owned()),
        }
        known_members(&value, "it", &JOURNAL_MEMBERS)?;
        let base = root
            .get("base")
            .and_then(Value::as_str)
            .and_then(BaseIdentity::parse)
            .ok_or_else(|| "it has no valid `base` identity".to_owned())?;
        let files = root
            .get("files")
            .and_then(Value::as_array)
            .ok_or_else(|| "it has no `files` list".to_owned())?
            .iter()
            .map(|entry| {
                known_members(entry, "a `files` entry", &ENTRY_MEMBERS)?;
                let path = entry
                    .get("path")
                    .and_then(Value::as_str)
                    .and_then(|text| RelativePath::parse(text).ok())
                    .ok_or_else(|| "a `files` entry has no valid `path`".to_owned())?;
                let blob = |name: &str| {
                    entry
                        .get(name)
                        .and_then(Value::as_str)
                        .and_then(BlobId::parse)
                        .ok_or_else(|| format!("the entry for {path} has no valid `{name}`"))
                };
                let flag = |name: &str| entry.get(name).and_then(Value::as_bool).unwrap_or(false);
                // An entry without an action was written by a first
                // composition, which only creates.
                let action = entry
                    .get("action")
                    .and_then(Value::as_str)
                    .unwrap_or("create");
                let entry = match action {
                    "create" => JournalEntry::Create {
                        blob: blob("blob")?,
                    },
                    "modify" => JournalEntry::Modify {
                        blob: blob("blob")?,
                        executable: flag("executable"),
                        previous: blob("previous")?,
                        previous_executable: flag("previous_executable"),
                    },
                    "remove" => JournalEntry::Remove {
                        previous: blob("previous")?,
                        previous_executable: flag("previous_executable"),
                    },
                    other => return Err(format!("the entry for {path} has the action {other:?}")),
                };
                Ok((path, entry))
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
        let pending = match root.get("pending") {
            None => None,
            Some(pending) => {
                known_members(pending, "its `pending`", &PENDING_MEMBERS)?;
                let lock = pending
                    .get("lock")
                    .and_then(Value::as_str)
                    .and_then(BlobId::parse)
                    .ok_or_else(|| "its `pending` has no valid `lock`".to_owned())?;
                let files = pending
                    .get("files")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "its `pending` has no `files` list".to_owned())?
                    .iter()
                    .map(|entry| {
                        entry
                            .as_str()
                            .and_then(|text| RelativePath::parse(text).ok())
                            .ok_or_else(|| "a `pending` file is not a valid path".to_owned())
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Some(Pending { lock, files })
            }
        };
        Ok(Self {
            base,
            files,
            directories,
            pending,
        })
    }
}

/// Refuses `value`, an object of the journal, when it has a member other
/// than `members`: a member this version does not know may change what
/// recovery must do, so ignoring it could revert what it should keep.
fn known_members(value: &Value, what: &str, members: &[&str]) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{what} is not a JSON object"))?;
    match object.keys().find(|key| !members.contains(&key.as_str())) {
        Some(unknown) => Err(format!(
            "{what} has the member `{unknown}`, which this version of Assemblr does not know"
        )),
        None => Ok(()),
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
    /// The journal of a composition cannot be read: it is damaged, or
    /// written by a version of Assemblr with a format or members this one
    /// does not know. Nothing is done with it.
    Journal {
        /// The journal.
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
            Self::Journal { path, detail } => write!(
                f,
                "{} cannot be read: {detail}; the composition it records is left as it is",
                path.display()
            ),
            Self::Listing { path, error } => {
                write!(f, "{} is not a base listing: {error}", path.display())
            }
        }
    }
}

impl std::error::Error for StoreError {}
