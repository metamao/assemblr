//! Exact unit versions and dependency version requirements.
//!
//! Both follow SemVer. Requirements use Cargo's requirement semantics, the
//! provisional Alpha default while the range dialect is open (OC-15). The
//! `semver` crate stays an implementation detail so the dialect can change
//! without changing this API.

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

/// An exact SemVer version, such as `1.2.3` or `2.0.0-beta.1`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(semver::Version);

impl Version {
    /// Parses an exact SemVer version. Surrounding whitespace and a `v`
    /// prefix are rejected, not stripped.
    pub fn parse(input: &str) -> Result<Self, VersionError> {
        semver::Version::parse(input)
            .map(Self)
            .map_err(|error| VersionError {
                input: input.to_owned(),
                reason: error.to_string(),
            })
    }

    /// Compares SemVer precedence, which ignores build metadata:
    /// `1.0.0+a` and `1.0.0+b` have equal precedence although they are
    /// different versions.
    pub fn cmp_precedence(&self, other: &Self) -> Ordering {
        self.0.cmp_precedence(&other.0)
    }

    /// Whether this is a pre-release such as `2.0.0-beta.1`.
    pub fn is_prerelease(&self) -> bool {
        !self.0.pre.is_empty()
    }

    /// The version a Git tag publishes. Only tags of the form `v<version>`,
    /// with an exact SemVer version, publish one: `v1.2.3` publishes `1.2.3`,
    /// while `1.2.3`, `v1.0`, and `latest` publish nothing.
    pub fn from_tag(tag: &str) -> Option<Self> {
        Self::parse(tag.strip_prefix('v')?).ok()
    }
}

impl FromStr for Version {
    type Err = VersionError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A string that is not an exact SemVer version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionError {
    input: String,
    reason: String,
}

impl VersionError {
    /// The rejected input, exactly as given.
    pub fn input(&self) -> &str {
        &self.input
    }
}

impl fmt::Display for VersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid version {:?}: {}; expected an exact SemVer version such as \"1.2.3\"",
            self.input, self.reason
        )
    }
}

impl std::error::Error for VersionError {}

/// A SemVer requirement on a dependency's version, such as `^2.1.0` or
/// `>=1.2, <1.5`.
///
/// A bare version such as `1.2.3` means `^1.2.3`, as in Cargo.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VersionRequirement(semver::VersionReq);

impl VersionRequirement {
    /// Parses a requirement using Cargo's requirement syntax.
    pub fn parse(input: &str) -> Result<Self, RequirementError> {
        semver::VersionReq::parse(input)
            .map(Self)
            .map_err(|error| RequirementError {
                input: input.to_owned(),
                reason: error.to_string(),
            })
    }

    /// Whether `version` satisfies this requirement.
    ///
    /// As in Cargo, a pre-release satisfies a requirement only when one of
    /// its comparators names a pre-release of the same `major.minor.patch`,
    /// so `^1.0.0` and `*` never match `1.1.0-beta.1`. Build metadata is
    /// ignored when matching.
    pub fn matches(&self, version: &Version) -> bool {
        self.0.matches(&version.0)
    }
}

impl FromStr for VersionRequirement {
    type Err = RequirementError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input)
    }
}

impl fmt::Display for VersionRequirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A string that is not a valid version requirement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequirementError {
    input: String,
    reason: String,
}

impl RequirementError {
    /// The rejected input, exactly as given.
    pub fn input(&self) -> &str {
        &self.input
    }
}

impl fmt::Display for RequirementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid version requirement {:?}: {}; expected a SemVer requirement such as \"^1.2.0\"",
            self.input, self.reason
        )
    }
}

impl std::error::Error for RequirementError {}

#[cfg(test)]
mod tests {
    use super::{Version, VersionRequirement};

    #[test]
    fn exact_versions_parse_and_round_trip() {
        for input in ["0.1.0", "1.2.3", "2.0.0-beta.1", "1.0.0+build.5"] {
            assert_eq!(Version::parse(input).unwrap().to_string(), input);
        }
    }

