# 0024: `assemblr update`: resolving with a reduced lock, the lock as a record of the resolution, and one reconciliation

- **Status:** Accepted
- **Date:** 2026-10-22

## Context

A consumer had no way to move locked versions forward within the
requirements it already declared: `compose` keeps the lock, and `add` changes
a requirement. The owner decided OC-38 on #63 (decision log D-23): an
explicit `assemblr update [<identity>...]` that never edits the declaration
and reconciles like `compose`. This ADR records how it is built on the
composition of ADR 0016, 0017, 0019, and 0020.

## Decision

1. **One pipeline.** `update_units` is `compose_with` with a list of named
   units and no declaration edit, not a second composition. Gate, recovery,
   verification of the stored base, rendering, the per-path three-way
   decision, the journal, the pending state, and the report's file lines are
   those of `compose`, so a conflict is left in the file, `--continue` and
   `--abort` work, and an interrupted update is recovered by the next
   `compose`. The command adds no write step and so no new fault-injection
   point.
2. **Resolution.** The resolver is given the lock minus the named units, or
   an empty lock when none is named, as `unit update` does (ADR 0012). The
   other locked versions keep precedence and move only when a requirement of
   the new ones forces them. The command is refused for a project that has no
   lock and for a name the lock does not hold.
3. **The changes are told against the whole lock.** `update_lock` runs on the
   resolved graph with the complete previous lock: it reports every version
   that changed, including those the resolver was never told about, and it
   still requires every kept version to be published with the content it was
   locked to (the integrity check the reduced lock would skip). `compose` and
   `add` report their changes the same way, so the two never disagree.
4. **The lock records the resolution.** The early return "the project already
   holds the declared composition" now requires the resolved lock to equal
   the previous one. A resolution that changed versions or provenance without
   changing a file is written (store, journal, lock) like any other
   composition. This also fixes a case that predates the command: a
   requirement edited to a version that delivers identical files left a stale
   lock.
5. **Reporting.** `update` prints `Updated: a 1.0.0 -> 1.1.0, ...` on stdout
   with the file lines, and `Nothing to update: ...` when nothing moved and
   nothing was touched. A unit that moved without being named is listed on
   stderr with the reason (the versions selected for the named units need
   it), so a targeted update is never silently widened.
6. **Diagnostics.** Naming a unit with a requirement is refused with the way to
   change a requirement (`add unit@req`). A cross-version collision uses the
   hints of ADR 0023.
7. **Testing.** Integration tests run the binary: the whole graph and the
   declaration left byte-identical, a newer version outside the requirement
   staying out, nothing to update, a lock-only change, a named unit with a
   forced move reported, a transitive name, unknown names and a project never
   composed, an edited file merged and a conflict aborted and continued,
   refusal while pending, amendments applied across versions, and a collision
   refusing the whole update.

## Consequences

- `update` and `compose` share every guarantee and every failure mode of
  reconciliation; there is one place to fix.
- A consumer can take a provider release without editing the declaration, and
  a requirement that is too tight shows as a unit that does not move, which
  `add unit@req` widens.
- The lock can change without the base changing. Nothing else records that a
  composition was re-resolved, so the lock is the record.
- `--latest`, `--dry-run`, and automatic updates are not built (the owner's
  decision).
