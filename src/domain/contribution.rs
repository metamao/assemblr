//! The effective-contribution check that gates publication: a unit must
//! contribute something, and nothing it contributes may be redundant (see
//! `docs/domain/authoring-workflow.md`).
//!
//! A unit contributes through the files it introduces, the dependencies it
//! aggregates, the amendments it makes to its dependencies' files, and the
//! options it sets on them. Aggregation counts: a unit whose purpose is to
//! compose other units into one reusable dependency contributes that
//! composition, whatever its size. Since amendments and options presuppose a
//! dependency, a unit is without contribution exactly when it delivers no
//! file and declares no dependency.
//!
//! An amendment whose copy is the provider's file byte for byte is
//! redundant: it changes nothing at its base and claims the file for no
//! reason, which keeps every other unit from amending it.

use std::collections::BTreeMap;
use std::fmt;

use super::composition::FileTree;
use super::identity::UnitIdentity;
use super::manifest::Manifest;
use super::path::RelativePath;
use super::version::Version;

/// The problems with the contribution of the unit `manifest` describes,
/// whose `src/` is `src` and whose copies are `amendments`. `providers`
/// holds the locked version and the `src/` files of each dependency that is
/// locked; an amendment whose provider is not among them, or is locked at
/// another version than the declared base, is left to the composition to
/// report. Problems are ordered: no contribution first, then redundant
/// amendments by path.
pub fn contribution_problems(
    manifest: &Manifest,
    src: &FileTree,
    amendments: &FileTree,
    providers: &BTreeMap<&UnitIdentity, (&Version, &FileTree)>,
) -> Vec<ContributionError> {
    let mut problems = Vec::new();
    if src.is_empty() && manifest.dependencies().is_empty() {
        problems.push(ContributionError::NoContribution);
    }
    for (path, declaration) in manifest.amendments() {
        let Some((locked, files)) = providers.get(declaration.unit()) else {
            continue;
        };
        if *locked != declaration.base() {
            continue;
        }
        let (Some(copy), Some(base)) = (amendments.get(path), files.get(path)) else {
            continue;
        };
        if copy.contents() == base.contents() {
            problems.push(ContributionError::RedundantAmendment {
                path: path.clone(),
                unit: declaration.unit().clone(),
                base: declaration.base().clone(),
            });
        }
    }
    problems
}

/// Why a unit's contribution does not qualify for publication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContributionError {
    /// The unit delivers no file and declares no dependency, so it
    /// contributes nothing.
    NoContribution,
    /// An amendment copy is identical to the provider's file at the base.
    RedundantAmendment {
        /// The amended path.
        path: RelativePath,
        /// The provider.
        unit: UnitIdentity,
        /// The declared base, which is the locked version.
        base: Version,
    },
}

impl fmt::Display for ContributionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoContribution => f.write_str(
                "the unit contributes nothing: it has no file under `src/` and declares no \
                 dependency, so there is nothing to publish",
            ),
            Self::RedundantAmendment { path, unit, base } => write!(
                f,
                "amendments/{path} is identical to src/{path} of {unit} {base}, so the amendment \
                 changes nothing; edit the copy or remove it and its declaration"
            ),
        }
    }
}

impl std::error::Error for ContributionError {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "fixtures are valid by construction")]

    use std::collections::BTreeMap;

    use super::{ContributionError, contribution_problems};
    use crate::domain::{
        AmendmentDeclaration, Description, FileTree, Manifest, Placeholders, RelativePath,
        UnitFile, UnitIdentity, Version,
    };

    fn identity(input: &str) -> UnitIdentity {
        input.parse().unwrap()
    }

    fn version(input: &str) -> Version {
        input.parse().unwrap()
    }

    fn path(input: &str) -> RelativePath {
        RelativePath::parse(input).unwrap()
    }

    fn tree(files: &[(&str, &str)]) -> FileTree {
        files
            .iter()
            .map(|(name, contents)| {
                (
                    path(name),
                    UnitFile::new(contents.as_bytes().to_vec(), false),
                )
            })
            .collect()
    }

    fn manifest(dependencies: &[&str], amendments: &[(&str, &str, &str)]) -> Manifest {
        Manifest::new(
            identity("@acme/app"),
            version("0.1.0"),
            Description::new("An app.").unwrap(),
            dependencies
                .iter()
                .map(|unit| (identity(unit), "^1".parse().unwrap()))
                .collect(),
            Placeholders::default(),
            amendments
                .iter()
                .map(|(file, unit, base)| {
                    (
                        path(file),
                        AmendmentDeclaration::new(identity(unit), version(base)),
                    )
                })
                .collect(),
        )
        .unwrap()
    }

    #[test]
    fn a_unit_with_neither_files_nor_dependencies_contributes_nothing() {
        let problems = contribution_problems(
            &manifest(&[], &[]),
            &FileTree::new(),
            &FileTree::new(),
            &BTreeMap::new(),
        );

        assert_eq!(problems, [ContributionError::NoContribution]);
        assert!(problems[0].to_string().contains("contributes nothing"));
    }

    #[test]
    fn a_file_or_a_dependency_is_a_contribution() {
        let no_problems = contribution_problems(
            &manifest(&[], &[]),
            &tree(&[("a.txt", "a")]),
            &FileTree::new(),
            &BTreeMap::new(),
        );
        assert!(no_problems.is_empty());

        // Aggregating one dependency counts, as does aggregating several.
        for dependencies in [&["@acme/node"][..], &["@acme/node", "@acme/web"][..]] {
            let no_problems = contribution_problems(
                &manifest(dependencies, &[]),
                &FileTree::new(),
                &FileTree::new(),
                &BTreeMap::new(),
            );
            assert!(no_problems.is_empty(), "{dependencies:?}");
        }
    }

    #[test]
    fn an_amendment_identical_to_its_base_is_redundant() {
        let node = identity("@acme/node");
        let locked = version("1.0.0");
        let provider = tree(&[("a.txt", "base"), ("b.txt", "base")]);
        let providers = BTreeMap::from([(&node, (&locked, &provider))]);
        let manifest = manifest(
            &["@acme/node"],
            &[
                ("a.txt", "@acme/node", "1.0.0"),
                ("b.txt", "@acme/node", "1.0.0"),
            ],
        );

        let problems = contribution_problems(
            &manifest,
            &FileTree::new(),
            &tree(&[("a.txt", "base"), ("b.txt", "edited")]),
            &providers,
        );

        assert_eq!(
            problems,
            [ContributionError::RedundantAmendment {
                path: path("a.txt"),
                unit: node.clone(),
                base: locked.clone(),
            }]
        );
        assert!(problems[0].to_string().contains("identical to src/a.txt"));
    }

    #[test]
    fn amendments_the_composition_reports_are_not_compared() {
        let node = identity("@acme/node");
        let other = version("1.1.0");
        let provider = tree(&[("a.txt", "base")]);
        // The provider is locked at another version than the declared base,
        // and a copy without a provider file is a missing path: both are the
        // composition's to report.
        let providers = BTreeMap::from([(&node, (&other, &provider))]);
        let manifest = manifest(
            &["@acme/node"],
            &[
                ("a.txt", "@acme/node", "1.0.0"),
                ("c.txt", "@acme/node", "1.0.0"),
            ],
        );

        let problems = contribution_problems(
            &manifest,
            &FileTree::new(),
            &tree(&[("a.txt", "base"), ("c.txt", "copy")]),
            &providers,
        );

        assert!(problems.is_empty(), "{problems:?}");
    }
}
