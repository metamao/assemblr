//! Reading `manifest.jsonc` text into a validated [`Manifest`].
//!
//! The reader is pure: it takes the manifest's text, not a path. It reports
//! every problem it finds, each with the path of the field concerned, sorted
//! by path so the result does not depend on the order of keys in the source.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use super::jsonc::{self, Json, SyntaxError};
use crate::domain::{
    self, AmendmentDeclaration, BlankDescription, Description, IdentityError, Manifest,
    ManifestError, PlaceholderName, PlaceholderNameError, Placeholders, Project, RelativePath,
    RelativePathError, RequirementError, UnitIdentity, Version, VersionError, VersionRequirement,
};

pub(crate) const MANIFEST_FIELDS: &[&str] = &[
    "$schema",
    "name",
    "version",
    "description",
    "dependencies",
    "placeholders",
    "amendments",
];
const PROJECT_FIELDS: &[&str] = &["$schema", "dependencies"];
const PLACEHOLDER_FIELDS: &[&str] = &["replace", "ignore"];
const AMENDMENT_FIELDS: &[&str] = &["unit", "base"];
/// The fields of parameters and options, reported as removed rather than
/// unknown.
const REMOVED_FIELDS: &[&str] = &["params", "options"];

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

/// Reads and validates the text of a consumer project's `assemblr.jsonc`,
/// by the manifest's rules for `dependencies` (see
/// `docs/domain/consumer-projects.md`).
pub fn read_project(text: &str) -> Result<Project, ReadManifestError> {
    let json = jsonc::parse(text).map_err(ReadManifestError::Syntax)?;
    let mut reader = Reader::default();
    let project = reader.project(&json);
    let mut issues = reader.issues;
    issues.sort_by(|a, b| a.path.cmp(&b.path));
    match project {
        Some(project) if issues.is_empty() => Ok(project),
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
            Self::Syntax(error) => write!(f, "not valid JSONC: {error}"),
            Self::Invalid(issues) => {
                let plural = if issues.len() == 1 { "" } else { "s" };
                write!(f, "{} problem{plural}:", issues.len())?;
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

/// The location of a value in a manifest, such as `amendments["a.json"].base`.
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
            Json::OtherNumber(_) => Self::OtherNumber,
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
    /// The field is not part of the manifest format. Holds the fields the
    /// object accepts.
    UnknownField(&'static [&'static str]),
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
    /// The key or entry is not a placeholder name.
    InvalidPlaceholder(PlaceholderNameError),
    /// The entry appears more than once in the same list. The first
    /// occurrence is the one kept.
    DuplicateEntry,
    /// The placeholder is listed both to replace and to ignore.
    ReplacedAndIgnored(PlaceholderName),
    /// The key is not a valid unit file path.
    InvalidPath(RelativePathError),
    /// The description is empty or only whitespace.
    BlankDescription(BlankDescription),
    /// The field belonged to parameters and options, which Assemblr no
    /// longer has: a unit ships placeholders instead.
    Removed(RemovedField),
    /// The manifest's fields are inconsistent with each other.
    Inconsistent(ManifestError),
}

impl fmt::Display for IssueKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownField(known) => {
                f.write_str("unknown field; the fields here are ")?;
                for (position, name) in known.iter().enumerate() {
                    let separator = match (position, known.len() - position) {
                        (0, _) => "",
                        (1, 1) => " and ",
                        (_, 1) => ", and ",
                        _ => ", ",
                    };
                    write!(f, "{separator}`{name}`")?;
                }
                Ok(())
            }
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
            Self::InvalidPlaceholder(error) => error.fmt(f),
            Self::DuplicateEntry => f.write_str("duplicate entry"),
            Self::ReplacedAndIgnored(name) => write!(
                f,
                "{name} is in both `replace` and `ignore`; keep it in `replace` if the consumer \
                 sets it, or in `ignore` if it is not a placeholder"
            ),
            Self::InvalidPath(error) => error.fmt(f),
            Self::BlankDescription(error) => error.fmt(f),
            Self::Removed(removed) => removed.fmt(f),
            Self::Inconsistent(error) => error.fmt(f),
        }
    }
}

/// A field of parameters and options, which Assemblr no longer reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovedField {
    /// A unit's `params`.
    Params,
    /// A unit's `options` for its dependencies.
    UnitOptions,
    /// A consumer project's `options`.
    ProjectOptions,
}

impl fmt::Display for RemovedField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Params => {
                "parameters were replaced by placeholders: write a token such as \
                 __PROJECT_NAME__ in the files and declare it under \"placeholders\".\"replace\" \
                 with a description, then remove `params`"
            }
            Self::UnitOptions => {
                "options were removed: to fix a value a dependency leaves to the consumer, amend \
                 its file with `assemblr unit amend`, then remove `options`"
            }
            Self::ProjectOptions => {
                "options were removed: the units' files hold placeholders that you replace in \
                 the files themselves, and `assemblr status` lists them; remove `options`"
            }
        })
    }
}

