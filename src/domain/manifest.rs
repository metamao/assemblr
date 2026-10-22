//! The unit manifest: a unit's identity, version, description, dependencies,
//! dependency options, parameters, and amendment declarations.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use super::identity::UnitIdentity;
use super::parameter::{Description, ParameterDefinition, ParameterName, ParameterValue};
use super::path::RelativePath;
use super::version::{Version, VersionRequirement};

/// The values a consumer requires for one unit's parameters.
pub type OptionValues = BTreeMap<ParameterName, ParameterValue>;

/// One requirement a unit imposes on a dependency's parameter: a value, or a
/// mapping from one of its own parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionValue {
    /// A fixed value.
    Literal(ParameterValue),
    /// The effective value of the unit's own parameter of this name, which
    /// the dependency's parameter is then required to have. Nothing is
    /// transformed or renamed on the way.
    From(ParameterName),
}

/// What a unit requires for one dependency's parameters.
pub type UnitOptionValues = BTreeMap<ParameterName, OptionValue>;

/// The provenance of one amendment: the direct dependency whose `src/` file
/// the copy at `amendments/<path>` was taken from, and the exact published
/// version it was taken at (see `docs/domain/amendments-and-composition.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmendmentDeclaration {
    unit: UnitIdentity,
    base: Version,
}

impl AmendmentDeclaration {
    /// An amendment of `unit`'s file at version `base`.
    pub fn new(unit: UnitIdentity, base: Version) -> Self {
        Self { unit, base }
    }

    /// The direct dependency that provides the amended file.
    pub fn unit(&self) -> &UnitIdentity {
        &self.unit
    }

    /// The exact version of that dependency the copy was taken from.
    pub fn base(&self) -> &Version {
        &self.base
    }
}

/// A validated unit manifest.
///
/// Invariants beyond those of its parts:
///
/// - every `options` key is a declared dependency, because options target
///   only direct dependencies;
/// - every amendment targets a declared dependency, because an amendment
///   targets only a direct dependency;
/// - a unit never depends on itself.
///
/// Whether an option value suits the dependency's parameter, and whether an
/// amended path exists in the dependency's `src/`, depend on the dependency's
/// own content, so they are checked during composition, not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    name: UnitIdentity,
    version: Version,
    description: Description,
    dependencies: BTreeMap<UnitIdentity, VersionRequirement>,
    options: BTreeMap<UnitIdentity, UnitOptionValues>,
    params: BTreeMap<ParameterName, ParameterDefinition>,
    amendments: BTreeMap<RelativePath, AmendmentDeclaration>,
}

impl Manifest {
    /// Creates a manifest, reporting every invariant it would break.
    pub fn new(
        name: UnitIdentity,
        version: Version,
        description: Description,
        dependencies: BTreeMap<UnitIdentity, VersionRequirement>,
        options: BTreeMap<UnitIdentity, UnitOptionValues>,
        params: BTreeMap<ParameterName, ParameterDefinition>,
        amendments: BTreeMap<RelativePath, AmendmentDeclaration>,
    ) -> Result<Self, Vec<ManifestError>> {
        let declared = dependencies.keys().cloned().collect();
        let errors = relation_errors(
            Some(&name),
            &declared,
            options.keys(),
            amendments
                .iter()
                .map(|(path, declaration)| (path, &declaration.unit)),
        );
        let mut errors = errors;
        for (target, values) in &options {
            for (parameter, value) in values {
                if let OptionValue::From(source) = value {
                    if !params.contains_key(source) {
                        errors.push(ManifestError::UndeclaredMappingSource {
                            target: target.clone(),
                            parameter: parameter.clone(),
                            source: source.clone(),
                        });
                    }
                }
            }
        }
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
            amendments,
        })
    }

    /// The same manifest with `declaration` added for `path`, or `None` when
    /// `path` is already amended or the target is not a direct dependency.
    pub fn with_amendment(
        &self,
        path: RelativePath,
        declaration: AmendmentDeclaration,
    ) -> Option<Self> {
        if self.amendments.contains_key(&path) || !self.dependencies.contains_key(&declaration.unit)
        {
            return None;
        }
        let mut manifest = self.clone();
        manifest.amendments.insert(path, declaration);
        Some(manifest)
    }

    /// The same manifest with the base of each amendment in `bases` replaced
    /// by the given version, or `None` when a path is not amended.
    pub fn with_bases(&self, bases: &BTreeMap<RelativePath, Version>) -> Option<Self> {
        let mut manifest = self.clone();
        for (path, base) in bases {
            let declaration = manifest.amendments.get_mut(path)?;
            declaration.base = base.clone();
        }
        Some(manifest)
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
    pub fn options(&self) -> &BTreeMap<UnitIdentity, UnitOptionValues> {
        &self.options
    }

    /// The unit's own public parameters.
    pub fn params(&self) -> &BTreeMap<ParameterName, ParameterDefinition> {
        &self.params
    }

    /// The unit's amendments, by the amended file's path in the provider's
    /// `src/`, which is also the copy's path under `amendments/`.
    pub fn amendments(&self) -> &BTreeMap<RelativePath, AmendmentDeclaration> {
        &self.amendments
    }
}

