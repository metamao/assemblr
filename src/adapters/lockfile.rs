//! The unit lockfile, `assemblr-lock.json`: its format, and reading it.
//!
//! The file is JSON with a `format` number and every locked unit's version,
//! commit, and tree, keys sorted and indented, so that it diffs cleanly when
//! it is tracked (see `docs/domain/authoring-workflow.md`). It is CLI
//! managed: a file of another format, or with unexpected content, is
//! reported and left as it is, never guessed at. Writing it is part of
//! publishing a build (see `super::build::output`). A published version's
//! lockfile is read from Git at the tagged commit (see [`read_published`]).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::compatibility;
use super::git::{GitRepository, ReadFileError};
use crate::domain::{
    BaseIdentity, LockedUnit, ObjectId, Provenance, ReconciliationState, RelativePath,
    UnitIdentity, UnitLock, Version,
};

/// The lockfile's name, at the unit repository's top level.
pub(crate) const LOCKFILE: &str = "assemblr-lock.json";
/// The version of the lockfile's format.
const FORMAT: u64 = 1;
/// The version of the consumer project's reconciliation state's format,
/// inside the lockfile.
const PROJECT_FORMAT: u64 = 1;
/// The largest lockfile read, in bytes: far more than any composition
/// needs.
const LOCKFILE_LIMIT: u64 = 8 * 1024 * 1024;

/// A lockfile's contents: the locked units, and, for a consumer project,
/// its reconciliation state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lockfile {
    /// The locked units.
    pub lock: UnitLock,
    /// The reconciliation state, present in a consumer project's lockfile
    /// and absent in a unit's.
    pub project: Option<ReconciliationState>,
}

/// Reads a unit's lockfile in `root`, or `None` if there is none yet. A
/// consumer project's lockfile is not a unit's, and is reported as such.
pub(crate) fn read(root: &Path) -> Result<Option<UnitLock>, LockfileError> {
    read_with_text(root).map(|found| found.map(|(_, lock)| lock))
}

/// Reads a unit's lockfile in `root` together with its exact bytes, or
/// `None` if there is none yet.
pub(crate) fn read_with_text(root: &Path) -> Result<Option<(Vec<u8>, UnitLock)>, LockfileError> {
    let Some((bytes, lockfile)) = read_lockfile(root)? else {
        return Ok(None);
    };
    if lockfile.project.is_some() {
        return Err(LockfileError::Invalid {
            path: root.join(LOCKFILE),
            detail: "it is a consumer project's lockfile, not a unit's".to_owned(),
        });
    }
    Ok(Some((bytes, lockfile.lock)))
}

/// Reads the lockfile in `root`, a unit's or a consumer project's, with its
/// exact bytes, or `None` if there is none yet.
pub(crate) fn read_lockfile(root: &Path) -> Result<Option<(Vec<u8>, Lockfile)>, LockfileError> {
    let path = root.join(LOCKFILE);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(LockfileError::Io { path, error }),
    };
    if !metadata.is_file() {
        return Err(LockfileError::NotAFile { path });
    }
    let mut bytes = Vec::new();
    fs::File::open(&path)
        .and_then(|file| file.take(LOCKFILE_LIMIT + 1).read_to_end(&mut bytes))
        .map_err(|error| LockfileError::Io {
            path: path.clone(),
            error,
        })?;
    if bytes.len() as u64 > LOCKFILE_LIMIT {
        return Err(LockfileError::Invalid {
            path,
            detail: format!("it is larger than the {LOCKFILE_LIMIT} bytes Assemblr reads"),
        });
    }
    let lockfile = parse(&bytes).map_err(|detail| LockfileError::Invalid { path, detail })?;
    Ok(Some((bytes, lockfile)))
}

