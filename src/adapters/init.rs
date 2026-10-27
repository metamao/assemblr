//! `unit init`: a unit scaffold on a Git repository (see
//! `docs/domain/authoring-workflow.md` and `docs/adr/0014-unit-init.md`).
//!
//! The scaffold is the directory's Git repository, `manifest.jsonc`,
//! `manifest.schema.json`, `.gitignore` with `/build/`, and a `README.md`
//! and an `AGENTS.md` for the author to fill in, and an empty `src/` for
//! the unit's files. Nothing else is created: no `amendments/`, no file in
//! `src/` (anything there is delivered to consumers), no commit. Everything
//! is decided
//! before anything is written: the directory's place in Git is discovered,
//! and each file is read, so that an existing manifest is never overwritten
//! and running the command again only completes what is missing. A manifest
//! of another identity, a directory inside another unit's repository, and a
//! file that is not what the scaffold would write there refuse the command
//! before it changes anything. A directory inside a repository that is not a
//! unit's, such as a home directory kept in Git, gets its own nested
//! repository, as `git init` would give it.
//!
//! In a catalog, a unit is a directory of the catalog's `units/`, in the
//! catalog's repository (see `docs/domain/units.md`). There the scaffold is
//! the same except for the repository, which is the catalog's, and the
//! manifest, which declares no version: it is the catalog's. The unit is
//! named after its directory, with the catalog's owner. Without a
//! directory, `unit init` at a catalog's top level puts the unit in
//! `units/<name>`.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use std::collections::BTreeSet;

use super::git::{CATALOG, Discovered, GitError, UNITS, discover_repository, init_repository};
use super::manifest::{
    InCatalog, ReadManifestError, read_catalog, read_catalog_manifest, read_manifest,
};
use super::scaffold::{unit_agents, unit_readme};
use super::schema::{MANIFEST_SCHEMA, MANIFEST_SCHEMA_REFERENCE, manifest_schema};
use crate::domain::{Catalog, Description, IdentityError, RelativePath, UnitIdentity, Version};

/// The unit's manifest, at the repository's top level.
const MANIFEST: &str = "manifest.jsonc";
/// Git's ignore file, at the repository's top level.
const GITIGNORE: &str = ".gitignore";
/// The unit's description for people, at the repository's top level.
const README: &str = "README.md";
/// The unit's instructions for coding agents, at the repository's top level.
const AGENTS: &str = "AGENTS.md";
/// The unit's files, at the repository's top level.
const SRC: &str = "src";
/// The line that keeps `build/` out of Git.
const IGNORE_BUILD: &str = "/build/";
/// The version a new unit starts at.
const FIRST_VERSION: &str = "0.1.0";
/// The largest `.gitignore` or schema file read, in bytes.
const FILE_LIMIT: u64 = 1024 * 1024;
/// Where a replaced file is written before the rename, beside it. The
/// names are reserved.
const SCHEMA_NEW: &str = ".assemblr-manifest.schema.json.new";
const GITIGNORE_NEW: &str = ".assemblr-gitignore.new";

