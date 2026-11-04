# 0041: Format compatibility: one set of rules for every format, a declared minimum Assemblr, and samples that never change

- **Status:** Accepted
- **Date:** 2026-11-04

## Context

Assemblr persists or publishes about eighteen formats: the authored JSONC
files (`manifest.jsonc`, `catalog.jsonc`, `assemblr.jsonc`), the lockfile,
the content-addressed stores, the registry index, the journals, and
`--json` output. Every JSON file Assemblr writes already had a `format`
number and a strict reader, but only the journals had written rules
(ADR 0027), and ADR 0026 promised nothing between alpha builds. A newer
format looked like damage everywhere but the consumer journal. The authored
files had no marker at all, so an older Assemblr could only call a newer
unit's field unknown. Removing `params` and `options` (#125) had already
made earlier published versions unreadable. The owner decided the policy on
2026-11-04, one question at a time, before the first beta
([compatibility.md](../domain/compatibility.md), OC-57).

## Decision

1. **ADR 0027's rules apply to every format**: a new number for every change
   to members or meaning, readers that refuse what they do not know and
   change nothing, and a refusal that names a newer Assemblr. One helper,
   `compatibility::unknown_format`, words the refusal for the lockfile, the
   registry index, the bases record, and the update journal: a number above
   the newest it reads says that a newer Assemblr wrote the file and to
   upgrade; any other unknown number says no version writes it.
2. **A declared minimum Assemblr.** `manifest.jsonc` and `catalog.jsonc`
   take `assemblr`, the oldest version that reads the unit
   (**Provisional (D-62)** for the name, the form, and where it is written).
   - The reader compares it with the running version and, when it is
     newer, reports only that: the unknown fields that come with a newer
     format are expected. The registry already leaves out a version whose
     manifest does not read, with the reason, so an older Assemblr resolves
     an earlier version and says why.
   - `unit build` writes it after the build is written, through a staging
     file and a rename, and reports it; a failure to write it is a warning
     and leaves the file as it was. `unit publish` tags the commit as it
     is, so it refuses a commit whose value differs from the build's, as it
     refuses a stale lockfile. `unit init` writes it in a new manifest.
   - The value is `compatibility::required()`. Every form a unit can use
     today is read since `0.1.0-alpha.1`, the first version with the field,
     so there is one constant and no table of features. The first form an
     older version cannot read turns the constant into a computation over
     what the unit uses.
3. **Samples that never change.** `tests/formats/` holds one sample of every
   promised format, and a unit test next to each reader reads it back. The
   base-listing sample pins the identity it hashes to.
4. **Project and machine state migrate on write**, as the registry index
   already does: the newest format is written only when the file is written
   anyway.

## Alternatives considered

- **No field, only a better message** (recommended to the owner). The
  strict reader already makes an older Assemblr skip a newer unit. The
  owner chose the field for an exact message and because a change of
  meaning without a new field would otherwise pass unnoticed.
- **A `format` number in the manifest.** Assemblr would raise it with each
  manifest change, but it says nothing about which Assemblr to install.
- **The author writes the field, `unit check` verifies it.** One more step
  at every publication, and a value too high still passes.
- **Assemblr writes its own version.** Simplest, but a unit would require
  an upgrade even when it uses nothing new.
- **The field in the lockfile, which a build already writes.** No edit of
  an authored file, but a version without the field reads a published
  lockfile only when the unit has amendments, so it would ignore the
  field instead of refusing it.
- **Keeping each project file's existing format** until a feature needs a
  new one, as Cargo does with `Cargo.lock`. Better for a team on mixed
  versions, but every old format needs a writer as well as a reader.
- **A version field in every `--json` output.** Programs would check a
  number that never changes while output only grows; it can be added when
  a breaking change needs it.

## Consequences

- Readers keep every format they promise, so the readers grow with each
  format change; the samples make removing one a failing test.
- A unit built with this version gains a line in `manifest.jsonc` or
  `catalog.jsonc`, which the author commits. A published alpha unit without
  it stays readable.
- An Assemblr built before the field refuses a manifest that has it as an
  unknown field. That is the alpha behavior the promise does not cover.
- A team on mixed versions blocks the older machines after the first write
  by a newer one; the message says to upgrade.
