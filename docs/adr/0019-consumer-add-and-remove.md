# 0019: `add` and `remove`: editing the declaration, the default requirement, and writing it through the locked handle

- **Status:** Accepted
- **Date:** 2026-10-21

## Context

The second part of item 14 (#43) adds `assemblr add` and `assemblr remove`.
The product rules are in
[`consumer-projects.md`](../domain/consumer-projects.md#add-and-remove-details--provisional-d-17)
and the decisions behind them in the autonomous decision log (D-17). Both
commands change the declaration and then compose it, and the declaration is
also the handle that serializes compositions (ADR 0016 point 8). This ADR
records how the declaration is edited, how the default requirement is found,
and how the write fits the journal of ADR 0017.

## Decision

1. **One pipeline.** `compose_with` takes an optional `DeclarationEdit`.
   After the stored base is verified and before resolution, the edit is
   planned against the declaration text read through the lock handle:
   the edited text, the project it declares, and what changed. Resolution,
   composition, rendering, reconciliation, and the ignore rules then run on
   the edited project exactly as for `compose`, so the two cannot drift.
   Nothing is written until all of it has passed.
2. **The text is edited, not regenerated.** `declaration` uses the JSONC
   editor of ADR 0015 with `Insert`, `Set`, and `Delete` edits, so
   comments, indentation, and line endings stay. A new selection goes after
   the last one. The edited text is read back and must equal the value the
   edit meant, so a layout the editor misjudges is a refusal, as for a
   merge.
3. **The default requirement is found by resolving.** Without a
   requirement, a probe resolves the project with the unit selected as `*`
   (any stable version) under the existing lock, and records `^` plus the
   version the lock then holds for the unit. A unit in the composition
   already keeps its locked version under the lock's precedence, which is
   what promotes it without changing a file. A unit that is selected
   already skips the probe and keeps its own requirement.
4. **The declaration is written in place, through the handle.** A rename
   would put another file where the declaration was and leave the lock on
   one that is no longer it, so a process waiting on the lock would read the
   old text. `write_in_place` truncates and writes through the handle that
   holds the lock, which `compose_with` now opens writable where the file
   system allows. It is not atomic, so the journal covers it.
5. **The journal records it.** The declaration is one more `modify` entry,
   with its planned content and its previous text kept under `previous/`.
   It is written after the files and before the lock, so a lock that records
   the journal's base proves the declaration was written, and a revert
   restores it through the handle. Recovery reads the declaration through
   the handle too, since Windows enforces the lock against other handles. A
   declaration edited by hand during an interruption is kept and named like
   any other file.
6. **A composition that changes nothing but the declaration still
   writes.** The early return for an unchanged composition applies only
   when the declaration does not change either; otherwise the store, the
   journal, the declaration, and the lock are written, and the report says
   that the composition did not change.
7. **Testing.** `declaration` has unit tests for adding, replacing, and
   removing in commented, empty, and incomplete declarations. Fault
   injection covers the declaration step, the lock, and the end, and an
   edit made during an interruption. Integration tests run the binary:
   adding with and without a requirement, promotion, replacement, every
   refusal leaving the project byte-identical, removal with options,
   retirement of files, an edited file refusing a removal, and the
   convergence of a sequence of adds with one `compose`.

No dependency was added.

## Consequences

- `compose_with` opens the declaration writable when it can, so a read-only
  declaration is still composed, with a read-only handle. An `add` or
  `remove` on one is refused before anything is written.
- The declaration is not written atomically. An interruption inside the
  write is the one case where the journal's previous text is what restores
  it, which the next `compose` does before anything else.
- A composition that adds a unit contributing no files and no lock change
  beyond its own entry keeps the pre-existing behavior of rewriting the lock
  only when the declaration changes, which an `add` always does.
- The probe resolves the graph twice for an `add` without a requirement.
  Fine for Alpha.
