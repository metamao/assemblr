//! A consumer project's declaration and its lock state (see
//! `docs/domain/consumer-projects.md`).
//!
//! `assemblr.jsonc` declares the units the project explicitly selects, with
//! the manifest's serialization of `dependencies`. A project declares no
//! placeholders, no `amendments`, and no identity of its own. Its lock
//! records the resolved units as a unit's lock does, plus the
//! **reconciliation state**: the identity of the composition last
//! materialized, which the next composition reconciles against, and the
//! paths that composition manages.

use std::collections::BTreeSet;
use std::fmt;

use super::path::RelativePath;
use super::resolution::Dependencies;
use super::snapshot::BaseIdentity;

/// A consumer project's declaration: its explicit selections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    dependencies: Dependencies,
}

impl Project {
    /// The declaration of these selections.
    pub fn new(dependencies: Dependencies) -> Self {
        Self { dependencies }
    }

    /// The explicit selections, by identity, with their requirements.
    pub fn dependencies(&self) -> &Dependencies {
        &self.dependencies
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
