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
