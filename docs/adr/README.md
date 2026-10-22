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
| [0010](0010-unit-amend.md) | `unit amend`: the search, the manifest edit, and recovery; amendments in a build | Accepted, point 8 superseded by 0011 |
| [0011](0011-published-amendments.md) | Published amendments: reading them, validating them at index time, and composing them at the base | Accepted, the composition at the base only narrowed by 0023, points 5 and 6 narrowed by 0015 |
| [0012](0012-unit-update.md) | `unit update`: resolving with a partial lock, rewriting bases in place, and the update journal | Accepted, the refusal of a changed file and of a published amender off its base lifted by 0023 |
| [0013](0013-unit-check-and-publish.md) | `unit check` and `unit publish`: shared validation, the committed tree, the tag, and one-source indexing | Accepted |
| [0014](0014-unit-init.md) | `unit init`: repository discovery and initialization, the scaffold's writes, and the embedded manifest schema | Accepted |
| [0015](0015-value-merge.md) | The value-level merge of several amendments: the domain engine, the JSONC editor, and the projection and round-trip checks | Accepted, point 6 clarified by 0018, the refusal of other formats narrowed by 0021 |
| [0016](0016-consumer-compose.md) | The first consumer `compose`: the project root, the lockfile's reconciliation state, the stored base, ignore rules, and the journal | Accepted, points 7 and 9 extended by 0017, point 6 revised by 0018, point 8 extended by 0019 |
| [0017](0017-consumer-reconciliation.md) | Consumer reconciliation: the per-path three-way decision, the value merge onto the project's text, the journal of changes, and recovery | Accepted, point 8 revised by 0018, the refusal of text conflicts replaced by 0020, text merged by line by 0022 |
| [0018](0018-ignore-classification-and-merge-integrity.md) | Which ignore rules decide the lock, and what a failed merge check is | Accepted |
| [0019](0019-consumer-add-and-remove.md) | `add` and `remove`: editing the declaration, the default requirement, and writing it through the locked handle | Accepted |
| [0020](0020-pending-resolution.md) | The pending-resolution state: marked files, a journal that waits, and `compose --continue` and `--abort` | Accepted, the whole-file markers of point 3 narrowed to structured files by 0022, points 5 and 6 amended by #72 |
| [0021](0021-text-merge.md) | The line-level merge of text: the domain engine, which files it covers, and what it reports | Accepted |
| [0022](0022-text-reconciliation.md) | Text reconciliation: the line merge onto the project's file, and conflicts marked where they happen | Accepted |
| [0023](0023-cross-version-amendments.md) | Amendments across provider versions: deriving against the base, applying to the version resolved, and rebasing in `unit update` | Accepted, point 4 amended by #70 |
| [0024](0024-consumer-update.md) | `assemblr update`: resolving with a reduced lock, the lock as a record of the resolution, and one reconciliation | Accepted |
| [0025](0025-parameter-mappings-and-examples.md) | Parameter mappings and examples: option values as a union, values decided from dependents to dependencies, and development values | Accepted |
| [0026](0026-private-alpha-distribution.md) | Private alpha distribution: prebuilt binaries as draft GitHub Releases | Accepted |
| [0027](0027-journal-format-compatibility.md) | Journal formats: a number for every change, and readers that refuse what they do not know | Accepted |
