//! Reading a unit's files from its working tree, as `unit build` composes
//! them (see `docs/domain/authoring-workflow.md`).
//!
//! A file is selected the way committing everything would publish it: every
//! tracked file, and every untracked file that the working tree's
//! `.gitignore` files do not ignore. Its contents are read from the working
//! tree as they are now, never from the index. Only `.gitignore` files
//! decide what is ignored: `.git/info/exclude` and `core.excludesFile` are
//! specific to one clone or one machine, and are not applied.
//!
//! Git lists the paths, and the files are read from the file system. Git only
//! reads the index and the working tree here. It writes nothing, and runs no
//! configured program.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, Metadata};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use super::object::EntryKind;
use super::{
    FILE_LIMIT, GitError, GitErrorKind, GitRepository, ReadSrcError, SRC_ENTRY_LIMIT,
    SRC_SIZE_LIMIT, SrcEntryError, SrcEntryErrorReason, UnitDirectory, src_path,
};
use crate::domain::{FileTree, RelativePath, UnitFile};

/// A path the working tree would publish, and its mode in the index if it
/// is tracked.
type Listing = BTreeMap<Vec<u8>, Option<EntryKind>>;

impl GitRepository {
    /// Reads the `src/` files of the working tree, with their current
    /// contents.
    ///
    /// A tracked file deleted from the working tree is not part of it.
    /// Symbolic links, including tracked ones checked out as plain files,
    /// submodules, nested repositories, and other entries a unit file cannot
    /// be are rejected, as are paths that break the portable path rules.
    /// Every one is reported, sorted by path. `src/` is bounded as a
    /// published version's is.
    ///
    /// On Unix a file is executable when its owner may execute it, as Git
    /// decides. Windows file systems record no such bit, so there a tracked
    /// file is executable when the index records it so, and an untracked
    /// file is not.
    pub fn read_working_src(&self) -> Result<FileTree, ReadSrcError> {
        self.read_working_files(UnitDirectory::Src)
    }

    /// Reads the working tree's `amendments/` copies, selected and read
    /// exactly as [`read_working_src`](Self::read_working_src) reads `src/`.
    pub fn read_working_amendments(&self) -> Result<FileTree, ReadSrcError> {
        self.read_working_files(UnitDirectory::Amendments)
    }

    /// The names directly under the working tree's `units/` that it would
    /// publish, sorted, each with whether it is a real directory there: a
    /// catalog's units are its directories, and anything else in `units/`
    /// is reported. A directory whose files were all deleted is not listed.
    pub(crate) fn working_units(&self) -> Result<BTreeMap<String, bool>, GitError> {
        let root = self.work_tree()?;
        let listing = self.listing(&root, super::catalog::UNITS)?;
        let prefix = format!("{}/", super::catalog::UNITS);
        let mut units = BTreeMap::new();
        for path in listing.keys() {
            let Some(rest) = path.strip_prefix(prefix.as_bytes()) else {
                continue;
            };
            let (name, nested) = match rest.iter().position(|&byte| byte == b'/') {
                Some(slash) => (&rest[..slash], true),
                None => (rest, false),
            };
            let Ok(name) = std::str::from_utf8(name) else {
                units.insert(String::from_utf8_lossy(name).into_owned(), false);
                continue;
            };
            if units.contains_key(name) {
                continue;
            }
            let directory = root.join(super::catalog::UNITS).join(name);
            let directory = match fs::symlink_metadata(&directory) {
                Ok(metadata) => nested && metadata.is_dir(),
                // Deleted with everything in it.
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(_) => false,
            };
            units.insert(name.to_owned(), directory);
        }
        Ok(units)
    }

