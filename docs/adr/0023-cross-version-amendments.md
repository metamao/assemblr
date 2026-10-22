# 0023: Amendments across provider versions: deriving against the base, applying to the version resolved, and rebasing in `unit update`

- **Status:** Accepted
- **Date:** 2026-10-22

## Context

A reference scenario of four units showed that each release of a provider
stopped every composition that mentioned a unit amending it, until that unit
had published again, whether or not the release touched anything the unit
contributed. The cost was one release per amender per provider release, in
dependency order, and a consumer could not take a provider release until all
were done. The merge-engine decision (#33) had deferred and refused
cross-version application and the reconciliation of a changed base in `unit
update` (OC-52, OC-53). The owner decided both on #59 (decision log D-22):
amendments are applied to the version resolved, without republishing the
amender. This ADR records how that is built.

## Decision

1. **The composer keeps each amendment's base.** `compose` no longer rejects a
   published amender whose declared base is not the version its provider
   resolved to. It records the base with the copy (`Amendment::base`) and
   leaves the combining to the build. The unit being built keeps the old rule
   (`AmendmentBase` for the root): its base is the version it is locked to.
   Resolution is unchanged, so the amender's declared requirement still says
   which provider versions are eligible.
2. **The base file comes from the registry, once per version.** When a
   composition resolves a provider to another version than an amendment's
   base, `render_files` reads the provider's `src/` at the base through the
   same reader a build uses (the indexed commit, which `verify_bases` has
   checked against the amender's lockfile) and caches each version's tree. A
   file whose bytes are the same at both versions is applied as if taken from
   the version resolved, so a release that does not touch the amended file
   costs nothing and needs no merge, whatever the format.
3. **JSON and JSONC are planned against the resolved file.** `merge_json_from`
   derives each amender's operations against that amender's own base,
   combines them, and plans them against the file at the version resolved, so
   the provider's own changes stay and a change to a value an amender changed
   is a conflict. Each amender's round-trip guard runs against its own base.
   The same function serves the old case, where every base is the provider's
   file.
4. **Text is rebased, then combined.** For text, an amender whose base is not
   the provider's file is first moved: the copy, its base, and the provider's
   file are merged by line with neither side placed before the other
   (`InsertTie::Conflict`). Then every amender's moved copy is combined
   against the provider's file as before (ADR 0021). A collision of the first
   step is a `Rebase` failure naming the amender and the lines of its base.
5. **Formats without a merge, and non-text.** YAML, TOML, XML, INI, and any
   file or copy that is not UTF-8 text is applied across versions only when
   the provider's file is the same bytes at both versions (point 2).
   Otherwise the composition fails with `CrossVersion` (no merge).
6. **Rendering (OC-52).** The base and the copy are rendered with one context:
   the composition's values for the provider's parameters, plus `own` for the
   copy. The resolved file is rendered with the same context. The owner's
   direction was to derive "using the context appropriate to the verified
   base"; a fallback to the base version's defaults was examined and not
   adopted. Rendering both with the composition's values already makes the
   derived edit independent of the values (both sides move together), and a
   default of the base version would pick a value no composition gave, which
   can make a difference between the versions look like the author's edit.
   Instead, every provider parameter that the base file or the copy names
   must be declared at the resolved version with the same kind of value;
   otherwise `CrossVersion` fails, naming the parameter and both versions.
   `parameters_referenced` lists the names. A parameter that keeps its name
   and kind and changes meaning is not detected, which the contract says.
7. **`unit update` rebases on source templates (OC-53).** `reconcile_amendments`
   takes the unit's copies. An unchanged file is carried over as before. A
   changed file of text outside the unsupported formats is merged by line from
   the old base to the new version, with the copy as one side and the
   provider's change as the other, on the source text, so template
   expressions survive and no structured parser sees a template. A rebased
   JSON file that is not a template must parse, or the update is refused
   (`RebasedNotJson`). A collision lists each place with what the provider and
   the amendment do there (`ReconcileCause::Collides`). Nothing is written on
   any refusal.
8. **Writing and recovery.** The journal gains an optional `copies` object
   (path to before and after), absent when no copy is rewritten, so journals
   of earlier versions still read and an update that rewrites no copy writes
   the same journal. The order of replacement is the copies in path order,
   the manifest, then the lockfile. Recovery treats every file the same way:
   a file may hold its planned content or its previous content, the files
   that hold the planned content must come first in that order, a file whose
   planned content equals its previous content is consistent anywhere, and
   any other content stops the completion. Each copy is staged beside its
   path under `<name>.assemblr-update-new`; a staging file in the way refuses
   the update before anything is written. Fault injection covers each step.
9. **The published-amender check is gone.** `published_amenders_at_base` and
   its error are removed: an update no longer fails because a published
   amendment's provider would not resolve to its base.
10. **Diagnostics.** `CrossVersion` and the merge failures that involve an
    amender based on another version name the amender, its base, and the
    version resolved; the hints say to stay on the base (`unit@=base`) or use
    an amender version taken from the version resolved. A successful
    cross-version application prints nothing (the owner's decision).
11. **Testing.** Domain tests cover the rebase (clean, collision, no merge,
    not text); adapter tests cover the journal with copies at every failing
    step, a copy edited since, and the journal's compatibility; integration
    tests run the binary on units shaped like the reference scenario
    (`tests/cross_version.rs`): releases that change other files and values
    (a script and a dependency of `package.json`, a guidance file),
    a collision with the way out in the hint, amenders taken from different
    versions in either order, a templated file whose parameter is kept,
    renamed, and retyped, the declared range, a lockfile that misidentifies a
    base, a YAML file that changed, and a file the provider removed; and
    `tests/build.rs` covers `unit update` clean and colliding, the
    dependent-build case, and an update that no longer waits for the amender.

## Consequences

- A provider release no longer costs a release per amender. An amender needs
  a new version when a release falls outside its declared requirement, or
  when its contribution must change to fit an incompatible release.
- Failures move to the moment a composition first resolves the new provider
  version: a consumer who takes a provider release that collides with an
  amendment learns it from `compose`, which changes nothing, and can stay on
  the earlier version meanwhile. An amender's author learns it from `unit
  update`.
- A clean merge shows structural compatibility, not that the result works.
  Nothing marks a cross-version application, so a consumer who wants to know
  which amendments were applied across versions reads the lock (versions) and
  the amenders' manifests (bases).
- Each composition that crosses versions reads the provider's tree at the
  base, once per version, and reads its two files; the work grows with the
  number of distinct bases, not with the number of files.
- Source-level reconciliation in `unit update` and rendered-level
  composition remain two operations with two outcomes: the first can refuse
  where the second would have applied, and the author fixes it by amending
  again.