/// Reads the lockfile committed at the root of `commit`'s tree in
/// `repository`, or `None` if the commit has none.
///
/// A catalog has one lockfile, at its root, for what its units depend on in
/// other repositories. Read for one of its units, the lock also holds the
/// unit's siblings, at the catalog's version and this commit, since they
/// are published with it.
pub(crate) fn read_published(
    repository: &GitRepository,
    commit: &ObjectId,
) -> Result<Option<UnitLock>, PublishedLockfileError> {
    let bytes = repository
        .read_top_level_file(commit, LOCKFILE, LOCKFILE_LIMIT)
        .map_err(PublishedLockfileError::Read)?;
    let Some(lock) = bytes
        .as_deref()
        .map(|bytes| match parse(bytes) {
            Ok(Lockfile {
                project: None,
                lock,
            }) => Ok(lock),
            Ok(Lockfile { .. }) => Err(PublishedLockfileError::Invalid(
                "it is a consumer project's lockfile, not a unit's".to_owned(),
            )),
            Err(detail) => Err(PublishedLockfileError::Invalid(detail)),
        })
        .transpose()?
    else {
        return Ok(None);
    };
    let siblings = repository
        .siblings_at(commit)
        .map_err(|error| PublishedLockfileError::Read(ReadFileError::Git(error)))?;
    let Some(siblings) = siblings else {
        return Ok(Some(lock));
    };
    let mut units = lock.units().clone();
    for (identity, version, provenance) in siblings {
        units.insert(identity, LockedUnit::new(version, provenance));
    }
    Ok(Some(UnitLock::new(units)))
}

/// The lockfile's contents for `lock`, a unit's.
pub(crate) fn serialize(lock: &UnitLock) -> io::Result<Vec<u8>> {
    serialize_lockfile(lock, None)
}

/// The lockfile's contents for `lock`, with a consumer project's
/// reconciliation `state` when there is one.
pub(crate) fn serialize_lockfile(
    lock: &UnitLock,
    state: Option<&ReconciliationState>,
) -> io::Result<Vec<u8>> {
    let units: Map<String, Value> = lock
        .units()
        .iter()
        .map(|(identity, unit)| {
            (
                identity.to_string(),
                json!({
                    "commit": unit.provenance().commit().as_str(),
                    "tree": unit.provenance().tree().as_str(),
                    "version": unit.version().to_string(),
                }),
            )
        })
        .collect();
    let mut document = json!({ "format": FORMAT, "units": units });
    if let Some(state) = state {
        let paths = |paths: &BTreeSet<RelativePath>| -> Vec<Value> {
            paths
                .iter()
                .map(|path| Value::String(path.as_str().to_owned()))
                .collect()
        };
        document["project"] = json!({
            "base": state.base().as_str(),
            "format": PROJECT_FORMAT,
            "managed": paths(state.managed()),
            "unbased": paths(state.unbased()),
        });
    }
    let mut bytes = serde_json::to_vec_pretty(&document)?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Reads a lockfile, or describes what is wrong with it.
pub(crate) fn parse(bytes: &[u8]) -> Result<Lockfile, String> {
    let value: Value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let root = object(&value, "the lockfile", &["format", "units", "project"])?;
    match root.get("format").and_then(Value::as_u64) {
        Some(FORMAT) => {}
        Some(other) => return Err(compatibility::unknown_format("it", other, FORMAT)),
        None => return Err("it has no format number".to_owned()),
    }
    let mut units = BTreeMap::new();
    let entries = root
        .get("units")
        .ok_or_else(|| "it has no `units`".to_owned())?;
    for (key, unit) in object(entries, "`units`", &[])? {
        let identity = UnitIdentity::parse(key).map_err(|error| error.to_string())?;
        let context = format!("unit {identity}");
        let unit = object(unit, &context, &["commit", "tree", "version"])?;
        let text = |name: &str| {
            unit.get(name)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{context} has no `{name}` string"))
        };
        let version = Version::parse(text("version")?)
            .map_err(|error| format!("{context} has an invalid `version`: {error}"))?;
        let id = |name: &str| {
            ObjectId::parse(text(name)?).ok_or_else(|| format!("{context} has an invalid `{name}`"))
        };
        let provenance = Provenance::new(id("commit")?, id("tree")?);
        units.insert(identity, LockedUnit::new(version, provenance));
    }
    let project = root.get("project").map(parse_state).transpose()?;
    Ok(Lockfile {
        lock: UnitLock::new(units),
        project,
    })
}

/// Reads a consumer project's reconciliation state.
fn parse_state(value: &Value) -> Result<ReconciliationState, String> {
    let state = object(
        value,
        "`project`",
        &["format", "base", "managed", "unbased"],
    )?;
    match state.get("format").and_then(Value::as_u64) {
        Some(PROJECT_FORMAT) => {}
        Some(other) => {
            return Err(compatibility::unknown_format(
                "`project`",
                other,
                PROJECT_FORMAT,
            ));
        }
        None => return Err("`project` has no format number".to_owned()),
    }
    let base = state
        .get("base")
        .and_then(Value::as_str)
        .and_then(BaseIdentity::parse)
        .ok_or_else(|| "`project` has no valid `base` identity".to_owned())?;
    let paths = |name: &str| -> Result<BTreeSet<RelativePath>, String> {
        let items = state
            .get(name)
            .and_then(Value::as_array)
            .ok_or_else(|| format!("`project` has no `{name}` list"))?;
        items
            .iter()
            .map(|item| {
                item.as_str()
                    .and_then(|text| RelativePath::parse(text).ok())
                    .ok_or_else(|| format!("`project.{name}` holds an invalid path"))
            })
            .collect()
    };
    Ok(ReconciliationState::new(
        base,
        paths("managed")?,
        paths("unbased")?,
    ))
}

/// `value` as an object. With `fields`, it must have no other keys.
fn object<'a>(
    value: &'a Value,
    context: &str,
    fields: &[&str],
) -> Result<&'a Map<String, Value>, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{context} is not a JSON object"))?;
    if !fields.is_empty() {
        if let Some(key) = object.keys().find(|key| !fields.contains(&key.as_str())) {
            return Err(format!("{context} has an unexpected `{key}`"));
        }
    }
    Ok(object)
}

