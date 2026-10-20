# 0003: Managed local registry with Git-based units

- **Status:** Accepted. The mechanisms below are provisional defaults.
  Amended by [0004](0004-registry-validation-and-synchronization.md).
  Locations, persistence, and the test seam are implemented by
  [0007](0007-local-registry-storage.md).
- **Date:** 2026-10-18

## Context

The foundation domain contracts defined two local registry modalities: a Local
Directory registry that read working directories, including uncommitted
changes, and a Local Git registry that read tagged versions. Registries were
arbitrary filesystem locations, possibly grouping directories scanned for
units, configured by the user. No ADR recorded that model. It lived only in
`docs/domain/`.

The owner has replaced it. The registry now affects unit identity, discovery,
publication, resolution, reproducibility, amendment provenance, and artifact
integrity, so it is a first-class subsystem that Assemblr manages itself. The
product semantics are in
[`registries-and-discovery.md`](../domain/registries-and-discovery.md). This ADR
records the engineering consequences and the default mechanisms.

The design draws on the analogies of npm (CLI-managed state and caching), Cargo
(separate source, index, and cache; SemVer resolution; locks), Homebrew taps
(explicitly registered collections), Helm repositories (catalog metadata
instead of directory discovery), and Go modules (Git version provenance). These
are analogies, not evidence. No prototype of this registry has been evaluated.

## Decision

This decision supersedes the Local Directory and Local Git registry model
described above. It does not supersede ADR 0001 or ADR 0002.

The domain contracts fix these points:

- **One managed local registry**, global to the user's installation, owned by
  the CLI, and initialized lazily on first use. Users do not maintain registry
  configuration files.
- **One Git repository per unit.** Each published version is a `v<version>`
  tag whose manifest declares exactly that version. Git is version history,
  not only transport.
- **Registered sources** are unit repositories and catalogs. Catalogs are Git
  repositories with a declarative manifest of unit identities and Git sources.
  Both are registered explicitly. Nothing is discovered by scanning
  directories.
- **Synchronization is explicit.** Discovery reads the registry index and never
  fetches.

These mechanisms are **provisional defaults**. A later ADR may replace any of
them on technical evidence, provided the approved product semantics are kept:

1. **Locations.** Platform application directories, XDG-compatible on Linux
   and native on Windows and macOS, with configuration, persistent metadata,
   managed sources, historical artifacts, and disposable cache kept apart.
   There is no hardcoded `~/.assemblr`. Names and layout are internal.
2. **Persistence.** A small, file-based store, versioned so its format can
   migrate. No database until evidence shows the file store is insufficient.
   Mutations are computed and validated completely before being written
   (AGENTS.md §4). Writes are crash-conscious, but no atomicity is claimed until
   a mechanism such as write-to-temp plus rename is implemented and tested on
   every CI platform.
3. **Git.** Use Git's object and tag infrastructure. Read tagged content
   without touching authors' working trees. Consider annotated release tags,
   subject to OC-26. Delegate authentication to the user's Git and SSH
   configuration. Do not use submodules. The choice between invoking the `git`
   executable and a Git library is deferred to the first implementation and
   recorded then.
4. **Snapshots.** Immutable and content-addressed, with explicit Git
   provenance, so amendment bases can be reconstructed. Each version's tag
   commit and content identity are recorded when it is first observed, so a
   moved tag can be detected. Historical snapshots are not garbage-collected
   automatically in Alpha. Disposable caches are kept separate from them.
5. **Validation.** One validation model shared by unit registration, catalog
   registration, version discovery, publication, and synchronization. Registry
   operations are idempotent when inputs and intended state are unchanged.

The functional-core boundary of ADR 0001 is unchanged. The registry's
persistent state belongs to adapters. Domain code receives registry contents
as explicit input data and never reads global state itself.

No dependency is added by this ADR. Crates such as `directories` are
candidates when the registry is implemented, under ADR 0002's policy.

## Consequences

- Assemblr takes on persistent-state management: format migration,
  crash-consciousness, index synchronization, Git integrity checks, retention
  of historical artifacts, and source availability across machines.
- Tests need a seam that points the registry at a temporary directory, so they
  never read or modify the developer's real registry (AGENTS.md §8). Whether
  that seam is also a user-visible override, such as an environment variable
  or flag, is a product question. The owner has since classified registry
  isolation for tests as an engineering requirement, not a user-facing product
  capability. The registry implementation must provide it. The mechanism,
  including any override the binary needs, is an engineering decision,
  recorded when the registry is implemented.
- Registered repositories are untrusted input. Their repository-local Git
  configuration can name programs that `git` executes, so Git invocations on
  sources must not run commands configured by the source (AGENTS.md §5).
- Without automatic cleanup, historical storage grows until a retention policy
  is decided, after the consumer reconciliation and lock contracts (OC-1,
  OC-8). OC-1 has since been resolved by
  [`consumer-projects.md`](../domain/consumer-projects.md). Retention remains
  open under OC-8.
- Several product details remain open, for example development dependencies
  on unpublished units (OC-30), freshness outside synchronization (OC-31), the
  catalog schema (OC-32), and synchronization granularity and removal effects
  (OC-33). See [open-contracts.md](../domain/open-contracts.md). OC-30 and
  OC-31 have since been resolved, and OC-32 and OC-33 narrowed (see
  [ADR 0004](0004-registry-validation-and-synchronization.md)).
