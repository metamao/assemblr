//! References to local unit repositories.
//!
//! A path is made absolute against the working directory and normalized
//! lexically, without consulting the file system, so symbolic links are kept
//! as given. The result is compared exactly afterwards (see ADR 0007).

use std::fmt;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::domain::SourceReference;

/// The reference of the local repository at `path`: absolute, with `.` and
/// `..` resolved by name.
///
/// Remote references, such as `https://…` URLs and `user@host:path`, are not
/// supported yet, and neither are paths that are not UTF-8.
pub fn local_reference(path: &Path) -> Result<SourceReference, ReferenceError> {
    if path.as_os_str().is_empty() {
        return Err(ReferenceError::Empty);
    }
    if let Some(text) = path.to_str() {
        if is_remote(text) {
            return Err(ReferenceError::Remote(text.to_owned()));
        }
    }
    let absolute = std::path::absolute(path).map_err(ReferenceError::WorkingDirectory)?;
    let normalized = normalize(&absolute);
    normalized
        .to_str()
        .map(SourceReference::new)
        .ok_or(ReferenceError::NotUtf8(normalized))
}

/// Whether a reference names a remote repository, the way Git reads it: a
/// URL with a scheme, or the `host:path` form, whose colon comes before any
/// slash. A single letter before the colon is a Windows drive.
fn is_remote(text: &str) -> bool {
    if text.contains("://") {
        return true;
    }
    let Some(colon) = text.find(':') else {
        return false;
    };
    let before = &text[..colon];
    before.len() > 1 && !before.contains(['/', '\\'])
}

/// Resolves `.` and `..` by name. `..` at the root stays at the root.
fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    normalized.pop();
                }
            }
            other => normalized.push(other),
        }
    }
    normalized
}

/// Whether `reference` is in the form [`local_reference`] gives: absolute
/// and already normalized.
pub(super) fn is_local_reference(reference: &str) -> bool {
    let path = Path::new(reference);
    path.is_absolute() && normalize(path).as_os_str() == path.as_os_str()
}

/// Why a path cannot be a registry reference.
#[derive(Debug)]
pub enum ReferenceError {
    /// The path is empty.
    Empty,
    /// The reference names a remote repository.
    Remote(String),
    /// The working directory could not be read to make the path absolute.
    WorkingDirectory(io::Error),
    /// The absolute path is not UTF-8.
    NotUtf8(PathBuf),
}

impl fmt::Display for ReferenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("the repository path is empty"),
            Self::Remote(reference) => write!(
                f,
                "{reference} is a remote repository; only local repositories can be \
                 registered for now, so give the path of a local clone"
            ),
            Self::WorkingDirectory(error) => {
                write!(f, "could not read the working directory: {error}")
            }
            Self::NotUtf8(path) => write!(
                f,
                "{} is not valid UTF-8, which registry references must be",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ReferenceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::WorkingDirectory(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{ReferenceError, is_local_reference, is_remote, local_reference, normalize};

    #[test]
    fn remote_references_are_recognized_as_git_does() {
        for remote in [
            "https://github.com/acme/node.git",
            "ssh://git@github.com/acme/node.git",
            "file:///srv/node.git",
            "git@github.com:acme/node.git",
            "host:node",
        ] {
            assert!(is_remote(remote), "{remote}");
        }
        for local in [
            "node",
            "./node",
            "../units/node",
            "/srv/units/node",
            "./host:node",
            "C:\\units\\node",
            "C:/units/node",
            "dir/host:node",
        ] {
            assert!(!is_remote(local), "{local}");
        }
    }

    #[test]
    fn remote_references_are_rejected() {
        assert!(matches!(
            local_reference(Path::new("git@github.com:acme/node.git")),
            Err(ReferenceError::Remote(_))
        ));
        assert!(matches!(
            local_reference(Path::new("")),
            Err(ReferenceError::Empty)
        ));
    }

    #[test]
    fn dot_components_are_resolved_by_name() {
        let root = std::path::absolute(Path::new("/")).unwrap();
        let path = root
            .join("units")
            .join(".")
            .join("old")
            .join("..")
            .join("node");
        assert_eq!(normalize(&path), root.join("units").join("node"));
        assert_eq!(
            normalize(&root.join("..").join("units")),
            root.join("units")
        );
    }

    #[test]
    fn relative_paths_are_made_absolute_against_the_working_directory() {
        let reference = local_reference(Path::new("units/./node/../web/")).unwrap();
        let expected = std::env::current_dir().unwrap().join("units").join("web");

        assert_eq!(Path::new(reference.as_str()), expected);
        assert!(is_local_reference(reference.as_str()));
        assert!(!is_local_reference("units/web"));
    }
}
