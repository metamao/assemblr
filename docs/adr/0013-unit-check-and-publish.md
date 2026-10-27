# 0013: `unit check` and `unit publish`: shared validation, the committed tree, the tag, and one-source indexing

- **Status:** Accepted. Extended by
  [0039](0039-catalog-repositories.md), where one tag publishes every unit
  of a catalog and the registry is updated for all of them.
- **Date:** 2026-10-21

## Context

The authoring-completeness increment (#32) adds the two commands that close
the authoring loop before `unit init`: `unit check` validates a unit without
building it, and `unit publish` creates the version tag and indexes it. The
product rules are in
[`authoring-workflow.md`](../domain/authoring-workflow.md#check-details--approved-d-1-d-3)
and the decisions behind them in the autonomous decision log (D-1, D-3,
D-4). This ADR records how they are implemented: where the build's
validation now lives, what a publication validates and how it reads it, how
the clean state is defined, how the tag is created given that Assemblr
otherwise runs Git in a scrubbed environment, and how the registry index is
updated for one source.

## Decision

1. **One validation, three commands.** The part of `build_unit` that ran
   from dependency reading through rendering is now `validate` in
   `adapters::build`, taking the unit's inputs (manifest, `src/`,
   `amendments/`, previous lock) and the registry index, and returning the
   validated lock, its changes, the rendered files, and the
   effective-contribution problems. `build_unit` calls it on the working
   tree and ignores the contribution problems (OC-28); `check_unit` calls it
   on the working tree and `publish_unit` on the tree at `HEAD`, both
   through `publication_checks`, which adds the lockfile and contribution
   checks. The build's write preconditions (`output::check`) stay in the
   build. Diagnostics therefore have one wording, and every problem the
   build reports independently is reported by the other two in the same
   order.
2. **The contribution rule is domain code.** `domain::contribution`
   decides, from the manifest, the `src/` tree, the copies, and each locked
   provider's version and files, whether the unit contributes anything and
   which copies are identical to their base. It compares bytes only when the
   provider is locked at the declared base and both files exist; the other
   cases are the composition's to report.
3. **A publication validates the committed tree.** `publish_unit` reads
   `HEAD`'s manifest, `src/`, `amendments/`, and lockfile with the readers of
   ADR 0006 and ADR 0011 (`read_published`, `read_src`, `read_amendments`,
   `read_top_level_file`), by commit id, so what is validated is exactly
   what the registry and dependents will read. The working tree is read only
   to locate the repository and to check that it is clean.
4. **Clean means "committing everything changes nothing".** Two read-only
   commands under ADR 0006's environment define it: `git status
   --porcelain=v2 -z --untracked-files=no --no-renames` for tracked changes
   and conflicts, and `git ls-files -z --others --exclude-per-directory=.gitignore`
   over the whole tree for untracked files, the same selection the build's
   working-tree reader uses (ADR 0008). `git status` alone would honor
   `.git/info/exclude`, which the build does not, so a file the build
   composed could pass as "ignored" and be missing from the publication.
   `git status` refreshes stat information in memory and, with
   `GIT_OPTIONAL_LOCKS=0`, writes nothing, which is why it is used rather
   than `diff-index`, whose stale-stat false positives would need an index
   write to clear.
5. **The tag is the author's write.** `create_annotated_tag` runs `git tag
   --annotate --message <identity> <version> <tag> <commit>` with the
   author's own Git configuration and identity: the global and system
   configuration apply, and so do `GIT_AUTHOR_*`, `GIT_COMMITTER_*`, and
   `GIT_CONFIG_*`. Only the variables that would redirect the write
   (`GIT_DIR`, `GIT_WORK_TREE`, `GIT_COMMON_DIR`, `GIT_INDEX_FILE`,
   `GIT_OBJECT_DIRECTORY`, `GIT_ALTERNATE_OBJECT_DIRECTORIES`,
   `GIT_NAMESPACE`, `GIT_CEILING_DIRECTORIES`) are removed, and `GIT_DIR`
   and `GIT_WORK_TREE` are set explicitly to the repository found under
   ADR 0006's rules. Network and prompts stay disabled
   (`GIT_ALLOW_PROTOCOL=none`, `GIT_NO_LAZY_FETCH=1`,
   `GIT_TERMINAL_PROMPT=0`), stdin is null, and the tag names the commit by
   id, never `HEAD`. ADR 0006's scrubbed environment exists for reading
   untrusted repositories; it would hide the author's identity from the one
   command that needs it, and a tag created without it fails with
   "Committer identity unknown" unless the repository has local
   configuration. Signing follows the author's settings; a signing failure
   is Git's diagnostic. `git tag` runs no hooks.
6. **Which tag exists is read, not guessed.** `VersionTag` now carries the
   commit each version tag peels to, so `publish_unit` compares the tag of
   the version with `HEAD` and finds tags of equal precedence from the same
   listing the registry reads, without a second Git command.
7. **The registry is rehearsed, then updated for one source.** When the
   index records the repository's normalized path as the source of the
   manifest's identity, the tags the registry would observe, plus the new
   one, are run through `RegistryIndex::sync` for that one identity before
   the tag is created; any error refuses the publication. After the tag,
   `Registry::index_source` takes the registry lock, reads the index again,
   observes this one source with the same validation as synchronization, and
   replaces the index atomically. `sync` leaves identities absent from its
   map untouched, so no other source is read or affected. A failure here is
   the approved partial outcome, with a non-zero exit so scripts notice.
8. **Everything before the tag is a refusal; only the index can fail
   after.** The order is: lock, interrupted-update check, `HEAD`, clean
   state, committed content, the shared validation and publication checks,
   the tag listing, the registration lookup and rehearsal; then the tag;
   then the index. A tag that already points at `HEAD` skips its creation
   and still runs the index update, which is how a failed index update is
   retried.
9. **Testing.** Domain tests cover the contribution rule. Integration tests
   run the binary through the loop (build, commit, publish, a dependent's
   build, with and without an amendment), each refusal with the tree, the
   tags, and the index checked unchanged, a tag missing its identity
   (`user.useConfigOnly`), the partial outcome (the registry's temporary
   index path blocked by a directory) with recovery by `registry sync` and
   by publishing again, the unregistered and the other-clone cases, the
   rehearsal against a moved indexed tag, determinism of the diagnostics,
   and a configured remote that receives nothing.

No dependency was added.

## Consequences

- `unit check` costs a build's reads without its writes; it holds the build
  lock while it runs, so a concurrent build waits.
- The Git directory gains no new file: the check and the publication use
  the build lock of ADR 0008.
- A publication needs the author's Git identity visible to `git tag`, as a
  commit does, and a tracked lockfile; a `.gitignore` that ignores the
  lockfile makes every publication fail with a clear message.
- The build's `DevelopmentValues` diagnostic now says "composing it" rather
  than "a build", since three commands share it.
- `publish_unit` reads the repository's version tags and, for a registered
  source, validates its not-yet-indexed tags twice, in the rehearsal and in
  the update. Fine for Alpha.
