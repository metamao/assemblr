# 0015: The value-level merge of several amendments: the domain engine, the JSONC editor, and the projection and round-trip checks

- **Status:** Accepted
- **Date:** 2026-10-21

## Context

The owner decided OC-10 for unit composition on #33: several amendments of
one JSON or JSONC file are combined by a value-level N-way merge, with the
rules recorded in
[`amendments-and-composition.md`](../domain/amendments-and-composition.md#the-merge-engine--approved).
The increment is #38. This ADR records how the engine is built: where the
merge lives, how documents are represented, how the edited text is produced
without a concrete-syntax-tree library, and which checks make a hand-written
editor safe to ship. The research prototype (`system-design`, EXP-10, engine
D) is evidence for the approach, not code that was adopted.

## Decision

1. **The merge is domain code; text is an adapter's.** `domain::merge`
   defines the document model (`Value`: null, booleans, numbers as canonical
   text, strings, arrays, and objects as ordered members), the operations
   (`Ensure` an object, `Add`, `Change` with the expected old value,
   `Remove`), and the three pure steps: `derive` (base to copy), `combine`
   (every amender's operations, equal ones shared, different ones a
   conflict, an edit inside a removed or replaced value a conflict), and
   `plan` (the operations checked against the provider's file and turned
   into `Edit`s plus the value the result must hold). `adapters::jsonc_edit`
   reads text into the model and applies edits to text;
   `adapters::merge::merge_json` orchestrates and checks. `compose` stays
   pure: it carries every amender's copy in the composed file for JSON and
   JSONC (recognized by the delivered path's extension, a pure rule), and
   keeps refusing other formats. The build renders each side and calls the
   merge, since the contract merges rendered content and rendering is an
   adapter.
2. **Numbers.** A number is its canonical JSON text: the JSONC reader's
   visitor keeps 64-bit integers as before and now keeps other numbers as
   the shortest text `serde_json` prints for them, so `1.50` and `1.5` are
   one value and a copy with a float round-trips. Values compare by text.
3. **Objects only have paths; lists are values.** A path is a sequence of
   member keys. A list is compared whole: the owner excluded identity
   inference, so two amenders changing one list differently conflict and
   identical changes merge. A copy that adds a non-empty object yields an
   `Ensure` plus one `Add` per member, so two amenders creating the same
   object share it, as the contract's flat model requires; a value one
   amender added is not in another's base, so a second amender setting it
   differently is a conflict of two additions.
4. **Determinism.** Amenders are keyed by identity; operations are planned
   parents first, then by anchor, first amender, and key; conflicts are
   sorted. Members added after one anchor by several amenders are chained
   in amender order, each inserted after the one before it, so the editor
   needs no position arithmetic and the result does not depend on the order
   the copies were read in.
5. **A hand-written JSONC editor on the existing scanner.** `jsonc-parser`,
   the prototype's CST, needs Rust newer than the MSRV (ADR 0002). The
   span scanner of `adapters::jsonc`, proven by `declare_amendment` and
   `rebase_amendments`, locates members in the comment-blanked text whose
   offsets are the original's. The editor applies one edit at a time and
   rescans, since later edits name members earlier ones inserted; it
   follows the file's indentation unit, line endings, single-line objects,
   and trailing commas, and renders inserted values with that unit. It
   never reads a comment as content, and it trims a comma that a removal
   would leave trailing unless the file already used trailing commas.
6. **Two checks around the engine.** The **projection check** reads the
   edited text back and requires exactly the planned value, so a layout the
   editor misjudged is a refusal naming the file, never a silently different
   file. The **round-trip guard** replays each amender's own operations on
   its base and requires its copy's value, member order aside; comments the
   copy adds and member orders it changes, which the model has no operation
   for, are reported as warnings naming the amender. Both run on every
   merge.
7. **Diagnostics.** A conflict names its path (`/scripts/dev`), each change
   with the amenders that make it, and, for an unexpected value, what the
   provider's file holds. The build reports every conflict of a file
   together, after every other problem the composition found, and the hint
   says to make the copies agree or expose the value as a parameter.
8. **Testing.** Domain tests cover derive, combine, and plan on a
   `package.json`-shaped document; editor tests cover insertion first, after
   an anchor, into an empty object, on one line, with JSONC comments and
   trailing commas, replacement, removal of middle and last members, and
   CRLF; merge tests cover independent copies, conflicts, notes, and
   unreadable sides. Integration tests run the binary against repositories:
   two published amenders and the built unit on `package.json` and a
   templated `wrangler.jsonc` with `own` values, identical and different
   contributions, the flat model, removals against edits, whole-list
   changes, comments and member order as warnings, declaration and
   registration order swapped with identical bytes, YAML and invalid copies
   refused, CRLF, and the single-amender path unchanged. CI runs them on
   Linux and Windows. No dependency was added.

## Consequences

- The build's cost grows by a parse of each copy and of the result for a
  file with several amenders; files are small.
- A copy's comments and member order are not carried into the composed
  file, and the author is told so. Comments that should ship belong in the
  provider's file.
- Widening the engine to TOML or YAML means a reader to `Value`, an editor,
  and the same two checks; the domain steps do not change. Cross-version
  application (OC-52) and update reconciliation (OC-53) reuse `derive`,
  `combine`, and `plan` against another target.
- The editor's layout rules are heuristics guarded by the projection check:
  a file whose layout they misjudge is refused when the edited text would
  not hold the planned value, and the refusal names the file; a misjudged
  indentation that keeps the value yields valid but less tidy JSON.
- `unit amend` no longer refuses a JSON or JSONC file that a locked
  dependency already amends (ADR 0011, point 6, narrowed to the other
  formats): it creates the copy and names the other amenders, since the
  build now combines the copies.
