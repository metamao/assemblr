//! The consumer project's state directory, `.assemblr/`: the stored
//! reconciliation base and the journal of a composition being applied
//! (see `docs/adr/0016-consumer-compose.md`).
//!
//! The base is stored as one pack, a single file named by the base's
//! identity under `packs/`, holding the canonical listing and the bytes of
//! every stored file once, so that it travels with the project in version
//! control as one file instead of one per composed file (ADR 0044). Only
//! the packs of the bases in use are kept: once the lock records a base,
//! every other pack goes. Nothing in the store depends on the file
//! system's modes or on a checkout's line-ending rules: modes are in the
//! listing, and the directory's own `.gitattributes` marks everything
//! binary and generated, so that Git never converts the stored bytes and
//! diffs leave them out. Verification reads the listing and every blob back
//! and compares them with the identities the lock records.
//!
//! Alpha versions before the pack stored each file's bytes under
//! `objects/` and each listing under `bases/`. That layout is not read: it
//! is recognized, and refused with a diagnostic of its own.
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

use std::collections::{BTreeMap, BTreeSet};
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
/// What it says: no line-ending conversion, no textual diff, and a
/// generated file for hosting services such as GitHub, which collapse it in
/// a pull request.
const ATTRIBUTES_TEXT: &str = "* -text -diff linguist-generated\n";
/// The ignore file that keeps the transient state out of version control.
const IGNORE: &str = ".gitignore";
/// What it says.
const IGNORE_TEXT: &str = "/journal.json\n/journal.json.new\n/previous/\n";
/// The previous content of the files a composition replaces or removes,
/// by content identity, until it completes.
const PREVIOUS: &str = "previous";
/// The packs, by base identity.
const PACKS: &str = "packs";
/// The first line of a pack, which names its format.
const PACK_HEADER: &str = "assemblr-pack-v1\n";
/// The directories of the layout alpha versions wrote before the pack.
const EARLIER: [&str; 2] = ["objects", "bases"];
/// The journal of a composition being applied.
const JOURNAL: &str = "journal.json";
/// Where the journal is written before it is put in place.
const JOURNAL_NEW: &str = "journal.json.new";
/// The version of the journal's format this version writes. Every change to
/// the journal's members takes a new number, and a reader refuses a number
/// or a member it does not know (ADR 0027).
const JOURNAL_FORMAT: u64 = 3;
/// The formats this version reads. Format 1 has the same members: its
/// journals were written before the number was raised, by versions that
/// ignored members they did not know, so a format-2 journal is one they
/// refuse instead of misreading its `pending` record. Format 3 adds the
/// `declaration` of a `pending` record; a format-2 record has none.
const JOURNAL_FORMATS: [u64; 3] = [1, 2, 3];
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
const PENDING_MEMBERS: [&str; 3] = ["lock", "files", "declaration"];
/// The largest listing or journal read, in bytes.
const TEXT_LIMIT: u64 = 64 * 1024 * 1024;

/// The store under `root/.assemblr`.
pub(crate) struct Store {
    dir: PathBuf,
    /// The store's own files that writing the base restored, because
    /// they had been edited.
    restored: std::cell::RefCell<Vec<&'static str>>,
    /// The blobs of the packs [`verify_base`](Self::verify_base) verified,
    /// which [`read_blob`](Self::read_blob) serves.
    verified: std::cell::RefCell<BTreeMap<BlobId, Vec<u8>>>,
}

impl Store {
    pub(crate) fn new(root: &Path) -> Self {
        Self {
            dir: root.join(STATE_DIR),
            restored: std::cell::RefCell::new(Vec::new()),
            verified: std::cell::RefCell::new(BTreeMap::new()),
        }
    }

