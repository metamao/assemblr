//! The domain layer: pure concepts, invariants, and validation.
//!
//! Domain code takes data and returns data. It never touches the filesystem,
//! processes, the environment, the network, or clocks (see
//! `docs/adr/0001-architecture.md`).

mod composition;
mod contribution;
mod discovery;
mod identity;
mod lock;
mod manifest;
mod merge;
mod parameter;
mod path;
mod project;
mod provenance;
mod reconcile;
mod registry;
mod resolution;
mod snapshot;
mod text_merge;
mod update;
mod version;

pub use composition::{
    Amendment, ComposedFile, Composition, CompositionError, ContributedPath, FileTree,
    OptionRequirement, ParameterValues, Root, RootTargetReason, TEMPLATE_SUFFIX, UnitContent,
    UnitFile, compose, delivered_path,
};
pub use contribution::{ContributionError, contribution_problems};
pub use discovery::{EmptyQuery, SearchQuery};
pub use identity::{CatalogIdentity, IdentityError, IdentityErrorReason, Segment, UnitIdentity};
pub use lock::{
    AmendmentBaseError, BaseIntegrityError, LockError, LockedUnit, PublishedAmendmentError,
    UnitLock, amendment_bases, published_amendments, relock, verify_bases,
};
pub use manifest::{
    AmendmentDeclaration, Manifest, ManifestError, OptionValue, OptionValues, UnitOptionValues,
};
pub use merge::{
    Combined, Conflict, ConflictKind, Derived, Edit, MergeFormat, Operation, Plan, Value,
    ValueChange, combine, derive, plan, show_path, write_json_string,
};
pub use parameter::{
    AllowedValues, AllowedValuesError, BlankDescription, Description, OWN, ParameterDefinition,
    ParameterName, ParameterNameError, ParameterNameErrorReason, ParameterType, ParameterValue,
    ParameterValueError, ValueKind,
};
pub use path::{RelativePath, RelativePathError, RelativePathErrorReason};
pub use project::{Project, ReconciliationState};
pub use provenance::{ObjectId, Provenance};
pub use reconcile::{
    Action, Contributor, Current, Disagreement, Mergeability, Reconciliation, merge_values,
    reconcile,
};
pub use registry::{
    Change, Excluded, IntegrityChange, InvalidIndex, ObservedTag, ObservedVersion, RegisteredUnit,
    RegistryError, RegistryIndex, SourceReference, declared_identity,
};
pub use resolution::{
    AvailableVersions, Dependencies, Lock, LockChange, Requirement, Requirer, Resolution,
    ResolutionError, ResolvedGraph, ResolvedUnit, resolve,
};
pub use snapshot::{BaseIdentity, BlobId, ListingError, Snapshot, SnapshotFile};
pub use text_merge::{
    Chained, Conflict as TextConflict, InsertTie, Segment as TextSegment, TextMerge,
    describe_change, describe_lines, is_text, merge_text, split_lines,
};
pub use update::{ReconcileCause, ReconcileError, Reconciled, reconcile_amendments, update_lock};
pub use version::{RequirementError, Version, VersionError, VersionRequirement};

pub(crate) use manifest::relation_errors;
