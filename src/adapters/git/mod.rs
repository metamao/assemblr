//! Reading a unit's published versions and their files from a local Git
//! repository.
//!
//! A [`GitRepository`] reads Git's object database through the `git`
//! executable (see `docs/adr/0006-reading-git-repositories.md`). It never
//! checks anything out, so the working tree, the index, and `HEAD` are left
//! as they are, and content is read exactly as committed. Repositories are
//! untrusted: nothing a repository configures is run, nothing is fetched, and
//! malformed objects are reported as errors.
//!
//! A published version's `src/` and `amendments/` trees and its lockfile are
//! read by commit id (see [`GitRepository::read_src`],
//! [`GitRepository::read_amendments`], and
//! [`GitRepository::read_top_level_file`]). A unit build also reads the
//! unit's own `src/` from its working tree (see
//! [`GitRepository::read_working_src`]).

mod authoring;
mod command;
mod object;
mod working_tree;

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use command::{Object, Objects};
use object::{EntryKind, TreeEntry, commit_tree, parse_tree};

use super::manifest::{ReadManifestError, read_manifest};
use crate::domain::{
    FileTree, Manifest, ObjectId, Provenance, RelativePath, RelativePathError, UnitFile, Version,
};

// Bounds on what is read from a repository, which is untrusted. Git stores
// objects compressed, and trees can share subtrees, so a small repository can
// describe files far larger, or far more numerous, than it occupies. Every
// bound is far above what a unit needs, and exceeding one is an error, never
// a silent truncation.

/// The largest `manifest.jsonc`, in bytes.
pub(super) const MANIFEST_LIMIT: u64 = 1024 * 1024;
/// The largest commit object, in bytes.
const COMMIT_LIMIT: u64 = 1024 * 1024;
/// The largest directory listing (tree object), in bytes.
const TREE_LIMIT: u64 = 4 * 1024 * 1024;
/// The largest `src/` or `amendments/` file, in bytes.
pub(super) const FILE_LIMIT: u64 = 16 * 1024 * 1024;
/// The largest total size of the files of a unit's `src/`, and separately
/// of its `amendments/`, in bytes.
const SRC_SIZE_LIMIT: u64 = 128 * 1024 * 1024;
/// The most files and directories `src/`, and separately `amendments/`, may
/// hold, counting each path a shared subtree appears at.
const SRC_ENTRY_LIMIT: usize = 10_000;

const MANIFEST: &[u8] = b"manifest.jsonc";

/// A local Git repository, with or without a working tree.
#[derive(Debug, Clone)]
pub struct GitRepository {
    /// The path the repository was opened at.
    path: PathBuf,
    /// Its Git directory, absolute: `path/.git`, or `path` itself when it is
    /// bare.
    git_dir: PathBuf,
}

impl GitRepository {
    /// Opens the repository at `path`: a working tree whose repository is
    /// `path/.git`, or a bare repository at `path` itself.
    ///
    /// Git does not search parent directories, so a directory inside another
    /// repository is not a repository.
    pub fn open(path: &Path) -> Result<Self, GitError> {
        let not_a_directory = || GitError {
            repository: path.to_owned(),
            kind: GitErrorKind::NotARepository("it is not a directory".to_owned()),
        };
        if !path.is_dir() {
            return Err(not_a_directory());
        }
        // Git runs in `path`, so a relative `GIT_DIR` would be resolved from
        // there a second time.
        let absolute = std::path::absolute(path).map_err(|_| not_a_directory())?;
        let dot_git = absolute.join(".git");
        let git_dir = if dot_git.exists() { dot_git } else { absolute };
        let repository = Self {
            path: path.to_owned(),
            git_dir,
        };
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

    /// The repository's Git directory, absolute. For a linked worktree or a
    /// submodule, whose `.git` is a file, this is the directory it names.
    pub fn git_directory(&self) -> Result<PathBuf, GitError> {
        let args = ["rev-parse", "--absolute-git-dir"];
        let output = self.output(&args)?;
        let path = std::str::from_utf8(&output)
            .ok()
            .map(|path| path.trim_end_matches(['\r', '\n']))
            .filter(|path| !path.is_empty())
            .ok_or_else(|| {
                self.error(GitErrorKind::UnexpectedOutput {
                    command: format!("git {}", args.join(" ")),
                    detail: format!("unexpected path {:?}", String::from_utf8_lossy(&output)),
                })
            })?;
        Ok(PathBuf::from(path))
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
        let mut objects = self.objects()?;
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
            let (commit, outcome) = self.read_version(&mut objects, &object, &version)?;
            tags.push(VersionTag {
                name: name.to_owned(),
                version,
                commit,
                outcome,
            });
        }
        objects.finish()?;
        tags.sort_by(|a, b| (&a.version, &a.name).cmp(&(&b.version, &b.name)));
        Ok(tags)
    }

