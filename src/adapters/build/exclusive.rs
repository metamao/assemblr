//! Serializing the builds of one unit, and recognizing what an interrupted
//! build left (see `docs/adr/0008-unit-build.md`).
//!
//! Both files live in the repository's Git directory, which Git ignores and
//! never shows as untracked. `assemblr-build.lock` is locked exclusively for
//! a whole build, as the registry locks its writers (ADR 0007), and is never
//! deleted. `assemblr-build.pending` exists from the moment a build starts
//! writing beside `build/` until it has finished. Its presence is how a later
//! build recognizes leftovers at the reserved paths as an interrupted build's,
//! rather than someone else's content.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The file whose lock serializes builds.
const LOCK: &str = "assemblr-build.lock";
/// The marker of a build that has started writing and not finished.
const PENDING: &str = "assemblr-build.pending";

/// The exclusive right to build a unit, held until it is dropped.
#[derive(Debug)]
pub(super) struct BuildLock {
    _file: File,
    pending: PathBuf,
}

impl BuildLock {
    /// Takes the lock in `git_dir`, creating its file if needed. Waits while
    /// another build of the same repository holds it.
    pub(super) fn acquire(git_dir: &Path) -> Result<Self, BuildLockError> {
        let path = git_dir.join(LOCK);
        let failed = |error| BuildLockError {
            path: path.clone(),
            error,
        };
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(failed)?;
        // Called through the trait: newer standard libraries have an inherent
        // `File::lock`, which the minimum supported Rust does not.
        fs4::FileExt::lock(&file).map_err(failed)?;
        Ok(Self {
            _file: file,
            pending: git_dir.join(PENDING),
        })
    }

    /// Whether a build started writing beside `build/` and did not finish.
    pub(super) fn interrupted(&self) -> io::Result<bool> {
        match fs::symlink_metadata(&self.pending) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Records, durably, that this build is about to write beside `build/`.
    pub(super) fn begin(&self) -> io::Result<()> {
        let mut file = File::create(&self.pending)?;
        file.write_all(b"A build of this repository's unit started writing and did not finish.\n")?;
        file.sync_all()?;
        #[cfg(unix)]
        if let Some(dir) = self.pending.parent() {
            File::open(dir)?.sync_all()?;
        }
        Ok(())
    }

    /// Records that nothing this build wrote beside `build/` remains.
    pub(super) fn finish(&self) -> io::Result<()> {
        fs::remove_file(&self.pending)
    }
}

/// The build lock could not be taken.
#[derive(Debug)]
pub struct BuildLockError {
    path: PathBuf,
    error: io::Error,
}

impl fmt::Display for BuildLockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "could not lock {}, which keeps builds of this unit from running at the same \
             time: {}",
            self.path.display(),
            self.error
        )
    }
}

impl std::error::Error for BuildLockError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}
