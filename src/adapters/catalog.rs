//! Catalogs: `registry.jsonc`, which declares unit repositories, and the
//! metadata index Assemblr generates from them (see
//! `docs/domain/registries-and-discovery.md` and
//! `docs/adr/0030-catalog-index.md`).
//!
//! `catalog init` writes a new catalog's `registry.jsonc`. `catalog refresh`
//! reads every declared repository, validates its published versions as
//! registration does, and replaces `registry-index.json` only when every
//! source is valid. `catalog add` and `catalog remove` edit
//! `registry.jsonc` and refresh in one step, so that neither file is
//! written unless the other can be. All of them work in the catalog's
//! working tree; committing the two files is a separate step, which the
//! CLI takes unless told not to (see `docs/adr/0033-catalog-entry-commands.md`).

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::git::{
    GitError, GitRepository, HeadManifestError, MANIFEST_LIMIT, ReadFileError, init_repository,
};
use super::jsonc::{self, Json, SyntaxError};
use super::jsonc_edit::apply;
use super::manifest::read_manifest;
use super::registry::{
    Problem, ReferenceError, Registry, TagProblem, catalog_reference, observe_tags,
};
use crate::domain::{
    CatalogIdentity, Edit, Excluded, IdentityError, Manifest, ObjectId, ObservedTag, Provenance,
    RegisteredCatalog, RegistryIndex, SourceReference, UnitIdentity, Version, declared_identity,
};

/// The catalog's declaration, at its top level.
pub const CATALOG_MANIFEST: &str = "registry.jsonc";
/// The generated index, beside the declaration.
pub const CATALOG_INDEX: &str = "registry-index.json";
/// Where a new index is written before it replaces the old one.
const INDEX_NEW: &str = ".registry-index.json.new";
/// Where an edited declaration is written before it replaces the old one.
const MANIFEST_NEW: &str = ".registry.jsonc.new";
/// The member of `registry.jsonc` that declares the units.
const UNITS: &str = "units";
/// The version of the index's format.
const INDEX_FORMAT: u64 = 1;
/// The largest `registry.jsonc` read, in bytes.
const MANIFEST_SIZE_LIMIT: u64 = 1024 * 1024;

/// A catalog's declaration: its identity and the repository each unit is
/// expected in, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogManifest {
    name: CatalogIdentity,
    units: BTreeMap<UnitIdentity, String>,
}

impl CatalogManifest {
    /// The catalog's identity.
    pub fn name(&self) -> &CatalogIdentity {
        &self.name
    }

    /// Each declared unit and its repository reference, as written.
    pub fn units(&self) -> &BTreeMap<UnitIdentity, String> {
        &self.units
    }
}

/// Reads a catalog's `registry.jsonc`: an object with `name`, a catalog
/// identity, and `units`, a map from unit identities to repository
/// references as plain strings. `$schema` is allowed for editors; any other
/// member is an error.
pub fn read_catalog_manifest(text: &str) -> Result<CatalogManifest, CatalogManifestError> {
    let invalid = |detail: String| CatalogManifestError::Invalid(detail);
    let Json::Object(members) = jsonc::parse(text).map_err(CatalogManifestError::Syntax)? else {
        return Err(invalid("it is not a JSON object".to_owned()));
    };
    let mut name = None;
    let mut units = None;
    for (key, value) in members {
        let duplicate = match key.as_str() {
            "name" => name.is_some(),
            "units" => units.is_some(),
            _ => false,
        };
        if duplicate {
            return Err(invalid(format!("`{key}` appears more than once")));
        }
        match (key.as_str(), value) {
            ("name", Json::String(text)) => {
                name = Some(CatalogIdentity::parse(&text).map_err(CatalogManifestError::Name)?);
            }
            ("name", _) => return Err(invalid("`name` is not a string".to_owned())),
            ("units", Json::Object(entries)) => {
                let mut declared = BTreeMap::new();
                for (identity, reference) in entries {
                    let unit =
                        UnitIdentity::parse(&identity).map_err(CatalogManifestError::Unit)?;
                    let Json::String(reference) = reference else {
                        return Err(invalid(format!("the reference of {unit} is not a string")));
                    };
                    if declared.insert(unit.clone(), reference).is_some() {
                        return Err(invalid(format!("{unit} is declared more than once")));
                    }
                }
                units = Some(declared);
            }
            ("units", _) => return Err(invalid("`units` is not an object".to_owned())),
            ("$schema", Json::String(_)) => {}
            (other, _) => return Err(invalid(format!("`{other}` is not a catalog field"))),
        }
    }
    Ok(CatalogManifest {
        name: name.ok_or_else(|| invalid("it has no `name`".to_owned()))?,
        units: units.ok_or_else(|| invalid("it has no `units`".to_owned()))?,
    })
}

/// Why a `registry.jsonc` cannot be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogManifestError {
    /// It is not JSONC.
    Syntax(SyntaxError),
    /// `name` is not a valid catalog identity.
    Name(IdentityError),
    /// A key of `units` is not a valid unit identity.
    Unit(IdentityError),
    /// Its structure is wrong. Holds what is wrong.
    Invalid(String),
}

