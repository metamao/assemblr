# 0040: Stored amendment bases in a catalog

- **Status:** Accepted
- **Date:** 2026-10-27

## Context

The owner decided on #128 that the base of an amendment of a sibling is
stored in the catalog, in the content-addressed format of a consumer
project's store, that `unit build` merges a change of the file below a
copy into it, and that `unit check` and `unit publish` refuse a copy
whose stored base is not that file now (see
[the base in a catalog](../domain/amendments-and-composition.md#the-base-in-a-catalog)).
Until then a base was always a published version, read back from the
registered repositories and verified against the lockfile.

## Decision

1. **One store, the consumer's.** The catalog's `.assemblr/` holds the
   bases with the consumer project's store code: blobs under `objects/` by
   SHA-256, and one listing for the whole catalog under `bases/`, by its
   identity. The listing's paths are `<unit directory>/<amended path>`.
   The tracked record `.assemblr/amendment-bases.json`, with a `format`
   number and the listing's `base` identity, takes the place of the
   consumer's lockfile entry; a reader refuses a format or member it does
   not know (ADR 0027). Every read verifies the listing against the
   identity and each blob against its name (D-58).
2. **Every amendment in a catalog has a stored base**, of a sibling's file
   or of another repository's, since a sibling's copy below can be part of
   either base (D-59). The declaration of a sibling's file names its base
   `"catalog"`, read as the catalog's version (D-57), so the existing
   checks that a declared base is the locked version hold unchanged.
3. **A copy of the working tree is on the composition below it.** In a
   build, check, or publication, a sibling's copy is composed as the
   unit's own copy is: on the file the units below compose, with no base
   reconstructed from a published version. The rebase makes that true
   before composing, and the stale-base refusal keeps it true in what is
   published, so a dependent composes a published catalog unit with the
   existing verification.
4. **The rebase is the update's.** The three-way merge is the one
   `unit update` uses for a copy, JSON value by value and text by line,
   with the text markers naming the stored base and the file below the copy
   now. Units are rebased each after the siblings they depend on, with the
   rebased copies below in place, so layered copies (ADR 0032) follow.
5. **Writes come last.** A clean rebase is computed in memory and written
   only after the whole build validates: each copy through a new file and
   a rename, then the store, whose record is replaced last. A failed write
   puts the replaced copies back. A rebase with conflicts writes every
   rebased copy and the new bases, and builds nothing, as the pending state
   does (D-60).

## Consequences

- `.assemblr/` must be committed with the catalog. A clone without it
  fails explicitly, naming each copy without a base; its recovery is to
  take the copy again with `unit amend` and redo the changes.
- Amendments that a catalog declared by hand before this change, which
  have no stored base, fail the same way until they are taken again.
- A failure while writing that cannot put a copy back leaves it rebased
  over its old stored base, and says so; the next build rebases it again
  from that base, with the change already on both sides.
- `unit add` and `unit update` stay refused in a catalog (D-56).
