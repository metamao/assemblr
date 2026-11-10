//! References to unit repositories.
//!
//! A local path is made absolute against the working directory and
//! normalized lexically, without consulting the file system, so symbolic
//! links are kept as given. A remote reference is kept exactly as given,
//! once its transport is known to be one Assemblr fetches with. Either is
//! compared exactly afterwards (see ADR 0007 and ADR 0029), except where
//! `unit publish` looks for its own registration ([`registered_reference`]).

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::domain::{RegisteredUnit, RegistryIndex, SourceReference};

/// The reference of the repository `given`: a remote reference as
/// [`remote_reference`] accepts it, or the reference of a local path, as
/// [`local_reference`] makes it.
pub fn source_reference(given: &Path) -> Result<SourceReference, ReferenceError> {
    match given.to_str() {
        // Git reads `name:path` as SSH even when a directory of that name
        // exists, which is rarely what the user means.
        Some(text) if is_remote(text) && !text.contains("://") && given.exists() => {
            Err(ReferenceError::Ambiguous(text.to_owned()))
        }
        Some(text) if is_remote(text) => remote_reference(text),
        _ => local_reference(given),
    }
}

/// A remote reference, exactly as given: an `https://`, `ssh://`, or
/// `file://` URL, or the scp-like `[user@]host:path` that Git reads as SSH.
///
/// Every other transport is refused, Git's `<transport>::<address>` helpers
/// above all, since `ext::` runs a command. So is an `https://` URL with
/// user information, which would store a credential in the index, and a
/// reference Git could read as an option.
pub(super) fn remote_reference(text: &str) -> Result<SourceReference, ReferenceError> {
    let unsupported = || Err(ReferenceError::Transport(text.to_owned()));
    if text.starts_with('-') || text.chars().any(char::is_control) {
        return unsupported();
    }
    if let Some((scheme, rest)) = text.split_once("://") {
        match scheme {
            "https" => {
                let authority = rest.split('/').next().unwrap_or_default();
                if authority.contains('@') {
                    return Err(ReferenceError::Credentials(text.to_owned()));
                }
            }
            "ssh" | "file" => {}
            _ => return unsupported(),
        }
        if rest.is_empty() {
            return unsupported();
        }
    } else {
        // `host:path`, which `is_remote` recognized. `<transport>::…` is a
        // remote helper, never SSH.
        let colon = text.find(':').unwrap_or_default();
        if text[colon..].starts_with("::") || colon + 1 == text.len() {
            return unsupported();
        }
    }
    Ok(SourceReference::new(text))
}

/// Whether a stored reference names a remote repository.
pub(crate) fn is_remote_reference(reference: &SourceReference) -> bool {
    is_remote(reference.as_str())
}

/// The reference of the local repository at `path`: absolute, with `.` and
/// `..` resolved by name.
///
/// A remote reference, such as an `https://…` URL or `user@host:path`, is an
/// error here, and so is a path that is not UTF-8.
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

/// The reference `index` registers the local repository at `root` by: the
/// reference of `root` itself when that is registered, and otherwise the one
/// registered local source that is the same directory, reached through a
/// symbolic link or spelled in another letter case, as
/// `registries-and-discovery.md` allows for `unit publish` only.
/// When neither is registered, the reference of `root`.
///
/// It lets `unit publish` recognize its own registration whichever path
/// the working directory reports. Stored references are still compared
/// exactly everywhere else, source conflicts included.
pub(crate) fn registered_reference(
    index: &RegistryIndex,
    root: &Path,
) -> Result<SourceReference, ReferenceError> {
    let reference = local_reference(root)?;
    if !index.identities_of(&reference).is_empty() {
        return Ok(reference);
    }
    let Ok(directory) = fs::canonicalize(root) else {
        return Ok(reference);
    };
    let same: BTreeSet<&SourceReference> = index
        .units()
        .values()
        .map(RegisteredUnit::source)
        .filter(|source| is_local_reference(source.as_str()))
        .filter(|source| {
            fs::canonicalize(source.as_str()).is_ok_and(|resolved| resolved == directory)
        })
        .collect();
    let mut same = same.into_iter();
    match (same.next(), same.next()) {
        (Some(only), None) => Ok(only.clone()),
        // Two registered references to one directory cannot both be meant;
        // neither is chosen.
        _ => Ok(reference),
    }
}