/// Initializes the unit `identity` in `directory`, which is created when it
/// does not exist. Without a directory, the unit gets a new one named after
/// it in the current directory, `web` for `@acme/web`: that directory must
/// not exist, be empty, or already hold a manifest, so that a unit never
/// lands among unrelated files by default. `description` is the manifest's
/// description; without one, `The <identity> unit.`
pub fn init_unit(
    directory: Option<&Path>,
    identity: &UnitIdentity,
    description: Option<&str>,
) -> Result<Initialized, InitFailure> {
    let derived;
    let directory = match directory {
        Some(directory) => directory,
        None => {
            // At a catalog's top level, a new unit goes in its `units/`.
            let parent = if Path::new(CATALOG).exists() {
                Path::new(UNITS)
            } else {
                Path::new("")
            };
            derived = new_unit_directory(parent, identity)?;
            &derived
        }
    };
    let description = match description {
        Some(text) => Description::new(text).map_err(|_| InitFailure::BlankDescription)?,
        None => Description::new(&format!("The {identity} unit."))
            .map_err(|_| InitFailure::BlankDescription)?,
    };
    let directory = std::path::absolute(directory).map_err(|error| InitFailure::Directory {
        path: directory.to_owned(),
        error,
    })?;
    if fs::symlink_metadata(directory.join(CATALOG)).is_ok() {
        return Err(InitFailure::CatalogTopLevel(directory));
    }
    let catalog = in_catalog(&directory, identity)?;
    let exists = match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.is_dir() => true,
        Ok(_) => return Err(InitFailure::NotADirectory(directory)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(InitFailure::Directory {
                path: directory,
                error,
            });
        }
    };

    // Where the directory stands in Git. A directory that does not exist
    // yet is looked at from its nearest existing parent.
    let probe = if exists {
        directory.clone()
    } else {
        nearest_existing_parent(&directory)
    };
    let git = match discover_repository(&probe).map_err(InitFailure::Git)? {
        // A catalog's units are in the catalog's repository.
        Discovered::WorkingTree { top_level }
            if catalog
                .as_ref()
                .is_some_and(|(root, ..)| same_directory(&top_level, root)) =>
        {
            GitOutcome::InCatalog { catalog: top_level }
        }
        _ if catalog.is_some() => {
            let root = catalog.map(|(root, ..)| root).unwrap_or_default();
            return Err(InitFailure::CatalogNotARepository(root));
        }
        Discovered::None => GitOutcome::Initialized,
        Discovered::Bare => return Err(InitFailure::Bare(directory)),
        Discovered::WorkingTree { top_level } => {
            if exists && same_directory(&top_level, &directory) {
                if !directory.join(".git").is_dir() {
                    return Err(InitFailure::LinkedWorkTree(directory));
                }
                GitOutcome::Adopted
            } else if top_level.join(MANIFEST).exists() {
                // Inside a unit's repository: a repository holds one unit.
                return Err(InitFailure::Inside {
                    directory,
                    top_level,
                });
            } else if top_level.join(CATALOG).exists() {
                // Inside a catalog, but not in its `units/`: a catalog's
                // units are its `units/<name>`, never nested elsewhere.
                return Err(InitFailure::InsideCatalog {
                    directory,
                    top_level,
                });
            } else {
                // Inside some other repository, such as a home directory
                // kept in Git: the unit gets its own, nested repository.
                GitOutcome::Nested { within: top_level }
            }
        }
    };

    // The files, as they are now.
    let manifest = match read_file(&directory.join(MANIFEST))? {
        None => {
            Planned::Create(manifest_text(identity, &description, catalog.is_some()).into_bytes())
        }
        Some(bytes) => {
            let text = String::from_utf8(bytes)
                .map_err(|_| InitFailure::ManifestNotUtf8(directory.join(MANIFEST)))?;
            let existing = match &catalog {
                Some((_, catalog, directories)) => read_catalog_manifest(
                    &text,
                    &InCatalog::new(catalog, identity.name(), directories),
                ),
                None => read_manifest(&text),
            }
            .map_err(|error| InitFailure::InvalidManifest {
                path: directory.join(MANIFEST),
                error,
            })?;
            if existing.name() != identity {
                return Err(InitFailure::OtherIdentity {
                    path: directory.join(MANIFEST),
                    declared: existing.name().clone(),
                });
            }
            Planned::Keep
        }
    };
    let schema = match read_file(&directory.join(MANIFEST_SCHEMA))? {
        None => Planned::Create(manifest_schema().as_bytes().to_vec()),
        Some(bytes) if bytes == manifest_schema().as_bytes() => Planned::Keep,
        Some(_) => Planned::Replace(manifest_schema().as_bytes().to_vec()),
    };
    let gitignore = match read_file(&directory.join(GITIGNORE))? {
        None => Planned::Create(format!("{IGNORE_BUILD}\n").into_bytes()),
        Some(bytes) if ignores_build(&bytes) => Planned::Keep,
        Some(bytes) => {
            let mut text = bytes;
            let newline: &[u8] = if text.contains(&b'\r') {
                b"\r\n"
            } else {
                b"\n"
            };
            if !text.is_empty() && !text.ends_with(b"\n") {
                text.extend_from_slice(newline);
            }
            text.extend_from_slice(IGNORE_BUILD.as_bytes());
            text.extend_from_slice(newline);
            // The existing bytes are kept as they are, whatever their
            // encoding; only the new line is added.
            Planned::Replace(text)
        }
    };
    // The author's own files, created only when missing and never read:
    // whatever is there is theirs.
    let readme = match occupied(&directory.join(README))? {
        false => Planned::Create(unit_readme(identity, &description).into_bytes()),
        true => Planned::Keep,
    };
    let agents = match occupied(&directory.join(AGENTS))? {
        false => Planned::Create(unit_agents(identity).into_bytes()),
        true => Planned::Keep,
    };
    // Whatever is at `src` already is the author's; a build reports it if
    // it is not a directory.
    let src = !occupied(&directory.join(SRC))?;
    for name in [SCHEMA_NEW, GITIGNORE_NEW] {
        if fs::symlink_metadata(directory.join(name)).is_ok() {
            return Err(InitFailure::StagingOccupied(directory.join(name)));
        }
    }

    // Writing, in an order that leaves the repository usable after a failure:
    // the directory and its repository first, then the manifest, which
    // makes the directory a unit, then the files that complete it.
    let mut done = Initialized {
        directory: directory.clone(),
        identity: identity.clone(),
        directory_created: false,
        git,
        manifest: FileOutcome::Kept,
        schema: FileOutcome::Kept,
        gitignore: FileOutcome::Kept,
        readme: FileOutcome::Kept,
        agents: FileOutcome::Kept,
        src: FileOutcome::Kept,
    };
    let failed = |done: &Initialized, step: Step, error: io::Error| InitFailure::Write {
        step,
        error,
        done: Box::new(done.clone()),
    };
    if !exists {
        fs::create_dir_all(&directory)
            .map_err(|error| failed(&done, Step::Directory(directory.clone()), error))?;
        done.directory_created = true;
    }
    if matches!(
        done.git,
        GitOutcome::Initialized | GitOutcome::Nested { .. }
    ) {
        init_repository(&directory).map_err(|error| InitFailure::Init {
            error,
            done: Box::new(done.clone()),
        })?;
    }
    let steps: [(&str, Planned, Option<&str>); 5] = [
        (MANIFEST, manifest, None),
        (MANIFEST_SCHEMA, schema, Some(SCHEMA_NEW)),
        (GITIGNORE, gitignore, Some(GITIGNORE_NEW)),
        (README, readme, None),
        (AGENTS, agents, None),
    ];
    for (name, planned, staging) in steps {
        let path = directory.join(name);
        let staged = staging.map(|staging| directory.join(staging));
        let outcome = match apply(planned, &path, staged.as_deref()) {
            Ok(outcome) => outcome,
            Err(error) => return Err(failed(&done, Step::File(path), error)),
        };
        match name {
            MANIFEST => done.manifest = outcome,
            MANIFEST_SCHEMA => done.schema = outcome,
            GITIGNORE => done.gitignore = outcome,
            README => done.readme = outcome,
            _ => done.agents = outcome,
        }
    }
    if src {
        let path = directory.join(SRC);
        fs::create_dir(&path).map_err(|error| failed(&done, Step::Directory(path), error))?;
        done.src = FileOutcome::Created;
    }
    Ok(done)
}