impl fmt::Display for CatalogManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(error) => error.fmt(f),
            Self::Name(error) => write!(f, "`name` is not a catalog identity: {error}"),
            Self::Unit(error) => write!(f, "a key of `units` is not a unit identity: {error}"),
            Self::Invalid(detail) => f.write_str(detail),
        }
    }
}

impl std::error::Error for CatalogManifestError {}

/// Creates the catalog `identity` in `directory`, or, without one, in a new
/// directory named after it, `web` for `@acme/web`. The directory must not
/// exist or be empty. It becomes a Git repository with an empty
/// `registry.jsonc`. Nothing is committed.
pub fn init_catalog(
    directory: Option<&Path>,
    identity: &CatalogIdentity,
) -> Result<PathBuf, CatalogInitFailure> {
    let directory = directory.map_or_else(|| PathBuf::from(identity.name()), Path::to_owned);
    match fs::read_dir(&directory) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                return Err(CatalogInitFailure::NotEmpty(directory));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(&directory).map_err(|error| CatalogInitFailure::Io {
                path: directory.clone(),
                error,
            })?;
        }
        Err(error) => {
            return Err(CatalogInitFailure::Io {
                path: directory,
                error,
            });
        }
    }
    init_repository(&directory).map_err(CatalogInitFailure::Git)?;
    let path = directory.join(CATALOG_MANIFEST);
    let text = format!("{{\n  \"name\": \"{identity}\",\n  \"units\": {{}}\n}}\n");
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .map_err(|error| CatalogInitFailure::Io { path, error })?;
    Ok(directory)
}

/// Why `catalog init` failed.
#[derive(Debug)]
pub enum CatalogInitFailure {
    /// The directory holds files already.
    NotEmpty(PathBuf),
    /// A file or directory could not be created.
    Io {
        /// Which.
        path: PathBuf,
        /// Why.
        error: io::Error,
    },
    /// The Git repository could not be created.
    Git(GitError),
}

