# 0031: Catalog registration: catalogs in the registry index, versions from the catalog index, and verification by fetching

- **Status:** Accepted, pending the owner's decision in #95 on fetching
- **Date:** 2026-10-23

## Context

[ADR 0030](0030-catalog-index.md) gave catalogs a committed
`registry-index.json`. Consumers now need to register a catalog and keep it
synchronized (#95), so that its units reach `search`, `info`, and `add` on
any machine with Git access. The contracts approve that a catalog-managed
unit's versions are discovered through the catalog's index, that the index is
untrusted and verified (OC-41), and that units a catalog drops leave
discovery. How the index is verified is open, and the approved
"Metadata and content" rule says unit content is not downloaded
indiscriminately when a catalog is registered.

## Decision

1. **Verification by fetching.** Registering or synchronizing a catalog
   fetches every unit it declares into its copy (ADR 0029) and reads its tags
   as `registry add` does. Each version the catalog's index lists must be a
   valid tag with the listed commit and tree, and its manifest must equal the
   listed one after the normalization `catalog refresh` applies. This is the
   strongest check, it reuses every existing validation, and it makes every
   later operation offline, since builds already read content from copies.
   It is the part that conflicts with the approved rule and needs the
   owner's decision (#95).
2. **Versions come from the catalog's index.** A unit only catalogs manage
   is indexed with the versions their indexes list, plus those already
   indexed, so an indexed version stays checked against its tag. A new tag
   is not discoverable until the maintainer refreshes and pushes the
   catalog, as ADR 0005 expects. A unit also registered directly, by the
   same reference, is indexed with every valid tag, as before.
3. **What the catalog is read from.** `registry.jsonc` and
   `registry-index.json` at the catalog's `HEAD` commit, for local and remote
   catalogs alike, never a working tree. The index records that commit. The
   two files must agree, and a catalog without an index is refused, so that
   consumers never fall back to deriving one.
4. **References.** A local catalog's relative references resolve against
   its directory, and the resolved reference is what is stored and compared.
   A remote catalog may declare only remote references, since a local path
   in it means nothing on another machine.
5. **Registry index format 3.** Units gain `direct`, and the index gains
   `catalogs`: identity to `source`, `commit`, and the resolved reference of
   each declared unit. Formats 1 and 2 are read as having only direct units.
   The domain checks that every declared unit is registered from the
   reference its catalog gives, and that every unit that is not direct is
   declared by a catalog.
6. **Membership.** A unit leaves the registry when no registered catalog
   declares it and it is not registered directly. Removing a catalog
   removes such units; removing a direct registration of a declared unit
   keeps it, managed. Copies are never deleted, so content locks need stays
   readable.

## Consequences

- The first registration of a catalog fetches every unit's full history,
  and each synchronization fetches again. Fine for the small private
  catalogs of the alpha; a metadata-first strategy would replace decision 1
  without changing the index format's meaning.
- One unreachable unit rejects the whole registration or synchronization,
  as for directly registered units.
- `unit publish` of a unit only catalogs manage indexes nothing, and says so.
- Two catalogs can declare the same unit only by the same reference, after
  resolution; anything else is a source conflict.
