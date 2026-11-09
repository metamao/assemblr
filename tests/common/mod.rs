//! Fixtures shared by the integration tests, which run the real binary and
//! real Git against repositories built in temporary directories.
//!
//! Isolation lives here, in one place, so that no test reads or changes the
//! developer's registry, Git configuration, or repositories (AGENTS.md §8):
//!
//! - Every test works in its own directory under the system's temporary
//!   directory, outside this repository. Under `target/`, a fixture would sit
//!   inside this crate's working tree: Git would read its ignore rules and,
//!   where a fixture has no repository of its own, find this one.
//! - `git` and `assemblr` run without the caller's `GIT_*` variables, without
//!   the system's Git configuration, and with a global configuration of the
//!   test's own: the file `.gitconfig` in the test's home, which a test writes
//!   when it needs one.
//! - `assemblr` keeps its registry under the test's `data` directory.
//!
//! Each test crate includes this module with `mod common;` and uses only part
//! of it.

#![allow(
    dead_code,
    reason = "each test crate includes this module and uses only part of it"
)]

use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

#[cfg(windows)]
pub(crate) const NULL_DEVICE: &str = "NUL";
#[cfg(not(windows))]
pub(crate) const NULL_DEVICE: &str = "/dev/null";

/// The directory that holds every test's directory: in the system's
/// temporary directory, and distinct for each checkout, so that test runs in
/// two worktrees never share one.
fn root() -> PathBuf {
    let mut hasher = DefaultHasher::new();
    env!("CARGO_MANIFEST_DIR").hash(&mut hasher);
    // The temporary directory can be reached through a symbolic link, as on
    // macOS, where it is under `/var`, a link to `/private/var`. A command
    // reads its working directory with links resolved, so a path given with
    // the link would name the same repository differently (ADR 0007).
    // Windows would resolve to a `\\?\` path, which Git does not accept.
    #[cfg(unix)]
    let temp = std::env::temp_dir().canonicalize().unwrap();
    #[cfg(not(unix))]
    let temp = std::env::temp_dir();
    temp.join(format!("assemblr-tests-{:016x}", hasher.finish()))
}

/// An empty directory for the test `test` of this test crate, outside the
/// repository. What an earlier run left there is removed.
pub(crate) fn scratch(test: &str) -> PathBuf {
    let dir = root().join(env!("CARGO_CRATE_NAME")).join(test);
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A test's directories: its unit repositories and projects in `dir`, the
/// registry in `data`, and a home in `home`.
pub(crate) struct Fixture {
    pub(crate) dir: PathBuf,
    pub(crate) data: PathBuf,
    pub(crate) home: PathBuf,
}

impl Fixture {
    pub(crate) fn new(test: &str) -> Self {
        let dir = scratch(test);
        let fixture = Self {
            data: dir.join("data"),
            home: dir.join("home"),
            dir,
        };
        fs::create_dir_all(&fixture.data).unwrap();
        fs::create_dir_all(&fixture.home).unwrap();
        fixture
    }

    /// The command `assemblr` in `dir`, isolated as [`assemblr`] describes.
    pub(crate) fn command(&self, dir: &Path, args: &[&str]) -> Command {
        let mut command = assemblr(dir, &self.data, &self.home);
        command.args(args);
        command
    }
}

/// The binary under test, run in `dir` with its registry under `data` and
/// `home` as the home directory. Git, which the binary runs for some
/// operations with the author's own configuration, sees `home/.gitconfig` as
/// that configuration, no system configuration, none of the caller's `GIT_*`
/// variables, and an identity, as an author's shell would provide one. A
/// coding agent's configuration, which `skill install` writes, is under
/// `home` too, on every platform.
pub(crate) fn assemblr(dir: &Path, data: &Path, home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_assemblr"));
    without_git_variables(&mut command);
    command
        .current_dir(dir)
        .env("ASSEMBLR_DATA_DIR", data)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", home.join(".gitconfig"))
        .env("GIT_AUTHOR_NAME", "Author")
        .env("GIT_AUTHOR_EMAIL", "author@example.com")
        .env("GIT_COMMITTER_NAME", "Author")
        .env("GIT_COMMITTER_EMAIL", "author@example.com");
    command
}

/// Removes every inherited `GIT_*` variable, such as `GIT_DIR` when tests
/// run from a Git hook, which would point a command at another repository.
fn without_git_variables(command: &mut Command) {
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            command.env_remove(name);
        }
    }
}

/// `git` in `dir`, isolated from the developer's configuration, with fixed
/// identities and dates so that object ids are reproducible. Git does not
/// look for a repository above the tests' root directory.
pub(crate) fn git_command(dir: &Path) -> Command {
    let mut command = Command::new("git");
    without_git_variables(&mut command);
    command
        .current_dir(dir)
        .env("GIT_CEILING_DIRECTORIES", root())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", NULL_DEVICE)
        .env("GIT_AUTHOR_NAME", "Author")
        .env("GIT_AUTHOR_EMAIL", "author@example.com")
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_NAME", "Author")
        .env("GIT_COMMITTER_EMAIL", "author@example.com")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .args(["-c", "core.autocrlf=false", "-c", "init.defaultBranch=main"])
        .stdin(Stdio::null());
    command
}

/// Runs `git` in `dir`, as [`git_command`] sets it up.
pub(crate) fn git_output(dir: &Path, args: &[&str]) -> Output {
    git_command(dir).args(args).output().unwrap()
}

/// Runs `git` in `dir` and returns its trimmed output. A failure fails the
/// test.
pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
    let output = git_output(dir, args);
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

pub(crate) fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub(crate) fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// `output`, which must be a success.
pub(crate) fn succeeds(output: Output) -> Output {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        stdout(&output),
        stderr(&output)
    );
    output
}

/// `output`, which must be a failure with exit code 1.
pub(crate) fn fails(output: Output) -> Output {
    assert_eq!(
        output.status.code(),
        Some(1),
        "stdout: {}\nstderr: {}",
        stdout(&output),
        stderr(&output)
    );
    output
}