/// Checks the relations between a manifest's name, dependencies, option
/// targets, and amendment targets, in deterministic order.
///
/// Readers call this with whatever parts are valid, so these problems are
/// reported together with problems in other fields. `name` is `None` when the
/// manifest's own identity is missing or invalid. `declared` holds every
/// validly named dependency, including one whose requirement is invalid.
pub(crate) fn relation_errors<'a>(
    name: Option<&UnitIdentity>,
    declared: &BTreeSet<UnitIdentity>,
    option_targets: impl IntoIterator<Item = &'a UnitIdentity>,
    amendment_targets: impl IntoIterator<Item = (&'a RelativePath, &'a UnitIdentity)>,
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
    let mut undeclared: Vec<(&RelativePath, &UnitIdentity)> = amendment_targets
        .into_iter()
        .filter(|(_, target)| !declared.contains(*target))
        .collect();
    undeclared.sort();
    errors.extend(undeclared.into_iter().map(|(path, target)| {
        ManifestError::UndeclaredAmendmentTarget {
            path: path.clone(),
            unit: target.clone(),
        }
    }));
    errors
}

/// A manifest whose parts are individually valid but inconsistent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    /// The unit declares a dependency on itself.
    SelfDependency(UnitIdentity),
    /// `options` targets an identity that is not a declared dependency.
    UndeclaredOptionTarget(UnitIdentity),
    /// A mapping in `options` names a parameter the unit does not declare.
    UndeclaredMappingSource {
        /// The dependency the option targets.
        target: UnitIdentity,
        /// The dependency's parameter the mapping configures.
        parameter: ParameterName,
        /// The parameter the mapping names.
        source: ParameterName,
    },
    /// An amendment targets an identity that is not a declared dependency.
    UndeclaredAmendmentTarget {
        /// The amended path.
        path: RelativePath,
        /// The identity the declaration names.
        unit: UnitIdentity,
    },
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
            Self::UndeclaredMappingSource {
                target,
                parameter,
                source,
            } => write!(
                f,
                "the option `{parameter}` of {target} is mapped from `{source}`, which is not a \
                 parameter this unit declares in `params`"
            ),
            Self::UndeclaredAmendmentTarget { path, unit } => write!(
                f,
                "the amendment of {path} names {unit}, which is not declared in `dependencies`; \
                 an amendment targets a direct dependency"
            ),
        }
    }
}

