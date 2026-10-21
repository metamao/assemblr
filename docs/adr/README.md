# Architectural Decision Records

ADRs record significant **engineering** decisions: architecture, toolchain,
dependency policy, and cross-cutting technical choices. Product semantics
belong in [`docs/domain/`](../domain/), not here.

## Process

- Add a new ADR when a decision changes module boundaries, introduces a
  significant dependency, or sets a convention future work must follow.
- Number ADRs sequentially (`NNNN-short-title.md`). Never reuse a number.
- Do not rewrite an accepted ADR's decision. Supersede it with a new ADR and
  update the old one's status to `Superseded by NNNN`.
- Keep ADRs short: context, decision, consequences.

## Index

| ADR | Title | Status |
|-----|-------|--------|
| [0001](0001-architecture.md) | Functional core, imperative shell in one Cargo package | Accepted |
| [0002](0002-toolchain-and-dependencies.md) | Toolchain, MSRV, and dependency policy | Accepted |
| [0003](0003-managed-local-registry.md) | Managed local registry with Git-based units | Accepted (provisional mechanisms), amended by 0004 |
| [0004](0004-registry-validation-and-synchronization.md) | Registry validation, synchronization, and offline reads | Accepted, amended by 0005 |
| [0005](0005-catalog-metadata-index.md) | Catalog metadata index and metadata-first registry | Accepted |
| [0006](0006-reading-git-repositories.md) | Reading Git repositories through the `git` executable | Accepted, extended by 0008 |
| [0007](0007-local-registry-storage.md) | Local registry storage, replacement, and locking | Accepted |
| [0008](0008-unit-build.md) | Unit builds: working-tree reading, the unit lockfile, and publishing `build/` | Accepted |
| [0009](0009-rendering.md) | Rendering templates with a purpose-built interpolator | Accepted |
| [0010](0010-unit-amend.md) | `unit amend`: the search, the manifest edit, and recovery; amendments in a build | Accepted |