    fn read_version(
        &self,
        objects: &mut Objects<'_>,
        tag: &ObjectId,
        version: &Version,
    ) -> Result<(Option<ObjectId>, Result<PublishedVersion, InvalidVersion>), GitError> {
        // `^{}` peels annotated tags, including tags of tags, to the object
        // they finally point at.
        let target = self.require(objects, &format!("{tag}^{{}}"), 0)?;
        if target.header.kind != "commit" {
            return Ok((
                None,
                Err(InvalidVersion::NotACommit {
                    object_type: target.header.kind,
                }),
            ));
        }
        let commit = target.header.id;
        let outcome = match self.read_manifest(objects, &commit)? {
            Ok((_, manifest)) if manifest.version() != version => {
                Err(InvalidVersion::VersionMismatch {
                    manifest: manifest.version().clone(),
                })
            }
            Ok((tree, manifest)) => Ok(PublishedVersion {
                manifest,
                provenance: Provenance::new(commit.clone(), tree),
            }),
            Err(invalid) => Err(invalid),
        };
        Ok((Some(commit), outcome))
    }

    /// Reads `manifest.jsonc` as committed at `HEAD`, without the working
    /// tree or the index. Registration takes a unit's identity from it when
    /// no version is published (see `docs/domain/registries-and-discovery.md`).
    pub fn head_manifest(&self) -> Result<Manifest, HeadManifestError> {
        let mut objects = self.objects()?;
        let Some(head) = objects.header("HEAD^{commit}")? else {
            return Err(HeadManifestError::NoCommit);
        };
        let manifest = self
            .read_manifest(&mut objects, &head.id)?
            .map_err(HeadManifestError::Invalid)?
            .1;
        objects.finish()?;
        Ok(manifest)
    }

    /// Reads the manifest at each of `commits`, in order, as the content of
    /// a published version recorded at that commit.
    ///
    /// A commit is read by its id, never through a tag, so a tag moved since
    /// the commit was recorded does not change what is read. A commit that
    /// is not in the repository is an error, as is an object that is not a
    /// commit.
    pub fn read_published(
        &self,
        commits: &[&ObjectId],
    ) -> Result<Vec<Result<PublishedVersion, InvalidVersion>>, GitError> {
        let mut objects = self.objects()?;
        let mut versions = Vec::with_capacity(commits.len());
        for &commit in commits {
            let header = self.require(&mut objects, commit.as_str(), 0)?.header;
            if header.kind != "commit" {
                versions.push(Err(InvalidVersion::NotACommit {
                    object_type: header.kind,
                }));
                continue;
            }
            versions.push(
                self.read_manifest(&mut objects, commit)?
                    .map(|(tree, manifest)| PublishedVersion {
                        manifest,
                        provenance: Provenance::new(commit.clone(), tree),
                    }),
            );
        }
        objects.finish()?;
        Ok(versions)
    }

    /// The root tree of `commit` and the manifest in it, or why it has no
    /// valid manifest.
    fn read_manifest(
        &self,
        objects: &mut Objects<'_>,
        commit: &ObjectId,
    ) -> Result<Result<(ObjectId, Manifest), InvalidVersion>, GitError> {
        let tree = match self.commit_tree(objects, commit)? {
            Ok(tree) => tree,
            Err(too_large) => return Ok(Err(InvalidVersion::ObjectTooLarge(too_large))),
        };
        let root = match self.read_tree(objects, &tree)? {
            Ok(root) => root,
            Err(too_large) => return Ok(Err(InvalidVersion::ObjectTooLarge(too_large))),
        };
        let Some(entry) = root.into_iter().find(|entry| entry.name == MANIFEST) else {
            return Ok(Err(InvalidVersion::MissingManifest));
        };
        if !matches!(entry.kind, EntryKind::File { .. }) {
            return Ok(Err(InvalidVersion::ManifestNotAFile));
        }
        let blob = self.require(objects, entry.id.as_str(), MANIFEST_LIMIT)?;
        let Some(bytes) = blob.contents else {
            return Ok(Err(InvalidVersion::ManifestTooLarge {
                size: blob.header.size,
                limit: MANIFEST_LIMIT,
            }));
        };
        let Ok(text) = String::from_utf8(bytes) else {
            return Ok(Err(InvalidVersion::ManifestNotUtf8));
        };
        Ok(read_manifest(&text)
            .map(|manifest| (tree, manifest))
            .map_err(InvalidVersion::InvalidManifest))
    }

