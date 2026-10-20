# 0006: Reading Git repositories through the `git` executable

- **Status:** Accepted
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
   - `-c core.fsmonitor=false`, `--no-pager`, `GIT_TERMINAL_PROMPT=0`, and
     `GIT_OPTIONAL_LOCKS=0`.

   The repository's own configuration still applies, because it records the
   object format and extensions. This is why only plumbing that runs no
   configured program is used.
5. **Treat objects as untrusted.** Assemblr parses raw commits and trees
   itself. Malformed objects are errors, as are tree entries whose names
   contain `/` or repeat another entry's name, because either would give two
   entries the same path. A manifest is read only if the object header shows
   it is at most 1 MiB.
6. **Provenance and content identity.** A published version's provenance is
   its tagged commit and that commit's root tree. The tree id identifies the
   version's content: it covers every file the version publishes, and commits
   with the same tree publish the same content. Object ids are hexadecimal,
   with 40 digits for SHA-1 or 64 for SHA-256, depending on the repository's
   format. How lock state and snapshots record and verify this identity is
   part of OC-8 and is not decided here.
7. **Git version.** The settings above assume Git 2.32 or later, the first
   release to honor `GIT_CONFIG_GLOBAL`. Older versions still keep the
   settings that prevent fetching and replacement. The version is not
   checked at runtime.

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
- Reading `src/` trees is not bounded in size yet. Bounds become practical
  when the content cache exists (OC-42).
