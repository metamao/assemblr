//! What `unit publish` needs from the unit's own repository beyond reading
//! published content: the commit at `HEAD`, whether the working tree is
//! committed and clean, and creating the version tag (see
//! `docs/adr/0013-unit-check-and-publish.md`).
//!
//! The clean-state check uses the rules the build reads `src/` with: a
//! tracked change, staged or not, or an untracked file that no `.gitignore`
//! ignores, is uncommitted. `.git/info/exclude` and the user's excludes do
//! not apply, so a file the build composed is never left out of the
//! publication as "ignored".
//!
//! Creating the tag and initializing a repository for `unit init` are the
//! Git commands Assemblr runs that write, and the ones that run with the
//! author's own Git configuration and identity: the tag and the repository
//! are the author's. Network access and prompts stay disabled, and the
//! repository is still named explicitly where one exists.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::command::discovery_command;
use super::{GitError, GitErrorKind, GitRepository};
use crate::domain::ObjectId;

/// The environment variables that name a repository, its object store, or
/// its reference namespace, and would point a write somewhere else.
pub(super) const LOCATION_VARIABLES: [&str; 8] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_CEILING_DIRECTORIES",
];

impl GitRepository {
    /// The commit `HEAD` names, or `None` in a repository with no commit
    /// yet.
    pub fn head_commit(&self) -> Result<Option<ObjectId>, GitError> {
        let mut objects = self.objects()?;
        let head = objects.header("HEAD^{commit}")?.map(|header| header.id);
        objects.finish()?;
        Ok(head)
    }

    /// The paths of the working tree at `work_tree`, the repository's top
    /// level, whose state is not committed: tracked paths with a staged or
    /// unstaged change, paths in conflict, and untracked paths that no
    /// `.gitignore` ignores. Each is given as Git prints it, relative to the
    /// top level, sorted. Empty means committing everything would change
    /// nothing.
    pub fn uncommitted_paths(&self, work_tree: &Path) -> Result<Vec<String>, GitError> {
        let args = [
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=no",
            "--no-renames",
        ];
        let status = self.work_tree_output(work_tree, &args)?;
        let mut paths = Vec::new();
        for record in status.split(|&byte| byte == 0).filter(|r| !r.is_empty()) {
            let unexpected = || {
                self.error(GitErrorKind::UnexpectedOutput {
                    command: format!("git {}", args.join(" ")),
                    detail: format!("unexpected record {:?}", String::from_utf8_lossy(record)),
                })
            };
            // Ordinary changed entries have eight fields before the path,
            // unmerged entries ten. Headers (`#`) are not printed without
            // `--branch`, and `--no-renames` leaves no two-path records.
            let fields = match record.first() {
                Some(b'1') => 8,
                Some(b'u') => 10,
                _ => return Err(unexpected()),
            };
            let mut rest = record;
            for _ in 0..fields {
                let space = rest
                    .iter()
                    .position(|&byte| byte == b' ')
                    .ok_or_else(unexpected)?;
                rest = &rest[space + 1..];
            }
            if rest.is_empty() {
                return Err(unexpected());
            }
            paths.push(String::from_utf8_lossy(rest).into_owned());
        }
        let others = self.work_tree_output(
            work_tree,
            &[
                "ls-files",
                "-z",
                "--others",
                "--exclude-per-directory=.gitignore",
            ],
        )?;
        paths.extend(
            others
                .split(|&byte| byte == 0)
                .filter(|path| !path.is_empty())
                .map(|path| String::from_utf8_lossy(path).into_owned()),
        );
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    /// Creates the annotated tag `name` with `message` at `commit`, with
    /// the author's own Git configuration, so that the tagger is the author
    /// and signing follows the author's settings. The command runs in
    /// `work_tree`, the repository's top level, without network access and
    /// without prompting. A tag that already exists is an error.
    pub fn create_annotated_tag(
        &self,
        work_tree: &Path,
        name: &str,
        message: &str,
        commit: &ObjectId,
    ) -> Result<(), GitError> {
        let args = [
            "tag",
            "--annotate",
            "--message",
            message,
            name,
            commit.as_str(),
        ];
        let mut command = Command::new("git");
        for variable in LOCATION_VARIABLES {
            command.env_remove(variable);
        }
        command
            .current_dir(work_tree)
            .env("GIT_DIR", &self.git_dir)
            .env("GIT_WORK_TREE", work_tree)
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_ALLOW_PROTOCOL", "none")
            .env("GIT_TERMINAL_PROMPT", "0")
            .arg("--no-pager")
            .args(args)
            .stdin(Stdio::null());
        let output = command
            .output()
            .map_err(|error| self.error(GitErrorKind::Unavailable(error)))?;
        if !output.status.success() {
            let shown = [
                "tag",
                "--annotate",
                "--message",
                "<message>",
                name,
                commit.as_str(),
            ];
            return Err(self.failed(&shown, output.status, &output.stderr));
        }
        Ok(())
    }
}

/// What Git finds when it looks for a repository from a directory, as it
/// does for the user: upwards through the parents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Discovered {
    /// The directory is inside the working tree whose top level this is,
    /// or is that top level.
    WorkingTree {
        /// The top level, as Git reports it.
        top_level: PathBuf,
    },
    /// The directory is in a bare repository.
    Bare,
    /// No repository.
    None,
}

