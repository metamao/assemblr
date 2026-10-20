//! Adapters: serialization and integrations with external systems, such as
//! Git and the local registry.
//!
//! Adapters translate between external representations and domain values.
//! They may depend on the domain, never the other way around (see
//! `docs/adr/0001-architecture.md`).

mod git;
mod jsonc;
mod manifest;
mod registry;

pub use git::{
    GitError, GitErrorKind, GitRepository, HeadManifestError, InvalidVersion, ObjectTooLarge,
    PublishedVersion, ReadSrcError, SrcEntryError, SrcEntryErrorReason, VersionTag,
};
pub use jsonc::SyntaxError;
pub use manifest::{
    FieldPath, IssueKind, JsonKind, ManifestIssue, PathSegment, ReadManifestError, read_manifest,
};
pub use registry::{
    Added, Problem, ReferenceError, Registry, RegistryFailure, StorageError, Synced, TagProblem,
    local_reference,
};
