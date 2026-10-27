# Amendments and Composition

Composition combines a unit's contributions with those of its resolved
dependencies. Several units may contribute to the same file. This document
defines how units modify files their dependencies provide, and what Assemblr
promises when it combines those contributions.

## Principle — Approved

> **Copies are the authoring interface. Operations are the internal engine.**

Authors modify ordinary, full-file copies of dependency files. Assemblr derives
internal change operations from the difference between each copy and its exact
historical base, combines operations from all contributors, and writes results
back through format-aware adapters.

Assemblr does **not** promise universal, automatic, conflict-free merging. It
promises to preserve what it can safely preserve and to **report** everything
else.

## Amendments — Approved

The term **amendment** is approved and replaces the earlier term *fragment*.

- An amendment is an editable **full-file copy** of a file that a dependency
  provides, modified by the amending unit. It lives under `amendments/`.
- Authors edit ordinary copies. They do not hand-write low-level merge
  operations.
- An amendment preserves its **provenance** and its **exact historical base**:
  the dependency identity, the exact version, and the source file the copy was
  taken from. This is what lets Assemblr derive the changes and reconcile them
  when the dependency is updated later.
- An amendment's base must stay available. Updating a copied dependency file
  without its exact historical base can silently reverse upstream changes.
- A base comes from the dependency's exact historical version, preserved
  through immutable snapshots and Git provenance. It is never replaced by
  whatever currently exists in the dependency author's working tree (see
  [registries-and-discovery.md](registries-and-discovery.md#integrity-and-snapshots--approved)).
  In a catalog, whose units change together, the base is stored instead
  (see [the base in a catalog](#the-base-in-a-catalog)).
- The engine may need **format-aware adapters** and **limited explicit
  metadata** where ambiguity cannot be resolved safely.

Amendments are created with `unit amend` and reconciled with `unit update`
(see [authoring-workflow.md](authoring-workflow.md)).

### The base

Decided by the owner on #100 (2026-10-23), replacing the original-file base
of the first amendments increment.

- An amendment's base is **layered**: the provider's file in its **`src/`**,
  at the exact version the amending unit is locked to, as the units **below**
  the amender compose it. The units below an amender are its dependency
  closure, direct and transitive, each at the amender's locked version; the
  base is the provider's file with the amendments of every unit below that
  amends it applied. It is the file as it appears in the amender's `build/`
  before the amender's own copy, and `unit amend` hands out exactly that.
- A unit can therefore change what a unit below it added or changed through
  an amendment, as it changes anything else in its base. Its copy is taken
  on top of the composition below, whatever the file's format: one amender
  above another is a chain, not a merge.
- Amenders that are not below one another are **siblings**. Their amendments
  are combined **side by side**, with no precedence by name or order (see
  [merge semantics](#merge-semantics--approved)). Identical contributions
  from siblings to one value are compatible and become one change (decided
  by the owner, #33).
- Where siblings conflict, the base of a unit above both carries a
  **conflict marker** at each contested value or run of lines, and that unit
  resolves the conflict by replacing the marker in its copy. The marker
  shapes are **Provisional (D-41)**: for a JSON or JSONC file, the contested
  value becomes an object whose only member, `"<<<<<<< conflict"`, names each
  side's value by its amender, `null` for a side that removes the value;
  for a text file, the contested lines are
  written between `<<<<<<< <amender>`, `||||||| base`, `=======`, and
  `>>>>>>> <amender>` lines, as diff3 conflicts are shown, with each
  separator naming its side when more than two sides changed the lines.
  A conflict that no unit above both siblings resolves is reported by every
  composer; what it means for the composition is decided by the flow (see
  [composition, builds, and reconciliation](#composition-builds-and-reconciliation)).
- Amendments are reconciled across versions with the same three inputs,
  the base before, the copy, and the base after, whichever unit moved:
  a change below that the copy did not touch flows through, and a change to
  what the copy changed is a conflict (see
  [across provider versions](#across-provider-versions--approved)).

### The base in a catalog

Decided by the owner on #128 (2026-10-27, adopting the catalog design).

The units of a [catalog](units.md#catalogs--approved) change and publish
together, so an amendment of a sibling's file has no earlier published
version to take its base from, and the sibling as it is now is no base: a
later edit to the sibling would be removed silently by the copy.

- The base of an amendment of a sibling is **stored in the catalog**, under
  `.assemblr/` at its top level, in the content-addressed format consumer
  projects use for their reconciliation base. It is the file as the units
  below the amender compose it, as for any amendment, when `unit amend`
  takes the copy.
- When that file changes, `unit build` merges the change into the copy with
  three inputs, the stored base, the copy, and the file now. If the merge is
  clean, it updates the copy and the stored base and reports it. If it
  conflicts, it stops, leaving each conflict between markers in the copy
  (D-41), with a diagnostic naming the file and the units involved.
- `unit check` and `unit publish` refuse a stored base that is not the file
  below the copy now, so a published version needs no merge across versions
  within the catalog: its copies are on the files below them at the same
  version.
- A stored base is verified against its recorded identity before use, and a
  missing or corrupt one is an explicit failure.
- Layered amendments between siblings follow [the base](#the-base): the
  base is the file as the units below compose it, so a unit above another
  is rebased after it.

Details:

- **Provisional (D-57):** the declaration of an amendment of a sibling names
  its base as `"catalog"`, the catalog's version, as a sibling is required:
  `"package.json": { "unit": "@acme/node-base", "base": "catalog" }`. A
  version as a sibling's base is an error, and so is `"catalog"` for a unit
  of another repository or outside a catalog.
- **Provisional (D-58):** the catalog stores one listing of every base, by
  `<unit directory>/<amended path>`, under `.assemblr/bases/`, with each
  base's bytes under `.assemblr/objects/`, and
  `.assemblr/amendment-bases.json` names the listing by its identity.
  `.assemblr/` is tracked with the catalog. The base of an amendment the
  catalog no longer declares is removed by the next build that writes the
  store.
- **Provisional (D-59):** every amendment of a unit in a catalog has a
  stored base, of a sibling's file or of a file of another repository, since
  a sibling's copy below can be part of either. `unit amend` works in a
  unit's directory in a catalog.
- **Provisional (D-60):** a build at the catalog's top level rebases every
  unit's copies; in a unit's directory, the copies of that unit and of the
  units of the catalog below it. Each unit is rebased after the units of
  the catalog it depends on. A clean rebase is written only once the whole
  build validates, together with it; a failed build leaves `amendments/`
  and `.assemblr/` as they were. A rebase with conflicts writes every
  rebased copy and the new bases, and builds nothing. A file whose rebase
  conflicts is not rebased in the units above until it is resolved.

### Target

- An amendment targets a unit **below** the amending unit: any unit in its
  resolved dependency graph, direct or transitive, which provides the file's
  path in its `src/` at its locked version. The provider need not be declared
  in the amender's own `dependencies`; the declaration names it, with the
  base version, so that the base can be identified and verified. Amending a
  file of a unit that is not in the graph is an error.

### Layout and declaration

- An amendment copy lives at **`amendments/<path>`**, where `<path>` is the
  file's path in the provider's `src/`, with the same path rules (see
  [units.md](units.md#src-unit-files)). An amendment changes a file's content,
  never its path.
- The amending unit's own `src/` must not hold a file at the same path. That
  is a path collision.
- The manifest declares each amendment by its path, with the identity of the
  target unit and the **exact base version**. The declared version must equal
  the version of that unit in the
  [unit lock state](authoring-workflow.md#unit-lock-state--approved).
  The declaration is a member of the top-level **`amendments`** map, keyed by
  the path, whose value names the provider as `unit` and the base as `base`
  (see
  [manifest.md](manifest.md#serialization-of-dependencies-placeholders-and-amendments--approved)):

  ```jsonc
  "amendments": {
    "wrangler.toml": { "unit": "@metamao/cloudflare-worker", "base": "2.1.0" }
  }
  ```

- A copy under `amendments/` that the manifest does not declare, and a
  declaration whose copy does not exist, are errors: an undeclared copy has
  no base, and a declared base without a copy amends nothing.
- Amendments change **content only**. They do not delete, rename, or create
  files, and they do not change a file's executable bit; the provider's bit
  stands.

### Base identity and verification

- The exact content of a base is identified through the amending unit's unit
  lockfile: its entry for the provider records the base version together
  with the commit and tree that version was published with, and its entries
  for the units below that amend the same file record the versions whose
  amendments the base is composed of (see
  [authoring-workflow.md](authoring-workflow.md#unit-lock-state--approved)).
  The lockfile is tracked, so a published version of the amending unit carries
  it. A published version with amendments carries a lockfile consistent with
  its declarations.
- Whoever composes an amendment, a dependent unit's build or a consumer
  project, verifies the base: every lockfile entry the base depends on, the
  provider's at the declared version and each amender below's, is compared
  with the composer's own registry record for that unit and version. They
  agree when they record the same tree, the version's content identity; the
  commit is provenance, so the same tree under another commit, as a pull
  request's merge commit gives, agrees (owner decision on #123). If they
  agree, the base is verified.
  If any tree differs, that is an **integrity conflict**: the composition
  fails and changes nothing (see
  [registries-and-discovery.md](registries-and-discovery.md#integrity-and-snapshots--approved)).
  If a version or its content is unavailable, see **OC-9**.
- A published amender's base is the composition at **its** locked versions.
  When a composition resolves the provider, or a unit below the amender that
  amends the file, to another version than the amender locked, the
  amender's base is reconstructed from the published content at the
  amender's locked versions, so that its changes are derived against the
  base its author saw, and applied to the composition's own base
  (**Provisional (D-43)**; see
  [across provider versions](#across-provider-versions--approved)).
- **Verifying a base is distinct from comparing it with the resolved
  version.** A composition may resolve the target unit to a version other
  than the declared base; the unit lockfile does not determine what
  dependents and consumers resolve. That is normal, never an integrity
  violation. The amendment's changes are derived against its verified base
  and applied to the file at the resolved version, and incompatible changes
  are reported (**OC-10**). Only when the resolved version equals the base is
  the result simply the amended copy.

Open:

- Behavior when a base's historical Git objects are unavailable. **OC-9.**

A conflict between units' contributions in a consumer composition is a
composition error, not a pending-resolution state (**OC-47**), except a
conflict between sibling amendments, which the owner reopened on #101: it is
left in the project for the consumer to resolve, **Provisional (D-44)** (see
[pending-resolution details](consumer-projects.md#pending-resolution-details--approved-d-18-sibling-conflicts-provisional-d-44)).

## Merge semantics — Approved

Requirements:

- **Several units may contribute** to the same file.
- **Independent changes are preserved.**
- **Incompatible changes are detected** and reported as merge conflicts.
- **No edit is silently lost.**
- Changes may be **additions, modifications, and deletions**.
- The **previously generated state stays available** as a reconciliation base.
- Every amendment keeps its **exact original version and base**.
- Merge results and conflict detection are **deterministic**.
- Applying updates must not arbitrarily favor incoming changes over
  legitimate edits made downstream (for example, consumer edits).
- Plans are **validated before** any persistent state is modified.
- **Unsupported or ambiguous** changes are reported, never guessed.

### Modifying a dependency's existing values

A unit may make explicit, controlled modifications to values a dependency
already provides, when:

1. the change's historical base is known, and
2. no incompatible contribution exists.

This is **not** general last-writer-wins. Two units that change the same value
incompatibly produce a conflict.

### Ordering

When several contributors insert at the same location and the order may be
significant, Assemblr **prefers reporting a conflict** to choosing an order
arbitrarily.

Open:

- The exact same-anchor ordering policy. **OC-12.**

### Structured and unknown formats

- Structured formats may use **value-level operations** and
  **format-preserving adapters**.
- **List identity** rules may be needed where identity cannot be inferred
  safely. Inferring list identity incorrectly can silently corrupt results.
- Unknown or unsupported formats get **conservative** behavior: report rather
  than guess.

### The merge engine — Approved

Decided by the owner on #33, ahead of the consumer lifecycle, for unit
composition:

- **A value-level N-way merge.** Operations are derived from each amendment
  copy against its verified base, the operations of every amender are
  combined, and the result is applied to the provider's file with
  deterministic conflict detection and strict validation of the output: the
  written text must parse back to exactly the merged value.
- **JSON and JSONC first**, with format-aware parsing and a format-preserving
  writer. XML and binary files with several amenders stay refused, and a
  single amender's copy keeps taking the file's place as it does today,
  whatever the format. Other UTF-8 text, YAML, TOML, and INI included, is
  combined by line, as the next section describes (YAML, TOML, and INI:
  owner decision on #92, which replaces their refusal decided on #33).
- **Identical contributions** from several amenders are one change.
  **Different changes to one value conflict**, with no precedence and no
  last-writer-wins. A removal, or a replacement by a scalar or a list,
  conflicts with another amender's edit inside the removed value.
- **No layered overrides**: an amender cannot change a value another
  amendment introduced; the flat model stands.
- **Lists** are compared whole. No element identity is inferred; a list that
  several amenders change identically is one change, and lists they change
  differently conflict, so changes inside one list are never merged
  independently. Changes to the objects around a list are unaffected.
  **OC-4** stays open for identity hints.
- **Lists of scalars** that several units amend are joined, decided by the
  owner for the alpha: the result holds the provider's items first, then each
  amender's new items in order of identity, without duplicates. Lists of
  objects are still compared whole. Which changes count as joinable is
  **Provisional (D-38)**: an amender's copy must be the provider's list
  followed by new scalars, or a new list several amenders create; any other
  change is compared whole. Consumer reconciliation keeps comparing lists
  whole (D-38).
- **Insertion order.** When the order of several contributions could affect
  meaning, a conflict is reported rather than an order invented. **OC-12**
  stays open; see the implementation notes for JSON objects.
- **Cross-version application** (**OC-52**) and reconciliation of a changed
  base in `unit update` (**OC-53**) were deferred here and are decided in
  [the next section](#across-provider-versions--approved).
- **Stability.** Results are deterministic for the same inputs and engine
  version. Byte-identical formatting across Assemblr releases is not
  promised; semantic compatibility is, and a consequential change to merge
  behavior is an explicit decision.
- The same merge model is the foundation for consumer reconciliation, which
  [reconciliation details](consumer-projects.md#reconciliation-details--provisional-d-13-to-d-15)
  implements as **Provisional (D-13)**.

### Text files — Approved direction, details Provisional (D-20)

Several amendments of one text file are combined by a conservative
**line-level three-way merge**, decided by the owner after the reference
scenario showed that units which each add a section to a shared guidance file,
such as `AGENTS.md`, could not be composed. The details are **Provisional
(D-20)** (ADR 0021):

- **Scope.** Any file whose provider file and copies are UTF-8 text without a
  NUL, YAML, TOML, and INI files included (owner decision on #92), outside
  the structured formats the merge does not support: XML files, and JSON in
  another case, stay refused with several amenders (**Provisional (D-37)**).
  A file or copy that is not text stays refused. A single amender's copy
  keeps taking the file's place whole.
- **Method.** The provider's file and each copy are compared, as they
  are, line by line (a line keeps its terminator), and each
  copy's changes against the verified base are combined. The result is the
  base with every amender's changes applied.
- **Compatible changes.** Changes in different places are all kept. An
  identical change by several amenders is one change. Insertions that several
  amenders make at the same position are all kept, each distinct insertion
  once, in the order of the amenders' identities. No amender's lines are
  dropped, and the build and `unit check` report every such position as a
  warning, since the order is not something any unit asked for. A consumer's
  composition reports it only when it writes the file, since only the units'
  authors can act on it, and `--verbose` reports it every time
  (**Provisional (D-30)**). This is the
  text counterpart of the placement of members several amenders add at one
  place in a JSON object, and **OC-12** stays open for both.
- **What a clean merge does not promise.** A line merge compares lines, not
  what a format makes of them. A merge without a conflict shows that every
  amender's lines are in the file, not that the file means what each amender
  intended. In formats where the order or the repetition of lines decides the
  outcome, the order the merge fixes can decide which line takes effect, for
  example:
  - `.env` and other key-value files: two units that each add `PORT=` leave
    the key twice, and readers that take the last value apply the unit whose
    identity sorts later;
  - `CODEOWNERS`: the last matching pattern wins, so the order of two units'
    rules decides who owns a path;
  - `.gitignore` and similar ignore files: a `!` pattern re-includes only what
    the patterns before it excluded, so the order of two units' patterns can
    decide whether a path is ignored;
  - YAML, TOML, and INI files: two units that each add the same key leave
    it twice, which some readers reject and others resolve by taking one of
    them, and lines added under different parents can land in a block
    neither unit meant (owner decision on #92: merged by line, with this
    limit, rather than refused);
  - XML under an extension the merge does not recognize (for example `.svg`
    or `.csproj`), which is merged as text.

  Such cases are reported only as the warning for insertions at one position.
  Changes to the same line still conflict. The author verifies the composed
  file with the format's own tools against `build/` (see
  [testing](authoring-workflow.md#testing-native-validation-vs-functional-correctness--approved)),
  and a consumer verifies the composed project the same way. Units
  that need a precise order in such a file arrange that one of them owns the
  file or that their lines do not depend on each other. Only the formats
  listed under **Scope** are refused, by extension; none is refused for being
  order-sensitive (owner decision D-27, OC-12).
- **Conflicts.** Changes that overlap or touch, including changes to adjacent
  lines, and an insertion next to a changed line, are a conflict unless they
  are identical. The build is rejected naming the file, the lines of the
  provider's file, and what each amender does there, and writes nothing.
  Lines every amender agrees on at the start and the end of a conflict are not
  part of it.
- **Guard.** An amender's changes alone, replayed on the base, must give its
  copy back byte for byte. A mismatch is a defect in the merge and rejects
  the build, never a silent difference.
- **Determinism.** The result depends on the files and the amenders'
  identities only.

Implementation notes, **Provisional (D-6 to D-8)**, corrected by the review
recorded in the decision log:

- Object member order carries no meaning in the JSON grammar, so members
  that several amenders add at the same place are placed in a fixed order,
  by amender identity, which never changes the document's value. It can
  change what a program does with the file: Node reads the conditions of a
  `package.json` `exports` in order and takes the first that matches. For
  such objects the placement is an invented order, which the rule above
  prefers to report as a conflict.

  **Known limitation.** The merge does not preserve the meaning of member
  order where a consumer of the JSON gives it one. When several units
  amend one object, the order of their insertions, and a copy's reordering
  of existing members, are not guaranteed to carry the semantics a program
  such as Node attaches to them. Warnings and regression tests describe
  today's behavior; they do not resolve the problem. The owner decided to
  keep the generic behavior for now: no technology-specific rules in the
  engine, no new manifest metadata, and no conflict for every same-anchor
  insertion, which would refuse the common case of two units adding a
  script each, without evidence from real projects. **OC-12 stays open** and
  the model is revisited when Assemblr is evaluated on real projects. A
  format where order means something gets a conflict when it is supported.
- What the value model cannot carry from a copy, a comment the copy adds or
  a member order it changes, is reported as a warning naming the amender,
  and the composition proceeds with the provider's layout. A copy alone
  takes the file's place whole, so its author is told by `unit check` and
  `unit build` what a second amender would make the merge drop, instead of
  hearing of it only from the consumers who compose it; a consumer merging
  a new version into an edited file is told the same of the composition's
  own changes.
- A copy whose derived operations do not reproduce its value when replayed
  on its base is not a formatting matter but a defect in the derivation, and
  it is an error naming the amender, never a warning.
- The format is recognized by the delivered file's extension: `.json` or `.jsonc` is JSON; `.xml` and
  `.json` or `.jsonc` in another case are structured formats without an
  adapter; any other path, `.yaml`, `.yml`, `.toml`, and `.ini` included, is
  text. A consumer's reconciliation still treats YAML, TOML, and INI as
  formats without a merge (**Provisional (D-37)**; see
  [reconciliation details](consumer-projects.md#reconciliation-details--provisional-d-13-to-d-15)).

Open:

- The manifest representation of list identity hints. **OC-4.**
- Further format adapters. **OC-10.** The consumer side is
  **Provisional (D-13)**, without Git, in
  [reconciliation details](consumer-projects.md#reconciliation-details--provisional-d-13-to-d-15).

### Composition, builds, and reconciliation

For unit authoring, composition output goes to `build/`, which is rebuilt from
scratch and needs no reconciliation history (see
[authoring-workflow.md](authoring-workflow.md#build--approved)).

**Implemented.** A unit's build applies the amendments of every unit in its
composition: its own, whose bases are the composition at its
[unit lock state](authoring-workflow.md#unit-lock-state--approved) by
construction, and those that its resolved dependencies publish, direct or
transitive. A published amender's bases are verified against the composer's
registry record before composition, as approved above. The amenders of one
file are layered: the composition of a set of amenders is the composition of
those below the set's maximal amenders, with the maximal amenders' changes
derived against their own bases and combined on it, by [the merge
engine](#the-merge-engine--approved) for a JSON or JSONC file and by line for
text, on the files and copies as they are. One amender whose base is the
composition below it takes the file's place with its copy, whatever
the format, and no change is derived. A conflict between siblings is marked
in the composition, and resolved by a unit above both that replaces the
marker; a marker no amender above resolves is a conflict of the composition
as a whole. In a **consumer composition** the marked file is the
composition's contribution, the project enters the pending-resolution state
with it, and the report names the file, the siblings, and each contested
value or line (**Provisional (D-44)**; see [pending-resolution
details](consumer-projects.md#pending-resolution-details--approved-d-18-sibling-conflicts-provisional-d-44)).
In a **unit's build**, a marked file the unit does not
amend yet puts the build in the **pending state** (see
[authoring-workflow.md](authoring-workflow.md#build-details--approved)): the
unit gets the marked copy as its own amendment and resolves it. The other
cases that cannot be applied fail explicitly, naming each amender, its
version, the provider, the base, and the resolved version, and change
nothing (standing default D2): a copy whose base cannot be reconstructed, a
path the provider no longer has, a
provider file in a structured format without an adapter or that is not text
that changed between the two versions, and such a file that siblings amend.
`unit update` reconciles a unit's own amendments with their bases before and
after the update, leaves what both changed between markers, and hands the
unit the sibling conflicts its new dependencies bring (see
[authoring-workflow.md](authoring-workflow.md#update-details--approved)).

Consumer projects reconcile downstream modifications with three inputs: the
**reconciliation base** (the previously materialized composition), **current**
(the consumer's actual files), and **incoming** (the newly desired
composition). The merge semantics above apply. See
[consumer-projects.md](consumer-projects.md#reconciliation--approved).

### Across provider versions — Approved

Decided by the owner on #59 after the reference scenario showed that each
release of a provider stopped every composition that mentioned its amenders
until each amender had published again:

- **Where.** A composition, a dependent unit's build or a consumer's, derives
  each amendment's changes against its verified base and applies them to the
  provider's file at the version the composition resolved. The resolved
  version must satisfy the amender's declared requirement on the provider:
  that requirement says which versions are eligible, and the merge decides
  whether the contributions apply.
- **No republication.** A newer provider release never requires a new version
  of the amender. An amender needs one when the release falls outside its
  declared requirement, or when its own contribution must change to fit an
  incompatible release.
- **Compatible and colliding.** Changes the engines can apply without a
  collision (different values, different lines, identical changes) compose.
  A collision, in a value or in lines of text, has no precedence and no
  overwrite: between siblings it is marked in the file for the unit above or
  the consumer to resolve (layered model, below); any other is a composition
  error naming the amender, the file, and the place, and the project or unit
  is left unchanged.
- **Layered model.** Each amender's contribution is derived against its own
  base: the provider's file as the units below the amender compose it, at
  the amender's locked versions. Siblings stay independent: applying one
  never establishes the base or the precedence of another. An amender above
  another composes its copy on top of the other's, and sees the other's
  changes in its base.
- **A moved world.** When the composition resolves the provider, or a unit
  below the amender that amends the file, to a version other than the one
  the amender locked, the amender's base is reconstructed at its locked
  versions from the published content, recursively for the amenders below,
  and its changes are moved onto the composition's own base for the file:
  a change below that the amender's copy did not touch flows through, and a
  change to lines or values the copy changed too is a collision
  (**Provisional (D-43)**).
- **Insertions at one place in text.** When the provider's release and an
  amendment taken from an earlier version both insert lines at the same
  position of a text file, and neither changes a line there, both insertions
  are kept, **the provider's first** (decided by the owner after the second
  decision-log review, #70). The provider's release is the file the amendment
  is now applied to, so its lines keep their place, and the amender's follow
  them. Nothing either side wrote is lost, and nothing is reported: it is an
  ordinary cross-version application. Insertions by several amenders at one
  place keep the order of their identities, after the provider's, as in
  [text files](#text-files--approved-direction-details-provisional-d-20). Changes
  that overlap or touch, including an insertion next to a line the other side
  changed, still collide.
- **Formats without a merge.** An XML file, and a file that is not UTF-8
  text, is applied across versions only when the provider's file has the same
  bytes at the base and at the resolved version. YAML, TOML, and INI files are
  text here (owner decision on #92). Otherwise the
  composition is refused, with no fallback and no replacement.
- **The unit's own build** keeps its rule: the declared base is the version the
  unit is locked to (`unit amend`, `unit build`). Only dependents' builds and
  consumer compositions apply across versions.
- **Verification.** Every lockfile entry a base depends on is verified
  against the registry's record, as approved above, and every base file and
  copy is read from its immutable version. Results depend on the published
  files and copies at the versions involved and the unit's own copies only,
  so they are deterministic.
- **Visibility.** A successful cross-version application is an ordinary
  successful operation: it adds no note to a composition. A merge that is
  clean shows structural compatibility, not that the result is correct for the
  software, which the author and the consumer verify with the project's own
  tools. `--verbose` on `compose`, `add`, `remove`, and `update` lists, as notes,
  each amendment applied to another version of its provider than its base,
  whether or not the composition writes the file (**Provisional (D-31)**).
- **`unit update`.** For each of the unit's own amendments, the base before
  the update and the base after it are composed. An
  unchanged base carries the copy over untouched, its declared base following
  the lock. A base that changed, because the provider moved or a unit below
  did, **rebases** the copy: the base before, the copy, and the base after are
  merged three ways, by value for a JSON or JSONC file and by line
  otherwise, neither side taking precedence, except that
  insertions both make at one position are both kept with the base's first,
  as in a composition. What both the copy and the update change is left in
  the copy between markers, `current` for the copy and `incoming` for the
  base after the update, and reported; the declared base, the copy, and the
  lock change together, and the author resolves the markers before the next
  build. For a file with no
  merge or that is not text whose base changed, the update writes nothing and
  says so.

## Evidence (non-normative)

Research experiments on copying, textual merging, structural merging, and
value-level merging informed the principle and requirements above. They are
historical evidence, not contract. Their comparative scores are **not**
evidence for any production merge or reconciliation algorithm. They counted a
conservative false conflict and a silent corruption as equally bad, whereas
these contracts rank preventing silent loss above automating merges, so the
scores measure automation rather than safety. The experimental implementations
have not undergone production-quality engineering review, and the production
strategy is not decided (**OC-10**).

### Negative findings

The following failure modes were observed. They justify the direction but do
not establish production readiness. The approved contracts above are written
to avoid them, and implementations must not reintroduce them:

- Line-based merging was unreliable when several units modified the same
  structured file.
- Structural compatibility does not prove domain-level semantic compatibility.
- Inferring list identity can silently produce incorrect results.
- Updating a copied dependency without its exact historical base can silently
  reverse upstream changes.
- Format-preserving writers can produce documents that are syntactically valid
  but semantically wrong.
- Several changes to the same code expression may conflict intrinsically.