/// Looks for the repository `directory` is in, with the read environment of
/// ADR 0006 and no repository named, so that Git searches the parents.
pub fn discover_repository(directory: &Path) -> Result<Discovered, GitError> {
    let run = |args: &[&str]| -> Result<Result<Vec<u8>, String>, GitError> {
        let output = discovery_command(directory, args)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| GitError {
                repository: directory.to_owned(),
                kind: GitErrorKind::Unavailable(error),
            })?;
        if output.status.success() {
            return Ok(Ok(output.stdout));
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        if stderr.contains("not a git repository") {
            return Ok(Err(stderr));
        }
        Err(GitError {
            repository: directory.to_owned(),
            kind: GitErrorKind::Failed {
                command: format!("git {}", args.join(" ")),
                status: output.status,
                stderr,
            },
        })
    };
    let Ok(bare) = run(&["rev-parse", "--is-bare-repository"])? else {
        return Ok(Discovered::None);
    };
    if bare.trim_ascii() == b"true" {
        return Ok(Discovered::Bare);
    }
    let args = ["rev-parse", "--show-toplevel"];
    let Ok(top_level) = run(&args)? else {
        return Ok(Discovered::None);
    };
    let top_level = std::str::from_utf8(&top_level)
        .ok()
        .map(|path| path.trim_end_matches(['\r', '\n']))
        .filter(|path| !path.is_empty())
        .ok_or_else(|| GitError {
            repository: directory.to_owned(),
            kind: GitErrorKind::UnexpectedOutput {
                command: format!("git {}", args.join(" ")),
                detail: format!("unexpected path {:?}", String::from_utf8_lossy(&top_level)),
            },
        })?;
    Ok(Discovered::WorkingTree {
        top_level: PathBuf::from(top_level),
    })
}

/// Initializes a repository in `directory`, which must exist and be in no
/// repository, with the author's own Git configuration, so that settings
/// such as the default branch name are the author's. Nothing is committed.
pub fn init_repository(directory: &Path) -> Result<(), GitError> {
    let args = ["init", "--quiet"];
    let mut command = Command::new("git");
    for variable in LOCATION_VARIABLES {
        command.env_remove(variable);
    }
    let output = command
        .current_dir(directory)
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_ALLOW_PROTOCOL", "none")
        .env("GIT_TERMINAL_PROMPT", "0")
        .arg("--no-pager")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| GitError {
            repository: directory.to_owned(),
            kind: GitErrorKind::Unavailable(error),
        })?;
    if !output.status.success() {
        return Err(GitError {
            repository: directory.to_owned(),
            kind: GitErrorKind::Failed {
                command: format!("git {}", args.join(" ")),
                status: output.status,
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            },
        });
    }
    Ok(())
}