/// Writes one planned file: created new, or replaced through `staged`.
fn apply(planned: Planned, path: &Path, staged: Option<&Path>) -> io::Result<FileOutcome> {
    match planned {
        Planned::Keep => Ok(FileOutcome::Kept),
        Planned::Create(bytes) => {
            create_new(path, &bytes)?;
            Ok(FileOutcome::Created)
        }
        Planned::Replace(bytes) => {
            replace(staged.unwrap_or(path), path, &bytes)?;
            Ok(FileOutcome::Replaced)
        }
    }
}

/// What `unit init` plans for one file.
enum Planned {
    Keep,
    Create(Vec<u8>),
    Replace(Vec<u8>),
}

/// The nearest existing ancestor of `path`, or the root.
fn nearest_existing_parent(path: &Path) -> PathBuf {
    let mut current = path;
    while let Some(parent) = current.parent() {
        if parent.is_dir() {
            return parent.to_owned();
        }
        current = parent;
    }
    current.to_owned()
}

/// Whether two paths name the same directory, through links and spelling.
fn same_directory(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Reads a top-level file of the scaffold, or `None` when it does not
/// exist. Anything but a regular file is refused.
fn read_file(path: &Path) -> Result<Option<Vec<u8>>, InitFailure> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(InitFailure::Io {
                path: path.to_owned(),
                error,
            });
        }
    };
    if !metadata.is_file() {
        return Err(InitFailure::NotAFile(path.to_owned()));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(FILE_LIMIT + 1).read_to_end(&mut bytes))
        .map_err(|error| InitFailure::Io {
            path: path.to_owned(),
            error,
        })?;
    if bytes.len() as u64 > FILE_LIMIT {
        return Err(InitFailure::TooLarge {
            path: path.to_owned(),
            limit: FILE_LIMIT,
        });
    }
    Ok(Some(bytes))
}

