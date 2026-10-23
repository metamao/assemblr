# 0029: Remote unit sources: bare copies, fetching, and transports

- **Status:** Accepted
- **Date:** 2026-10-23

## Context

The contracts allow HTTPS, SSH, and local unit repositories
([registries-and-discovery.md](../domain/registries-and-discovery.md)), but
the registry read only local paths (ADR 0007). The owner needs a private
GitHub catalog of unit repositories that works from any machine with access
to it (#88). This ADR covers single unit repositories reached by URL.
Catalogs come later.

The contract fixes what a fetch may do: `registry add` and `registry sync`
fetch, and `search`, `info`, `unit check`, `unit build`, and `unit update`
never fetch implicitly (ADR 0005). Sources are untrusted (AGENTS.md §5), and
authentication is the user's existing Git and SSH configuration, with no
credential manager.

## Decision

1. **References.** A remote reference is an `https://`, `ssh://`, or
   `file://` URL, or the scp-like `[user@]host:path` Git reads as SSH. It is
   stored exactly as given, as the contract requires. Other transports are
   refused before anything runs: Git's `<transport>::` helpers (`ext::` runs
   a command), plain `http://` and `git://` (not authenticated), anything
   starting with `-`, and control characters. An `https://` URL with user
   information is refused, since it would put a token in the index and in
   every listing. `file://` is accepted because it is safe and keeps the
   feature testable offline. `name:path` that also names an existing local
   path is refused as ambiguous, with a hint to write `./name:path`.
2. **Copies.** Each remote source has a bare repository under
   `<data>/assemblr/sources/<sha256 of the reference>`, beside the registry
   directory of ADR 0007, not inside it. A copy is a cache of what the
   remote published, not registry state. A failed registration can leave a
   copy behind, but never changes the index. The next fetch reuses it.
   `registry remove` leaves the copy, so content a lock needs stays readable.
3. **What is fetched.** `git fetch --no-tags --prune --force` with the
   refspecs `+refs/tags/*:refs/tags/*` and `+HEAD:refs/heads/head`. The copy's
   `HEAD` points at `refs/heads/head`, so a unit with no published version
   takes its identity from the remote's `HEAD`, as a local one does. Tags are
   forced and pruned, so a moved or deleted tag shows in the copy and the
   existing integrity rules reject it. The copy sets `gc.auto=0`, so a
   commit the index recorded stays readable after its tag moves. Objects are
   checked with `fetch.fsckObjects`. A copy is created in a sibling directory
   and renamed into place, so a copy that exists is complete.
4. **The fetch environment.** Unlike reads (ADR 0006), a fetch keeps the
   user's Git configuration and environment, including `GIT_SSH_COMMAND` and
   credential helpers, because that is where their credentials live. The
   user's configuration is trusted; the remote is not. The variables that
   name a repository (`GIT_DIR` and the others authoring removes) are
   removed, `GIT_DIR` is set to the copy, `GIT_ALLOW_PROTOCOL` is
   `https:ssh:file` whatever the configuration says, prompts are off
   (`GIT_TERMINAL_PROMPT=0`), and the reference is passed after `--`. The
   copy is initialized with `--template=` and the read environment, so no
   hook from the user's template directory is installed.
5. **Reads.** Every other operation opens the copy with the read
   environment and never fetches. A missing copy is reported, with a hint
   that `registry sync` fetches it again. Builds and compositions find the
   copy through the user's registry, since there is one registry per user.
6. **Index format 2.** The index's `source` may now be a remote reference.
   The format number moves to 2, so a version of Assemblr that reads only
   format 1 refuses the index instead of failing on an unexpected source.
   Format 1 is still read and is written as 2 by the next change. A stored
   remote reference must pass the same check as a new one.

## Consequences

- `registry add` and `registry sync` reach the network for remote sources.
  `sync` holds the registry lock while it fetches, as it already did while
  reading (ADR 0007).
- Two concurrent fetches of the same new source can collide on Git's ref
  locks. One fails and reports it.
- SSH can still prompt on the terminal for a key's passphrase, since
  Assemblr does not replace the user's SSH command.
- A unit is registered either by local path or by URL on one machine, not
  both: two references for one identity are a source conflict. An author
  who wants a new version visible elsewhere publishes, pushes the tag, and
  runs `registry sync` there.
- The consumer lock records no source, only version, commit, and tree, so
  a lock written on one machine verifies on another that registered the
  same unit from any reference (OC-49).
- Partial fetches and shallow copies are not used (OC-42). A copy holds
  every object reachable from the remote's tags and `HEAD`.
