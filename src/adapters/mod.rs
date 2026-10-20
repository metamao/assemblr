//! Adapters: serialization and integrations with external systems, such as
//! Git.
//!
//! Adapters translate between external representations and domain values.
//! They may depend on the domain, never the other way around (see
//! `docs/adr/0001-architecture.md`).

mod git;
mod jsonc;
mod manifest;

pub use git::{
    GitError, GitErrorKind, GitRepository, InvalidVersion, ObjectId, Provenance, PublishedVersion,
    ReadSrcError, SrcEntryError, SrcEntryErrorReason, VersionTag,
};
pub use jsonc::SyntaxError;
pub use manifest::{
    FieldPath, IssueKind, JsonKind, ManifestIssue, PathSegment, ReadManifestError, read_manifest,
};