    #[test]
    fn versions_that_are_not_exact_semver_are_rejected() {
        for input in [
            "", "1", "1.0", "v1.0.0", " 1.0.0", "1.0.0 ", "01.0.0", "^1.0.0",
        ] {
            let error = Version::parse(input).unwrap_err();
            assert_eq!(error.input(), input);
            assert!(
                error.to_string().contains("exact SemVer version"),
                "{error}"
            );
        }
    }

    #[test]
    fn version_tags_publish_their_exact_version() {
        for (tag, version) in [
            ("v1.2.3", "1.2.3"),
            ("v2.0.0-beta.1", "2.0.0-beta.1"),
            ("v1.0.0+build.5", "1.0.0+build.5"),
        ] {
            assert_eq!(
                Version::from_tag(tag),
                Some(Version::parse(version).unwrap())
            );
        }
    }

    #[test]
    fn other_tags_publish_no_version() {
        for tag in [
            "",
            "v",
            "1.2.3",
            "v1.0",
            "V1.0.0",
            "vv1.0.0",
            "v 1.0.0",
            "v1.0.0 ",
            "v01.0.0",
            "release-1",
            "latest",
            "nested/v1.0.0",
        ] {
            assert_eq!(Version::from_tag(tag), None, "{tag:?}");
        }
    }

    #[test]
    fn versions_order_by_semver_precedence() {
        let mut versions: Vec<Version> = ["1.10.0", "1.2.0", "1.2.0-beta.1", "0.9.9"]
            .iter()
            .map(|input| input.parse().unwrap())
            .collect();
        versions.sort();
        let ordered: Vec<String> = versions.iter().map(ToString::to_string).collect();
        assert_eq!(ordered, ["0.9.9", "1.2.0-beta.1", "1.2.0", "1.10.0"]);
    }

    #[test]
    fn precedence_ignores_build_metadata() {
        let a = Version::parse("1.0.0+a").unwrap();
        let b = Version::parse("1.0.0+b").unwrap();
        assert_ne!(a, b);
        assert_eq!(a.cmp_precedence(&b), std::cmp::Ordering::Equal);
        let beta = Version::parse("1.0.0-beta.1").unwrap();
        assert_eq!(beta.cmp_precedence(&a), std::cmp::Ordering::Less);
    }

    #[test]
    fn requirements_use_cargo_syntax() {
        for (input, canonical) in [
            ("^2.1.0", "^2.1.0"),
            ("1.2.3", "^1.2.3"),
            ("~1", "~1"),
            ("=1.0.0", "=1.0.0"),
            (">=1.2, <1.5", ">=1.2, <1.5"),
            ("*", "*"),
        ] {
            let requirement = VersionRequirement::parse(input).unwrap();
            assert_eq!(requirement.to_string(), canonical, "input {input:?}");
        }
    }

    #[test]
    fn pre_releases_match_only_requirements_that_name_them() {
        let matches = |requirement: &str, version: &str| {
            VersionRequirement::parse(requirement)
                .unwrap()
                .matches(&Version::parse(version).unwrap())
        };
        assert!(matches("^1.0.0", "1.2.0"));
        assert!(!matches("^1.0.0", "1.1.0-beta.1"));
        assert!(!matches("*", "1.1.0-beta.1"));
        assert!(matches("^1.1.0-beta", "1.1.0-beta.2"));
        assert!(matches("^1.1.0-beta", "1.2.0"));
        assert!(!matches("^1.1.0-beta", "1.2.0-beta.1"));
        assert!(matches("=1.1.0-beta.1", "1.1.0-beta.1"));
        assert!(matches("^1.0.0", "1.0.0+build.5"));
    }

    #[test]
    fn malformed_requirements_are_rejected() {
        for input in ["", "v1.0.0", "^", "1.0.0.0", "latest"] {
            let error = VersionRequirement::parse(input).unwrap_err();
            assert_eq!(error.input(), input);
            assert!(error.to_string().contains("SemVer requirement"), "{error}");
        }
    }
}
