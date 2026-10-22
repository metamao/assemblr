# 0021: The line-level merge of text: the domain engine, which files it covers, and what it reports

- **Status:** Accepted
- **Date:** 2026-10-21

## Context

A reference scenario built from four units found that two units which each
add a section to a shared guidance file (`AGENTS.md`) could not be composed:
several amendments of one file were combined for JSON and JSONC only
(ADR 0015). The owner then authorized a conservative, deterministic
line-level merge of text, for unit composition first and for consumer
reconciliation next (ADR 0022), without implicit precedence, silent loss, or
weaker provenance. The product decisions are in the autonomous decision log
(D-20); this ADR records how the engine is built and where it applies.

## Decision

1. **The engine is domain code and pure.** `domain::text_merge` merges the
   sides, each a text derived from one base, and returns segments of decided
   lines and conflicts. It takes strings and returns data. The adapters read
   and write text and turn conflicts into diagnostics.
2. **Changes are hunks.** Each side is compared line by line with the base,
   a line keeping its terminator, so `a\r\n` and `a\n` differ and a last line
   without a terminator is not the same line as that line with one. A
   longest-common-subsequence comparison on the lines between the common
   prefix and suffix gives the hunks. It aligns repeated lines the same way
   for the same input, and a middle section whose table would pass four
   million cells is one hunk, which is coarse but never wrong: it can only
   make more changes collide. No dependency was added.
3. **Clusters decide.** Hunks of different sides that overlap or touch form a
   cluster, so changes to adjacent lines are not independent, as in Git. A
   cluster of one hunk, or of identical hunks, is that change. A cluster of
   insertions at one position is chained when the caller asks: each distinct
   insertion once, in the order of the sides. Any other cluster is a conflict
   holding the base's lines and each side's, after the lines every side
   agrees on at both ends are taken out. If the sides hold the same lines
   after all, it is not a conflict.
4. **Unit composition chains; reconciliation will not.** Two units appending
   to one file is the case the merge exists for, and their order is a layout
   that loses nothing; the order is that of the units' identities, as for
   members added to a JSON object (ADR 0015, point 4), and every chained
   position is a warning at `unit check`, `unit build`, and `compose`. Between
   a consumer's file and the composition (ADR 0022) neither side may be
   placed before the other, so the same position is a conflict. This is the
   only difference, and it is a parameter of the engine, not two engines.
5. **Which files.** The format is the delivered path's extension (ADR 0015
   point 6 recognizes it the same way): `.json` and `.jsonc` keep the value
   merge; `.yaml`, `.yml`, `.toml`, `.xml`, `.ini`, and JSON in another case
   are structured formats whose lines a merge cannot tell apart from data,
   such as a key twice, so they stay refused; every other path is text. A
   file or a copy that is not UTF-8 without a NUL is refused too. The
   composer checks this on the contents it has, so the refusal is a
   composition error before any rendering, and `unit amend` applies the same
   rule when it decides whether to create a copy of a file a dependency
   amends.
6. **Guard.** For each amender, the merge of that copy alone against the base
   must give the copy back byte for byte, as the JSON merge replays each
   amender's operations. A mismatch is a defect in the merge and rejects the
   build.
7. **Diagnostics.** A conflict names the file, the lines of the provider's
   file it covers (`line 8`, `lines 7-9`, `after line 12`), and for each
   amender what it adds, sets, or removes there. The hint says that no
   amender takes precedence and what to change.
8. **Testing.** Domain tests cover single sides, independent and identical
   changes, adjacency, insertions at one position, trimming, missing
   terminators, repeated lines, and the size limit; a deterministic random
   test checks that one side returns exactly, that an unchanged side or a
   repeated change changes nothing, and that the order the sides are given in
   never matters. Integration tests run the binary on units shaped like the
   reference scenario: sections appended by two units, edits in different
   places, the unit's own amendment with a published one, a conflict, equal
   changes, rendered templates, CRLF, a consumer composition, structured and
   non-text refusals, and `unit amend`.

## Consequences

- Units can extend one prose file independently. A unit that edits a line
  another unit edits, or the line next to it, must coordinate or move its
  text to a file of its own.
- The merge cannot see meaning. Two units may add lines that are each fine
  and wrong together (a duplicated setting in a file whose format has no
  adapter). Structured formats are refused for that reason; for other text,
  `unit check` reports chained positions, and native validation on `build/`
  remains the author's (authoring-workflow, native validation).
- A text file can no longer be a way to say "only one unit may touch this".
  A unit that wants that owns the file and no other unit amends it.
- Adding TOML or YAML later is an adapter with a reader, a writer, and the
  checks of ADR 0015; the line merge stays for the formats that have none.
