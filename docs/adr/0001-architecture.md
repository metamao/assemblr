# 0001: Functional core, imperative shell in one Cargo package

- **Status:** Accepted
- **Date:** 2026-10-18

## Context

Assemblr's core concerns are dependency resolution, composition, merge
semantics, and conflict detection. They must be deterministic and testable
without Git, a filesystem, or a terminal. Its edges talk to untrusted
external state: Git repositories, unit files, registries, and users. The owner
selected this architecture before implementation began.

## Decision

Use a **functional core / imperative shell** architecture, informed by
**ports and adapters**, in a **single Cargo package** that builds:

- a reusable library (`src/lib.rs`, crate `assemblr`), and
- the `assemblr` CLI binary (`src/main.rs`).

Conceptual layers and allowed dependencies:

| Layer | Responsibility | May depend on |
|-------|----------------|---------------|
| Domain | Pure concepts, invariants, identities, resolution/merge semantics, conflict models | std and pure-data crates only |
| Application | Use cases, planning, orchestration of external capabilities | Domain |
| Adapters | Git, filesystem, registries, serialization, rendering, caching | Domain, Application |
| CLI | Argument parsing, dispatch, diagnostics, output formatting | Library public API |

Dependencies point inward. The domain never depends on the application,
adapters, or CLI. Where the application needs an external capability, it
defines the narrowest interface it needs, and an adapter implements it.

Module placement:

- Domain, application, and adapter modules live in the **library**, declared
  from `src/lib.rs`.
- CLI modules belong to the **binary target**. They are declared from
  `src/main.rs` and live under `src/cli/` when the binary outgrows one file.
  `src/lib.rs` never declares or depends on `cli`.
- `clap` and other presentation concerns stay out of the library.

Layers are **boundaries, not scaffolding**. Directories and modules are created
only when real functionality needs them. No empty layer modules, placeholder
traits, or speculative interfaces.

A multi-crate workspace is **not** used. Splitting into crates requires a new
ADR showing a concrete need, such as compile times, independent publication,
or enforced boundaries that module visibility cannot provide.

## Consequences

- Domain logic can be tested exhaustively with plain unit tests, without I/O.
- Boundary discipline relies on module visibility, review, and `AGENTS.md`
  rather than crate boundaries. Reviewers must reject inward-pointing
  violations, such as domain code importing `std::fs` or `std::process`.
- The library API is internal for now. `publish = false` prevents accidental
  publication to crates.io.
