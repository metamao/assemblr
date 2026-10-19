# 0005: Catalog metadata index and metadata-first registry

- **Status:** Accepted
- **Date:** 2026-10-19

## Context

[ADR 0004](0004-registry-validation-and-synchronization.md) confined remote
fetching to `registry add` and `registry sync`. The owner has since approved a
catalog metadata index: a catalog publishes Assemblr-generated metadata about
the published versions of the units it declares, a maintainer regenerates it
with a catalog refresh operation, and the local registry is built primarily
from metadata, retrieving complete unit content only when composition needs
it. The product semantics are in
[`registries-and-discovery.md`](../domain/registries-and-discovery.md). This ADR
records their engineering consequences. It amends ADR 0004 and does not
replace any of its other decisions.

## Decision

1. **Fetching scope.** Remote reads are invoked from `registry add`,
   `registry sync`, and catalog refresh, and from on-demand retrieval of unit
   content for composition. Which operations trigger that retrieval is open
   (OC-42). Until it is decided, no other operation fetches, and `search`,
   `info`, `unit check`, `unit build`, and `unit update` keep reading local
   state only. This amends decision 2 of ADR 0004.
2. **Three kinds of registry data.** Source declarations, validated unit
   metadata, and complete unit content are kept apart, so metadata can be
   ingested and reused without content, and content is cached only when
   needed. Content caches stay distinct from the immutable historical
   snapshots of ADR 0003.
3. **One metadata model.** Metadata from catalog indexes and metadata obtained
   directly from individually registered units are normalized into the same
   domain data before validation, so resolution and discovery do not depend on
   how a unit was registered.
4. **Deterministic generation.** Catalog refresh is a pure transformation of
   the declared sources and their published history into index data, with
   explicit ordering and no dependence on time, locale, or enumeration order
   (AGENTS.md §3).
5. **Catalog metadata is untrusted input.** Ingested indexes pass through the
   common validation model of ADR 0003, and never through a separate,
   weaker path. How much is verified against the units' own repositories, and
   how verification results are cached, is not decided (OC-41). The
   implementation must not choose either unverified trust or full content
   downloads on every ordinary registry operation.

The index's file name, format, storage, and hash representation are open
(OC-40). No dependency is added by this ADR.

## Consequences

- The local registry depends on catalog maintainers to refresh and publish
  their indexes. A newly tagged version of a catalog-managed unit is not
  discoverable through the catalog until then, so diagnostics must say when
  the index may be stale.
- Ordinary discovery stays cheap and offline, at the cost of a verification
  design that is still open.
- Composition gains a content-retrieval path that can fail for an unreachable
  source after metadata validated successfully. Its diagnostics fall under
  OC-9.