impl fmt::Display for CatalogInitFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotEmpty(directory) => write!(f, "{} is not empty", directory.display()),
            Self::Io { path, error } => write!(f, "could not create {}: {error}", path.display()),
            Self::Git(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for CatalogInitFailure {}

/// The outcome of a refresh.
#[derive(Debug)]
pub struct Refreshed {
    catalog: CatalogIdentity,
    versions: BTreeMap<UnitIdentity, Vec<Version>>,
    excluded: Vec<Excluded<TagProblem>>,
    changed: bool,
}

impl Refreshed {
    /// The catalog's identity.
    pub fn catalog(&self) -> &CatalogIdentity {
        &self.catalog
    }

    /// Every declared unit and its indexed versions, in order.
    pub fn versions(&self) -> &BTreeMap<UnitIdentity, Vec<Version>> {
        &self.versions
    }

    /// The version tags excluded as invalid.
    pub fn excluded(&self) -> &[Excluded<TagProblem>] {
        &self.excluded
    }

    /// Whether `registry-index.json` was written: `false` when it already
    /// held this index.
    pub fn changed(&self) -> bool {
        self.changed
    }
}

/// Regenerates the index of the catalog whose top level is `root`, from
/// its `registry.jsonc` as it is in the working tree.
///
/// Every declared repository is read, a remote one after it is fetched
/// into `registry`'s copy, and its published versions are validated as
/// registration validates them. An invalid version is excluded and
/// reported. A repository that cannot be read, a unit whose manifests
/// declare another identity, and any other source-level problem fail the
/// refresh, every problem is reported, and the index is left as it was.
pub fn refresh_catalog(root: &Path, registry: &Registry) -> Result<Refreshed, RefreshFailure> {
    let (_, manifest) = read_declaration(root)?;
    let (text, mut refreshed) = build_index(root, &manifest, registry)?;
    refreshed.changed = write_if_changed(root, CATALOG_INDEX, INDEX_NEW, &text)?;
    Ok(refreshed)
}

/// Reads `registry.jsonc` from the working tree at `root`: its text and
/// what it declares.
fn read_declaration(root: &Path) -> Result<(String, CatalogManifest), RefreshFailure> {
    let manifest_path = root.join(CATALOG_MANIFEST);
    let text = read_limited(&manifest_path)?;
    let manifest = read_catalog_manifest(&text).map_err(|error| RefreshFailure::Manifest {
        path: manifest_path,
        error,
    })?;
    Ok((text, manifest))
}

/// A unit repository as a catalog reads it: fetched when remote, its
/// version tags observed, and the identity they, or the manifest at its
/// `HEAD` when no tag is valid, declare.
struct ReadUnit {
    identity: UnitIdentity,
    repository: GitRepository,
    tags: Vec<ObservedTag<TagProblem>>,
}

/// Reads the unit repository `source`, or returns every problem that keeps
/// it from being one.
fn read_unit(registry: &Registry, source: &SourceReference) -> Result<ReadUnit, Vec<Problem>> {
    let unreadable = |error| Problem::Unreadable {
        source: source.clone(),
        error,
    };
    if let Err(error) = registry.fetch_source(source) {
        return Err(vec![Problem::Unfetchable {
            source: source.clone(),
            error,
        }]);
    }
    let repository = registry
        .open_source(source)
        .map_err(|error| vec![unreadable(error)])?;
    let tags = observe_tags(&repository, None).map_err(|error| vec![unreadable(error)])?;
    let identity = match declared_identity(source, &tags) {
        Ok(Some(found)) => found,
        Ok(None) => match repository.head_manifest() {
            Ok(manifest) => manifest.name().clone(),
            Err(HeadManifestError::Git(error)) => return Err(vec![unreadable(error)]),
            Err(error) => {
                return Err(vec![Problem::NoIdentity {
                    source: source.clone(),
                    error,
                }]);
            }
        },
        Err(errors) => return Err(errors.into_iter().map(Problem::Invalid).collect()),
    };
    Ok(ReadUnit {
        identity,
        repository,
        tags,
    })
}

/// The index `manifest` describes, as the text `registry-index.json` holds,
/// with what it lists. Nothing is written; `changed` is left `false`.
fn build_index(
    root: &Path,
    manifest: &CatalogManifest,
    registry: &Registry,
) -> Result<(String, Refreshed), RefreshFailure> {
    let mut problems = Vec::new();
    let mut index = RegistryIndex::new();
    let mut entries = BTreeMap::new();
    let mut excluded = Vec::new();
    for (identity, declared) in manifest.units() {
        let unit_problem = |problem| RefreshProblem::Unit {
            unit: identity.clone(),
            problem,
        };
        let source = match catalog_reference(declared) {
            Ok(source) => source,
            Err(error) => {
                problems.push(RefreshProblem::Reference {
                    unit: identity.clone(),
                    error,
                });
                continue;
            }
        };
        let read = match read_unit(registry, &source) {
            Ok(read) => read,
            Err(found) => {
                problems.extend(found.into_iter().map(unit_problem));
                continue;
            }
        };
        if &read.identity != identity {
            problems.push(RefreshProblem::IdentityMismatch {
                unit: identity.clone(),
                found: read.identity,
                source,
            });
            continue;
        }
        match index.add(source.clone(), identity.clone(), read.tags) {
            Ok(change) => {
                let (next, mut dropped) = change.into_parts();
                index = next;
                excluded.append(&mut dropped);
                entries.insert(identity.clone(), (declared.clone(), read.repository));
            }
            Err(errors) => problems.extend(
                errors
                    .into_iter()
                    .map(|error| unit_problem(Problem::Invalid(error))),
            ),
        }
    }
    if !problems.is_empty() {
        return Err(RefreshFailure::Rejected(problems));
    }
    let mut units = Map::new();
    let mut versions = BTreeMap::new();
    for (identity, unit) in index.units() {
        let Some((declared, repository)) = entries.get(identity) else {
            continue;
        };
        let mut indexed = Map::new();
        for (version, provenance) in unit.versions() {
            let manifest = published_manifest(repository, provenance).map_err(|error| {
                RefreshFailure::Rejected(vec![RefreshProblem::Unit {
                    unit: identity.clone(),
                    problem: Problem::Unreadable {
                        source: unit.source().clone(),
                        error,
                    },
                }])
            })?;
            indexed.insert(
                version.to_string(),
                json!({
                    "commit": provenance.commit().as_str(),
                    "tree": provenance.tree().as_str(),
                    "manifest": manifest,
                }),
            );
        }
        versions.insert(identity.clone(), unit.versions().keys().cloned().collect());
        units.insert(
            identity.to_string(),
            json!({ "source": declared, "versions": indexed }),
        );
    }
    let document = json!({
        "format": INDEX_FORMAT,
        "name": manifest.name().to_string(),
        "units": units,
    });
    let mut text =
        serde_json::to_string_pretty(&document).map_err(|error| RefreshFailure::Write {
            path: root.join(CATALOG_INDEX),
            error: io::Error::other(error),
        })?;
    text.push('\n');
    Ok((
        text,
        Refreshed {
            catalog: manifest.name().clone(),
            versions,
            excluded,
            changed: false,
        },
    ))
}

/// What `catalog add` did.
#[derive(Debug)]
pub enum CatalogAdded {
    /// The unit was declared from the reference, and the index refreshed.
    Added {
        /// The unit, as its repository declares it.
        unit: UnitIdentity,
        /// The refreshed index.
        refreshed: Refreshed,
    },
    /// The catalog declared the unit from this reference already. Nothing
    /// was written.
    AlreadyDeclared {
        /// The unit.
        unit: UnitIdentity,
    },
}

/// Declares the unit repository `reference` in the catalog whose top level
/// is `root`, and refreshes its index.
///
/// The unit's identity is read from the repository, as registration reads
/// it, so it is never typed twice. `reference` must be one a catalog may
/// declare, a remote one. The entry is added to `registry.jsonc` in the
/// position its identity sorts to, keeping the file's comments and layout,
/// and the whole index is then refreshed as [`refresh_catalog`] refreshes
/// it. Neither file is written unless the refresh succeeds.
pub fn add_to_catalog(
    root: &Path,
    reference: &str,
    registry: &Registry,
) -> Result<CatalogAdded, CatalogChangeFailure> {
    let (text, manifest) = read_declaration(root)?;
    let source = catalog_reference(reference).map_err(CatalogChangeFailure::Reference)?;
    let read =
        read_unit(registry, &source).map_err(|problems| CatalogChangeFailure::Unreadable {
            source: source.clone(),
            problems,
        })?;
    let unit = read.identity;
    if let Some(declared) = manifest.units().get(&unit) {
        if declared == reference {
            return Ok(CatalogAdded::AlreadyDeclared { unit });
        }
        return Err(CatalogChangeFailure::DeclaredElsewhere {
            unit,
            declared: declared.clone(),
        });
    }
    let after = manifest
        .units()
        .range(..unit.clone())
        .next_back()
        .map(|(identity, _)| identity.to_string());
    let edit = Edit::Insert {
        parent: vec![UNITS.to_owned()],
        key: unit.to_string(),
        value: crate::domain::Value::String(reference.to_owned()),
        after,
    };
    let mut expected = manifest.units().clone();
    expected.insert(unit.clone(), reference.to_owned());
    let edited = edit_declaration(&text, &edit, &expected)?;
    let refreshed = write_change(root, &edited, registry)?;
    Ok(CatalogAdded::Added { unit, refreshed })
}

/// Removes the unit `unit` from the catalog whose top level is `root`, and
/// refreshes its index, as [`add_to_catalog`] adds one. Neither file is
/// written unless the refresh succeeds. Returns the reference the unit was
/// declared from and the refreshed index.
pub fn remove_from_catalog(
    root: &Path,
    unit: &UnitIdentity,
    registry: &Registry,
) -> Result<(String, Refreshed), CatalogChangeFailure> {
    let (text, manifest) = read_declaration(root)?;
    let Some(declared) = manifest.units().get(unit).cloned() else {
        return Err(CatalogChangeFailure::NotDeclared {
            unit: unit.clone(),
            catalog: manifest.name().clone(),
        });
    };
    let edit = Edit::Delete {
        path: vec![UNITS.to_owned(), unit.to_string()],
    };
    let mut expected = manifest.units().clone();
    expected.remove(unit);
    let edited = edit_declaration(&text, &edit, &expected)?;
    let refreshed = write_change(root, &edited, registry)?;
    Ok((declared, refreshed))
}

/// `text` with `edit` applied, checked by reading it back: it must declare
/// exactly `expected`, so a layout the editor misjudges is a refusal, never
/// a different catalog.
fn edit_declaration(
    text: &str,
    edit: &Edit,
    expected: &BTreeMap<UnitIdentity, String>,
) -> Result<String, CatalogChangeFailure> {
    let cannot = |detail: String| CatalogChangeFailure::Edit(detail);
    let edited =
        apply(text, std::slice::from_ref(edit)).map_err(|error| cannot(error.to_string()))?;
    let read = read_catalog_manifest(&edited).map_err(|error| cannot(error.to_string()))?;
    if read.units() != expected {
        return Err(cannot(format!(
            "the edited {CATALOG_MANIFEST} does not declare what was intended"
        )));
    }
    Ok(edited)
}

/// Refreshes the index for the declaration `edited`, then writes the index
/// and the declaration, the declaration last.
fn write_change(
    root: &Path,
    edited: &str,
    registry: &Registry,
) -> Result<Refreshed, CatalogChangeFailure> {
    let manifest = read_catalog_manifest(edited)
        .map_err(|error| CatalogChangeFailure::Edit(error.to_string()))?;
    let (index, mut refreshed) = build_index(root, &manifest, registry)?;
    refreshed.changed = write_if_changed(root, CATALOG_INDEX, INDEX_NEW, &index)?;
    write_if_changed(root, CATALOG_MANIFEST, MANIFEST_NEW, edited).map_err(|failure| {
        CatalogChangeFailure::Declaration {
            failure,
            index_written: refreshed.changed,
        }
    })?;
    Ok(refreshed)
}

/// Why `catalog add` or `catalog remove` changed nothing, or, for
/// [`Declaration`](Self::Declaration), changed only the index.
#[derive(Debug)]
pub enum CatalogChangeFailure {
    /// The catalog could not be read, or the refresh failed. Nothing was
    /// written.
    Refresh(RefreshFailure),
    /// The reference is not one a catalog may declare.
    Reference(ReferenceError),
    /// The repository could not be read as a unit's.
    Unreadable {
        /// The repository.
        source: SourceReference,
        /// Every problem found.
        problems: Vec<Problem>,
    },
    /// The catalog declares the unit from another reference.
    DeclaredElsewhere {
        /// The unit.
        unit: UnitIdentity,
        /// The reference it declares.
        declared: String,
    },
    /// The catalog does not declare the unit.
    NotDeclared {
        /// The unit.
        unit: UnitIdentity,
        /// The catalog.
        catalog: CatalogIdentity,
    },
    /// `registry.jsonc` could not be edited safely.
    Edit(String),
    /// `registry.jsonc` could not be written, after the index was, when
    /// `index_written`.
    Declaration {
        /// Why.
        failure: RefreshFailure,
        /// Whether `registry-index.json` was replaced already.
        index_written: bool,
    },
}

impl From<RefreshFailure> for CatalogChangeFailure {
    fn from(failure: RefreshFailure) -> Self {
        Self::Refresh(failure)
    }
}

impl fmt::Display for CatalogChangeFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refresh(failure) => failure.fmt(f),
            Self::Reference(error) => error.fmt(f),
            Self::Unreadable { source, problems } => {
                write!(f, "{source} cannot be added:")?;
                for problem in problems {
                    write!(f, "\n{problem}")?;
                }
                Ok(())
            }
            Self::DeclaredElsewhere { unit, declared } => write!(
                f,
                "the catalog declares {unit} from {declared} already; a unit has one repository"
            ),
            Self::NotDeclared { unit, catalog } => {
                write!(f, "the catalog {catalog} does not declare {unit}")
            }
            Self::Edit(detail) => write!(f, "could not edit {CATALOG_MANIFEST}: {detail}"),
            Self::Declaration { failure, .. } => failure.fmt(f),
        }
    }
}

