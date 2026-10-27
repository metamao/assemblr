# 0006: Reading Git repositories through the `git` executable

- **Status:** Accepted. Extended by [0008](0008-unit-build.md), which also
  reads a unit's working tree and index, for `unit build`, and by
  [0013](0013-unit-check-and-publish.md), whose tag creation is the one
  command that runs with the author's own configuration. Point 7 narrowed
  by [0039](0039-catalog-repositories.md): a unit of a catalog has the tree
  of its directory as its content identity.
- **Date:** 2026-10-20

## Context

Every unit is a Git repository, and each published version is a `v<version>`
tag whose manifest declares that version
([`registries-and-discovery.md`](../domain/registries-and-discovery.md#units-are-git-repositories--approved)).
Assemblr must read tags, manifests, and `src/` trees at tagged commits
without switching or modifying the author's working tree.
[ADR 0003](0003-managed-local-registry.md) deferred the choice between the
`git` executable and a Git library to the first implementation, and noted
that a repository's own configuration can name programs that `git` runs.
Standing default D5 makes Git usage and the representation of provenance
engineering decisions recorded in ADRs.

The options considered:

- **The `git` executable.** Adds no dependency. It reads every repository the
  user's Git can, including SHA-256 repositories, partial clones, and linked
  worktrees. Later fetching can use the user's Git and SSH configuration for
  authentication, as ADR 0003 requires. On the other hand, Git must be
  installed, each command starts a process, and Assemblr must guard against
  configuration and environment that would change what Git does.
- **`git2` (libgit2).** Adds a C library to the build on every CI platform.
  Its transports and credentials do not follow the user's Git
  configuration.
- **`gix` (gitoxide).** Pure Rust, but a large dependency tree for the few
  read-only operations needed. Its behavior can differ from the Git the user
  has configured.

## Decision

1. **Use the `git` executable** found on the `PATH`, through
   `std::process::Command`, with arguments passed as a list and the working
   directory set explicitly.
2. **Read only the object database, with plumbing:** `rev-parse`,
   `for-each-ref`, and a single `cat-file --batch` session per operation, so
   process starts do not grow with the number of objects. Nothing is checked
   out. Content is therefore read exactly as committed, without line-ending
   conversion or filters, and the working tree, the index, and `HEAD` are
   never touched.
3. **Pin the repository.** `GIT_DIR` is set to `<path>/.git`, or to `<path>`
   for a bare repository, so Git never discovers a parent repository.
4. **Fix the environment.**
   - Inherited `GIT_*` variables are removed. The rest of the environment is
     kept, because Git needs it on Windows.
   - `GIT_CONFIG_NOSYSTEM=1` and a null `GIT_CONFIG_GLOBAL`: the user's and
     the system's configuration do not affect a read.
   - `--no-replace-objects`: a replace ref cannot substitute other content
     for what a tag published.
   - `GIT_NO_LAZY_FETCH=1` and `GIT_ALLOW_PROTOCOL=none`: a missing object is
     an error, never a fetch. A fetch would reach the network and run the
     transport the repository configures. The second setting also covers Git
     before 2.45, which ignores the first.
   - `-c core.fsmonitor=` (empty, which disables it in every version; before
     Git 2.36 the value names a hook, so `false` would name a program),
     `--no-pager`, `GIT_TERMINAL_PROMPT=0`, and `GIT_OPTIONAL_LOCKS=0`.

   The repository's own configuration still applies, because it records the
   object format and extensions. This is why only plumbing that runs no
   configured program is used.
5. **Treat objects as untrusted.** Assemblr parses raw commits and trees
   itself. Malformed objects are errors, as are tree entries whose names
   contain `/` or repeat another entry's name, because either would give two
   entries the same path.
6. **Bound what is read.** Git stores objects compressed, so a small object
   can expand to gigabytes. Trees can also share subtrees, so a few objects
   can describe billions of paths. A `cat-file --batch-check` session
   reports each object's size before its contents are requested, so contents
   over a bound are never read or streamed. The bounds are fixed constants,
   far above what a unit needs:

   | What | Bound |
   |------|-------|
   | `manifest.jsonc` | 1 MiB |
   | A commit | 1 MiB |
   | A directory listing (tree object) | 4 MiB |
   | A `src/` file | 16 MiB |
   | All `src/` files together | 128 MiB |
   | Files and directories in `src/`, counted at every path a shared subtree appears | 10,000 |

   Exceeding a bound is an explicit error that names the bound, and the
   file or directory when there is one. Nothing is truncated. A version tag
   whose commit, root tree, or manifest exceeds a bound is an invalid
   version.
7. **Provenance and content identity**, approved by the owner. The tagged
   commit is a published version's provenance, and the commit's root tree id
   is its content identity.
   - The root tree identifies the **entire repository snapshot** at that
     commit, not only the files that contribute to a composition, such as
     the manifest and `src/`.
   - A change to any file in the repository, such as a README, therefore
     gives a different identity. Commits with the same tree have the same
     content.
   - Object ids are hexadecimal, with 40 digits for SHA-1 or 64 for SHA-256,
     depending on the repository's format.
   - How lock state and snapshots record and verify this identity remains
     open under OC-8.
8. **Git version: 2.32 or later**, the first release to honor
   `GIT_CONFIG_GLOBAL`. The version is not checked at runtime. The test suite
   passes against Git 2.32.0 built from source, as well as the version CI
   provides.
   - `GIT_NO_LAZY_FETCH` exists only from Git 2.45. Before that,
     `GIT_ALLOW_PROTOCOL=none` alone prevents lazy fetching. With it removed,
     Git 2.32 fetches the missing blobs of a partial clone, and with it in
     place, it does not.
   - Git 2.32's own documentation (`Documentation/git.txt`) describes every
     other setting used here.

## Consequences

- Assemblr needs Git on the `PATH`. When it cannot run `git`, the error says
  so.
- Fetching and cloning, in the registry increment, need the user's Git and
  SSH configuration for authentication. They therefore need a different,
  deliberately chosen environment, recorded when they are implemented.
- SHA-1 repositories identify content with Git's SHA-1, which Git hardens
  against known collision attacks. SHA-256 repositories use SHA-256.
- Tests need Git, which the CI runners provide. Fixtures are written with
  Git's plumbing, so trees holding symbolic links, submodules, or names a
  file system rejects can be tested on every platform.
- A unit that needs more than the bounds in decision 6 cannot be read. The
  bounds can be raised when a real unit needs it, in a new ADR rather than
  through configuration.
- CI tests only the Git version its runners provide. The 2.32 minimum was
  verified once, by hand. A CI job that builds an old Git is possible if
  that evidence needs to stay current.