/// Whether anything, a link included, is at `path`.
fn occupied(path: &Path) -> Result<bool, InitFailure> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(InitFailure::Io {
            path: path.to_owned(),
            error,
        }),
    }
}

/// Whether a `.gitignore` already keeps `build/` at the top level out of
/// Git: a line `/build/`, `build/`, `/build`, or `build`.
fn ignores_build(bytes: &[u8]) -> bool {
    String::from_utf8_lossy(bytes).lines().any(|line| {
        matches!(
            line.trim_end_matches('\r').trim(),
            "/build/" | "build/" | "/build" | "build"
        )
    })
}

/// The text of a new manifest, which must read back as the manifest it
/// describes; the scaffold never writes a manifest Assemblr rejects. A
/// unit `in_catalog` has no version of its own.
fn manifest_text(identity: &UnitIdentity, description: &Description, in_catalog: bool) -> String {
    let name = serde_json::Value::String(identity.to_string());
    let description_text = serde_json::Value::String(description.as_str().to_owned());
    if in_catalog {
        return format!(
            "{{\n  \"$schema\": \"{MANIFEST_SCHEMA_REFERENCE}\",\n  \"name\": {name},\n  \
             \"description\": {description_text}\n}}\n"
        );
    }
    let text = format!(
        "{{\n  \"$schema\": \"{MANIFEST_SCHEMA_REFERENCE}\",\n  \"name\": {name},\n  \"version\": \
         \"{FIRST_VERSION}\",\n  \"description\": {description_text}\n}}\n",
    );
    debug_assert!(
        read_manifest(&text).is_ok_and(|manifest| {
            manifest.name() == identity
                && manifest.version()
                    == &Version::parse(FIRST_VERSION)
                        .unwrap_or_else(|_| unreachable!("{FIRST_VERSION} is a SemVer version"))
                && manifest.description() == description
        }),
        "the scaffold's manifest must read back as written"
    );
    text
}

/// Creates `path` new, with `contents`, flushed.
fn create_new(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    if let Err(error) = file.write_all(contents).and_then(|()| file.sync_all()) {
        // A file this call created is not left half written: a rerun
        // would keep it as the author's.
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error);
    }
    Ok(())
}

/// Replaces `path` with `contents` through the new file `staged` and a
/// rename.
fn replace(staged: &Path, path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Err(error) = create_new(staged, contents) {
        let _ = fs::remove_file(staged);
        return Err(error);
    }
    fs::rename(staged, path).inspect_err(|_| {
        let _ = fs::remove_file(staged);
    })
}

/// What writing failed at.
#[derive(Debug)]
pub enum Step {
    /// Creating the directory.
    Directory(PathBuf),
    /// Creating or replacing the file.
    File(PathBuf),
}

/// An initialized scaffold.
#[derive(Debug, Clone)]
pub struct Initialized {
    directory: PathBuf,
    identity: UnitIdentity,
    directory_created: bool,
    git: GitOutcome,
    manifest: FileOutcome,
    schema: FileOutcome,
    gitignore: FileOutcome,
    readme: FileOutcome,
    agents: FileOutcome,
    src: FileOutcome,
}

