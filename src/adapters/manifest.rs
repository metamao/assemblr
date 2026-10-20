//! Reading `manifest.jsonc` text into a validated [`Manifest`].
//!
//! The reader is pure: it takes the manifest's text, not a path. It reports
//! every problem it finds, each with the path of the field concerned, sorted
//! by path so the result does not depend on the order of keys in the source.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use super::jsonc::{self, Json, SyntaxError};
use crate::domain::{
    self, AllowedValues, AllowedValuesError, BlankDescription, Description, IdentityError,
    Manifest, ManifestError, OptionValues, ParameterDefinition, ParameterName, ParameterNameError,
    ParameterType, ParameterValue, ParameterValueError, RequirementError, UnitIdentity, Version,
    VersionError, VersionRequirement,
};

const MANIFEST_FIELDS: &[&str] = &[
    "$schema",
    "name",
    "version",
    "description",
    "dependencies",
    "options",
    "params",
];
const PARAMETER_FIELDS: &[&str] = &["type", "description", "default", "allowed"];
const SCALAR: &[JsonKind] = &[JsonKind::String, JsonKind::Integer, JsonKind::Boolean];

/// Reads and validates the text of a `manifest.jsonc`.
///
/// Syntax errors stop reading and are reported alone. Otherwise every
/// validation problem is reported, sorted by field path.
pub fn read_manifest(text: &str) -> Result<Manifest, ReadManifestError> {
    let json = jsonc::parse(text).map_err(ReadManifestError::Syntax)?;
    let mut reader = Reader::default();
    let manifest = reader.manifest(&json);
    let mut issues = reader.issues;
    issues.sort_by(|a, b| a.path.cmp(&b.path));
    match manifest {
        Some(manifest) if issues.is_empty() => Ok(manifest),
        _ => Err(ReadManifestError::Invalid(issues)),
    }
}

/// Why a manifest could not be read.
#[derive(Debug, Clone, PartialEq)]
pub enum ReadManifestError {
    /// The text is not valid JSONC.
    Syntax(SyntaxError),
    /// The text is valid JSONC, but not a valid manifest. Holds every
    /// problem found, sorted by field path.
    Invalid(Vec<ManifestIssue>),
}

impl fmt::Display for ReadManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(error) => write!(f, "the manifest is not valid JSONC: {error}"),
            Self::Invalid(issues) => {
                let plural = if issues.len() == 1 { "" } else { "s" };
                write!(f, "the manifest has {} problem{plural}:", issues.len())?;
                for issue in issues {
                    write!(f, "\n  {issue}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ReadManifestError {}

/// One problem in a manifest, located by the path of the field concerned.
#[derive(Debug, Clone, PartialEq)]
pub struct ManifestIssue {
    path: FieldPath,
    kind: IssueKind,
}

impl ManifestIssue {
    /// The field the problem concerns.
    pub fn path(&self) -> &FieldPath {
        &self.path
    }

    /// What is wrong with it.
    pub fn kind(&self) -> &IssueKind {
        &self.kind
    }
}

impl fmt::Display for ManifestIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.kind)
    }
}

/// The location of a value in a manifest, such as `params.appName.default`.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct FieldPath(Vec<PathSegment>);

impl FieldPath {
    /// The segments from the document root, outermost first.
    pub fn segments(&self) -> &[PathSegment] {
        &self.0
    }

    fn key(&self, key: &str) -> Self {
        let mut segments = self.0.clone();
        segments.push(PathSegment::Key(key.to_owned()));
        Self(segments)
    }

    fn index(&self, index: usize) -> Self {
        let mut segments = self.0.clone();
        segments.push(PathSegment::Index(index));
        Self(segments)
    }
}

impl fmt::Display for FieldPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return f.write_str("(manifest)");
        }
        for (position, segment) in self.0.iter().enumerate() {
            match segment {
                PathSegment::Key(key) if is_plain_key(key) => {
                    if position > 0 {
                        f.write_str(".")?;
                    }
                    f.write_str(key)?;
                }
                PathSegment::Key(key) => write!(f, "[{key:?}]")?,
                PathSegment::Index(index) => write!(f, "[{index}]")?,
            }
        }
        Ok(())
    }
}

fn is_plain_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// One step in a [`FieldPath`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PathSegment {
    /// An object member.
    Key(String),
    /// An array element, counted from zero.
    Index(usize),
}

