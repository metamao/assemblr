# 0004: Registry validation, synchronization, and offline reads

- **Status:** Accepted. Amended by
  [0005](0005-catalog-metadata-index.md).
- **Date:** 2026-10-18

## Context

[ADR 0003](0003-managed-local-registry.md) introduced the managed local
registry and left several product details open. The owner has since settled
them: the catalog manifest format, source-conflict rules, atomic registration
and synchronization, how `unit publish` relates to the index, and the rule that
dependencies come only from published versions (OC-30, OC-31). The product
semantics are in
[`registries-and-discovery.md`](../domain/registries-and-discovery.md). This ADR
records their engineering consequences. It amends ADR 0003 and does not
replace any of its decisions.

## Decision

1. **Whole-index replacement.** Registration and synchronization build a
   complete proposed index, validate it fully, and only then replace the
   active index. On failure the active index is untouched. This is a product
   requirement, so the implementation must provide it with a mechanism that is
   implemented and tested on every CI platform, such as writing a new index and
   renaming it into place. Until that exists, nothing claims atomicity
   (AGENTS.md §4).
2. **Fetching is confined.** Only `registry add` and `registry sync` obtain
   changes from remote sources. `search`, `info`, `unit check`, `unit build`,
   and `unit update` read local state only, and report that synchronization is
   needed when a required published version is missing. The capability to
   fetch is therefore invoked only from those registry use cases.
3. **Catalog revisions are recorded.** Synchronizing a remote catalog reads its
   Git default branch and records the exact catalog commit in the index. That
   commit is provenance for the inventory of sources. It is separate from unit
   snapshots, which are retained for locks and reconciliation.
4. **References are compared exactly.** Source references are compared as
   given: identical references may be deduplicated, and different references
   to the same identity conflict. No URL normalization, history comparison, or
   repository fingerprinting is implemented, so no dependency is needed for
   them.
5. **Publication writes the index.** When the unit is already registered,
   `unit publish` creates the tag and then updates the index, with the same
   validate-then-replace discipline. A failed index update after a successful
   tag is reported as a partial outcome, never hidden, and `registry sync`
   recovers.
6. **Validation never executes source code.** This applies the untrusted-input
   rules of AGENTS.md §5 and the Git configuration caveat in ADR 0003 to every
   registration and synchronization check.

No dependency is added by this ADR.

## Consequences

- One failing validation can block a whole synchronization. Which failures do
  so, and which only exclude the affected versions, is still open (OC-33). The
  implementation must not choose until it is decided.
- Every registry mutation produces a complete new index, so its cost grows
  with the registry. That is acceptable for a local, file-based Alpha store.
  Revisit it with evidence if it becomes a problem.
- Authoring commands work offline against the index, so the "synchronization
  needed" diagnostic must be precise enough to act on.
- Behavior for local catalogs with uncommitted changes is not decided (OC-32).
  Implementations must not infer it.
