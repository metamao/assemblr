# 0010: `unit amend`: the search, the manifest edit, and recovery; amendments in a build

- **Status:** Accepted
- **Date:** 2026-10-21

## Context

The first amendments increment (#26) adds `unit amend` and lets `unit build`
apply the unit's own amendments. The product rules are in
[`amendments-and-composition.md`](../domain/amendments-and-composition.md)
and [`authoring-workflow.md`](../domain/authoring-workflow.md#amend-details--approved).
This ADR records how they are implemented: where amendment composition
lives, how `unit amend` finds a file and records the declaration without
losing the manifest's comments, how a failure or an interruption between its
two writes is handled, and how published versions with amendments are kept
out of the index until they can be composed.

## Decision

1. **Where the code lives.** Amendment declarations are part of the domain
   manifest (`AmendmentDeclaration`), and the manifest's invariant that an
   amendment targets a declared dependency is checked with the other
   relations. Applying amendments is composition (`domain::composition`):
   `UnitContent` carries a unit's copies beside its `src/` files, and
   `compose` replaces a provider's file with the copy when the declared base
   is the resolved version, keeping the provider's delivered path, executable
   bit, and template selection, and recording the amending unit so that the
   renderer can supply `own`. The composition reports every amendment that
   cannot be applied and leaves the provider's file in place, so collisions
   are still found. Telling apart *why* a provider is not at its base needs
   the previous lock, so that check is `domain::lock::amendment_bases`,
   which `adapters::build` runs after resolution; it then drops the
   composition's plainer report for the same path. `unit amend` is
   `adapters::build::amend`, beside the build, because it reads the unit,
   the registry, and the dependencies exactly as a build does and must hold
   the same lock.
2. **The renderer's `own`.** `render::Context::amended` adds the amending
   unit's values; `{{ own.x }}` resolves against them and nothing else, and
   a plain reference never reaches them. The grammar and the scanner are
   unchanged (ADR 0009).
3. **Reading `amendments/`.** The working-tree reader of ADR 0008 is the
   same for `src/` and `amendments/`; the directory is a parameter, and the
   diagnostics name it. Git lists both directories, and the files are read
   from the file system with the same bounds and rejections.
4. **The search.** `unit amend` resolves under the lockfile with the same
   code as a build and refuses to continue when the resolution would change
   the lockfile, because the base of an amendment is a locked version and
   only `unit build` writes the lockfile. It reads the `src/` trees of the
   direct dependencies searched, at the locked commits, from Git. A name is
   compared with each path's last segment, a path with the whole path, each
   also against the delivered path of a template; the copy keeps the source
   path. Matches are collected in identity and path order, and anything but
   exactly one is an error.
5. **Editing the manifest in place.** The manifest is JSONC with comments,
   and rewriting it from the model would lose them. The JSONC reader already
   blanks comments and trailing commas into spaces, keeping every byte
   offset; a small scanner over that strict text (`jsonc::members`) locates
   the root object's members and their value spans, assuming only what the
   parser has already verified and returning nothing on any surprise. The
   declaration is inserted into the original text: after the last member of
   the `amendments` object, into an empty one, or as a new `amendments`
   member after the last root member, following the indentation of its
   neighbours (two levels for the inner member) or inline when the manifest
   is on one line. A trailing comma after the previous last member stays
   valid JSONC after the insertion. The edited text is read back with the
   ordinary reader and must equal the manifest plus the declaration;
   otherwise nothing is written and the error carries the member to add by
   hand. This is the whole editor: it adds one member and nothing else.
6. **Writing and recovery.** Nothing is written until every check has
   passed, and no write follows a symbolic link (AGENTS.md §5): `amendments/`
   and every directory of the copy's path under it must be a real directory
   or not exist yet, the copy's own path must be free or hold an unmodified
   copy, and nothing may exist at the staging path. The copy is created
   first, with `create_new`, in directories the command creates one at a
   time with `create_dir`, and flushed; an existing file is never truncated.
   The manifest is then written to `.assemblr-manifest.jsonc.new` beside it,
   also `create_new`, flushed, and renamed over `manifest.jsonc`; that name
   is reserved, and anything found there is reported, never replaced. If
   the copy cannot be created, it and the directories created for it are
   removed, and nothing changed. If the manifest cannot be written, the
   staged file and the copy are removed again; if the copy cannot be
   removed, the error says it exists undeclared. An interruption between the two steps leaves
   the same state, which `unit build` reports as an undeclared copy. Running
   the same `unit amend` again detects a copy whose bytes are still the
   provider's file and completes the amendment by declaring it; a copy with
   other bytes is the author's work and is never overwritten. No journal is
   needed: the two states an interruption can leave are both detectable
   from the files themselves.
7. **The build lock.** `unit amend` takes the exclusive lock of ADR 0008
   from the moment the repository opens until the manifest is replaced, so
   a concurrent build reads either the old manifest or the new one, never a
   half-written file, and the lockfile it reads cannot change under it.
8. **Published versions with amendments.** Registration and synchronization
   read each newly discovered version's manifest and `src/`. A version that
   declares amendments, or whose root tree has an `amendments` entry, is
   excluded at version level with that reason, like invalid `src/` content.
   Its amendment content is not validated and no composition can apply it,
   so indexing it would present an unvalidated publication as a valid one.
   The composition also refuses a dependency whose manifest declares
   amendments, as a second guard. Both are removed by the increment that
   composes published amendments, which validates them instead.

## Consequences

- A unit with amendments builds, but its published versions are not indexed
  until published amendments can be composed, so no dependent can use them
  yet. The exclusion is reported with every `registry add` and `sync`.
- `unit amend` fails without a lockfile, so the first command in a unit with
  dependencies is `unit build`.
- The in-place editor handles one insertion. Other manifest edits, such as
  the changes `unit update` makes to declared bases, will need the same
  scanner to locate a member's value and replace it; that is an extension
  of `jsonc::members`, not a new mechanism.
- The diagnostics of the working-tree reader name `amendments/` where they
  used to name `src/`; `SrcEntryError` carries the directory.
