# 0008: Unit builds: working-tree reading, the unit lockfile, and publishing `build/`

- **Status:** Accepted. Extended by
  [0039](0039-catalog-repositories.md), where a catalog's units build from
  one working tree under one lockfile.
- **Date:** 2026-10-20

## Context

`unit build` is the first operation that combines the registry, published
versions read from Git, the unit's own working tree, and persistent output
in the unit's repository. The product rules are in
[`authoring-workflow.md`](../domain/authoring-workflow.md), including the
owner's decisions on the unit lockfile, the working tree's `src/`, and how
`build/` and the lockfile stay consistent. This ADR records how they are
implemented.

Two files change together, `build/` and `assemblr-lock.json`, and no file
system replaces two entries atomically. If `build/` were replaced first and
writing the lockfile then failed, the two would describe different
dependency graphs while `build/` looked complete.

## Decision

1. **Where the code lives.** The rules are domain code: resolution,
   composition, the collision key, and `relock`, which derives the new lock
   from a resolution and rejects a kept version whose published content
   changed. `adapters::build` orchestrates a build: it reads the working
   tree, the registry, and published content, calls the domain, and
   publishes the result. ADR 0001 lets the application layer depend only on
   the domain, through interfaces that adapters implement. Each capability a
   build needs has one implementation, and AGENTS.md (§2) asks for concrete
   types until a second implementation or a test needs a seam. The
   orchestration therefore stays among the adapters, as the registry's does
   (ADR 0007), and an application layer appears when a use case needs such a
   seam.
2. **Dependencies come from Git, at the indexed commits.** The registry
   index records versions and provenance, not dependencies. A build reads
   the manifest of every indexed version of every identity it can reach,
   starting from the unit's own dependencies and following the
   dependencies of every version, because resolution may choose any of
   them. Each manifest is read at the commit the index records, never
   through a tag, with one `cat-file` session per repository and the bounds
   of ADR 0006. A manifest that no longer matches its index entry, or a
   repository that cannot be read, rejects the build, even when only
   versions that no requirement allows lead to it. Resolution decides
   identities in an order derived from the dependencies of every published
   version (`decision_order` in `domain/resolution.rs`), and the order
   decides which unit keeps its higher version in a conflict. An unread
   source can therefore change the result. The resolution test
   `a_dependency_of_a_version_never_selected_still_orders_decisions` shows
   such a case. Relaxing this needs a different decision order, which OC-9
   records as open. The `src/` of each resolved version is read the same
   way. Extending the index format so that
   `sync` records dependencies was rejected for now: it needs a format
   migration and saves only manifest reads, which are small and bounded.
   Revisit it if builds become slow with many published versions.
3. **The working tree.** The unit's `manifest.jsonc` is read from the file
   system, bounded like a published one (1 MiB). `src/` is selected with
   two read-only Git commands, run with ADR 0006's environment and with
   `GIT_DIR` and `GIT_WORK_TREE` set explicitly:
   - `git ls-files -z --stage -- src` lists tracked paths with their index
     mode;
   - `git ls-files -z --others --exclude-per-directory=.gitignore -- src`
     lists untracked paths that no `.gitignore` file ignores.

   Without `--exclude-standard`, Git applies neither `.git/info/exclude` nor
   `core.excludesFile`, and ADR 0006's null global configuration excludes the
   user's configuration as well. Git reads the index and the working tree
   here, writes nothing (`GIT_OPTIONAL_LOCKS=0`, fsmonitor disabled), and
   runs no configured program. Each listed file is then read from the file
   system:
   - a path that no longer exists was deleted and is skipped;
   - a symbolic link, at the file or at any directory between `src/` and
     it, is rejected, as is any entry that is not a regular file;
   - a path the index records as a symbolic link or a submodule is rejected,
     even when Windows checked a link out as a plain file;
   - ADR 0006's bounds apply: 16 MiB per file, 128 MiB in all, and 10,000
     files and directories;
   - the executable bit is the owner's execute bit on Unix. On Windows it is
     the index mode for tracked files, and not executable for untracked
     ones.

   `amendments/` is listed the same way, only to detect content.