impl std::error::Error for CatalogChangeFailure {}

/// The manifest published at `provenance`'s commit, as JSON: comments and
/// trailing commas removed, and members in sorted order, so the index does
/// not depend on how the author laid the file out.
fn published_manifest(
    repository: &GitRepository,
    provenance: &Provenance,
) -> Result<Value, GitError> {
    let unexpected = |detail: String| {
        GitError::not_a_repository(
            repository.path().to_owned(),
            format!(
                "the manifest of commit {} cannot be read again: {detail}",
                provenance.commit()
            ),
        )
    };
    let bytes =
        match repository.read_top_level_file(provenance.commit(), "manifest.jsonc", MANIFEST_LIMIT)
        {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Err(unexpected("it has no manifest.jsonc".to_owned())),
            Err(ReadFileError::Git(error)) => return Err(error),
            Err(error) => return Err(unexpected(error.to_string())),
        };
    let text = String::from_utf8(bytes).map_err(|error| unexpected(error.to_string()))?;
    let strict = jsonc::to_strict_json(&text).map_err(|error| unexpected(error.to_string()))?;
    serde_json::from_str(&strict).map_err(|error| unexpected(error.to_string()))
}

fn read_limited(path: &Path) -> Result<String, RefreshFailure> {
    let read = |error| RefreshFailure::Read {
        path: path.to_owned(),
        error,
    };
    let file = fs::File::open(path).map_err(read)?;
    let mut text = String::new();
    file.take(MANIFEST_SIZE_LIMIT + 1)
        .read_to_string(&mut text)
        .map_err(read)?;
    if text.len() as u64 > MANIFEST_SIZE_LIMIT {
        return Err(read(io::Error::other(format!(
            "it is larger than {MANIFEST_SIZE_LIMIT} bytes"
        ))));
    }
    Ok(text)
}

