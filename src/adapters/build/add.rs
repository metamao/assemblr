//! `unit add`: declares a dependency in the unit's `manifest.jsonc` and
//! builds the unit as `unit build` does (see
//! `docs/domain/authoring-workflow.md`).
//!
//! The declaration is planned on the manifest's text, so comments and
//! formatting survive, and the edited manifest is validated as a build
//! validates the working tree's before anything is written. A build that
//! would be rejected leaves the manifest as it was. Required parameters of
//! the added unit that have no default are mapped from parameters of this
//! unit's own, declared for them when the unit has none, unless `--option`
//! gives a literal.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::amend::{MANIFEST_NEW, write_manifest};
use super::{
    BuildFailure, BuildProblem, Built, MANIFEST, ManifestFileError, Opened, open,
    publish_validated, resolve_graph, validate_opened,
};
use crate::adapters::jsonc_edit::{apply, parse_value};
use crate::adapters::manifest::read_manifest;
use crate::adapters::project::declaration::{
    DeclarationChange, EditFailure, add_after, option_value, selected_requirement, set_options,
};
use crate::adapters::registry::Registry;
use crate::domain::{
    Edit, Manifest, OptionValue, ParameterDefinition, ParameterName, ParameterValue, UnitIdentity,
    Value, Version, VersionRequirement, mapping_problem,
};

const OPTIONS: &str = "options";
const PARAMS: &str = "params";

/// Declares `identity` as a dependency of the unit whose repository's top
/// level is `root`, then builds the unit as `build_unit` does. Without a
/// `requirement` the unit is recorded with a caret requirement on the
/// version the resolution picks, which is its locked version when it is a
/// transitive dependency already; a unit declared already keeps its own.
/// Each of `options`, a parameter name and the text of its value, is set
/// as a literal in the unit's `options` entry, read by the parameter's type
/// as the consumer's `add` reads it. Every other required parameter of the
/// added unit that has no default is mapped from a parameter of this unit
/// with the same name, declared for it when the unit has none.
pub fn add_dependency(
    root: &Path,
    registry: &Registry,
    identity: &UnitIdentity,
    requirement: Option<&str>,
    options: &[(&str, &str)],
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
    let planned = plan(&opened, identity, requirement, options)?;
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
        parameters: planned.parameters,
    })
}

/// The edited manifest, decided and not written.
struct Planned {
    text: String,
    manifest: Manifest,
    change: DeclarationChange,
    parameters: Vec<AddedParameter>,
}