/// Why the lockfile could not be read.
#[derive(Debug)]
pub enum LockfileError {
    /// It could not be read.
    Io {
        /// The lockfile.
        path: PathBuf,
        /// Why.
        error: io::Error,
    },
    /// It is a directory, a symbolic link, or another special file.
    NotAFile {
        /// The lockfile.
        path: PathBuf,
    },
    /// Its contents are not a lockfile this version of Assemblr reads.
    Invalid {
        /// The lockfile.
        path: PathBuf,
        /// What is wrong.
        detail: String,
    },
}

impl fmt::Display for LockfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, error } => write!(f, "could not read {}: {error}", path.display()),
            Self::NotAFile { path } => write!(
                f,
                "{} is not a regular file; Assemblr manages it as one",
                path.display()
            ),
            Self::Invalid { path, detail } => {
                write!(f, "{} is not a valid lockfile: {detail}", path.display())
            }
        }
    }
}

impl std::error::Error for LockfileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { error, .. } => Some(error),
            _ => None,
        }
    }
}

/// Why a published version's lockfile could not be read.
#[derive(Debug)]
pub enum PublishedLockfileError {
    /// The committed `assemblr-lock.json` could not be read.
    Read(ReadFileError),
    /// Its contents are not a lockfile this version of Assemblr reads.
    Invalid(String),
}

impl fmt::Display for PublishedLockfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // These two name the file themselves.
            Self::Read(
                error @ (ReadFileError::NotAFile { .. } | ReadFileError::TooLarge { .. }),
            ) => error.fmt(f),
            Self::Read(error) => write!(f, "could not read `{LOCKFILE}`: {error}"),
            Self::Invalid(detail) => write!(f, "`{LOCKFILE}` is not a valid lockfile: {detail}"),
        }
    }
}

