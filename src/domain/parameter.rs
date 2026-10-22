//! Parameters: a unit's declared, public configuration inputs.

use std::fmt;
use std::str::FromStr;

/// The name of a parameter, such as `appName` or `node_compat`.
///
/// A name starts with an ASCII letter and continues with ASCII letters,
/// digits, or underscores. This provisional rule keeps names usable in the
/// template syntax (OC-5), and can be widened compatibly. The name `own` is
/// reserved: in an amendment copy it is the root of the amending author's
/// own template context, so no unit's parameter may be called that.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParameterName(String);

/// The reserved root of the amending author's template context.
pub const OWN: &str = "own";

impl ParameterName {
    /// Parses a parameter name. Invalid names are rejected, never normalized.
    pub fn parse(input: &str) -> Result<Self, ParameterNameError> {
        let mut chars = input.chars();
        let reason = match chars.next() {
            None => Some(ParameterNameErrorReason::Empty),
            Some(first) if !first.is_ascii_alphabetic() => {
                Some(ParameterNameErrorReason::InvalidStart(first))
            }
            Some(_) if input == OWN => Some(ParameterNameErrorReason::Reserved),
            Some(_) => chars
                .find(|c| !(c.is_ascii_alphanumeric() || *c == '_'))
                .map(ParameterNameErrorReason::InvalidCharacter),
        };
        match reason {
            None => Ok(Self(input.to_owned())),
            Some(reason) => Err(ParameterNameError {
                input: input.to_owned(),
                reason,
            }),
        }
    }

    /// The name as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for ParameterName {
    type Err = ParameterNameError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input)
    }
}

impl fmt::Display for ParameterName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A string that is not a valid parameter name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterNameError {
    input: String,
    reason: ParameterNameErrorReason,
}

impl ParameterNameError {
    /// The rejected input, exactly as given.
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Why the input was rejected.
    pub fn reason(&self) -> &ParameterNameErrorReason {
        &self.reason
    }
}

impl fmt::Display for ParameterNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid parameter name {:?}: ", self.input)?;
        match self.reason {
            ParameterNameErrorReason::Empty => f.write_str("it is empty"),
            ParameterNameErrorReason::InvalidStart(c) => {
                write!(
                    f,
                    "it starts with {c:?}; a name must start with an ASCII letter"
                )
            }
            ParameterNameErrorReason::InvalidCharacter(c) => write!(
                f,
                "it contains {c:?}; only ASCII letters, digits, and underscores are allowed"
            ),
            ParameterNameErrorReason::Reserved => write!(
                f,
                "`{OWN}` is reserved for the amending author's template context"
            ),
        }
    }
}

impl std::error::Error for ParameterNameError {}

/// The rule an invalid parameter name breaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterNameErrorReason {
    /// The name is empty.
    Empty,
    /// The first character is not an ASCII letter.
    InvalidStart(char),
    /// A later character is not an ASCII letter, digit, or underscore.
    InvalidCharacter(char),
    /// The name is `own`, the reserved root of the amending author's
    /// template context.
    Reserved,
}

/// A required human-readable description that is not empty or only
/// whitespace. The text is kept exactly as written.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// The type of a parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParameterType {
    /// Any string.
    String,
    /// A 64-bit signed integer.
    Integer,
    /// `true` or `false`.
    Boolean,
    /// A string restricted to declared allowed values.
    EnumeratedString(AllowedValues),
}

impl ParameterType {
    /// The kind of value this type accepts.
    pub fn value_kind(&self) -> ValueKind {
        match self {
            Self::String | Self::EnumeratedString(_) => ValueKind::String,
            Self::Integer => ValueKind::Integer,
            Self::Boolean => ValueKind::Boolean,
        }
    }

    /// Checks that a value has this type and, for an enumerated string, is
    /// one of the allowed values.
    pub fn check(&self, value: &ParameterValue) -> Result<(), ParameterValueError> {
        let expected = self.value_kind();
        let found = value.kind();
        if expected != found {
            return Err(ParameterValueError::TypeMismatch { expected, found });
        }
        if let (Self::EnumeratedString(allowed), ParameterValue::String(text)) = (self, value) {
            if !allowed.contains(text) {
                return Err(ParameterValueError::NotAllowed {
                    value: text.clone(),
                    allowed: allowed.clone(),
                });
            }
        }
        Ok(())
    }
}

/// The values an enumerated string accepts: at least one, with no
/// duplicates, in their declared order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedValues(Vec<String>);

impl AllowedValues {
    /// Validates a declared list of allowed values.
    pub fn new(values: Vec<String>) -> Result<Self, AllowedValuesError> {
        if values.is_empty() {
            return Err(AllowedValuesError::Empty);
        }
        let mut sorted: Vec<&String> = values.iter().collect();
        sorted.sort();
        let mut duplicates: Vec<String> = sorted
            .windows(2)
            .filter(|pair| pair[0] == pair[1])
            .map(|pair| pair[0].clone())
            .collect();
        duplicates.dedup();
        if duplicates.is_empty() {
            Ok(Self(values))
        } else {
            Err(AllowedValuesError::Duplicates(duplicates))
        }
    }