fn plan(
    opened: &Opened,
    identity: &UnitIdentity,
    requirement: Option<&str>,
    options: &[(&str, &str)],
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
    let mut probed = None;
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
                let requirement = format!("^{}", locked.version());
                // A caret on the version picked resolves to it again under
                // the same lock.
                probed = Some(resolved);
                requirement
            }
        },
    };
    let (text, change) = add_after(text, identity, &requirement, &preferred).map_err(cannot)?;
    let edited = read_manifest(&text).map_err(|error| {
        rejected(AddProblem::CannotEdit(format!(
            "the edited manifest is not valid: {error}"
        )))
    })?;

    // The added unit's parameters, at the version the edited manifest
    // resolves it to.
    let resolved = match probed {
        Some(resolved) => resolved,
        None => {
            let mut found = Vec::new();
            let Some(resolved) = resolve_graph(
                Some(edited.name()),
                edited.dependencies(),
                &previous,
                &opened.index,
                &mut found,
            ) else {
                return Err(AddFailure::rejected(BuildFailure::Rejected(found)));
            };
            resolved
        }
    };
    let Some(locked) = resolved.lock.units().get(identity) else {
        return Err(rejected(AddProblem::CannotEdit(format!(
            "the resolution did not select {identity}"
        ))));
    };
    let version = locked.version().clone();
    let params = resolved
        .manifest_of(identity)
        .map(|dependency| resolved.params(dependency))
        .unwrap_or_default();
    let existing = resolved.params(manifest);
    let own = edited.options().get(identity).cloned().unwrap_or_default();

    // What the dependency's `options` entry gains, literals first: each as
    // the manifest holds it, and as the text is edited to hold it.
    let mut set: Vec<(ParameterName, OptionValue)> = Vec::new();
    let mut parameters = Vec::new();
    let mut inherited: Vec<ParameterName> = Vec::new();
    for (name, raw) in options {
        let parameter = ParameterName::parse(name)
            .map_err(|error| rejected(AddProblem::InvalidOption(error.to_string())))?;
        let value = option_value(
            raw,
            params
                .get(&parameter)
                .map(ParameterDefinition::parameter_type),
        );
        let literal = match value {
            Value::String(text) => ParameterValue::String(text),
            Value::Bool(flag) => ParameterValue::Boolean(flag),
            Value::Number(number) => match number.parse() {
                Ok(number) => ParameterValue::Integer(number),
                Err(_) => {
                    return Err(rejected(AddProblem::InvalidOption(format!(
                        "the value of `{parameter}` is not a string, an integer, or a boolean"
                    ))));
                }
            },
            _ => {
                return Err(rejected(AddProblem::InvalidOption(format!(
                    "the value of `{parameter}` is not a string, an integer, or a boolean"
                ))));
            }
        };
        set.push((parameter.clone(), OptionValue::Literal(literal)));
        parameters.push(AddedParameter::Literal(parameter));
    }
    for (name, definition) in &params {
        if !definition.is_required()
            || own.contains_key(name)
            || manifest.inherited().get(name) == Some(identity)
            || options.iter().any(|(given, _)| *given == name.as_str())
        {
            continue;
        }
        match existing.get(name) {
            Some(existing) => {
                if let Some(reason) = mapping_problem(existing, definition) {
                    return Err(rejected(AddProblem::ParameterClash {
                        dependency: identity.clone(),
                        version: version.clone(),
                        parameter: name.clone(),
                        reason,
                    }));
                }
                parameters.push(AddedParameter::MappedToExisting(name.clone()));
                set.push((name.clone(), OptionValue::From(name.clone())));
            }
            None => {
                // The unit's build uses the example as the development
                // value; without one, the inherited parameter cannot be
                // built.
                if definition.example().is_none() {
                    return Err(rejected(AddProblem::NoExample {
                        dependency: identity.clone(),
                        version: version.clone(),
                        parameter: name.clone(),
                    }));
                }
                // An inherited parameter passes its value on by itself, so
                // the dependency's `options` gains nothing for it.
                inherited.push(name.clone());
                parameters.push(AddedParameter::Inherited(name.clone()));
            }
        }
    }
    let values: Vec<(String, Value)> = set
        .iter()
        .map(|(name, value)| (name.as_str().to_owned(), option_text(value)))
        .collect();
    let text = declare_params(&text, identity, &inherited).map_err(cannot)?;
    let text = set_options(&text, identity, &values).map_err(cannot)?;

    // What the text must describe: the manifest plus the declarations,
    // which a reading the editor misjudged would not match.
    let mut inherits = manifest.inherited().clone();
    inherits.extend(inherited.into_iter().map(|name| (name, identity.clone())));
    let mut unit_options = manifest.options().clone();
    if !set.is_empty() {
        unit_options
            .entry(identity.clone())
            .or_default()
            .extend(set);
    }
    let mut dependencies = manifest.dependencies().clone();
    let parsed = VersionRequirement::parse(&requirement)
        .map_err(|error| rejected(AddProblem::InvalidRequirement(error.to_string())))?;
    dependencies.insert(identity.clone(), parsed);
    let expected = Manifest::new(
        manifest.name().clone(),
        manifest.version().clone(),
        manifest.description().clone(),
        dependencies,
        unit_options,
        manifest.params().clone(),
        inherits,
        manifest.amendments().clone(),
    )
    .map_err(|errors| {
        rejected(AddProblem::CannotEdit(
            errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; "),
        ))
    })?;
    let manifest = read_manifest(&text).map_err(|error| {
        rejected(AddProblem::CannotEdit(format!(
            "the edited manifest is not valid: {error}"
        )))
    })?;
    if manifest != expected {
        return Err(rejected(AddProblem::CannotEdit(
            "the edited manifest does not hold the intended declarations".to_owned(),
        )));
    }
    Ok(Planned {
        text,
        manifest,
        change,
        parameters,
    })
}

/// `text` with each of `inherited` added to `params`, after the last one,
/// as inherited from `dependency`. A manifest without `params` gets the
/// object after `options`, or after `dependencies`.
fn declare_params(
    text: &str,
    dependency: &UnitIdentity,
    inherited: &[ParameterName],
) -> Result<String, EditFailure> {
    let mut text = text.to_owned();
    for name in inherited {
        let root = parse_value(&text).map_err(|error| EditFailure::Cannot(error.to_string()))?;
        let Value::Object(members) = &root else {
            return Err(EditFailure::Cannot("it is not a JSON object".to_owned()));
        };
        let value = Value::Object(vec![(
            "from".to_owned(),
            Value::String(dependency.to_string()),
        )]);
        let edit = match root.member(PARAMS) {
            Some(Value::Object(params)) => Edit::Insert {
                parent: vec![PARAMS.to_owned()],
                key: name.as_str().to_owned(),
                value,
                after: params.last().map(|(name, _)| name.clone()),
            },
            Some(_) => return Err(EditFailure::Cannot("`params` is not an object".to_owned())),
            None => Edit::Insert {
                parent: Vec::new(),
                key: PARAMS.to_owned(),
                value: Value::Object(vec![(name.as_str().to_owned(), value)]),
                after: anchor(members, &[OPTIONS, "dependencies", "description"]),
            },
        };
        text = apply(&text, &[edit]).map_err(|error| EditFailure::Cannot(error.to_string()))?;
    }
    Ok(text)
}