impl std::error::Error for PublishedLockfileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read(error) => Some(error),
            Self::Invalid(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{parse, serialize};
    use crate::domain::{LockedUnit, ObjectId, Provenance, UnitLock};

    fn lock() -> UnitLock {
        let id = |digit: &str| ObjectId::parse(&digit.repeat(40)).unwrap();
        UnitLock::new(BTreeMap::from([
            (
                "@acme/web".parse().unwrap(),
                LockedUnit::new("2.0.0".parse().unwrap(), Provenance::new(id("3"), id("4"))),
            ),
            (
                "@acme/node".parse().unwrap(),
                LockedUnit::new(
                    "1.2.0-rc.1+build".parse().unwrap(),
                    Provenance::new(id("1"), id("2")),
                ),
            ),
        ]))
    }

    #[test]
    fn a_lock_round_trips_through_its_file() {
        assert_eq!(parse(&serialize(&lock()).unwrap()).unwrap().lock, lock());
    }

    #[test]
    fn the_file_is_sorted_indented_and_ends_with_a_newline() {
        let text = String::from_utf8(serialize(&lock()).unwrap()).unwrap();
        let ones = "1".repeat(40);
        let twos = "2".repeat(40);
        let threes = "3".repeat(40);
        let fours = "4".repeat(40);
        assert_eq!(
            text,
            format!(
                "{{\n  \"format\": 1,\n  \"units\": {{\n    \"@acme/node\": {{\n      \
                 \"commit\": \"{ones}\",\n      \"tree\": \"{twos}\",\n      \
                 \"version\": \"1.2.0-rc.1+build\"\n    }},\n    \"@acme/web\": {{\n      \
                 \"commit\": \"{threes}\",\n      \"tree\": \"{fours}\",\n      \
                 \"version\": \"2.0.0\"\n    }}\n  }}\n}}\n"
            )
        );
    }

    #[test]
    fn an_empty_lock_is_a_valid_file() {
        assert_eq!(
            parse(&serialize(&UnitLock::default()).unwrap())
                .unwrap()
                .lock,
            UnitLock::default()
        );
    }

    #[test]
    fn unexpected_content_is_rejected_with_its_reason() {
        let sha = "1".repeat(40);
        let unit = |extra: &str| {
            format!(
                r#"{{"format": 1, "units": {{"@acme/node": {{"version": "1.0.0", "commit": "{sha}", "tree": "{sha}"{extra}}}}}}}"#
            )
        };
        for (text, reason) in [
            ("[]".to_owned(), "is not a JSON object"),
            (r#"{"units": {}}"#.to_owned(), "no format number"),
            (r#"{"format": 2, "units": {}}"#.to_owned(), "format 2"),
            (r#"{"format": 1}"#.to_owned(), "no `units`"),
            (
                r#"{"format": 1, "units": {}, "x": 1}"#.to_owned(),
                "unexpected `x`",
            ),
            (r#"{"format": 1, "units": {"acme": {}}}"#.to_owned(), "acme"),
            (unit(r#", "source": "/x""#), "unexpected `source`"),
            (unit("").replace("1.0.0", "1.0"), "invalid `version`"),
            (
                unit("").replace(&format!(r#""tree": "{sha}""#), r#""tree": "x""#),
                "invalid `tree`",
            ),
            ("<<<<<<< HEAD".to_owned(), "expected value"),
        ] {
            let error = parse(text.as_bytes()).unwrap_err();
            assert!(error.contains(reason), "{text}: {error}");
        }
        assert!(parse(unit("").as_bytes()).is_ok());
    }

    #[test]
    fn every_promised_lockfile_format_is_read() {
        // Samples never change (ADR 0041): a reader that stops reading one
        // fails here.
        let unit = parse(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/formats/lockfile-1.json"
        )))
        .unwrap();
        assert_eq!(unit.lock.units().len(), 1);
        assert!(unit.project.is_none());
        let project = parse(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/formats/lockfile-1-project.json"
        )))
        .unwrap();
        assert_eq!(project.lock, unit.lock);
        assert_eq!(project.project.unwrap().managed().len(), 2);
    }
}
