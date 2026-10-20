//! Composition of a resolved graph: the effective parameter values of every
//! unit and the file tree the units' `src/` files compose into.
//!
//! [`compose`] works on plain data: the resolved graph, each resolved unit's
//! manifest and `src/` files, the root, and the values the root supplies. It
//! either returns the composition or every problem that prevents it.
//!
//! # Parameter values
//!
//! A unit's parameter receives requirements from the `options` of the units
//! that depend on it directly and, for an explicit selection, from the root.
//! Each requirement must name a parameter the resolved version declares and
//! satisfy its type. Identical requirements are compatible. Different ones
//! are an options conflict: no requirer takes precedence, whatever its depth
//! or declaration order. A parameter without requirements takes its default.
//! One without a default is required, and the error names who must supply
//! it: the units that depend on it directly, and the root when it is an
//! explicit selection or the unit being built. A unit never configures a
//! transitive dependency, and neither does the root.
//!
//! # Files
//!
//! The composed file tree is the union of the units' `src/` files, each
//! recording the unit that contributed it. File contents are opaque bytes
//! with an executable bit, delivered unchanged: nothing is interpolated until
//! a template syntax exists (OC-5). Two files collide when their paths are
//! equal or differ only in case, which a case-insensitive file system cannot
//! keep apart, or when one file's path is a directory of another's. Case is
//! compared by Unicode lowercasing, without Unicode normalization. Every
//! collision is reported, whether the units are independent or one depends
//! on the other, and no unit wins.
//!
//! Every problem is reported, in the order [`compose`] documents.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use super::identity::UnitIdentity;
use super::manifest::{Manifest, OptionValues};
use super::parameter::{ParameterName, ParameterValue, ParameterValueError};
use super::path::RelativePath;
use super::resolution::{Requirer, ResolvedGraph};
use super::version::Version;

/// A unit's `src/` files, by path relative to `src/`.
pub type FileTree = BTreeMap<RelativePath, UnitFile>;

/// The effective value of each of a unit's parameters.
pub type ParameterValues = BTreeMap<ParameterName, ParameterValue>;

/// One file of a unit: its bytes and whether it is executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitFile {
    contents: Vec<u8>,
    executable: bool,
}

impl UnitFile {
    /// A file with these exact bytes.
    pub fn new(contents: Vec<u8>, executable: bool) -> Self {
        Self {
            contents,
            executable,
        }
    }

    /// The file's bytes, exactly as read.
    pub fn contents(&self) -> &[u8] {
        &self.contents
    }

    /// Whether the file is executable.
    pub fn is_executable(&self) -> bool {
        self.executable
    }
}

/// What composition needs from one unit: its manifest and its `src/` files.
#[derive(Debug, Clone, Copy)]
pub struct UnitContent<'a> {
    manifest: &'a Manifest,
    files: &'a FileTree,
}

impl<'a> UnitContent<'a> {
    /// The content of the unit `manifest` describes.
    pub fn new(manifest: &'a Manifest, files: &'a FileTree) -> Self {
        Self { manifest, files }
    }
}

/// What the resolved graph is composed for.
#[derive(Debug, Clone, Copy)]
pub enum Root<'a> {
    /// A consumer project. Its explicit selections are the graph's root
    /// requirements, and its values may configure only them.
    Project,
    /// A unit built from its working tree. Its manifest declares the graph's
    /// root requirements, it is composed with its dependencies, and the
    /// root's values may configure only it.
    Unit(UnitContent<'a>),
}

/// Composes `graph` for `root`.
///
/// `units` holds the content of every resolved unit at its resolved version;
/// entries for identities outside the graph are ignored. `root_values` holds
/// the values the root supplies, by the unit they configure: the project's
/// `options` for a consumer project, or the values a unit build receives for
/// the unit being built. On failure every problem is reported: each cause
/// once, in this order, each kind sorted by unit, parameter or path, and then
/// requirer:
///
/// 1. content that does not match the resolved graph;
/// 2. root values for a unit the root may not configure;
/// 3. requirements naming undeclared parameters or holding invalid values;
/// 4. options conflicts;
/// 5. missing required values;
/// 6. path collisions.
///
/// A parameter whose requirements are invalid is not also reported as
/// missing or conflicting.
pub fn compose<'a>(
    graph: &ResolvedGraph,
    root: Root<'a>,
    units: &BTreeMap<UnitIdentity, UnitContent<'a>>,
    root_values: &BTreeMap<UnitIdentity, OptionValues>,
) -> Result<Composition<'a>, Vec<CompositionError>> {
    let members = members(graph, root, units)?;
    let mut errors = root_target_errors(graph, root, &members, root_values);
    let (values, parameter_errors) = parameter_values(graph, root, &members, root_values);
    errors.extend(parameter_errors);
    let (files, collisions) = compose_files(&members);
    errors.extend(collisions);
    if errors.is_empty() {
        Ok(Composition { values, files })
    } else {
        Err(errors)
    }
}

/// The result of composing a resolved graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composition<'a> {
    values: BTreeMap<UnitIdentity, ParameterValues>,
    files: BTreeMap<RelativePath, ComposedFile<'a>>,
}

impl<'a> Composition<'a> {
    /// The effective parameter values of every composed unit, including the
    /// unit being built. A unit without parameters has an empty entry.
    pub fn values(&self) -> &BTreeMap<UnitIdentity, ParameterValues> {
        &self.values
    }

    /// The composed file tree, by path.
    pub fn files(&self) -> &BTreeMap<RelativePath, ComposedFile<'a>> {
        &self.files
    }
}

/// A file in the composed tree and the unit that contributed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComposedFile<'a> {
    unit: &'a UnitIdentity,
    file: &'a UnitFile,
}

impl<'a> ComposedFile<'a> {
    /// The unit whose `src/` provides the file.
    pub fn unit(&self) -> &'a UnitIdentity {
        self.unit
    }

    /// The file, unchanged.
    pub fn file(&self) -> &'a UnitFile {
        self.file
    }
}

/// A value required for a parameter and who requires it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionRequirement {
    requirer: Requirer,
    value: ParameterValue,
}

impl OptionRequirement {
    /// Who requires the value: the root, or a unit through its `options`.
    pub fn requirer(&self) -> &Requirer {
        &self.requirer
    }

    /// The required value.
    pub fn value(&self) -> &ParameterValue {
        &self.value
    }
}

impl fmt::Display for OptionRequirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} from {}", self.value, self.requirer)
    }
}

/// A file a unit contributes, at its path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ContributedPath {
    path: RelativePath,
    unit: UnitIdentity,
}

impl ContributedPath {
    /// The file's path.
    pub fn path(&self) -> &RelativePath {
        &self.path
    }

    /// The unit that contributes it.
    pub fn unit(&self) -> &UnitIdentity {
        &self.unit
    }
}

impl fmt::Display for ContributedPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} from {}", self.path, self.unit)
    }
}

/// Why the root may not configure a unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootTargetReason {
    /// The unit is not part of the composition.
    NotComposed,
    /// The unit is part of the composition only as a dependency of other
    /// units, not as an explicit selection.
    Transitive,
    /// A unit build's values configure only the unit being built, and this
    /// is another unit.
    NotTheBuiltUnit,
}

