# 0039: Catalog repositories

- **Status:** Accepted
- **Date:** 2026-10-27

## Context

The owner adopted a design in which one repository, a catalog, holds
several units in `units/` and publishes them together at the version in
`catalog.jsonc`, with one tag (#127; see
[units.md](../domain/units.md#catalogs--approved)). Siblings require each
other as `"catalog"`, the catalog's version. A unit's repository and its
build, check, publication, and registration all assumed one unit at the
top level, with its own version and lockfile.

## Decision

1. **A unit inside a commit.** Reading a version takes a unit directory.
   When the commit has `catalog.jsonc`, the unit's manifest, files, and
   tree are those of `units/<name>`, and its manifest is read with the
   catalog's rules: no `version`, the name the directory gives it, and
   `"catalog"` turned into `=<catalog version>`. A catalog tag is valid only
   when every unit in it is: they are published together.
2. **Content identity is the unit's directory tree.** The provenance of a
   catalog unit's version is the tagged commit and the tree of
   `units/<name>`, so an unchanged unit keeps its tree from one catalog
   version to the next, and the lock's integrity checks work unchanged.
3. **One lockfile, without the siblings.** The lockfile at the root records
   what the units depend on in other repositories. Read for one unit of a
   published commit, the lock is completed with its siblings at the
   catalog's version, with that commit and their directory trees, which is
   what the registry indexes for them. So a dependent composes a published
   catalog unit with the same verification as any other.
4. **Siblings from the given content, through the index.** A build, check,
   or publication gives the resolution the siblings' manifests and content,
   from the working tree or from the commit being published, and registers
   them in a copy of the registry index at their one version, in place of
   any registration they have. Resolution, locking, base verification, and
   composition then run unchanged. A working-tree sibling has no commit, so
   its provenance is the all-zero object id, which never leaves the build:
   the siblings are removed from the lock before it is written or compared
   with the lockfile.
5. **Joint resolution.** The catalog's lock is the resolution of every
   unit at once, with a virtual root that requires each one exactly. Each
   unit is then validated under that lock. A per-unit lock would retire
   what the other units depend on.
6. **Location from the directory.** A directory with `catalog.jsonc` is a
   catalog's top level; a directory of `units/` beside one is a unit of it.
   `unit build` and `unit check` act on every unit at the top level and on
   one unit in its directory; `unit publish` runs at the top level only.
7. **Writing several builds.** Nothing is written unless every unit
   builds. The builds are then published one unit at a time with the
   existing mechanism, the first with the lockfile. The guarantee that a
   `build/` matches the lockfile holds per unit only: a failure after the
   first unit leaves the units already built with the new lockfile and the
   others with their previous build, and the failure names them.
8. **Registration by source.** The registry index records units, each with
   its source, so several units now share a source. Registering, removing,
   and synchronizing work on a source and all its units.

## Consequences

- `unit add`, `unit amend`, and `unit update` are refused in a catalog for
  now (D-56), and amendments between siblings wait for a stored base
  (#128).
- The JSON Schema of the manifest no longer requires `version` and accepts
  `"catalog"`: one schema serves both kinds of unit, and Assemblr's own
  validation stays authoritative.
- An interrupted build at the catalog's top level that left files beside a
  later unit's `build/` may see them reported rather than deleted, since
  the record of an interrupted build is shared by the repository. Nothing
  is deleted that Assemblr did not write.