/// The kind of a JSON value, as reported in type errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonKind {
    /// `null`.
    Null,
    /// `true` or `false`.
    Boolean,
    /// An integer in the 64-bit signed range.
    Integer,
    /// A fraction, an exponent, or an integer outside the 64-bit signed range.
    OtherNumber,
    /// A string.
    String,
    /// An array.
    Array,
    /// An object.
    Object,
}

impl JsonKind {
    fn of(json: &Json) -> Self {
        match json {
            Json::Null => Self::Null,
            Json::Bool(_) => Self::Boolean,
            Json::Integer(_) => Self::Integer,
            Json::OtherNumber => Self::OtherNumber,
            Json::String(_) => Self::String,
            Json::Array(_) => Self::Array,
            Json::Object(_) => Self::Object,
        }
    }
}

impl fmt::Display for JsonKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Null => "null",
            Self::Boolean => "a boolean",
            Self::Integer => "an integer",
            Self::OtherNumber => "a number that is not a 64-bit integer",
            Self::String => "a string",
            Self::Array => "an array",
            Self::Object => "an object",
        })
    }
}

/// What is wrong with a manifest field.
#[derive(Debug, Clone, PartialEq)]
pub enum IssueKind {
    /// The field is not part of the manifest format.
    UnknownField,
    /// A required field is absent.
    MissingField,
    /// The key appears more than once in the same object. The first
    /// occurrence is the one validated.
    DuplicateKey,
    /// The value has the wrong JSON type.
    WrongType {
        /// The kinds accepted here.
        expected: &'static [JsonKind],
        /// The kind found.
        found: JsonKind,
    },
    /// The value or key is not a valid unit identity.
    InvalidIdentity(IdentityError),
    /// The value is not an exact SemVer version.
    InvalidVersion(VersionError),
    /// The value is not a valid version requirement.
    InvalidRequirement(RequirementError),
    /// The key is not a valid parameter name.
    InvalidParameterName(ParameterNameError),
    /// The description is empty or only whitespace.
    BlankDescription(BlankDescription),
    /// The parameter `type` is not one of the supported types.
    UnknownParameterType(String),
    /// `allowed` is declared for a parameter that is not a string.
    AllowedValuesRequireString,
    /// The `allowed` list is invalid.
    InvalidAllowedValues(AllowedValuesError),
    /// The default does not satisfy the parameter's type.
    InvalidDefault(ParameterValueError),
    /// The manifest's fields are inconsistent with each other.
    Inconsistent(ManifestError),
}

impl fmt::Display for IssueKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownField => f.write_str("unknown field"),
            Self::MissingField => f.write_str("missing required field"),
            Self::DuplicateKey => f.write_str("duplicate key"),
            Self::WrongType { expected, found } => {
                f.write_str("expected ")?;
                for (position, kind) in expected.iter().enumerate() {
                    let separator = match (position, expected.len() - position) {
                        (0, _) => "",
                        (1, 1) => " or ",
                        (_, 1) => ", or ",
                        _ => ", ",
                    };
                    write!(f, "{separator}{kind}")?;
                }
                write!(f, ", found {found}")
            }
            Self::InvalidIdentity(error) => error.fmt(f),
            Self::InvalidVersion(error) => error.fmt(f),
            Self::InvalidRequirement(error) => error.fmt(f),
            Self::InvalidParameterName(error) => error.fmt(f),
            Self::BlankDescription(error) => error.fmt(f),
            Self::UnknownParameterType(name) => write!(
                f,
                "unknown parameter type {name:?}; expected \"string\", \"integer\", or \"boolean\""
            ),
            Self::AllowedValuesRequireString => {
                f.write_str("allowed values are only supported for parameters of type \"string\"")
            }
            Self::InvalidAllowedValues(error) => error.fmt(f),
            Self::InvalidDefault(error) => write!(f, "invalid default: {error}"),
            Self::Inconsistent(error) => error.fmt(f),
        }
    }
}

/// The base type named by a parameter's `type` field.
#[derive(Clone, Copy)]
enum BaseType {
    String,
    Integer,
    Boolean,
}

type Members<'j> = BTreeMap<&'j str, &'j Json>;

/// The `dependencies` read so far.
#[derive(Default)]
struct Dependencies {
    /// Entries whose identity and requirement are both valid.
    requirements: BTreeMap<UnitIdentity, VersionRequirement>,
    /// Every validly named dependency, even one with an invalid requirement,
    /// so that `options` targeting it are not reported as undeclared.
    declared: BTreeSet<UnitIdentity>,
}

