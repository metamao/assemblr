//! Reading a unit's published versions and their files from a local Git
//! repository.
//!
//! A [`GitRepository`] reads Git's object database through the `git`
//! executable (see `docs/adr/0006-reading-git-repositories.md`). It never
//! checks anything out, so the working tree, the index, and `HEAD` are left
//! as they are, and content is read exactly as committed. Repositories are
//! untrusted: nothing a repository configures is run, nothing is fetched, and
//! malformed objects are reported as errors.

mod command;
mod object;

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use command::CatFile;
use object::{EntryKind, TreeEntry, commit_tree, parse_tree};

pub use object::ObjectId;

use super::manifest::{ReadManifestError, read_manifest};
use crate::domain::{FileTree, Manifest, RelativePath, RelativePathError, UnitFile, Version};

/// The largest `manifest.jsonc` that is read, in bytes. Manifests are small,
/// and the bound keeps a repository from making Assemblr load an arbitrarily
/// large file.
const MANIFEST_SIZE_LIMIT: u64 = 1024 * 1024;

/// Read the whole object.
const NO_LIMIT: u64 = u64::MAX;

const MANIFEST: &[u8] = b"manifest.jsonc";
const SRC: &[u8] = b"src";

/// A local Git repository, with or without a working tree.
#[derive(Debug, Clone)]
pub struct GitRepository {
    /// The path the repository was opened at.
    path: PathBuf,
    /// Its Git directory: `path/.git`, or `path` itself when it is bare.
    git_dir: PathBuf,
}

impl GitRepository {
    /// Opens the repository at `path`: a working tree whose repository is
    /// `path/.git`, or a bare repository at `path` itself.
    ///
    /// Git does not search parent directories, so a directory inside another
    /// repository is not a repository.
    pub fn open(path: &Path) -> Result<Self, GitError> {
        let dot_git = path.join(".git");
        let git_dir = if dot_git.exists() {
            dot_git
        } else {
            path.to_owned()
        };
        let repository = Self {
            path: path.to_owned(),
            git_dir,
        };
        if !path.is_dir() {
            let reason = "it is not a directory".to_owned();
            return Err(repository.error(GitErrorKind::NotARepository(reason)));
        }
        match repository.output(&["rev-parse", "--git-dir"]) {
            Ok(_) => Ok(repository),
            Err(GitError {
                kind: GitErrorKind::Failed { stderr, .. },
                ..
            }) => Err(repository.error(GitErrorKind::NotARepository(stderr))),
            Err(error) => Err(error),
        }
    }

    /// The path the repository was opened at.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads every version tag and the manifest it publishes, ordered by
    /// version and then by tag name.
    ///
    /// A tag `v<version>` is a version tag. Other tags publish no version and
    /// are not returned. A version tag that does not publish a valid manifest
    /// of its version is returned with the reason, and the other tags are
    /// still read.
    pub fn published_versions(&self) -> Result<Vec<VersionTag>, GitError> {
        let args = [
            "for-each-ref",
            "--format=%(objectname) %(refname)",
            "refs/tags/",
        ];
        let refs = self.output(&args)?;
        let mut cat_file = self.cat_file()?;
        let mut tags = Vec::new();
        for line in refs
            .split(|&byte| byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let unexpected = || {
                self.error(GitErrorKind::UnexpectedOutput {
                    command: format!("git {}", args.join(" ")),
                    detail: format!("unexpected line {:?}", String::from_utf8_lossy(line)),
                })
            };
            let space = line
                .iter()
                .position(|&byte| byte == b' ')
                .ok_or_else(unexpected)?;
            let object = std::str::from_utf8(&line[..space])
                .ok()
                .and_then(ObjectId::parse)
                .ok_or_else(unexpected)?;
            // A name that is not UTF-8 cannot be `v<version>`.
            let Some(name) = std::str::from_utf8(&line[space + 1..])
                .ok()
                .and_then(|refname| refname.strip_prefix("refs/tags/"))
            else {
                continue;
            };
            let Some(version) = Version::from_tag(name) else {
                continue;
            };
            let outcome = self.read_version(&mut cat_file, &object, &version)?;
            tags.push(VersionTag {
                name: name.to_owned(),
                version,
                outcome,
            });
        }
        cat_file.finish()?;
        tags.sort_by(|a, b| (&a.version, &a.name).cmp(&(&b.version, &b.name)));
        Ok(tags)
    }