/// The text of `manifest` with `declaration` added for `path`, as
/// `unit amend` records a new amendment: a member of the top-level
/// `amendments` object, which is created after the last member when the
/// manifest has none. Every other byte of the text, comments and formatting
/// included, is kept; the new member follows the indentation of its
/// neighbours.
///
/// `text` must be the text `manifest` was read from. The edited text is read
/// again and must describe exactly `manifest` plus the declaration; if it
/// does not, or the text cannot be located precisely, nothing is produced
/// and the error carries the member to add by hand.
pub fn declare_amendment(
    text: &str,
    manifest: &Manifest,
    path: &RelativePath,
    declaration: &AmendmentDeclaration,
) -> Result<String, EditError> {
    let entry = format!(
        "{}: {{ \"unit\": {}, \"base\": {} }}",
        json_string(path.as_str()),
        json_string(&declaration.unit().to_string()),
        json_string(&declaration.base().to_string())
    );
    let failure = || EditError {
        action: EditAction::Add,
        entries: vec![entry.clone()],
    };
    let expected = manifest
        .with_amendment(path.clone(), declaration.clone())
        .ok_or_else(failure)?;
    let edited = insert_amendment(text, &entry).ok_or_else(failure)?;
    match read_manifest(&edited) {
        Ok(manifest) if manifest == expected => Ok(edited),
        _ => Err(failure()),
    }
}

/// `text` with `entry` inserted as a member of its `amendments` object, or
/// `None` when the text cannot be located precisely.
fn insert_amendment(text: &str, entry: &str) -> Option<String> {
    let strict = jsonc::to_strict_json(text).ok()?;
    let open = jsonc::root_object(&strict)?;
    let (members, _) = jsonc::members(&strict, open)?;
    let last = members.last()?;
    // The members' own layout: the whitespace before the first key on its
    // line, or nothing when the first member shares the brace's line.
    let indent = |key_start: usize| -> Option<String> {
        let line_start = text.get(..key_start)?.rfind('\n')? + 1;
        let indent = text.get(line_start..key_start)?;
        indent
            .chars()
            .all(|c| c == ' ' || c == '\t')
            .then(|| indent.to_owned())
    };
    let outer = indent(members.first()?.key_start);
    let inner = outer.as_ref().map(|indent| indent.repeat(2));
    // The manifest's own line ending, so that the edit does not mix them.
    let newline = match text.find('\n') {
        Some(at) if text[..at].ends_with('\r') => "\r\n",
        _ => "\n",
    };
    let amendments = members.iter().find(|member| member.key == "amendments");
    let (at, insertion) = match amendments {
        Some(member) => {
            let (declared, close) = jsonc::members(&strict, member.value_start)?;
            match declared.last() {
                Some(last) => {
                    let indent = indent(declared.first()?.key_start);
                    let insertion = match indent {
                        Some(indent) => format!(",{newline}{indent}{entry}"),
                        None => format!(", {entry}"),
                    };
                    (last.value_end, insertion)
                }
                None => {
                    let insertion = match (&inner, &outer) {
                        (Some(inner), Some(outer)) => {
                            format!("{newline}{inner}{entry}{newline}{outer}")
                        }
                        _ => format!(" {entry} "),
                    };
                    let _ = close;
                    (member.value_start + 1, insertion)
                }
            }
        }
        None => {
            let insertion = match (&inner, &outer) {
                (Some(inner), Some(outer)) => format!(
                    ",{newline}{outer}\"amendments\": {{{newline}{inner}{entry}{newline}{outer}}}"
                ),
                _ => format!(", \"amendments\": {{ {entry} }}"),
            };
            (last.value_end, insertion)
        }
    };
    let mut edited = String::with_capacity(text.len() + insertion.len());
    edited.push_str(text.get(..at)?);
    edited.push_str(&insertion);
    edited.push_str(text.get(at..)?);
    Some(edited)
}

/// The text of `manifest` with the `base` of each amendment in `bases`
/// replaced by the given version, as `unit update` carries an amendment to
/// the provider's new version. Only the value of each `base` member changes;
/// every other byte of the text, comments and formatting included, is kept.
///
/// `text` must be the text `manifest` was read from. The edited text is read
/// again and must describe exactly `manifest` with those bases; if it does
/// not, or the text cannot be located precisely, nothing is produced and
/// the error carries the members to change by hand.
pub fn rebase_amendments(
    text: &str,
    manifest: &Manifest,
    bases: &BTreeMap<RelativePath, Version>,
) -> Result<String, EditError> {
    let entries: Vec<String> = bases
        .iter()
        .filter_map(|(path, base)| {
            let declaration = manifest.amendments().get(path)?;
            Some(format!(
                "{}: {{ \"unit\": {}, \"base\": {} }}",
                json_string(path.as_str()),
                json_string(&declaration.unit().to_string()),
                json_string(&base.to_string())
            ))
        })
        .collect();
    let failure = || EditError {
        action: EditAction::Replace,
        entries: entries.clone(),
    };
    let expected = manifest.with_bases(bases).ok_or_else(failure)?;
    let edited = replace_bases(text, bases).ok_or_else(failure)?;
    match read_manifest(&edited) {
        Ok(manifest) if manifest == expected => Ok(edited),
        _ => Err(failure()),
    }
}

