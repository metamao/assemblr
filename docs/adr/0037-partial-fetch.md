# 0037: Partial fetch: metadata at registration, a version's files when first read

- **Status:** Accepted
- **Date:** 2026-10-27

## Context

ADR 0029 fetches every object reachable from a remote's tags and `HEAD`.
Registration reads only metadata, the manifest and the lockfile of each
version, so it downloads the files of every published version to read a few
small files from each. The catalog design (#126, #127) puts several units in
one repository, which makes that cost grow with every unit and version. The
contract asks that content be retrieved when composition needs it, not when a
source is registered ([Metadata and content](../domain/registries-and-discovery.md#metadata-and-content--approved),
OC-42).

A spike on GitHub (ripgrep, vite) measured a blobless fetch plus the
manifests by id at about a third and a fifth of a full fetch.

## Decision

1. **Registration fetches no file contents.** `registry add` and `registry
   sync` fetch with `--filter=blob:none`: commits, tags, and trees. Then they
   fetch, by id and without a filter, the blobs of the metadata files at the
   top level of every version tag and of `HEAD` (`manifest.jsonc`,
   `assemblr-lock.json`, and the catalog files), with
   `fetch.negotiationAlgorithm=noop`, since the copy already has everything
   that leads to them. Missing objects are found with `git rev-list
   --missing=print`, because `git cat-file` on a missing object is fatal when
   lazy fetching is off.
2. **A version's files are fetched when first read.** A copy of a remote is
   opened with its reference. Before it reads the files of a commit, it lists
   what that commit lacks and fetches exactly those objects by id, then
   checks nothing is still missing. This happens in `add`, `compose`, and
   `update` in a project and in `unit build`, `unit check`, `unit update`,
   and `unit amend` for a dependency, as content on demand already did for
   catalog units (ADR 0031). Only the versions read are fetched. Once
   fetched, a version reads offline.
3. **No lazy fetching.** Reads keep the environment of ADR 0006, which sets
   `GIT_NO_LAZY_FETCH=1` and `GIT_ALLOW_PROTOCOL=none`. Git never fetches
   behind Assemblr's back, and a Git older than 2.45, which ignores the first
   setting, still cannot reach any transport. Every fetch is one Assemblr
   runs explicitly, with the fetch environment of ADR 0029.
4. **Registration checks trees, not sizes.** A file's size is in its blob,
   which registration no longer has. Registration still excludes a version
   whose `src/` or `amendments/` holds a symbolic link, a submodule, an
   invalid path, or too many entries, since those are in the trees. A file
   or directory larger than Assemblr reads is reported when the version is
   first read, and that operation fails. The same rule applies to local
   repositories, so a version is indexed or excluded the same way whatever
   the transport. This changes an approved statement and awaits the
   owner's decision in #126.
5. **Servers without filters.** A server that does not support filters sends
   everything, which is still correct. Git's own `file://` transport does so
   unless the repository sets `uploadpack.allowFilter`; the tests set it.

## Consequences

- Registration and synchronization download a fraction of what they did.
  The first build or composition of a version needs the remote reachable.
  A failure says which reference could not be fetched.
- A version whose files are too large is indexed and fails on first use,
  with a hint to select another version, instead of being excluded when
  registered.
- Copies made by a full fetch keep working: they lack nothing.
- A copy now holds the promisor settings Git writes for a filtered fetch.
  Reads ignore them, since lazy fetching is off.
- The fetch by id relies on the server accepting a want for any reachable
  object (`uploadpack.allowReachableSHA1InWant` or protocol v2, which
  GitHub and GitLab use). SSH and GitLab were not measured.
