//! Assemblr composes software projects from versioned, reusable units.
//!
//! This library is the reusable core behind the `assemblr` command-line
//! interface. The approved product contracts live in `docs/domain/`, and
//! modules are added here only as real functionality is implemented against
//! them.
//!
//! # Architecture
//!
//! Assemblr follows a functional core / imperative shell design informed by
//! ports and adapters (see `docs/adr/0001-architecture.md`):
//!
//! - **Domain** ([`domain`]): pure concepts, invariants, identities,
//!   resolution and merge semantics, and conflict models. No Git, CLI,
//!   filesystem, network, or other infrastructure dependencies.
//! - **Application**: use cases that plan work and coordinate external
//!   capabilities.
//! - **Adapters** ([`adapters`]): Git, filesystem, local registries,
//!   serialization, caching, and other integrations.
//!
//! The CLI layer (argument parsing, dispatch, diagnostics, output formatting)
//! belongs to the binary target, not to this library.

pub mod adapters;
pub mod domain;
