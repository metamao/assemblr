//! The registry's files: the index, replaced whole, and the lock that
//! serializes writers (see ADR 0007).
//!
//! Readers never take the lock. They see either the previous index or the
//! next one, because a new index is written to a temporary file and renamed
//! over the old one. Writers take the lock, read the index again under it,
//! and only then write, so no writer's update is lost.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::reference::is_local_reference;
use crate::domain::{
    ObjectId, Provenance, RegisteredUnit, RegistryIndex, SourceReference, UnitIdentity, Version,
};

/// The index's file.
const INDEX: &str = "index.json";
/// Where a new index is written before it replaces the old one.
const TEMPORARY: &str = "index.json.tmp";
/// The file writers lock.
const LOCK: &str = "lock";
/// The version of the index's format. A different version is rejected,
/// never guessed at.
const FORMAT: u64 = 1;

/// The registry's directory.
#[derive(Debug, Clone)]
pub(super) struct Store {
    dir: PathBuf,
}

/// The writers' lock, held until it is dropped.
#[derive(Debug)]
pub(super) struct Lock {
    _file: File,
}

impl Store {
    pub(super) fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Reads the index, or `None` if nothing has been registered yet.
    pub(super) fn load(&self) -> Result<Option<RegistryIndex>, StorageError> {
        let path = self.dir.join(INDEX);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(StorageError::io("read", &path, error)),
        };
        parse(&bytes)
            .map(Some)
            .map_err(|detail| StorageError::InvalidIndex { path, detail })
    }

    /// Takes the writers' lock, creating the registry's directory if it
    /// does not exist yet. Waits while another process holds it.
    pub(super) fn lock(&self) -> Result<Lock, StorageError> {
        fs::create_dir_all(&self.dir)
            .map_err(|error| StorageError::io("create", &self.dir, error))?;
        let path = self.dir.join(LOCK);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| StorageError::io("open", &path, error))?;
        // Called through the trait: newer standard libraries have an inherent
        // `File::lock`, which the minimum supported Rust does not.
        fs4::FileExt::lock(&file).map_err(|error| StorageError::io("lock", &path, error))?;
        Ok(Lock { _file: file })
    }

    /// Replaces the index with `index`. The lock proves the caller holds it.
    pub(super) fn replace(&self, _lock: &Lock, index: &RegistryIndex) -> Result<(), StorageError> {
        let temporary = self.dir.join(TEMPORARY);
        let mut bytes = serde_json::to_vec_pretty(&serialize(index))
            .map_err(|error| StorageError::io("serialize", &temporary, error.into()))?;
        bytes.push(b'\n');
        let written = File::create(&temporary).and_then(|mut file| {
            file.write_all(&bytes)?;
            file.sync_all()
        });
        if let Err(error) = written {
            // The active index is untouched. The temporary file is only
            // removed for tidiness, and a later write replaces it anyway.
            let _ = fs::remove_file(&temporary);
            return Err(StorageError::io("write", &temporary, error));
        }
        let path = self.dir.join(INDEX);
        fs::rename(&temporary, &path).map_err(|error| StorageError::io("replace", &path, error))?;
        // On Unix, the rename itself is durable only once the directory is
        // synchronized. The index has already been replaced, so a failure
        // here is not reported as a failed write.
        #[cfg(unix)]
        let _ = File::open(&self.dir).and_then(|dir| dir.sync_all());
        Ok(())
    }
}

fn serialize(index: &RegistryIndex) -> Value {
    let units: Map<String, Value> = index
        .units()
        .iter()
        .map(|(identity, unit)| {
            let versions: Map<String, Value> = unit
                .versions()
                .iter()
                .map(|(version, provenance)| {
                    (
                        version.to_string(),
                        json!({
                            "commit": provenance.commit().as_str(),
                            "tree": provenance.tree().as_str(),
                        }),
                    )
                })
                .collect();
            (
                identity.to_string(),
                json!({ "source": unit.source().as_str(), "versions": versions }),
            )
        })
        .collect();
    json!({ "format": FORMAT, "units": units })
}

/// Reads an index, or describes what is wrong with it.
fn parse(bytes: &[u8]) -> Result<RegistryIndex, String> {
    let value: Value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let root = object(&value, "the index")?;
    match root.get("format").and_then(Value::as_u64) {
        Some(FORMAT) => {}
        Some(other) => {
            return Err(format!(
                "it has format {other}, which this version of Assemblr does not read"
            ));
        }
        None => return Err("it has no format number".to_owned()),
    }
    let mut units = BTreeMap::new();
    for (key, unit) in object(field(root, "units", "the index")?, "`units`")? {
        let identity = UnitIdentity::parse(key).map_err(|error| error.to_string())?;
        let context = format!("unit {identity}");
        let unit = object(unit, &context)?;
        let source = field(unit, "source", &context)?
            .as_str()
            .filter(|source| is_local_reference(source))
            .ok_or_else(|| format!("{context} has no absolute, normalized source path"))?;
        let mut versions = BTreeMap::new();
        for (key, provenance) in object(field(unit, "versions", &context)?, &context)? {
            let version = Version::parse(key).map_err(|error| error.to_string())?;
            let context = format!("{identity} {version}");
            let provenance = object(provenance, &context)?;
            let id = |name| {
                field(provenance, name, &context)?
                    .as_str()
                    .and_then(ObjectId::parse)
                    .ok_or_else(|| format!("{context} has an invalid `{name}`"))
            };
            versions.insert(version, Provenance::new(id("commit")?, id("tree")?));
        }
        units.insert(
            identity,
            RegisteredUnit::new(SourceReference::new(source), versions),
        );
    }
    RegistryIndex::from_units(units).map_err(|errors| {
        errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    })
}