/// Replaces the file `name` at `root` with `text` unless it holds it
/// already: written beside it as `staging`, flushed, and renamed over it.
fn write_if_changed(
    root: &Path,
    name: &str,
    staging: &str,
    text: &str,
) -> Result<bool, RefreshFailure> {
    let path = root.join(name);
    match fs::read(&path) {
        Ok(current) if current == text.as_bytes() => return Ok(false),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(RefreshFailure::Read { path, error }),
    }
    let staging = root.join(staging);
    let write = |error| RefreshFailure::Write {
        path: path.clone(),
        error,
    };
    let result = fs::File::create(&staging)
        .and_then(|mut file| {
            file.write_all(text.as_bytes())?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&staging, &path));
    if let Err(error) = result {
        let _ = fs::remove_file(&staging);
        return Err(write(error));
    }
    Ok(true)
}

/// Why a refresh failed. The index is unchanged.
#[derive(Debug)]
pub enum RefreshFailure {
    /// `registry.jsonc` or the index could not be read.
    Read {
        /// Which file.
        path: PathBuf,
        /// Why.
        error: io::Error,
    },
    /// `registry.jsonc` is not a valid catalog declaration.
    Manifest {
        /// Its path.
        path: PathBuf,
        /// What is wrong.
        error: CatalogManifestError,
    },
    /// Declared repositories are invalid. Every problem found is listed.
    Rejected(Vec<RefreshProblem>),
    /// The index could not be written.
    Write {
        /// Its path.
        path: PathBuf,
        /// Why.
        error: io::Error,
    },
}

impl fmt::Display for RefreshFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, error } => write!(f, "could not read {}: {error}", path.display()),
            Self::Manifest { path, error } => write!(f, "{} is invalid: {error}", path.display()),
            Self::Rejected(problems) => {
                for (index, problem) in problems.iter().enumerate() {
                    if index > 0 {
                        f.write_str("\n")?;
                    }
                    problem.fmt(f)?;
                }
                Ok(())
            }
            Self::Write { path, error } => {
                write!(f, "could not write {}: {error}", path.display())
            }
        }
    }
}

