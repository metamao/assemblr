//! Unit identities of the form `@owner/name`.

use std::fmt;
use std::str::FromStr;

/// The identity of a unit: `@owner/unit-name`.
///
/// The owner and the unit name are each a non-empty segment of lowercase
/// ASCII letters, digits, and hyphens that begins and ends with a letter or
/// digit and contains no consecutive hyphens. Owner and name together
/// determine identity. A malformed identity is rejected, never normalized.
///
/// Identities order by owner, then by unit name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnitIdentity {
    owner: String,
    name: String,
}

impl UnitIdentity {
    /// Parses an identity, rejecting anything that is not exactly
    /// `@owner/unit-name`.
    pub fn parse(input: &str) -> Result<Self, IdentityError> {
        let error = |reason| IdentityError {
            input: input.to_owned(),
            reason,
        };
        let rest = input
            .strip_prefix('@')
            .ok_or_else(|| error(IdentityErrorReason::MissingAtSign))?;
        let (owner, name) = rest
            .split_once('/')
            .ok_or_else(|| error(IdentityErrorReason::MissingSlash))?;
        check_segment(owner, Segment::Owner).map_err(error)?;
        check_segment(name, Segment::UnitName).map_err(error)?;
        Ok(Self {
            owner: owner.to_owned(),
            name: name.to_owned(),
        })
    }

    /// The owner namespace, without the leading `@`.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// The unit name, without the owner.
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl FromStr for UnitIdentity {
    type Err = IdentityError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input)
    }
}

impl fmt::Display for UnitIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}/{}", self.owner, self.name)
    }
}

fn check_segment(segment: &str, which: Segment) -> Result<(), IdentityErrorReason> {
    if segment.is_empty() {
        return Err(IdentityErrorReason::EmptySegment(which));
    }
    if let Some(character) = segment
        .chars()
        .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-'))
    {
        return Err(IdentityErrorReason::InvalidCharacter {
            segment: which,
            character,
        });
    }
    if segment.starts_with('-') {
        return Err(IdentityErrorReason::LeadingHyphen(which));
    }
    if segment.ends_with('-') {
        return Err(IdentityErrorReason::TrailingHyphen(which));
    }
    if segment.contains("--") {
        return Err(IdentityErrorReason::ConsecutiveHyphens(which));
    }
    Ok(())
}

/// A string that is not a valid unit identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityError {
    input: String,
    reason: IdentityErrorReason,
}

impl IdentityError {
    /// The rejected input, exactly as given.
    pub fn input(&self) -> &str {
        &self.input
    }

    /// Why the input was rejected.
    pub fn reason(&self) -> &IdentityErrorReason {
        &self.reason
    }
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid unit {:?}: {}", self.input, self.reason)
    }
}

impl std::error::Error for IdentityError {}

/// The part of an identity that a validation rule applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Segment {
    /// The owner namespace, between `@` and `/`.
    Owner,
    /// The unit name, after `/`.
    UnitName,
}

impl fmt::Display for Segment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Owner => "owner",
            Self::UnitName => "unit name",
        })
    }
}

/// The rule an invalid identity breaks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityErrorReason {
    /// The identity does not start with `@`.
    MissingAtSign,
    /// No `/` separates the owner from the unit name.
    MissingSlash,
    /// The owner or the unit name is empty.
    EmptySegment(Segment),
    /// A character other than a lowercase ASCII letter, digit, or hyphen.
    InvalidCharacter {
        /// Where the character appears.
        segment: Segment,
        /// The first offending character.
        character: char,
    },
    /// The segment starts with a hyphen.
    LeadingHyphen(Segment),
    /// The segment ends with a hyphen.
    TrailingHyphen(Segment),
    /// The segment contains `--`.
    ConsecutiveHyphens(Segment),
}