    fn read_version(
        &self,
        cat_file: &mut CatFile<'_>,
        tag: &ObjectId,
        version: &Version,
    ) -> Result<Result<PublishedVersion, InvalidVersion>, GitError> {
        // `^{}` peels annotated tags, including tags of tags, to the object
        // they finally point at.
        let target = self.require(cat_file, &format!("{tag}^{{}}"), NO_LIMIT)?;
        if target.kind != "commit" {
            return Ok(Err(InvalidVersion::NotACommit {
                object_type: target.kind,
            }));
        }
        let commit = target.id.clone();
        let tree = commit_tree(&contents(target)).ok_or_else(|| self.malformed(&commit))?;
        let root = self.read_tree(cat_file, &tree)?;
        let Some(entry) = root.into_iter().find(|entry| entry.name == MANIFEST) else {
            return Ok(Err(InvalidVersion::MissingManifest));
        };
        if !matches!(entry.kind, EntryKind::File { .. }) {
            return Ok(Err(InvalidVersion::ManifestNotAFile));
        }
        let blob = self.require(cat_file, entry.id.as_str(), MANIFEST_SIZE_LIMIT)?;
        let Some(bytes) = blob.contents else {
            return Ok(Err(InvalidVersion::ManifestTooLarge {
                size: blob.size,
                limit: MANIFEST_SIZE_LIMIT,
            }));
        };
        let Ok(text) = String::from_utf8(bytes) else {
            return Ok(Err(InvalidVersion::ManifestNotUtf8));
        };
        let manifest = match read_manifest(&text) {
            Ok(manifest) => manifest,
            Err(error) => return Ok(Err(InvalidVersion::InvalidManifest(error))),
        };
        if manifest.version() != version {
            return Ok(Err(InvalidVersion::VersionMismatch {
                manifest: manifest.version().clone(),
            }));
        }
        Ok(Ok(PublishedVersion {
            manifest,
            provenance: Provenance { commit, tree },
        }))
    }

    /// Reads the `src/` files of `commit`, exactly as committed. A commit
    /// without `src/` has no files.
    ///
    /// Entries a unit file cannot be are rejected: symbolic links,
    /// submodules, and invalid paths. Every one is reported, sorted by path.
    pub fn read_src(&self, commit: &ObjectId) -> Result<FileTree, ReadSrcError> {
        let mut cat_file = self.cat_file()?;
        let object = self.require(&mut cat_file, commit.as_str(), NO_LIMIT)?;
        if object.kind != "commit" {
            return Err(ReadSrcError::NotACommit {
                commit: commit.clone(),
                object_type: object.kind,
            });
        }
        let tree = commit_tree(&contents(object)).ok_or_else(|| self.malformed(commit))?;
        let root = self.read_tree(&mut cat_file, &tree)?;
        let Some(src) = root.into_iter().find(|entry| entry.name == SRC) else {
            cat_file.finish()?;
            return Ok(FileTree::new());
        };
        if src.kind != EntryKind::Directory {
            return Err(ReadSrcError::SrcNotADirectory);
        }

        let mut files = Vec::new();
        let mut errors = Vec::new();
        let mut directories = vec![(Vec::new(), src.id)];
        while let Some((prefix, id)) = directories.pop() {
            for entry in self.read_tree(&mut cat_file, &id)? {
                let path = [prefix.as_slice(), &entry.name].concat();
                let reason = match entry.kind {
                    EntryKind::Directory => {
                        directories.push(([path.as_slice(), b"/"].concat(), entry.id));
                        continue;
                    }
                    EntryKind::File { executable } => match src_path(&path) {
                        Ok(path) => {
                            files.push((path, entry.id, executable));
                            continue;
                        }
                        Err(reason) => reason,
                    },
                    EntryKind::SymbolicLink => SrcEntryErrorReason::SymbolicLink,
                    EntryKind::Submodule => SrcEntryErrorReason::Submodule,
                    EntryKind::Other(mode) => SrcEntryErrorReason::UnsupportedMode(mode),
                };
                errors.push(SrcEntryError {
                    path: String::from_utf8_lossy(&path).into_owned(),
                    reason,
                });
            }
        }
        if !errors.is_empty() {
            errors.sort_by(|a, b| a.path.cmp(&b.path));
            return Err(ReadSrcError::Entries(errors));
        }

        let mut tree = FileTree::new();
        for (path, id, executable) in files {
            let blob = self.require(&mut cat_file, id.as_str(), NO_LIMIT)?;
            if blob.kind != "blob" {
                return Err(self.malformed(&id).into());
            }
            tree.insert(path, UnitFile::new(contents(blob), executable));
        }
        cat_file.finish()?;
        Ok(tree)
    }

    /// Reads an object that must exist.
    fn require(
        &self,
        cat_file: &mut CatFile<'_>,
        name: &str,
        limit: u64,
    ) -> Result<command::Object, GitError> {
        cat_file
            .read(name, limit)?
            .ok_or_else(|| self.error(GitErrorKind::MissingObject(name.to_owned())))
    }