/// A problem that prevents composition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompositionError {
    /// The content supplied for a unit does not match the resolved graph:
    /// it is missing, or its manifest declares another identity, version, or
    /// set of dependencies. This is a defect in the caller, not in the unit.
    ContentMismatch {
        /// The unit whose content does not match.
        unit: UnitIdentity,
        /// The version the graph resolved, or the built unit's own version.
        version: Version,
    },
    /// The root supplies values for a unit it may not configure.
    RootTarget {
        /// The unit the values target.
        unit: UnitIdentity,
        /// Why the root may not configure it.
        reason: RootTargetReason,
    },
    /// A requirement sets a parameter that the unit's resolved version does
    /// not declare.
    UndeclaredParameter {
        /// The unit the requirement targets.
        unit: UnitIdentity,
        /// Its resolved version.
        version: Version,
        /// The parameter it does not declare.
        parameter: ParameterName,
        /// Who sets the parameter.
        requirer: Requirer,
        /// The parameters the unit declares, sorted.
        declared: Vec<ParameterName>,
    },
    /// A requirement's value does not satisfy the parameter's type or
    /// allowed values.
    InvalidValue {
        /// The unit the requirement targets.
        unit: UnitIdentity,
        /// Its resolved version.
        version: Version,
        /// The parameter.
        parameter: ParameterName,
        /// The invalid requirement.
        requirement: OptionRequirement,
        /// Why the value is invalid.
        error: ParameterValueError,
    },
    /// Requirements set different values for the same parameter.
    OptionsConflict {
        /// The unit whose parameter is in conflict.
        unit: UnitIdentity,
        /// Its resolved version.
        version: Version,
        /// The parameter.
        parameter: ParameterName,
        /// Every requirement on the parameter, ordered by requirer.
        requirements: Vec<OptionRequirement>,
    },
    /// A required parameter receives no value.
    MissingValue {
        /// The unit whose parameter has no value.
        unit: UnitIdentity,
        /// Its resolved version.
        version: Version,
        /// The required parameter.
        parameter: ParameterName,
        /// Who may supply the value: the units that depend on this one
        /// directly, and the root when it may configure this unit.
        responsible: Vec<Requirer>,
    },
    /// Files whose paths are equal or differ only in case.
    PathCollision {
        /// Every colliding file, ordered by path and then unit.
        files: Vec<ContributedPath>,
    },
    /// A file's path is a directory of other files' paths, compared
    /// ignoring case.
    NestedPathCollision {
        /// The file whose path the others need as a directory.
        file: ContributedPath,
        /// The files beneath it, ordered by path and then unit.
        nested: Vec<ContributedPath>,
    },
}

impl fmt::Display for CompositionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ContentMismatch { unit, version } => write!(
                f,
                "internal error: the content supplied for {unit} {version} does not match \
                 the resolved dependency graph"
            ),
            Self::RootTarget { unit, reason } => match reason {
                RootTargetReason::NotComposed => write!(
                    f,
                    "values are supplied for {unit}, which is not part of the composition; \
                     values may configure only explicitly selected units"
                ),
                RootTargetReason::Transitive => write!(
                    f,
                    "values are supplied for {unit}, which is only a dependency of other \
                     units; values may configure only explicitly selected units, and the units \
                     that depend on {unit} configure it through their `options`"
                ),
                RootTargetReason::NotTheBuiltUnit => write!(
                    f,
                    "values are supplied for {unit}, which is not the unit being built; the \
                     unit configures its dependencies through its `options`"
                ),
            },
            Self::UndeclaredParameter {
                unit,
                version,
                parameter,
                requirer,
                declared,
            } => {
                write!(
                    f,
                    "{requirer} sets parameter `{parameter}` of {unit} {version}, which does \
                     not declare it; "
                )?;
                if declared.is_empty() {
                    f.write_str("it declares no parameters")
                } else {
                    f.write_str("its parameters are ")?;
                    write_list(f, declared.iter().map(|name| format!("`{name}`")))
                }
            }
            Self::InvalidValue {
                unit,
                version,
                parameter,
                requirement,
                error,
            } => write!(
                f,
                "{} sets parameter `{parameter}` of {unit} {version} to {}: {error}",
                requirement.requirer, requirement.value
            ),
            Self::OptionsConflict {
                unit,
                version,
                parameter,
                requirements,
            } => {
                write!(
                    f,
                    "options conflict on parameter `{parameter}` of {unit} {version}: "
                )?;
                write_list(f, requirements)?;
                f.write_str("; no requirement takes precedence, so they must agree")
            }
            Self::MissingValue {
                unit,
                version,
                parameter,
                responsible,
            } => {
                write!(
                    f,
                    "required parameter `{parameter}` of {unit} {version} has no value and no \
                     default; it must be supplied by "
                )?;
                write_list(f, responsible)
            }
            Self::PathCollision { files } => {
                f.write_str("path collision: ")?;
                write_list(f, files)?;
                let first = files.first().map(ContributedPath::path);
                if files.iter().all(|file| Some(file.path()) == first) {
                    f.write_str(
                        "; no unit takes precedence, and a unit changes a dependency's file \
                         through an amendment",
                    )
                } else {
                    f.write_str(
                        "; these paths differ only in case, which a case-insensitive file \
                         system cannot keep apart",
                    )
                }
            }
            Self::NestedPathCollision { file, nested } => {
                write!(f, "path collision: {file} is a file, but ")?;
                write_list(f, nested)?;
                f.write_str(" need it to be a directory")
            }
        }
    }
}

impl std::error::Error for CompositionError {}

fn write_list<T: fmt::Display>(
    f: &mut fmt::Formatter<'_>,
    items: impl IntoIterator<Item = T>,
) -> fmt::Result {
    for (index, item) in items.into_iter().enumerate() {
        if index > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{item}")?;
    }
    Ok(())
}

/// A unit taking part in the composition: a resolved unit, or the unit
/// being built.
struct Member<'a> {
    identity: &'a UnitIdentity,
    manifest: &'a Manifest,
    files: &'a FileTree,
    /// How the unit appears as the requirer of its `options`.
    requirer: Requirer,
}

/// Every unit taking part in the composition, by identity, after checking
/// that the supplied content matches the graph.
fn members<'a>(
    graph: &ResolvedGraph,
    root: Root<'a>,
    units: &BTreeMap<UnitIdentity, UnitContent<'a>>,
) -> Result<BTreeMap<&'a UnitIdentity, Member<'a>>, Vec<CompositionError>> {
    let mut members = BTreeMap::new();
    let mut errors = Vec::new();
    if let Root::Unit(content) = root {
        let manifest = content.manifest;
        if manifest.dependencies() == graph.root() && !graph.units().contains_key(manifest.name()) {
            members.insert(
                manifest.name(),
                Member {
                    identity: manifest.name(),
                    manifest,
                    files: content.files,
                    requirer: Requirer::Root,
                },
            );
        } else {
            errors.push(CompositionError::ContentMismatch {
                unit: manifest.name().clone(),
                version: manifest.version().clone(),
            });
        }
    }
    for (identity, resolved) in graph.units() {
        let content = units.get(identity).filter(|content| {
            let manifest = content.manifest;
            manifest.name() == identity
                && manifest.version() == resolved.version()
                && manifest.dependencies() == resolved.dependencies()
        });
        match content {
            Some(content) => {
                let manifest = content.manifest;
                members.insert(
                    manifest.name(),
                    Member {
                        identity: manifest.name(),
                        manifest,
                        files: content.files,
                        requirer: Requirer::Unit(identity.clone(), resolved.version().clone()),
                    },
                );
            }
            None => errors.push(CompositionError::ContentMismatch {
                unit: identity.clone(),
                version: resolved.version().clone(),
            }),
        }
    }
    if errors.is_empty() {
        Ok(members)
    } else {
        Err(errors)
    }
}

