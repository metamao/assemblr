//! Publishing a build: `build/` and the lockfile, in an order that never
//! leaves a `build/` that looks complete but does not match the lockfile
//! (see `docs/adr/0008-unit-build.md`).
//!
//! Everything is written beside its destination first: the new build into
//! `.assemblr-build.new/`, the new lockfile into `.assemblr-lock.json.new`,
//! each file flushed to disk. Then, by renames:
//!
//! 1. the previous `build/` moves aside to `.assemblr-build.old/`;
//! 2. the new lockfile replaces the old one;
//! 3. the new build becomes `build/`;
//! 4. the previous build is deleted.
//!
//! The two files are not replaced atomically together. Instead, `build/`
//! exists only before step 1, holding the previous build with the previous
//! lockfile, and after step 3, holding the new build with the new lockfile.
//! In between there is no `build/` at all, so an interrupted build leaves
//! none, rather than one that looks complete. A failure is undone where
//! renaming back can undo it, and the error says what state remains. The
//! next build deletes whatever an interrupted one left beside `build/`.

use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use super::lockfile::{LOCKFILE, serialize};
use crate::domain::{RelativePath, UnitFile, UnitLock};

/// The build's output directory, at the repository's top level.
pub(super) const BUILD: &str = "build";
/// Where a new build is written before it becomes `build/`.
const STAGING: &str = ".assemblr-build.new";
/// Where the previous build waits while the new one is put in place.
const RETIRED: &str = ".assemblr-build.old";
/// Where a new lockfile is written before it replaces the old one.
const LOCK_STAGING: &str = ".assemblr-lock.json.new";

/// Checks, before anything is written, that each path a build replaces or
/// deletes is absent or what Assemblr writes there: a real directory, or a
/// regular file for the new lockfile. A symbolic link or any other entry is
/// left alone and reported.
pub(super) fn check(root: &Path) -> Vec<OutputProblem> {
    let mut problems = Vec::new();
    for (name, directory) in [
        (BUILD, true),
        (STAGING, true),
        (RETIRED, true),
        (LOCK_STAGING, false),
    ] {
        let path = root.join(name);
        let found = match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => Found::Unreadable(error),
            Ok(metadata) if metadata.file_type().is_symlink() => Found::SymbolicLink,
            Ok(metadata) if metadata.is_dir() && directory => continue,
            Ok(metadata) if metadata.is_file() && !directory => continue,
            Ok(metadata) if metadata.is_dir() => Found::Directory,
            Ok(metadata) if metadata.is_file() => Found::File,
            Ok(_) => Found::Other,
        };
        problems.push(OutputProblem {
            path,
            expected: if directory {
                Expected::Directory
            } else {
                Expected::File
            },
            found,
        });
    }
    problems
}

/// Publishes `files` as `build/` and, when `lock` is given, writes it as the
/// lockfile. [`check`] must have found nothing.
pub(super) fn publish(
    root: &Path,
    files: &[(&RelativePath, &UnitFile)],
    lock: Option<&UnitLock>,
) -> Result<Published, PublishError> {
    publish_with(root, files, lock, &mut |_| Ok(()))
}

/// The steps of publishing, so that tests can make each one fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    ClearLeftovers,
    Stage,
    StageLock,
    RetireBuild,
    ReplaceLock,
    PlaceBuild,
    RestoreBuild,
    RemoveRetired,
}