    /// The names of the store's own files, `.gitattributes` and
    /// `.gitignore`, that [`write_base`](Self::write_base) restored
    /// because they had been edited.
    pub(super) fn restored(&self) -> Vec<&'static str> {
        self.restored.borrow().clone()
    }

    /// The store's directories as `git check-ignore` must see them, with a
    /// trailing slash, since a rule such as `.assemblr/packs/` matches
    /// the directory only when it is named as one.
    pub(super) fn directories(&self) -> [String; 2] {
        [format!("{STATE_DIR}/"), format!("{STATE_DIR}/{PACKS}/")]
    }

    fn pack_path(&self, base: &BaseIdentity) -> PathBuf {
        self.dir.join(PACKS).join(base.digits())
    }

    /// Writes `snapshot`'s pack, with its listing and the blobs of `files`,
    /// the bytes of each path to store, creating the directory and its
    /// attributes when they do not exist. A pack already stored with other
    /// bytes, which a composition with other unbased files leaves, is
    /// replaced: it is named after a base the lock does not record, since a
    /// composition the lock records writes nothing.
    pub(crate) fn write_base(
        &self,
        snapshot: &Snapshot,
        files: &[(&RelativePath, &[u8])],
    ) -> Result<(), StoreError> {
        let io = |path: &Path, error: io::Error| StoreError::Io {
            path: path.to_owned(),
            error,
        };
        for dir in [self.dir.clone(), self.dir.join(PACKS)] {
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
        let mut blobs = BTreeMap::new();
        for (path, bytes) in files {
            if let Some(file) = snapshot.files().get(*path) {
                blobs.insert(file.blob().clone(), *bytes);
            }
        }
        let pack = encode_pack(&snapshot.listing(), &blobs);
        let path = self.pack_path(&snapshot.identity());
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => {
                let existing = fs::read(&path).map_err(|error| io(&path, error))?;
                if existing == pack {
                    return Ok(());
                }
                fs::remove_file(&path).map_err(|error| io(&path, error))?;
            }
            Ok(_) => {
                return Err(StoreError::Corrupt {
                    path,
                    detail: "it is not a regular file".to_owned(),
                });
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io(&path, error)),
        }
        write_new(&path, &pack).map_err(|error| io(&path, error))
    }

    /// Reads the base `identity` back and verifies it: the listing's bytes
    /// hash to the identity, and every blob of a path not in `unbased` is
    /// stored with the bytes its identity names, in the base's pack.
    pub(crate) fn verify_base(
        &self,
        identity: &BaseIdentity,
        unbased: &BTreeSet<RelativePath>,
    ) -> Result<Snapshot, StoreError> {
        let pack_path = self.pack_path(identity);
        let bytes = match read_bounded(&pack_path, u64::MAX) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if EARLIER
                    .iter()
                    .any(|dir| fs::symlink_metadata(self.dir.join(dir)).is_ok())
                {
                    return Err(StoreError::EarlierLayout(self.dir.clone()));
                }
                return Err(StoreError::Missing(pack_path));
            }
            Err(error) => {
                return Err(StoreError::Io {
                    path: pack_path,
                    error,
                });
            }
        };
        let corrupt = |detail: String| StoreError::Corrupt {
            path: pack_path.clone(),
            detail,
        };
        let (listing, blobs) = decode_pack(&bytes).map_err(corrupt)?;
        if !identity.verifies(listing) {
            return Err(corrupt(
                "its listing does not hash to the identity the lockfile records".to_owned(),
            ));
        }
        let snapshot = Snapshot::parse_listing(listing).map_err(|error| StoreError::Listing {
            path: pack_path.clone(),
            error,
        })?;
        let named: BTreeSet<&BlobId> = snapshot.files().values().map(|file| file.blob()).collect();
        if let Some(blob) = blobs.keys().find(|blob| !named.contains(blob)) {
            return Err(corrupt(format!(
                "it holds the content {}, which its listing does not name",
                blob.as_str()
            )));
        }
        for (path, file) in snapshot.files() {
            if !unbased.contains(path) && !blobs.contains_key(file.blob()) {
                return Err(corrupt(format!("it does not hold the content of {path}")));
            }
        }
        self.verified.borrow_mut().extend(
            blobs
                .into_iter()
                .map(|(blob, bytes)| (blob, bytes.to_vec())),
        );
        Ok(snapshot)
    }

    /// The stored bytes of `blob`, from a pack
    /// [`verify_base`](Self::verify_base) verified.
    pub(crate) fn read_blob(&self, blob: &BlobId) -> Result<Vec<u8>, StoreError> {
        self.verified
            .borrow()
            .get(blob)
            .cloned()
            .ok_or_else(|| StoreError::Missing(self.dir.join(PACKS)))
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
            .map_err(|problem| match problem {
                JournalProblem::Unknown(detail) => StoreError::Journal { path, detail },
                JournalProblem::Damaged(detail) => StoreError::Corrupt { path, detail },
            })
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

    /// Removes the pack of every stored base but `keep`, the bases in use.
    /// Only the bases in use are kept, since an earlier one is recorded
    /// nowhere but in version control's history, which keeps it.
    /// This is cleanup of content nothing refers to, so a failure leaves
    /// files that are harmless, as an interrupted removal would, and is not
    /// reported.
    pub(crate) fn keep_only(&self, keep: &[&BaseIdentity]) {
        let packs = self.dir.join(PACKS);
        if let Ok(entries) = fs::read_dir(&packs) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let kept = keep.iter().any(|base| name.to_str() == Some(base.digits()));
                if !kept && entry.file_type().is_ok_and(|kind| kind.is_file()) {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
        // An empty directory is not tracked; one left empty goes.
        let _ = fs::remove_dir(&packs);
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
    /// The identity of the declaration the composition was planned from,
    /// which `--continue` requires to be the one in place still. Absent
    /// in a journal an earlier version wrote.
    pub(super) declaration: Option<BlobId>,
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
            let mut record = json!({
                "lock": pending.lock.as_str(),
                "files": pending.files.iter().map(RelativePath::as_str).collect::<Vec<_>>(),
            });
            if let (Some(declaration), Some(record)) =
                (&pending.declaration, record.as_object_mut())
            {
                record.insert(
                    "declaration".to_owned(),
                    Value::String(declaration.as_str().to_owned()),
                );
            }
            object.insert("pending".to_owned(), record);
        }
        let mut bytes = serde_json::to_vec_pretty(&document)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    fn parse(bytes: &[u8]) -> Result<Self, JournalProblem> {
        let value: Value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        let root = value
            .as_object()
            .ok_or_else(|| "the journal is not a JSON object".to_owned())?;
        match root.get("format").and_then(Value::as_u64) {
            Some(format) if JOURNAL_FORMATS.contains(&format) => {}
            Some(other) => {
                return Err(JournalProblem::Unknown(format!(
                    "it has format {other}, which this version of Assemblr does not read"
                )));
            }
            None => return Err("it has no format number".to_owned().into()),
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
                    other => {
                        return Err(format!("the entry for {path} has the action {other:?}").into());
                    }
                };
                Ok((path, entry))
            })
            .collect::<Result<Vec<_>, JournalProblem>>()?;
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
                let declaration = match pending.get("declaration") {
                    None => None,
                    Some(value) => {
                        Some(value.as_str().and_then(BlobId::parse).ok_or_else(|| {
                            "its `pending` has an invalid `declaration`".to_owned()
                        })?)
                    }
                };
                Some(Pending {
                    lock,
                    files,
                    declaration,
                })
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

/// Why a journal cannot be read: it is damaged, or it has a format or a
/// member this version does not know, which a newer version may have
/// written.
enum JournalProblem {
    Damaged(String),
    Unknown(String),
}

impl From<String> for JournalProblem {
    fn from(detail: String) -> Self {
        Self::Damaged(detail)
    }
}

/// Refuses `value`, an object of the journal, when it has a member other
/// than `members`: a member this version does not know may change what
/// recovery must do, so ignoring it could revert what it should keep.
fn known_members(value: &Value, what: &str, members: &[&str]) -> Result<(), JournalProblem> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{what} is not a JSON object"))?;
    match object.keys().find(|key| !members.contains(&key.as_str())) {
        Some(unknown) => Err(JournalProblem::Unknown(format!(
            "{what} has the member `{unknown}`, which this version of Assemblr does not know"
        ))),
        None => Ok(()),
    }
}