    /// Whether `value` is one of the allowed values.
    pub fn contains(&self, value: &str) -> bool {
        self.0.iter().any(|allowed| allowed == value)
    }

    /// The allowed values, in declared order.
    pub fn as_slice(&self) -> &[String] {
        &self.0
    }
}

impl fmt::Display for AllowedValues {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, value) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{value:?}")?;
        }
        Ok(())
    }
}

/// An invalid list of allowed values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllowedValuesError {
    /// The list is empty, so no value could ever be valid.
    Empty,
    /// Values listed more than once, sorted.
    Duplicates(Vec<String>),
}

impl fmt::Display for AllowedValuesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("the list of allowed values is empty"),
            Self::Duplicates(values) => {
                f.write_str("allowed values are listed more than once: ")?;
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{value:?}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for AllowedValuesError {}

/// The kind of a parameter value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    /// A string, including an enumerated string.
    String,
    /// A 64-bit signed integer.
    Integer,
    /// A boolean.
    Boolean,
}

impl fmt::Display for ValueKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::String => "a string",
            Self::Integer => "an integer",
            Self::Boolean => "a boolean",
        })
    }
}

/// A value supplied for a parameter, or a parameter's default.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ParameterValue {
    /// A string, including a value of an enumerated string.
    String(String),
    /// A 64-bit signed integer.
    Integer(i64),
    /// A boolean.
    Boolean(bool),
}

impl ParameterValue {
    /// The kind of this value.
    pub fn kind(&self) -> ValueKind {
        match self {
            Self::String(_) => ValueKind::String,
            Self::Integer(_) => ValueKind::Integer,
            Self::Boolean(_) => ValueKind::Boolean,
        }
    }
}

impl fmt::Display for ParameterValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(text) => write!(f, "{text:?}"),
            Self::Integer(number) => write!(f, "{number}"),
            Self::Boolean(flag) => write!(f, "{flag}"),
        }
    }
}

/// A value that does not satisfy a parameter's type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParameterValueError {
    /// The value has the wrong kind.
    TypeMismatch {
        /// The kind the parameter accepts.
        expected: ValueKind,
        /// The kind that was supplied.
        found: ValueKind,
    },
    /// The string is not one of the enumerated string's allowed values.
    NotAllowed {
        /// The supplied string.
        value: String,
        /// The values the parameter accepts.
        allowed: AllowedValues,
    },
}

impl fmt::Display for ParameterValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TypeMismatch { expected, found } => {
                write!(f, "expected {expected}, found {found}")
            }
            Self::NotAllowed { value, allowed } => {
                write!(f, "{value:?} is not one of the allowed values: {allowed}")
            }
        }
    }
}

impl std::error::Error for ParameterValueError {}

/// A parameter's declaration: its type, description, and optional default.
///
/// A parameter without a default is required. One with a default is
/// optional, and the default applies when no value is supplied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParameterDefinition {
    parameter_type: ParameterType,
    description: Description,
    default: Option<ParameterValue>,
    example: Option<ParameterValue>,
}

impl ParameterDefinition {
    /// Creates a definition, rejecting a default that does not satisfy the
    /// parameter's type.
    pub fn new(
        parameter_type: ParameterType,
        description: Description,
        default: Option<ParameterValue>,
    ) -> Result<Self, ParameterValueError> {
        if let Some(value) = &default {
            parameter_type.check(value)?;
        }
        Ok(Self {
            parameter_type,
            description,
            default,
            example: None,
        })
    }

    /// The same definition with an authoring-time example, rejecting one that
    /// does not satisfy the parameter's type and allowed values.
    ///
    /// An example is not a default: it never makes the parameter optional and
    /// never supplies a value to a composition. It is the value a unit's own
    /// build, check, and publication use when the parameter is required and
    /// has no default.
    pub fn with_example(mut self, example: ParameterValue) -> Result<Self, ParameterValueError> {
        self.parameter_type.check(&example)?;
        self.example = Some(example);
        Ok(self)
    }

    /// The authoring-time example, if the parameter has one.
    pub fn example(&self) -> Option<&ParameterValue> {
        self.example.as_ref()
    }

    /// The parameter's type.
    pub fn parameter_type(&self) -> &ParameterType {
        &self.parameter_type
    }

    /// The parameter's description.
    pub fn description(&self) -> &Description {
        &self.description
    }

    /// The default value, if the parameter is optional.
    pub fn default(&self) -> Option<&ParameterValue> {
        self.default.as_ref()
    }

    /// Whether a value must be supplied, which is the case exactly when the
    /// parameter has no default.
    pub fn is_required(&self) -> bool {
        self.default.is_none()
    }