/// `text` with the `base` value of each declaration in `bases` replaced, or
/// `None` when a declaration or its `base` cannot be located precisely.
fn replace_bases(text: &str, bases: &BTreeMap<RelativePath, Version>) -> Option<String> {
    let strict = jsonc::to_strict_json(text).ok()?;
    let open = jsonc::root_object(&strict)?;
    let (members, _) = jsonc::members(&strict, open)?;
    let amendments = members.iter().find(|member| member.key == "amendments")?;
    let (declared, _) = jsonc::members(&strict, amendments.value_start)?;
    // Each replacement's span, from the end of the text backwards, so that
    // earlier offsets stay valid while later spans are replaced.
    let mut replacements = Vec::new();
    for (path, base) in bases {
        let declaration = declared.iter().find(|member| member.key == path.as_str())?;
        let (fields, _) = jsonc::members(&strict, declaration.value_start)?;
        let field = fields.iter().find(|member| member.key == "base")?;
        replacements.push((
            field.value_start,
            field.value_end,
            json_string(&base.to_string()),
        ));
    }
    replacements.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
    let mut edited = text.to_owned();
    for (start, end, value) in replacements {
        edited.get(start..end)?;
        edited.replace_range(start..end, &value);
    }
    Some(edited)
}

/// `value` as a JSON string literal.
fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| format!("{value:?}"))
}

/// The manifest could not be edited safely. Nothing was changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditError {
    action: EditAction,
    entries: Vec<String>,
}

/// What the edit would have done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditAction {
    /// Add one member to the `amendments` object.
    Add,
    /// Replace the `base` of existing members.
    Replace,
}

impl EditError {
    /// The `amendments` members the edit would have added or replaced, as
    /// JSON text.
    pub fn entries(&self) -> &[String] {
        &self.entries
    }
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let entries = self.entries.join(", ");
        match self.action {
            EditAction::Add => write!(
                f,
                "`manifest.jsonc` could not be edited safely, so it is unchanged; add this member \
                 to its top-level `amendments` object by hand: {entries}"
            ),
            EditAction::Replace => write!(
                f,
                "`manifest.jsonc` could not be edited safely, so it is unchanged; replace these \
                 members of its top-level `amendments` object by hand: {entries}"
            ),
        }
    }
}

impl std::error::Error for EditError {}

type Members<'j> = BTreeMap<&'j str, &'j Json>;

/// The `dependencies` read so far.
#[derive(Default)]
struct Dependencies {
    /// Entries whose identity and requirement are both valid.
    requirements: BTreeMap<UnitIdentity, VersionRequirement>,
    /// Every validly named dependency, even one with an invalid
    /// requirement, so that a dependency on the unit itself is reported.
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
        let fields = self.fields_or_removed(&root, json, MANIFEST_FIELDS, REMOVED_FIELDS)?;

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
        let placeholders = fields.get("placeholders").map_or_else(
            || Some(Placeholders::default()),
            |json| self.placeholders(&root.key("placeholders"), json),
        );
        let amendments = fields.get("amendments").map_or_else(BTreeMap::new, |json| {
            self.amendments(&root.key("amendments"), json)
        });
        for (field, removed) in [
            ("params", RemovedField::Params),
            ("options", RemovedField::UnitOptions),
        ] {
            if fields.contains_key(field) {
                self.report(root.key(field), IssueKind::Removed(removed));
            }
        }

        // When `dependencies` is not an object, no dependency can be judged
        // to be the unit itself, so only its type error is reported.
        let dependencies = dependencies.unwrap_or_else(|| Some(Dependencies::default()))?;
        for error in domain::relation_errors(name.as_ref(), &dependencies.declared) {
            self.inconsistent(error);
        }