    fn read_working_files(&self, directory: UnitDirectory) -> Result<FileTree, ReadSrcError> {
        let root = self.work_tree()?;
        // In a catalog, the unit's directory holds its own.
        let location = match &self.unit {
            Some(unit) => format!("{}/{unit}/{directory}", super::catalog::UNITS),
            None => directory.as_str().to_owned(),
        };
        let listing = self.listing(&root, &location)?;
        let prefix = format!("{location}/");
        let entry_error = |path: &str, reason| SrcEntryError {
            directory,
            path: path.to_owned(),
            reason,
        };
        let mut entries = Vec::new();
        let mut errors = Vec::new();
        let mut directories = BTreeSet::new();
        for (path, index) in &listing {
            let Some(relative) = path.strip_prefix(prefix.as_bytes()) else {
                // The directory itself is listed when it is not a directory.
                return Err(ReadSrcError::NotADirectory(directory));
            };
            let display = String::from_utf8_lossy(relative).into_owned();
            if let Some(nested) = relative.strip_suffix(b"/") {
                // Git lists a nested repository as a directory.
                errors.push(entry_error(
                    &String::from_utf8_lossy(nested),
                    SrcEntryErrorReason::NotARegularFile,
                ));
                continue;
            }
            match index {
                None | Some(EntryKind::File { .. }) => {}
                Some(kind) => {
                    errors.push(entry_error(&display, unsupported(kind)));
                    continue;
                }
            }
            match src_path(relative) {
                Ok(relative) => {
                    let mut segments: Vec<&str> = relative.segments().collect();
                    segments.pop();
                    let mut directory = String::new();
                    for segment in segments {
                        if !directory.is_empty() {
                            directory.push('/');
                        }
                        directory.push_str(segment);
                        directories.insert(directory.clone());
                    }
                    let executable = matches!(index, Some(EntryKind::File { executable: true }));
                    entries.push((relative, executable));
                }
                Err(reason) => errors.push(entry_error(&display, reason)),
            }
        }
        if listing.len() + directories.len() > SRC_ENTRY_LIMIT {
            return Err(ReadSrcError::TooManyEntries {
                directory,
                limit: SRC_ENTRY_LIMIT,
            });
        }

        let src = root.join(&location);
        let mut checked = Directories::default();
        let mut files = Vec::new();
        let mut total: u64 = 0;
        for (path, indexed_executable) in entries {
            let file = src.join(path.as_str());
            if let Err(reason) = checked.within(&src, &path) {
                if let Some((directory, reason)) = reason {
                    errors.push(entry_error(&directory, reason));
                }
                continue;
            }
            let metadata = match fs::symlink_metadata(&file) {
                Ok(metadata) => metadata,
                // Deleted from the working tree, so not part of it.
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    errors.push(entry_error(path.as_str(), unreadable(&error)));
                    continue;
                }
            };
            if let Err(reason) = regular_file(&metadata) {
                errors.push(entry_error(path.as_str(), reason));
                continue;
            }
            if metadata.len() > FILE_LIMIT {
                errors.push(entry_error(
                    path.as_str(),
                    SrcEntryErrorReason::TooLarge {
                        size: metadata.len(),
                        limit: FILE_LIMIT,
                    },
                ));
                continue;
            }
            total = total.saturating_add(metadata.len());
            let executable = executable(&metadata, indexed_executable);
            files.push((path, file, executable));
        }
        if !errors.is_empty() {
            errors.sort_by(|a, b| a.path.cmp(&b.path));
            errors.dedup();
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
        let mut total: u64 = 0;
        for (path, file, executable) in files {
            let contents = match read_bounded(&file) {
                Ok(contents) => contents,
                Err(reason) => {
                    errors.push(entry_error(path.as_str(), reason));
                    continue;
                }
            };
            // The files may have grown since their sizes were checked.
            total = total.saturating_add(contents.len() as u64);
            tree.insert(path, UnitFile::new(contents, executable));
        }
        if !errors.is_empty() {
            return Err(ReadSrcError::Entries(errors));
        }
        if total > SRC_SIZE_LIMIT {
            return Err(ReadSrcError::TooLarge {
                directory,
                size: total,
                limit: SRC_SIZE_LIMIT,
            });
        }
        Ok(tree)
    }

    /// The repository's top level, absolute: the directory it was opened
    /// at, which must hold its `.git`.
    fn work_tree(&self) -> Result<PathBuf, GitError> {
        let not_a_work_tree = |reason: &str| {
            self.error(GitErrorKind::NotARepository(format!(
                "{reason}; a unit's repository has a working tree with `.git` at its top level"
            )))
        };
        let root = std::path::absolute(&self.path)
            .map_err(|_| not_a_work_tree("its path cannot be made absolute"))?;
        if self.git_dir != root.join(".git") {
            return Err(not_a_work_tree("it has no `.git` here"));
        }
        Ok(root)
    }

