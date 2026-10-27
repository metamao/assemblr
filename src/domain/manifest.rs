//! The unit manifest: a unit's identity, version, description,
//! dependencies, placeholders, and amendment declarations.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use super::identity::UnitIdentity;
use super::path::RelativePath;
use super::placeholder::Placeholders;
use super::version::{Version, VersionRequirement};

/// A required human-readable description that is not empty or only
/// whitespace. The text is kept exactly as written.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Description(String);

impl Description {
    /// Accepts any text that contains something other than whitespace.
    pub fn new(text: &str) -> Result<Self, BlankDescription> {
        if text.trim().is_empty() {
            Err(BlankDescription)
        } else {
            Ok(Self(text.to_owned()))
        }
    }

    /// The description as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Description {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A description that is empty or only whitespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlankDescription;

impl fmt::Display for BlankDescription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the description must not be empty or only whitespace")
    }
}

impl std::error::Error for BlankDescription {}

/// The provenance of one amendment: the unit whose `src/` file the copy at
/// `amendments/<path>` was taken from, a dependency direct or transitive,
/// and the exact published version it was taken at (see
/// `docs/domain/amendments-and-composition.md`).
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

    /// The unit that provides the amended file.
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
/// Beyond the invariants of its parts, a unit never depends on itself.
/// Whether an amendment's provider is among the unit's dependencies, direct
/// or transitive, whether an amended path exists in the provider's `src/`,
/// and whether the unit's placeholders suit its files and its graph depend
/// on content beyond the manifest, so they are checked during composition
/// and the build, not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    name: UnitIdentity,
    version: Version,
    description: Description,
    dependencies: BTreeMap<UnitIdentity, VersionRequirement>,
    placeholders: Placeholders,
    amendments: BTreeMap<RelativePath, AmendmentDeclaration>,
}

impl Manifest {
    /// Creates a manifest, reporting every invariant it would break.
    pub fn new(
        name: UnitIdentity,
        version: Version,
        description: Description,
        dependencies: BTreeMap<UnitIdentity, VersionRequirement>,
        placeholders: Placeholders,
        amendments: BTreeMap<RelativePath, AmendmentDeclaration>,
    ) -> Result<Self, Vec<ManifestError>> {
        let declared = dependencies.keys().cloned().collect();
        let errors = relation_errors(Some(&name), &declared);
        if !errors.is_empty() {
            return Err(errors);
        }
        Ok(Self {
            name,
            version,
            description,
            dependencies,
            placeholders,
            amendments,
        })
    }

    /// The same manifest with `declaration` added for `path`, or `None` when
    /// `path` is already amended.
    pub fn with_amendment(
        &self,
        path: RelativePath,
        declaration: AmendmentDeclaration,
    ) -> Option<Self> {
        if self.amendments.contains_key(&path) {
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

    /// The same manifest without amendments.
    pub fn without_amendments(&self) -> Self {
        let mut manifest = self.clone();
        manifest.amendments.clear();
        manifest
    }

    /// The same manifest with `dependency` required as `requirement`, in
    /// place of any requirement it had, or `None` when that is the unit
    /// itself.
    pub fn with_dependency(
        &self,
        dependency: UnitIdentity,
        requirement: VersionRequirement,
    ) -> Option<Self> {
        if dependency == self.name {
            return None;
        }
        let mut manifest = self.clone();
        manifest.dependencies.insert(dependency, requirement);
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

    /// The placeholders the unit's files hold for the consumer to replace,
    /// and the tokens that only look like placeholders.
    pub fn placeholders(&self) -> &Placeholders {
        &self.placeholders
    }

    /// The unit's amendments, by the amended file's path in the provider's
    /// `src/`, which is also the copy's path under `amendments/`.
    pub fn amendments(&self) -> &BTreeMap<RelativePath, AmendmentDeclaration> {
        &self.amendments
    }
}

/// Checks the relation between a manifest's name and its dependencies.
///
/// Readers call this with whatever parts are valid, so these problems are
/// reported together with problems in other fields. `name` is `None` when the
/// manifest's own identity is missing or invalid. `declared` holds every
/// validly named dependency, including one whose requirement is invalid.
pub(crate) fn relation_errors(
    name: Option<&UnitIdentity>,
    declared: &BTreeSet<UnitIdentity>,
) -> Vec<ManifestError> {
    match name {
        Some(name) if declared.contains(name) => vec![ManifestError::SelfDependency(name.clone())],
        _ => Vec::new(),
    }
}

/// A manifest whose parts are individually valid but inconsistent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    /// The unit declares a dependency on itself.
    SelfDependency(UnitIdentity),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SelfDependency(name) => {
                write!(f, "unit {name} declares a dependency on itself")
            }
        }
    }
}

impl std::error::Error for ManifestError {}

#[cfg(test)]
mod tests {
    use super::{AmendmentDeclaration, Description, Manifest, ManifestError};
    use crate::domain::{Placeholders, RelativePath, UnitIdentity, Version, VersionRequirement};

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
        amendments: &[(&str, &str)],
    ) -> Result<Manifest, Vec<ManifestError>> {
        let dependencies = dependencies
            .iter()
            .map(|name| (identity(name), VersionRequirement::parse("^1.0.0").unwrap()))
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
            Placeholders::default(),
            amendments,
        )
    }

    #[test]
    fn amendments_may_target_declared_dependencies() {
        let manifest = manifest(&["@metamao/node"], &[("package.json", "@metamao/node")]).unwrap();
        assert_eq!(manifest.dependencies().len(), 1);
        let amendment = &manifest.amendments()[&path("package.json")];
        assert_eq!(amendment.unit(), &identity("@metamao/node"));
        assert_eq!(amendment.base().to_string(), "1.2.0");
        assert!(manifest.without_amendments().amendments().is_empty());
    }

    #[test]
    fn a_unit_cannot_depend_on_itself() {
        let errors = manifest(&["@metamao/app"], &[]).unwrap_err();
        assert_eq!(
            errors,
            [ManifestError::SelfDependency(identity("@metamao/app"))]
        );
        assert!(errors[0].to_string().contains("on itself"));
    }

    #[test]
    fn a_dependency_is_added_or_replaced_but_never_the_unit_itself() {
        let manifest = manifest(&["@metamao/node"], &[]).unwrap();
        let requirement = VersionRequirement::parse("^2.0.0").unwrap();
        let added = manifest
            .with_dependency(identity("@metamao/docker"), requirement.clone())
            .unwrap();
        assert_eq!(added.dependencies().len(), 2);
        assert!(
            manifest
                .with_dependency(identity("@metamao/app"), requirement)
                .is_none()
        );
    }

    #[test]
    fn an_amendment_is_added_once_and_may_name_a_transitive_dependency() {
        let manifest = manifest(&["@metamao/node"], &[]).unwrap();
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
                .is_some()
        );
    }
}
