//! The domain layer: pure concepts, invariants, and validation.
//!
//! Domain code takes data and returns data. It never touches the filesystem,
//! processes, the environment, the network, or clocks (see
//! `docs/adr/0001-architecture.md`).

mod identity;
mod manifest;
mod parameter;
mod resolution;
mod version;

pub use identity::{IdentityError, IdentityErrorReason, Segment, UnitIdentity};
pub use manifest::{Manifest, ManifestError, OptionValues};
pub use parameter::{
    AllowedValues, AllowedValuesError, BlankDescription, Description, ParameterDefinition,
    ParameterName, ParameterNameError, ParameterNameErrorReason, ParameterType, ParameterValue,
    ParameterValueError, ValueKind,
};
pub use resolution::{
    AvailableVersions, Dependencies, Lock, LockChange, Requirement, Requirer, Resolution,
    ResolutionError, ResolvedGraph, ResolvedUnit, resolve,
};
pub use version::{RequirementError, Version, VersionError, VersionRequirement};

pub(crate) use manifest::relation_errors;