/// The first of `preferred` that `members` holds, or the last member.
fn anchor(members: &[(String, Value)], preferred: &[&str]) -> Option<String> {
    preferred
        .iter()
        .find(|key| members.iter().any(|(name, _)| name == *key))
        .map(|key| (*key).to_owned())
        .or_else(|| members.last().map(|(name, _)| name.clone()))
}

/// An option value as the manifest writes it: the literal, or the mapping
/// object `{ "from": "<name>" }`.
fn option_text(value: &OptionValue) -> Value {
    match value {
        OptionValue::Literal(literal) => parameter_value(literal),
        OptionValue::From(source) => Value::Object(vec![(
            "from".to_owned(),
            Value::String(source.as_str().to_owned()),
        )]),
    }
}

fn parameter_value(value: &ParameterValue) -> Value {
    match value {
        ParameterValue::String(text) => Value::String(text.clone()),
        ParameterValue::Integer(number) => Value::Number(number.to_string()),
        ParameterValue::Boolean(flag) => Value::Bool(*flag),
    }
}

/// What `unit add` did.
#[derive(Debug)]
pub struct DependencyAdded {
    built: Built,
    change: DeclarationChange,
    parameters: Vec<AddedParameter>,
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

    /// What was set for the dependency's parameters, in the order the
    /// options were given and then by parameter name.
    pub fn parameters(&self) -> &[AddedParameter] {
        &self.parameters
    }
}

/// How `unit add` satisfied one parameter of the added unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddedParameter {
    /// `--option` set a literal.
    Literal(ParameterName),
    /// The parameter is mapped from a parameter the unit declares already.
    MappedToExisting(ParameterName),
    /// The unit declares the parameter as inherited from the dependency,
    /// `{ "from": "<dependency>" }`, which takes the dependency's definition
    /// and passes its value on.
    Inherited(ParameterName),
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
            BuildFailure::Rejected(problems) => {
                Self::Rejected(problems.into_iter().map(AddProblem::Build).collect())
            }
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
    Build(BuildProblem),
    /// The unit was named as its own dependency.
    Itself(UnitIdentity),
    /// The requirement is not a SemVer requirement.
    InvalidRequirement(String),
    /// An `--option` names an invalid parameter or holds a value the
    /// manifest cannot write.
    InvalidOption(String),
    /// The manifest's text could not be edited safely.
    CannotEdit(String),
    /// The dependency requires a parameter the unit declares already with
    /// a type that cannot be mapped to it.
    ParameterClash {
        /// The dependency.
        dependency: UnitIdentity,
        /// Its resolved version.
        version: Version,
        /// The parameter, which both declare.
        parameter: ParameterName,
        /// Why the unit's parameter cannot be mapped.
        reason: String,
    },
    /// The dependency requires a parameter without an example, so a
    /// parameter mapped to it could not be built.
    NoExample {
        /// The dependency.
        dependency: UnitIdentity,
        /// Its resolved version.
        version: Version,
        /// The parameter.
        parameter: ParameterName,
    },
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
            Self::InvalidOption(detail) => write!(f, "invalid option: {detail}"),
            Self::CannotEdit(detail) => write!(
                f,
                "`manifest.jsonc` could not be edited safely, so it is unchanged: {detail}"
            ),
            Self::ParameterClash {
                dependency,
                version,
                parameter,
                reason,
            } => write!(
                f,
                "{dependency} {version} needs a value for `{parameter}`, and the unit's own \
                 parameter `{parameter}` cannot supply it: {reason}"
            ),
            Self::NoExample {
                dependency,
                version,
                parameter,
            } => write!(
                f,
                "{dependency} {version} needs a value for `{parameter}` and declares no example \
                 for it, so a parameter of this unit mapped to it could not be built"
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
        Self::Build(BuildProblem::Manifest(error))
    }
}