    /// Checks a supplied value against this parameter's type.
    pub fn check(&self, value: &ParameterValue) -> Result<(), ParameterValueError> {
        self.parameter_type.check(value)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AllowedValues, AllowedValuesError, BlankDescription, Description, ParameterDefinition,
        ParameterName, ParameterNameErrorReason, ParameterType, ParameterValue,
        ParameterValueError, ValueKind,
    };

    fn allowed(values: &[&str]) -> AllowedValues {
        AllowedValues::new(values.iter().map(|value| (*value).to_owned()).collect()).unwrap()
    }

    fn description() -> Description {
        Description::new("A parameter.").unwrap()
    }

    #[test]
    fn parameter_names_are_identifiers() {
        for input in [
            "appName",
            "node_compat",
            "a",
            "port8080",
            "A",
            "owner",
            "Own",
        ] {
            assert_eq!(ParameterName::parse(input).unwrap().as_str(), input);
        }
        for (input, reason) in [
            ("", ParameterNameErrorReason::Empty),
            ("8080port", ParameterNameErrorReason::InvalidStart('8')),
            ("_private", ParameterNameErrorReason::InvalidStart('_')),
            ("app-name", ParameterNameErrorReason::InvalidCharacter('-')),
            ("app.name", ParameterNameErrorReason::InvalidCharacter('.')),
            ("app name", ParameterNameErrorReason::InvalidCharacter(' ')),
            ("appNäme", ParameterNameErrorReason::InvalidCharacter('ä')),
            // Reserved for the amending author's template context. Other
            // spellings are ordinary names.
            ("own", ParameterNameErrorReason::Reserved),
        ] {
            let error = ParameterName::parse(input).unwrap_err();
            assert_eq!(error.reason(), &reason, "input {input:?}");
            assert_eq!(error.input(), input);
        }
    }

    #[test]
    fn descriptions_must_not_be_blank_and_are_kept_verbatim() {
        assert_eq!(Description::new(" Text. ").unwrap().as_str(), " Text. ");
        assert_eq!(Description::new(""), Err(BlankDescription));
        assert_eq!(Description::new(" \t\n"), Err(BlankDescription));
    }

    #[test]
    fn allowed_values_are_non_empty_and_unique() {
        assert_eq!(allowed(&["npm", "pnpm"]).as_slice(), ["npm", "pnpm"]);
        assert_eq!(
            AllowedValues::new(Vec::new()),
            Err(AllowedValuesError::Empty)
        );
        let duplicates = ["b", "a", "b", "a", "c", "b"].map(str::to_owned).to_vec();
        assert_eq!(
            AllowedValues::new(duplicates),
            Err(AllowedValuesError::Duplicates(vec![
                "a".to_owned(),
                "b".to_owned()
            ]))
        );
    }

    #[test]
    fn requiredness_follows_from_the_default() {
        let required =
            ParameterDefinition::new(ParameterType::Boolean, description(), None).unwrap();
        assert!(required.is_required());
        let optional = ParameterDefinition::new(
            ParameterType::Boolean,
            description(),
            Some(ParameterValue::Boolean(false)),
        )
        .unwrap();
        assert!(!optional.is_required());
        assert_eq!(optional.default(), Some(&ParameterValue::Boolean(false)));
    }

    #[test]
    fn defaults_must_satisfy_the_type() {
        let error = ParameterDefinition::new(
            ParameterType::Integer,
            description(),
            Some(ParameterValue::String("8080".to_owned())),
        )
        .unwrap_err();
        assert_eq!(
            error,
            ParameterValueError::TypeMismatch {
                expected: ValueKind::Integer,
                found: ValueKind::String,
            }
        );
        assert_eq!(error.to_string(), "expected an integer, found a string");
    }

    #[test]
    fn enumerated_strings_accept_only_allowed_values() {
        let parameter_type = ParameterType::EnumeratedString(allowed(&["npm", "pnpm"]));
        assert_eq!(
            parameter_type.check(&ParameterValue::String("pnpm".to_owned())),
            Ok(())
        );
        let error = parameter_type
            .check(&ParameterValue::String("yarn".to_owned()))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "\"yarn\" is not one of the allowed values: \"npm\", \"pnpm\""
        );
        assert!(matches!(
            parameter_type.check(&ParameterValue::Boolean(true)),
            Err(ParameterValueError::TypeMismatch {
                expected: ValueKind::String,
                found: ValueKind::Boolean,
            })
        ));
    }

    #[test]
    fn plain_types_accept_any_value_of_their_kind() {
        for (parameter_type, value) in [
            (ParameterType::String, ParameterValue::String(String::new())),
            (ParameterType::Integer, ParameterValue::Integer(i64::MIN)),
            (ParameterType::Boolean, ParameterValue::Boolean(true)),
        ] {
            assert_eq!(parameter_type.check(&value), Ok(()));
        }
    }
}