impl std::error::Error for RefreshFailure {}

/// A problem with one declared unit.
#[derive(Debug)]
pub enum RefreshProblem {
    /// Its reference cannot be a repository reference.
    Reference {
        /// The unit.
        unit: UnitIdentity,
        /// Why.
        error: ReferenceError,
    },
    /// Its repository's manifests declare another identity.
    IdentityMismatch {
        /// The identity the catalog declares.
        unit: UnitIdentity,
        /// The identity the repository declares.
        found: UnitIdentity,
        /// The repository.
        source: SourceReference,
    },
    /// Its repository failed validation as a registered source would.
    Unit {
        /// The unit.
        unit: UnitIdentity,
        /// What failed.
        problem: Problem,
    },
}

impl fmt::Display for RefreshProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reference { unit, error } => write!(f, "{unit}: {error}"),
            Self::IdentityMismatch {
                unit,
                found,
                source,
            } => write!(
                f,
                "{unit}: the catalog declares {source} for it, but that repository is {found}"
            ),
            Self::Unit { unit, problem } => write!(f, "{unit}: {problem}"),
        }
    }
}

/// The largest catalog index read from a catalog's history, in bytes.
const INDEX_SIZE_LIMIT: u64 = 16 * 1024 * 1024;

/// Whether the repository's `HEAD` is a catalog: its top level has a
/// `registry.jsonc` and no `manifest.jsonc`. A repository with no commit is
/// not one. One with both files is refused, since it is not clear which it
/// is.
pub(crate) fn is_catalog(repository: &GitRepository) -> Result<bool, CatalogProblem> {
    let Some(head) = repository
        .head_commit()
        .map_err(CatalogProblem::Unreadable)?
    else {
        return Ok(false);
    };
    let has = |name: &str| match repository.read_top_level_file(&head, name, 0) {
        Ok(None) => Ok(false),
        Ok(Some(_)) | Err(ReadFileError::TooLarge { .. } | ReadFileError::NotAFile { .. }) => {
            Ok(true)
        }
        Err(ReadFileError::Git(error)) => Err(CatalogProblem::Unreadable(error)),
        Err(error) => Err(CatalogProblem::Unreadable(GitError::not_a_repository(
            repository.path().to_owned(),
            error.to_string(),
        ))),
    };
    match (has(CATALOG_MANIFEST)?, has("manifest.jsonc")?) {
        (true, true) => Err(CatalogProblem::Ambiguous),
        (catalog, _) => Ok(catalog),
    }
}

/// A catalog as committed at `HEAD`: its declaration and its generated
/// index, checked against each other.
#[derive(Debug, Clone)]
pub(crate) struct PublishedCatalog {
    identity: CatalogIdentity,
    commit: ObjectId,
    units: BTreeMap<UnitIdentity, ListedUnit>,
}

impl PublishedCatalog {
    pub(crate) fn identity(&self) -> &CatalogIdentity {
        &self.identity
    }

    pub(crate) fn units(&self) -> &BTreeMap<UnitIdentity, ListedUnit> {
        &self.units
    }

    /// The catalog as the registry records it, registered from `source`.
    pub(crate) fn registered(&self, source: SourceReference) -> RegisteredCatalog {
        RegisteredCatalog::new(
            source,
            self.commit.clone(),
            self.units
                .iter()
                .map(|(identity, unit)| (identity.clone(), unit.source.clone()))
                .collect(),
        )
    }
}

/// A unit a published catalog declares: its repository, resolved, and the
/// versions the index lists, each with its provenance and manifest.
#[derive(Debug, Clone)]
pub(crate) struct ListedUnit {
    source: SourceReference,
    versions: BTreeMap<Version, (Provenance, Manifest)>,
}

/// The versions a catalog index lists for a unit, each with its provenance
/// and manifest, as read.
type ListedVersions = BTreeMap<Version, (Provenance, Value)>;

impl ListedUnit {
    pub(crate) fn source(&self) -> &SourceReference {
        &self.source
    }

    /// The listed versions and their provenance.
    pub(crate) fn provenances(&self) -> BTreeMap<Version, Provenance> {
        self.versions
            .iter()
            .map(|(version, (provenance, _))| (version.clone(), provenance.clone()))
            .collect()
    }

    /// The manifest the index lists for `version`.
    pub(crate) fn manifest(&self, version: &Version) -> Option<&Manifest> {
        self.versions.get(version).map(|(_, manifest)| manifest)
    }
}

/// Reads the catalog committed at `HEAD` of `repository`.
///
/// Both `registry.jsonc` and `registry-index.json` are read from the
/// commit, never from a working tree. The index must be the one
/// `catalog refresh` writes for that declaration: the same catalog, the
/// same units, and the same references, and every listed manifest a valid
/// one of the unit and version it is listed for. The units' repositories
/// are not read.
pub(crate) fn read_published_catalog(
    repository: &GitRepository,
) -> Result<PublishedCatalog, CatalogProblem> {
    let commit = repository
        .head_commit()
        .map_err(CatalogProblem::Unreadable)?
        .ok_or(CatalogProblem::NoCommit)?;
    read_catalog_at(repository, commit)
}