4. **The lockfile format.** `assemblr-lock.json` is JSON with `format: 1`
   and a `units` object. Each identity maps to its `version`, `commit`, and
   `tree`. Keys are sorted and indented by two spaces, with a final newline.
   Reading it is strict: another format number, an unexpected key, an
   invalid identity, version, or object id, or a file over 8 MiB is
   reported, and the file is left as it is. It is rewritten only when the
   new lock differs from the one read.
5. **Serializing builds.** A build takes an exclusive lock on
   `assemblr-build.lock` in the repository's Git directory, as found by
   `git rev-parse --absolute-git-dir`. It uses `fs4`: `flock` on Unix and
   `LockFileEx` on Windows, as for the registry's writers (ADR 0007). The
   lock is taken as soon as the repository opens, before the build reads
   the manifest, `src/`, the lockfile, or the record of an interrupted
   build, and it is held through resolution and publication. A second
   build of the same repository waits for it.
   - **Location.** The Git directory is the one place every build of a
     working tree can find, and Git neither tracks nor shows what is in it.
     So builds need no new reserved name in the working tree, and leave
     nothing there that `git status` would show. A linked worktree has its
     own Git directory, and so its own lock, matching its own `build/`.
   - **Lifecycle.** The file is created by the first build and never
     deleted. Deleting it would let a waiting build lock a file that a new
     build has just replaced. The operating system releases the lock when
     its process exits, even after a crash, so no stale lock needs cleaning
     up.
   - **Limits.** The lock is advisory: it serializes Assemblr builds, not
     other programs writing in the repository. On network file systems,
     `flock` may not lock across machines. The Git directory must be
     writable. A build holds the lock while it reads every dependency
     repository, so a concurrent build of the same unit waits for all of
     it. From Rust 1.89, the standard library's `File::lock` can replace
     `fs4`, as for the registry.
6. **Publishing `build/` and the lockfile.** Nothing is written until every
   validation has passed. Then, in the unit's top-level directory:
   1. if the record of an interrupted build exists (below), what that build
      left is deleted: the directories `.assemblr-build.new/` and
      `.assemblr-build.old/`, and the file `.assemblr-lock.json.new`. The
      record of this build is then written: the file
      `assemblr-build.pending` in the Git directory, flushed to disk;
   2. the new build is written to `.assemblr-build.new/`, with every file
      created new (`create_new`) in directories the build itself created,
      and flushed with `sync_all`; on Unix each directory is flushed too;
   3. a changed lockfile is written to `.assemblr-lock.json.new` and
      flushed;
   4. the previous `build/` is renamed to `.assemblr-build.old/`;
   5. the new lockfile is renamed over `assemblr-lock.json`;
   6. `.assemblr-build.new/` is renamed to `build/`, and on Unix the
      top-level directory is flushed, best effort;
   7. `.assemblr-build.old/` is deleted. If that fails, the build has
      succeeded and a warning names the leftover. Once nothing it wrote
      beside `build/` remains, the record is deleted.

   There is no cross-file atomicity. The guarantee is about `build/`: it
   exists only before step 4, holding the previous build that matches the
   previous lockfile, or after step 6, holding the new build that matches
   the new lockfile. Between them there is no `build/`, so an interruption
   leaves no build rather than one that looks complete. A failure is undone
   by renaming back where that is still valid, and the error states what
   remains:

   | Failure | Undo | What remains |
   |---------|------|--------------|
   | Steps 1 to 4 | Delete what was staged, then the record | `build/` and the lockfile unchanged |
   | Step 5 | Rename the previous build back | Unchanged; if the rename back fails, no `build/` and the previous lockfile |
   | Step 6, lockfile unchanged | Rename the previous build back | Unchanged; if the rename back fails, no `build/` |
   | Step 6, lockfile replaced | None: the previous build no longer matches | The new lockfile, no `build/` |

   **Recognizing leftovers.** The record exists exactly while something
   this build wrote beside `build/` may remain: it is written before the
   first such write, and deleted only when none of the three paths exists.
   A failed build whose cleanup succeeds therefore leaves no record. A build
   interrupted or unable to clean up leaves it, and the next build, holding
   the lock, deletes what is at the three paths. Without a record, anything
   at those paths is someone else's content. It is reported and never
   deleted, whatever it is. Paths are checked before validation ends:
   - `build/` must be absent or a real directory;
   - with a record, the staging directories must be absent or real
     directories, and the staging file absent or a regular file;
   - without a record, all three must be absent.

   A symbolic link or any other entry is reported and left unmodified.
   Deleting a directory never follows a symbolic link inside it, because
   `std::fs::remove_dir_all` does not. Someone who puts content at a
   reserved path after a build was interrupted, and before the next build,
   loses it. That window is the only one, and the names are documented as
   reserved.

   Limits:
   - Durability after a power loss is what `sync_all` and rename provide on
     the file system in use. Windows cannot flush directory entries.
   - On Windows, a rename fails while another program holds a file in
     `build/` open without sharing deletion, such as a development server.
     The failure is reported, and the previous build stays in place.
   - The staging names are reserved beside `build/`. An interrupted build
     leaves them, and Git shows them as untracked until the next build.
