//! The domain layer: pure concepts, invariants, and validation.
//!
//! Domain code takes data and returns data. It never touches the filesystem,
//! processes, the environment, the network, or clocks (see
//! `docs/adr/0001-architecture.md`).

mod composition;
mod identity;
mod lock;
mod manifest;
mod parameter;
mod path;
mod provenance;
mod registry;
mod resolution;
mod version;

pub use composition::{
    ComposedFile, Composition, CompositionError, ContributedPath, FileTree, OptionRequirement,
    ParameterValues, Root, RootTargetReason, TEMPLATE_SUFFIX, UnitContent, UnitFile, compose,
    delivered_path,
};
pub use identity::{IdentityError, IdentityErrorReason, Segment, UnitIdentity};
pub use lock::{LockError, LockedUnit, UnitLock, relock};
pub use manifest::{Manifest, ManifestError, OptionValues};
pub use parameter::{
    AllowedValues, AllowedValuesError, BlankDescription, Description, OWN, ParameterDefinition,
    ParameterName, ParameterNameError, ParameterNameErrorReason, ParameterType, ParameterValue,
    ParameterValueError, ValueKind,
};
pub use path::{RelativePath, RelativePathError, RelativePathErrorReason};
pub use provenance::{ObjectId, Provenance};
pub use registry::{
    Change, Excluded, IntegrityChange, InvalidIndex, ObservedTag, ObservedVersion, RegisteredUnit,
    RegistryError, RegistryIndex, SourceReference, declared_identity,
};
pub use resolution::{
    AvailableVersions, Dependencies, Lock, LockChange, Requirement, Requirer, Resolution,
    ResolutionError, ResolvedGraph, ResolvedUnit, resolve,
};
pub use version::{RequirementError, Version, VersionError, VersionRequirement};

pub(crate) use manifest::relation_errors;