    /// Reads the file `name` at the root of `commit`'s tree, if there is
    /// one, with at most `limit` bytes: `Ok(None)` when the root has no
    /// entry of that name, and an error when the entry is not a regular
    /// file or is larger than `limit`.
    pub fn read_top_level_file(
        &self,
        commit: &ObjectId,
        name: &str,
        limit: u64,
    ) -> Result<Option<Vec<u8>>, ReadFileError> {
        let mut objects = self.objects()?;
        let root = self.root_entries(&mut objects, commit)?;
        let Some(entry) = root.into_iter().find(|entry| entry.name == name.as_bytes()) else {
            objects.finish()?;
            return Ok(None);
        };
        if !matches!(entry.kind, EntryKind::File { .. }) {
            return Err(ReadFileError::NotAFile {
                name: name.to_owned(),
            });
        }
        let blob = self.require(&mut objects, entry.id.as_str(), limit)?;
        if blob.header.kind != "blob" {
            return Err(self.malformed(&entry.id).into());
        }
        let Some(contents) = blob.contents else {
            return Err(ReadFileError::TooLarge {
                name: name.to_owned(),
                size: blob.header.size,
                limit,
            });
        };
        objects.finish()?;
        Ok(Some(contents))
    }

    /// The entries of `commit`'s root tree.
    fn root_entries(
        &self,
        objects: &mut Objects<'_>,
        commit: &ObjectId,
    ) -> Result<Vec<TreeEntry>, RootError> {
        let header = objects
            .header(commit.as_str())?
            .ok_or_else(|| self.error(GitErrorKind::MissingObject(commit.to_string())))?;
        if header.kind != "commit" {
            return Err(RootError::NotACommit {
                commit: commit.clone(),
                object_type: header.kind,
            });
        }
        let tree = self
            .commit_tree(objects, commit)?
            .map_err(RootError::ObjectTooLarge)?;
        self.read_tree(objects, &tree)?
            .map_err(RootError::ObjectTooLarge)
    }

    /// Reads the `src/` files of `commit`, exactly as committed. A commit
    /// without `src/` has no files.
    ///
    /// Entries a unit file cannot be are rejected: symbolic links,
    /// submodules, invalid paths, and files or directories larger than
    /// Assemblr reads. Every one is reported, sorted by path. `src/` as a
    /// whole is bounded too, in its number of entries and its total size.
    pub fn read_src(&self, commit: &ObjectId) -> Result<FileTree, ReadSrcError> {
        self.read_unit_files(commit, UnitDirectory::Src)
    }

    /// Reads the `amendments/` copies of `commit`, exactly as committed and
    /// by the rules of [`read_src`](Self::read_src). A commit without
    /// `amendments/` has no copies.
    pub fn read_amendments(&self, commit: &ObjectId) -> Result<FileTree, ReadSrcError> {
        self.read_unit_files(commit, UnitDirectory::Amendments)
    }

