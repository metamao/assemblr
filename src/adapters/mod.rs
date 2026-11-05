//! Adapters: serialization and integrations with external systems, such as
//! Git, the local registry, and unit builds.
//!
//! Adapters translate between external representations and domain values.
//! They may depend on the domain, never the other way around (see
//! `docs/adr/0001-architecture.md`).

mod build;
mod compatibility;
mod git;
mod init;
mod jsonc;
mod jsonc_edit;
mod layered;
mod lockfile;
mod manifest;
mod merge;
mod project;
mod registry;
mod scaffold;
mod schema;
mod skill;

pub use build::{
    AddFailure, AddProblem, AmendFailure, AmendProblem, AmendState, Amended, BaseStoreError,
    BuildFailure, BuildLockError, BuildProblem, Built, Carried, CheckFailure, CheckProblem,
    Checked, CrossVersionReason, Declared, DependencyAdded, FileState, IndexOutcome, Location,
    LockfileOutcome, ManifestFileError, Match, OutputProblem, Pending, PendingFile, PublishError,
    PublishFailure, PublishProblem, PublishState, Published, PublishedProblem, RebaseWriteError,
    RebasedCopy, Recovered, RecoveryError, UnresolvedFile, UpdateFailure, UpdateProblem,
    UpdateState, UpdateWriteError, Updated, WriteError, add_dependency, amend_unit, build_catalog,
    build_unit, check_catalog, check_unit, locate_unit, publish_unit, update_unit,
};
pub use git::{
    Discovered, FetchError, FetchErrorKind, GitError, GitErrorKind, GitRepository,
    HeadManifestError, InvalidVersion, ObjectTooLarge, PublishedCatalog, PublishedVersion,
    ReadFileError, ReadSrcError, SrcEntryError, SrcEntryErrorReason, TagContent, UnitDirectory,
    VersionTag, discover_repository, init_repository,
};
pub use init::{FileOutcome, GitOutcome, InitFailure, Initialized, Step as InitStep, init_unit};
pub use jsonc::SyntaxError;
pub use jsonc_edit::{EditError as JsonEditError, ValueError};
pub use lockfile::{Lockfile, LockfileError, PublishedLockfileError};
pub use manifest::{
    EditError, FieldPath, IssueKind, JsonKind, ManifestIssue, PathSegment, ReadManifestError,
    RemovedField, declare_amendment, read_manifest, read_project, rebase_amendments,
};
pub use merge::{MergeFailure, MergeNote, Merged, ReconcileFailure, TextConflictReport};
pub use project::{
    ComposeFailure, ComposeProblem, ComposeRecovery, Composed, CompositionState, Continued,
    DECLARATION, DeclarationChange, FileChange, FileConflict, FileConflictKind, FileEdit,
    InitProjectFailure, MarkedFile, Origin, ProjectInitialized, ProjectStatus, StoreError,
    UnitStatus, WriteStep, abort_compose, add_unit, compose_project, continue_compose,
    init_project, project_status, remove_unit, update_units,
};
pub use registry::{
    Added, Found, Indexed, InfoFailure, Problem, ReferenceError, Registry, RegistryFailure, Search,
    StorageError, Synced, TagProblem, UnitInfo, local_reference, source_reference,
};
pub use schema::{MANIFEST_SCHEMA, MANIFEST_SCHEMA_REFERENCE, manifest_schema};
pub use skill::{
    SkillFailure, SkillInstalled, SkillOutcome, claude_skill_path, install_claude_skill,
};
