# 0020: The pending-resolution state: marked files, a journal that waits, and `compose --continue` and `--abort`

- **Status:** Accepted
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
   reverts everything, marked files included.
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
   line starting `<<<<<<< current` or `>>>>>>> incoming`. Then the kept lock
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
  is not done.
- A line-level text merge can replace whole-file markers with hunks without
  changing the state machine: only `marked_change` produces the file.
- A file that legitimately needs a line starting with a marker prefix
  cannot be continued until that line changes; the check is on the two
  distinctive prefixes only, so Markdown heading underlines are fine.
