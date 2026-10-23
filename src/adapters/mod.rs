//! Adapters: serialization and integrations with external systems, such as
//! Git, the local registry, and unit builds.
//!
//! Adapters translate between external representations and domain values.
//! They may depend on the domain, never the other way around (see
//! `docs/adr/0001-architecture.md`).

mod build;
mod catalog;
mod git;
mod init;
mod jsonc;
mod jsonc_edit;
mod lockfile;
mod manifest;
mod merge;
mod project;
mod registry;
mod render;
mod schema;

pub use build::{
    AmendFailure, AmendProblem, AmendState, Amended, BuildFailure, BuildLockError, BuildProblem,
    Built, Carried, CheckFailure, CheckProblem, Checked, CrossVersionReason, FileState,
    IndexOutcome, LockfileOutcome, ManifestFileError, Match, OutputProblem, PublishError,
    PublishFailure, PublishProblem, PublishState, Published, PublishedProblem, Recovered,
    RecoveryError, TemplateError, UpdateFailure, UpdateProblem, UpdateState, UpdateWriteError,
    Updated, WriteError, amend_unit, build_unit, check_unit, publish_unit, update_unit,
};
pub use catalog::{
    CATALOG_INDEX, CATALOG_MANIFEST, CatalogInitFailure, CatalogManifest, CatalogManifestError,
    RefreshFailure, RefreshProblem, Refreshed, init_catalog, read_catalog_manifest,
    refresh_catalog,
};
pub use git::{
    Discovered, FetchError, FetchErrorKind, GitError, GitErrorKind, GitRepository,
    HeadManifestError, InvalidVersion, ObjectTooLarge, PublishedVersion, ReadFileError,
    ReadSrcError, SrcEntryError, SrcEntryErrorReason, UnitDirectory, VersionTag,
    discover_repository, init_repository,
};
pub use init::{FileOutcome, GitOutcome, InitFailure, Initialized, Step as InitStep, init_unit};
pub use jsonc::SyntaxError;
pub use jsonc_edit::{EditError as JsonEditError, ValueError};
pub use lockfile::{Lockfile, LockfileError, PublishedLockfileError};
pub use manifest::{
    EditError, FieldPath, IssueKind, JsonKind, ManifestIssue, PathSegment, ReadManifestError,
    declare_amendment, read_manifest, read_project, rebase_amendments,
};
pub use merge::{MergeFailure, MergeNote, Merged, ReconcileFailure, TextConflictReport};
pub use project::{
    ComposeFailure, ComposeProblem, ComposeRecovery, Composed, Continued, DECLARATION,
    DeclarationChange, FileChange, FileConflict, FileConflictKind, InitProjectFailure,
    ProjectInitialized, StoreError, WriteStep, abort_compose, add_unit, compose_project,
    continue_compose, init_project, remove_unit, update_units,
};
pub use registry::{
    Added, Found, Indexed, InfoFailure, Problem, ReferenceError, Registry, RegistryFailure, Search,
    StorageError, Synced, TagProblem, UnitInfo, local_reference, source_reference,
};
pub use render::{
    Context as RenderContext, RenderError, RenderErrorKind, parameters_referenced, render,
};
pub use schema::{MANIFEST_SCHEMA, MANIFEST_SCHEMA_REFERENCE, manifest_schema};
