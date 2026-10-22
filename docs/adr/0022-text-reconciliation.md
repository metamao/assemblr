# 0022: Text reconciliation: the line merge onto the project's file, and conflicts marked where they happen

- **Status:** Accepted
- **Date:** 2026-10-21

## Context

A reference scenario showed that a one-line edit by the consumer and a
one-line change by a new composition, in different places of one README,
produced a conflict marked as three whole copies (1,500 lines for a short
file), and that the same two edits in different places should not have been
a conflict at all. ADR 0017 left text as a conflict by design and said a
line-level merge could be added without changing the decision table; ADR 0020
said hunks could replace whole-file markers without changing the state
machine. The owner authorized both, on the engine of ADR 0021. The product
decisions are in the autonomous decision log (D-21).

## Decision

1. **Text is mergeable.** The adapter's mergeability for a path is
   `Format` only for the structured formats without an adapter (YAML, TOML,
   XML, INI, and JSON in another case, as in ADR 0021); JSON and JSONC keep
   the value merge; every other path is `Mergeable`. A file whose base is
   not stored is still `BaseNotStored`, and one whose versions are not UTF-8
   text still fails to merge and is refused whole. The decision table of
   ADR 0017 does not change.
2. **The merge is the domain's.** `adapters::project::lines` calls
   `domain::merge_text` with the project's file as `Current` and the
   composition's as `Incoming`, with insertions at one position as a conflict
   (`InsertTie::Conflict`): neither side may be placed before the other, so
   the unit-composition rule of ADR 0021, point 4, does not apply. A clean
   merge is written like a merged JSON file (`merged` in the report).
3. **Line endings.** A project file with CRLF line endings over an LF base
   and an LF composition is compared as LF, and what is written has CRLF
   line endings again; marker lines end as the file's lines do.
4. **Conflicts are marked where they happen.** A line conflict is a new
   `FileConflictKind::Lines`. When the composition would be left pending, the
   marked file holds the merged text with each conflict between markers: the
   project's lines, the base's, and the composition's, after the lines the
   three versions agree on at the start and the end of the disagreement are
   taken out. The file's other lines are the merge, so the composition's
   non-conflicting changes are applied to the project's side, which ADR 0020
   listed as a cost of whole-file markers. The state machine is unchanged:
   the pending record, `--continue` (no line starting with a marker), and
   `--abort` are as they were.
5. **Structured files keep whole-file markers.** For a JSON or JSONC value
   conflict, and for a structured format with no merge, the marked file is
   the three whole copies, as before. A line-level view of a JSON file whose
   values collide can merge the lines of other values into a result the
   value merge refused (a key twice), and marking only the colliding values
   would need the value merge to say where in the text each conflict is.
   That is not done; the cost is a noisy marked file for structured files.
6. **Testing.** Unit tests cover the reconciliation (different places,
   insertions at one position, CRLF, counted places) and the marker
   rendering (several places, a side without a final line ending, text that
   cannot be marked). Integration tests run the binary: a README both sides
   changed in different places (merged, no pending state), overlapping edits
   (only the disagreeing lines marked; `--continue` refused with a marker,
   then finalized; `--abort` restoring the consumer's edit), insertions at
   one position, CRLF merged and marked, and a YAML file still marked whole.

## Consequences

- A consumer's edits survive a new composition when they are elsewhere in
  the file, without a pending state, and a conflict is as small as the
  disagreement.
- The report says `merged` for a text file too; it does not say that the
  merge is by line, and a merged file is not checked for meaning, as no
  merged text is.
- `--abort` after a localized conflict restores the consumer's file whatever
  it holds, as before.
- A structured file both sides changed in different places is still a
  conflict (YAML, TOML) or merged by value (JSON, JSONC).
