//! `unit add`: declares a dependency in the unit's `manifest.jsonc` and
//! builds the unit as `unit build` does (see
//! `docs/domain/authoring-workflow.md`).
//!
//! The declaration is planned on the manifest's text, so comments and
//! formatting survive, and the edited manifest is validated as a build
//! validates the working tree's before anything is written. A build that
//! would be rejected leaves the manifest as it was.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::amend::{MANIFEST_NEW, write_manifest};
use super::{
    BuildFailure, BuildProblem, Built, MANIFEST, ManifestFileError, Opened, open,
    publish_validated, resolve_graph, validate_opened,
};
use crate::adapters::manifest::read_manifest;
use crate::adapters::project::declaration::{
    DeclarationChange, EditFailure, add_after, selected_requirement,
};
use crate::adapters::registry::Registry;
use crate::domain::{Manifest, UnitIdentity, Version, VersionRequirement};

/// Declares `identity` as a dependency of the unit whose repository's top
/// level is `root`, then builds the unit as `build_unit` does. Without a
/// `requirement` the unit is recorded with a caret requirement on the
/// version the resolution picks, which is its locked version when it is a
/// transitive dependency already; a unit declared already keeps its own.
pub fn add_dependency(
    root: &Path,
    registry: &Registry,
    identity: &UnitIdentity,
    requirement: Option<&str>,
) -> Result<DependencyAdded, AddFailure> {
    let mut opened = open(root, registry).map_err(AddFailure::rejected)?;
    let staged = root.join(MANIFEST_NEW);
    if fs::symlink_metadata(&staged).is_ok() {
        return Err(AddFailure::Rejected(vec![AddProblem::StagingOccupied {
            path: staged,
        }]));
    }
    if identity == opened.manifest.name() {
        return Err(AddFailure::Rejected(vec![AddProblem::Itself(
            identity.clone(),
        )]));
    }
    let planned = plan(&opened, identity, requirement)?;
    let validated =
        validate_opened(&mut opened, &planned.manifest).map_err(AddFailure::rejected)?;
    // The manifest first: it is what the author asked for, and a build
    // that fails after it is redone by `unit build`.
    if let Err(error) = write_manifest(root, &staged, planned.text.as_bytes()) {
        let _ = fs::remove_file(&staged);
        return Err(AddFailure::Write {
            path: root.join(MANIFEST),
            error,
        });
    }
    let built = publish_validated(root, &opened, &planned.text, &planned.manifest, validated)
        .map_err(|failure| AddFailure::Build(Box::new(failure)))?;
    Ok(DependencyAdded {
        built,
        change: planned.change,
    })
}

/// The edited manifest, decided and not written.
struct Planned {
    text: String,
    manifest: Manifest,
    change: DeclarationChange,
}

fn plan(
    opened: &Opened,
    identity: &UnitIdentity,
    requirement: Option<&str>,
) -> Result<Planned, AddFailure> {
    let rejected = |problem| AddFailure::Rejected(vec![problem]);
    let cannot = |failure: EditFailure| match failure {
        EditFailure::Cannot(detail) => rejected(AddProblem::CannotEdit(detail)),
        EditFailure::NotSelected => rejected(AddProblem::CannotEdit(
            "the dependency is not declared".to_owned(),
        )),
    };
    let text = opened.text.as_str();
    let manifest = &opened.manifest;
    let previous = opened.previous.clone().unwrap_or_default();
    let preferred = ["description", "version", "name"];
    // The requirement as the manifest spells it, so that the edit compares
    // text with text and never rewrites an equal requirement.
    let declared = selected_requirement(text, identity);
    let requirement = match requirement {
        // `unit add <unit>@1.2.3` records `^1.2.3`, as the consumer's `add`
        // does: the file always spells its operator.
        Some(requirement) if Version::parse(requirement).is_ok() => format!("^{requirement}"),
        Some(requirement) => {
            VersionRequirement::parse(requirement)
                .map_err(|error| rejected(AddProblem::InvalidRequirement(error.to_string())))?;
            requirement.to_owned()
        }
        None => match declared {
            Some(declared) => declared,
            None => {
                // The version a resolution picks when any version is allowed,
                // under the lock, so that a transitive dependency keeps its
                // locked version.
                let (probe, _) = add_after(text, identity, "*", &preferred).map_err(cannot)?;
                let probe = read_manifest(&probe).map_err(|error| {
                    rejected(AddProblem::CannotEdit(format!(
                        "the edited manifest is not valid: {error}"
                    )))
                })?;
                let mut found = Vec::new();
                let Some(resolved) = resolve_graph(
                    Some(probe.name()),
                    probe.dependencies(),
                    &previous,
                    &opened.index,
                    &mut found,
                ) else {
                    return Err(AddFailure::rejected(BuildFailure::Rejected(found)));
                };
                let Some(locked) = resolved.lock.units().get(identity) else {
                    return Err(rejected(AddProblem::CannotEdit(format!(
                        "the resolution did not select {identity}, so there is no version to \
                         record"
                    ))));
                };
                // A caret on the version picked resolves to it again under
                // the same lock.
                format!("^{}", locked.version())
            }
        },
    };
    let (text, change) = add_after(text, identity, &requirement, &preferred).map_err(cannot)?;
    // What the text must describe: the manifest plus the declaration, which
    // a reading the editor misjudged would not match.
    let parsed = VersionRequirement::parse(&requirement)
        .map_err(|error| rejected(AddProblem::InvalidRequirement(error.to_string())))?;
    let expected = manifest
        .with_dependency(identity.clone(), parsed)
        .ok_or_else(|| rejected(AddProblem::Itself(identity.clone())))?;
    let manifest = read_manifest(&text).map_err(|error| {
        rejected(AddProblem::CannotEdit(format!(
            "the edited manifest is not valid: {error}"
        )))
    })?;
    if manifest != expected {
        return Err(rejected(AddProblem::CannotEdit(
            "the edited manifest does not hold the intended declaration".to_owned(),
        )));
    }
    Ok(Planned {
        text,
        manifest,
        change,
    })
}