/// Reads the catalog at `commit`, as [`read_published_catalog`] does.
pub(crate) fn read_catalog_at(
    repository: &GitRepository,
    commit: ObjectId,
) -> Result<PublishedCatalog, CatalogProblem> {
    let read = |name: &str, limit| -> Result<Option<String>, CatalogProblem> {
        let unreadable = |detail: String| {
            CatalogProblem::Unreadable(GitError::not_a_repository(
                repository.path().to_owned(),
                format!("its {name} cannot be read: {detail}"),
            ))
        };
        match repository.read_top_level_file(&commit, name, limit) {
            Ok(Some(bytes)) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|error| unreadable(error.to_string())),
            Ok(None) => Ok(None),
            Err(ReadFileError::Git(error)) => Err(CatalogProblem::Unreadable(error)),
            Err(error) => Err(unreadable(error.to_string())),
        }
    };
    let declaration =
        read(CATALOG_MANIFEST, MANIFEST_SIZE_LIMIT)?.ok_or(CatalogProblem::NoCommit)?;
    let manifest = read_catalog_manifest(&declaration).map_err(CatalogProblem::Declaration)?;
    let index = read(CATALOG_INDEX, INDEX_SIZE_LIMIT)?.ok_or(CatalogProblem::NoIndex)?;
    let index: Value = serde_json::from_str(&index)
        .map_err(|error| CatalogProblem::InvalidIndex(error.to_string()))?;
    let listed = parse_index(&index).map_err(CatalogProblem::InvalidIndex)?;
    let stale = |detail: String| CatalogProblem::StaleIndex(detail);
    if listed.name != manifest.name().to_string() {
        return Err(stale(format!(
            "it is the index of {}, not of {}",
            listed.name,
            manifest.name()
        )));
    }
    let mut units = BTreeMap::new();
    for (identity, declared) in manifest.units() {
        let Some((indexed, versions)) = listed.units.get(identity) else {
            return Err(stale(format!("it does not list {identity}")));
        };
        if indexed != declared {
            return Err(stale(format!(
                "it lists {identity} from {indexed}, but registry.jsonc declares {declared}"
            )));
        }
        let reference = catalog_reference(declared).map_err(|error| CatalogProblem::Reference {
            unit: identity.clone(),
            error,
        })?;
        let mut checked = BTreeMap::new();
        for (version, (provenance, listed)) in versions {
            let invalid = |detail: String| {
                CatalogProblem::InvalidIndex(format!(
                    "the manifest it lists for {identity} {version} {detail}"
                ))
            };
            let manifest = read_manifest(&listed.to_string())
                .map_err(|error| invalid(format!("is invalid: {error}")))?;
            if manifest.name() != identity {
                return Err(invalid(format!("declares {}", manifest.name())));
            }
            if manifest.version() != version {
                return Err(invalid(format!("declares version {}", manifest.version())));
            }
            checked.insert(version.clone(), (provenance.clone(), manifest));
        }
        units.insert(
            identity.clone(),
            ListedUnit {
                source: reference,
                versions: checked,
            },
        );
    }
    if let Some(extra) = listed
        .units
        .keys()
        .find(|identity| !manifest.units().contains_key(*identity))
    {
        return Err(stale(format!(
            "it lists {extra}, which registry.jsonc does not declare"
        )));
    }
    Ok(PublishedCatalog {
        identity: manifest.name().clone(),
        commit,
        units,
    })
}

/// A catalog index as read, before it is checked against the declaration.
struct ListedIndex {
    name: String,
    units: BTreeMap<UnitIdentity, (String, ListedVersions)>,
}

/// Reads a catalog index, or describes what is wrong with it.
fn parse_index(value: &Value) -> Result<ListedIndex, String> {
    let object = |value: &'_ Value, context: &str| -> Result<Map<String, Value>, String> {
        value
            .as_object()
            .cloned()
            .ok_or_else(|| format!("{context} is not a JSON object"))
    };
    let root = object(value, "the index")?;
    match root.get("format").and_then(Value::as_u64) {
        Some(INDEX_FORMAT) => {}
        Some(other) => {
            return Err(format!(
                "it has format {other}, which this version of Assemblr does not read"
            ));
        }
        None => return Err("it has no format number".to_owned()),
    }
    let name = root
        .get("name")
        .and_then(Value::as_str)
        .ok_or("it has no `name`")?
        .to_owned();
    let mut units = BTreeMap::new();
    for (key, unit) in object(root.get("units").ok_or("it has no `units`")?, "`units`")? {
        let identity = UnitIdentity::parse(&key).map_err(|error| error.to_string())?;
        let context = format!("{identity}");
        let unit = object(&unit, &context)?;
        let source = unit
            .get("source")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{context} has no `source`"))?
            .to_owned();
        let mut versions = BTreeMap::new();
        let listed = unit
            .get("versions")
            .ok_or_else(|| format!("{context} has no `versions`"))?;
        for (key, entry) in object(listed, &context)? {
            let version = Version::parse(&key).map_err(|error| error.to_string())?;
            let context = format!("{identity} {version}");
            let entry = object(&entry, &context)?;
            let id = |name: &str| {
                entry
                    .get(name)
                    .and_then(Value::as_str)
                    .and_then(ObjectId::parse)
                    .ok_or_else(|| format!("{context} has an invalid `{name}`"))
            };
            let provenance = Provenance::new(id("commit")?, id("tree")?);
            let manifest = entry
                .get("manifest")
                .filter(|manifest| manifest.is_object())
                .ok_or_else(|| format!("{context} has no `manifest` object"))?
                .clone();
            versions.insert(version, (provenance, manifest));
        }
        units.insert(identity, (source, versions));
    }
    Ok(ListedIndex { name, units })
}