    fn read_unit_files(
        &self,
        commit: &ObjectId,
        directory: UnitDirectory,
    ) -> Result<FileTree, ReadSrcError> {
        let mut objects = self.objects()?;
        let root = self.root_entries(&mut objects, commit)?;
        let Some(src) = root
            .into_iter()
            .find(|entry| entry.name == directory.as_str().as_bytes())
        else {
            objects.finish()?;
            return Ok(FileTree::new());
        };
        if src.kind != EntryKind::Directory {
            return Err(ReadSrcError::NotADirectory(directory));
        }

        let mut files = Vec::new();
        let mut errors = Vec::new();
        let mut entries = 0;
        let mut directories = vec![(Vec::new(), src.id)];
        while let Some((prefix, id)) = directories.pop() {
            let listing = match self.read_tree(&mut objects, &id)? {
                Ok(listing) => listing,
                Err(too_large) => {
                    errors.push(SrcEntryError {
                        directory,
                        path: String::from_utf8_lossy(prefix.strip_suffix(b"/").unwrap_or(&prefix))
                            .into_owned(),
                        reason: SrcEntryErrorReason::TooLarge {
                            size: too_large.size,
                            limit: too_large.limit,
                        },
                    });
                    continue;
                }
            };
            entries += listing.len();
            if entries > SRC_ENTRY_LIMIT {
                return Err(ReadSrcError::TooManyEntries {
                    directory,
                    limit: SRC_ENTRY_LIMIT,
                });
            }
            for entry in listing {
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
                    directory,
                    path: String::from_utf8_lossy(&path).into_owned(),
                    reason,
                });
            }
        }

        // Sizes first, so nothing is read unless the whole directory fits.
        let mut total: u64 = 0;
        for (path, id, _) in &files {
            let header = self.require(&mut objects, id.as_str(), 0)?.header;
            if header.kind != "blob" {
                return Err(self.malformed(id).into());
            }
            if header.size > FILE_LIMIT {
                errors.push(SrcEntryError {
                    directory,
                    path: path.to_string(),
                    reason: SrcEntryErrorReason::TooLarge {
                        size: header.size,
                        limit: FILE_LIMIT,
                    },
                });
            }
            total = total.saturating_add(header.size);
        }
        if !errors.is_empty() {
            errors.sort_by(|a, b| a.path.cmp(&b.path));
            return Err(ReadSrcError::Entries(errors));
        }
        if total > SRC_SIZE_LIMIT {
            return Err(ReadSrcError::TooLarge {
                directory,
                size: total,
                limit: SRC_SIZE_LIMIT,
            });
        }

        let mut tree = FileTree::new();
        for (path, id, executable) in files {
            let blob = self.require(&mut objects, id.as_str(), FILE_LIMIT)?;
            let contents = blob.contents.ok_or_else(|| self.malformed(&id))?;
            tree.insert(path, UnitFile::new(contents, executable));
        }
        objects.finish()?;
        Ok(tree)
    }

    /// Reads an object that must exist, with its contents if they are at
    /// most `limit` bytes.
    fn require(
        &self,
        objects: &mut Objects<'_>,
        name: &str,
        limit: u64,
    ) -> Result<Object, GitError> {
        objects
            .read(name, limit)?
            .ok_or_else(|| self.error(GitErrorKind::MissingObject(name.to_owned())))
    }

    /// The tree of `commit`, or the commit's size if it is too large to read.
    fn commit_tree(
        &self,
        objects: &mut Objects<'_>,
        commit: &ObjectId,
    ) -> Result<Result<ObjectId, ObjectTooLarge>, GitError> {
        let object = self.require(objects, commit.as_str(), COMMIT_LIMIT)?;
        let Some(contents) = object.contents else {
            return Ok(Err(ObjectTooLarge::new(object, COMMIT_LIMIT)));
        };
        commit_tree(&contents)
            .map(Ok)
            .ok_or_else(|| self.malformed(commit))
    }

    /// The entries of tree `id`, or its size if it is too large to read.
    fn read_tree(
        &self,
        objects: &mut Objects<'_>,
        id: &ObjectId,
    ) -> Result<Result<Vec<TreeEntry>, ObjectTooLarge>, GitError> {
        let object = self.require(objects, id.as_str(), TREE_LIMIT)?;
        if object.header.kind != "tree" {
            return Err(self.malformed(id));
        }
        let Some(contents) = object.contents else {
            return Ok(Err(ObjectTooLarge::new(object, TREE_LIMIT)));
        };
        parse_tree(id, &contents)
            .map(Ok)
            .ok_or_else(|| self.malformed(id))
    }

    fn malformed(&self, id: &ObjectId) -> GitError {
        self.error(GitErrorKind::MalformedObject(id.clone()))
    }
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
    commit: Option<ObjectId>,
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

    /// The commit the tag points at, through any annotated tags, or `None`
    /// when it points at another kind of object.
    pub fn commit(&self) -> Option<&ObjectId> {
        self.commit.as_ref()
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

/// Why a version tag does not publish a valid version.
#[derive(Debug, Clone, PartialEq)]
pub enum InvalidVersion {
    /// The tag points at something other than a commit, such as a tree.
    NotACommit {
        /// The type of object it points at.
        object_type: String,
    },
    /// The tagged commit, or its root directory listing, is larger than
    /// Assemblr reads.
    ObjectTooLarge(ObjectTooLarge),
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
            Self::ObjectTooLarge(too_large) => too_large.fmt(f),
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

/// A directory of a unit repository that holds unit files: `src/`, or
/// `amendments/`, which follows the same rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitDirectory {
    /// `src/`, the files the unit introduces.
    Src,
    /// `amendments/`, the unit's copies of dependency files.
    Amendments,
}

impl UnitDirectory {
    /// The directory's name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Src => "src",
            Self::Amendments => "amendments",
        }
    }
}