fn object<'a>(value: &'a Value, context: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{context} is not a JSON object"))
}

fn field<'a>(
    object: &'a Map<String, Value>,
    name: &str,
    context: &str,
) -> Result<&'a Value, String> {
    object
        .get(name)
        .ok_or_else(|| format!("{context} has no `{name}`"))
}

/// A failure to use the registry's files.
#[derive(Debug)]
pub enum StorageError {
    /// No directory for the registry could be determined.
    NoLocation,
    /// The directory given through the environment is not absolute.
    RelativeLocation(PathBuf),
    /// A file or directory could not be used.
    Io {
        /// What was being done, such as `read` or `lock`.
        action: &'static str,
        /// The file or directory.
        path: PathBuf,
        /// Why it failed.
        error: io::Error,
    },
    /// The index is not one this version of Assemblr wrote.
    InvalidIndex {
        /// The index's file.
        path: PathBuf,
        /// What is wrong with it.
        detail: String,
    },
}

impl StorageError {
    fn io(action: &'static str, path: &Path, error: io::Error) -> Self {
        Self::Io {
            action,
            path: path.to_owned(),
            error,
        }
    }
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoLocation => f.write_str(
                "could not determine the user's data directory, where the registry is kept",
            ),
            Self::RelativeLocation(path) => write!(
                f,
                "the registry location {} is not an absolute path",
                path.display()
            ),
            Self::Io {
                action,
                path,
                error,
            } => write!(f, "could not {action} {}: {error}", path.display()),
            Self::InvalidIndex { path, detail } => write!(
                f,
                "the registry index {} is invalid: {detail}; it was left as it is",
                path.display()
            ),
        }
    }
}

impl std::error::Error for StorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { error, .. } => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{parse, serialize};
    use crate::domain::{
        ObjectId, Provenance, RegisteredUnit, RegistryIndex, SourceReference, UnitIdentity, Version,
    };

    fn root() -> String {
        let root = std::path::absolute(std::path::Path::new("/")).unwrap();
        root.to_str().unwrap().to_owned()
    }

    fn sample() -> RegistryIndex {
        let provenance = Provenance::new(
            ObjectId::parse(&"a".repeat(40)).unwrap(),
            ObjectId::parse(&"b".repeat(64)).unwrap(),
        );
        RegistryIndex::from_units(BTreeMap::from([
            (
                UnitIdentity::parse("@acme/node").unwrap(),
                RegisteredUnit::new(
                    SourceReference::new(format!("{}units", root())),
                    BTreeMap::from([
                        (Version::parse("1.10.0").unwrap(), provenance.clone()),
                        (Version::parse("1.2.0").unwrap(), provenance),
                    ]),
                ),
            ),
            (
                UnitIdentity::parse("@acme/web").unwrap(),
                RegisteredUnit::new(SourceReference::new(root()), BTreeMap::new()),
            ),
        ]))
        .unwrap()
    }

    #[test]
    fn the_index_round_trips() {
        let index = sample();
        let bytes = serde_json::to_vec(&serialize(&index)).unwrap();

        assert_eq!(parse(&bytes), Ok(index));
    }

    #[test]
    fn invalid_indexes_are_rejected_with_a_reason() {
        let source = format!("{}units", root());
        let cases = [
            ("[]".to_owned(), "not a JSON object"),
            (r#"{"units": {}}"#.to_owned(), "no format"),
            (r#"{"format": 2, "units": {}}"#.to_owned(), "format 2"),
            (r#"{"format": 1, "units": {"acme": {}}}"#.to_owned(), "acme"),
            (
                r#"{"format": 1, "units": {"@acme/node": {"source": "units", "versions": {}}}}"#
                    .to_owned(),
                "absolute",
            ),
            (
                serde_json::json!({"format": 1, "units": {"@acme/node": {
                    "source": source, "versions": {"1.0.0": {"commit": "x", "tree": "y"}}
                }}})
                .to_string(),
                "invalid `commit`",
            ),
            (
                serde_json::json!({"format": 1, "units": {
                    "@acme/node": {"source": source, "versions": {}},
                    "@acme/web": {"source": source, "versions": {}},
                }})
                .to_string(),
                "several identities",
            ),
        ];
        for (json, expected) in cases {
            let error = parse(json.as_bytes()).unwrap_err();
            assert!(error.contains(expected), "{json}: {error}");
        }
    }
}