/// Whether the root may configure `unit`, and if not, why.
fn root_target(
    graph: &ResolvedGraph,
    root: Root<'_>,
    members: &BTreeMap<&UnitIdentity, Member<'_>>,
    unit: &UnitIdentity,
) -> Result<(), RootTargetReason> {
    match root {
        Root::Unit(content) if content.manifest.name() == unit => Ok(()),
        Root::Unit(_) => Err(RootTargetReason::NotTheBuiltUnit),
        Root::Project if graph.root().contains_key(unit) => Ok(()),
        Root::Project if members.contains_key(unit) => Err(RootTargetReason::Transitive),
        Root::Project => Err(RootTargetReason::NotComposed),
    }
}

fn root_target_errors(
    graph: &ResolvedGraph,
    root: Root<'_>,
    members: &BTreeMap<&UnitIdentity, Member<'_>>,
    root_values: &BTreeMap<UnitIdentity, OptionValues>,
) -> Vec<CompositionError> {
    root_values
        .keys()
        .filter_map(|unit| {
            root_target(graph, root, members, unit).err().map(|reason| {
                CompositionError::RootTarget {
                    unit: unit.clone(),
                    reason,
                }
            })
        })
        .collect()
}

/// A requirement borrowed from the inputs.
type Requested<'a> = (&'a Requirer, &'a ParameterValue);

/// The effective parameter values of every member, and the errors that
/// prevent some of them, in the documented order.
fn parameter_values(
    graph: &ResolvedGraph,
    root: Root<'_>,
    members: &BTreeMap<&UnitIdentity, Member<'_>>,
    root_values: &BTreeMap<UnitIdentity, OptionValues>,
) -> (
    BTreeMap<UnitIdentity, ParameterValues>,
    Vec<CompositionError>,
) {
    let root_requirer = Requirer::Root;
    // Every requirement on each parameter of each unit, from members'
    // options and from the root values the root may supply.
    let mut requested: BTreeMap<(&UnitIdentity, &ParameterName), Vec<Requested<'_>>> =
        BTreeMap::new();
    for member in members.values() {
        for (target, values) in member.manifest.options() {
            for (parameter, value) in values {
                requested
                    .entry((target, parameter))
                    .or_default()
                    .push((&member.requirer, value));
            }
        }
    }
    for (target, values) in root_values {
        if root_target(graph, root, members, target).is_ok() {
            for (parameter, value) in values {
                requested
                    .entry((target, parameter))
                    .or_default()
                    .push((&root_requirer, value));
            }
        }
    }

    let mut invalid = Vec::new();
    let mut conflicts = Vec::new();
    // The value of each parameter whose requirements agree, and every
    // parameter whose requirements were reported instead.
    let mut agreed = BTreeMap::new();
    let mut reported = BTreeSet::new();
    for ((target, parameter), mut requirements) in requested {
        requirements.sort_by(|left, right| left.0.cmp(right.0));
        // Every target is a member: options target declared dependencies,
        // which the graph resolved, and root values are kept only for
        // members.
        let Some(member) = members.get(target) else {
            continue;
        };
        let version = member.manifest.version();
        let Some(definition) = member.manifest.params().get(parameter) else {
            let declared: Vec<ParameterName> = member.manifest.params().keys().cloned().collect();
            invalid.extend(requirements.iter().map(|(requirer, _)| {
                CompositionError::UndeclaredParameter {
                    unit: target.clone(),
                    version: version.clone(),
                    parameter: parameter.clone(),
                    requirer: (*requirer).clone(),
                    declared: declared.clone(),
                }
            }));
            continue;
        };
        let invalid_values: Vec<CompositionError> = requirements
            .iter()
            .filter_map(|(requirer, value)| {
                definition
                    .check(value)
                    .err()
                    .map(|error| CompositionError::InvalidValue {
                        unit: target.clone(),
                        version: version.clone(),
                        parameter: parameter.clone(),
                        requirement: requirement(requirer, value),
                        error,
                    })
            })
            .collect();
        match requirements.first() {
            _ if !invalid_values.is_empty() => {
                invalid.extend(invalid_values);
                reported.insert((target, parameter));
            }
            Some((_, first)) if requirements.iter().all(|(_, value)| value == first) => {
                agreed.insert((target, parameter), *first);
            }
            _ => {
                conflicts.push(CompositionError::OptionsConflict {
                    unit: target.clone(),
                    version: version.clone(),
                    parameter: parameter.clone(),
                    requirements: requirements
                        .iter()
                        .map(|(requirer, value)| requirement(requirer, value))
                        .collect(),
                });
                reported.insert((target, parameter));
            }
        }
    }

    let mut missing = Vec::new();
    let mut values = BTreeMap::new();
    for (&identity, member) in members {
        let mut effective = ParameterValues::new();
        for (parameter, definition) in member.manifest.params() {
            let key = (identity, parameter);
            if let Some(&value) = agreed.get(&key) {
                effective.insert(parameter.clone(), value.clone());
            } else if reported.contains(&key) {
                // Its requirements were reported as invalid or conflicting.
            } else if let Some(default) = definition.default() {
                effective.insert(parameter.clone(), default.clone());
            } else {
                missing.push(CompositionError::MissingValue {
                    unit: identity.clone(),
                    version: member.manifest.version().clone(),
                    parameter: parameter.clone(),
                    responsible: responsible(graph, root, members, identity),
                });
            }
        }
        values.insert(identity.clone(), effective);
    }

    let mut errors = invalid;
    errors.extend(conflicts);
    errors.extend(missing);
    (values, errors)
}

fn requirement(requirer: &Requirer, value: &ParameterValue) -> OptionRequirement {
    OptionRequirement {
        requirer: requirer.clone(),
        value: value.clone(),
    }
}

/// Who may supply a value for `unit`'s parameters: the members that depend
/// on it directly, and the root when it may configure the unit.
fn responsible(
    graph: &ResolvedGraph,
    root: Root<'_>,
    members: &BTreeMap<&UnitIdentity, Member<'_>>,
    unit: &UnitIdentity,
) -> Vec<Requirer> {
    let mut responsible: BTreeSet<Requirer> = members
        .values()
        .filter(|member| member.manifest.dependencies().contains_key(unit))
        .map(|member| member.requirer.clone())
        .collect();
    if root_target(graph, root, members, unit).is_ok() {
        responsible.insert(Requirer::Root);
    }
    responsible.into_iter().collect()
}

