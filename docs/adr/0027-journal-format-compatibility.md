# 0027: Journal formats: a number for every change, and readers that refuse what they do not know

- **Status:** Accepted
- **Date:** 2026-10-22

## Context

Two journals carry recovery state between Assemblr runs. The consumer
journal (`.assemblr/journal.json`, ADR 0016 and 0017) records a composition
being applied, and since ADR 0020 a composition waiting in the pending state.
The unit-update journal (`assemblr-update.json` in the Git directory,
ADR 0012) records a `unit update` being written. Both kept format 1 while
they gained members: `pending` (ADR 0020) and `copies` (ADR 0023). The
consumer journal's reader ignored members it did not know, so a version
from before ADR 0020 would read a waiting composition's journal as an
interrupted one and revert it, discarding the consumer's resolution. No
such version has been distributed yet. The first distributed binary (#68)
is where this must stop (#73, from the second decision-log review).

## Decision

1. **Every change to a journal's members takes a new format number.** A
   member a reader does not understand can change what recovery must do, so
   an optional member is a format change too.
2. **A reader refuses a format number it does not know and a member it does
   not know**, at every level (the journal, a file entry, `pending`,
   `copies`), and does nothing with the journal: no completion, no revert.
   The project or repository is left as it is, and the diagnostic names the
   journal and what is unknown. For the consumer journal it is
   `StoreError::Journal`, with a hint that a newer version may have written
   it and should finish it, instead of a base that does not verify. A
   journal that is damaged (not JSON, or a member missing or invalid) is
   not one a newer version wrote and stays `StoreError::Corrupt`. The
   unit-update journal's reader was already strict about members (ADR 0012)
   and reports `RecoveryError::Invalid`.
3. **The consumer journal moves to format 2**, with the same members as
   format 1. The number is what protects against versions built before this
   change: they read only format 1 and ignored unknown members, so they
   refuse a format-2 journal instead of misreading its `pending` record.
   This version reads formats 1 and 2, so a journal an earlier version left
   is still recovered, `pending` included. *Amended (#101):* format 3 adds
   `declaration` to `pending`; formats 1 to 3 are read.
4. **The unit-update journal stays at format 1.** Its reader has refused
   unknown members since it was written, so a version from before `copies`
   refuses a journal that has them. Its next change takes format 2 under
   point 1.
5. **Recovery guarantees are unchanged** for every journal this version
   writes: the steps, their order, and fault injection are untouched.

## Alternatives considered

- **Keep format 1 and only make the reader strict.** It protects versions
  from now on, but versions built before this change would still misread a
  waiting composition. Raising the number costs one constant and one
  accepted value.
- **Ignore unknown members that are marked optional.** Recovery cannot know
  whether a member it does not understand is safe to ignore, which is the
  defect.
- **Version the lockfile and the store the same way here.** They are not
  recovery state, and the release ADR (0026) already says persisted formats
  may change between alpha builds. Left for when a format change needs it.
  *Amended (ADR 0041):* these rules now apply to every format.

## Consequences

- A journal written by a newer version blocks an older one until the newer
  version finishes the composition or the update. That refusal is the
  point. Downgrading in the middle of a composition needs the newer binary.
- Adding a member to a journal is a format change. A reader must know
  every earlier format it still accepts.
- Tests cover format 2 being written, format 1 being read with `pending`,
  refusals of a newer format and of unknown members at each level, and every
  consumer command refusing such a journal while leaving the project
  byte-identical.