/// Walks a parsed manifest, collecting issues and the valid parts.
#[derive(Default)]
struct Reader {
    issues: Vec<ManifestIssue>,
}

impl Reader {
    fn report(&mut self, path: FieldPath, kind: IssueKind) {
        self.issues.push(ManifestIssue { path, kind });
    }

    fn wrong_type(&mut self, path: FieldPath, expected: &'static [JsonKind], found: &Json) {
        let found = JsonKind::of(found);
        self.report(path, IssueKind::WrongType { expected, found });
    }

    fn manifest(&mut self, json: &Json) -> Option<Manifest> {
        let root = FieldPath::default();
        let fields = self.fields(&root, json, MANIFEST_FIELDS)?;

        if let Some(schema) = fields.get("$schema") {
            self.string(&root.key("$schema"), schema);
        }
        let name = self
            .required(&root, &fields, "name")
            .and_then(|json| self.identity(&root.key("name"), json));
        let version = self
            .required(&root, &fields, "version")
            .and_then(|json| self.version(&root.key("version"), json));
        let description = self
            .required(&root, &fields, "description")
            .and_then(|json| self.description(&root.key("description"), json));
        let dependencies = fields
            .get("dependencies")
            .map(|json| self.dependencies(&root.key("dependencies"), json));
        let options = fields.get("options").map_or_else(BTreeMap::new, |json| {
            self.options(&root.key("options"), json)
        });
        let params = fields
            .get("params")
            .map_or_else(BTreeMap::new, |json| self.params(&root.key("params"), json));

        // When `dependencies` is not an object, no option target can be
        // judged undeclared, so only its type error is reported.
        let dependencies = dependencies.unwrap_or_else(|| Some(Dependencies::default()))?;
        for error in domain::relation_errors(name.as_ref(), &dependencies.declared, options.keys())
        {
            self.inconsistent(error);
        }

        // `relation_errors` has already reported every inconsistency that
        // `Manifest::new` could find, so its errors carry nothing new.
        Manifest::new(
            name?,
            version?,
            description?,
            dependencies.requirements,
            options,
            params,
        )
        .ok()
    }

    fn inconsistent(&mut self, error: ManifestError) {
        let path = match &error {
            ManifestError::SelfDependency(name) => FieldPath::default()
                .key("dependencies")
                .key(&name.to_string()),
            ManifestError::UndeclaredOptionTarget(target) => {
                FieldPath::default().key("options").key(&target.to_string())
            }
        };
        self.report(path, IssueKind::Inconsistent(error));
    }

