//! `assemblr skill install`: the skill that tells a coding agent to read
//! `assemblr guide` (see `docs/domain/registries-and-discovery.md`).
//!
//! The skill only points to the guide, so it stays true across versions of
//! Assemblr. It is written only when the user runs the command, and a file
//! that Assemblr did not write is never replaced without `--force`.

use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// The variable that moves Claude Code's configuration from `~/.claude`.
const CLAUDE_CONFIG_DIR: &str = "CLAUDE_CONFIG_DIR";

/// The last line of every skill Assemblr writes, by which it recognizes one
/// it may update. It names no command, so it never needs to change; a later
/// version that changes it must still recognize this one.
const MARKER: &str = "<!-- assemblr-skill -->";

/// The Claude Code skill. Its description lists the tasks that call for
/// Assemblr, so that Claude Code loads it for them; its body names no
/// command but `assemblr guide`.
const CLAUDE_SKILL: &str = concat!(
    "---\n",
    "name: assemblr\n",
    "description: Use when creating a new project, adding a capability (auth, lint, \
     infrastructure, configuration) to a project, updating the dependencies of a project with \
     assemblr.jsonc, starting from an existing project as a base, or making configuration \
     reusable across projects. Assemblr composes projects from reusable, versioned units.\n",
    "---\n",
    "\n",
    "Assemblr (the `assemblr` CLI) is installed. It composes projects from reusable, versioned \
     units. Before creating a project, adding a capability (auth, lint, infrastructure, \
     configuration), updating a project that has `assemblr.jsonc`, or making something reusable \
     across projects, run `assemblr guide` and follow it.\n",
    "\n",
    "<!-- assemblr-skill -->\n",
);

/// The largest existing skill file read, in bytes.
const FILE_LIMIT: u64 = 1024 * 1024;

/// What installing the skill did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillOutcome {
    /// There was no skill; it was written.
    Installed,
    /// An earlier skill that Assemblr wrote was replaced, or `--force`
    /// replaced another file.
    Updated,
    /// The skill was already the one this version writes.
    UpToDate,
}

/// The skill's file and what installing it did.
#[derive(Debug)]
pub struct SkillInstalled {
    pub path: PathBuf,
    pub outcome: SkillOutcome,
}

#[derive(Debug)]
pub enum SkillFailure {
    /// Neither `CLAUDE_CONFIG_DIR` nor a home directory says where Claude
    /// Code keeps its configuration.
    NoLocation,
    /// `CLAUDE_CONFIG_DIR` is a relative path.
    RelativeLocation(PathBuf),
    /// The skill's file exists and Assemblr did not write it.
    NotOurs(PathBuf),
    /// The skill's path is a symbolic link, which Assemblr never writes
    /// through.
    Link(PathBuf),
    /// The skill's path is a directory, which even `--force` keeps.
    Directory(PathBuf),
    Io {
        path: PathBuf,
        error: io::Error,
    },
}

impl fmt::Display for SkillFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoLocation => f.write_str(
                "cannot find Claude Code's configuration: there is no home directory and \
                 CLAUDE_CONFIG_DIR is not set",
            ),
            Self::RelativeLocation(path) => write!(
                f,
                "CLAUDE_CONFIG_DIR is the relative path {}; it must be absolute",
                path.display()
            ),
            Self::NotOurs(path) => write!(
                f,
                "{} already exists, and Assemblr did not write it",
                path.display()
            ),
            Self::Link(path) => write!(f, "{} is a symbolic link", path.display()),
            Self::Directory(path) => write!(f, "{} is a directory", path.display()),
            Self::Io { path, error } => write!(f, "could not write {}: {error}", path.display()),
        }
    }
}

/// Where Claude Code reads the user's skill named `assemblr`: under
/// `CLAUDE_CONFIG_DIR` when it is set, otherwise under `~/.claude`.
pub fn claude_skill_path() -> Result<PathBuf, SkillFailure> {
    let config = match std::env::var_os(CLAUDE_CONFIG_DIR) {
        Some(dir) if !dir.is_empty() => {
            let dir = PathBuf::from(dir);
            if !dir.is_absolute() {
                return Err(SkillFailure::RelativeLocation(dir));
            }
            dir
        }
        _ => dirs::home_dir()
            .ok_or(SkillFailure::NoLocation)?
            .join(".claude"),
    };
    Ok(config.join("skills").join("assemblr").join("SKILL.md"))
}

/// Installs the Claude Code skill at `path`. A file there is replaced when
/// Assemblr wrote it, or with `force`; the new file is written beside it and
/// renamed over it, so a failure never leaves half a skill.
pub fn install_claude_skill(path: &Path, force: bool) -> Result<SkillInstalled, SkillFailure> {
    let io_error = |error| SkillFailure::Io {
        path: path.to_owned(),
        error,
    };
    let outcome = match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => SkillOutcome::Installed,
        Err(error) => return Err(io_error(error)),
        Ok(metadata) if metadata.is_file() => {
            let mut text = String::new();
            let read = fs::File::open(path)
                .and_then(|file| file.take(FILE_LIMIT).read_to_string(&mut text));
            // A file that is not UTF-8 is not one Assemblr wrote.
            let ours = read.is_ok() && text.lines().any(|line| line == MARKER);
            if ours && text == CLAUDE_SKILL {
                SkillOutcome::UpToDate
            } else if ours || force {
                SkillOutcome::Updated
            } else {
                return Err(SkillFailure::NotOurs(path.to_owned()));
            }
        }
        Ok(metadata) if metadata.is_dir() => {
            return Err(SkillFailure::Directory(path.to_owned()));
        }
        // A link, or another kind of file, which `--force` replaces.
        Ok(_) if force => SkillOutcome::Updated,
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(SkillFailure::Link(path.to_owned()));
        }
        Ok(_) => return Err(SkillFailure::NotOurs(path.to_owned())),
    };
    if outcome != SkillOutcome::UpToDate {
        let dir = path
            .parent()
            .ok_or_else(|| SkillFailure::NotOurs(path.to_owned()))?;
        fs::create_dir_all(dir).map_err(io_error)?;
        let new = dir.join(".SKILL.md.assemblr-new");
        fs::write(&new, CLAUDE_SKILL).map_err(io_error)?;
        if let Err(error) = fs::rename(&new, path) {
            let _ = fs::remove_file(&new);
            return Err(io_error(error));
        }
    }
    Ok(SkillInstalled {
        path: path.to_owned(),
        outcome,
    })
}

#[cfg(test)]
mod tests {
    use super::{CLAUDE_SKILL, MARKER};

    #[test]
    fn the_skill_ends_with_the_marker_and_names_only_the_guide() {
        assert_eq!(CLAUDE_SKILL.lines().last(), Some(MARKER));
        let commands: Vec<_> = CLAUDE_SKILL
            .split('`')
            .skip(1)
            .step_by(2)
            .filter(|quoted| quoted.starts_with("assemblr "))
            .collect();
        assert_eq!(
            commands,
            ["assemblr guide"],
            "the skill must stay true across versions"
        );
    }
}
