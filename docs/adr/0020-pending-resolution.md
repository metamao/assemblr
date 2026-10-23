# 0020: The pending-resolution state: marked files, a journal that waits, and `compose --continue` and `--abort`

- **Status:** Accepted, the whole-file markers of point 3 narrowed to structured files by 0022, points 5 and 6 amended by #72, points 1, 2, 5, and 6 amended by #101
- **Date:** 2026-10-21

## Context

The second part of item 14 (#48) adds the pending-resolution state the
contract describes (`consumer-projects.md`, conflict resolution). Until now
a conflict refused the whole composition (ADR 0017). The product decisions
are in the autonomous decision log (D-18); this ADR records how the state is
represented, entered, finalized, and left, on top of the journal of ADR
0017.

## Decision

1. **Pending is a journal that waits.** `Journal` gains an optional
   `pending` record: the files that hold markers and the identity of the
   lockfile `--continue` writes, kept under `previous/` with the other
   transient bytes. The format number stays 1, and a journal without the
   record reads as before. The state is in `.assemblr/`, git-ignored with
   the rest of the journal, and the lock is untouched until `--continue`.
   *Amended (#101):* `pending` also records the blob identity of the
   `assemblr.jsonc` text the composition was planned from, which takes the
   consumer journal to format 3 (ADR 0027); formats 1 to 3 are read.
2. **Entering.** Reconciliation is planned as before. A conflict is turned
   into a change when it is a text conflict (a format with no merge, or the
   merge engine's value conflicts or its failure to merge) and the base, the
   project's file, and the composition's file are UTF-8 text without NUL;
   otherwise it stays a refusal, and a single refused conflict refuses the
   whole composition, so a project never mixes marked and refused conflicts.
   A marked file is a `modify` change like any other, with its previous
   content kept, so the existing steps (store, previous, journal, files,
   declaration) apply unchanged. After the declaration, the lock is kept,
   and the journal is rewritten with the `pending` record in one rename.
   Until that rename the journal is an ordinary one, so an interruption
   reverts everything, marked files included. *Amended (#101):* a file
   whose sibling amendments conflict (ADR 0032) reaches reconciliation as
   the composition's contribution, markers included, instead of refusing
   it; it is pending when the reconciliation writes it, or when the
   project's copy is kept because it already holds those markers (a clone
   of a waiting project). Nothing else about entering changes.
3. **The marked file** holds the project's content, the base's, and the
   composition's, each as it is, between `<<<<<<< current (your file)`,
   `||||||| base (the last composition)`, `=======`, and `>>>>>>> incoming
   (the declared composition)`. Marker lines end as the project's file does.
   The file's mode follows reconciliation: the composition's change to the
   executable bit wins, else the project's stands.
4. **Gate.** A composition that finds a pending journal whose composition
   the lock does not record refuses, naming the files and the two ways out.
   Anything else found in the journal is recovered as before. `add`,
   `remove`, and `compose` share the gate because they share
   `compose_with`.
5. **Continue** takes the project's lock, then: a lock that records the
   journal's base means an earlier continue was interrupted after writing
   it, so only the journal is removed; a journal without the record is an
   interrupted composition, which `compose` recovers, so continue refuses;
   otherwise every pending file is read through real directories only and
   must be absent (the consumer deleted it) or a regular file without a
   line starting `<<<<<<< current` or `>>>>>>> incoming`. *Amended (#101):*
   before the files, the declaration's bytes must hash to the identity the
   `pending` record holds, else continue refuses with the two ways out; and
   a line opened or closed by a unit's identity (`<<<<<<< @`, `>>>>>>> @`),
   or the `"<<<<<<< conflict"` member, counts as a marker too. Only
   Assemblr's own marker lines count (`layered::has_markers`), so a file
   that shows `<<<<<<< HEAD` in a code block is continued. Then the kept
   lock
   is checked to be the composition's own over a base that verifies, and is
   written and renamed into place as a composition's lock is; the journal
   and `previous/` go last. The unresolved files are all listed, and the
   project is unchanged.
6. **Abort** is the journal's revert with one difference: the pending files
   are restored whatever they hold, or whether they exist, since the
   consumer's edits to them are the resolution being given up. Every other
   entry follows the revert rules of ADR 0017 point 7, so a file the
   composition changed unmarked and the consumer edited since is kept and
   named. The declaration is restored through the locked handle.
   *Amended (#101):* the declaration counts among the pending files, so when
   the journal holds its previous content it is restored whatever it holds
   now, and the summary names it only then; and a pending file the
   composition created is removed whatever it holds, since a sibling
   conflict can now mark a `create` change. A journal
   without the `pending` record, and not completed by the lock, is an
   interrupted composition: abort refuses it as continue does, and
   `compose` recovers it.
7. **Exit status and reporting.** A composition that waits exits 1 after
   printing the counts and the paths, conflicts marked `conflict`, and says
   on stderr to resolve, then `--continue`, or `--abort`. The lock changes
   are not reported until the lock is written.
8. **Testing.** Fault injection covers a failure while recording the wait
   (reverted with the marked files), continue refused with a marker left and
   with the project unchanged, continue after resolving and after deleting a
   file, a continue that fails at the lock and one that fails after it, and
   abort over a half-resolved file. Integration tests run the binary from
   the marked conflict through refusals while pending, continue, and abort
   (the project byte-identical, the store only gaining the aborted base's
   objects), JSON value conflicts marked beside text ones, and the exclusion
   of the two flags.

## Consequences

- No new dependency, no Git involvement, and no change to the lockfile or
  the stored base.
- An aborted or interrupted composition leaves its base's objects in the
  store, as an interrupted one already did; collecting unreferenced objects
  is not done. *Amended (#72):* `--abort` now removes the aborted base's
  listing and the objects no other stored listing names
  (`Store::discard_base`), keeping the lock's base and, when a listing
  cannot be read, every object. An interrupted composition's revert still
  leaves them. `--continue` also refuses a line starting `||||||| base`,
  so a base section left behind by a partial clean-up is not finalized; the
  `=======` separator stays unchecked for the reason below.
- The markers cover the whole file, so the composition's non-conflicting
  changes to a marked file are not applied to the project's side: the
  consumer carries them over from the incoming section. This is the cost a
  line-level merge, or for JSON a marker around only the colliding values,
  would remove.
- A clone of a pending project has no journal and a lock that still records
  the previous composition, so its next `compose` finds the conflict again
  and wraps the marked file in a second set of markers. The pending state is
  not portable by design (the journal is transient, ADR 0017 point 6), and
  the second set is visible and must be removed before `--continue`. A file
  marked between siblings (#101) is deterministic, so the clone's copy equals
  the composition's and is kept, pending, without a second set; `--abort`
  in the clone has nothing to restore and leaves the markers, which the
  consumer resolves and continues.
- A line-level text merge can replace whole-file markers with hunks without
  changing the state machine: only `marked_change` produces the file.
- A file that legitimately needs a line starting with a marker prefix
  cannot be continued until that line changes; the check is on the two
  distinctive prefixes only, so Markdown heading underlines are fine.
