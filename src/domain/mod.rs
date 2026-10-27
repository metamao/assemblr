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
mod path;
mod placeholder;
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
    Amendment, ComposedFile, Composition, CompositionError, ContributedPath, FileTree, Root,
    UnitContent, UnitFile, compose, dependency_closure,
};
pub use contribution::{ContributionError, contribution_problems};
pub use discovery::{EmptyQuery, SearchQuery};
pub use identity::{IdentityError, IdentityErrorReason, Segment, UnitIdentity};
pub use lock::{
    AmendmentBaseError, BaseIntegrityError, LockError, LockedUnit, PublishedAmendmentError,
    UnitLock, amendment_bases, published_amendments, relock, verify_bases,
};
pub use manifest::{
    AmendmentDeclaration, BlankDescription, Catalog, Description, Manifest, ManifestError,
};
pub use merge::{
    Combined, Conflict, ConflictKind, Derived, Edit, Lists, MergeFormat, Operation, Plan, Value,
    ValueChange, combine, derive, plan, show_path, write_json_string,
};
pub use path::{RelativePath, RelativePathError, RelativePathErrorReason};
pub use placeholder::{
    Location, PlaceholderCheck, PlaceholderDeclaration, PlaceholderError, PlaceholderList,
    PlaceholderName, PlaceholderNameError, PlaceholderReport, Placeholders, Remaining,
    UnresolvedTokens, check_placeholders, declarations, first_on_each_line,
};
pub use project::{Project, ReconciliationState};
pub use provenance::{ObjectId, Provenance};
pub use reconcile::{
    Action, Contributor, Current, Disagreement, Mergeability, Reconciliation, merge_values,
    reconcile,
};
pub use registry::{
    Change, Excluded, IntegrityChange, InvalidIndex, ObservedTag, ObservedVersion, RegisteredUnit,
    RegistryError, RegistryIndex, Removed, SourceReference, Subject, declared_identities,
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
pub use update::{ReconcileCause, ReconcileError, update_lock};
pub use version::{RequirementError, Version, VersionError, VersionRequirement};

pub(crate) use manifest::relation_errors;
