# 0033: Catalog entry commands: `catalog init`, `add`, and `remove`, URL-only references, and commits

- **Status:** Superseded by [0038](0038-remove-registry-catalogs.md)
- **Date:** 2026-10-23

## Context

A catalog's maintainer added a unit by writing its identity and repository
reference into `registry.jsonc` by hand, ran `catalog refresh`, and then
committed both files. Measured with coding agents, that is where catalogs
fail: every agent wrote the entries by hand, copied local paths that mean
nothing on another machine, and only one in four committed the index
([ADR 0030](0030-catalog-index.md) left committing to the maintainer). The
approved contract had no command for catalog entries. The owner decided to
add `catalog add` and `catalog remove`, to commit the catalog's files by
default without pushing, to refuse local paths in `registry.jsonc`
altogether, and to rename `registry create` to `catalog init`, so that every
maintainer command is under `catalog` and every consumer command under
`registry`. The details are provisional (D-45).

## Decision

1. **Identity from the unit.** `catalog add <reference>` reads the unit's
   identity the way registration reads it (its valid tags, or its manifest
   at `HEAD` when it has none) and declares it under that identity. The
   maintainer never types an identity, so a mismatch cannot be written.
2. **One step, all or nothing.** The entry is added to, or removed from, the
   text of `registry.jsonc` with the JSONC editor `assemblr add` uses, which
   keeps comments and layout, and the edit is read back to check it
   declares exactly what was meant. A new entry goes where its identity
   sorts. The whole index is then refreshed from the edited declaration, as
   `catalog refresh` refreshes it, and only when that succeeds are the index
   and then `registry.jsonc` written, each by write-to-temp and rename. If
   the second write fails, the diagnostic says that the index was replaced
   and that `catalog refresh` restores agreement.
3. **One repository per unit.** Adding a unit declared from the same
   reference changes nothing. One declared from another reference is
   refused: moving a unit is a `remove` and an `add`, so it is never
   silent.
4. **Remote references only.** A catalog declares `https://` and `ssh://`
   URLs, `git@host:path`, and `file://` URLs, which tests and local mirrors
   use. A local path is refused by `add`, by `refresh`, and by registration,
   for a catalog registered from a path as well, so a catalog that works on
   its maintainer's machine works on every machine. This removes relative
   reference resolution and its open questions (OC-32, OC-49).
5. **Commit, never push.** `catalog add`, `remove`, and `refresh` commit
   `registry.jsonc` and `registry-index.json` when the working tree
   changes them, with `git commit -- <the two paths>`, so other staged
   changes stay staged and out of the commit. The messages are fixed
   (`Add <unit> to the catalog`, `Remove <unit> from the catalog`, `Refresh
   the catalog index`). Like the tag of `unit publish`, the commit runs with
   the maintainer's Git configuration and identity, including the
   repository's hooks, without network access or prompts. `--no-commit`
   stops after writing. Pushing stays the maintainer's step.

## Consequences

- Adding or removing one unit fetches every declared unit, since the whole
  index is regenerated; one unreachable unit blocks the change, as it
  blocks a refresh.
- A failed commit leaves the files written and says so; nothing is undone.
- A catalog that declared local paths, which an earlier Assemblr accepted,
  is refused until its entries are replaced with URLs.
- `unit publish` cannot point an author to the catalog when the catalog is
  not registered on that machine; units hold no reference to their
  catalogs, by contract.
