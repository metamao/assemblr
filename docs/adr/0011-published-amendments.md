# 0011: Published amendments: reading them, validating them at index time, and composing them at the base

- **Status:** Accepted, the composition of a published amendment only at its base lifted by 0023
- **Date:** 2026-10-21

## Context

The second amendments increment (#28) lets a unit with amendments be used as
a dependency: a dependent's `unit build` composes the amendments that
published versions in its graph declare, when each provider resolves to the
declared base. The product rules are in
[`amendments-and-composition.md`](../domain/amendments-and-composition.md#base-identity-and-verification)
and
[`registries-and-discovery.md`](../domain/registries-and-discovery.md#invalid-versions-and-failed-sources--approved).
The owner decided (#28) what registration and synchronization validate, and
that `unit amend` refuses a file a locked dependency already amends. This
ADR records how that is implemented and supersedes point 8 of ADR 0010,
which excluded every published version with amendments from the index.

## Decision

1. **Reading a published version.** The Git tree reader of ADR 0006 takes
   the directory as a parameter: `src/` and `amendments/` are read by the
   same code, with the same bounds and rejections, and the diagnostics name
   the directory. A published lockfile is read through a bounded reader of
   one file at the root of a commit's tree (`GitRepository::read_top_level_file`),
   which returns nothing when the entry is absent and an error when it is not
   a regular file or exceeds the limit; the lockfile format stays in one
   module, `adapters::lockfile`, which the build, `unit amend`, and the
   registry share. Everything is read by commit id, never through a tag.
2. **Index-time validation is self-contained.** For a newly discovered tag
   whose manifest declares amendments or whose tree holds `amendments/`,
   registration and synchronization check only what the version shows on its
   own: `amendments/` reads as unit files, copies and declarations
   correspond both ways, and, when anything is declared, the lockfile exists,
   parses, and locks each amended provider at the declared base
   (`domain::lock::published_amendments`). A failure excludes the tag at
   version level with every problem listed, like invalid `src/` content.
   Checks that depend on other sources are not run here: whether the
   provider provides the path at the base, and whether the base's commit and
   tree are the ones this registry records, would otherwise depend on which
   sources are registered and on their synchronization order (AGENTS.md §3).
   Indexed versions are not re-validated; their integrity protection is
   unchanged, so an indexed version whose tag moves or becomes invalid still
   rejects the whole synchronization.
3. **Base verification lives in the build.** After resolution and before
   composition, the build reads each locked version's `src/` and, when its
   manifest declares amendments, its `amendments/` and lockfile. For each
   declaration it compares the lockfile's entry for the provider at the base
   with the registry index record for that version
   (`domain::lock::verify_bases`); a disagreement is an integrity conflict
   that rejects the build with both records named. A base the index does not
   record is not reported there: the provider then resolves to another
   version, which the composition reports. The verification runs whatever
   else the build finds, so it is reported together with the composition's
   problems.
4. **Composition applies every member's declarations.** `compose` no longer
   distinguishes the root's amendments from a resolved unit's: every member's
   declared amendments are checked and applied by the same rules, and each
   amendment error names its amender, the unit being built or the published
   unit and its version. The contract restricts an amender to its direct
   dependencies; it does not restrict the amender's distance from the root,
   so a transitive dependency's amendments apply too.
5. **One amender per provider file.** Declarations are grouped by provider
   and path before any is checked. A group with several members is reported
   once, with the amenders in order (the unit being built first, then by
   identity), and none of its declarations is checked or applied: the
   provider's file stands, so collisions are still found, and no amender
   takes precedence. The key is the provider's identity and the source path,
   so files of different providers that share a name are unrelated. No
   merge, re-basing, or ordering is attempted (OC-10, OC-12).
6. **`unit amend` looks at the locked dependencies.** Before creating a
   copy, the command checks every locked version's manifest, direct and
   transitive, for an amendment of the same path of the same provider, and
   refuses with that amender and version, so a copy that only a refused build
   could use is never created. Delivered-path collisions are left to the
   build, as before.

## Consequences

- A unit with amendments can be published and used as a dependency. Its
  version is indexed once it is self-consistent, and a registry that
  excluded such versions under ADR 0010 indexes them on its next
  synchronization, since excluded tags were never recorded.
- A dependent that resolves the provider to a newer version than an
  amender's base fails explicitly until applying an amendment to another
  version is decided (OC-10, OC-52). The locked dependent keeps building.
- The build reads two more objects per published amender: its
  `amendments/` tree and its lockfile, at the indexed commit.
- The lockfile's role as the identity of each base is now exercised across
  repositories: tampering with a published lockfile is detected as an
  integrity conflict by every dependent.