/// Why a catalog cannot be registered or synchronized.
#[derive(Debug)]
pub enum CatalogProblem {
    /// The repository could not be read.
    Unreadable(GitError),
    /// The repository has both a `registry.jsonc` and a `manifest.jsonc`.
    Ambiguous,
    /// `HEAD` has no commit with a `registry.jsonc`.
    NoCommit,
    /// The committed `registry.jsonc` is invalid.
    Declaration(CatalogManifestError),
    /// No `registry-index.json` is committed.
    NoIndex,
    /// The committed index cannot be read as one.
    InvalidIndex(String),
    /// The committed index is not the one for the committed declaration.
    StaleIndex(String),
    /// A declared reference cannot be a repository reference.
    Reference {
        /// The unit.
        unit: UnitIdentity,
        /// Why.
        error: ReferenceError,
    },
}

impl CatalogProblem {
    /// Whether running `catalog refresh` in the catalog, and committing and
    /// pushing the result, is what fixes it.
    pub fn needs_refresh(&self) -> bool {
        matches!(self, Self::NoIndex | Self::StaleIndex(_))
    }
}

impl fmt::Display for CatalogProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable(error) => error.fmt(f),
            Self::Ambiguous => write!(
                f,
                "it has both a {CATALOG_MANIFEST} and a manifest.jsonc, so it is not clear \
                 whether it is a catalog or a unit"
            ),
            Self::NoCommit => write!(f, "it has no committed {CATALOG_MANIFEST}"),
            Self::Declaration(error) => write!(f, "its {CATALOG_MANIFEST} is invalid: {error}"),
            Self::NoIndex => write!(f, "it has no committed {CATALOG_INDEX}"),
            Self::InvalidIndex(detail) => write!(f, "its {CATALOG_INDEX} is invalid: {detail}"),
            Self::StaleIndex(detail) => write!(
                f,
                "its {CATALOG_INDEX} does not match its {CATALOG_MANIFEST}: {detail}"
            ),
            Self::Reference { unit, error } => write!(f, "{unit}: {error}"),
        }
    }
}

impl std::error::Error for CatalogProblem {}

#[cfg(test)]
mod tests {
    use super::{CatalogManifestError, read_catalog_manifest};

    #[test]
    fn a_catalog_declares_a_name_and_units() {
        let manifest = read_catalog_manifest(
            r#"{
  // The catalog of web units.
  "$schema": "./registry.schema.json",
  "name": "@metamao/web",
  "units": {
    "@metamao/worker": "git@github.com:metamao/worker.git",
    "@metamao/node": "../node",
  },
}"#,
        )
        .unwrap();
        assert_eq!(manifest.name().to_string(), "@metamao/web");
        let units: Vec<(String, &str)> = manifest
            .units()
            .iter()
            .map(|(unit, reference)| (unit.to_string(), reference.as_str()))
            .collect();
        assert_eq!(
            units,
            [
                ("@metamao/node".to_owned(), "../node"),
                (
                    "@metamao/worker".to_owned(),
                    "git@github.com:metamao/worker.git"
                ),
            ]
        );
    }

    #[test]
    fn invalid_declarations_are_rejected_with_a_reason() {
        for (text, expected) in [
            ("[]", "not a JSON object"),
            (r#"{"units": {}}"#, "no `name`"),
            (r#"{"name": "@a/b"}"#, "no `units`"),
            (r#"{"name": "web", "units": {}}"#, "catalog identity"),
            (
                r#"{"name": "@a/b", "units": {"node": "../node"}}"#,
                "unit identity",
            ),
            (
                r#"{"name": "@a/b", "units": {"@a/node": {"git": "../node"}}}"#,
                "not a string",
            ),
            (
                r#"{"name": "@a/b", "units": {"@a/node": "x", "@a/node": "y"}}"#,
                "more than once",
            ),
            (
                r#"{"name": "@a/b", "name": "@a/c", "units": {}}"#,
                "more than once",
            ),
            (
                r#"{"name": "@a/b", "units": {}, "version": "1.0.0"}"#,
                "not a catalog field",
            ),
        ] {
            let error = read_catalog_manifest(text).unwrap_err();
            assert!(error.to_string().contains(expected), "{text}: {error}");
        }
        assert!(matches!(
            read_catalog_manifest("{"),
            Err(CatalogManifestError::Syntax(_))
        ));
    }
}
