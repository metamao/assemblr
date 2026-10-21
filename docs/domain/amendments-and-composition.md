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

Open:

- The contexts used to render a base, and the file at the resolved version,
  when the resolved version of the provider is not the base version, for
  example when their parameters differ. **OC-52.**
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
  writer. TOML, YAML, text, and binary files with several amenders stay
  refused, and a single amender's copy keeps taking the file's place as it
  does today, whatever the format.
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
  base in `unit update` (**OC-53**) stay deferred and refused.
- **Stability.** Results are deterministic for the same inputs and engine
  version. Byte-identical formatting across Assemblr releases is not
  promised; semantic compatibility is, and a consequential change to merge
  behavior is an explicit decision.
- The same merge model is the foundation for consumer reconciliation, which
  [reconciliation details](consumer-projects.md#reconciliation-details--approved)
  implements provisionally.

Implementation notes (provisional, decision log D-6 to D-8):

- Object member order carries no meaning in JSON, so members that several
  amenders add at the same place are placed in a fixed order, by amender
  identity, which never changes the document's value; a format where order
  means something gets a conflict under OC-12 when it is supported.
- What the value model cannot carry from a copy, a comment the copy adds or
  a member order it changes, is reported as a warning naming the amender,
  and the composition proceeds with the provider's layout.
- The format is recognized by the delivered file's extension, `.json` or
  `.jsonc`, after the template suffix is removed.

Open:

- The manifest representation of list identity hints. **OC-4.**
- Further format adapters, and how an amendment is applied to a provider
  version other than its base. **OC-10.** The consumer side is decided
  provisionally, without Git, in
  [reconciliation details](consumer-projects.md#reconciliation-details--approved).

### Composition, builds, and reconciliation

For unit authoring, composition output goes to `build/`, which is rebuilt from
scratch and needs no reconciliation history (see
[authoring-workflow.md](authoring-workflow.md#build--approved)).

**Implemented so far.** A unit's build applies the amendments of every unit
in its composition: its own, whose providers resolve to the declared bases
by construction of the
[unit lock state](authoring-workflow.md#unit-lock-state--approved), and
those that its resolved dependencies publish, direct or transitive, when the
provider resolves to the declared base. The direct-dependency restriction
binds the amender to the provider, not the amender to the unit being built.
A published amender's bases are verified against the composer's registry
record before composition, as approved above. With the base and the resolved
version equal, one amender's result is the rendered copy, and no change is
derived. Several amenders of one JSON or JSONC file are combined by
[the merge engine](#the-merge-engine--approved) on the rendered content:
the provider's file and each copy rendered with the provider's values, the
copy with its amender's as `own`. The cases that need the open decisions
above fail explicitly, naming each amender, its version, the provider, the
base, and the resolved version, and change nothing (standing default D2):
an amendment whose provider resolves to a version other than its base is
not applied, and a provider file of any other format that several units
amend, in any combination of the unit being built and published amenders,
is not amended by any of them. No re-basing is performed. `unit update`
carries a unit's own amendment to the provider's new version only when the
amended source file is unchanged between the two versions, and refuses the
update otherwise (see
[authoring-workflow.md](authoring-workflow.md#update-details--approved)).

Consumer projects reconcile downstream modifications with three inputs: the
**reconciliation base** (the previously materialized composition), **current**
(the consumer's actual files), and **incoming** (the newly desired
composition). The merge semantics above apply. See
[consumer-projects.md](consumer-projects.md#reconciliation--approved).

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