impl fmt::Display for UnitDirectory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why the unit files of a directory could not be read.
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
    /// The commit, or its root directory listing, is larger than Assemblr
    /// reads.
    ObjectTooLarge(ObjectTooLarge),
    /// The entry at the directory's path is not a directory.
    NotADirectory(UnitDirectory),
    /// Entries that cannot be unit files, sorted by path. Every one is
    /// reported.
    Entries(Vec<SrcEntryError>),
    /// The directory holds more files and directories than Assemblr reads.
    TooManyEntries {
        /// The directory.
        directory: UnitDirectory,
        /// The most entries read.
        limit: usize,
    },
    /// The directory's files total more bytes than Assemblr reads.
    TooLarge {
        /// The directory.
        directory: UnitDirectory,
        /// Their total size, in bytes.
        size: u64,
        /// The largest total read, in bytes.
        limit: u64,
    },
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
            Self::ObjectTooLarge(too_large) => too_large.fmt(f),
            Self::NotADirectory(directory) => write!(f, "`{directory}` is not a directory"),
            Self::TooManyEntries { directory, limit } => write!(
                f,
                "`{directory}/` has more than {limit} files and directories, the most Assemblr \
                 reads"
            ),
            Self::TooLarge {
                directory,
                size,
                limit,
            } => write!(
                f,
                "`{directory}/` files total {size} bytes, more than the {limit} Assemblr reads"
            ),
            Self::Entries(errors) => {
                let plural = if errors.len() == 1 { "y" } else { "ies" };
                let directory = errors
                    .first()
                    .map_or(UnitDirectory::Src, |error| error.directory);
                write!(
                    f,
                    "`{directory}/` has {} unsupported entr{plural}:",
                    errors.len()
                )?;
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

/// Why a commit's root tree could not be listed.
enum RootError {
    Git(GitError),
    NotACommit {
        commit: ObjectId,
        object_type: String,
    },
    ObjectTooLarge(ObjectTooLarge),
}

impl From<GitError> for RootError {
    fn from(error: GitError) -> Self {
        Self::Git(error)
    }
}

impl From<RootError> for ReadSrcError {
    fn from(error: RootError) -> Self {
        match error {
            RootError::Git(error) => Self::Git(error),
            RootError::NotACommit {
                commit,
                object_type,
            } => Self::NotACommit {
                commit,
                object_type,
            },
            RootError::ObjectTooLarge(too_large) => Self::ObjectTooLarge(too_large),
        }
    }
}

impl From<RootError> for ReadFileError {
    fn from(error: RootError) -> Self {
        match error {
            RootError::Git(error) => Self::Git(error),
            RootError::NotACommit {
                commit,
                object_type,
            } => Self::NotACommit {
                commit,
                object_type,
            },
            RootError::ObjectTooLarge(too_large) => Self::ObjectTooLarge(too_large),
        }
    }
}

/// Why a file at the root of a commit's tree could not be read.
#[derive(Debug)]
pub enum ReadFileError {
    /// The repository could not be read.
    Git(GitError),
    /// The object is not a commit.
    NotACommit {
        /// The object.
        commit: ObjectId,
        /// Its type.
        object_type: String,
    },
    /// The commit, or its root directory listing, is larger than Assemblr
    /// reads.
    ObjectTooLarge(ObjectTooLarge),
    /// The entry is a directory, a symbolic link, or a submodule.
    NotAFile {
        /// The file's name.
        name: String,
    },
    /// The file is larger than the caller reads.
    TooLarge {
        /// The file's name.
        name: String,
        /// Its size, in bytes.
        size: u64,
        /// The largest size read, in bytes.
        limit: u64,
    },
}

impl From<GitError> for ReadFileError {
    fn from(error: GitError) -> Self {
        Self::Git(error)
    }
}

impl fmt::Display for ReadFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(error) => error.fmt(f),
            Self::NotACommit {
                commit,
                object_type,
            } => write!(f, "{commit} is a {object_type}, not a commit"),
            Self::ObjectTooLarge(too_large) => too_large.fmt(f),
            Self::NotAFile { name } => write!(f, "`{name}` is not a regular file"),
            Self::TooLarge { name, size, limit } => write!(
                f,
                "`{name}` is {size} bytes, larger than the {limit} bytes Assemblr reads"
            ),
        }
    }
}

