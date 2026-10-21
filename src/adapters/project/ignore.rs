//! Which of a project's paths its version control ignores.
//!
//! A composition stores the reconciliation base in the project, where it is
//! tracked. A delivered file the consumer's Git ignores, such as a `.env`,
//! must not reach that tracked store through the back door, so the
//! composition asks Git which planned paths are ignored, with the
//! consumer's own configuration: the repository's `.gitignore` files, its
//! `info/exclude`, and the user's global excludes all count. Git reads only
//! here; nothing is staged or written. A project outside any repository
//! ignores nothing.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::domain::RelativePath;

/// The variables that would point Git at another repository. The user's
/// `GIT_CEILING_DIRECTORIES` is kept: it bounds the search upwards, which
/// is the user's to decide for a project nested in another repository.
const LOCATION_VARIABLES: [&str; 7] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
];

/// The subset of `paths`, relative to `root`, that the repository `root`
/// is in ignores, or an empty set when `root` is in no repository.
pub(super) fn ignored(root: &Path, paths: &[&str]) -> Result<BTreeSet<String>, IgnoreError> {
    if paths.is_empty() {
        return Ok(BTreeSet::new());
    }
    let mut command = Command::new("git");
    for variable in LOCATION_VARIABLES {
        command.env_remove(variable);
    }
    let mut child = command
        .current_dir(root)
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_ALLOW_PROTOCOL", "none")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .args(["--no-pager", "check-ignore", "-z", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| IgnoreError::Unavailable(error.to_string()))?;
    let mut input = Vec::new();
    for path in paths {
        input.extend_from_slice(path.as_bytes());
        input.push(0);
    }
    if let Some(mut stdin) = child.stdin.take() {
        // Git may exit early; a closed pipe is then not an error here.
        let _ = stdin.write_all(&input);
    }
    let output = child
        .wait_with_output()
        .map_err(|error| IgnoreError::Unavailable(error.to_string()))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    match output.status.code() {
        // 0: some path is ignored; 1: none is.
        Some(0 | 1) => {}
        // 128: not a repository, which ignores nothing. Anything else is
        // a failure to read the rules.
        Some(128) if stderr.contains("not a git repository") => return Ok(BTreeSet::new()),
        _ => {
            return Err(IgnoreError::Failed(stderr.trim().to_owned()));
        }
    }
    Ok(output
        .stdout
        .split(|&byte| byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect())
}

/// Whether `path` is one of `ignored`, by its text.
pub(super) fn is_ignored(ignored: &BTreeSet<String>, path: &RelativePath) -> bool {
    ignored.contains(path.as_str())
}

/// Why the ignore rules could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IgnoreError {
    /// `git` could not be run.
    Unavailable(String),
    /// `git check-ignore` failed.
    Failed(String),
}

impl std::fmt::Display for IgnoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(error) => write!(
                f,
                "could not run `git` to read the project's ignore rules: {error}; Assemblr needs \
                 Git installed and on the PATH"
            ),
            Self::Failed(stderr) => {
                write!(
                    f,
                    "`git check-ignore` failed to read the project's ignore rules: {stderr}"
                )
            }
        }
    }
}

impl std::error::Error for IgnoreError {}