impl Initialized {
    /// The unit's directory, absolute.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// The unit's identity.
    pub fn identity(&self) -> &UnitIdentity {
        &self.identity
    }

    /// Whether the directory was created.
    pub fn directory_created(&self) -> bool {
        self.directory_created
    }

    /// What happened to the Git repository.
    pub fn git(&self) -> &GitOutcome {
        &self.git
    }

    /// What happened to `manifest.jsonc`: created, or kept as it was.
    pub fn manifest(&self) -> FileOutcome {
        self.manifest
    }

    /// What happened to `manifest.schema.json`: created, replaced with the
    /// schema this Assemblr writes, or kept because it already was.
    pub fn schema(&self) -> FileOutcome {
        self.schema
    }

    /// What happened to `.gitignore`: created, replaced with `/build/`
    /// appended, or kept because it already ignores `build/`.
    pub fn gitignore(&self) -> FileOutcome {
        self.gitignore
    }

    /// What happened to `README.md`: created, or kept as it was.
    pub fn readme(&self) -> FileOutcome {
        self.readme
    }

    /// What happened to `AGENTS.md`: created, or kept as it was.
    pub fn agents(&self) -> FileOutcome {
        self.agents
    }

    /// What happened to `src/`: created empty, or kept as it was.
    pub fn src(&self) -> FileOutcome {
        self.src
    }
}

/// What `unit init` did about the Git repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitOutcome {
    /// The directory is a unit's in a catalog, whose repository holds it.
    InCatalog {
        /// The catalog's top level.
        catalog: PathBuf,
    },
    /// There was none, and one was initialized.
    Initialized,
    /// The directory lies inside another repository that is not a unit's,
    /// and got its own repository, nested in that one.
    Nested {
        /// The top level of the enclosing repository.
        within: PathBuf,
    },
    /// The directory is the top level of an existing repository, which was
    /// adopted.
    Adopted,
}

/// What `unit init` did to one file of the scaffold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOutcome {
    /// It did not exist and was created.
    Created,
    /// It existed and was replaced.
    Replaced,
    /// It was left as it was.
    Kept,
}

/// The directory a unit gets when none is given: its name, in `parent`
/// under the current directory, refused when it holds other files and no
/// manifest.
fn new_unit_directory(parent: &Path, identity: &UnitIdentity) -> Result<PathBuf, InitFailure> {
    let name = identity.name();
    // A name Windows reserves, such as `con`, cannot be a directory there.
    if RelativePath::parse(name).is_err() {
        return Err(InitFailure::ReservedName(name.to_owned()));
    }
    let directory = parent.join(name);
    match fs::read_dir(&directory) {
        Ok(mut entries) => {
            if entries.next().is_some() && !directory.join(MANIFEST).exists() {
                return Err(InitFailure::NotEmpty(directory));
            }
            Ok(directory)
        }
        // Missing, a file of that name, or one that cannot be read: the
        // checks of any directory report the last two.
        Err(_) => Ok(directory),
    }
}

/// The catalog `directory` is a unit's directory in, when it is one of a
/// catalog's `units/`: its top level, its `catalog.jsonc`, and the names of
/// its unit directories, `directory`'s included. `identity` must be the
/// one the catalog gives the directory.
fn in_catalog(
    directory: &Path,
    identity: &UnitIdentity,
) -> Result<Option<(PathBuf, Catalog, BTreeSet<String>)>, InitFailure> {
    let (Some(name), Some(units)) = (directory.file_name(), directory.parent()) else {
        return Ok(None);
    };
    let Some(root) = units.parent() else {
        return Ok(None);
    };
    let catalog_path = root.join(CATALOG);
    if units.file_name() != Some(UNITS.as_ref()) || fs::symlink_metadata(&catalog_path).is_err() {
        return Ok(None);
    }
    let bytes = read_file(&catalog_path)?.unwrap_or_default();
    let text =
        String::from_utf8(bytes).map_err(|_| InitFailure::ManifestNotUtf8(catalog_path.clone()))?;
    let catalog = read_catalog(&text).map_err(|error| InitFailure::InvalidCatalog {
        path: catalog_path,
        error,
    })?;
    let name = name.to_string_lossy();
    let expected = catalog
        .unit_identity(&name)
        .map_err(|error| InitFailure::CatalogDirectory {
            directory: directory.to_owned(),
            error,
        })?;
    if &expected != identity {
        return Err(InitFailure::NotTheCatalogName {
            given: identity.clone(),
            expected,
        });
    }
    // The other units, so that an existing manifest's `"catalog"`
    // requirements are read as the build reads them.
    let mut directories: BTreeSet<String> = fs::read_dir(units)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    directories.insert(name.into_owned());
    Ok(Some((root.to_owned(), catalog, directories)))
}