impl std::error::Error for ManifestError {}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{AmendmentDeclaration, Manifest, ManifestError, OptionValue, UnitOptionValues};
    use crate::domain::{
        Description, ParameterName, ParameterValue, RelativePath, UnitIdentity, Version,
        VersionRequirement,
    };

    fn identity(input: &str) -> UnitIdentity {
        input.parse().unwrap()
    }

    fn path(input: &str) -> RelativePath {
        input.parse().unwrap()
    }

    fn declaration(unit: &str) -> AmendmentDeclaration {
        AmendmentDeclaration::new(identity(unit), Version::parse("1.2.0").unwrap())
    }

    fn manifest(
        dependencies: &[&str],
        option_targets: &[&str],
        amendments: &[(&str, &str)],
    ) -> Result<Manifest, Vec<ManifestError>> {
        let dependencies = dependencies
            .iter()
            .map(|name| (identity(name), VersionRequirement::parse("^1.0.0").unwrap()))
            .collect();
        let option_values: UnitOptionValues = [(
            ParameterName::parse("enabled").unwrap(),
            OptionValue::Literal(ParameterValue::Boolean(true)),
        )]
        .into();
        let options = option_targets
            .iter()
            .map(|name| (identity(name), option_values.clone()))
            .collect();
        let amendments = amendments
            .iter()
            .map(|(amended, unit)| (path(amended), declaration(unit)))
            .collect();
        Manifest::new(
            identity("@metamao/app"),
            Version::parse("1.0.0").unwrap(),
            Description::new("An app.").unwrap(),
            dependencies,
            options,
            BTreeMap::new(),
            amendments,
        )
    }

    #[test]
    fn options_and_amendments_may_target_declared_dependencies() {
        let manifest = manifest(
            &["@metamao/node"],
            &["@metamao/node"],
            &[("package.json", "@metamao/node")],
        )
        .unwrap();
        assert_eq!(manifest.options().len(), 1);
        assert_eq!(manifest.dependencies().len(), 1);
        let amendment = &manifest.amendments()[&path("package.json")];
        assert_eq!(amendment.unit(), &identity("@metamao/node"));
        assert_eq!(amendment.base().to_string(), "1.2.0");
    }

    #[test]
    fn every_inconsistency_is_reported_in_order() {
        let errors = manifest(
            &["@metamao/app"],
            &["@z/z", "@metamao/app", "@a/a"],
            &[
                ("z.txt", "@z/z"),
                ("a.txt", "@metamao/app"),
                ("b.txt", "@a/a"),
            ],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [
                ManifestError::SelfDependency(identity("@metamao/app")),
                ManifestError::UndeclaredOptionTarget(identity("@a/a")),
                ManifestError::UndeclaredOptionTarget(identity("@z/z")),
                ManifestError::UndeclaredAmendmentTarget {
                    path: path("b.txt"),
                    unit: identity("@a/a"),
                },
                ManifestError::UndeclaredAmendmentTarget {
                    path: path("z.txt"),
                    unit: identity("@z/z"),
                },
            ]
        );
    }

    #[test]
    fn errors_explain_the_rule() {
        let errors =
            manifest(&[], &["@metamao/node"], &[("a.txt", "@metamao/docker")]).unwrap_err();
        let message = errors[0].to_string();
        assert!(message.contains("@metamao/node"), "{message}");
        assert!(message.contains("direct dependencies"), "{message}");
        let message = errors[1].to_string();
        assert!(message.contains("a.txt"), "{message}");
        assert!(message.contains("@metamao/docker"), "{message}");
        assert!(message.contains("direct dependency"), "{message}");
    }

    #[test]
    fn an_amendment_is_added_once_and_only_for_a_direct_dependency() {
        let manifest = manifest(&["@metamao/node"], &[], &[]).unwrap();
        let amended = manifest
            .with_amendment(path("package.json"), declaration("@metamao/node"))
            .unwrap();
        assert_eq!(amended.amendments().len(), 1);
        assert!(
            amended
                .with_amendment(path("package.json"), declaration("@metamao/node"))
                .is_none()
        );
        assert!(
            manifest
                .with_amendment(path("package.json"), declaration("@metamao/docker"))
                .is_none()
        );
    }
}