7. **Path collisions** compare a key: the path decomposed canonically
   (NFD), each character mapped to its lowercase and then to its uppercase
   when each mapping is a single character, and decomposed again. Paths that
   differ only in case, Unicode normalization, or both collide (see
   [`units.md`](../domain/units.md#src-unit-files)). This applies to every
   composition, not only to builds.
   - This is not Unicode's canonical caseless matching (Unicode, section
     3.13), which uses full case folding: that would map `ß` to `ss` and
     report collisions that no file system has. Case-insensitive file
     systems compare one character to one character, NTFS for example
     through a table of uppercase mappings.
   - Lowercasing alone was the first rule, and it misses pairs that NTFS
     equates through their uppercase, such as `ς` and `σ`, `µ` and `μ`,
     `ſ` and `s`, and `ı` and `i`. Lowercase then uppercase equates those,
     and the pairs that lowercasing equates.
   - Each character is mapped on its own, so the context-dependent
     lowercasing of the Greek final sigma does not apply.
   - Only the standard library's case mappings are used, with the Unicode
     version of the toolchain. Rust 1.85 and 1.99 may differ for characters
     added between them.
8. **Testing failures.** The publishing code takes an internal hook that
   unit tests use to make each step fail. The tests check the resulting
   `build/` and lockfile, and the record, for every row of the table above,
   and the recovery from an interrupted build. Integration tests run the
   binary for the failures a user can cause, such as a `build/` that is a
   file or a symbolic link, or content at a reserved path. An integration
   test starts eight builds of one unit at once against two registries,
   whose resolutions lock different versions, so that which build runs first
   decides the result. It checks that `build/` always matches the lockfile
   and that nothing is left over. Without the lock, it fails.

Dependencies added under ADR 0002's policy:

| Crate | Purpose |
|-------|---------|
| `unicode-normalization` 0.1 | Canonical decomposition (NFD) for the collision key. Pure Rust, MSRV 1.36. Brings `tinyvec`. Its data tables carry the Unicode-3.0 license next to MIT or Apache-2.0. |

## Consequences

- A build reads the manifests of every published version it might choose,
  so its cost grows with the number of versions, within ADR 0006's bounds.
- A dependency whose repository is gone fails every build that can reach
  it, even through a version that would not be chosen, until the
  registration is removed (OC-9).
- `build/` can be missing after a failure or an interruption, never stale
  and looking current. The next successful build restores it.
- The domain's collision rule changed for every composition: paths that
  differ only in Unicode normalization now collide, and so do the pairs a
  case-insensitive file system equates through uppercase.
- Builds of one repository are serialized. A second build waits, without a
  message, until the first has finished.
