//! A consumer project's declaration and its lock state (see
//! `docs/domain/consumer-projects.md`).
//!
//! `assemblr.jsonc` declares the units the project explicitly selects and
//! the values it supplies to their parameters, with the manifest's
//! serialization and rules: options target declared selections only. A
//! project declares no `params`, no `amendments`, and no identity of its
//! own. Its lock records the resolved units as a unit's lock does, plus the
//! **reconciliation state**: the identity of the composition last
//! materialized, which the next composition reconciles against, and the
//! paths that composition manages.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use super::identity::UnitIdentity;
use super::manifest::{ManifestError, OptionValues, relation_errors};
use super::path::RelativePath;
use super::resolution::Dependencies;
use super::snapshot::BaseIdentity;

/// A consumer project's declaration: its explicit selections and the
/// values it supplies to them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    dependencies: Dependencies,
    options: BTreeMap<UnitIdentity, OptionValues>,
}

impl Project {
    /// A declaration, after checking that every `options` entry targets a
    /// declared dependency. Every inconsistency is reported.
    pub fn new(
        dependencies: Dependencies,
        options: BTreeMap<UnitIdentity, OptionValues>,
    ) -> Result<Self, Vec<ManifestError>> {
        let declared: BTreeSet<UnitIdentity> = dependencies.keys().cloned().collect();
        let errors = relation_errors(
            None,
            &declared,
            options.keys(),
            std::iter::empty::<(&RelativePath, &UnitIdentity)>(),
        );
        if errors.is_empty() {
            Ok(Self {
                dependencies,
                options,
            })
        } else {
            Err(errors)
        }
    }

    /// The explicit selections, by identity, with their requirements.
    pub fn dependencies(&self) -> &Dependencies {
        &self.dependencies
    }

    /// The values supplied to the selections' parameters.
    pub fn options(&self) -> &BTreeMap<UnitIdentity, OptionValues> {
        &self.options
    }
}

/// The reconciliation state a consumer project's lock records: what was
/// last materialized, identified so that it can be verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciliationState {
    base: BaseIdentity,
    managed: BTreeSet<RelativePath>,
    unbased: BTreeSet<RelativePath>,
}

impl ReconciliationState {
    /// The state after materializing the composition identified by `base`
    /// at `managed`, of which `unbased` have no stored base (they are
    /// ignored by the project's version control, so their base is not kept
    /// in the tracked store).
    pub fn new(
        base: BaseIdentity,
        managed: BTreeSet<RelativePath>,
        unbased: BTreeSet<RelativePath>,
    ) -> Self {
        Self {
            base,
            managed,
            unbased,
        }
    }

    /// The identity of the reconciliation base.
    pub fn base(&self) -> &BaseIdentity {
        &self.base
    }

    /// Every path the composition manages, sorted.
    pub fn managed(&self) -> &BTreeSet<RelativePath> {
        &self.managed
    }

    /// The managed paths whose base is not stored.
    pub fn unbased(&self) -> &BTreeSet<RelativePath> {
        &self.unbased
    }
}

impl fmt::Display for ReconciliationState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "base {} over {} managed path(s)",
            self.base,
            self.managed.len()
        )
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "fixtures are valid by construction")]

    use std::collections::BTreeMap;

    use super::Project;
    use crate::domain::{ManifestError, ParameterValue, UnitIdentity};

    fn identity(input: &str) -> UnitIdentity {
        input.parse().unwrap()
    }

    #[test]
    fn options_must_target_declared_selections() {
        let dependencies = BTreeMap::from([(identity("@acme/web"), "^1".parse().unwrap())]);
        let options = BTreeMap::from([(
            identity("@acme/db"),
            BTreeMap::from([("port".parse().unwrap(), ParameterValue::Integer(5432))]),
        )]);
        assert_eq!(
            Project::new(dependencies.clone(), options).unwrap_err(),
            [ManifestError::UndeclaredOptionTarget(identity("@acme/db"))]
        );
        let project = Project::new(dependencies.clone(), BTreeMap::new()).unwrap();
        assert_eq!(project.dependencies(), &dependencies);
    }
}