/// Whether a reference names a remote repository, the way Git reads it: a
/// URL with a scheme, or the `host:path` form, whose colon comes before any
/// slash. A single letter before the colon is a Windows drive.
pub(super) fn is_remote(text: &str) -> bool {
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
    /// The reference names a remote repository where only a local one is
    /// accepted.
    Remote(String),
    /// The reference reads as `host:path`, but a local path of that name
    /// exists.
    Ambiguous(String),
    /// The remote reference uses a transport Assemblr does not fetch with.
    Transport(String),
    /// The remote reference carries user information, such as a token.
    Credentials(String),
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
                "{reference} reads as a remote repository reference, where a local path is \
                 needed; start the path with `./` if a directory name before its first `/` \
                 contains `:`"
            ),
            Self::Ambiguous(reference) => write!(
                f,
                "{reference} is a local path, but Git reads it as the SSH address \
                 `host:path`; write it starting with `./` to register the local repository"
            ),
            Self::Transport(reference) => write!(
                f,
                "{reference} is not a repository reference Assemblr can fetch; use an \
                 `https://` or `ssh://` URL, `git@host:path`, or a local path"
            ),
            Self::Credentials(reference) => write!(
                f,
                "{reference} contains credentials, which would be stored in the registry; \
                 remove them and let Git's credential helper or SSH supply them"
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

    use super::{
        ReferenceError, is_local_reference, is_remote, local_reference, normalize,
        remote_reference, source_reference,
    };

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
    fn windows_drive_and_share_paths_are_local() {
        for local in [
            "C:\\projects\\unit",
            "C:/projects/unit",
            "c:\\projects\\unit",
            "C:projects\\unit",
            "\\\\server\\share\\unit",
            "//server/share/unit",
        ] {
            assert!(!is_remote(local), "{local}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_are_stored_absolute_with_backslashes() {
        for given in [
            "C:\\projects\\unit",
            "C:/projects/unit",
            "C:\\projects\\.\\old\\..\\unit\\",
        ] {
            let reference = local_reference(Path::new(given)).unwrap();
            assert_eq!(reference.as_str(), "C:\\projects\\unit", "{given}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_colon_after_a_leading_dot_slash_is_part_of_a_local_path() {
        let reference = local_reference(Path::new("./host:repo")).unwrap();
        let expected = std::env::current_dir().unwrap().join("host:repo");

        assert_eq!(Path::new(reference.as_str()), expected);
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

    #[test]
    fn remote_references_are_kept_exactly_as_given() {
        for remote in [
            "https://github.com/acme/node.git",
            "https://github.com/acme/node",
            "ssh://git@github.com/acme/node.git",
            "git@github.com:acme/node.git",
            "github.com:acme/node",
            "file:///srv/units/node.git",
        ] {
            let reference = source_reference(Path::new(remote)).unwrap();
            assert_eq!(reference.as_str(), remote);
        }
    }

    #[test]
    fn transports_that_are_unsafe_or_unauthenticated_are_refused() {
        for refused in [
            "ext::sh -c touch% /tmp/pwned",
            "fd::17",
            "http://github.com/acme/node.git",
            "git://github.com/acme/node.git",
            "ftp://example.com/node.git",
            "-uhost:node",
            "host:",
            "https://",
            "https://github.com/acme/node\ngit",
        ] {
            assert!(
                matches!(remote_reference(refused), Err(ReferenceError::Transport(_))),
                "{refused}"
            );
        }
    }

    #[test]
    fn credentials_in_https_references_are_refused() {
        for refused in [
            "https://token@github.com/acme/node.git",
            "https://me:secret@github.com/acme/node.git",
        ] {
            assert!(
                matches!(
                    remote_reference(refused),
                    Err(ReferenceError::Credentials(_))
                ),
                "{refused}"
            );
        }
        // SSH names the account to log in as, which is not a secret.
        assert!(remote_reference("ssh://git@github.com/acme/node.git").is_ok());
    }

    /// The reference each of `paths` is registered by, one identity each.
    #[cfg(unix)]
    fn registered(paths: &[(&str, &Path)]) -> crate::domain::RegistryIndex {
        use std::collections::BTreeMap;

        use crate::domain::{RegisteredUnit, RegistryIndex, UnitIdentity};

        RegistryIndex::from_units(
            paths
                .iter()
                .map(|(identity, path)| {
                    (
                        UnitIdentity::parse(identity).unwrap(),
                        RegisteredUnit::new(local_reference(path).unwrap(), BTreeMap::new()),
                    )
                })
                .collect(),
        )
        .unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn a_registration_is_found_through_symbolic_links_only_when_unambiguous() {
        use std::os::unix::fs::symlink;

        use super::registered_reference;

        let dir = std::env::temp_dir().join(format!(
            "assemblr-registered-reference-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let unit = dir.join("unit");
        std::fs::create_dir_all(&unit).unwrap();
        let (one, two) = (dir.join("one"), dir.join("two"));
        symlink(&unit, &one).unwrap();
        symlink(&unit, &two).unwrap();
        let reference = |path: &Path| local_reference(path).unwrap();

        // Through a link, the registered reference is found.
        let index = registered(&[("@acme/unit", &one)]);
        assert_eq!(
            registered_reference(&index, &unit).unwrap(),
            reference(&one)
        );
        // An exact registration wins over one through a link.
        let index = registered(&[("@acme/a", &one), ("@acme/b", &unit)]);
        assert_eq!(
            registered_reference(&index, &unit).unwrap(),
            reference(&unit)
        );
        // Two links to the directory: neither is chosen.
        let index = registered(&[("@acme/a", &one), ("@acme/b", &two)]);
        assert_eq!(
            registered_reference(&index, &unit).unwrap(),
            reference(&unit)
        );
        // Nothing registered: the directory's own reference.
        let index = registered(&[]);
        assert_eq!(registered_reference(&index, &one).unwrap(), reference(&one));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