fn publish_with(
    root: &Path,
    files: &[(&RelativePath, &UnitFile)],
    lock: Option<&UnitLock>,
    fault: &mut dyn FnMut(Step) -> io::Result<()>,
) -> Result<Published, PublishError> {
    let build = root.join(BUILD);
    let staging = root.join(STAGING);
    let retired = root.join(RETIRED);
    let lockfile = root.join(LOCKFILE);
    let lock_staging = root.join(LOCK_STAGING);
    let failed = |operation, path: &Path, error, state| PublishError {
        operation,
        path: path.to_owned(),
        error,
        state,
    };
    let discard = || {
        // Only tidiness: the next build deletes these anyway.
        let _ = fs::remove_dir_all(&staging);
        let _ = fs::remove_file(&lock_staging);
    };

    // What an interrupted build left. `check` found real directories and a
    // regular file here, or nothing.
    for (leftover, directory) in [(&staging, true), (&retired, true), (&lock_staging, false)] {
        let removed =
            fault(Step::ClearLeftovers).and_then(|()| remove_leftover(leftover, directory));
        if let Err(error) = removed {
            return Err(failed("delete", leftover, error, State::Unchanged));
        }
    }

    if let Err(error) = fault(Step::Stage).and_then(|()| stage(&staging, files)) {
        discard();
        return Err(failed("write", &staging, error, State::Unchanged));
    }
    if let Some(lock) = lock {
        let written = fault(Step::StageLock)
            .and_then(|()| serialize(lock))
            .and_then(|bytes| write_synced(&lock_staging, &bytes));
        if let Err(error) = written {
            discard();
            return Err(failed("write", &lock_staging, error, State::Unchanged));
        }
    }

    let had_build = fs::symlink_metadata(&build).is_ok();
    if had_build {
        if let Err(error) = fault(Step::RetireBuild).and_then(|()| fs::rename(&build, &retired)) {
            discard();
            return Err(failed("move aside", &build, error, State::Unchanged));
        }
    }
    // Puts the previous build back, while it still matches the lockfile.
    let restore = |fault: &mut dyn FnMut(Step) -> io::Result<()>| {
        let restored = !had_build
            || fault(Step::RestoreBuild)
                .and_then(|()| fs::rename(&retired, &build))
                .is_ok();
        if restored {
            State::Unchanged
        } else {
            State::BuildRemoved
        }
    };

    if lock.is_some() {
        if let Err(error) =
            fault(Step::ReplaceLock).and_then(|()| fs::rename(&lock_staging, &lockfile))
        {
            let state = restore(fault);
            discard();
            return Err(failed("replace", &lockfile, error, state));
        }
    }

    if let Err(error) = fault(Step::PlaceBuild).and_then(|()| fs::rename(&staging, &build)) {
        let state = if lock.is_some() {
            // The previous build no longer matches the lockfile, so it is
            // not put back.
            let _ = fs::remove_dir_all(&retired);
            State::LockfileWithoutBuild
        } else {
            restore(fault)
        };
        discard();
        return Err(failed("create", &build, error, state));
    }
    sync_directory(root);

    let leftover = if had_build {
        fault(Step::RemoveRetired)
            .and_then(|()| fs::remove_dir_all(&retired))
            .err()
            .map(|error| (retired, error))
    } else {
        None
    };
    Ok(Published { leftover })
}

/// Deletes `path` if it exists: a directory with everything in it, or a
/// file. Anything else is left alone.
fn remove_leftover(path: &Path, directory: bool) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
        Ok(metadata) if metadata.file_type().is_symlink() => Err(not_owned()),
        Ok(metadata) if directory && metadata.is_dir() => fs::remove_dir_all(path),
        Ok(metadata) if !directory && metadata.is_file() => fs::remove_file(path),
        Ok(_) => Err(not_owned()),
    }
}

fn not_owned() -> io::Error {
    io::Error::other("it is not what Assemblr writes there, so it is left as it is")
}

/// Writes `files` into `staging`, which must not exist, and flushes every
/// file and directory to disk.
fn stage(staging: &Path, files: &[(&RelativePath, &UnitFile)]) -> io::Result<()> {
    // A new directory: nothing in it can be a symbolic link out of it.
    fs::create_dir(staging)?;
    let mut directories = BTreeSet::from([staging.to_owned()]);
    for (path, file) in files {
        let target = staging.join(path.as_str());
        let mut parent = target.parent();
        while let Some(directory) = parent.filter(|directory| *directory != staging) {
            if !directories.insert(directory.to_owned()) {
                break;
            }
            parent = directory.parent();
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        write_file(&target, file)?;
    }
    sync_directories(&directories)
}

fn write_file(target: &Path, file: &UnitFile) -> io::Result<()> {
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)?;
    out.write_all(file.contents())?;
    #[cfg(unix)]
    if file.is_executable() {
        use std::os::unix::fs::PermissionsExt;
        // Executable by whoever may read it, as Git checks files out.
        let mode = out.metadata()?.permissions().mode();
        out.set_permissions(fs::Permissions::from_mode(mode | ((mode & 0o444) >> 2)))?;
    }
    out.sync_all()
}

fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Flushes the entries of each directory, so that the files written into
/// them survive a crash. Windows has no such operation.
#[cfg(unix)]
fn sync_directories(directories: &BTreeSet<PathBuf>) -> io::Result<()> {
    for directory in directories {
        File::open(directory)?.sync_all()?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn sync_directories(_directories: &BTreeSet<PathBuf>) -> io::Result<()> {
    Ok(())
}

/// Makes the renames in `root` durable, best effort: they have already
/// happened, so a failure here is not a failed build.
fn sync_directory(root: &Path) {
    #[cfg(unix)]
    let _ = File::open(root).and_then(|dir| dir.sync_all());
    #[cfg(not(unix))]
    let _ = root;
}

/// A published build.
#[derive(Debug)]
pub(super) struct Published {
    /// The previous build, if it could not be deleted, and why.
    pub(super) leftover: Option<(PathBuf, io::Error)>,
}

/// A path a build would replace or delete that is not what Assemblr writes
/// there.
#[derive(Debug)]
pub struct OutputProblem {
    path: PathBuf,
    expected: Expected,
    found: Found,
}

#[derive(Debug, Clone, Copy)]
enum Expected {
    Directory,
    File,
}

#[derive(Debug)]
enum Found {
    SymbolicLink,
    Directory,
    File,
    Other,
    Unreadable(io::Error),
}

impl OutputProblem {
    /// The path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl fmt::Display for OutputProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let path = self.path.display();
        let found = match &self.found {
            Found::Unreadable(error) => {
                return write!(f, "could not inspect {path}: {error}");
            }
            Found::SymbolicLink => "a symbolic link",
            Found::Directory => "a directory",
            Found::File => "a regular file",
            Found::Other => "a special file",
        };
        let expected = match self.expected {
            Expected::Directory => "only a directory it created",
            Expected::File => "only a file it created",
        };
        write!(
            f,
            "{path} is {found}; a build replaces {expected} there, so it leaves this one \
             as it is; move or delete it"
        )
    }
}

impl std::error::Error for OutputProblem {}

/// What remains after a failed publication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// `build/` and the lockfile are as they were.
    Unchanged,
    /// The previous `build/` was removed. The lockfile is as it was.
    BuildRemoved,
    /// The lockfile was updated, and there is no `build/`.
    LockfileWithoutBuild,
}

/// A failure while publishing a build.
#[derive(Debug)]
pub struct PublishError {
    operation: &'static str,
    path: PathBuf,
    error: io::Error,
    state: State,
}

impl PublishError {
    /// What remains of `build/` and the lockfile.
    pub fn state(&self) -> State {
        self.state
    }
}

impl fmt::Display for PublishError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "could not {} {}: {}; ",
            self.operation,
            self.path.display(),
            self.error
        )?;
        f.write_str(match self.state {
            State::Unchanged => "build/ and assemblr-lock.json were not changed",
            State::BuildRemoved => {
                "the previous build/ was removed, and assemblr-lock.json was not changed"
            }
            State::LockfileWithoutBuild => {
                "assemblr-lock.json was updated, but build/ was not created"
            }
        })
    }
}

