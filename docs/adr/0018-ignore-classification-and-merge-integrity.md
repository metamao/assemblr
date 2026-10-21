# 0018: Which ignore rules decide the lock, and what a failed merge check is

- **Status:** Accepted
- **Date:** 2026-10-21

## Context

An owner-requested review of the decision log (#34, tracked in #45) found
two places where an earlier decision did not hold up against the code or
the contracts. ADR 0016 point 6 and ADR 0017 point 8 asked Git, with the
consumer's own configuration, which delivered files the project ignores, and
the lock recorded the answer as the `unbased` set. The answer included
`.git/info/exclude` and the user's global excludes, which belong to one
machine, so the committed lock could differ between two clones, and a file
one developer ignores could enter the tracked base on another's machine. The
round-trip guard of ADR 0015 point 6 required an amender's own operations to
reproduce its copy, but the implementation only warned when they did not.

## Decision

1. **The lock follows the project's rules.** `git check-ignore --verbose -z`
   names the file of the pattern that decides each path. Git's precedence
   makes the split exact: a `.gitignore` match wins over `info/exclude`,
   which wins over the global excludes, and a `!` pattern re-includes. A
   path whose deciding pattern is in a `.gitignore` the repository holds
   (a relative path, not under `.git/`) is unbased. Nothing else is, so the
   same project locks the same way on every machine. An uncommitted edit of
   a `.gitignore` still counts for the machine that made it, as it does for
   the build's reading of `src/` (ADR 0008).
2. **The consumer's own rules are respected, not applied.** A delivered file
   that only a local rule ignores, and whose content the base would store for
   the first time, is refused before anything is written
   (`ComposeProblem::LocallyIgnored`), naming the file, the pattern, and its
   file. Storing it would put into the tracked base what the consumer kept
   out of version control; leaving it unstored would lock the project
   differently here. The diagnostic names the ways out: a `.gitignore` rule
   (unbased for everyone), a `!` pattern there (stored for everyone), or
   removing the local rule. A file the base already stores, or that was
   stored before, is not refused when a local rule appears later: nothing new
   enters the store.
3. **Transitions are announced.** A path that was unbased and is no longer
   ignored by the project gets a note that its composed content is stored
   from now on. Only the incoming composition's bytes ever enter the store,
   never the consumer's edits, which go to the project or to the ignored
   `previous/`.
4. **Existing locks keep working.** The lock format is unchanged, and the
   set was already recomputed at every composition that writes, so a lock
   recorded under the old rule corrects itself at the next one. A project
   whose old lock depended on a local rule is refused once, with the way out.
5. **A failed round-trip is an error.** `round_trip` replays an amender's
   operations on its base and requires its copy's value, member order aside.
   A mismatch, or operations that cannot be replayed, is
   `MergeFailure::RoundTrip` naming the amender: it is a defect in the
   derivation, and composing with it would write a file the copy did not ask
   for. Comments and member orders a copy changes stay warnings, because the
   value model leaves them out on purpose. This clarifies ADR 0015 point 6,
   which already required the value.
6. **Early and complete notes.** `unit check` and `unit build` report, for
   the unit's own JSON and JSONC copies, what a second amender would make the
   merge drop (`unshared_notes`), rendering the base quietly so that a base
   that does not render adds no new problem. A merged consumer file reports
   the composition's member orders and comments that its text does not carry.

## Alternatives considered

- **Read the same rules as the build, `.gitignore` files only, and ignore
  local rules.** Deterministic and simple, but it discards the consumer's
  safety preference: a locally ignored `.env` would be stored without
  warning. The review asked not to do that.
- **Treat a local-only file as unbased on this machine.** The lock would
  differ between clones, which is the defect.
- **Store it and warn.** Quiet failure of the same preference.
- **Run Git against a scratch Git directory to evaluate `.gitignore` files
  alone.** Heavier and tracked files would no longer count as tracked.
  The verbose output gives the same answer from one read-only call.

## Consequences

- A consumer whose global excludes name a path a unit delivers meets a
  refusal on the first composition, once, and settles it in the project.
- The classification relies on the verbose output of `git check-ignore`,
  which the minimum supported Git version (OC-43, still open) must provide.
  The parser skips records it does not understand, and a unit test covers
  the shape.
- Order-sensitive JSON, such as Node's `exports`, is not changed by this ADR.
  The invented order between two units' insertions at one anchor is OC-12.
  The owner decided to keep the generic behavior and record it as a known
  limitation; the entry stays open and is revisited with real projects.
