# 0031: Catalog registration: catalogs in the registry index, versions from the catalog index, and content on demand

- **Status:** Accepted
- **Date:** 2026-10-23

## Context

[ADR 0030](0030-catalog-index.md) gave catalogs a committed
`registry-index.json`. Consumers now need to register a catalog and keep it
synchronized (#95), so that its units reach `search`, `info`, and `add` on
any machine with Git access. The contracts approve that a catalog-managed
unit's versions are discovered through the catalog's index, that the index is
untrusted and verified (OC-41), and that units a catalog drops leave
discovery. How the index is verified is open. The approved "Metadata and
content" rule says unit content is not downloaded indiscriminately when a
catalog is registered, and the owner decided in #95 that registration reads
only the catalog's metadata and that content is fetched when composition
first needs it (OC-42).

## Decision

1. **Metadata only at registration.** Registering or synchronizing a
   catalog fetches the catalog alone. Each manifest its index lists is
   parsed as a manifest and must declare the listed unit and version; the
   listed commit and tree are taken as given. `search` and `info` describe a
   unit whose content was not retrieved from that listed manifest, read from
   the catalog at the recorded commit.
2. **Versions come from the catalog's index.** A unit only catalogs manage
   is indexed with the versions their indexes list, plus those already
   indexed, so a version a catalog drops by mistake stays resolvable. A
   listed version whose commit or tree differs from the one indexed is an
   integrity error, as a moved tag is. A new tag
   is not discoverable until the maintainer refreshes and pushes the
   catalog, as ADR 0005 expects. A unit also registered directly, by the
   same reference, is indexed from its own tags, as before, and the
   catalog's listing is only checked against them.
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
7. **Content on demand.** Every read of a registered remote unit's content
   goes through one function that fetches the copy when it is missing or
   lacks a commit the index records, then requires those commits to exist.
   The build then checks the commit's tree and manifest against the index,
   as it does for every unit, so a catalog that lies about a version is
   caught before anything is written. Retrieving an indexed commit is not
   read as the implicit fetch of remote changes that `unit build` and
   `unit update` must not do (D-39), because it cannot change the index or
   a resolution.

## Consequences

- Registering a catalog of many units costs one fetch, and a unit's history
  is fetched only when a project or a unit first uses it.
- A listed manifest that is wrong in a way its structure does not show,
  such as another commit's description, is visible in `search` and `info`
  until the content is retrieved; the build rejects that version. This is
  the trust the approved rule accepts for metadata.
- The first build that needs a unit needs network access and the user's Git
  credentials for it; later builds read the copy offline.
- A rewritten published version, whose recorded commit is gone, fails when
  its content is first retrieved, not at registration.
- One unreachable catalog rejects the whole registration or
  synchronization, as for directly registered units.
- `unit publish` of a unit only catalogs manage indexes nothing, and says so.
- Two catalogs can declare the same unit only by the same reference, after
  resolution; anything else is a source conflict.
