//! `unit check`: every validation a build runs, on the working tree, plus
//! the checks that gate publication, with nothing written (see
//! `docs/domain/authoring-workflow.md` and
//! `docs/adr/0013-unit-check-and-publish.md`).
//!
//! A check reads the unit exactly as a build does and runs the shared
//! validation (see [`validate`](super::validate)). On top of it, the unit
//! lockfile must exist and record the current resolution, since a published
//! version carries its lockfile, and the unit must make an effective,
//! non-redundant contribution (OC-11). A check never writes `build/`, the
//! lockfile, or anything else in the working tree.

use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

use super::exclusive::BuildLock;
use super::{BuildProblem, Inputs, MANIFEST, ManifestFileError, read_working_manifest, validate};
use crate::adapters::git::{GitRepository, ReadSrcError};
use crate::adapters::lockfile;
use crate::adapters::registry::Registry;
use crate::domain::{ContributionError, LockChange, UnitIdentity, Version};

/// Checks the unit whose repository's top level is `root`, resolving its
/// dependencies from `registry`.
pub fn check_unit(root: &Path, registry: &Registry) -> Result<Checked, CheckFailure> {
    let manifest_path = root.join(MANIFEST);
    if let Err(error) = fs::symlink_metadata(&manifest_path) {
        if error.kind() == io::ErrorKind::NotFound {
            return Err(CheckFailure::Rejected(vec![CheckProblem::Build(
                BuildProblem::Manifest(ManifestFileError::Missing {
                    path: manifest_path,
                }),
            )]));
        }
    }
    let rejected = |problem| CheckFailure::Rejected(vec![CheckProblem::Build(problem)]);
    let repository =
        GitRepository::open(root).map_err(|error| rejected(BuildProblem::Repository(error)))?;
    // Read under the lock that serializes builds, so that the manifest and
    // the lockfile are seen before or after any build or update, never in
    // between.
    let git_dir = repository
        .git_directory()
        .map_err(|error| rejected(BuildProblem::Repository(error)))?;
    let _guard =
        BuildLock::acquire(&git_dir).map_err(|error| rejected(BuildProblem::BuildLock(error)))?;
    match super::update::interrupted(&git_dir) {
        Ok(None) => {}
        Ok(Some(journal)) => return Err(rejected(BuildProblem::UpdateInterrupted { journal })),
        Err(error) => return Err(rejected(BuildProblem::GitDirectory(error))),
    }

    let mut problems = Vec::new();
    let manifest = read_working_manifest(root)
        .map_err(|error| problems.push(BuildProblem::Manifest(error)))
        .ok();
    let src = match repository.read_working_src() {
        Ok(src) => Some(src),
        Err(ReadSrcError::Git(error)) => {
            problems.push(BuildProblem::Repository(error));
            None
        }
        Err(error) => {
            problems.push(BuildProblem::Src(error));
            None
        }
    };
    let amendments = match repository.read_working_amendments() {
        Ok(amendments) => Some(amendments),
        Err(ReadSrcError::Git(error)) => {
            problems.push(BuildProblem::Repository(error));
            None
        }
        Err(error) => {
            problems.push(BuildProblem::Amendments(error));
            None
        }
    };
    let previous = lockfile::read(root)
        .map_err(|error| problems.push(BuildProblem::Lockfile(error)))
        .ok();
    let index = registry
        .index()
        .map_err(|error| problems.push(BuildProblem::Registry(error)))
        .ok();
    let mut problems: Vec<CheckProblem> = problems.into_iter().map(CheckProblem::Build).collect();
    let (Some(manifest), Some(previous), Some(index)) = (manifest, previous, index) else {
        return Err(CheckFailure::Rejected(problems));
    };
    let outcome = publication_checks(
        &Inputs {
            manifest: &manifest,
            src: src.as_ref(),
            amendments: amendments.as_ref(),
            previous: &previous.clone().unwrap_or_default(),
        },
        previous.is_some(),
        &index,
        &mut problems,
    );
    match outcome {
        Some(outcome) if problems.is_empty() => Ok(Checked {
            unit: manifest.name().clone(),
            version: manifest.version().clone(),
            files: outcome.files,
            dependencies: outcome.dependencies,
        }),
        _ => Err(CheckFailure::Rejected(problems)),
    }
}

