//! Which of a project's paths its version control ignores, and by whose
//! rule.
//!
//! A composition stores the reconciliation base in the project, where it is
//! tracked. A delivered file the consumer's Git ignores, such as a `.env`,
//! must not reach that tracked store through the back door, so the
//! composition asks Git which planned paths are ignored. Git reads only
//! here; nothing is staged or written. A project outside any repository
//! ignores nothing.
//!
//! Two kinds of rule answer, and they are told apart, because only one of
//! them travels with the project:
//!
//! - **project rules** are the repository's `.gitignore` files. They are
//!   committed, so every clone applies the same ones, and they decide which
//!   files are *unbased*, a fact the lockfile records;
//! - **local rules** are `.git/info/exclude` and the user's global
//!   excludes. They belong to one machine. They never decide what the
//!   lockfile records, since the same project would then lock differently
//!   on two machines, but they are the consumer's own safety preference, so
//!   a file only they ignore is reported (see `Ignores::local`) and never
//!   stored in the tracked base without the project saying so.
//!
//! Git's precedence makes the split exact: a `.gitignore` match wins over
//! `info/exclude`, which wins over the global excludes, and `check-ignore
//! --verbose` names the file of the winning pattern.

use std::collections::{BTreeMap, BTreeSet};
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

/// What Git's rules say about a set of paths.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Ignores {
    project: BTreeSet<String>,
    local: BTreeMap<String, LocalRule>,
}

/// The local rule that ignores a path that no project rule ignores.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LocalRule {
    source: String,
    pattern: String,
}

impl LocalRule {
    /// The file the rule is in: `.git/info/exclude`, or the global
    /// excludes file.
    pub(super) fn source(&self) -> &str {
        &self.source
    }

    /// The pattern, as written.
    pub(super) fn pattern(&self) -> &str {
        &self.pattern
    }
}

impl Ignores {
    /// Whether a project rule, a `.gitignore` file, ignores `path`.
    pub(super) fn is_project(&self, path: &RelativePath) -> bool {
        self.project.contains(path.as_str())
    }

    /// The local rule that ignores `path` when no project rule does.
    pub(super) fn local(&self, path: &RelativePath) -> Option<&LocalRule> {
        self.local.get(path.as_str())
    }

    /// Whether any rule, project or local, ignores `path`, which is how a
    /// directory is named, with its trailing slash.
    pub(super) fn any(&self, path: &str) -> bool {
        self.project.contains(path) || self.local.contains_key(path)
    }
}

/// What the rules of the repository `root` is in say about `paths`,
/// relative to `root`, or nothing when `root` is in no repository.
pub(super) fn ignored(root: &Path, paths: &[&str]) -> Result<Ignores, IgnoreError> {
    if paths.is_empty() {
        return Ok(Ignores::default());
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
        .args(["--no-pager", "check-ignore", "--verbose", "-z", "--stdin"])
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
        Some(128) if stderr.contains("not a git repository") => return Ok(Ignores::default()),
        _ => {
            return Err(IgnoreError::Failed(stderr.trim().to_owned()));
        }
    }
    Ok(parse(&output.stdout))
}

/// Reads the output of `git check-ignore --verbose -z`: for each path
/// that some pattern matches, the file of the pattern, its line number,
/// the pattern, and the path, each ended by a NUL. A pattern that starts
/// with `!` re-includes the path, which is then not ignored.
fn parse(stdout: &[u8]) -> Ignores {
    let fields: Vec<String> = stdout
        .split(|&byte| byte == 0)
        .map(|field| String::from_utf8_lossy(field).into_owned())
        .collect();
    let mut ignores = Ignores::default();
    for record in fields.chunks_exact(4) {
        let [source, _line, pattern, path] = record else {
            continue;
        };
        if pattern.starts_with('!') {
            continue;
        }
        if is_project_source(source) {
            ignores.project.insert(path.clone());
        } else {
            ignores.local.insert(
                path.clone(),
                LocalRule {
                    source: source.clone(),
                    pattern: pattern.clone(),
                },
            );
        }
    }
    ignores
}

/// Whether the pattern's file is a `.gitignore` of the repository, which
/// Git names relative to the top level, and not `.git/info/exclude` or a
/// global excludes file, whose paths are inside the Git directory or
/// absolute.
fn is_project_source(source: &str) -> bool {
    let path = Path::new(source);
    !path.is_absolute()
        && !source.starts_with('/')
        && !source.starts_with(".git/")
        && path.file_name().is_some_and(|name| name == ".gitignore")
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

#[cfg(test)]
mod tests {
    use super::{LocalRule, parse};

    fn record(source: &str, pattern: &str, path: &str) -> Vec<u8> {
        format!("{source}\0{}\0{pattern}\0{path}\0", 1).into_bytes()
    }

    #[test]
    fn rules_are_told_apart_by_the_file_of_the_winning_pattern() {
        let mut output = Vec::new();
        output.extend(record(".gitignore", ".env", ".env"));
        output.extend(record("sub/.gitignore", "*.tmp", "sub/a.tmp"));
        output.extend(record(".git/info/exclude", "mine.txt", "mine.txt"));
        output.extend(record(
            "/home/me/.config/git/ignore",
            ".vscode/",
            ".vscode/a.json",
        ));
        output.extend(record("C:/Users/me/.gitignore_global", "*.log", "a.log"));
        // A global file that happens to be named `.gitignore` is still the
        // user's, since its path is absolute.
        output.extend(record("/home/me/.gitignore", "notes", "notes"));
        // A re-included path is not ignored, whatever else matches.
        output.extend(record(".gitignore", "!keep.log", "keep.log"));

        let ignores = parse(&output);

        let project: Vec<&str> = ignores.project.iter().map(String::as_str).collect();
        assert_eq!(project, [".env", "sub/a.tmp"]);
        let local: Vec<(&str, &str)> = ignores
            .local
            .iter()
            .map(|(path, rule): (&String, &LocalRule)| (path.as_str(), rule.source()))
            .collect();
        assert_eq!(
            local,
            [
                (".vscode/a.json", "/home/me/.config/git/ignore"),
                ("a.log", "C:/Users/me/.gitignore_global"),
                ("mine.txt", ".git/info/exclude"),
                ("notes", "/home/me/.gitignore"),
            ]
        );
        assert!(!ignores.any("keep.log"));
        assert!(ignores.any(".env") && ignores.any("mine.txt"));
    }

    #[test]
    fn nothing_matched_is_nothing_ignored_and_a_truncated_record_is_skipped() {
        assert_eq!(parse(b""), super::Ignores::default());
        assert_eq!(parse(b".gitignore\x001\0.env"), super::Ignores::default());
    }
}