    /// Every path under `directory` that the working tree would publish:
    /// tracked paths with their index mode, and untracked paths that no
    /// `.gitignore` file ignores.
    fn listing(&self, root: &Path, directory: &str) -> Result<Listing, GitError> {
        let staged =
            self.work_tree_output(root, &["ls-files", "-z", "--stage", "--", directory])?;
        let mut listing = Listing::new();
        // While a merge is in conflict a path has several stages. Stage 0
        // (merged) or 2 (ours) decides its mode, as for the file Git leaves
        // in the working tree.
        let mut stages: BTreeMap<Vec<u8>, u8> = BTreeMap::new();
        for record in staged.split(|&byte| byte == 0).filter(|r| !r.is_empty()) {
            let unexpected = || {
                self.error(GitErrorKind::UnexpectedOutput {
                    command: format!("git ls-files --stage -- {directory}"),
                    detail: format!("unexpected record {:?}", String::from_utf8_lossy(record)),
                })
            };
            let tab = record
                .iter()
                .position(|&byte| byte == b'\t')
                .ok_or_else(unexpected)?;
            let (info, path) = (&record[..tab], &record[tab + 1..]);
            let mut fields = info.split(|&byte| byte == b' ');
            let (Some(mode), Some(_), Some(stage), None) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                return Err(unexpected());
            };
            let rank = match stage {
                b"0" => 0,
                b"2" => 1,
                _ => 2,
            };
            if stages.get(path).is_none_or(|&best| rank < best) {
                stages.insert(path.to_vec(), rank);
                listing.insert(path.to_vec(), Some(EntryKind::from_mode(mode)));
            }
        }
        let others = self.work_tree_output(
            root,
            &[
                "ls-files",
                "-z",
                "--others",
                "--exclude-per-directory=.gitignore",
                "--",
                directory,
            ],
        )?;
        for path in others.split(|&byte| byte == 0).filter(|p| !p.is_empty()) {
            listing.entry(path.to_vec()).or_insert(None);
        }
        Ok(listing)
    }
}

/// The directories of `src/` already checked, and whether each is a real
/// directory.
#[derive(Default)]
struct Directories {
    checked: BTreeMap<String, Result<(), Option<SrcEntryErrorReason>>>,
}

impl Directories {
    /// Whether every directory between `src` and `path` is a real directory.
    /// A symbolic link is reported once, at its own path. A directory that
    /// is gone, or no longer a directory, means `path` was deleted.
    fn within(
        &mut self,
        src: &Path,
        path: &RelativePath,
    ) -> Result<(), Option<(String, SrcEntryErrorReason)>> {
        let mut segments: Vec<&str> = path.segments().collect();
        segments.pop();
        let mut directory = String::new();
        for segment in std::iter::once("").chain(segments) {
            if !segment.is_empty() {
                if !directory.is_empty() {
                    directory.push('/');
                }
                directory.push_str(segment);
            }
            let first = !self.checked.contains_key(&directory);
            let outcome =
                self.checked.entry(directory.clone()).or_insert_with(
                    || match fs::symlink_metadata(src.join(&directory)) {
                        Ok(metadata) if metadata.file_type().is_symlink() => {
                            Err(Some(SrcEntryErrorReason::SymbolicLink))
                        }
                        Ok(metadata) if metadata.is_dir() => Ok(()),
                        Ok(_) => Err(None),
                        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(None),
                        Err(error) => Err(Some(unreadable(&error))),
                    },
                );
            match outcome {
                Ok(()) => {}
                Err(Some(reason)) if first => return Err(Some((directory, reason.clone()))),
                Err(_) => return Err(None),
            }
        }
        Ok(())
    }
}

/// Why an entry the index records with `kind` cannot be a unit file.
fn unsupported(kind: &EntryKind) -> SrcEntryErrorReason {
    match kind {
        EntryKind::SymbolicLink => SrcEntryErrorReason::SymbolicLink,
        EntryKind::Submodule => SrcEntryErrorReason::Submodule,
        EntryKind::Other(mode) => SrcEntryErrorReason::UnsupportedMode(mode.clone()),
        EntryKind::File { .. } | EntryKind::Directory => SrcEntryErrorReason::NotARegularFile,
    }
}

fn regular_file(metadata: &Metadata) -> Result<(), SrcEntryErrorReason> {
    if metadata.file_type().is_symlink() {
        Err(SrcEntryErrorReason::SymbolicLink)
    } else if metadata.is_file() {
        Ok(())
    } else {
        Err(SrcEntryErrorReason::NotARegularFile)
    }
}

#[cfg(unix)]
fn executable(metadata: &Metadata, _indexed: bool) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o100 != 0
}

#[cfg(not(unix))]
fn executable(_metadata: &Metadata, indexed: bool) -> bool {
    indexed
}

fn unreadable(error: &io::Error) -> SrcEntryErrorReason {
    SrcEntryErrorReason::Unreadable(error.to_string())
}

/// Reads a regular file, at most [`FILE_LIMIT`] bytes of it.
fn read_bounded(path: &Path) -> Result<Vec<u8>, SrcEntryErrorReason> {
    let mut contents = Vec::new();
    File::open(path)
        .and_then(|file| file.take(FILE_LIMIT + 1).read_to_end(&mut contents))
        .map_err(|error| unreadable(&error))?;
    let size = contents.len() as u64;
    if size > FILE_LIMIT {
        return Err(SrcEntryErrorReason::TooLarge {
            size,
            limit: FILE_LIMIT,
        });
    }
    Ok(contents)
}
