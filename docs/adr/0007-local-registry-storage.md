# 0007: Local registry storage, replacement, and locking

- **Status:** Accepted
- **Date:** 2026-10-20

## Context

The first registry commands (`registry add`, `list`, `remove`, `sync`) need
the persistent state that [ADR 0003](0003-managed-local-registry.md) and
[ADR 0004](0004-registry-validation-and-synchronization.md) describe: a
validated index, initialized lazily, replaced whole only after a change is
validated, and isolated from the developer's registry in tests. They also
need a decision those ADRs left open: how concurrent writers avoid losing each
other's updates. Replacing a file by renaming a temporary one gives readers a
consistent view, but two writers that each read the old index and write a new
one still lose the first writer's change.

The product rules are in
[`registries-and-discovery.md`](../domain/registries-and-discovery.md).

## Decision

1. **Where the code lives.** The rules for what enters the index are pure
   domain code (`domain::RegistryIndex`): conflicts, identities, versions of
   equal precedence, and integrity of indexed versions. The registry adapter
   (`adapters::Registry`) reads sources through `GitRepository`, stores the
   index, and orchestrates each operation. ADR 0001 places registries among
   the adapters, and no use case spans several adapters yet, so there is no
   application layer; one appears when a use case such as `unit build`
   combines the registry with other capabilities. `ObjectId` and `Provenance`
   move into the domain, because the index records them.
2. **Location.** The registry is the directory `assemblr/registry` under the
   platform's local data directory, found with the `dirs` crate:
   `$XDG_DATA_HOME` or `~/.local/share` on Linux, `%LOCALAPPDATA%` on Windows,
   and `~/Library/Application Support` on macOS. The local, not roaming,
   directory is used on Windows because references are absolute paths on one
   machine. Configuration, caches, and snapshots get their own directories
   when they exist.
3. **Test seam.** When `ASSEMBLR_DATA_DIR` is set, the registry is under it
   instead. It must be an absolute path. It exists so that tests and isolated
   environments never read or change the user's registry (ADR 0003,
   Consequences). It affects only where state is kept, never what an
   operation means, and it is not advertised in `--help`.
4. **Index format.** One JSON file, `index.json`, with a `format` number
   (now 1) and the registered units: each identity's source reference and
   indexed versions, with their commit and tree. The file is internal, not a
   contract. A file of another format, or one that breaks the index's
   invariants, is reported as an error and left as it is: Assemblr never
   guesses at it or rewrites it implicitly. A future format change brings its
   own migration.
5. **Replacement.** A writer serializes the complete new index to
   `index.json.tmp`, flushes it to disk (`sync_all`), and renames it over
   `index.json`. On Unix it then synchronizes the directory, best effort, so
   the rename itself survives a crash. Readers take no lock and see either the
   previous index or the next one. If writing the temporary file fails, the
   index is untouched; this is tested by putting a directory where the
   temporary file goes.

   Platform caveats:
   - On Windows, the standard library opens files with `FILE_SHARE_DELETE`,
     so Assemblr's own readers do not block a rename. Another program holding
     `index.json` open without that flag, such as some antivirus scanners, can
     make the rename fail. The failure is reported, and the previous index
     stays in place.
   - Durability after a power loss is not promised beyond what `sync_all`
     and rename provide on the file system in use.
6. **Locking.** Writers serialize through an exclusive lock on the file
   `registry/lock`: `flock` on Unix and `LockFileEx` on Windows, through the
   `fs4` crate. A writer takes the lock, reads the index again, decides the
   change against that index, writes it, and releases the lock. A second
   writer waits for the first. The operating system releases the lock when
   its process exits, even after a crash, so no stale lock needs cleaning up.
   - The standard library's `File::lock` does the same from Rust 1.89, above
     the MSRV of 1.85 (ADR 0002). `fs4` is called through its trait, so the
     newer inherent method never shadows it. When the MSRV reaches 1.89, `fs4`
     can be replaced by the standard library.
   - The lock is advisory: it serializes Assemblr processes, not other
     programs. On network file systems, `flock` may not lock across machines.
   - A concurrency test starts eight registrations at once and checks that
     every one is in the index. Without the lock it fails.
7. **State safety.**
   - A registration is decided once before the lock is taken, and again
     under it. A rejected registration therefore fails before creating
     anything.
   - `list`, and `sync` and `remove` on an empty registry, only read. The
     registry directory and its lock file are created by the first write.
   - A failed operation leaves the index file byte for byte as it was.
8. **Content validation.** Each version is validated when it is first
   indexed: its manifest, and its `src/` read with the bounds of ADR 0006, so
   an indexed version is a valid published artifact. A version already
   indexed is not read again while its tag points at the recorded commit,
   because the same commit has the same tree and therefore the same content.
   Registering a repository with many versions reads each version's `src/`
   once, up to 128 MiB each, one at a time.

Dependencies added under ADR 0002's policy:

| Crate | Purpose |
|-------|---------|
| `dirs` 6 | The platform's local data directory. MSRV-compatible. Brings `dirs-sys` and `option-ext`, plus `libc` on Unix and `windows-sys` on Windows. |
| `fs4` 1.1 (feature `sync` only) | Exclusive file locks on Unix and Windows, released when the process exits. MSRV 1.75. Brings `rustix` on Unix and `windows-sys` on Windows. |

## Consequences

- Every registry write rewrites the whole index under the lock, so writers
  are serialized and the cost grows with the registry, as ADR 0004 accepted.
- `sync` holds the lock while it reads every registered repository, so a
  concurrent `add` waits for it.
- Tests isolate the registry through `ASSEMBLR_DATA_DIR`, and must keep
  doing so.
- References are absolute paths, so an index is specific to one machine.
  Lock portability across machines is still open (OC-49).
