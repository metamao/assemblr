//! What the registry knows about one unit: the index's entry and the
//! manifest of a published version, read at the commit the index records.
//!
//! Nothing is fetched and nothing is written: the repository is read through
//! [`GitRepository`] as for a build, and a version whose content no longer
//! matches the index is reported, never described from what it holds now.

use std::fmt;

use super::{GitError, InvalidVersion, Registry, StorageError};
use crate::domain::{
    Manifest, Provenance, RegisteredUnit, SearchQuery, SourceReference, UnitIdentity, Version,
};

/// A registered unit and one of its published versions.
#[derive(Debug, Clone)]
pub struct UnitInfo {
    identity: UnitIdentity,
    source: SourceReference,
    versions: Vec<Version>,
    provenance: Provenance,
    manifest: Manifest,
}

impl UnitInfo {
    /// The unit's identity.
    pub fn identity(&self) -> &UnitIdentity {
        &self.identity
    }

    /// Where the unit is registered from.
    pub fn source(&self) -> &SourceReference {
        &self.source
    }

    /// Every indexed version, in SemVer order.
    pub fn versions(&self) -> &[Version] {
        &self.versions
    }

    /// The version described.
    pub fn version(&self) -> &Version {
        self.manifest.version()
    }

    /// The described version's commit and content identity.
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    /// The manifest published with the described version.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
}

/// Why a unit could not be described.
#[derive(Debug)]
pub enum InfoFailure {
    /// The index could not be read.
    Storage(StorageError),
    /// No registered source provides the unit.
    NotRegistered(UnitIdentity),
    /// The unit is registered but has no published version.
    NoVersions {
        identity: UnitIdentity,
        source: SourceReference,
    },
    /// The unit has no indexed version `version`.
    UnknownVersion {
        identity: UnitIdentity,
        version: Version,
        available: Vec<Version>,
    },
    /// The unit's repository could not be read.
    Source {
        identity: UnitIdentity,
        error: Box<GitError>,
    },
    /// The version's commit no longer holds a valid manifest.
    Invalid {
        identity: UnitIdentity,
        version: Version,
        error: Box<InvalidVersion>,
    },
    /// The version's content is not what the index recorded.
    NotIndexed {
        identity: UnitIdentity,
        version: Version,
    },
}

impl fmt::Display for InfoFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(error) => error.fmt(f),
            Self::NotRegistered(identity) => {
                write!(f, "no registered source provides {identity}")
            }
            Self::NoVersions { identity, source } => write!(
                f,
                "{identity}, registered from {source}, has no published version to describe"
            ),
            Self::UnknownVersion {
                identity,
                version,
                available,
            } => {
                write!(f, "{identity} has no indexed version {version}; it has ")?;
                let list: Vec<String> = available.iter().map(ToString::to_string).collect();
                f.write_str(&list.join(", "))
            }
            Self::Source { identity, error } => {
                write!(f, "the repository of {identity} cannot be read: {error}")
            }
            Self::Invalid {
                identity,
                version,
                error,
            } => write!(
                f,
                "{identity} {version} no longer has a valid manifest at its indexed commit: \
                 {error}"
            ),
            Self::NotIndexed { identity, version } => write!(
                f,
                "{identity} {version} does not match what the index recorded for it: its \
                 content or manifest changed after it was indexed"
            ),
        }
    }
}

impl std::error::Error for InfoFailure {}

/// The units a search matched, and the units it could not describe.
#[derive(Debug)]
pub struct Search {
    found: Vec<Found>,
    undescribed: Vec<InfoFailure>,
}

impl Search {
    /// The matched units, in identity order.
    pub fn found(&self) -> &[Found] {
        &self.found
    }

    /// Why units with a published version could not be described, in
    /// identity order. They were matched on their identity alone, so a
    /// match on their description may be missing.
    pub fn undescribed(&self) -> &[InfoFailure] {
        &self.undescribed
    }
}

/// A unit a search matched.
#[derive(Debug, Clone)]
pub struct Found {
    identity: UnitIdentity,
    source: SourceReference,
    has_versions: bool,
    info: Option<UnitInfo>,
}