    fn read_tree(
        &self,
        cat_file: &mut CatFile<'_>,
        id: &ObjectId,
    ) -> Result<Vec<TreeEntry>, GitError> {
        let object = self.require(cat_file, id.as_str(), NO_LIMIT)?;
        if object.kind != "tree" {
            return Err(self.malformed(id));
        }
        parse_tree(id, &contents(object)).ok_or_else(|| self.malformed(id))
    }

    fn malformed(&self, id: &ObjectId) -> GitError {
        self.error(GitErrorKind::MalformedObject(id.clone()))
    }
}

/// The contents of an object read without a limit.
fn contents(object: command::Object) -> Vec<u8> {
    object.contents.unwrap_or_default()
}

/// The path of a `src/` file, from its path relative to `src/`.
fn src_path(path: &[u8]) -> Result<RelativePath, SrcEntryErrorReason> {
    let path = std::str::from_utf8(path).map_err(|_| SrcEntryErrorReason::NotUtf8)?;
    RelativePath::parse(path).map_err(SrcEntryErrorReason::InvalidPath)
}

/// A tag that names a published version, `v<version>`, and what reading it
/// found.
#[derive(Debug, Clone, PartialEq)]
pub struct VersionTag {
    name: String,
    version: Version,
    outcome: Result<PublishedVersion, InvalidVersion>,
}

impl VersionTag {
    /// The tag's name, such as `v1.2.0`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The version the tag names.
    pub fn version(&self) -> &Version {
        &self.version
    }

    /// The published version, or why the tag does not publish a valid one.
    pub fn outcome(&self) -> Result<&PublishedVersion, &InvalidVersion> {
        self.outcome.as_ref()
    }
}

/// A version tag's valid content: a manifest of the tag's version, and where
/// it was read from.
#[derive(Debug, Clone, PartialEq)]
pub struct PublishedVersion {
    manifest: Manifest,
    provenance: Provenance,
}

impl PublishedVersion {
    /// The manifest at the tagged commit.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The tagged commit and its tree.
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
}

/// Where a published version's content was read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    commit: ObjectId,
    tree: ObjectId,
}

impl Provenance {
    /// The commit the version tag points at.
    pub fn commit(&self) -> &ObjectId {
        &self.commit
    }

    /// The commit's tree, which identifies the version's content: commits
    /// with the same tree publish the same files.
    pub fn tree(&self) -> &ObjectId {
        &self.tree
    }
}

/// Why a version tag does not publish a valid version.
#[derive(Debug, Clone, PartialEq)]
pub enum InvalidVersion {
    /// The tag points at something other than a commit, such as a tree.
    NotACommit {
        /// The type of object it points at.
        object_type: String,
    },
    /// The tagged commit has no `manifest.jsonc` at its root.
    MissingManifest,
    /// `manifest.jsonc` is a directory, a symbolic link, or a submodule.
    ManifestNotAFile,
    /// `manifest.jsonc` is larger than Assemblr reads.
    ManifestTooLarge {
        /// Its size, in bytes.
        size: u64,
        /// The largest size read, in bytes.
        limit: u64,
    },
    /// `manifest.jsonc` is not UTF-8 text.
    ManifestNotUtf8,
    /// `manifest.jsonc` is not a valid manifest.
    InvalidManifest(ReadManifestError),
    /// The manifest declares a version other than the tag's.
    VersionMismatch {
        /// The version the manifest declares.
        manifest: Version,
    },
}

impl fmt::Display for InvalidVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotACommit { object_type } => {
                write!(f, "the tag points at a {object_type}, not a commit")
            }
            Self::MissingManifest => f.write_str("the tagged commit has no `manifest.jsonc`"),
            Self::ManifestNotAFile => f.write_str("`manifest.jsonc` is not a regular file"),
            Self::ManifestTooLarge { size, limit } => write!(
                f,
                "`manifest.jsonc` has {size} bytes, more than the {limit} Assemblr reads"
            ),
            Self::ManifestNotUtf8 => f.write_str("`manifest.jsonc` is not UTF-8 text"),
            Self::InvalidManifest(error) => error.fmt(f),
            Self::VersionMismatch { manifest } => {
                write!(f, "the manifest declares version {manifest}")
            }
        }
    }
}

/// Why the `src/` files of a commit could not be read.
#[derive(Debug)]
pub enum ReadSrcError {
    /// The repository could not be read.
    Git(GitError),
    /// The object is not a commit.
    NotACommit {
        /// The object.
        commit: ObjectId,
        /// Its type.
        object_type: String,
    },
    /// `src` is not a directory.
    SrcNotADirectory,
    /// Entries that cannot be unit files, sorted by path. Every one is
    /// reported.
    Entries(Vec<SrcEntryError>),
}

