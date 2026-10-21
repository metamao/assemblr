# 0016: The first consumer `compose`: the project root, the lockfile's reconciliation state, the stored base, ignore rules, and the journal

- **Status:** Accepted, points 7 and 9 extended by 0017, point 6 revised by 0018, point 8 extended by 0019
- **Date:** 2026-10-21

## Context

The first consumer-lifecycle increment (#40) adds `assemblr init` and the
first `assemblr compose`. The owner decided OC-8 for consumer projects on
#41: the lockfile reuses the unit lockfile's conventions plus a versioned
reconciliation state, the base's identity is SHA-256 over a canonical
versioned representation independent of Git and the operating system, the
base is stored in the project as tracked content-addressed state that a
checkout cannot alter, ignored content must not reach that tracked state,
and a base that does not verify is an explicit failure. The product rules
are in
[`consumer-projects.md`](../domain/consumer-projects.md#composition-details--approved).
This ADR records how they are implemented and what the mechanisms are.

## Decision

1. **The project reuses the build's pipeline.** `compose` already takes
   `Root::Project` with the project's selections as the graph's root
   requirements and its `options` as the root's values. The build's
   validation was split into `resolve_graph` (dependencies read from the
   registry, resolution under the lock, `relock`, the locked content, and
   the published amenders' base verification) and `render_files` (templates
   and the merge of several amendments), both taking an optional root
   identity, so that a project (`None`) and a unit share them. The
   unit-only steps, development values, amendment bases, and the
   contribution check, stay in the unit's `validate`. `Dependencies::read`
   gained the form that takes a root identity and requirements.
2. **The declaration** is read by the manifest reader's own field readers
   (`read_project`), with the fields `$schema`, `dependencies`, and
   `options`, and the manifest's relation rules; `domain::Project` holds
   the result. `init` writes the empty declaration and refuses a unit's
   directory.
3. **One lockfile format, two readers.** The parser accepts an optional
   `project` member and returns a `Lockfile` with the units and an optional
   `ReconciliationState`. The unit readers (`read`, `read_published`)
   reject a lockfile that has `project`, and `compose` rejects one that
   lacks it, so neither side adopts the other's file. `units` entries are
   byte-compatible with ADR 0008's.
4. **The identity** is computed in the domain (`domain::snapshot`): a
   `Snapshot` of path to mode and content hash, its canonical `listing`
   (header `assemblr-base-v1`, then `<mode> <blob> <path>\n` in path order),
   and `BaseIdentity`, `sha256-v1:` plus the hex SHA-256 of the listing's
   bytes. `sha2` provides SHA-256 (ADR 0002). Tests pin the listing's
   bytes, that insertion order is irrelevant, and that a content, mode,
   path, or line-ending change each changes the identity.
5. **The store** (`adapters::project::store`) is `.assemblr/` in the
   project: `objects/<2 hex>/<62 hex>` holds each file's bytes once,
   `bases/<64 hex>` holds each listing, and `.gitattributes` holds
   `* -text`. A per-directory attributes file overrides a root `text=auto`,
   so a checkout converts nothing under it; modes are read from the
   listing, never from the file system, so `core.filemode=false` on Windows
   changes nothing. Objects are written with `create_new` and compared when
   they exist. Verification reads the listing, hashes it against the lock's
   identity, parses it, and hashes every blob of a path not listed as
   unbased. An integration test clones a composed project with
   `core.autocrlf=true` and checks the store's bytes and the base's
   verification.
6. **Ignore rules** are read with `git check-ignore -z --stdin` under the
   consumer's own Git configuration (global excludes included), read-only,
   with the repository-location variables removed but the user's
   `GIT_CEILING_DIRECTORIES` kept; exit status 1 means nothing is ignored
   and 128 with "not a git repository" means no repository. A project
   nested in another repository inherits that repository's rules, which is
   what Git would do when the consumer commits. The tests therefore build
   their projects in the system's temporary directory, outside this
   repository, whose `target/` is ignored.
7. **The journal** is `.assemblr/journal.json`: a format number, the
   identity of the composition being applied, the planned files with their
   content identities, and the directories to create. It is written through
   `journal.json.new` and a rename, after the store and before the files,
   and removed after the lock. The next `compose` recovers from a present
   journal. Since the lock is written after every file, a lock whose
   reconciliation state records the journal's identity proves the
   composition complete, and only the journal is removed; reverting there
   would delete the files under a lock that claims them. Otherwise files
   whose bytes still hash to the planned identity are removed, others kept
   and reported, created directories removed when empty, the journal
   removed; then the composition is planned afresh, in which a kept file is
   existing content. A first composition creates files only, so reverting
   to the pre-composition state needs no copy of anything. Unit tests
   inject a failure at each step of the write sequence and check what the
   recovery does.
8. **Serialization of compositions.** Compositions of one project run one
   at a time under an exclusive lock on the declaration's file handle
   (`fs4`), which exists in every project and is never written by
   `compose`, so no lock file is left behind. The declaration is read
   through that same handle: on Windows the lock is mandatory, and a second
   handle cannot read the locked file.
9. **Write order and failure reporting.** Store, journal, directories and
   files (`create_new`, executable bit set on Unix at creation), lock
   through `.assemblr-lock.json.new` and a rename, journal removal. A
   failure names the step; before the journal nothing of the project was
   written; after it, the journal records the composition for the next
   run.
10. **Testing.** Domain tests for the declaration, the listing and
    identity, and the lock parser; integration tests run the binary: `init`;
    the first compose with files, modes, the lock's two halves, the store's
    layout, and the second run's no-op; options and declaration-order
    independence; rejections with the project unmodified, including
    existing content, a unit's lockfile, and a unit's manifest; reserved
    paths; a changed declaration refused and a tampered blob, a tampered
    listing, and a missing store each refused by name; an interrupted state
    constructed on disk and reverted, with an edited file kept, and one
    whose lock is in place completed; unit tests that fail each step of the
    write sequence and check the recovery; an ignored
    `.env` written but unbased, an ignored `.assemblr/`, and a project with
    no repository; a clone with line-ending conversion.

## Consequences

- A consumer project is self-contained: its lock and its stored base travel
  with it, and another machine verifies them without regenerating anything.
  The cost is a second copy of every composed file in version control,
  under `.assemblr/`, which the owner accepted.
- Reconciliation (item 14) has what it needs: the base's listing and blobs
  by identity, the managed paths, and the same canonical identity for the
  incoming composition.
- The unit lockfile reader now refuses a consumer's lockfile, which an
  author could only meet by composing a project inside a unit's repository.
- `sha2` enters the dependency tree.
- A project nested in an ignored directory of another repository gets every
  delivered file unbased, with warnings; `GIT_CEILING_DIRECTORIES` is the
  user's way to bound that.