impl std::error::Error for PublishError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};

    use super::{
        BUILD, LOCK_STAGING, PublishError, Published, RETIRED, STAGING, State, Step, check,
        publish_with,
    };
    use crate::adapters::build::lockfile::{LOCKFILE, read};
    use crate::domain::{
        FileTree, LockedUnit, ObjectId, Provenance, RelativePath, UnitFile, UnitLock,
    };

    /// An empty directory for one test.
    fn scratch(test: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("assemblr-output-{}", std::process::id()))
            .join(test);
        if dir.exists() {
            fs::remove_dir_all(&dir).unwrap();
        }
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn lock(version: &str) -> UnitLock {
        let id = ObjectId::parse(&"1".repeat(40)).unwrap();
        UnitLock::new(BTreeMap::from([(
            "@acme/node".parse().unwrap(),
            LockedUnit::new(version.parse().unwrap(), Provenance::new(id.clone(), id)),
        )]))
    }

    fn tree(files: &[(&str, &str)]) -> FileTree {
        files
            .iter()
            .map(|(path, contents)| {
                (
                    path.parse::<RelativePath>().unwrap(),
                    UnitFile::new(contents.as_bytes().to_vec(), false),
                )
            })
            .collect()
    }

    /// Publishes `files` and `lock` into `root`, failing at `failing`.
    fn publish(
        root: &Path,
        files: &[(&str, &str)],
        lock: Option<&UnitLock>,
        failing: &[Step],
    ) -> Result<Published, PublishError> {
        let tree = tree(files);
        let files: Vec<_> = tree.iter().collect();
        publish_with(root, &files, lock, &mut |step| {
            if failing.contains(&step) {
                Err(io::Error::other(format!("injected failure at {step:?}")))
            } else {
                Ok(())
            }
        })
    }

    /// The files under `dir`, by path, with their contents.
    fn contents(dir: &Path) -> Vec<(String, String)> {
        let mut found = Vec::new();
        let mut pending = vec![dir.to_owned()];
        while let Some(next) = pending.pop() {
            for entry in fs::read_dir(next).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    let relative = path.strip_prefix(dir).unwrap();
                    found.push((
                        relative.to_string_lossy().replace('\\', "/"),
                        fs::read_to_string(&path).unwrap(),
                    ));
                }
            }
        }
        found.sort();
        found
    }

    /// The repository's top level, ignoring nothing.
    fn top_level(root: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// A repository holding a previous build and its lockfile.
    fn previous(test: &str) -> PathBuf {
        let root = scratch(test);
        publish(&root, &[("old.txt", "old")], Some(&lock("1.0.0")), &[]).unwrap();
        root
    }

    fn old_build() -> Vec<(String, String)> {
        vec![("old.txt".to_owned(), "old".to_owned())]
    }

    fn new_build() -> Vec<(String, String)> {
        vec![
            ("a/b.txt".to_owned(), "b".to_owned()),
            ("new.txt".to_owned(), "new".to_owned()),
        ]
    }

    const NEW: &[(&str, &str)] = &[("new.txt", "new"), ("a/b.txt", "b")];

    #[test]
    fn a_build_replaces_build_and_the_lockfile_and_leaves_nothing_else() {
        let root = previous("replaces");
        publish(&root, NEW, Some(&lock("1.1.0")), &[]).unwrap();
        assert_eq!(contents(&root.join(BUILD)), new_build());
        assert_eq!(read(&root).unwrap(), Some(lock("1.1.0")));
        assert_eq!(top_level(&root), [LOCKFILE, BUILD]);
    }

    #[test]
    fn without_a_new_lock_the_lockfile_is_not_touched() {
        let root = previous("no-lock");
        let before = fs::read(root.join(LOCKFILE)).unwrap();
        publish(&root, NEW, None, &[]).unwrap();
        assert_eq!(contents(&root.join(BUILD)), new_build());
        assert_eq!(fs::read(root.join(LOCKFILE)).unwrap(), before);
    }

    /// Every failure that leaves the previous build and lockfile in place.
    #[test]
    fn a_failure_before_the_lockfile_is_replaced_changes_nothing() {
        for step in [
            Step::ClearLeftovers,
            Step::Stage,
            Step::StageLock,
            Step::RetireBuild,
            Step::ReplaceLock,
        ] {
            let root = previous(&format!("unchanged-{step:?}"));
            let error = publish(&root, NEW, Some(&lock("1.1.0")), &[step]).unwrap_err();
            assert_eq!(error.state(), State::Unchanged, "{step:?}");
            assert!(
                error.to_string().contains("were not changed"),
                "{step:?}: {error}"
            );
            assert_eq!(contents(&root.join(BUILD)), old_build(), "{step:?}");
            assert_eq!(read(&root).unwrap(), Some(lock("1.0.0")), "{step:?}");
            assert_eq!(top_level(&root), [LOCKFILE, BUILD], "{step:?}");
        }
    }

    #[test]
    fn a_lockfile_that_cannot_be_replaced_or_restored_leaves_no_build() {
        let root = previous("removed");
        let error = publish(
            &root,
            NEW,
            Some(&lock("1.1.0")),
            &[Step::ReplaceLock, Step::RestoreBuild],
        )
        .unwrap_err();
        assert_eq!(error.state(), State::BuildRemoved);
        assert!(!root.join(BUILD).exists());
        assert_eq!(read(&root).unwrap(), Some(lock("1.0.0")));
    }

    #[test]
    fn a_build_that_cannot_be_placed_after_the_lockfile_leaves_no_build() {
        let root = previous("lockfile-only");
        let error = publish(&root, NEW, Some(&lock("1.1.0")), &[Step::PlaceBuild]).unwrap_err();
        assert_eq!(error.state(), State::LockfileWithoutBuild);
        assert!(
            error
                .to_string()
                .contains("assemblr-lock.json was updated, but build/ was not created"),
            "{error}"
        );
        // The previous build does not match the new lockfile, so it is not
        // put back.
        assert!(!root.join(BUILD).exists());
        assert_eq!(read(&root).unwrap(), Some(lock("1.1.0")));
    }

    #[test]
    fn a_build_that_cannot_be_placed_without_a_lock_change_restores_the_previous_one() {
        let root = previous("restored");
        let error = publish(&root, NEW, None, &[Step::PlaceBuild]).unwrap_err();
        assert_eq!(error.state(), State::Unchanged);
        assert_eq!(contents(&root.join(BUILD)), old_build());
    }

    #[test]
    fn a_previous_build_that_cannot_be_deleted_is_reported_and_deleted_next_time() {
        let root = previous("leftover");
        let published = publish(&root, NEW, Some(&lock("1.1.0")), &[Step::RemoveRetired]).unwrap();
        assert_eq!(published.leftover.unwrap().0, root.join(RETIRED));
        assert_eq!(contents(&root.join(BUILD)), new_build());
        publish(&root, NEW, None, &[]).unwrap();
        assert_eq!(top_level(&root), [LOCKFILE, BUILD]);
    }

    #[test]
    fn what_an_interrupted_build_left_is_deleted_by_the_next() {
        // As a build interrupted after replacing the lockfile leaves it: no
        // `build/`, the previous one aside, and the new one half written.
        let root = scratch("interrupted");
        for leftover in [STAGING, RETIRED] {
            fs::create_dir_all(root.join(leftover).join("nested")).unwrap();
            fs::write(root.join(leftover).join("nested/file.txt"), "partial").unwrap();
        }
        fs::write(root.join(LOCK_STAGING), "{").unwrap();
        assert!(check(&root).is_empty());
        publish(&root, NEW, Some(&lock("1.1.0")), &[]).unwrap();
        assert_eq!(contents(&root.join(BUILD)), new_build());
        assert_eq!(top_level(&root), [LOCKFILE, BUILD]);
    }

    #[test]
    fn paths_a_build_would_replace_must_be_what_it_writes_there() {
        let root = scratch("check");
        fs::write(root.join(BUILD), "a file").unwrap();
        fs::create_dir(root.join(LOCK_STAGING)).unwrap();
        let problems = check(&root);
        let messages: Vec<String> = problems.iter().map(ToString::to_string).collect();
        assert_eq!(problems.len(), 2, "{messages:?}");
        assert!(messages[0].contains("is a regular file"), "{}", messages[0]);
        assert!(messages[0].contains("move or delete it"), "{}", messages[0]);
        assert!(messages[1].contains("is a directory"), "{}", messages[1]);
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_at_build_is_left_alone() {
        let root = scratch("symlink");
        let elsewhere = scratch("symlink-target");
        fs::write(elsewhere.join("keep.txt"), "keep").unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.join(BUILD)).unwrap();
        let problems = check(&root);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].to_string().contains("symbolic link"));
        assert_eq!(problems[0].path(), root.join(BUILD));
        assert_eq!(
            fs::read_to_string(elsewhere.join("keep.txt")).unwrap(),
            "keep"
        );
    }

    #[cfg(unix)]
    #[test]
    fn executable_files_stay_executable() {
        use std::os::unix::fs::PermissionsExt;
        let root = scratch("executable");
        let script = UnitFile::new(b"#!/bin/sh\n".to_vec(), true);
        let plain = UnitFile::new(b"text".to_vec(), false);
        let (run, readme) = ("run.sh".parse().unwrap(), "README".parse().unwrap());
        super::publish(&root, &[(&run, &script), (&readme, &plain)], None).unwrap();
        let mode = |name: &str| {
            fs::metadata(root.join(BUILD).join(name))
                .unwrap()
                .permissions()
                .mode()
        };
        assert_ne!(mode("run.sh") & 0o100, 0);
        assert_eq!(mode("README") & 0o111, 0);
    }
}