/// The composed file tree, or every collision that prevents it.
fn compose_files<'a>(
    members: &BTreeMap<&'a UnitIdentity, Member<'a>>,
) -> (
    BTreeMap<RelativePath, ComposedFile<'a>>,
    Vec<CompositionError>,
) {
    // Files by their path ignoring case.
    let mut folded: BTreeMap<String, Vec<ContributedPath>> = BTreeMap::new();
    let mut files = BTreeMap::new();
    for member in members.values() {
        for (path, file) in member.files {
            folded
                .entry(path.as_str().to_lowercase())
                .or_default()
                .push(ContributedPath {
                    path: path.clone(),
                    unit: member.identity.clone(),
                });
            files.insert(
                path.clone(),
                ComposedFile {
                    unit: member.identity,
                    file,
                },
            );
        }
    }

    let mut collisions: Vec<CompositionError> = folded
        .values()
        .filter(|group| group.len() > 1)
        .map(|group| {
            let mut files = group.clone();
            files.sort();
            CompositionError::PathCollision { files }
        })
        .collect();

    let mut nested: BTreeMap<&ContributedPath, Vec<ContributedPath>> = BTreeMap::new();
    for (key, group) in &folded {
        let directories = key.match_indices('/').map(|(index, _)| &key[..index]);
        for directory in directories {
            for file in folded.get(directory).into_iter().flatten() {
                nested
                    .entry(file)
                    .or_default()
                    .extend(group.iter().cloned());
            }
        }
    }
    collisions.extend(nested.into_iter().map(|(file, mut beneath)| {
        beneath.sort();
        CompositionError::NestedPathCollision {
            file: file.clone(),
            nested: beneath,
        }
    }));

    (files, collisions)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        CompositionError, ContributedPath, FileTree, OptionRequirement, Root, RootTargetReason,
        UnitContent, UnitFile, compose,
    };
    use crate::domain::{
        AllowedValues, AvailableVersions, Dependencies, Description, Lock, Manifest, OptionValues,
        ParameterDefinition, ParameterName, ParameterType, ParameterValue, ParameterValueError,
        RelativePath, Requirer, UnitIdentity, ValueKind, Version, resolve,
    };

    fn identity(input: &str) -> UnitIdentity {
        input.parse().unwrap()
    }

    fn version(input: &str) -> Version {
        input.parse().unwrap()
    }

    fn name(input: &str) -> ParameterName {
        input.parse().unwrap()
    }

    fn path(input: &str) -> RelativePath {
        input.parse().unwrap()
    }

    fn text(value: &str) -> ParameterValue {
        ParameterValue::String(value.to_owned())
    }

    fn unit(name: &str, resolved: &str) -> Requirer {
        Requirer::Unit(identity(name), version(resolved))
    }

    fn required(parameter_type: ParameterType) -> ParameterDefinition {
        ParameterDefinition::new(
            parameter_type,
            Description::new("A parameter.").unwrap(),
            None,
        )
        .unwrap()
    }

    fn optional(parameter_type: ParameterType, default: ParameterValue) -> ParameterDefinition {
        ParameterDefinition::new(
            parameter_type,
            Description::new("A parameter.").unwrap(),
            Some(default),
        )
        .unwrap()
    }

    fn enumerated(values: &[&str]) -> ParameterType {
        ParameterType::EnumeratedString(
            AllowedValues::new(values.iter().map(|value| (*value).to_owned()).collect()).unwrap(),
        )
    }

    /// A unit for a test: its manifest fields and its `src/` files.
    #[derive(Clone)]
    struct Spec {
        name: &'static str,
        version: &'static str,
        dependencies: Vec<(&'static str, &'static str)>,
        options: Vec<(&'static str, &'static str, ParameterValue)>,
        params: Vec<(&'static str, ParameterDefinition)>,
        files: Vec<(&'static str, Vec<u8>, bool)>,
    }

    impl Spec {
        fn new(name: &'static str, version: &'static str) -> Self {
            Self {
                name,
                version,
                dependencies: Vec::new(),
                options: Vec::new(),
                params: Vec::new(),
                files: Vec::new(),
            }
        }

        fn depends(mut self, target: &'static str, range: &'static str) -> Self {
            self.dependencies.push((target, range));
            self
        }

        fn option(
            mut self,
            target: &'static str,
            parameter: &'static str,
            value: ParameterValue,
        ) -> Self {
            self.options.push((target, parameter, value));
            self
        }

        fn param(mut self, parameter: &'static str, definition: ParameterDefinition) -> Self {
            self.params.push((parameter, definition));
            self
        }

        /// Adds a file whose contents name the unit and the path.
        fn file(self, file: &'static str) -> Self {
            let contents = format!("{} {file}", self.name).into_bytes();
            self.file_with(file, contents, false)
        }

        fn file_with(mut self, file: &'static str, contents: Vec<u8>, executable: bool) -> Self {
            self.files.push((file, contents, executable));
            self
        }

        fn manifest(&self) -> Manifest {
            let mut options: BTreeMap<UnitIdentity, OptionValues> = BTreeMap::new();
            for (target, parameter, value) in &self.options {
                options
                    .entry(identity(target))
                    .or_default()
                    .insert(name(parameter), value.clone());
            }
            Manifest::new(
                identity(self.name),
                version(self.version),
                Description::new("A unit.").unwrap(),
                dependencies(&self.dependencies),
                options,
                self.params
                    .iter()
                    .map(|(parameter, definition)| (name(parameter), definition.clone()))
                    .collect(),
            )
            .unwrap()
        }

        fn tree(&self) -> FileTree {
            self.files
                .iter()
                .map(|(file, contents, executable)| {
                    (path(file), UnitFile::new(contents.clone(), *executable))
                })
                .collect()
        }
    }

    fn dependencies(entries: &[(&str, &str)]) -> Dependencies {
        entries
            .iter()
            .map(|(target, range)| (identity(target), range.parse().unwrap()))
            .collect()
    }

    /// What a test composes for: a consumer project with its explicit
    /// selections, or a unit being built.
    enum For {
        Project(Vec<(&'static str, &'static str)>),
        Build(Spec),
    }

    /// A composition with owned, comparable parts.
    #[derive(Debug, PartialEq, Eq)]
    struct Composed {
        values: Vec<(String, Vec<(String, ParameterValue)>)>,
        files: Vec<(String, String, Vec<u8>, bool)>,
    }

    type RootValue = (&'static str, &'static str, ParameterValue);

    /// Resolves `published` for `root` and composes the result.
    fn run(
        root: &For,
        published: &[Spec],
        root_values: &[RootValue],
    ) -> Result<Composed, Vec<CompositionError>> {
        let contents: Vec<(Manifest, FileTree)> = published
            .iter()
            .map(|spec| (spec.manifest(), spec.tree()))
            .collect();
        let mut available = AvailableVersions::new();
        for (manifest, _) in &contents {
            available
                .entry(manifest.name().clone())
                .or_default()
                .insert(manifest.version().clone(), manifest.dependencies().clone());
        }
        let built = match root {
            For::Build(spec) => Some((spec.manifest(), spec.tree())),
            For::Project(_) => None,
        };
        let requirements = match (root, &built) {
            (_, Some((manifest, _))) => manifest.dependencies().clone(),
            (For::Project(selections), None) => dependencies(selections),
            (For::Build(_), None) => unreachable!(),
        };
        let resolution = resolve(
            built.as_ref().map(|(manifest, _)| manifest.name()),
            &requirements,
            &available,
            &Lock::new(),
        )
        .unwrap();
        let units: BTreeMap<UnitIdentity, UnitContent<'_>> = contents
            .iter()
            .filter(|(manifest, _)| {
                resolution
                    .graph()
                    .units()
                    .get(manifest.name())
                    .is_some_and(|resolved| resolved.version() == manifest.version())
            })
            .map(|(manifest, files)| (manifest.name().clone(), UnitContent::new(manifest, files)))
            .collect();
        let root = match &built {
            Some((manifest, files)) => Root::Unit(UnitContent::new(manifest, files)),
            None => Root::Project,
        };
        let mut values: BTreeMap<UnitIdentity, OptionValues> = BTreeMap::new();
        for (target, parameter, value) in root_values {
            values
                .entry(identity(target))
                .or_default()
                .insert(name(parameter), value.clone());
        }
        compose(resolution.graph(), root, &units, &values).map(|composition| Composed {
            values: composition
                .values()
                .iter()
                .map(|(unit, values)| {
                    (
                        unit.to_string(),
                        values
                            .iter()
                            .map(|(parameter, value)| (parameter.to_string(), value.clone()))
                            .collect(),
                    )
                })
                .collect(),
            files: composition
                .files()
                .iter()
                .map(|(file, composed)| {
                    (
                        file.to_string(),
                        composed.unit().to_string(),
                        composed.file().contents().to_vec(),
                        composed.file().is_executable(),
                    )
                })
                .collect(),
        })
    }

    fn values_of(composed: &Composed, target: &str) -> Vec<(String, ParameterValue)> {
        composed
            .values
            .iter()
            .find(|(unit, _)| unit == target)
            .map(|(_, values)| values.clone())
            .unwrap()
    }

    fn values(entries: &[(&str, ParameterValue)]) -> Vec<(String, ParameterValue)> {
        entries
            .iter()
            .map(|(parameter, value)| ((*parameter).to_owned(), value.clone()))
            .collect()
    }

    fn requirement(requirer: Requirer, value: ParameterValue) -> OptionRequirement {
        OptionRequirement { requirer, value }
    }

    fn contributed(file: &str, contributor: &str) -> ContributedPath {
        ContributedPath {
            path: path(file),
            unit: identity(contributor),
        }
    }

    fn project(selections: &[(&'static str, &'static str)]) -> For {
        For::Project(selections.to_vec())
    }

    fn node() -> Spec {
        Spec::new("@metamao/node", "1.0.0")
            .param(
                "packageManager",
                optional(enumerated(&["npm", "pnpm", "bun"]), text("npm")),
            )
            .param(
                "version",
                optional(ParameterType::Integer, ParameterValue::Integer(22)),
            )
    }

    // Parameter values.

    #[test]
    fn defaults_apply_when_nothing_is_required() {
        let composed = run(&project(&[("@metamao/node", "^1")]), &[node()], &[]).unwrap();
        assert_eq!(
            values_of(&composed, "@metamao/node"),
            values(&[
                ("packageManager", text("npm")),
                ("version", ParameterValue::Integer(22)),
            ])
        );
    }

    #[test]
    fn every_composed_unit_has_values_even_without_parameters() {
        let units = [
            Spec::new("@metamao/app", "1.0.0").depends("@metamao/plain", "^1"),
            Spec::new("@metamao/plain", "1.0.0"),
        ];
        let composed = run(&project(&[("@metamao/app", "^1")]), &units, &[]).unwrap();
        let listed: Vec<&str> = composed
            .values
            .iter()
            .map(|(unit, _)| unit.as_str())
            .collect();
        assert_eq!(listed, ["@metamao/app", "@metamao/plain"]);
        assert!(composed.values.iter().all(|(_, values)| values.is_empty()));
    }

    #[test]
    fn a_dependent_s_options_override_a_default() {
        let units = [
            Spec::new("@metamao/bun-app", "1.0.0")
                .depends("@metamao/node", "^1")
                .option("@metamao/node", "packageManager", text("bun")),
            node(),
        ];
        let composed = run(&project(&[("@metamao/bun-app", "^1")]), &units, &[]).unwrap();
        assert_eq!(
            values_of(&composed, "@metamao/node"),
            values(&[
                ("packageManager", text("bun")),
                ("version", ParameterValue::Integer(22)),
            ])
        );
    }

    #[test]
    fn consumer_values_configure_explicit_selections() {
        let composed = run(
            &project(&[("@metamao/node", "^1")]),
            &[node()],
            &[("@metamao/node", "version", ParameterValue::Integer(24))],
        )
        .unwrap();
        assert_eq!(
            values_of(&composed, "@metamao/node"),
            values(&[
                ("packageManager", text("npm")),
                ("version", ParameterValue::Integer(24)),
            ])
        );
    }

    #[test]
    fn identical_requirements_are_compatible() {
        let units = [
            Spec::new("@metamao/api", "1.0.0")
                .depends("@metamao/node", "^1")
                .option("@metamao/node", "packageManager", text("pnpm")),
            Spec::new("@metamao/web", "1.0.0")
                .depends("@metamao/node", "^1")
                .option("@metamao/node", "packageManager", text("pnpm")),
            node(),
        ];
        let composed = run(
            &project(&[
                ("@metamao/api", "^1"),
                ("@metamao/web", "^1"),
                ("@metamao/node", "^1"),
            ]),
            &units,
            &[("@metamao/node", "packageManager", text("pnpm"))],
        )
        .unwrap();
        assert_eq!(
            values_of(&composed, "@metamao/node")[0],
            ("packageManager".to_owned(), text("pnpm"))
        );
    }

    #[test]
    fn different_requirements_are_an_options_conflict_naming_every_requirer() {
        let units = [
            Spec::new("@metamao/api", "1.0.0")
                .depends("@metamao/node", "^1")
                .option("@metamao/node", "packageManager", text("pnpm")),
            Spec::new("@metamao/web", "1.0.0")
                .depends("@metamao/node", "^1")
                .option("@metamao/node", "packageManager", text("bun")),
            node(),
        ];
        let errors = run(
            &project(&[
                ("@metamao/api", "^1"),
                ("@metamao/web", "^1"),
                ("@metamao/node", "^1"),
            ]),
            &units,
            &[("@metamao/node", "packageManager", text("npm"))],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::OptionsConflict {
                unit: identity("@metamao/node"),
                version: version("1.0.0"),
                parameter: name("packageManager"),
                requirements: vec![
                    requirement(Requirer::Root, text("npm")),
                    requirement(unit("@metamao/api", "1.0.0"), text("pnpm")),
                    requirement(unit("@metamao/web", "1.0.0"), text("bun")),
                ],
            }]
        );
        let message = errors[0].to_string();
        for expected in [
            "options conflict",
            "`packageManager` of @metamao/node 1.0.0",
            "\"npm\" from the root",
            "\"pnpm\" from @metamao/api 1.0.0",
            "\"bun\" from @metamao/web 1.0.0",
            "no requirement takes precedence",
        ] {
            assert!(message.contains(expected), "{message}");
        }
    }

    #[test]
    fn a_default_never_conflicts_with_a_requirement() {
        let units = [
            Spec::new("@metamao/api", "1.0.0")
                .depends("@metamao/node", "^1")
                .option("@metamao/node", "packageManager", text("pnpm")),
            Spec::new("@metamao/web", "1.0.0").depends("@metamao/node", "^1"),
            node(),
        ];
        let composed = run(
            &project(&[("@metamao/api", "^1"), ("@metamao/web", "^1")]),
            &units,
            &[],
        )
        .unwrap();
        assert_eq!(
            values_of(&composed, "@metamao/node")[0],
            ("packageManager".to_owned(), text("pnpm"))
        );
    }

    #[test]
    fn a_requirement_must_name_a_parameter_the_resolved_version_declares() {
        let units = [
            Spec::new("@metamao/app", "1.0.0")
                .depends("@metamao/node", "^1")
                .option("@metamao/node", "packageManager", text("pnpm")),
            node(),
            // The resolved version no longer declares `packageManager`.
            Spec::new("@metamao/node", "1.1.0")
                .param(
                    "version",
                    optional(ParameterType::Integer, ParameterValue::Integer(22)),
                )
                .param("runtime", optional(ParameterType::String, text("node"))),
        ];
        let errors = run(&project(&[("@metamao/app", "^1")]), &units, &[]).unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::UndeclaredParameter {
                unit: identity("@metamao/node"),
                version: version("1.1.0"),
                parameter: name("packageManager"),
                requirer: unit("@metamao/app", "1.0.0"),
                declared: vec![name("runtime"), name("version")],
            }]
        );
        let message = errors[0].to_string();
        for expected in [
            "@metamao/app 1.0.0 sets parameter `packageManager` of @metamao/node 1.1.0",
            "does not declare it",
            "`runtime`, `version`",
        ] {
            assert!(message.contains(expected), "{message}");
        }
    }

    #[test]
    fn an_undeclared_parameter_of_a_unit_without_parameters_says_so() {
        let errors = run(
            &project(&[("@metamao/plain", "^1")]),
            &[Spec::new("@metamao/plain", "1.0.0")],
            &[("@metamao/plain", "anything", ParameterValue::Boolean(true))],
        )
        .unwrap_err();
        let message = errors[0].to_string();
        assert!(
            message.contains("the root sets parameter `anything`"),
            "{message}"
        );
        assert!(message.contains("it declares no parameters"), "{message}");
    }

    #[test]
    fn values_must_match_the_parameter_type_and_allowed_values() {
        let units = [
            Spec::new("@metamao/app", "1.0.0")
                .depends("@metamao/node", "^1")
                .option("@metamao/node", "packageManager", text("yarn")),
            node(),
        ];
        let errors = run(
            &project(&[("@metamao/app", "^1"), ("@metamao/node", "^1")]),
            &units,
            &[("@metamao/node", "version", text("22"))],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [
                CompositionError::InvalidValue {
                    unit: identity("@metamao/node"),
                    version: version("1.0.0"),
                    parameter: name("packageManager"),
                    requirement: requirement(unit("@metamao/app", "1.0.0"), text("yarn")),
                    error: ParameterValueError::NotAllowed {
                        value: "yarn".to_owned(),
                        allowed: AllowedValues::new(vec![
                            "npm".to_owned(),
                            "pnpm".to_owned(),
                            "bun".to_owned(),
                        ])
                        .unwrap(),
                    },
                },
                CompositionError::InvalidValue {
                    unit: identity("@metamao/node"),
                    version: version("1.0.0"),
                    parameter: name("version"),
                    requirement: requirement(Requirer::Root, text("22")),
                    error: ParameterValueError::TypeMismatch {
                        expected: ValueKind::Integer,
                        found: ValueKind::String,
                    },
                },
            ]
        );
        let message = errors[1].to_string();
        assert!(
            message.contains("the root sets parameter `version` of @metamao/node 1.0.0 to \"22\""),
            "{message}"
        );
        assert!(message.contains("expected an integer"), "{message}");
    }

    #[test]
    fn an_invalid_requirement_is_reported_once_not_also_as_missing_or_conflicting() {
        let units = [
            Spec::new("@metamao/api", "1.0.0")
                .depends("@metamao/db", "^1")
                .option("@metamao/db", "port", text("5432")),
            Spec::new("@metamao/web", "1.0.0")
                .depends("@metamao/db", "^1")
                .option("@metamao/db", "port", ParameterValue::Integer(6543)),
            Spec::new("@metamao/db", "1.0.0").param("port", required(ParameterType::Integer)),
        ];
        let errors = run(
            &project(&[("@metamao/api", "^1"), ("@metamao/web", "^1")]),
            &units,
            &[],
        )
        .unwrap_err();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(matches!(errors[0], CompositionError::InvalidValue { .. }));
    }

    #[test]
    fn a_missing_value_for_an_explicit_selection_is_the_root_s_to_supply() {
        let units = [Spec::new("@metamao/worker", "1.0.0")
            .param("accountId", required(ParameterType::String))];
        let errors = run(&project(&[("@metamao/worker", "^1")]), &units, &[]).unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::MissingValue {
                unit: identity("@metamao/worker"),
                version: version("1.0.0"),
                parameter: name("accountId"),
                responsible: vec![Requirer::Root],
            }]
        );
        let message = errors[0].to_string();
        for expected in [
            "required parameter `accountId` of @metamao/worker 1.0.0",
            "no default",
            "supplied by the root",
        ] {
            assert!(message.contains(expected), "{message}");
        }
    }

    #[test]
    fn a_missing_transitive_value_is_attributed_to_the_units_that_depend_on_it() {
        let units = [
            Spec::new("@metamao/app", "1.0.0").depends("@metamao/api", "^1"),
            Spec::new("@metamao/api", "1.0.0").depends("@metamao/db", "^1"),
            Spec::new("@metamao/admin", "1.0.0").depends("@metamao/db", "^1"),
            Spec::new("@metamao/db", "1.0.0").param("url", required(ParameterType::String)),
        ];
        let errors = run(
            &project(&[("@metamao/app", "^1"), ("@metamao/admin", "^1")]),
            &units,
            &[],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::MissingValue {
                unit: identity("@metamao/db"),
                version: version("1.0.0"),
                parameter: name("url"),
                responsible: vec![
                    unit("@metamao/admin", "1.0.0"),
                    unit("@metamao/api", "1.0.0"),
                ],
            }]
        );
        let message = errors[0].to_string();
        assert!(!message.contains("the root"), "{message}");
        assert!(
            message.contains("supplied by @metamao/admin 1.0.0, @metamao/api 1.0.0"),
            "{message}"
        );
    }

    #[test]
    fn a_dependent_satisfies_a_required_parameter_through_its_options() {
        let units = [
            Spec::new("@metamao/api", "1.0.0")
                .depends("@metamao/db", "^1")
                .option("@metamao/db", "url", text("postgres://db")),
            Spec::new("@metamao/db", "1.0.0").param("url", required(ParameterType::String)),
        ];
        let composed = run(&project(&[("@metamao/api", "^1")]), &units, &[]).unwrap();
        assert_eq!(
            values_of(&composed, "@metamao/db"),
            values(&[("url", text("postgres://db"))])
        );
    }

    #[test]
    fn consumer_values_on_a_transitive_unit_are_rejected() {
        let units = [
            Spec::new("@metamao/app", "1.0.0").depends("@metamao/node", "^1"),
            node(),
        ];
        let errors = run(
            &project(&[("@metamao/app", "^1")]),
            &units,
            &[("@metamao/node", "packageManager", text("bun"))],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::RootTarget {
                unit: identity("@metamao/node"),
                reason: RootTargetReason::Transitive,
            }]
        );
        let message = errors[0].to_string();
        assert!(
            message.contains("only a dependency of other units"),
            "{message}"
        );
        assert!(message.contains("explicitly selected units"), "{message}");
    }

    #[test]
    fn consumer_values_on_a_unit_outside_the_composition_are_rejected() {
        let errors = run(
            &project(&[("@metamao/node", "^1")]),
            &[node()],
            &[("@metamao/absent", "anything", ParameterValue::Boolean(true))],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::RootTarget {
                unit: identity("@metamao/absent"),
                reason: RootTargetReason::NotComposed,
            }]
        );
        assert!(
            errors[0]
                .to_string()
                .contains("not part of the composition")
        );
    }

    // Unit builds.

    #[test]
    fn a_unit_build_composes_the_built_unit_with_its_dependencies() {
        let built = Spec::new("@metamao/bun-app", "0.1.0")
            .depends("@metamao/node", "^1")
            .option("@metamao/node", "packageManager", text("bun"))
            .param("title", required(ParameterType::String))
            .file("index.ts");
        let composed = run(
            &For::Build(built),
            &[node().file("package.json")],
            &[("@metamao/bun-app", "title", text("Demo"))],
        )
        .unwrap();
        assert_eq!(
            values_of(&composed, "@metamao/bun-app"),
            values(&[("title", text("Demo"))])
        );
        assert_eq!(
            values_of(&composed, "@metamao/node")[0],
            ("packageManager".to_owned(), text("bun"))
        );
        let files: Vec<(&str, &str)> = composed
            .files
            .iter()
            .map(|(file, contributor, _, _)| (file.as_str(), contributor.as_str()))
            .collect();
        assert_eq!(
            files,
            [
                ("index.ts", "@metamao/bun-app"),
                ("package.json", "@metamao/node")
            ]
        );
    }

    #[test]
    fn a_unit_build_s_values_configure_only_the_built_unit() {
        let built = Spec::new("@metamao/bun-app", "0.1.0").depends("@metamao/node", "^1");
        let errors = run(
            &For::Build(built),
            &[node()],
            &[("@metamao/node", "packageManager", text("bun"))],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::RootTarget {
                unit: identity("@metamao/node"),
                reason: RootTargetReason::NotTheBuiltUnit,
            }]
        );
        assert!(errors[0].to_string().contains("through its `options`"));
    }

    #[test]
    fn a_built_unit_s_missing_value_is_the_root_s_to_supply() {
        let built = Spec::new("@metamao/bun-app", "0.1.0")
            .depends("@metamao/db", "^1")
            .param("title", required(ParameterType::String));
        let db = Spec::new("@metamao/db", "1.0.0").param("url", required(ParameterType::String));
        let errors = run(&For::Build(built), &[db], &[]).unwrap_err();
        assert_eq!(
            errors,
            [
                CompositionError::MissingValue {
                    unit: identity("@metamao/bun-app"),
                    version: version("0.1.0"),
                    parameter: name("title"),
                    responsible: vec![Requirer::Root],
                },
                CompositionError::MissingValue {
                    unit: identity("@metamao/db"),
                    version: version("1.0.0"),
                    parameter: name("url"),
                    responsible: vec![Requirer::Root],
                },
            ]
        );
    }

    // Files.

    #[test]
    fn files_are_the_union_of_every_unit_s_files_with_their_contributor() {
        let units = [
            Spec::new("@metamao/app", "1.0.0")
                .depends("@metamao/node", "^1")
                .file("src/app.ts")
                .file("README.md"),
            node().file("package.json"),
        ];
        let composed = run(&project(&[("@metamao/app", "^1")]), &units, &[]).unwrap();
        let files: Vec<(&str, &str, &[u8])> = composed
            .files
            .iter()
            .map(|(file, contributor, contents, _)| {
                (file.as_str(), contributor.as_str(), contents.as_slice())
            })
            .collect();
        assert_eq!(
            files,
            [
                (
                    "README.md",
                    "@metamao/app",
                    b"@metamao/app README.md".as_slice()
                ),
                (
                    "package.json",
                    "@metamao/node",
                    b"@metamao/node package.json"
                ),
                ("src/app.ts", "@metamao/app", b"@metamao/app src/app.ts"),
            ]
        );
    }

    #[test]
    fn file_bytes_and_the_executable_bit_are_preserved_without_interpolation() {
        let binary = vec![0, 159, 146, 150, 255, b'\r', b'\n'];
        let template = b"name: {{ title }}\nrun: ${{ matrix.os }}\n".to_vec();
        let units = [Spec::new("@metamao/tools", "1.0.0")
            .param("title", optional(ParameterType::String, text("Demo")))
            .file_with("bin/run", binary.clone(), true)
            .file_with("ci.yml", template.clone(), false)];
        let composed = run(&project(&[("@metamao/tools", "^1")]), &units, &[]).unwrap();
        assert_eq!(
            composed.files,
            [
                (
                    "bin/run".to_owned(),
                    "@metamao/tools".to_owned(),
                    binary,
                    true
                ),
                (
                    "ci.yml".to_owned(),
                    "@metamao/tools".to_owned(),
                    template,
                    false
                ),
            ]
        );
    }

    #[test]
    fn independent_units_providing_the_same_path_collide() {
        let units = [
            Spec::new("@metamao/api", "1.0.0").file("README.md"),
            Spec::new("@metamao/web", "1.0.0").file("README.md"),
        ];
        let errors = run(
            &project(&[("@metamao/web", "^1"), ("@metamao/api", "^1")]),
            &units,
            &[],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::PathCollision {
                files: vec![
                    contributed("README.md", "@metamao/api"),
                    contributed("README.md", "@metamao/web"),
                ],
            }]
        );
        let message = errors[0].to_string();
        for expected in [
            "path collision",
            "README.md from @metamao/api",
            "README.md from @metamao/web",
            "amendment",
        ] {
            assert!(message.contains(expected), "{message}");
        }
    }

    #[test]
    fn a_unit_and_its_dependency_providing_the_same_path_collide() {
        let built = Spec::new("@metamao/bun-app", "0.1.0")
            .depends("@metamao/node", "^1")
            .file("package.json");
        let errors = run(&For::Build(built), &[node().file("package.json")], &[]).unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::PathCollision {
                files: vec![
                    contributed("package.json", "@metamao/bun-app"),
                    contributed("package.json", "@metamao/node"),
                ],
            }]
        );
    }

    #[test]
    fn paths_that_differ_only_in_case_collide() {
        let units = [
            Spec::new("@metamao/api", "1.0.0").file("README.md"),
            Spec::new("@metamao/web", "1.0.0").file("readme.md"),
        ];
        let errors = run(
            &project(&[("@metamao/api", "^1"), ("@metamao/web", "^1")]),
            &units,
            &[],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::PathCollision {
                files: vec![
                    contributed("README.md", "@metamao/api"),
                    contributed("readme.md", "@metamao/web"),
                ],
            }]
        );
        assert!(errors[0].to_string().contains("differ only in case"));
    }

    #[test]
    fn one_unit_s_paths_that_differ_only_in_case_collide() {
        let units = [Spec::new("@metamao/docs", "1.0.0")
            .file("Guide/Intro.md")
            .file("guide/intro.md")];
        let errors = run(&project(&[("@metamao/docs", "^1")]), &units, &[]).unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::PathCollision {
                files: vec![
                    contributed("Guide/Intro.md", "@metamao/docs"),
                    contributed("guide/intro.md", "@metamao/docs"),
                ],
            }]
        );
    }

    #[test]
    fn every_contributor_to_a_colliding_path_is_named_once() {
        let units = [
            Spec::new("@metamao/a", "1.0.0").file("LICENSE"),
            Spec::new("@metamao/b", "1.0.0").file("LICENSE"),
            Spec::new("@metamao/c", "1.0.0").file("license"),
        ];
        let errors = run(
            &project(&[
                ("@metamao/c", "^1"),
                ("@metamao/b", "^1"),
                ("@metamao/a", "^1"),
            ]),
            &units,
            &[],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::PathCollision {
                files: vec![
                    contributed("LICENSE", "@metamao/a"),
                    contributed("LICENSE", "@metamao/b"),
                    contributed("license", "@metamao/c"),
                ],
            }]
        );
    }

    #[test]
    fn a_file_where_another_unit_needs_a_directory_collides() {
        let units = [
            Spec::new("@metamao/api", "1.0.0").file("config"),
            Spec::new("@metamao/web", "1.0.0")
                .file("Config/app.json")
                .file("config/nested/db.json"),
        ];
        let errors = run(
            &project(&[("@metamao/api", "^1"), ("@metamao/web", "^1")]),
            &units,
            &[],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [CompositionError::NestedPathCollision {
                file: contributed("config", "@metamao/api"),
                nested: vec![
                    contributed("Config/app.json", "@metamao/web"),
                    contributed("config/nested/db.json", "@metamao/web"),
                ],
            }]
        );
        let message = errors[0].to_string();
        assert!(
            message.contains("config from @metamao/api is a file"),
            "{message}"
        );
        assert!(message.contains("need it to be a directory"), "{message}");
    }

    #[test]
    fn every_collision_is_reported() {
        let units = [
            Spec::new("@metamao/api", "1.0.0")
                .file(".gitignore")
                .file("Dockerfile")
                .file("docs"),
            Spec::new("@metamao/web", "1.0.0")
                .file(".gitignore")
                .file("dockerfile")
                .file("docs/index.md"),
        ];
        let errors = run(
            &project(&[("@metamao/api", "^1"), ("@metamao/web", "^1")]),
            &units,
            &[],
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [
                CompositionError::PathCollision {
                    files: vec![
                        contributed(".gitignore", "@metamao/api"),
                        contributed(".gitignore", "@metamao/web"),
                    ],
                },
                CompositionError::PathCollision {
                    files: vec![
                        contributed("Dockerfile", "@metamao/api"),
                        contributed("dockerfile", "@metamao/web"),
                    ],
                },
                CompositionError::NestedPathCollision {
                    file: contributed("docs", "@metamao/api"),
                    nested: vec![contributed("docs/index.md", "@metamao/web")],
                },
            ]
        );
    }

    // Inputs and ordering.

    #[test]
    fn content_that_does_not_match_the_graph_is_reported() {
        let app = Spec::new("@metamao/app", "1.0.0").depends("@metamao/node", "^1");
        let (app_manifest, app_files) = (app.manifest(), app.tree());
        let (node_manifest, node_files) = (node().manifest(), node().tree());
        let stale = Spec::new("@metamao/node", "0.9.0");
        let (stale_manifest, stale_files) = (stale.manifest(), stale.tree());
        let mut available = AvailableVersions::new();
        for manifest in [&app_manifest, &node_manifest] {
            available
                .entry(manifest.name().clone())
                .or_default()
                .insert(manifest.version().clone(), manifest.dependencies().clone());
        }
        let resolution = resolve(
            None,
            &dependencies(&[("@metamao/app", "^1")]),
            &available,
            &Lock::new(),
        )
        .unwrap();
        let mismatch = |unit: &str, resolved: &str| CompositionError::ContentMismatch {
            unit: identity(unit),
            version: version(resolved),
        };

        let missing = BTreeMap::new();
        assert_eq!(
            compose(
                resolution.graph(),
                Root::Project,
                &missing,
                &BTreeMap::new()
            )
            .unwrap_err(),
            [
                mismatch("@metamao/app", "1.0.0"),
                mismatch("@metamao/node", "1.0.0"),
            ]
        );

        let wrong_version: BTreeMap<_, _> = [
            (
                identity("@metamao/app"),
                UnitContent::new(&app_manifest, &app_files),
            ),
            (
                identity("@metamao/node"),
                UnitContent::new(&stale_manifest, &stale_files),
            ),
        ]
        .into();
        let errors = compose(
            resolution.graph(),
            Root::Project,
            &wrong_version,
            &BTreeMap::new(),
        )
        .unwrap_err();
        assert_eq!(errors, [mismatch("@metamao/node", "1.0.0")]);
        assert!(errors[0].to_string().contains("does not match"));

        // A built unit whose dependencies differ from the graph's root.
        let built = Root::Unit(UnitContent::new(&node_manifest, &node_files));
        let complete: BTreeMap<_, _> = [
            (
                identity("@metamao/app"),
                UnitContent::new(&app_manifest, &app_files),
            ),
            (
                identity("@metamao/node"),
                UnitContent::new(&node_manifest, &node_files),
            ),
        ]
        .into();
        assert_eq!(
            compose(resolution.graph(), built, &complete, &BTreeMap::new()).unwrap_err(),
            [mismatch("@metamao/node", "1.0.0")]
        );
    }

    /// Units whose composition fails in every way at once.
    fn every_problem() -> (For, Vec<Spec>, Vec<RootValue>) {
        let units = vec![
            Spec::new("@metamao/api", "1.0.0")
                .depends("@metamao/node", "^1")
                .depends("@metamao/db", "^1")
                .option("@metamao/node", "packageManager", text("pnpm"))
                .option("@metamao/node", "colour", text("blue"))
                .option("@metamao/db", "port", text("5432"))
                .file("README.md")
                .file("docs"),
            Spec::new("@metamao/web", "1.0.0")
                .depends("@metamao/node", "^1")
                .depends("@metamao/db", "^1")
                .option("@metamao/node", "packageManager", text("bun"))
                .file("readme.md")
                .file("docs/index.md"),
            node().file("README.md"),
            Spec::new("@metamao/db", "1.0.0")
                .param("port", required(ParameterType::Integer))
                .param("url", required(ParameterType::String)),
        ];
        let root = project(&[("@metamao/api", "^1"), ("@metamao/web", "^1")]);
        let root_values = vec![
            ("@metamao/node", "version", ParameterValue::Integer(24)),
            ("@metamao/web", "theme", text("dark")),
        ];
        (root, units, root_values)
    }

    #[test]
    fn every_problem_is_reported_in_a_stable_order() {
        let (root, units, root_values) = every_problem();
        let errors = run(&root, &units, &root_values).unwrap_err();
        let kinds: Vec<String> = errors
            .iter()
            .map(|error| match error {
                CompositionError::ContentMismatch { unit, .. } => format!("mismatch {unit}"),
                CompositionError::RootTarget { unit, .. } => format!("target {unit}"),
                CompositionError::UndeclaredParameter {
                    unit,
                    parameter,
                    requirer,
                    ..
                } => format!("undeclared {unit} {parameter} {requirer}"),
                CompositionError::InvalidValue {
                    unit, parameter, ..
                } => format!("invalid {unit} {parameter}"),
                CompositionError::OptionsConflict {
                    unit, parameter, ..
                } => format!("conflict {unit} {parameter}"),
                CompositionError::MissingValue {
                    unit, parameter, ..
                } => format!("missing {unit} {parameter}"),
                CompositionError::PathCollision { files } => format!("collision {}", files[0]),
                CompositionError::NestedPathCollision { file, .. } => format!("nested {file}"),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "target @metamao/node",
                "invalid @metamao/db port",
                "undeclared @metamao/node colour @metamao/api 1.0.0",
                "undeclared @metamao/web theme the root",
                "conflict @metamao/node packageManager",
                "missing @metamao/db url",
                "collision README.md from @metamao/api",
                "nested docs from @metamao/api",
            ]
        );
    }

    #[test]
    fn declaration_and_enumeration_order_never_change_the_outcome() {
        let (root, units, root_values) = every_problem();
        let succeeding = (
            project(&[("@metamao/bun-app", "^1"), ("@metamao/node", "^1")]),
            vec![
                Spec::new("@metamao/bun-app", "1.0.0")
                    .depends("@metamao/node", "^1")
                    .option("@metamao/node", "packageManager", text("bun"))
                    .file("src/index.ts")
                    .file("bunfig.toml"),
                node().file("package.json").file(".nvmrc"),
            ],
            vec![("@metamao/node", "version", ParameterValue::Integer(24))],
        );
        for (root, units, root_values) in [(root, units, root_values), succeeding] {
            let expected = run(&root, &units, &root_values);
            let reorder = |spec: &Spec| {
                let mut spec = spec.clone();
                spec.dependencies.reverse();
                spec.options.reverse();
                spec.params.reverse();
                spec.files.reverse();
                spec
            };
            let reordered_root = match &root {
                For::Project(selections) => {
                    let mut selections = selections.clone();
                    selections.reverse();
                    For::Project(selections)
                }
                For::Build(spec) => For::Build(reorder(spec)),
            };
            for rotation in 0..units.len() {
                let mut shuffled: Vec<Spec> = units.iter().map(reorder).collect();
                shuffled.rotate_left(rotation);
                let mut values = root_values.clone();
                let count = values.len().max(1);
                values.rotate_left(rotation % count);
                assert_eq!(run(&root, &shuffled, &values), expected);
                assert_eq!(run(&reordered_root, &shuffled, &values), expected);
            }
        }
    }
}
