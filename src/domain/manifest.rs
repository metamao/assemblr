//! The unit manifest: a unit's identity, version, description, dependencies,
//! dependency options, and parameters.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use super::identity::UnitIdentity;
use super::parameter::{Description, ParameterDefinition, ParameterName, ParameterValue};
use super::version::{Version, VersionRequirement};

/// The values a unit requires for one dependency's parameters.
pub type OptionValues = BTreeMap<ParameterName, ParameterValue>;

/// A validated unit manifest.
///
/// Invariants beyond those of its parts:
///
/// - every `options` key is a declared dependency, because options target
///   only direct dependencies;
/// - a unit never depends on itself.
///
/// Whether an option value suits the dependency's parameter depends on the
/// dependency's own manifest, so it is checked during composition, not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    name: UnitIdentity,
    version: Version,
    description: Description,
    dependencies: BTreeMap<UnitIdentity, VersionRequirement>,
    options: BTreeMap<UnitIdentity, OptionValues>,
    params: BTreeMap<ParameterName, ParameterDefinition>,
}

impl Manifest {
    /// Creates a manifest, reporting every invariant it would break.
    pub fn new(
        name: UnitIdentity,
        version: Version,
        description: Description,
        dependencies: BTreeMap<UnitIdentity, VersionRequirement>,
        options: BTreeMap<UnitIdentity, OptionValues>,
        params: BTreeMap<ParameterName, ParameterDefinition>,
    ) -> Result<Self, Vec<ManifestError>> {
        let declared = dependencies.keys().cloned().collect();
        let errors = relation_errors(Some(&name), &declared, options.keys());
        if !errors.is_empty() {
            return Err(errors);
        }
        Ok(Self {
            name,
            version,
            description,
            dependencies,
            options,
            params,
        })
    }

    /// The unit's identity.
    pub fn name(&self) -> &UnitIdentity {
        &self.name
    }

    /// The unit's exact version.
    pub fn version(&self) -> &Version {
        &self.version
    }

    /// The unit's description.
    pub fn description(&self) -> &Description {
        &self.description
    }

    /// The unit's direct dependencies and their version requirements.
    pub fn dependencies(&self) -> &BTreeMap<UnitIdentity, VersionRequirement> {
        &self.dependencies
    }

    /// The values the unit requires for its direct dependencies' parameters.
    pub fn options(&self) -> &BTreeMap<UnitIdentity, OptionValues> {
        &self.options
    }

    /// The unit's own public parameters.
    pub fn params(&self) -> &BTreeMap<ParameterName, ParameterDefinition> {
        &self.params
    }
}

/// Checks the relations between a manifest's name, dependencies, and option
/// targets, in deterministic order.
///
/// Readers call this with whatever parts are valid, so these problems are
/// reported together with problems in other fields. `name` is `None` when the
/// manifest's own identity is missing or invalid. `declared` holds every
/// validly named dependency, including one whose requirement is invalid.
pub(crate) fn relation_errors<'a>(
    name: Option<&UnitIdentity>,
    declared: &BTreeSet<UnitIdentity>,
    option_targets: impl IntoIterator<Item = &'a UnitIdentity>,
) -> Vec<ManifestError> {
    let mut errors = Vec::new();
    if let Some(name) = name {
        if declared.contains(name) {
            errors.push(ManifestError::SelfDependency(name.clone()));
        }
    }
    let mut undeclared: Vec<&UnitIdentity> = option_targets
        .into_iter()
        .filter(|target| !declared.contains(*target))
        .collect();
    undeclared.sort();
    undeclared.dedup();
    errors.extend(
        undeclared
            .into_iter()
            .map(|target| ManifestError::UndeclaredOptionTarget(target.clone())),
    );
    errors
}

/// A manifest whose parts are individually valid but inconsistent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    /// The unit declares a dependency on itself.
    SelfDependency(UnitIdentity),
    /// `options` targets an identity that is not a declared dependency.
    UndeclaredOptionTarget(UnitIdentity),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SelfDependency(name) => {
                write!(f, "unit {name} declares a dependency on itself")
            }
            Self::UndeclaredOptionTarget(target) => write!(
                f,
                "{target} is not declared in `dependencies`; options may only target direct dependencies"
            ),
        }
    }
}

impl std::error::Error for ManifestError {}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{Manifest, ManifestError, OptionValues};
    use crate::domain::{
        Description, ParameterName, ParameterValue, UnitIdentity, Version, VersionRequirement,
    };

    fn identity(input: &str) -> UnitIdentity {
        input.parse().unwrap()
    }

    fn manifest(
        dependencies: &[&str],
        option_targets: &[&str],
    ) -> Result<Manifest, Vec<ManifestError>> {
        let dependencies = dependencies
            .iter()
            .map(|name| (identity(name), VersionRequirement::parse("^1.0.0").unwrap()))
            .collect();
        let option_values: OptionValues = [(
            ParameterName::parse("enabled").unwrap(),
            ParameterValue::Boolean(true),
        )]
        .into();
        let options = option_targets
            .iter()
            .map(|name| (identity(name), option_values.clone()))
            .collect();
        Manifest::new(
            identity("@metamao/app"),
            Version::parse("1.0.0").unwrap(),
            Description::new("An app.").unwrap(),
            dependencies,
            options,
            BTreeMap::new(),
        )
    }

    #[test]
    fn options_may_target_declared_dependencies() {
        let manifest = manifest(&["@metamao/node"], &["@metamao/node"]).unwrap();
        assert_eq!(manifest.options().len(), 1);
        assert_eq!(manifest.dependencies().len(), 1);
    }

    #[test]
    fn every_inconsistency_is_reported_in_order() {
        let errors = manifest(&["@metamao/app"], &["@z/z", "@metamao/app", "@a/a"]).unwrap_err();
        assert_eq!(
            errors,
            [
                ManifestError::SelfDependency(identity("@metamao/app")),
                ManifestError::UndeclaredOptionTarget(identity("@a/a")),
                ManifestError::UndeclaredOptionTarget(identity("@z/z")),
            ]
        );
    }

    #[test]
    fn errors_explain_the_rule() {
        let errors = manifest(&[], &["@metamao/node"]).unwrap_err();
        let message = errors[0].to_string();
        assert!(message.contains("@metamao/node"), "{message}");
        assert!(message.contains("direct dependencies"), "{message}");
    }
}