impl Found {
    /// The unit's identity.
    pub fn identity(&self) -> &UnitIdentity {
        &self.identity
    }

    /// Where the unit is registered from.
    pub fn source(&self) -> &SourceReference {
        &self.source
    }

    /// Whether the unit has a published version.
    pub fn has_versions(&self) -> bool {
        self.has_versions
    }

    /// The unit's newest stable version, unless it has no published version
    /// or could not be described.
    pub fn info(&self) -> Option<&UnitInfo> {
        self.info.as_ref()
    }
}

impl Registry {
    /// Describes `identity` at `version`, or at its newest stable version
    /// (its newest one when every version is a pre-release). The manifest is
    /// read from the commit the index records.
    pub fn info(
        &self,
        identity: &UnitIdentity,
        version: Option<&Version>,
    ) -> Result<UnitInfo, InfoFailure> {
        let index = self.index().map_err(InfoFailure::Storage)?;
        let unit = index
            .units()
            .get(identity)
            .ok_or_else(|| InfoFailure::NotRegistered(identity.clone()))?;
        describe(self, identity, unit, version)
    }

    /// The registered units that `query` matches, in identity order, each
    /// with its newest stable version described as [`info`](Self::info)
    /// describes it. A unit that cannot be described is matched on its
    /// identity alone, and its failure is kept beside it.
    pub fn search(&self, query: &SearchQuery) -> Result<Search, StorageError> {
        let index = self.index()?;
        let mut found = Vec::new();
        let mut undescribed = Vec::new();
        for (identity, unit) in index.units() {
            let described = describe(self, identity, unit, None);
            let description = described
                .as_ref()
                .ok()
                .map(|info| info.manifest().description().as_str());
            if query.matches(identity, description) {
                found.push(Found {
                    identity: identity.clone(),
                    source: unit.source().clone(),
                    has_versions: !unit.versions().is_empty(),
                    info: described.as_ref().ok().cloned(),
                });
            }
            match described {
                Ok(_) | Err(InfoFailure::NoVersions { .. }) => {}
                Err(failure) => undescribed.push(failure),
            }
        }
        Ok(Search { found, undescribed })
    }
}

/// Describes the registered `unit` at `version`, or at its newest stable
/// version.
fn describe(
    registry: &Registry,
    identity: &UnitIdentity,
    unit: &RegisteredUnit,
    version: Option<&Version>,
) -> Result<UnitInfo, InfoFailure> {
    let versions: Vec<Version> = unit.versions().keys().cloned().collect();
    let chosen = match version {
        Some(version) => versions
            .iter()
            .find(|known| *known == version)
            .ok_or_else(|| InfoFailure::UnknownVersion {
                identity: identity.clone(),
                version: version.clone(),
                available: versions.clone(),
            })?,
        None => versions
            .iter()
            .rev()
            .find(|known| !known.is_prerelease())
            .or(versions.last())
            .ok_or_else(|| InfoFailure::NoVersions {
                identity: identity.clone(),
                source: unit.source().clone(),
            })?,
    };
    let provenance = unit.versions()[chosen].clone();
    let source_error = |error| InfoFailure::Source {
        identity: identity.clone(),
        error: Box::new(error),
    };
    let repository = registry.open_source(unit.source()).map_err(source_error)?;
    let outcome = repository
        .read_published(&[provenance.commit()])
        .map_err(source_error)?
        .pop();
    let published = match outcome {
        Some(Ok(published)) => published,
        Some(Err(error)) => {
            return Err(InfoFailure::Invalid {
                identity: identity.clone(),
                version: chosen.clone(),
                error: Box::new(error),
            });
        }
        None => {
            return Err(InfoFailure::NotIndexed {
                identity: identity.clone(),
                version: chosen.clone(),
            });
        }
    };
    let manifest = published.manifest();
    if manifest.name() != identity
        || manifest.version() != chosen
        || *published.provenance() != provenance
    {
        return Err(InfoFailure::NotIndexed {
            identity: identity.clone(),
            version: chosen.clone(),
        });
    }
    Ok(UnitInfo {
        identity: identity.clone(),
        source: unit.source().clone(),
        versions,
        provenance,
        manifest: manifest.clone(),
    })
}
