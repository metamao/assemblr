//! Adapters: serialization and integrations with external systems, such as
//! Git, the local registry, and unit builds.
//!
//! Adapters translate between external representations and domain values.
//! They may depend on the domain, never the other way around (see
//! `docs/adr/0001-architecture.md`).

mod build;
mod git;
mod jsonc;
mod lockfile;
mod manifest;
mod registry;
mod render;

pub use build::{
    AmendFailure, AmendProblem, AmendState, Amended, BuildFailure, BuildLockError, BuildProblem,
    Built, Carried, CheckFailure, CheckProblem, Checked, FileState, IndexOutcome, LockfileOutcome,
    ManifestFileError, Match, OutputProblem, PublishError, PublishFailure, PublishProblem,
    PublishState, Published, PublishedProblem, Recovered, RecoveryError, TemplateError,
    UpdateFailure, UpdateProblem, UpdateState, UpdateWriteError, Updated, WriteError, amend_unit,
    build_unit, check_unit, publish_unit, update_unit,
};
pub use git::{
    GitError, GitErrorKind, GitRepository, HeadManifestError, InvalidVersion, ObjectTooLarge,
    PublishedVersion, ReadFileError, ReadSrcError, SrcEntryError, SrcEntryErrorReason,
    UnitDirectory, VersionTag,
};
pub use jsonc::SyntaxError;
pub use lockfile::{LockfileError, PublishedLockfileError};
pub use manifest::{
    EditError, FieldPath, IssueKind, JsonKind, ManifestIssue, PathSegment, ReadManifestError,
    declare_amendment, read_manifest, rebase_amendments,
};
pub use registry::{
    Added, Indexed, Problem, ReferenceError, Registry, RegistryFailure, StorageError, Synced,
    TagProblem, local_reference,
};
pub use render::{Context as RenderContext, RenderError, RenderErrorKind, render};
