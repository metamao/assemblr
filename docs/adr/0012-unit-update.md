# 0012: `unit update`: resolving with a partial lock, rewriting bases in place, and the update journal

- **Status:** Accepted
- **Date:** 2026-10-21

## Context

The first `unit update` increment (#30) makes updating a dependency an
explicit author action: the locked versions change, and the unit's
amendments follow their providers when that needs no reconciliation rule.
The product rules are in
[`authoring-workflow.md`](../domain/authoring-workflow.md#update-details--approved).
This ADR records how they are implemented: how the resolver is told which
units to update while the rest of the lock keeps precedence, how an
amendment's carry-over is decided, how the manifest's declared bases are
rewritten without losing comments, and how an update that replaces two
files is made recoverable without ever leaving them half updated.

## Decision

1. **Where the code lives.** The rules are domain code, in
   `domain::update`: `update_lock` computes the lock after a resolution
   and how it differs from the previous one; `reconcile_amendments`
   classifies each amendment whose provider moves; and
   `published_amenders_at_base` checks a resolved graph for a published
   amendment composed away from its base. `relock`, which builds and
   amendments use, now delegates to `update_lock`. The command is
   `adapters::build::update`, beside the build and `unit amend`, because it
   reads the unit, the registry, and the dependencies exactly as they do and
   must hold the same lock (ADR 0008, point 5).
2. **Resolving with a partial lock.** The resolver is given the previous
   lock minus the identities the author updates, all of them when none is
   named, so that the provisional policy of
   [`dependencies.md`](../domain/dependencies.md#recommended-provisional)
   keeps every other locked version that still satisfies the requirements
   and replaces one only when an updated unit's requirements force it. The
   resolver reports changes relative to the lock it was given, so an updated
   unit that resolves to its locked version again would appear as newly
   added, and the content check that `relock` runs for kept versions would
   skip it. `update_lock` therefore computes both the reported changes and
   the kept-content check against the **whole** previous lock, as the owner
   required: a version that stays is kept, and must still be published with
   the content it was locked to, whether or not the resolver was told about
   it. A named identity must be locked or declared as a dependency by the
   unit or by a locked version, so that a dependency added to the manifest
   since the last build can be named before it is locked.
3. **Carry-over by bytes.** For each amendment whose provider moves from the
   declared base `B` to the selected version `N`, the provider's `src/` is
   read at both versions, by the commits the index records and under the
   ADR 0006 bounds, and the file at the amended path is compared byte for
   byte. Equal bytes carry the amendment over: the copy is untouched and the
   declaration's base becomes `N`. Anything else refuses the update with the
   cause: the file differs (reconciliation, OC-10 and OC-53), the path is
   missing at `N`, the path is missing at `B` (the amendment was invalid
   already), `B` is not indexed (the usual "synchronization needed"), or the
   declaration's base is not the locked version (a hand edit, whose real
   base is unknown). The executable bit is not compared: a composition takes
   it from the provider's file at the resolved version, so a changed bit
   flows through on its own. Template selection is by path, and the path is
   the same at both versions.
4. **Published amenders on the new graph.** Every manifest a build could
   read is already in hand (`Dependencies::read` reads each reachable
   version's manifest), so the check walks the resolved graph and each
   resolved version's declarations, and reports every provider that does
   not resolve to the declared base. Because it looks at the graph the
   update would lock, not at the previous lock, updating an amender to a
   version that amends the provider at the newly selected version passes.
5. **Rewriting bases in place.** The scanner of ADR 0010 (`jsonc::members`)
   locates the `amendments` member, the declaration for each carried path,
   and its `base` member; the value span of each `base` is replaced by the
   new version's JSON string, from the end of the text backwards so that
   earlier offsets stay valid. The edited text is read back and must equal
   the manifest with exactly those bases (`Manifest::with_bases`); otherwise
   nothing is written and the error carries the members to replace by hand.
   As in ADR 0010, the editor assumes only what the parser has verified and
   gives up on any surprise.
6. **The update journal.** An update replaces two files, and the two cannot
   be replaced atomically together. Retrying the command is not a sufficient
   recovery: a retry recomputes the plan, and a registry synchronized or an
   argument changed in between would select other versions than the
   interrupted update had decided, while the files might already hold half
   of that decision. The update is therefore an explicit transaction:
   - Before either file is touched, `assemblr-update.json` in the
     repository's Git directory records the **full contents** of both files
     before and after the update, the manifest only when an amendment is
     carried over. Full contents rather than hashes keep the record
     self-contained and need no new dependency; the record is bounded by the
     manifest and lockfile limits. It is written to `assemblr-update.json.new`
     beside it, flushed, and renamed into place, so it either exists complete
     or not at all. The Git directory is the home of the build lock and the
     build's interruption record (ADR 0008): Git ignores it and never shows
     it as untracked, so the record cannot be committed by accident.
   - Then the manifest, if planned, is written to
     `.assemblr-manifest.jsonc.new` (created new, flushed) and renamed over
     `manifest.jsonc`; then the lockfile the same way through
     `.assemblr-lock.json.new`; then the record is removed. The manifest
     goes first so that the lockfile, which a build trusts as the identity
     of each base, changes last.
   - A failure after the record is written is reported with the files
     already replaced, and the record is left: the next `unit update`
     completes the update. An interruption leaves the same state.
   - While the record exists, `unit build` and `unit amend` refuse to run,
     pointing at `unit update`. They hold the build lock when they look, as
     the update does when it writes, so they never see a half-written
     record.
   - Completion compares each file with its recorded contents. The states
     the commit order can leave are: nothing replaced, the manifest only,
     or both. In each, a file that still has its "before" content is
     replaced with its "after" content, through the same temporary file and
     rename, and a file that already has its "after" content is left. Any
     other combination means someone changed a file since the update
     started: nothing is written, the record stays, and the diagnostic says
     to restore the file to either recorded content, or to delete the record
     to abandon the update with the files as they are. A record that cannot
     be parsed is reported and kept. Completing applies exactly the plan the
     interrupted update had decided and reported nothing about; the command
     then says what it replaced, and goes on with the author's request.
   - Stale `.new` files beside the manifest or the lockfile are the
     interrupted update's own while its record exists, and are replaced;
     without a record they are someone else's, and `unit amend` and
     `unit build` report them as before.
7. **Testing failures.** The write sequence takes an internal hook, as the
   build's publishing does (ADR 0008, point 8), so that unit tests fail each
   step in turn and check both the reported state and that completion
   restores the planned outcome. Integration tests construct the record on
   disk for each state an interruption can leave, including a manifest
   edited meanwhile, and run the real binary.

## Consequences

- `unit update` is the second writer of the lockfile; the remaining open
  question of OC-46 is `unit publish`.
- An update writes at most two files and never touches `src/` or
  `amendments/`; a copy is carried over, never modified.
- A dependency whose amended file changed between the two versions cannot
  be updated until reconciliation is decided (OC-10, OC-53). The author
  keeps the locked version or narrows the range.
- The Git directory gains a third reserved name, `assemblr-update.json`,
  with its `.new` staging file.
- The build's diagnosis of a declaration that disagrees with the lock as a
  hand edit is correct again: the state an interrupted update leaves is
  detected by the record before a build reaches that check, unless the
  record was deleted by hand, which the diagnostic for that case explains.
- The journal holds the lockfile twice, which, with the build reading each
  published amender's lockfile in full (ADR 0011), is one more reason to
  keep lockfiles small until `unit publish` validates them (OC-46).