/// Why `unit init` failed.
#[derive(Debug)]
pub enum InitFailure {
    /// The directory is a catalog's top level, whose units go in `units/`.
    CatalogTopLevel(PathBuf),
    /// The directory is one of a catalog's `units/`, and the catalog is not
    /// a Git repository's top level.
    CatalogNotARepository(PathBuf),
    /// The catalog's `catalog.jsonc` is not valid.
    InvalidCatalog {
        /// The file.
        path: PathBuf,
        /// Why.
        error: ReadManifestError,
    },
    /// The directory is one of a catalog's `units/`, and its name is not a
    /// unit name.
    CatalogDirectory {
        /// The directory.
        directory: PathBuf,
        /// Why.
        error: IdentityError,
    },
    /// The identity is not the one the catalog gives the directory: its
    /// name, with the catalog's owner.
    NotTheCatalogName {
        /// The identity given.
        given: UnitIdentity,
        /// The catalog's.
        expected: UnitIdentity,
    },
    /// The description is empty or only whitespace.
    BlankDescription,
    /// The directory could not be examined.
    Directory {
        /// The directory.
        path: PathBuf,
        /// Why.
        error: io::Error,
    },
    /// The path exists and is not a directory.
    NotADirectory(PathBuf),
    /// No directory was given, and the one named after the unit holds
    /// other files and no manifest.
    NotEmpty(PathBuf),
    /// No directory was given, and the unit's name cannot be a directory's
    /// on every platform.
    ReservedName(String),
    /// The directory is inside a unit's repository, below its top level.
    Inside {
        /// The directory.
        directory: PathBuf,
        /// The top level of the repository it is in.
        top_level: PathBuf,
    },
    /// The directory is inside a catalog's repository, but not one of its
    /// `units/`.
    InsideCatalog {
        /// The directory.
        directory: PathBuf,
        /// The catalog's top level.
        top_level: PathBuf,
    },
    /// The directory is in a bare repository.
    Bare(PathBuf),
    /// The directory is the top level of a linked worktree, whose `.git` is
    /// a file.
    LinkedWorkTree(PathBuf),
    /// Git could not be run, or failed, while looking for a repository.
    Git(GitError),
    /// Git could not initialize the repository. `done` says what was
    /// written before.
    Init {
        /// Why.
        error: GitError,
        /// What was done before.
        done: Box<Initialized>,
    },
    /// `manifest.jsonc` exists and is not UTF-8 text.
    ManifestNotUtf8(PathBuf),
    /// `manifest.jsonc` exists and is not a valid manifest.
    InvalidManifest {
        /// The manifest.
        path: PathBuf,
        /// Why.
        error: ReadManifestError,
    },
    /// `manifest.jsonc` exists and declares another identity.
    OtherIdentity {
        /// The manifest.
        path: PathBuf,
        /// The identity it declares.
        declared: UnitIdentity,
    },
    /// A file of the scaffold is a directory, a symbolic link, or another
    /// special file.
    NotAFile(PathBuf),
    /// A file of the scaffold is larger than Assemblr reads.
    TooLarge {
        /// The file.
        path: PathBuf,
        /// The largest size read, in bytes.
        limit: u64,
    },
    /// A file could not be read.
    Io {
        /// The file.
        path: PathBuf,
        /// Why.
        error: io::Error,
    },
    /// A reserved staging path is occupied.
    StagingOccupied(PathBuf),
    /// Writing failed. `done` says what was written before.
    Write {
        /// What failed.
        step: Step,
        /// Why.
        error: io::Error,
        /// What was done before.
        done: Box<Initialized>,
    },
}