impl From<GitError> for ReadSrcError {
    fn from(error: GitError) -> Self {
        Self::Git(error)
    }
}

impl fmt::Display for ReadSrcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(error) => error.fmt(f),
            Self::NotACommit {
                commit,
                object_type,
            } => write!(f, "{commit} is a {object_type}, not a commit"),
            Self::SrcNotADirectory => f.write_str("`src` is not a directory"),
            Self::Entries(errors) => {
                let plural = if errors.len() == 1 { "y" } else { "ies" };
                write!(f, "`src/` has {} unsupported entr{plural}:", errors.len())?;
                for error in errors {
                    write!(f, "\n  {error}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ReadSrcError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Git(error) => Some(error),
            _ => None,
        }
    }
}

/// An entry of `src/` that cannot be a unit file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrcEntryError {
    path: String,
    reason: SrcEntryErrorReason,
}

impl SrcEntryError {
    /// The entry's path relative to `src/`. Bytes that are not UTF-8 are
    /// replaced with `U+FFFD`.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Why it cannot be a unit file.
    pub fn reason(&self) -> &SrcEntryErrorReason {
        &self.reason
    }
}

impl fmt::Display for SrcEntryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "src/{}: ", self.path)?;
        match &self.reason {
            SrcEntryErrorReason::SymbolicLink => {
                f.write_str("symbolic links are not supported in `src/`")
            }
            SrcEntryErrorReason::Submodule => f.write_str("submodules are not supported in `src/`"),
            SrcEntryErrorReason::UnsupportedMode(mode) => {
                write!(f, "Git file mode {mode} is not supported")
            }
            SrcEntryErrorReason::NotUtf8 => f.write_str("the path is not UTF-8"),
            SrcEntryErrorReason::InvalidPath(error) => error.fmt(f),
        }
    }
}

/// Why an entry of `src/` cannot be a unit file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SrcEntryErrorReason {
    /// It is a symbolic link.
    SymbolicLink,
    /// It is a submodule.
    Submodule,
    /// Its Git file mode is neither a regular nor an executable file's.
    UnsupportedMode(String),
    /// Its path is not UTF-8.
    NotUtf8,
    /// Its path breaks the portable path rules.
    InvalidPath(RelativePathError),
}

/// A failure to read a Git repository.
#[derive(Debug)]
pub struct GitError {
    repository: PathBuf,
    kind: GitErrorKind,
}

impl GitError {
    /// The path of the repository being read.
    pub fn repository(&self) -> &Path {
        &self.repository
    }

    /// What went wrong.
    pub fn kind(&self) -> &GitErrorKind {
        &self.kind
    }
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let repository = self.repository.display();
        match &self.kind {
            GitErrorKind::Unavailable(error) => write!(
                f,
                "could not run `git` to read {repository}: {error}; \
                 Assemblr needs Git installed and on the PATH"
            ),
            GitErrorKind::NotARepository(reason) => {
                write!(f, "{repository} is not a Git repository: {reason}")
            }
            GitErrorKind::Failed {
                command,
                status,
                stderr,
            } => write!(f, "`{command}` failed in {repository} ({status}): {stderr}"),
            GitErrorKind::MissingObject(name) => write!(
                f,
                "{repository} does not have object {name}; \
                 the repository may be incomplete, such as a partial clone"
            ),
            GitErrorKind::MalformedObject(id) => {
                write!(f, "object {id} in {repository} is malformed")
            }
            GitErrorKind::UnexpectedOutput { command, detail } => {
                write!(
                    f,
                    "`{command}` in {repository} gave unexpected output: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for GitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            GitErrorKind::Unavailable(error) => Some(error),
            _ => None,
        }
    }
}

/// What went wrong reading a Git repository.
#[derive(Debug)]
pub enum GitErrorKind {
    /// `git` could not be run or communicated with, for example because it
    /// is not installed.
    Unavailable(io::Error),
    /// The path is not a Git repository. Holds the reason.
    NotARepository(String),
    /// A Git command failed.
    Failed {
        /// The command.
        command: String,
        /// How it exited.
        status: ExitStatus,
        /// What it reported.
        stderr: String,
    },
    /// An object the repository refers to is not in it. Holds the name it
    /// was looked up by.
    MissingObject(String),
    /// An object is not in the form Git writes.
    MalformedObject(ObjectId),
    /// A Git command's output was not in the expected form.
    UnexpectedOutput {
        /// The command.
        command: String,
        /// What was unexpected.
        detail: String,
    },
}
