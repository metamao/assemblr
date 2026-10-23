# 0032: Layered amendments: the base below an amender, marked sibling conflicts, the pending state, and the rebase in `unit update`

- **Status:** Accepted
- **Date:** 2026-10-23

## Context

Under ADR 0011 an amender's base was the provider's own file, every amender
of a file was a sibling, two amenders of one provider file were refused
together (point 5), and `unit amend` refused a file a locked dependency
already amended (point 6). ADR 0015 merged siblings on that flat model. The
owner decided in #100, after the authoring-friction review, that an
amendment reaches any unit in the amender's dependency closure, that its
base is the file as the units below the amender compose it, that siblings
still combine side by side, and that a conflict between siblings is marked
in the copy a unit above them takes, where that unit resolves it, instead
of being refused. The product rules are in
[`amendments-and-composition.md`](../domain/amendments-and-composition.md#the-base)
and
[`authoring-workflow.md`](../domain/authoring-workflow.md#build-details--approved),
with the provisional details D-41 to D-43 in the decision log. This ADR
records how that is implemented. It supersedes points 4 to 6 of ADR 0011
and the flat model of point 3 of ADR 0015, and replaces point 7 of ADR
0023.

## Decision

1. **Reach is the closure, order is `below`.** The domain
   (`composition::amendment`) accepts a declaration whose provider is in
   the amender's dependency closure, computed by `dependency_closure` over
   the resolved graph, and refuses one whose provider is not
   (`AmendmentUnreachable`). Each `Amendment` carries `below`: the other
   amenders of the same provider file in its closure. Two amenders of one
   file where neither is below the other are siblings; `SeveralAmenders`
   now only refuses siblings of a file no merge covers, XML or non-text,
   where no chain orders them. The declaration's base is still the
   provider's version, so manifests and published units do not change.
2. **One layering engine, recursive on the set of amenders**
   (`adapters::layered`). `layer_json` and `layer_text` take the provider's
   file and one `Layer` per amender (copy, `below`, and an optional base).
   The maximal amenders, those below no other in the set, are combined side
   by side, with the value merge of ADR 0015 or the line merge of ADR 0021,
   on the composition of the rest, computed by the same function. A
   contested value is written as `{"<<<<<<< conflict": {amender: value, …}}`
   and contested lines as diff3-style blocks named by amender, with a
   `||||||| base` section; the markers are text, so a copy that resolves
   them is an ordinary copy. The chain case, a total order by `below`, needs
   no merge: the top copy is delivered as it is, in any format, which is
   what ADR 0021's text and XML rules already allowed one amender to do.
3. **A base at the amender's own versions.** A published amender's copy was
   taken against the units below it at the versions its lockfile recorded.
   When the composition resolves the provider or a lower amender to another
   version, `build::layering` reconstructs that base from the published
   content at the amender's locked versions, read from the registry, and
   the copy's changes are then applied to the composition as it is now,
   with the cross-version derivation of ADR 0023. A lower amender's base
   follows the same rule inside the reconstruction, memoized per unit and
   path, so a base is always the file the amender saw. Base verification
   (`lock::verify_bases`) therefore checks every lockfile entry a base
   depends on, the provider's and each lower amender's, against the
   registry; an entry that differs is an integrity error naming it.
4. **A marker nobody resolves.** `render_files` leaves a file whose
   layering has conflicts between siblings that no amender above both
   resolved out of the composed files and reports it as `UnresolvedFile`.
   A consumer composition fails with it. A unit build (`pending_files`)
   turns each such file the unit does not amend into a `PendingFile` when
   its provider is locked: the marked copy is written under `amendments/`,
   declared in the manifest with `declare_amendment`, and the lockfile is
   written, while `build/` is left as it was and the command exits
   non-zero. `unit build`, `unit check`, and `unit publish` refuse while
   any copy holds a marker (`has_markers`, which looks only for the opening
   and closing lines and the JSON member, so an `=` underline in Markdown
   is not one); `unit amend` of another file still works, and composes
   without the unit's own copies, so an unresolved copy never feeds a base.
5. **`unit amend` copies the layered base.** The command searches every
   locked unit, with `--unit` naming any of them, and writes the file as
   the dependencies compose it, layered on the sources so a template keeps
   its expressions (`layer_sources`), and names the amenders below. The
   redundancy check of OC-11 is against the provider's file, so a copy of
   a file only lower amenders changed is a change the author made.
6. **`unit update` rebases on layered bases.** The former byte-level
   carry-over and `reconcile_amendments` are replaced by a reconciliation
   in the adapter that composes the world before and after the update
   without the unit's copies (`without_amendments`), takes each amended
   file's base from both, and, when the bases differ, rebases the copy
   three ways: `rebase_json` derives the copy and the new base against the
   old one, combines them with lists whole, and plans the incoming changes
   onto the copy's own text, so the author's layout survives; `rebase_text`
   merges lines with the incoming side ordered first at an insertion tie,
   like `RebaseSide::Provider`. What both sides change is marked in the
   copy, as `{"<<<<<<< conflict": {"current": …, "incoming": …}}` or as a
   block of `current (your copy)`, `base (before the update)`, and
   `incoming (the base after the update)`, and the update is applied and
   exits non-zero. A file the unit amends that vanished, or a declaration
   the author edited, still refuses the update. Sibling conflicts the new
   graph brings into files the unit does not amend become pending copies in
   the same journal, created, declared, and reported as point 4 does.
7. **Testing.** Domain tests cover reach, siblings, chains, and the
   ordering of marked sides; `layered` unit tests cover values, lines,
   nested layers, and the rebase; integration tests in `tests/layered.rs`
   run the binary through a transitive amendment, a published chain whose
   lower amender moves, `unit add` and `unit update` creating a marked copy
   and the refusals until it is resolved, a consumer failing on a marker,
   declaration order, and a forged lower entry.

## Consequences

- A copy composes against what the units below contributed, so a dependent
  can override a sibling's value without forking the provider, and a
  conflict reaches exactly the unit that can decide it. The cost is that a
  unit's copy now depends on the amendments of every unit below it: a
  release below can rewrite the copy on `unit update`, or mark it, where
  before it could only be refused.
- Reconstructing a base reads published content at other versions than
  the ones resolved, once per amender and path, and the registry must hold
  those versions; the build reports a missing one as a resolution problem.
- Markers are part of a copy's text. A tool that parses `amendments/` must
  expect them, and the marker lines and member are reserved: a provider
  file that contains them cannot be distinguished from a pending copy.
- Consumer reconciliation (ADR 0017, 0020, 0022) still reports a sibling
  conflict as an error; marking it there is #101.