/// A pack's bytes: the header, the listing, and each blob once, in the
/// order of their identities, each part after a line that names it and
/// gives its length, so that any bytes can be stored. The bytes depend on
/// the listing and the blobs only. They are not compressed: Git compresses
/// them, and stores a pack that changed little as a delta of the one
/// before, which compressed bytes would prevent.
fn encode_pack(listing: &str, blobs: &BTreeMap<BlobId, &[u8]>) -> Vec<u8> {
    let size: usize = blobs.values().map(|bytes| bytes.len() + 96).sum();
    let mut pack = Vec::with_capacity(PACK_HEADER.len() + listing.len() + 32 + size);
    pack.extend_from_slice(PACK_HEADER.as_bytes());
    pack.extend_from_slice(format!("listing {}\n", listing.len()).as_bytes());
    pack.extend_from_slice(listing.as_bytes());
    for (blob, bytes) in blobs {
        pack.extend_from_slice(format!("blob {} {}\n", blob.as_str(), bytes.len()).as_bytes());
        pack.extend_from_slice(bytes);
    }
    pack
}

/// Reads a pack back: its listing, and its blobs, each verified against
/// its identity. Anything but what [`encode_pack`] writes is refused.
fn decode_pack(pack: &[u8]) -> Result<(&str, BTreeMap<BlobId, &[u8]>), String> {
    let rest = pack
        .strip_prefix(PACK_HEADER.as_bytes())
        .ok_or_else(|| "it is not a pack this version of Assemblr reads".to_owned())?;
    let (line, rest) = pack_line(rest)?;
    let length = line
        .strip_prefix("listing ")
        .and_then(pack_length)
        .ok_or_else(|| "it does not start with its listing".to_owned())?;
    let (listing, mut rest) = pack_take(rest, length)?;
    let listing =
        std::str::from_utf8(listing).map_err(|_| "its listing is not UTF-8 text".to_owned())?;
    let mut blobs: BTreeMap<BlobId, &[u8]> = BTreeMap::new();
    while !rest.is_empty() {
        let (line, after) = pack_line(rest)?;
        let (blob, length) = line
            .strip_prefix("blob ")
            .and_then(|line| line.split_once(' '))
            .and_then(|(blob, length)| Some((BlobId::parse(blob)?, pack_length(length)?)))
            .ok_or_else(|| format!("{line:?} does not introduce a content"))?;
        if blobs.keys().next_back().is_some_and(|last| *last >= blob) {
            return Err(format!(
                "the content {} is out of order or repeated",
                blob.as_str()
            ));
        }
        let (bytes, after) = pack_take(after, length)?;
        if BlobId::of(bytes) != blob {
            return Err(format!(
                "the bytes it holds as {} are not that content",
                blob.as_str()
            ));
        }
        blobs.insert(blob, bytes);
        rest = after;
    }
    Ok((listing, blobs))
}

