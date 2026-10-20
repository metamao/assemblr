# Dependencies

## Declaration — Approved

- A unit declares its dependencies **explicitly** in its manifest.
- Each dependency is an **owner-qualified identity** (`@owner/unit-name`) with
  a **SemVer range**.
- Dependencies are never inferred from owner names, unit names, repository
  layout, catalog membership, or registry structure.

`dependencies` is a map from identity to SemVer requirement (see
[manifest.md](manifest.md#serialization-of-dependencies-options-and-params--approved)).

## Resolution — Approved

Units may have recursive, transitive dependencies. For example:

```text
@metamao/cloudflare-angular-ssr
    -> @metamao/cloudflare-worker
        -> @metamao/node
```

Within a single composition:

- Missing dependencies are resolved **recursively**.
- Compatible shared dependencies are **reused**.
- One logical unit identity resolves to **exactly one version**.
- Resolution is **deterministic**: the same inputs and history give the same
  result.
- Lock state records the **exact** resolved versions. A unit repository keeps
  it in a unit lockfile (see
  [authoring-workflow.md](authoring-workflow.md#unit-lock-state--approved)), and
  a consumer project in its own lockfile (see
  [consumer-projects.md](consumer-projects.md#lock-and-reconciliation-state--approved)).
- Existing lock state **takes precedence** over fresh resolution. A composition
  that is already resolved does not change just because a newer version
  appears in the registry.
- Dependencies resolve **exclusively** from published, tagged versions
  available through the managed registry. A resolved version is obtained from
  its tagged content, never from a working tree.
- There are **no unpublished dependency overrides** in Alpha. A unit under
  development never consumes another unit's working tree as a dependency.
- Resolution during `unit check`, `unit build`, and `unit update` never
  fetches remote changes implicitly.

These are errors:

- **Dependency cycles.**
- **Missing dependencies**: no registered source provides the identity.
- **Unavailable versions**: a required published version is not available
  locally. The diagnostic reports that synchronization is needed.
- **Incompatible versions**: no single version satisfies every declared range
  for an identity.
- **Source conflicts**: several sources claim the same identity (see
  [registries-and-discovery.md](registries-and-discovery.md#source-conflicts--approved)).

Conflicts are **reported, never silently resolved**. Resolution must not pick
a winner by source precedence, declaration order, or installation order.

Transitive dependencies follow the same resolution and source-conflict rules
as direct ones.

Open:

- Which version is chosen for a newly incorporated unit when several satisfy
  every requirement. Whatever policy is chosen, it must be deterministic.
  **OC-16.**
- The SemVer range dialect, and pre-release and build metadata handling.
  **OC-15.**
- The lock-state file formats and locations: the consumer lockfile (**OC-8**)
  and the unit lockfile (**OC-46**).

## Version selection — Approved

In a consumer project, an operation that changes the composition's
requirements, such as `add` (see
[consumer-projects.md](consumer-projects.md#version-resolution--approved)),
follows the policy of **minimal necessary updates**:

- Locked versions that remain compatible are reused.
- Newly incorporated units get compatible **stable** published versions.
- A previously locked unit is upgraded only when the new constraints require
  it.
- Explicit requirements and pins are respected.
- Each unit identity keeps a single effective version.
- An unsatisfiable composition is rejected. A conflicting version is never
  chosen silently.
- Every dependency version changed as a consequence of the operation is
  explained.

Explicit update operations are distinct from these minimal changes (see
[Explicit updates](#explicit-updates--approved)).

### Recommended (provisional)

Until the resolver contract is finalized (**OC-15**, **OC-16**), the owner has
accepted these defaults for Alpha:

- When existing locked resolutions satisfy every requirement, they are
  preserved.
- A new resolution considers the **full transitive constraint set**.
- Among the versions that satisfy it, the **highest compatible stable
  published version** is preferred.
- Cargo-like SemVer requirement semantics are the starting point for the range
  dialect.

The owner has also accepted these details for Alpha. The related entries stay
open until the owner settles them: **OC-15** and **OC-16**.

- **Pre-releases** follow Cargo's matching rules. A pre-release matches only a
  requirement that names a pre-release of the same `major.minor.patch`, so it
  is never selected implicitly: `^1.0.0` and `*` never match `1.1.0-beta.1`.
  A requirement that names one, such as `^1.1.0-beta.1`, may select that
  pre-release, a later pre-release of `1.1.0`, or a higher release, and the
  highest of them is preferred. **OC-15.**
- **Build metadata** is part of a published version's identity: `1.0.0+a`
  and `1.0.0+b` are distinct versions. It is ignored when matching a
  requirement. When both satisfy every requirement, the one whose metadata
  sorts higher is preferred. **OC-15.**
- A **locked version is kept** when it satisfies every requirement under these
  rules and the dependencies of the locked versions can still be resolved.
  Otherwise it is replaced by the highest compatible version, as in a new
  resolution. A locked pre-release that no requirement names any more is
  therefore replaced.
- When raising one unit's version would force another unit's version down,
  the unit decided **first** keeps the higher version. A unit is decided
  before the units its published versions depend on, and otherwise in
  identity order. **OC-16.**
- **Diagnostics.** A requirement that no locally available version satisfies
  is reported as incompatible, with the versions that are available. An
  *unavailable version* is a locked version that still satisfies every
  requirement but is not available locally. A missing dependency or an
  unavailable locked version is always reported. Resolution never avoids one
  by choosing an older version of the unit that requires it.

## Explicit updates — Approved

A dependency is never updated just because a newer version becomes available.
Updating is an explicit author action (see
[authoring-workflow.md](authoring-workflow.md#update--approved)). Consumer
projects keep the same principle. Their explicit update operations are not yet
defined (**OC-38**).
