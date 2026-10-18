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

Open:

- The manifest syntax for declaring amendments. No earlier experimental
  syntax, such as `extends` or a `merges` field, is canonical. **OC-2.**
- The on-disk layout and naming of amendment copies, and how provenance
  metadata is stored. **OC-3.**
- Whether an amendment can delete an entire dependency file, or only change its
  content. **OC-19.**
- Behavior when a base's historical Git objects are unavailable. **OC-9.**

## Template contexts — Approved

Research established that values belonging to the original dependency must be
kept distinct from values owned by the amending unit.

- The namespace **`own.*`** is reserved for the amending author's **own**
  context.
- The original copy and the amended copy are rendered against **compatible
  base contexts** before changes are derived, so that the derived changes
  reflect the author's edits and not differences between contexts.

Open:

- The exact rendering protocol for originals and amendments, consistent with
  the experimental findings, including exactly what the `own.*` context
  contains. **OC-5.**

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

Open:

- The manifest representation of list identity hints. **OC-4.**
- Production merge algorithms and adapter selection. **OC-10.**

### Composition, builds, and reconciliation

For unit authoring, composition output goes to `build/`, which is rebuilt from
scratch and needs no reconciliation history (see
[authoring-workflow.md](authoring-workflow.md#build--approved)).

The same conceptual mechanism could reconcile downstream modifications using
three inputs: **base** (previous generated output), **current** (the current
file), and **incoming** (the newly generated composition). This direction is
supported by research. Its application to consumer projects belongs to the
reserved consumer-project domain (**OC-1**).

## Evidence (non-normative)

This section summarizes the research behind the direction above. It is
evidence, not contract. The experimental implementations have **not**
undergone production-quality engineering review and do not establish
production readiness.

### Experiments

- An initial experiment investigated copying dependency files and merging them
  textually.
- A follow-up experiment compared textual, structural, and value-level merging.
  It evaluated seven approaches on 56 representative scenarios drawn from real
  catalog and template files, and on 800 randomized pairs.

The strongest approach scored **53/56 acceptable outcomes** and
**800/800 randomized pairs**. An *acceptable outcome* is a correct result, a
correctly reported conflict, or explicitly reported unsupported residue. It
does not necessarily mean the files were merged automatically.

That approach used **full-file copies for authoring and value-level
operations for composition and reconciliation**. It derived changes from a
modified copy relative to its original, combined operations across
contributors, checked expected previous values, and wrote results through
format-aware adapters.

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