/// The line at the start of `bytes`, without its line end, and what
/// follows it. A part's line is short, so a longer one is damage.
fn pack_line(bytes: &[u8]) -> Result<(&str, &[u8]), String> {
    let end = bytes
        .iter()
        .take(256)
        .position(|&byte| byte == b'\n')
        .ok_or_else(|| "it is cut short or damaged".to_owned())?;
    let (line, rest) = bytes.split_at(end);
    let line = std::str::from_utf8(line).map_err(|_| "a part's line is not text".to_owned())?;
    Ok((line, rest.get(1..).unwrap_or_default()))
}

/// The first `length` bytes of `bytes`, and what follows them.
fn pack_take(bytes: &[u8], length: usize) -> Result<(&[u8], &[u8]), String> {
    if bytes.len() < length {
        return Err("it is cut short".to_owned());
    }
    Ok(bytes.split_at(length))
}

/// A length as a pack writes it: decimal digits, without leading zeros.
fn pack_length(text: &str) -> Option<usize> {
    let length: usize = text.parse().ok()?;
    (length.to_string() == text).then_some(length)
}

/// Creates `path` new with `bytes`, flushed.
pub(crate) fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
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
    /// The store, this directory, holds the layout alpha versions wrote
    /// before the pack, which this version does not read.
    EarlierLayout(PathBuf),
    /// A stored file is not what its name says.
    Corrupt {
        /// The file.
        path: PathBuf,
        /// What is wrong.
        detail: String,
    },
    /// The journal of a composition has a format or a member this version
    /// of Assemblr does not know, as one a newer version wrote would.
    /// Nothing is done with it.
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
            Self::EarlierLayout(path) => write!(
                f,
                "{} stores its bases as an earlier alpha of Assemblr did, one file per content, \
                 which this version does not read",
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "fixtures are valid by construction")]

    use std::collections::BTreeMap;

    use super::{Journal, JournalEntry, decode_pack, encode_pack};
    use crate::domain::{BaseIdentity, BlobId, Snapshot};

    fn read(sample: &[u8]) -> Journal {
        Journal::parse(sample).unwrap_or_else(|_| panic!("a promised journal format is read"))
    }

    #[test]
    fn every_promised_journal_format_is_read() {
        // Samples never change (ADR 0041): a reader that stops reading one
        // fails here.
        let one = read(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/formats/journal-1.json"
        )));
        assert!(matches!(one.files[0].1, JournalEntry::Create { .. }));
        let two = read(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/formats/journal-2.json"
        )));
        assert_eq!(two.directories.len(), 1);
        assert!(two.pending.unwrap().declaration.is_none());
        let three = read(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/formats/journal-3.json"
        )));
        assert!(matches!(three.files[0].1, JournalEntry::Modify { .. }));
        assert!(matches!(three.files[1].1, JournalEntry::Remove { .. }));
        assert!(three.pending.unwrap().declaration.is_some());
    }

    #[test]
    fn the_promised_pack_format_is_read_and_written_the_same_way() {
        // The sample never changes (ADR 0041): a pack this version writes is
        // byte for byte the one an earlier version wrote for the same base,
        // so committed packs do not churn, and every later version reads it.
        let sample = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/formats/pack-1"));
        let (listing, blobs) =
            decode_pack(sample).unwrap_or_else(|detail| panic!("the sample reads: {detail}"));
        let identity = BaseIdentity::parse(
            "sha256-v1:d74e67f01654c3108fbae0969bf36da2f73ce4425e199eefdcd6c505dbc5db11",
        )
        .unwrap();
        assert!(identity.verifies(listing));
        let snapshot = Snapshot::parse_listing(listing).unwrap();
        assert_eq!(snapshot.files().len(), 2);
        assert_eq!(blobs.len(), 2);
        let owned: BTreeMap<BlobId, &[u8]> = blobs.into_iter().collect();
        assert_eq!(encode_pack(listing, &owned), sample);
    }

    #[test]
    fn a_pack_with_anything_but_what_is_written_is_refused() {
        let blob = |bytes: &[u8]| (BlobId::of(bytes), bytes.to_vec());
        let (one, two) = (blob(b"one"), blob(b"two"));
        let (first, second) = if one.0 < two.0 {
            (one, two)
        } else {
            (two, one)
        };
        let part = |(id, bytes): &(BlobId, Vec<u8>)| {
            [
                format!("blob {} {}\n", id.as_str(), bytes.len()).into_bytes(),
                bytes.clone(),
            ]
            .concat()
        };
        let head = b"assemblr-pack-v1\nlisting 3\nabc".to_vec();
        assert!(decode_pack(&[head.clone(), part(&first), part(&second)].concat()).is_ok());
        for (case, pack) in [
            (
                "out of order",
                [head.clone(), part(&second), part(&first)].concat(),
            ),
            (
                "repeated",
                [head.clone(), part(&first), part(&first)].concat(),
            ),
            (
                "leading zero",
                b"assemblr-pack-v1\nlisting 03\nabc".to_vec(),
            ),
            ("no listing", b"assemblr-pack-v1\n".to_vec()),
            ("long length", b"assemblr-pack-v1\nlisting 99\nabc".to_vec()),
            (
                "unknown part",
                [head.clone(), b"tree x 1\nx".to_vec()].concat(),
            ),
        ] {
            assert!(decode_pack(&pack).is_err(), "{case}");
        }
    }
}
