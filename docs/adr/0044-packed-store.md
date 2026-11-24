# 0044: The packed store: one file per base, only the bases in use, and attributes that keep it out of diffs

- **Status:** Accepted
- **Date:** 2026-11-24

## Context

A consumer project and a catalog store their bases in `.assemblr/`
(ADR 0016, ADR 0040), and the store is committed. Until now it held each
file's bytes under `objects/<2 hex>/<62 hex>` and each listing under
`bases/<64 hex>`, and it was never cleaned. A pull request that composes a
project therefore lists one added file per composed file: the first
composition of `metamao/assemblr-web` added 205 files, 103 of them under
`.assemblr/`. The owner found the file tree and the diffs too noisy to
review, and asked for a fix that keeps everything the store provides: a
self-contained, verifiable base for every three-way merge.

What was checked before deciding, with that pull request's files:

- The cost was review, not size: 92 of its 100 objects were the same Git
  blob as the composed file, so Git stored them once.
- One uncompressed file per base costs what the earlier layout cost in a
  repository. After `git gc`, a pack of the composition took 359 KiB, and a
  second pack after an update that changed three files added 1 KiB; the
  earlier layout took 401 KiB, then 404 KiB. Gzipped packs took 357 KiB,
  then 713 KiB, since Git cannot store a compressed pack as a delta of the
  one before.
- GitHub collapses the diff of a path marked `linguist-generated`
  ([documentation](https://docs.github.com/en/repositories/working-with-files/managing-files/customizing-how-changed-files-appear-on-github)),
  as it does for `package-lock.json`. Its documentation does not say that
  the file tree leaves such a path out, so the attribute alone does not fix
  the tree.
- The format promise starts with the first beta
  ([compatibility.md](../domain/compatibility.md)), so the layout can change
  now.

The owner decided, one question at a time, on 2026-11-24: pack the store,
one file per base; keep only the bases in use; apply it to catalogs too;
mark the store `-diff` and `linguist-generated`.

## Decision

1. **One pack per base**, at `.assemblr/packs/<64 hex>`, named by the
   base's identity like the earlier listing. It holds `assemblr-pack-v1`,
   then `listing <length>` and the canonical listing, then
   `blob <sha256> <length>` and the bytes of each stored file, once each,
   in the order of their identities. Lengths are decimal without leading
   zeros, and nothing is compressed. The bytes depend on the listing and the
   stored blobs only, so writing the same base again writes the same file;
   a sample in `tests/formats/pack-1` pins the encoding. The lock, the
   listing, and the identity `sha256-v1:` do not change.
2. **Reading is strict.** A pack is read whole, and refused unless it is
   exactly what is written: the header, the listing, blobs in strictly
   increasing order, each hashing to its name, nothing after the last, and
   no blob the listing does not name. The listing is verified against the
   identity the lock records, and each path not unbased must have its
   blob. The blobs read are kept in memory for the merges of that run, so
   nothing reads the pack twice.
3. **Only the bases in use are kept.** After the lock is written, by
   `compose`, `compose --continue`, or the recovery that completes an
   interrupted composition, every other pack goes. `compose --abort` keeps
   only the lock's base. A composition waiting for the consumer keeps its
   pack beside the lock's until then. A catalog keeps only the pack its
   record names. Removal is cleanup: a failure leaves a harmless file, as
   an interrupted removal would, and is not reported. An earlier base is in
   version control's history with the lock that recorded it.
4. **Writing replaces a pack that differs.** A base with the same identity
   can store different files when the unbased set differs. A pack is
   written only for a base the lock does not record, since the same
   composition writes nothing, so replacing it never touches a base in use.
5. **Attributes.** `.assemblr/.gitattributes` is
   `* -text -diff linguist-generated`: no line-ending conversion as before,
   `git diff` and `git show` print one line for the store instead of its
   bytes, and hosting services that read Linguist's attributes collapse it.
6. **The earlier layout is not read.** No promise covers alpha formats
   ([compatibility.md](../domain/compatibility.md)), and the owner chose
   not to keep a reader for them. A store that holds `objects/` or `bases/`
   and no pack for the base is refused as such, not as a missing base, so
   the diagnostic does not send the user to version control, where only the
   same layout is. The hint says how to move: in a project, remove
   `.assemblr/` and `assemblr-lock.json` and compose again, which marks
   every file changed since the last composition as a conflict; in a
   catalog, remove `.assemblr/` and build, which names each copy to take
   again with `unit amend`.

## Alternatives considered

- **Marking the earlier layout generated only.** One line, no format
  change, but the file tree and the file count stay as they were, which is
  what the owner found noisy.
- **One store file that grows.** Every composition changes the same binary
  file, so two branches that compose conflict in a file nobody can merge by
  hand.
- **Packs of the new blobs only.** Smaller files, but reading a base needs
  every earlier pack, and losing one breaks it. Git already stores a pack
  that changed little as a delta, so it saves nothing.
- **A compressed pack.** Doubles the repository's growth at every update,
  as measured above, and needs a compression dependency.
- **Not committing the base**, and regenerating it from the lock as Copier
  does. No noise at all, but it changes the approved contract (OC-8), and
  depends on every locked source and on the rendering staying reproducible
  forever; Copier documents that failure when a template was made for an
  older version
  ([documentation](https://copier.readthedocs.io/en/latest/updating/)).
- **Storing the base in Git refs or notes.** They do not travel with a
  clone or a fork by default, and Assemblr would have to write to Git.
- **Reading the earlier layout until the first beta.** A project or a
  catalog would move to a pack the next time its store was written, with
  no step from its author, at the cost of about 80 lines kept only for the
  alpha. The owner preferred the clean break: the known alpha stores are
  two repositories of the project, which can be converted once.

## Consequences

- A composition's pull request lists three files under `.assemblr/` in a
  project (`.gitattributes`, `.gitignore`, and the pack), and four in a
  catalog (with `amendment-bases.json`). An update changes one pack: one
  file removed and one added.
- A project or catalog composed by an earlier alpha must move once: its
  next composition or build with this version refuses until it does.
  Moving shows, in that pull request, as the removal of every earlier
  store file.
- A pending composition, committed while it waits, shows two packs until it
  completes or is aborted.
- `git diff`, and the diff `git show` prints for a commit, no longer print
  the stored base; reading the file still does.
- The base is read into memory whole, as the merges already needed it.
- ADR 0016 point 5 and ADR 0040 point 1 describe the earlier layout, which
  this decision replaces.
