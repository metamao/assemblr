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
- The engine may need **format-aware adapters** and **limited explicit
  metadata** where ambiguity cannot be resolved safely.

Amendments are created with `unit amend` and reconciled with `unit update`
(see [authoring-workflow.md](authoring-workflow.md)).

### The base

- An amendment's base is a file in the **`src/`** of the unit that provides
  it, at an exact published version of that unit. It is never the composed
  output of that unit, so it never includes amendments made by other units.
- Amendments that several units make to the same file are combined **side by
  side** against that original (see
  [merge semantics](#merge-semantics--approved)).
- A unit therefore cannot independently override a value that another unit
  **added or changed through an amendment**: that value is not in its base,
  and setting it to a different value is a conflict. Identical contributions
  from several amenders to one value are compatible and become one change
  (decided by the owner, #33). A unit that wants such a value to be
  configurable exposes it as a parameter of its own, which other units set
  through `options`. Changing a value from another unit's `src/` is an
  ordinary amendment.
- Amendments that build on a dependency's composed output, including that
  dependency's own amendments, are outside Alpha.

### Target

- An amendment targets a **direct dependency** of the amending unit: an
  identity declared in its own `dependencies`, which provides the file's path
  in its `src/`. Amending a file of a transitive dependency, or of any other
  identity, is an error.

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
  [manifest.md](manifest.md#serialization-of-dependencies-options-params-and-amendments--approved)):

  ```jsonc
  "amendments": {
    "wrangler.toml.assemblr.tmpl": { "unit": "@metamao/cloudflare-worker", "base": "2.1.0" }
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
  lockfile: its entry for the target unit records the base version together
  with the commit and tree that version was published with (see
  [authoring-workflow.md](authoring-workflow.md#unit-lock-state--approved)).
  The lockfile is tracked, so a published version of the amending unit carries
  it. A published version with amendments carries a lockfile consistent with
  its declarations.
- Whoever composes an amendment, a dependent unit's build or a consumer
  project, verifies the base: the amending unit's lockfile entry for the
  declared version is compared with the composer's own registry record for
  that unit and version. If they agree, the base is verified. If they differ,
  that is an **integrity conflict**: the composition fails and changes
  nothing (see
  [registries-and-discovery.md](registries-and-discovery.md#integrity-and-snapshots--approved)).
  If the version or its content is unavailable, see **OC-9**.
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
- Whether conflicts between units' contributions in a consumer composition are
  resolved like conflicts with consumer modifications. **OC-47.**

## Template contexts — Approved

Research established that values belonging to the original dependency must be
kept distinct from values owned by the amending unit.

- A file is rendered as a deterministic function of its source and its
  **context**. A unit file's context is the resolved parameter values of the
  unit that provides it (see
  [parameters-and-options.md](parameters-and-options.md#rendering--approved)).
- The namespace **`own.*`** is reserved for the amending author's **own**
  context. `own` is not a valid parameter name, and it is defined only when an
  amendment copy is rendered. Rendering unit files needs only that
  reservation; the behavior below is a requirement on amendment composition.
- An amendment copy is rendered with its base's context, the provider's
  resolved parameter values, plus `own`: the amending unit's own resolved
  parameter values, the same values its `src/` files see.
- The original copy and the amended copy are rendered against **compatible
  base contexts** before changes are derived, so that the derived changes
  reflect the author's edits and not differences between contexts. Expressions
  inherited from the provider render identically on both sides; what remains
  is the author's edit, with the composition's `own` values.
- An amendment copy is rendered exactly when its base file is. A template is
  selected by the `.assemblr.tmpl` suffix of the provider's file (see
  [units.md](units.md#templates)), and the copy at `amendments/<path>` keeps
  that path, suffix included, so it inherits the outcome.
- **Composing and updating are distinct operations.** A composition derives
  and applies changes on **rendered** content, with the composition's own
  contexts. `unit update` reconciles the **source templates**, the base at the
  old version, the copy, and the base at the new version, so that the copy
  keeps its template expressions, including `own.*` and content inside
  template conditions that a particular composition disables.

**Across provider versions — decided by the owner (#59).** When the resolved
version of the provider is not the base, the copy's contribution is derived
from the **base file** and the copy, both rendered with **one** context: the
parameter values the composition gives the provider, with `own` as above. The
contribution is then applied to the provider's file at the resolved version,
rendered with the same context. Because the base and the copy are rendered
alike, what is derived is the author's edit, and a difference between the two
versions of the provider is the provider's own change. A parameter that a
template names (the base file or the copy) must be declared at the resolved
version with the same kind of value (string, integer, or boolean) as at the
base; otherwise the composition fails, naming the amender, the parameter, and
both versions, because the contribution cannot be told from the provider's
change. Assemblr does not fall back to a default the base version declared:
that would be a guess dressed as compatibility. A parameter whose meaning
changes while its name and kind stay is not detected.

Open:

- Whether template constructs beyond references, such as conditional
  content, are ever supported (see
  [parameters-and-options.md](parameters-and-options.md#rendering--approved)).
  **OC-5.**

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
  writer. TOML, YAML, XML, INI, and binary files with several amenders stay
  refused, and a single amender's copy keeps taking the file's place as it
  does today, whatever the format. Other UTF-8 text is combined by line, as
  the next section describes.
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
  [reconciliation details](consumer-projects.md#reconciliation-details--approved)
  implements provisionally.

### Text files — Approved direction, details provisional

Several amendments of one text file are combined by a conservative
**line-level three-way merge**, decided by the owner after the reference
scenario showed that units which each add a section to a shared guidance
file, such as `AGENTS.md`, could not be composed. The details are provisional
(decision log D-20; ADR 0021):

- **Scope.** Any file whose provider file and copies are UTF-8 text without a
  NUL, outside the structured formats the merge does not support: YAML, TOML,
  XML, and INI files, and JSON in another case, stay refused with several
  amenders, because a line merge can produce valid text and wrong data such
  as a key twice. A file or copy that is not text stays refused. A single
  amender's copy keeps taking the file's place whole.
- **Method.** The provider's file and each copy are rendered as for any
  amendment, compared line by line (a line keeps its terminator), and each
  copy's changes against the verified base are combined. The result is the
  base with every amender's changes applied.
- **Compatible changes.** Changes in different places are all kept. An
  identical change by several amenders is one change. Insertions that several
  amenders make at the same position are all kept, each distinct insertion
  once, in the order of the amenders' identities. No amender's lines are
  dropped, and the build and `unit check` report every such position as a
  warning, since the order is not something any unit asked for. This is the
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
  - XML, TOML, or YAML under an extension the merge does not recognize (for
    example `.svg`, `.csproj`, or `Cargo.lock`), which is merged as text.

  Such cases are reported only as the warning for insertions at one position.
  Changes to the same line still conflict. The author verifies the composed
  file with the format's own tools against `build/` (see
  [testing](authoring-workflow.md#testing-native-validation-vs-functional-correctness--approved)),
  and a consumer verifies the composed project the same way. Units
  that need a precise order in such a file arrange that one of them owns the
  file or that their lines do not depend on each other. Only the formats
  listed under **Scope** are refused, by extension; none is refused for being
  order-sensitive (decision log D-27, OC-12).
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

Implementation notes (provisional, decision log D-6 to D-8, corrected by the
review recorded in the decision log):

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
- The format is recognized by the delivered file's extension, after the
  template suffix is removed: `.json` or `.jsonc` is JSON; `.yaml`, `.yml`,
  `.toml`, `.xml`, `.ini`, and `.json` or `.jsonc` in another case are
  structured formats without an adapter; any other path is text.

Open:

- The manifest representation of list identity hints. **OC-4.**
- Further format adapters. **OC-10.** The consumer side is decided
  provisionally, without Git, in
  [reconciliation details](consumer-projects.md#reconciliation-details--approved).

### Composition, builds, and reconciliation

For unit authoring, composition output goes to `build/`, which is rebuilt from
scratch and needs no reconciliation history (see
[authoring-workflow.md](authoring-workflow.md#build--approved)).

**Implemented.** A unit's build applies the amendments of every unit in its
composition: its own, whose providers resolve to the declared bases by
construction of the
[unit lock state](authoring-workflow.md#unit-lock-state--approved), and those
that its resolved dependencies publish, direct or transitive. The
direct-dependency restriction binds the amender to the provider, not the
amender to the unit being built. A published amender's bases are verified
against the composer's registry record before composition, as approved
above. With the base and the resolved version equal, one amender's result is
the rendered copy, and no change is derived. Otherwise, or when several
amenders share a file, the changes are derived against each copy's base and
combined on the rendered content: by [the merge engine](#the-merge-engine--approved)
for a JSON or JSONC file, and by line for text, with the provider's file and
each copy rendered with the provider's values, the copy with its amender's as
`own`, and applied to the provider's file at the version resolved ([across
provider versions](#across-provider-versions--approved)). The cases that
cannot be applied fail explicitly, naming each amender, its version, the
provider, the base, and the resolved version, and change nothing (standing
default D2): a collision, a parameter that changed meaning, a path the
provider no longer has, a provider file in a structured format without an
adapter or that is not text that changed between the two versions, and such
a file that several units amend. `unit update` carries a unit's own
amendment to the provider's new version, rebasing the copy when the file
changed, and refuses the update, changing nothing, on a collision (see
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
  A collision, in a value or in lines of text, is a composition error naming
  the amender, the file, and the place, with no precedence and no overwrite.
  The project or unit is left unchanged.
- **Flat model.** Each amender's contribution is derived against its own base.
  Amenders taken from different versions of the provider stay independent:
  applying one never establishes the base or the precedence of another.
- **Formats without a merge.** A YAML, TOML, XML, or INI file, and a file that
  is not UTF-8 text, is applied across versions only when the provider's file
  has the same bytes at the base and at the resolved version. Otherwise the
  composition is refused, with no fallback and no replacement.
- **Rendering.** As described under
  [template contexts](#template-contexts--approved).
- **The unit's own build** keeps its rule: the declared base is the version the
  unit is locked to (`unit amend`, `unit build`). Only dependents' builds and
  consumer compositions apply across versions.
- **Verification.** Bases are verified exactly as before, against the registry's
  record of the base version; the base file is read from that immutable
  version. Results depend on the files at the two versions, the copy, and the
  composition's values only, so they are deterministic.
- **Visibility.** A successful cross-version application is an ordinary
  successful operation: it adds no note to a composition. A merge that is
  clean shows structural compatibility, not that the result is correct for the
  software, which the author and the consumer verify with the project's own
  tools.
- **`unit update`.** For each of the unit's own amendments whose provider
  moves, an unchanged file is carried over untouched; a file that changed is
  **rebased** on the source templates: the changes the copy makes against the
  old base are merged by line into the file at the new version, neither side
  taking precedence, and the declared base, the copy, and the lock change
  together. Source templates are merged as text, never as structured
  documents, since a template need not be one before it is rendered; a
  rebased JSON file that is not a template must still parse as JSON. On a
  collision, and for a file with no merge or that is not text, the update
  writes nothing and reports each collision with its lines, the provider's
  change, and the amendment's.

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
- Changes hidden by disabled template conditions can silently disappear.
