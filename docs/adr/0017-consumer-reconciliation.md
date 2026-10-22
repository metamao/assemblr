# 0017: Consumer reconciliation: the per-path three-way decision, the value merge onto the project's text, the journal of changes, and recovery

- **Status:** Accepted, point 8 revised by 0018, the refusal of text conflicts replaced by 0020, text merged by line by 0022
- **Date:** 2026-10-21

## Context

Item 14 of the trajectory (#43) evolves a composed project: a changed
declaration is reconciled against the stored base and the project's
current files. The product rules are in
[`consumer-projects.md`](../domain/consumer-projects.md#reconciliation-details--provisional-d-13-to-d-15).
The owner's merge-engine decision (ADR 0015) left the consumer side of
OC-10, and Git's part, to this item. The research in `system-design`
(EXP-10, EXP-11) compared Git's `merge-tree`, index stages, a private
ref, and a lock-based design with a per-path three-way over 24 lifecycle
scenarios: the per-path designs passed every scenario, including a project
with no repository, Git-backed bases were lost after an ordinary undo or
merged two-way in clones, `merge-tree` added only a rename guess, and
Git's line merge was never unsafe but produced false conflicts. This ADR
records how reconciliation is implemented.

## Decision

1. **The decision is domain data.** `domain::reconcile` takes the base
   and the incoming composition as snapshots (mode and content identity
   per path), what the project holds at the base's paths, and whether a
   path could be merged, and returns an `Action` per path (create, write,
   mode, merge, remove, keep) or a `Disagreement`. Content and mode are
   each decided three-way: a side equal to the base yields, equal sides
   agree, two different changes merge or disagree. Paths the base lacks
   are created whatever the project holds there; the adapter refuses
   existing content as the first `compose` does.
2. **The merge engine is generic over who contributes.** `Combined`,
   `Conflict`, `ConflictKind`, `combine`, and `plan` take a contributor
   type, defaulting to `UnitIdentity` so that unit composition is
   unchanged; reconciliation uses `Contributor::{Current, Incoming}`.
   `merge_values` derives each side's operations against the base,
   combines them, which finds the colliding changes, and plans only the
   operations the composition alone makes against the project's value: the
   shared and the project's own changes are already in its text. The
   adapter's `reconcile_json` edits those into the project's text with the
   JSONC editor and checks that the text parses back to the planned value.
3. **Reading the project.** Only the base's paths are read, and only
   through real directories: a symbolic link on the way makes the path
   "not a regular file", so that neither the read nor a later write
   leaves the project. The incoming composition's new paths go through
   the existing-content check. The
   executable bit comes from the file system on Unix and is the base's
   elsewhere, since the consumer cannot have changed it there. A file whose
   bytes are not the base's or the incoming content, but are one of them
   once CRLF line endings are read as LF, compares as that content and is
   marked so that what is written to it gets CRLF line endings; the JSONC
   editor keeps a document's line endings by itself, so a merged file
   needs no conversion.
4. **Mergeability** is decided per path: a base that is not stored (the
   path was unbased) comes first, then the format, so that the consumer is
   told the reason they can act on.
5. **Writes.** Every file, created or replaced, is written to
   `<name>.assemblr-new` beside its path and renamed into place, so that a
   path holds either its previous or its whole new content. A removed
   file's empty directories are removed, innermost first, never the
   project's root. The sequence is: store the base; keep the previous
   content of every file to replace or remove under
   `.assemblr/previous/<blob>`; write the journal; create directories;
   create and replace files in path order, then remove files, since a
   removal prunes directories a created file may need; write the lock;
   remove the journal and `previous/`. Fault injection covers each step.
6. **The journal** keeps format 1. Each entry gains an optional `action`
   (`create`, `modify`, `remove`), and `modify` and `remove` entries the
   previous content's identity and mode; an entry without an action is a
   creation, so journals written before this ADR are still read.
   `.assemblr/.gitignore`, written and restored like `.gitattributes`,
   lists the journal, its staging file, and `previous/`: the consumer's
   edits never enter the tracked store, and a clone of an interrupted
   project carries no journal, so its files are reconciled as the
   project's content.
7. **Recovery.** A lock that records the journal's identity completes the
   composition (ADR 0016, point 7). Otherwise each entry is reverted: a
   created file holding the planned content is removed; a replaced file
   holding the planned content is restored from `previous/` with its
   previous mode, and one holding the previous content is left; a removed
   path that is empty gets its previous content back. Anything else is
   kept and named. Stale staging files are removed.
8. **Ignore rules** are read at every composition that writes, for the
   incoming paths,
   so the lock's `unbased` set follows the rules in force; only newly
   unbased paths are announced. A path that was unbased has no stored
   base, which makes a two-sided change to it a conflict by identity.
9. **Reporting.** A reconciliation reports the counts and the paths
   created, modified, merged, and removed, the lock changes, and notes for
   a deleted file left deleted and for CRLF line endings. A first
   composition reports as before.
10. **Testing.** Domain tests cover the decision table and the value
    merge; fault-injection tests run `apply` and `recover` at each step of
    a first composition and of a reconciliation, including files edited
    meanwhile and a journal without actions; integration tests run the
    binary through an evolving declaration (a unit added with its
    amendment and option, removed again, every unit removed), every kind
    of conflict with the project byte-identical afterwards, CRLF line
    endings, and unbased files, including the ignore rules changing
    between compositions.

## Consequences

- Reconciliation needs no Git in the project and no Git merge driver;
  the consumer's Git configuration cannot change its result.
- A text file both sides changed is a conflict. A line-level three-way
  merge for text can be added later without changing the decision table:
  it would make `Mergeability::Format` mergeable for text.
- `.assemblr/.gitignore` is a tracked file of the consumer's project, like
  `.gitattributes`.
- A conflict between text stops the whole composition only until ADR 0020;
  other conflicts still do, and the consumer resolves them by making the
  file the composed content, restoring it, or changing the declaration.
