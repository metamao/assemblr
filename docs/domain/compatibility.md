# Compatibility

What every later version of Assemblr promises to read and how it treats what
an earlier or a later version wrote. Decided by the owner on 2026-11-04
([OC-57](open-contracts.md#oc-57)); the engineering is in
[ADR 0041](../adr/0041-format-compatibility.md).

## When the promise starts — Approved

- The promise starts with the **first beta release**. Alpha builds make no
  promise to each other ([ADR 0026](../adr/0026-private-alpha-distribution.md)),
  but a beta still reads every alpha format that has not changed by then.
- It covers what Assemblr persists or publishes: the files below. It does
  not cover text output or the agent guide, which are not contracts (see
  [registries-and-discovery.md](registries-and-discovery.md#agent-guide--approved)),
  or the bytes a composition produces, which
  [amendments-and-composition.md](amendments-and-composition.md) covers:
  semantic compatibility is promised, byte-identical formatting is not.

## Rules for every format — Approved

1. A change to a format's members or meaning takes a new `format` number.
   A file authors write, which has no number, takes a new field that older
   readers refuse.
2. A reader refuses a format number or a member it does not know, and
   changes nothing.
3. The refusal tells a file a newer Assemblr wrote apart from a damaged one,
   and says to upgrade Assemblr.
4. Every format a release promises to read has a sample in
   [`tests/formats/`](../../tests/formats/README.md) that a test reads back.
   Samples never change.

[ADR 0027](../adr/0027-journal-format-compatibility.md) set these rules for
the journals; they now apply to every format.

## Published units — Approved

- Every later version of Assemblr reads **every unit and catalog version
  published since the start, forever**: `manifest.jsonc`, `catalog.jsonc`,
  the published lockfile, `amendments/` and their declarations, a catalog's
  `.assemblr/`, the `v<version>` tag, and the way a version's content and
  stored bases are identified (`commit`, `tree`, `assemblr-base-v1`,
  `sha256-v1:`).
- Nothing is migrated: a published version never changes, so a later
  Assemblr keeps the readers of every earlier form instead.

### The oldest Assemblr that reads a unit

- A unit's `manifest.jsonc`, or a catalog's `catalog.jsonc`, declares the
  oldest version of Assemblr that reads it.
- **Assemblr computes the value, never the author:** the lowest version
  that reads everything the unit uses.
- A version of Assemblr older than the declared one does not read the unit:
  the registry leaves that version out and reports that it needs a newer
  Assemblr, so resolution uses an earlier version, and building such a unit
  is refused with the same reason.
- **Provisional (D-62):** the field is `assemblr`, an exact SemVer version,
  placed after `version`. `unit build` writes it after a successful build,
  as it writes the lockfile, and reports when it changed it; `unit publish`
  tags the commit as it is, so it refuses a commit whose value is not the
  one a build writes. A unit in a catalog declares none, since
  `catalog.jsonc` declares it for every unit, and a manifest there that
  holds one is refused. Every form a unit can use today is read since
  `0.1.0-alpha.1`, the first version with the field, so that is the value
  for every unit until a form an older version cannot read exists.

## Project and machine state — Approved

The files of a consumer project (`assemblr.jsonc`, `assemblr-lock.json`,
and `.assemblr/`) and the machine's registry (the index, remote copies, and
the agent skill):

- A newer Assemblr reads every format written since the start.
- It writes its own format only when it writes the file anyway; it never
  rewrites a file just to upgrade it. The registry index already works this
  way ([ADR 0038](../adr/0038-remove-registry-catalogs.md)).
- An older Assemblr refuses a format it does not know, changes nothing, and
  says to upgrade. Running an older version after a newer one has written
  the files is not supported.

## Recovery state — Approved

The journals of a composition and of `unit update`, and the conflict markers
a waiting composition leaves, follow
[ADR 0027](../adr/0027-journal-format-compatibility.md): the version that
wrote a journal finishes it, and a release reads every journal format
written since the start.

## Machine-readable output — Approved

`--json` of `status`, `info`, `search`, and `registry list`:

- Fields may be added. Existing fields keep their name, type, and meaning,
  and the order of keys is not significant.
- A change that breaks this needs a new flag or a `format` field. There is
  no version field until one is needed.
