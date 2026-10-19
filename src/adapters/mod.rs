//! Adapters: serialization and, later, integrations with external systems.
//!
//! Adapters translate between external representations and domain values.
//! They may depend on the domain, never the other way around (see
//! `docs/adr/0001-architecture.md`).

mod jsonc;
mod manifest;

pub use jsonc::SyntaxError;
pub use manifest::{
    FieldPath, IssueKind, JsonKind, ManifestIssue, PathSegment, ReadManifestError, read_manifest,
};