/// What `unit add` did.
#[derive(Debug)]
pub struct DependencyAdded {
    built: Built,
    change: DeclarationChange,
}

impl DependencyAdded {
    /// The build that followed.
    pub fn built(&self) -> &Built {
        &self.built
    }

    /// What happened to the dependency's declaration.
    pub fn change(&self) -> &DeclarationChange {
        &self.change
    }
}

/// Why `unit add` failed.
#[derive(Debug)]
pub enum AddFailure {
    /// The command was rejected before anything was written.
    Rejected(Vec<AddProblem>),
    /// The manifest could not be written. Nothing was changed.
    Write {
        /// The manifest.
        path: PathBuf,
        /// Why.
        error: io::Error,
    },
    /// The manifest was written, and the build that followed failed to
    /// write its result; `unit build` redoes it.
    Build(Box<BuildFailure>),
}

impl AddFailure {
    /// A build rejection, which leaves nothing changed, as this command's.
    fn rejected(failure: BuildFailure) -> Self {
        match failure {
            BuildFailure::Rejected(problems) => Self::Rejected(
                problems
                    .into_iter()
                    .map(|problem| AddProblem::Build(Box::new(problem)))
                    .collect(),
            ),
            failure => Self::Build(Box::new(failure)),
        }
    }
}

impl fmt::Display for AddFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(problems) => {
                for (index, problem) in problems.iter().enumerate() {
                    if index > 0 {
                        f.write_str("\n")?;
                    }
                    problem.fmt(f)?;
                }
                Ok(())
            }
            Self::Write { path, error } => {
                write!(f, "could not write {}: {error}", path.display())
            }
            Self::Build(failure) => failure.fmt(f),
        }
    }
}

impl std::error::Error for AddFailure {}

/// A problem that rejects `unit add`.
#[derive(Debug)]
pub enum AddProblem {
    /// A problem a build reports too.
    Build(Box<BuildProblem>),
    /// The unit was named as its own dependency.
    Itself(UnitIdentity),
    /// The requirement is not a SemVer requirement.
    InvalidRequirement(String),
    /// The manifest's text could not be edited safely.
    CannotEdit(String),
    /// Something exists at the reserved path the edited manifest is staged
    /// at.
    StagingOccupied {
        /// The path.
        path: PathBuf,
    },
}

impl fmt::Display for AddProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Build(problem) => problem.fmt(f),
            Self::Itself(identity) => write!(f, "{identity} cannot depend on itself"),
            Self::InvalidRequirement(detail) => write!(f, "invalid requirement: {detail}"),
            Self::CannotEdit(detail) => write!(
                f,
                "`manifest.jsonc` could not be edited safely, so it is unchanged: {detail}"
            ),
            Self::StagingOccupied { path } => write!(
                f,
                "{} exists; the name is reserved for the edited manifest before it replaces \
                 `manifest.jsonc`, so remove it if an earlier command left it, and otherwise \
                 move it away",
                path.display()
            ),
        }
    }
}

impl std::error::Error for AddProblem {}

impl From<ManifestFileError> for AddProblem {
    fn from(error: ManifestFileError) -> Self {
        Self::Build(Box::new(BuildProblem::Manifest(error)))
    }
}
