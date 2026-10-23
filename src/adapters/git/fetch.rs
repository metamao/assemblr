//! Fetching a remote unit repository into a bare copy that Assemblr keeps.
//!
//! The copy holds the remote's version tags and its `HEAD`, nothing else.
//! Every other read goes through [`GitRepository`] against the copy, offline,
//! with the read environment of ADR 0006. A fetch runs with the user's own
//! Git configuration and SSH settings, which hold their credentials, but
//! only over the transports ADR 0029 allows (see that ADR).

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use super::authoring::LOCATION_VARIABLES;
use super::command::discovery_command;

/// Where the copy records the remote's `HEAD`, so that `HEAD` of the copy
/// names the commit the remote's `HEAD` named.
const HEAD_REF: &str = "refs/heads/head";

/// The transports a fetch may use. Git's own default also allows `git://`
/// and plain `http://`, which are not authenticated, and `ext::`, which runs
/// a command.
const PROTOCOLS: &str = "https:ssh:file";

/// Fetches the version tags and `HEAD` of `reference` into the bare
/// repository at `copy`, creating it first when it does not exist.
///
/// Tags are fetched with force and pruned, so the copy shows a tag that
/// moved or disappeared on the remote, for the registry to report. Objects
/// are never garbage-collected, so a commit the registry recorded stays
/// readable after its tag moves.
pub(crate) fn fetch(reference: &str, copy: &Path) -> Result<(), FetchError> {
    let failure = |kind| FetchError {
        reference: reference.to_owned(),
        kind,
    };
    if !copy.join("HEAD").is_file() {
        create(copy).map_err(failure)?;
    }
    let args = [
        "-c",
        "gc.auto=0",
        "-c",
        "fetch.fsckObjects=true",
        "fetch",
        "--quiet",
        "--no-tags",
        "--prune",
        "--force",
        "--",
        reference,
        "+refs/tags/*:refs/tags/*",
        &format!("+HEAD:{HEAD_REF}"),
    ];
    let mut command = Command::new("git");
    for variable in LOCATION_VARIABLES {
        command.env_remove(variable);
    }
    command
        .current_dir(copy)
        .env("GIT_DIR", copy)
        .env("GIT_ALLOW_PROTOCOL", PROTOCOLS)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .env("LANGUAGE", "C")
        .arg("--no-pager")
        .args(args);
    run(command, "fetch").map_err(failure)
}

/// Creates the bare copy: initialized in a directory beside it and renamed
/// into place, so a copy that exists is complete.
fn create(copy: &Path) -> Result<(), FetchErrorKind> {
    let io = |action: &'static str, path: &Path| {
        let path = path.to_owned();
        move |error| FetchErrorKind::Io {
            action,
            path,
            error,
        }
    };
    let parent = copy.parent().unwrap_or(copy);
    fs::create_dir_all(parent).map_err(io("create", parent))?;
    let staging = staging_path(copy);
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(io("remove", &staging))?;
    }
    fs::create_dir(&staging).map_err(io("create", &staging))?;
    // No templates: the user's `init.templateDir` could install hooks.
    let init = ["init", "--quiet", "--bare", "--template="];
    run(discovery_command(&staging, &init), "init")?;
    for (name, value) in [
        ("gc.auto", "0"),
        ("core.logAllRefUpdates", "false"),
        ("fetch.fsckObjects", "true"),
    ] {
        let mut command = discovery_command(&staging, &["config", name, value]);
        command.env("GIT_DIR", &staging);
        run(command, "config")?;
    }
    let mut command = discovery_command(&staging, &["symbolic-ref", "HEAD", HEAD_REF]);
    command.env("GIT_DIR", &staging);
    run(command, "symbolic-ref")?;
    fs::rename(&staging, copy).map_err(io("rename", &staging))
}

fn staging_path(copy: &Path) -> PathBuf {
    let mut name = copy.file_name().unwrap_or_default().to_owned();
    name.push(".new");
    copy.with_file_name(name)
}

fn run(mut command: Command, operation: &'static str) -> Result<(), FetchErrorKind> {
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(FetchErrorKind::Unavailable)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(FetchErrorKind::Failed {
            operation,
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }
}

/// Why a remote repository could not be fetched.
#[derive(Debug)]
pub struct FetchError {
    reference: String,
    kind: FetchErrorKind,
}

impl FetchError {
    /// What went wrong.
    pub fn kind(&self) -> &FetchErrorKind {
        &self.kind
    }
}

/// What went wrong fetching a remote repository.
#[derive(Debug)]
pub enum FetchErrorKind {
    /// `git` could not be run.
    Unavailable(io::Error),
    /// The copy's directory could not be prepared.
    Io {
        /// What was being done.
        action: &'static str,
        /// To which path.
        path: PathBuf,
        /// Why it failed.
        error: io::Error,
    },
    /// A Git command failed, for example because the remote is unreachable
    /// or refused the credentials.
    Failed {
        /// The Git operation.
        operation: &'static str,
        /// How it exited.
        status: ExitStatus,
        /// What Git reported.
        stderr: String,
    },
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reference = &self.reference;
        match &self.kind {
            FetchErrorKind::Unavailable(error) => {
                write!(f, "could not run `git` to fetch {reference}: {error}")
            }
            FetchErrorKind::Io {
                action,
                path,
                error,
            } => write!(
                f,
                "could not {action} {} to fetch {reference}: {error}",
                path.display()
            ),
            FetchErrorKind::Failed {
                operation,
                status,
                stderr,
            } => {
                write!(f, "could not fetch {reference}: `git {operation}` {status}")?;
                if !stderr.is_empty() {
                    write!(f, ": {stderr}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for FetchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            FetchErrorKind::Unavailable(error) | FetchErrorKind::Io { error, .. } => Some(error),
            FetchErrorKind::Failed { .. } => None,
        }
    }
}