        // `relation_errors` has already reported every inconsistency that
        // `Manifest::new` could find, so its errors carry nothing new.
        Manifest::new(
            name?,
            version?,
            description?,
            dependencies.requirements,
            placeholders?,
            amendments,
        )
        .ok()
    }

    fn project(&mut self, json: &Json) -> Option<Project> {
        let root = FieldPath::default();
        let fields = self.fields_or_removed(&root, json, PROJECT_FIELDS, &["options"])?;
        if let Some(schema) = fields.get("$schema") {
            self.string(&root.key("$schema"), schema);
        }
        let dependencies = fields
            .get("dependencies")
            .map(|json| self.dependencies(&root.key("dependencies"), json));
        if fields.contains_key("options") {
            self.report(
                root.key("options"),
                IssueKind::Removed(RemovedField::ProjectOptions),
            );
        }
        let dependencies = dependencies.unwrap_or_else(|| Some(Dependencies::default()))?;
        Some(Project::new(dependencies.requirements))
    }

    fn inconsistent(&mut self, error: ManifestError) {
        let path = match &error {
            ManifestError::SelfDependency(name) => FieldPath::default()
                .key("dependencies")
                .key(&name.to_string()),
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
        known: &'static [&'static str],
    ) -> Option<Members<'j>> {
        self.fields_or_removed(path, json, known, &[])
    }

    /// [`fields`](Self::fields), where the `removed` fields are left for
    /// the caller to report as such.
    fn fields_or_removed<'j>(
        &mut self,
        path: &FieldPath,
        json: &'j Json,
        known: &'static [&'static str],
        removed: &[&str],
    ) -> Option<Members<'j>> {
        let members = self.object(path, json)?;
        for key in members
            .keys()
            .filter(|key| !known.contains(key) && !removed.contains(key))
        {
            self.report(path.key(key), IssueKind::UnknownField(known));
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

    fn placeholder_name(&mut self, path: &FieldPath, text: &str) -> Option<PlaceholderName> {
        PlaceholderName::parse(text)
            .map_err(|error| self.report(path.clone(), IssueKind::InvalidPlaceholder(error)))
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

    /// `placeholders`: the placeholders to replace, each with its
    /// description, and the tokens to ignore. `None` when it cannot be
    /// read.
    fn placeholders(&mut self, path: &FieldPath, json: &Json) -> Option<Placeholders> {
        let fields = self.fields(path, json, PLACEHOLDER_FIELDS)?;
        let mut valid = true;
        let mut replace = BTreeMap::new();
        if let Some(json) = fields.get("replace") {
            let path = path.key("replace");
            match self.object(&path, json) {
                Some(members) => {
                    for (key, value) in members {
                        let path = path.key(key);
                        let name = self.placeholder_name(&path, key);
                        let description = self.description(&path, value);
                        match (name, description) {
                            (Some(name), Some(description)) => {
                                replace.insert(name, description);
                            }
                            _ => valid = false,
                        }
                    }
                }
                None => valid = false,
            }
        }
        let mut ignore = std::collections::BTreeSet::new();
        if let Some(json) = fields.get("ignore") {
            let path = path.key("ignore");
            match json {
                Json::Array(items) => {
                    for (index, item) in items.iter().enumerate() {
                        let path = path.index(index);
                        let name = self
                            .string(&path, item)
                            .and_then(|text| self.placeholder_name(&path, text));
                        match name {
                            Some(name) if ignore.contains(&name) => {
                                self.report(path, IssueKind::DuplicateEntry);
                            }
                            Some(name) => {
                                ignore.insert(name);
                            }
                            None => valid = false,
                        }
                    }
                }
                _ => {
                    self.wrong_type(path, &[JsonKind::Array], json);
                    valid = false;
                }
            }
        }
        match Placeholders::new(replace, ignore) {
            Ok(placeholders) => valid.then_some(placeholders),
            Err(both) => {
                for name in both {
                    self.report(path.key("ignore"), IssueKind::ReplacedAndIgnored(name));
                }
                None
            }
        }
    }

    fn amendments(
        &mut self,
        path: &FieldPath,
        json: &Json,
    ) -> BTreeMap<RelativePath, AmendmentDeclaration> {
        let Some(members) = self.object(path, json) else {
            return BTreeMap::new();
        };
        let mut amendments = BTreeMap::new();
        for (key, value) in members {
            let path = path.key(key);
            let amended = RelativePath::parse(key)
                .map_err(|error| self.report(path.clone(), IssueKind::InvalidPath(error)))
                .ok();
            let declaration = self.amendment(&path, value);
            if let (Some(amended), Some(declaration)) = (amended, declaration) {
                amendments.insert(amended, declaration);
            }
        }
        amendments
    }

    fn amendment(&mut self, path: &FieldPath, json: &Json) -> Option<AmendmentDeclaration> {
        let fields = self.fields(path, json, AMENDMENT_FIELDS)?;
        let unit = self
            .required(path, &fields, "unit")
            .and_then(|json| self.identity(&path.key("unit"), json));
        let base = self
            .required(path, &fields, "base")
            .and_then(|json| self.version(&path.key("base"), json));
        Some(AmendmentDeclaration::new(unit?, base?))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FieldPath, IssueKind, JsonKind, ReadManifestError, RemovedField, declare_amendment,
        read_manifest, read_project, rebase_amendments,
    };
    use crate::domain::{
        AmendmentDeclaration, IdentityErrorReason, ManifestError, PlaceholderName, RelativePath,
        Segment, UnitIdentity, Version,
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
  "placeholders": {
    "replace": {
      /* Shown in the browser's title bar. */
      "__APP_TITLE__": "The application's visible name.",
      "__WORKER_NAME__": "The Worker's name in kebab-case.",
    },
    "ignore": ["__DEV__", "__WEBPACK_DEFINE__"],
  },
  "amendments": {
    "package.json": { "unit": "@metamao/node", "base": "1.4.2" },
    "config/wrangler.toml": { "unit": "@metamao/cloudflare-worker", "base": "2.1.0" },
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

    fn placeholder(input: &str) -> PlaceholderName {
        PlaceholderName::parse(input).unwrap()
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

        let placeholders = manifest.placeholders();
        let replace: Vec<(&str, &str)> = placeholders
            .replace()
            .iter()
            .map(|(name, description)| (name.as_str(), description.as_str()))
            .collect();
        assert_eq!(
            replace,
            [
                ("__APP_TITLE__", "The application's visible name."),
                ("__WORKER_NAME__", "The Worker's name in kebab-case."),
            ]
        );
        let ignore: Vec<&str> = placeholders
            .ignore()
            .iter()
            .map(|name| name.as_str())
            .collect();
        assert_eq!(ignore, ["__DEV__", "__WEBPACK_DEFINE__"]);

        let amendments: Vec<(String, String, String)> = manifest
            .amendments()
            .iter()
            .map(|(path, declaration)| {
                (
                    path.to_string(),
                    declaration.unit().to_string(),
                    declaration.base().to_string(),
                )
            })
            .collect();
        assert_eq!(
            amendments,
            [
                (
                    "config/wrangler.toml".to_owned(),
                    "@metamao/cloudflare-worker".to_owned(),
                    "2.1.0".to_owned()
                ),
                (
                    "package.json".to_owned(),
                    "@metamao/node".to_owned(),
                    "1.4.2".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn optional_collections_default_to_empty() {
        let manifest = read_manifest(&with(r#""placeholders": {}"#)).unwrap();
        assert!(manifest.dependencies().is_empty());
        assert!(manifest.placeholders().is_empty());
        assert!(manifest.amendments().is_empty());
        let manifest = read_manifest(&with(r#""dependencies": {}"#)).unwrap();
        assert!(manifest.placeholders().is_empty());
    }

    #[test]
    fn placeholder_declarations_are_validated() {
        let text = with(
            r#""placeholders": {
                 "replace": {
                   "__project-name__": "The name.",
                   "__BLANK__": "  ",
                   "__NUMBER__": 1,
                   "__BOTH__": "Both."
                 },
                 "ignore": ["__BOTH__", "plain", 3, "__TWICE__", "__TWICE__"]
               }"#,
        );
        let found = issues(&text);
        let found: Vec<(&str, &IssueKind)> = found
            .iter()
            .map(|(path, kind)| (path.as_str(), kind))
            .collect();
        assert_eq!(found.len(), 7, "{found:#?}");
        assert!(matches!(
            found[0],
            ("placeholders.ignore", IssueKind::ReplacedAndIgnored(name)) if name.as_str() == "__BOTH__"
        ));
        assert!(matches!(
            found[1],
            ("placeholders.ignore[1]", IssueKind::InvalidPlaceholder(_))
        ));
        assert!(matches!(
            found[2],
            (
                "placeholders.ignore[2]",
                IssueKind::WrongType {
                    found: JsonKind::Integer,
                    ..
                }
            )
        ));
        assert!(matches!(
            found[3],
            ("placeholders.ignore[4]", IssueKind::DuplicateEntry)
        ));
        assert!(matches!(
            found[4],
            (
                "placeholders.replace.__BLANK__",
                IssueKind::BlankDescription(_)
            )
        ));
        assert!(matches!(
            found[5],
            (
                "placeholders.replace.__NUMBER__",
                IssueKind::WrongType {
                    found: JsonKind::Integer,
                    ..
                }
            )
        ));
        assert!(matches!(
            found[6],
            (
                "placeholders.replace[\"__project-name__\"]",
                IssueKind::InvalidPlaceholder(_)
            )
        ));
        // `values` is not a field of `placeholders`.
        let unknown = issues(&with(r#""placeholders": {"values": {}}"#));
        assert!(matches!(
            unknown.as_slice(),
            [(path, IssueKind::UnknownField(_))] if path == "placeholders.values"
        ));
    }

    #[test]
    fn an_invalid_placeholder_name_suggests_its_spelling() {
        let issues = issues(&with(
            r#""placeholders": {"replace": {"__project-name__": "The name."}}"#,
        ));
        let message = issues[0].1.to_string();
        assert!(message.contains("\"__project-name__\""), "{message}");
        assert!(message.contains("\"__PROJECT_NAME__\""), "{message}");
    }

    #[test]
    fn parameters_and_options_are_rejected_with_the_way_forward() {
        let text = with(
            r#""dependencies": {"@metamao/node": "^1.0.0"},
               "params": {"appName": {"type": "string", "description": "A."}},
               "options": {"@metamao/node": {"major": 22}}"#,
        );
        assert_eq!(
            issues(&text),
            [
                (
                    "options".to_owned(),
                    IssueKind::Removed(RemovedField::UnitOptions)
                ),
                (
                    "params".to_owned(),
                    IssueKind::Removed(RemovedField::Params)
                ),
            ]
        );
        assert!(
            IssueKind::Removed(RemovedField::Params)
                .to_string()
                .contains("\"placeholders\".\"replace\""),
        );
        assert!(
            IssueKind::Removed(RemovedField::UnitOptions)
                .to_string()
                .contains("assemblr unit amend"),
        );

        let error =
            read_project(r#"{"dependencies": {}, "options": {"@metamao/node": {}}}"#).unwrap_err();
        let ReadManifestError::Invalid(issues) = error else {
            panic!("{error:?}");
        };
        assert_eq!(
            issues[0].kind(),
            &IssueKind::Removed(RemovedField::ProjectOptions)
        );
        assert!(issues[0].kind().to_string().contains("assemblr status"));
        // A project never had `params`: it is an unknown field there.
        let error = read_project(r#"{"params": {}}"#).unwrap_err();
        let ReadManifestError::Invalid(issues) = error else {
            panic!("{error:?}");
        };
        assert!(matches!(issues[0].kind(), IssueKind::UnknownField(_)));
    }

    #[test]
    fn amendment_declarations_are_validated() {
        let text = with(
            r#""dependencies": {"@metamao/node": "^1.0.0"},
               "amendments": {
                 "package.json": {"unit": "@metamao/node", "base": "1.2.0"},
                 "../escape": {"unit": "@metamao/node", "base": "1.2.0"},
                 "no-unit.txt": {"base": "1.2.0"},
                 "range.txt": {"unit": "@metamao/node", "base": "^1.2.0"},
                 "other.txt": {"unit": "@metamao/docker", "base": "1.0.0"},
                 "extra.txt": {"unit": "@metamao/node", "base": "1.2.0", "path": "x"},
                 "list.txt": []
               }"#,
        );
        let issues = issues(&text);
        let found: Vec<(&str, &IssueKind)> = issues
            .iter()
            .map(|(path, kind)| (path.as_str(), kind))
            .collect();
        assert_eq!(found.len(), 5, "{found:#?}");
        assert!(matches!(
            found[0],
            ("amendments[\"../escape\"]", IssueKind::InvalidPath(_))
        ));
        assert!(matches!(
            found[1],
            ("amendments[\"extra.txt\"].path", IssueKind::UnknownField(_))
        ));
        assert!(matches!(
            found[2],
            (
                "amendments[\"list.txt\"]",
                IssueKind::WrongType {
                    found: JsonKind::Array,
                    ..
                }
            )
        ));
        assert!(matches!(
            found[3],
            ("amendments[\"no-unit.txt\"].unit", IssueKind::MissingField)
        ));
        assert!(matches!(
            found[4],
            (
                "amendments[\"range.txt\"].base",
                IssueKind::InvalidVersion(_)
            )
        ));
    }

    #[test]
    fn unknown_fields_are_rejected_at_every_level() {
        let text = with(r#""fragments": [], "placeholders": {"replace": {}, "extra": 1}"#);
        assert_eq!(
            issues(&text),
            [
                (
                    "fragments".to_owned(),
                    IssueKind::UnknownField(super::MANIFEST_FIELDS)
                ),
                (
                    "placeholders.extra".to_owned(),
                    IssueKind::UnknownField(super::PLACEHOLDER_FIELDS)
                ),
            ]
        );
    }

    #[test]
    fn duplicate_keys_are_rejected() {
        let text = with(
            r#""version": "2.0.0", "placeholders": {"replace": {"__A__": "A.", "__A__": "B."}}"#,
        );
        assert_eq!(
            issues(&text),
            [
                (
                    "placeholders.replace.__A__".to_owned(),
                    IssueKind::DuplicateKey
                ),
                ("version".to_owned(), IssueKind::DuplicateKey),
            ]
        );
    }

    #[test]
    fn every_problem_is_reported_in_one_pass() {
        let text = r#"{
          "name": "@metamao/App",
          "version": "one",
          "dependencies": {"@metamao/node": 1},
          "params": {"x": {"type": "string"}},
          "placeholders": {"replace": {"__X__": ""}},
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
                "params",
                "placeholders.replace.__X__",
                "version",
            ]
        );
    }

    #[test]
    fn key_order_in_the_source_does_not_matter() {
        let forward = with(
            r#""dependencies": {"@a/a": "^1", "@b/b": "^2"},
               "placeholders": {"replace": {"__A__": "A.", "__B__": ""}, "ignore": ["__Y__", "__X__"]}"#,
        );
        let backward = r#"{
          "placeholders": {"ignore": ["__X__", "__Y__"], "replace": {"__B__": "", "__A__": "A."}},
          "dependencies": {"@b/b": "^2", "@a/a": "^1"},
          "description": "An app.", "version": "1.0.0", "name": "@metamao/app"
        }"#;
        assert_eq!(read_manifest(&forward), read_manifest(backward));
        assert_eq!(issues(backward).len(), 1);

        let reordered = r#"{
          "description": "Angular SSR on Cloudflare Workers.",
          "amendments": {
            "config/wrangler.toml": { "base": "2.1.0", "unit": "@metamao/cloudflare-worker" },
            "package.json": { "base": "1.4.2", "unit": "@metamao/node" }
          },
          "placeholders": {
            "ignore": ["__WEBPACK_DEFINE__", "__DEV__"],
            "replace": {
              "__WORKER_NAME__": "The Worker's name in kebab-case.",
              "__APP_TITLE__": "The application's visible name."
            }
          },
          "dependencies": { "@metamao/node": "~1.4", "@metamao/cloudflare-worker": "^2.1.0" },
          "version": "1.0.0",
          "name": "@metamao/cloudflare-angular-ssr"
        }"#;
        assert_eq!(
            read_manifest(reordered).unwrap(),
            read_manifest(FULL).unwrap()
        );
        assert!(
            read_manifest(FULL)
                .unwrap()
                .placeholders()
                .replace()
                .contains_key(&placeholder("__APP_TITLE__"))
        );
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
    fn an_unknown_field_lists_the_fields_allowed_there() {
        assert_eq!(
            IssueKind::UnknownField(&["a", "b"]).to_string(),
            "unknown field; the fields here are `a` and `b`"
        );
        assert_eq!(
            IssueKind::UnknownField(&["a", "b", "c"]).to_string(),
            "unknown field; the fields here are `a`, `b`, and `c`"
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
    fn no_truncation_of_a_manifest_panics() {
        for end in (0..=FULL.len()).filter(|end| FULL.is_char_boundary(*end)) {
            let _ = read_manifest(&FULL[..end]);
        }
    }

    /// `text` with an amendment of `path` from `@metamao/node` 1.4.2 added.
    fn declared(text: &str, path: &str) -> Result<String, String> {
        let manifest = read_manifest(text).unwrap();
        let declaration =
            AmendmentDeclaration::new(identity("@metamao/node"), Version::parse("1.4.2").unwrap());
        let path = RelativePath::parse(path).unwrap();
        declare_amendment(text, &manifest, &path, &declaration).map_err(|error| error.to_string())
    }

    #[test]
    fn a_declaration_is_added_to_the_existing_amendments_keeping_everything_else() {
        let edited = declared(FULL, "src/index.ts").unwrap();
        let expected = FULL.replace(
            r#"    "config/wrangler.toml": { "unit": "@metamao/cloudflare-worker", "base": "2.1.0" },"#,
            "    \"config/wrangler.toml\": { \"unit\": \"@metamao/cloudflare-worker\", \"base\": \"2.1.0\" },\n    \"src/index.ts\": { \"unit\": \"@metamao/node\", \"base\": \"1.4.2\" },",
        );
        assert_eq!(edited, expected);
        assert_eq!(read_manifest(&edited).unwrap().amendments().len(), 3);
    }

    #[test]
    fn a_declaration_creates_the_amendments_object_after_the_last_member() {
        let text = "{\n  \"name\": \"@metamao/app\",\n  \"version\": \"1.0.0\",\n  // Why.\n  \"description\": \"An app.\",\n  \"dependencies\": { \"@metamao/node\": \"^1\" }, // trailing\n}\n";
        assert_eq!(
            declared(text, "a \"quoted\" name.txt").unwrap(),
            "{\n  \"name\": \"@metamao/app\",\n  \"version\": \"1.0.0\",\n  // Why.\n  \"description\": \"An app.\",\n  \"dependencies\": { \"@metamao/node\": \"^1\" },\n  \"amendments\": {\n    \"a \\\"quoted\\\" name.txt\": { \"unit\": \"@metamao/node\", \"base\": \"1.4.2\" }\n  }, // trailing\n}\n"
        );
        // Tabs are followed too.
        let text = "{\n\t\"name\": \"@metamao/app\",\n\t\"version\": \"1.0.0\",\n\t\"description\": \"An app.\",\n\t\"dependencies\": { \"@metamao/node\": \"^1\" }\n}";
        assert_eq!(
            declared(text, "a.txt").unwrap(),
            "{\n\t\"name\": \"@metamao/app\",\n\t\"version\": \"1.0.0\",\n\t\"description\": \"An app.\",\n\t\"dependencies\": { \"@metamao/node\": \"^1\" },\n\t\"amendments\": {\n\t\t\"a.txt\": { \"unit\": \"@metamao/node\", \"base\": \"1.4.2\" }\n\t}\n}"
        );
    }

    #[test]
    fn a_declaration_keeps_the_manifest_s_line_endings() {
        let text = "{\r\n  \"name\": \"@metamao/app\",\r\n  \"version\": \"1.0.0\",\r\n  \"description\": \"An app.\",\r\n  \"dependencies\": { \"@metamao/node\": \"^1\" }\r\n}\r\n";
        assert_eq!(
            declared(text, "a.txt").unwrap(),
            "{\r\n  \"name\": \"@metamao/app\",\r\n  \"version\": \"1.0.0\",\r\n  \"description\": \"An app.\",\r\n  \"dependencies\": { \"@metamao/node\": \"^1\" },\r\n  \"amendments\": {\r\n    \"a.txt\": { \"unit\": \"@metamao/node\", \"base\": \"1.4.2\" }\r\n  }\r\n}\r\n"
        );
    }

    #[test]
    fn a_declaration_fills_an_empty_amendments_object() {
        let text = "{\n  \"name\": \"@metamao/app\",\n  \"version\": \"1.0.0\",\n  \"description\": \"An app.\",\n  \"amendments\": { /* none yet */ },\n  \"dependencies\": { \"@metamao/node\": \"^1\" }\n}";
        assert_eq!(
            declared(text, "a.txt").unwrap(),
            "{\n  \"name\": \"@metamao/app\",\n  \"version\": \"1.0.0\",\n  \"description\": \"An app.\",\n  \"amendments\": {\n    \"a.txt\": { \"unit\": \"@metamao/node\", \"base\": \"1.4.2\" }\n   /* none yet */ },\n  \"dependencies\": { \"@metamao/node\": \"^1\" }\n}"
        );
    }

    #[test]
    fn a_one_line_manifest_is_edited_inline() {
        let text = with(r#""dependencies": {"@metamao/node": "^1"}"#);
        assert_eq!(
            declared(&text, "a.txt").unwrap(),
            r#"{"name": "@metamao/app", "version": "1.0.0", "description": "An app.", "dependencies": {"@metamao/node": "^1"}, "amendments": { "a.txt": { "unit": "@metamao/node", "base": "1.4.2" } }}"#
        );
        let text = with(r#""dependencies": {"@metamao/node": "^1"}, "amendments": {}"#);
        assert_eq!(
            declared(&text, "a.txt").unwrap(),
            r#"{"name": "@metamao/app", "version": "1.0.0", "description": "An app.", "dependencies": {"@metamao/node": "^1"}, "amendments": { "a.txt": { "unit": "@metamao/node", "base": "1.4.2" } }}"#
        );
        let text = with(
            r#""dependencies": {"@metamao/node": "^1"}, "amendments": {"b.txt": {"unit": "@metamao/node", "base": "1.0.0"}}"#,
        );
        assert_eq!(
            declared(&text, "a.txt").unwrap(),
            r#"{"name": "@metamao/app", "version": "1.0.0", "description": "An app.", "dependencies": {"@metamao/node": "^1"}, "amendments": {"b.txt": {"unit": "@metamao/node", "base": "1.0.0"}, "a.txt": { "unit": "@metamao/node", "base": "1.4.2" }}}"#
        );
    }

    #[test]
    fn an_edit_that_cannot_be_made_safely_names_the_member_to_add() {
        // Already declared.
        let error = declared(FULL, "config/wrangler.toml").unwrap_err();
        assert!(error.contains("unchanged"), "{error}");
        assert!(
            error.contains(
                r#""config/wrangler.toml": { "unit": "@metamao/node", "base": "1.4.2" }"#
            ),
            "{error}"
        );
        // Text that is not the manifest's own.
        let manifest = read_manifest(FULL).unwrap();
        let declaration =
            AmendmentDeclaration::new(identity("@metamao/node"), Version::parse("1.4.2").unwrap());
        let path = RelativePath::parse("a.txt").unwrap();
        assert!(declare_amendment("{}", &manifest, &path, &declaration).is_err());
        assert!(declare_amendment("[]", &manifest, &path, &declaration).is_err());
    }

    /// `text` with the bases in `bases` replaced.
    fn rebased(text: &str, bases: &[(&str, &str)]) -> Result<String, String> {
        let manifest = read_manifest(text).unwrap();
        let bases = bases
            .iter()
            .map(|(path, base)| {
                (
                    RelativePath::parse(path).unwrap(),
                    Version::parse(base).unwrap(),
                )
            })
            .collect();
        rebase_amendments(text, &manifest, &bases).map_err(|error| error.to_string())
    }

    #[test]
    fn bases_are_replaced_in_place_keeping_everything_else() {
        let edited = rebased(
            FULL,
            &[("config/wrangler.toml", "2.2.0"), ("package.json", "1.5.0")],
        )
        .unwrap();
        let expected = FULL
            .replace(r#""base": "2.1.0""#, r#""base": "2.2.0""#)
            .replace(r#""base": "1.4.2""#, r#""base": "1.5.0""#);
        assert_eq!(edited, expected);
        let manifest = read_manifest(&edited).unwrap();
        assert_eq!(
            manifest.amendments()[&RelativePath::parse("config/wrangler.toml").unwrap()].base(),
            &Version::parse("2.2.0").unwrap()
        );
        // Comments inside the declaration, and a one-line manifest.
        let text = with(
            "\"dependencies\": {\"@metamao/node\": \"^1\"}, \"amendments\": {\"a.txt\": {\"unit\": \"@metamao/node\", /* was */ \"base\": /* 1.0.0 */ \"1.0.0\"}}",
        );
        assert_eq!(
            rebased(&text, &[("a.txt", "1.2.0")]).unwrap(),
            with(
                "\"dependencies\": {\"@metamao/node\": \"^1\"}, \"amendments\": {\"a.txt\": {\"unit\": \"@metamao/node\", /* was */ \"base\": /* 1.0.0 */ \"1.2.0\"}}"
            )
        );
    }

    #[test]
    fn a_rebase_that_cannot_be_made_safely_names_the_members_to_replace() {
        // A path that is not amended.
        let error = rebased(FULL, &[("src/index.ts", "1.5.0")]).unwrap_err();
        assert!(error.contains("unchanged"), "{error}");
        // The declared members are named with their new bases.
        let text = with(
            r#""dependencies": {"@metamao/node": "^1"}, "amendments": {"a.txt": {"unit": "@metamao/node", "base": "1.0.0"}}"#,
        );
        let manifest = read_manifest(&text).unwrap();
        let bases = std::collections::BTreeMap::from([(
            RelativePath::parse("a.txt").unwrap(),
            Version::parse("1.2.0").unwrap(),
        )]);
        let error = rebase_amendments("{}", &manifest, &bases).unwrap_err();
        assert_eq!(
            error.entries(),
            [r#""a.txt": { "unit": "@metamao/node", "base": "1.2.0" }"#]
        );
        assert!(
            error.to_string().contains("replace these members"),
            "{error}"
        );
    }

    #[test]
    fn diagnostics_name_the_field_and_the_fix() {
        let error = read_manifest(&with(r#""placeholders": {"ignore": "__A__"}"#)).unwrap_err();
        let message = error.to_string();
        assert!(message.starts_with("1 problem:"), "{message}");
        assert!(
            message.contains("placeholders.ignore: expected an array, found a string"),
            "{message}"
        );
        assert_eq!(FieldPath::default().to_string(), "(manifest)");
    }
}