    /// The members of an object, keeping the first of any duplicate keys.
    fn object<'j>(&mut self, path: &FieldPath, json: &'j Json) -> Option<Members<'j>> {
        let Json::Object(entries) = json else {
            self.wrong_type(path.clone(), &[JsonKind::Object], json);
            return None;
        };
        let mut members = BTreeMap::new();
        for (key, value) in entries {
            if members.contains_key(key.as_str()) {
                self.report(path.key(key), IssueKind::DuplicateKey);
            } else {
                members.insert(key.as_str(), value);
            }
        }
        Some(members)
    }

    /// The members of an object with a fixed set of fields, reporting the rest.
    fn fields<'j>(
        &mut self,
        path: &FieldPath,
        json: &'j Json,
        known: &[&str],
    ) -> Option<Members<'j>> {
        let members = self.object(path, json)?;
        for key in members.keys().filter(|key| !known.contains(key)) {
            self.report(path.key(key), IssueKind::UnknownField);
        }
        Some(members)
    }

    fn required<'j>(
        &mut self,
        path: &FieldPath,
        members: &Members<'j>,
        field: &str,
    ) -> Option<&'j Json> {
        let value = members.get(field).copied();
        if value.is_none() {
            self.report(path.key(field), IssueKind::MissingField);
        }
        value
    }

    fn string<'j>(&mut self, path: &FieldPath, json: &'j Json) -> Option<&'j str> {
        if let Json::String(text) = json {
            Some(text)
        } else {
            self.wrong_type(path.clone(), &[JsonKind::String], json);
            None
        }
    }

    fn scalar(&mut self, path: &FieldPath, json: &Json) -> Option<ParameterValue> {
        match json {
            Json::String(text) => Some(ParameterValue::String(text.clone())),
            Json::Integer(number) => Some(ParameterValue::Integer(*number)),
            Json::Bool(flag) => Some(ParameterValue::Boolean(*flag)),
            _ => {
                self.wrong_type(path.clone(), SCALAR, json);
                None
            }
        }
    }

    fn identity(&mut self, path: &FieldPath, json: &Json) -> Option<UnitIdentity> {
        let text = self.string(path, json)?;
        self.identity_key(path, text)
    }

    /// Parses an identity used as an object key. `path` ends at that key.
    fn identity_key(&mut self, path: &FieldPath, key: &str) -> Option<UnitIdentity> {
        UnitIdentity::parse(key)
            .map_err(|error| self.report(path.clone(), IssueKind::InvalidIdentity(error)))
            .ok()
    }

    fn parameter_name_key(&mut self, path: &FieldPath, key: &str) -> Option<ParameterName> {
        ParameterName::parse(key)
            .map_err(|error| self.report(path.clone(), IssueKind::InvalidParameterName(error)))
            .ok()
    }

    fn version(&mut self, path: &FieldPath, json: &Json) -> Option<Version> {
        let text = self.string(path, json)?;
        Version::parse(text)
            .map_err(|error| self.report(path.clone(), IssueKind::InvalidVersion(error)))
            .ok()
    }

    fn description(&mut self, path: &FieldPath, json: &Json) -> Option<Description> {
        let text = self.string(path, json)?;
        Description::new(text)
            .map_err(|error| self.report(path.clone(), IssueKind::BlankDescription(error)))
            .ok()
    }

    /// `None` when `dependencies` is not an object.
    fn dependencies(&mut self, path: &FieldPath, json: &Json) -> Option<Dependencies> {
        let members = self.object(path, json)?;
        let mut dependencies = Dependencies::default();
        for (key, value) in members {
            let path = path.key(key);
            let identity = self.identity_key(&path, key);
            let requirement = self.string(&path, value).and_then(|text| {
                VersionRequirement::parse(text)
                    .map_err(|error| {
                        self.report(path.clone(), IssueKind::InvalidRequirement(error))
                    })
                    .ok()
            });
            if let Some(identity) = identity {
                if let Some(requirement) = requirement {
                    dependencies
                        .requirements
                        .insert(identity.clone(), requirement);
                }
                dependencies.declared.insert(identity);
            }
        }
        Some(dependencies)
    }

    fn options(&mut self, path: &FieldPath, json: &Json) -> BTreeMap<UnitIdentity, OptionValues> {
        let Some(members) = self.object(path, json) else {
            return BTreeMap::new();
        };
        let mut options = BTreeMap::new();
        for (key, value) in members {
            let path = path.key(key);
            let identity = self.identity_key(&path, key);
            let values = self.option_values(&path, value);
            if let Some(identity) = identity {
                options.insert(identity, values);
            }
        }
        options
    }

    fn option_values(&mut self, path: &FieldPath, json: &Json) -> OptionValues {
        let Some(members) = self.object(path, json) else {
            return OptionValues::new();
        };
        let mut values = OptionValues::new();
        for (key, value) in members {
            let path = path.key(key);
            let name = self.parameter_name_key(&path, key);
            let value = self.scalar(&path, value);
            if let (Some(name), Some(value)) = (name, value) {
                values.insert(name, value);
            }
        }
        values
    }

    fn params(
        &mut self,
        path: &FieldPath,
        json: &Json,
    ) -> BTreeMap<ParameterName, ParameterDefinition> {
        let Some(members) = self.object(path, json) else {
            return BTreeMap::new();
        };
        let mut params = BTreeMap::new();
        for (key, value) in members {
            let path = path.key(key);
            let name = self.parameter_name_key(&path, key);
            let definition = self.parameter(&path, value);
            if let (Some(name), Some(definition)) = (name, definition) {
                params.insert(name, definition);
            }
        }
        params
    }

    fn parameter(&mut self, path: &FieldPath, json: &Json) -> Option<ParameterDefinition> {
        let fields = self.fields(path, json, PARAMETER_FIELDS)?;
        let base = self
            .required(path, &fields, "type")
            .and_then(|json| self.base_type(&path.key("type"), json));
        let description = self
            .required(path, &fields, "description")
            .and_then(|json| self.description(&path.key("description"), json));
        let allowed = fields
            .get("allowed")
            .map(|json| self.allowed(&path.key("allowed"), json));
        let default = fields
            .get("default")
            .and_then(|json| self.scalar(&path.key("default"), json));

        let parameter_type = match (base?, allowed) {
            (BaseType::String, None) => Some(ParameterType::String),
            (BaseType::String, Some(allowed)) => allowed.map(ParameterType::EnumeratedString),
            (_, Some(_)) => {
                self.report(path.key("allowed"), IssueKind::AllowedValuesRequireString);
                None
            }
            (BaseType::Integer, None) => Some(ParameterType::Integer),
            (BaseType::Boolean, None) => Some(ParameterType::Boolean),
        }?;
        if let Some(Err(error)) = default.as_ref().map(|value| parameter_type.check(value)) {
            self.report(path.key("default"), IssueKind::InvalidDefault(error));
            return None;
        }
        ParameterDefinition::new(parameter_type, description?, default).ok()
    }

    fn base_type(&mut self, path: &FieldPath, json: &Json) -> Option<BaseType> {
        match self.string(path, json)? {
            "string" => Some(BaseType::String),
            "integer" => Some(BaseType::Integer),
            "boolean" => Some(BaseType::Boolean),
            other => {
                self.report(
                    path.clone(),
                    IssueKind::UnknownParameterType(other.to_owned()),
                );
                None
            }
        }
    }

    fn allowed(&mut self, path: &FieldPath, json: &Json) -> Option<AllowedValues> {
        let Json::Array(items) = json else {
            self.wrong_type(path.clone(), &[JsonKind::Array], json);
            return None;
        };
        let mut values = Vec::with_capacity(items.len());
        let mut all_strings = true;
        for (index, item) in items.iter().enumerate() {
            match self.string(&path.index(index), item) {
                Some(text) => values.push(text.to_owned()),
                None => all_strings = false,
            }
        }
        if !all_strings {
            return None;
        }
        AllowedValues::new(values)
            .map_err(|error| self.report(path.clone(), IssueKind::InvalidAllowedValues(error)))
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::{FieldPath, IssueKind, JsonKind, ReadManifestError, read_manifest};
    use crate::domain::{
        AllowedValuesError, IdentityErrorReason, ManifestError, ParameterType, ParameterValue,
        ParameterValueError, Segment, UnitIdentity, ValueKind,
    };

    const FULL: &str = r#"{
  // Editor support only.
  "$schema": "./manifest.schema.json",
  "name": "@metamao/cloudflare-angular-ssr",
  "version": "1.0.0",
  "description": "Angular SSR on Cloudflare Workers.",
  "dependencies": {
    "@metamao/cloudflare-worker": "^2.1.0",
    "@metamao/node": "~1.4",
  },
  "options": {
    "@metamao/cloudflare-worker": { "nodeCompat": true, "compatibilityDate": "2026-01-01" },
    /* The package manager the stack is tested with. */
    "@metamao/node": { "packageManager": "pnpm", "major": 22 },
  },
  "params": {
    "appName": { "type": "string", "description": "The application name." },
    "port": { "type": "integer", "description": "Local port.", "default": 8080 },
    "ssr": { "type": "boolean", "description": "Render on the server.", "default": true },
    "locale": {
      "type": "string",
      "description": "Default locale.",
      "allowed": ["en", "es"],
      "default": "es",
    },
  },
}"#;

    /// Every issue as `(path, kind)`, with paths rendered as text.
    fn issues(text: &str) -> Vec<(String, IssueKind)> {
        match read_manifest(text) {
            Err(ReadManifestError::Invalid(issues)) => issues
                .into_iter()
                .map(|issue| (issue.path().to_string(), issue.kind().clone()))
                .collect(),
            other => panic!("expected validation issues, got {other:?}"),
        }
    }

    /// A minimal valid manifest with `extra` spliced in as more members.
    fn with(extra: &str) -> String {
        format!(
            r#"{{"name": "@metamao/app", "version": "1.0.0", "description": "An app.", {extra}}}"#
        )
    }

    fn identity(input: &str) -> UnitIdentity {
        input.parse().unwrap()
    }

    #[test]
    fn reads_every_field_form() {
        let manifest = read_manifest(FULL).unwrap();
        assert_eq!(
            manifest.name().to_string(),
            "@metamao/cloudflare-angular-ssr"
        );
        assert_eq!(manifest.version().to_string(), "1.0.0");
        assert_eq!(
            manifest.description().as_str(),
            "Angular SSR on Cloudflare Workers."
        );

        let dependencies: Vec<(String, String)> = manifest
            .dependencies()
            .iter()
            .map(|(name, requirement)| (name.to_string(), requirement.to_string()))
            .collect();
        assert_eq!(
            dependencies,
            [
                ("@metamao/cloudflare-worker".to_owned(), "^2.1.0".to_owned()),
                ("@metamao/node".to_owned(), "~1.4".to_owned()),
            ]
        );

        let node = &manifest.options()[&identity("@metamao/node")];
        let values: Vec<(String, ParameterValue)> = node
            .iter()
            .map(|(name, value)| (name.to_string(), value.clone()))
            .collect();
        assert_eq!(
            values,
            [
                ("major".to_owned(), ParameterValue::Integer(22)),
                (
                    "packageManager".to_owned(),
                    ParameterValue::String("pnpm".to_owned())
                ),
            ]
        );

        let params = manifest.params();
        let names: Vec<&str> = params.keys().map(|name| name.as_str()).collect();
        assert_eq!(names, ["appName", "locale", "port", "ssr"]);
        let app_name = params.values().next().unwrap();
        assert_eq!(app_name.parameter_type(), &ParameterType::String);
        assert!(app_name.is_required());
        let locale = &params[&"locale".parse().unwrap()];
        let ParameterType::EnumeratedString(allowed) = locale.parameter_type() else {
            panic!("locale should be an enumerated string");
        };
        assert_eq!(allowed.as_slice(), ["en", "es"]);
        assert_eq!(
            locale.default(),
            Some(&ParameterValue::String("es".to_owned()))
        );
        assert!(!locale.is_required());
        let port = &params[&"port".parse().unwrap()];
        assert_eq!(port.parameter_type(), &ParameterType::Integer);
        assert_eq!(port.default(), Some(&ParameterValue::Integer(8080)));
    }

    #[test]
    fn optional_collections_default_to_empty() {
        let manifest = read_manifest(&with(r#""params": {}"#)).unwrap();
        assert!(manifest.dependencies().is_empty());
        assert!(manifest.options().is_empty());
        assert!(manifest.params().is_empty());
    }

    #[test]
    fn syntax_errors_are_reported_with_their_position() {
        let Err(ReadManifestError::Syntax(error)) = read_manifest("{\n  \"name\": 'x'\n}") else {
            panic!("expected a syntax error");
        };
        assert_eq!((error.line(), error.column()), (2, 11));
    }

    #[test]
    fn required_fields_are_reported_when_missing_or_blank() {
        assert_eq!(
            issues("{}"),
            [
                ("description".to_owned(), IssueKind::MissingField),
                ("name".to_owned(), IssueKind::MissingField),
                ("version".to_owned(), IssueKind::MissingField),
            ]
        );
        let blank = r#"{"name": "@a/b", "version": "1.0.0", "description": "  "}"#;
        assert!(matches!(
            issues(blank).as_slice(),
            [(path, IssueKind::BlankDescription(_))] if path == "description"
        ));
    }

    #[test]
    fn the_root_must_be_an_object() {
        assert_eq!(
            issues("[]"),
            [(
                "(manifest)".to_owned(),
                IssueKind::WrongType {
                    expected: &[JsonKind::Object],
                    found: JsonKind::Array,
                }
            )]
        );
    }

    #[test]
    fn unknown_fields_are_rejected_at_every_level() {
        let text = with(
            r#""amendments": [], "params": {"a": {"type": "string", "description": "A.", "required": true}}"#,
        );
        assert_eq!(
            issues(&text),
            [
                ("amendments".to_owned(), IssueKind::UnknownField),
                ("params.a.required".to_owned(), IssueKind::UnknownField),
            ]
        );
    }

    #[test]
    fn duplicate_keys_are_rejected() {
        let text = with(
            r#""version": "2.0.0", "params": {"a": {"type": "string", "type": "integer", "description": "A."}}"#,
        );
        assert_eq!(
            issues(&text),
            [
                ("params.a.type".to_owned(), IssueKind::DuplicateKey),
                ("version".to_owned(), IssueKind::DuplicateKey),
            ]
        );
    }

    #[test]
    fn identities_versions_and_requirements_are_validated() {
        let text = r#"{
          "name": "@Metamao/app",
          "version": "1.0",
          "description": "An app.",
          "dependencies": { "@metamao/node": "latest", "metamao/docker": "^1.0.0" }
        }"#;
        let issues = issues(text);
        let paths: Vec<&str> = issues.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "dependencies[\"@metamao/node\"]",
                "dependencies[\"metamao/docker\"]",
                "name",
                "version",
            ]
        );
        assert!(matches!(issues[0].1, IssueKind::InvalidRequirement(_)));
        assert!(matches!(
            &issues[1].1,
            IssueKind::InvalidIdentity(error) if error.reason() == &IdentityErrorReason::MissingAtSign
        ));
        assert!(matches!(
            &issues[2].1,
            IssueKind::InvalidIdentity(error)
                if *error.reason() == IdentityErrorReason::InvalidCharacter {
                    segment: Segment::Owner,
                    character: 'M',
                }
        ));
        assert!(matches!(issues[3].1, IssueKind::InvalidVersion(_)));
    }

    #[test]
    fn options_must_target_declared_dependencies() {
        let text = with(
            r#""dependencies": {"@metamao/node": "^1.0.0"},
               "options": {"@metamao/node": {"major": 22}, "@metamao/docker": {"base": "alpine"}}"#,
        );
        assert_eq!(
            issues(&text),
            [(
                "options[\"@metamao/docker\"]".to_owned(),
                IssueKind::Inconsistent(ManifestError::UndeclaredOptionTarget(identity(
                    "@metamao/docker"
                )))
            )]
        );
    }

    #[test]
    fn a_dependency_with_an_invalid_requirement_still_counts_as_declared() {
        let text = with(
            r#""dependencies": {"@metamao/node": "latest"},
               "options": {"@metamao/node": {"major": 22}}"#,
        );
        let issues = issues(&text);
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(matches!(issues[0].1, IssueKind::InvalidRequirement(_)));
    }

    #[test]
    fn a_unit_cannot_depend_on_itself() {
        let text = with(r#""dependencies": {"@metamao/app": "^1.0.0"}"#);
        assert_eq!(
            issues(&text),
            [(
                "dependencies[\"@metamao/app\"]".to_owned(),
                IssueKind::Inconsistent(ManifestError::SelfDependency(identity("@metamao/app")))
            )]
        );
    }

    #[test]
    fn option_values_must_be_scalars_with_valid_names() {
        let text = with(
            r#""dependencies": {"@metamao/node": "^1.0.0"},
               "options": {"@metamao/node": {"versions": [22], "ratio": 0.5, "bad-name": true, "nothing": null}}"#,
        );
        let issues = issues(&text);
        let paths: Vec<&str> = issues.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "options[\"@metamao/node\"][\"bad-name\"]",
                "options[\"@metamao/node\"].nothing",
                "options[\"@metamao/node\"].ratio",
                "options[\"@metamao/node\"].versions",
            ]
        );
        assert!(matches!(issues[0].1, IssueKind::InvalidParameterName(_)));
        assert!(matches!(
            issues[2].1,
            IssueKind::WrongType {
                found: JsonKind::OtherNumber,
                ..
            }
        ));
        assert_eq!(
            issues[3].1.to_string(),
            "expected a string, an integer, or a boolean, found an array"
        );
    }

    #[test]
    fn parameter_definitions_are_validated() {
        let text = with(
            r#""params": {
                 "noType": {"description": "A."},
                 "badType": {"type": "number", "description": "A."},
                 "wrongDefault": {"type": "integer", "description": "A.", "default": "8080"},
                 "notAllowed": {"type": "string", "description": "A.", "allowed": ["a", "b"], "default": "c"},
                 "allowedOnBoolean": {"type": "boolean", "description": "A.", "allowed": ["true"]},
                 "emptyAllowed": {"type": "string", "description": "A.", "allowed": []},
                 "repeatedAllowed": {"type": "string", "description": "A.", "allowed": ["a", "a"]},
                 "mixedAllowed": {"type": "string", "description": "A.", "allowed": ["a", 1]},
                 "blank": {"type": "string", "description": ""}
               }"#,
        );
        let issues = issues(&text);
        let found: Vec<(&str, &IssueKind)> = issues
            .iter()
            .map(|(path, kind)| (path.as_str(), kind))
            .collect();
        assert_eq!(found.len(), 9, "{found:#?}");
        assert!(matches!(
            found[0],
            (
                "params.allowedOnBoolean.allowed",
                IssueKind::AllowedValuesRequireString
            )
        ));
        assert!(
            matches!(found[1], ("params.badType.type", IssueKind::UnknownParameterType(name)) if name == "number")
        );
        assert!(matches!(
            found[2],
            ("params.blank.description", IssueKind::BlankDescription(_))
        ));
        assert!(matches!(
            found[3],
            (
                "params.emptyAllowed.allowed",
                IssueKind::InvalidAllowedValues(AllowedValuesError::Empty)
            )
        ));
        assert!(matches!(
            found[4],
            (
                "params.mixedAllowed.allowed[1]",
                IssueKind::WrongType {
                    found: JsonKind::Integer,
                    ..
                }
            )
        ));
        assert!(matches!(
            found[5],
            ("params.noType.type", IssueKind::MissingField)
        ));
        assert!(matches!(
            found[6],
            ("params.notAllowed.default", IssueKind::InvalidDefault(ParameterValueError::NotAllowed { value, .. })) if value == "c"
        ));
        assert!(matches!(
            found[7],
            ("params.repeatedAllowed.allowed", IssueKind::InvalidAllowedValues(AllowedValuesError::Duplicates(values))) if values == &["a"]
        ));
        assert!(matches!(
            found[8],
            (
                "params.wrongDefault.default",
                IssueKind::InvalidDefault(ParameterValueError::TypeMismatch {
                    expected: ValueKind::Integer,
                    found: ValueKind::String,
                })
            )
        ));
    }

    #[test]
    fn every_problem_is_reported_in_one_pass() {
        let text = r#"{
          "name": "@metamao/App",
          "version": "one",
          "dependencies": {"@metamao/node": 1},
          "options": {"@metamao/docker": {}},
          "params": {"x": {"type": "string"}},
          "extra": true
        }"#;
        let paths: Vec<String> = issues(text).into_iter().map(|(path, _)| path).collect();
        assert_eq!(
            paths,
            [
                "dependencies[\"@metamao/node\"]",
                "description",
                "extra",
                "name",
                "options[\"@metamao/docker\"]",
                "params.x.description",
                "version",
            ]
        );
    }

    #[test]
    fn key_order_in_the_source_does_not_matter() {
        let forward = with(
            r#""dependencies": {"@a/a": "^1", "@b/b": "^2"},
               "options": {"@a/a": {"x": 1, "y": 2}, "@c/c": {}},
               "params": {"p": {"type": "integer", "description": "P.", "default": "1"}, "q": {"type": "x", "description": "Q."}}"#,
        );
        let backward = r#"{
          "params": {"q": {"description": "Q.", "type": "x"}, "p": {"default": "1", "description": "P.", "type": "integer"}},
          "options": {"@c/c": {}, "@a/a": {"y": 2, "x": 1}},
          "dependencies": {"@b/b": "^2", "@a/a": "^1"},
          "description": "An app.", "version": "1.0.0", "name": "@metamao/app"
        }"#;
        assert_eq!(read_manifest(&forward), read_manifest(backward));
        assert_eq!(issues(backward).len(), 3);

        let reordered = r#"{
          "description": "Angular SSR on Cloudflare Workers.",
          "params": {
            "ssr": { "default": true, "description": "Render on the server.", "type": "boolean" },
            "port": { "default": 8080, "type": "integer", "description": "Local port." },
            "locale": { "default": "es", "allowed": ["en", "es"], "description": "Default locale.", "type": "string" },
            "appName": { "description": "The application name.", "type": "string" }
          },
          "options": {
            "@metamao/node": { "major": 22, "packageManager": "pnpm" },
            "@metamao/cloudflare-worker": { "compatibilityDate": "2026-01-01", "nodeCompat": true }
          },
          "dependencies": { "@metamao/node": "~1.4", "@metamao/cloudflare-worker": "^2.1.0" },
          "version": "1.0.0",
          "name": "@metamao/cloudflare-angular-ssr"
        }"#;
        assert_eq!(
            read_manifest(reordered).unwrap(),
            read_manifest(FULL).unwrap()
        );
    }

    #[test]
    fn no_truncation_of_a_manifest_panics() {
        for end in (0..=FULL.len()).filter(|end| FULL.is_char_boundary(*end)) {
            let _ = read_manifest(&FULL[..end]);
        }
    }

    #[test]
    fn diagnostics_name_the_field_and_the_fix() {
        let error = read_manifest(&with(
            r#""params": {"port": {"type": "integer", "description": "P.", "default": 80.5}}"#,
        ))
        .unwrap_err();
        let message = error.to_string();
        assert!(
            message.starts_with("the manifest has 1 problem:"),
            "{message}"
        );
        assert!(
            message.contains(
                "params.port.default: expected a string, an integer, or a boolean, found a number that is not a 64-bit integer"
            ),
            "{message}"
        );
        assert_eq!(FieldPath::default().to_string(), "(manifest)");
    }
}