impl fmt::Display for IdentityErrorReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingAtSign => f.write_str("it must start with `@`, like `@acme/web`"),
            Self::MissingSlash => {
                f.write_str("it must separate the owner and the name with `/`, like `@acme/web`")
            }
            Self::EmptySegment(segment) => write!(f, "the {segment} is empty"),
            Self::InvalidCharacter { segment, character } => write!(
                f,
                "the {segment} contains {character:?}; only lowercase ASCII letters, digits, and hyphens are allowed"
            ),
            Self::LeadingHyphen(segment) => write!(f, "the {segment} starts with a hyphen"),
            Self::TrailingHyphen(segment) => write!(f, "the {segment} ends with a hyphen"),
            Self::ConsecutiveHyphens(segment) => {
                write!(f, "the {segment} contains consecutive hyphens")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{IdentityErrorReason, Segment, UnitIdentity};

    #[test]
    fn valid_identities_keep_their_segments() {
        for (input, owner, name) in [
            ("@metamao/node", "metamao", "node"),
            (
                "@metamao/cloudflare-angular-ssr",
                "metamao",
                "cloudflare-angular-ssr",
            ),
            ("@alice/d1-drizzle", "alice", "d1-drizzle"),
            ("@a/b", "a", "b"),
            ("@0/9", "0", "9"),
        ] {
            let identity = UnitIdentity::parse(input).unwrap();
            assert_eq!(identity.owner(), owner);
            assert_eq!(identity.name(), name);
            assert_eq!(identity.to_string(), input);
        }
    }

    #[test]
    fn malformed_identities_are_rejected_and_never_normalized() {
        use IdentityErrorReason::*;
        use Segment::*;

        let cases = [
            ("metamao/node", MissingAtSign),
            ("", MissingAtSign),
            (" @metamao/node", MissingAtSign),
            ("@metamao", MissingSlash),
            ("@/node", EmptySegment(Owner)),
            ("@metamao/", EmptySegment(UnitName)),
            (
                "@Metamao/node",
                InvalidCharacter {
                    segment: Owner,
                    character: 'M',
                },
            ),
            (
                "@metamao/Node",
                InvalidCharacter {
                    segment: UnitName,
                    character: 'N',
                },
            ),
            (
                "@metamao/node ",
                InvalidCharacter {
                    segment: UnitName,
                    character: ' ',
                },
            ),
            (
                "@metamao/cloud/worker",
                InvalidCharacter {
                    segment: UnitName,
                    character: '/',
                },
            ),
            (
                "@metamao/cloud_worker",
                InvalidCharacter {
                    segment: UnitName,
                    character: '_',
                },
            ),
            (
                "@metamao/nöde",
                InvalidCharacter {
                    segment: UnitName,
                    character: 'ö',
                },
            ),
            ("@-metamao/node", LeadingHyphen(Owner)),
            ("@metamao/-node", LeadingHyphen(UnitName)),
            ("@metamao-/node", TrailingHyphen(Owner)),
            ("@metamao/node-", TrailingHyphen(UnitName)),
            ("@meta--mao/node", ConsecutiveHyphens(Owner)),
            ("@metamao/cloud--worker", ConsecutiveHyphens(UnitName)),
        ];
        for (input, reason) in cases {
            let error = UnitIdentity::parse(input).unwrap_err();
            assert_eq!(error.reason(), &reason, "input {input:?}");
            assert_eq!(error.input(), input);
        }
    }

    #[test]
    fn errors_explain_the_expected_form() {
        let message = UnitIdentity::parse("@Metamao/node")
            .unwrap_err()
            .to_string();
        assert!(message.contains("\"@Metamao/node\""), "{message}");
        assert!(message.contains("owner contains 'M'"), "{message}");
        assert!(message.contains("lowercase ASCII letters"), "{message}");
    }

    #[test]
    fn identities_order_by_owner_then_name() {
        let mut identities: Vec<UnitIdentity> = ["@a-b/x", "@a/y", "@a/x", "@b/a"]
            .iter()
            .map(|input| input.parse().unwrap())
            .collect();
        identities.sort();
        let ordered: Vec<String> = identities.iter().map(ToString::to_string).collect();
        assert_eq!(ordered, ["@a/x", "@a/y", "@a-b/x", "@b/a"]);
    }
}
