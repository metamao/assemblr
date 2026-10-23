# 0030: Catalog index: `registry-index.json`, embedded manifests, and all-or-nothing refresh

- **Status:** Accepted
- **Date:** 2026-10-23

## Context

The contracts approve catalogs (a Git repository whose `registry.jsonc` maps
unit identities to references), `registry create`, and a catalog refresh that
regenerates a deterministic metadata index
([registries-and-discovery.md](../domain/registries-and-discovery.md),
[ADR 0005](0005-catalog-metadata-index.md)). The index's file name, format,
hash representation, and how refresh reports partial failures are open
(OC-40). The owner needs catalogs to start building units (#88's follow-up).
This ADR covers creating a catalog and refreshing its index. Registering and
synchronizing a catalog come later. The product choices are provisional
(D-36).

## Decision

1. **Reading.** Refresh reads `registry.jsonc` from the catalog's working
   tree, not from a commit, so a maintainer can refresh before committing an
   edit and commit both together. Only `name`, `units`, and `$schema` are
   accepted, and a duplicate key or a non-string reference is invalid
   (OC-32). Unknown fields fail rather than being ignored, so a field a later
   version gives meaning to is never silently dropped by an older one.
2. **Validation reuses registration.** Each reference is resolved, a relative
   one against the catalog's root, fetched when remote (ADR 0029), and its
   tags observed and validated through the same `RegistryIndex` rules as
   `registry add`. The repository must declare the identity the catalog gives
   it. No catalog-specific validation path exists, so a catalog cannot accept
   what registration would reject.
3. **Partial failures.** An invalid version is excluded and reported, and the
   refresh continues, as registration does. A source-level failure of any
   unit rejects the whole refresh after every unit has been tried, reports
   each failing unit, and leaves the index unchanged. A partial index would
   look complete to consumers and hide a unit that is only broken for now.
4. **The file.** `registry-index.json` at the catalog's root, next to
   `registry.jsonc`. JSON, pretty-printed with sorted keys and a trailing
   newline, so diffs are reviewable and the bytes depend only on the data.
   Its top level is `format` (1), `name`, and `units`; each unit has its
   declared `source` and `versions`; each version has `commit`, `tree`, and
   `manifest`. The file is replaced by write-to-temp and rename, and only
   when its content changes.
5. **Hashes.** The Git commit and tree ids, as lowercase hexadecimal. They are
   the provenance and content identity the registry index already records, so
   no second hash scheme is introduced.
6. **Embedded manifests.** Each version carries its `manifest.jsonc` at that
   commit, as strict JSON with comments removed. The manifest stays the
   authority; the copy lets a consumer read descriptions, dependencies, and
   parameters without the unit's content (ADR 0005), and lets a later
   verification compare it with the unit's own manifest byte for byte after
   the same normalization (OC-41).

## Consequences

- Refresh needs every declared unit to be reachable. One unreachable private
  repository blocks the refresh until it is fixed or removed from
  `registry.jsonc`.
- Refreshing writes remote copies into the maintainer's data directory, as
  `registry add` would, but never changes the maintainer's registry index.
- The index grows with every published version, since nothing is
  summarized. That is acceptable for the catalog sizes the alpha targets.
- A version of Assemblr that reads only `format` 1 must refuse a later
  format, as the registry index does (ADR 0027).
- `catalog refresh` reports a stale index only by changing it; nothing tells
  a consumer that an index is older than the units' tags. Freshness is part
  of synchronization (OC-33).