impl fmt::Display for InitFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CatalogTopLevel(path) => write!(
                f,
                "{} is a catalog's top level, with catalog.jsonc; its units go in units/, one \
                 directory each",
                path.display()
            ),
            Self::CatalogNotARepository(path) => write!(
                f,
                "the catalog at {} is not the top level of a Git repository; a catalog's units \
                 are in the catalog's repository",
                path.display()
            ),
            Self::InvalidCatalog { path, error } => write!(f, "{}: {error}", path.display()),
            Self::CatalogDirectory { directory, error } => write!(
                f,
                "a unit in a catalog is named after its directory, and the name of {} is not a \
                 unit name: {error}",
                directory.display()
            ),
            Self::NotTheCatalogName { given, expected } => write!(
                f,
                "a unit in a catalog is named after its directory, with the catalog's owner: \
                 this directory's unit is {expected}, not {given}"
            ),
            Self::BlankDescription => {
                f.write_str("the description must not be empty or only whitespace")
            }
            Self::Directory { path, error } => {
                write!(f, "could not examine {}: {error}", path.display())
            }
            Self::NotADirectory(path) => {
                write!(f, "{} exists and is not a directory", path.display())
            }
            Self::NotEmpty(path) => write!(
                f,
                "{} already exists and holds other files; a new unit gets a new directory",
                path.display()
            ),
            Self::ReservedName(name) => write!(
                f,
                "`{name}` cannot be a directory name on Windows, so the unit needs a directory"
            ),
            Self::Inside {
                directory,
                top_level,
            } => write!(
                f,
                "{} is inside the unit repository at {}; a repository holds exactly one unit, \
                 at its top level",
                directory.display(),
                top_level.display()
            ),
            Self::InsideCatalog {
                directory,
                top_level,
            } => write!(
                f,
                "{} is inside the catalog at {}, outside its units/; a catalog's units go in \
                 units/, one directory each",
                directory.display(),
                top_level.display()
            ),
            Self::Bare(path) => write!(
                f,
                "{} is in a bare Git repository; a unit needs a working tree",
                path.display()
            ),
            Self::LinkedWorkTree(path) => write!(
                f,
                "{} is a linked worktree; a unit's repository has its `.git` directory at its \
                 top level",
                path.display()
            ),
            Self::Git(error) => error.fmt(f),
            Self::Init { error, done } => write!(
                f,
                "could not initialize a Git repository in {}: {error}",
                done.directory().display()
            ),
            Self::ManifestNotUtf8(path) => write!(f, "{} is not UTF-8 text", path.display()),
            Self::InvalidManifest { path, error } => write!(
                f,
                "{} exists and is not a valid manifest, so it was left as it is: {error}",
                path.display()
            ),
            Self::OtherIdentity { path, declared } => write!(
                f,
                "{} already declares {declared}; a repository holds exactly one unit, and an \
                 existing manifest is never overwritten",
                path.display()
            ),
            Self::NotAFile(path) => write!(f, "{} is not a regular file", path.display()),
            Self::TooLarge { path, limit } => write!(
                f,
                "{} is larger than the {limit} bytes Assemblr reads",
                path.display()
            ),
            Self::Io { path, error } => write!(f, "could not read {}: {error}", path.display()),
            Self::StagingOccupied(path) => write!(
                f,
                "{} exists; the name is reserved for `unit init`, which replaces files through \
                 it, so move it away first",
                path.display()
            ),
            Self::Write { step, error, .. } => match step {
                Step::Directory(path) => {
                    write!(f, "could not create {}: {error}", path.display())
                }
                Step::File(path) => write!(f, "could not write {}: {error}", path.display()),
            },
        }
    }
}

impl std::error::Error for InitFailure {}
