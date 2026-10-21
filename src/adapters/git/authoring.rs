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
//! Creating the tag is the one Git command Assemblr runs that writes, and
//! the one that runs with the author's own Git configuration and identity:
//! the tag is the author's, in the author's repository. Network access and
//! prompts stay disabled, and the repository is still named explicitly.

use std::path::Path;
use std::process::{Command, Stdio};

use super::{GitError, GitErrorKind, GitRepository};
use crate::domain::ObjectId;

/// The environment variables that name a repository, its object store, or
/// its reference namespace, and would point a write somewhere else.
const LOCATION_VARIABLES: [&str; 8] = [
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