impl std::error::Error for ReadFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Git(error) => Some(error),
            _ => None,
        }
    }
}

/// Why the manifest committed at `HEAD` could not be read.
#[derive(Debug)]
pub enum HeadManifestError {
    /// The repository could not be read.
    Git(GitError),
    /// `HEAD` names no commit, as in a repository with no commits yet.
    NoCommit,
    /// The commit at `HEAD` has no valid manifest.
    Invalid(InvalidVersion),
}

impl From<GitError> for HeadManifestError {
    fn from(error: GitError) -> Self {
        Self::Git(error)
    }
}

impl fmt::Display for HeadManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(error) => error.fmt(f),
            Self::NoCommit => f.write_str("`HEAD` has no commit"),
            Self::Invalid(InvalidVersion::MissingManifest) => {
                f.write_str("the commit at `HEAD` has no `manifest.jsonc`")
            }
            Self::Invalid(invalid) => {
                write!(
                    f,
                    "the `manifest.jsonc` committed at `HEAD` is unusable: {invalid}"
                )
            }
        }
    }
}

impl std::error::Error for HeadManifestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Git(error) => Some(error),
            _ => None,
        }
    }
}

/// An entry of `src/` or `amendments/` that cannot be a unit file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrcEntryError {
    directory: UnitDirectory,
    path: String,
    reason: SrcEntryErrorReason,
}

impl SrcEntryError {
    /// The directory the entry is in.
    pub fn directory(&self) -> UnitDirectory {
        self.directory
    }

    /// The entry's path relative to that directory. Bytes that are not
    /// UTF-8 are replaced with `U+FFFD`.
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
        let directory = self.directory;
        write!(f, "{directory}/{}: ", self.path)?;
        match &self.reason {
            SrcEntryErrorReason::SymbolicLink => {
                write!(f, "symbolic links are not supported in `{directory}/`")
            }
            SrcEntryErrorReason::Submodule => {
                write!(f, "submodules are not supported in `{directory}/`")
            }
            SrcEntryErrorReason::UnsupportedMode(mode) => {
                write!(f, "Git file mode {mode} is not supported")
            }
            SrcEntryErrorReason::TooLarge { size, limit } => {
                write!(f, "{size} bytes, more than the {limit} Assemblr reads")
            }
            SrcEntryErrorReason::NotUtf8 => f.write_str("the path is not UTF-8"),
            SrcEntryErrorReason::InvalidPath(error) => error.fmt(f),
            SrcEntryErrorReason::NotARegularFile => {
                write!(f, "not a regular file, which `{directory}/` may hold only")
            }
            SrcEntryErrorReason::Unreadable(reason) => write!(f, "could not be read: {reason}"),
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
    /// The file, or the directory's listing, is larger than Assemblr reads.
    TooLarge {
        /// Its size, in bytes.
        size: u64,
        /// The largest size read, in bytes.
        limit: u64,
    },
    /// Its path is not UTF-8.
    NotUtf8,
    /// Its path breaks the portable path rules.
    InvalidPath(RelativePathError),
    /// In a working tree, it is not a regular file, for example a directory
    /// holding a nested repository.
    NotARegularFile,
    /// In a working tree, it could not be read. Holds the reason.
    Unreadable(String),
}

/// An object larger than Assemblr reads from a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectTooLarge {
    id: ObjectId,
    object_type: String,
    size: u64,
    limit: u64,
}

impl ObjectTooLarge {
    fn new(object: Object, limit: u64) -> Self {
        Self {
            id: object.header.id,
            object_type: object.header.kind,
            size: object.header.size,
            limit,
        }
    }

    /// The object.
    pub fn id(&self) -> &ObjectId {
        &self.id
    }

    /// Its type, such as `commit` or `tree`.
    pub fn object_type(&self) -> &str {
        &self.object_type
    }

    /// Its size, in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// The largest size read for an object of its type, in bytes.
    pub fn limit(&self) -> u64 {
        self.limit
    }
}

impl fmt::Display for ObjectTooLarge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} has {} bytes, more than the {} Assemblr reads",
            self.object_type, self.id, self.size, self.limit
        )
    }
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