/// What passing the publication checks tells about the unit.
pub(super) struct Publishable {
    /// How many files the composition delivers.
    pub(super) files: usize,
    /// How many dependencies, direct and transitive, are locked.
    pub(super) dependencies: usize,
}

/// Runs the build's validation on `inputs` and the checks that gate
/// publication on top of it: the lockfile exists (`locked`) and records the
/// resolution, and the contribution is effective and non-redundant. Every
/// problem is added to `problems`, in that order after the build's, and the
/// result is given only when the build's validation passed.
pub(super) fn publication_checks(
    inputs: &Inputs<'_>,
    locked: bool,
    index: &crate::domain::RegistryIndex,
    problems: &mut Vec<CheckProblem>,
) -> Option<Publishable> {
    let mut found = Vec::new();
    let validation = validate(inputs, index, &mut found);
    problems.extend(found.into_iter().map(CheckProblem::Build));
    if !locked {
        problems.push(CheckProblem::NoLockfile);
    }
    if let Some(validated) = &validation.result {
        if locked && !validated.changes.is_empty() {
            problems.push(CheckProblem::LockfileStale {
                changes: validated.changes.clone(),
            });
        }
    }
    problems.extend(
        validation
            .contribution
            .into_iter()
            .map(CheckProblem::Contribution),
    );
    validation.result.map(|validated| Publishable {
        dependencies: validated.lock.units().len(),
        files: validated.files.len(),
    })
}

/// A unit that passed every check.
#[derive(Debug)]
pub struct Checked {
    unit: UnitIdentity,
    version: Version,
    files: usize,
    dependencies: usize,
}

impl Checked {
    /// The unit checked.
    pub fn unit(&self) -> &UnitIdentity {
        &self.unit
    }

    /// Its version, as its working tree's manifest declares.
    pub fn version(&self) -> &Version {
        &self.version
    }

    /// How many files a build of it delivers.
    pub fn files(&self) -> usize {
        self.files
    }

    /// How many dependencies, direct and transitive, are locked.
    pub fn dependencies(&self) -> usize {
        self.dependencies
    }
}

/// Why a check failed. Nothing was written.
#[derive(Debug)]
pub enum CheckFailure {
    /// Every problem found is listed.
    Rejected(Vec<CheckProblem>),
}

impl fmt::Display for CheckFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self::Rejected(problems) = self;
        for (index, problem) in problems.iter().enumerate() {
            if index > 0 {
                f.write_str("\n")?;
            }
            problem.fmt(f)?;
        }
        Ok(())
    }
}

impl std::error::Error for CheckFailure {}

/// A problem that fails a check, and so a publication.
#[derive(Debug)]
pub enum CheckProblem {
    /// A problem a build would report too, with the same wording.
    Build(BuildProblem),
    /// There is no unit lockfile; `unit build` creates it.
    NoLockfile,
    /// The lockfile does not record the current resolution; `unit build`
    /// rewrites it.
    LockfileStale {
        /// How the resolution differs from it, by identity.
        changes: std::collections::BTreeMap<UnitIdentity, LockChange>,
    },
    /// The unit's contribution does not qualify for publication.
    Contribution(ContributionError),
}

impl fmt::Display for CheckProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Build(problem) => problem.fmt(f),
            Self::NoLockfile => f.write_str(
                "there is no assemblr-lock.json; a published version carries its lockfile, which \
                 `assemblr unit build` creates",
            ),
            Self::LockfileStale { changes } => {
                let changes: Vec<String> = changes
                    .iter()
                    .map(|(identity, change)| match change {
                        LockChange::Added(version) => format!("{identity} {version} (new)"),
                        LockChange::Changed { from, to } => format!("{identity} {from} -> {to}"),
                        LockChange::Retired(version) => {
                            format!("{identity} {version} (no longer required)")
                        }
                    })
                    .collect();
                write!(
                    f,
                    "assemblr-lock.json does not record the current resolution: {}; run `assemblr \
                     unit build`, which rewrites it",
                    changes.join(", ")
                )
            }
            Self::Contribution(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for CheckProblem {}
