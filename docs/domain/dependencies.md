# Dependencies

## Declaration — Approved

- A unit declares its dependencies **explicitly** in its manifest.
- Each dependency is an **owner-qualified identity** (`@owner/unit-name`) with
  a **SemVer range**.
- Dependencies are never inferred from owner names, unit names, repository
  layout, catalog membership, or registry structure.

`dependencies` is a map from identity to SemVer requirement (see
[manifest.md](manifest.md#serialization-of-dependencies-options-params-and-amendments--approved)).

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
- The SemVer range dialect. **OC-15.**
- The consumer lockfile's format and location (**OC-8**). The unit
  lockfile is `assemblr-lock.json` (see
  [unit lock state](authoring-workflow.md#unit-lock-state--approved)); which
  operations beyond `unit build` write it is still open (**OC-46**).

## Pre-releases and build metadata — Approved

Version requirements follow Cargo's matching rules for pre-releases:

- A pre-release version matches only a requirement that names a pre-release
  of the same `major.minor.patch`. A pre-release is therefore never selected
  implicitly: `^1.0.0` and `*` never match `1.1.0-beta.1`.
- A requirement that names a pre-release, such as `^1.1.0-beta.1`, may select
  that pre-release, a later pre-release of `1.1.0`, or a higher release. The
  highest of them is preferred.

Build metadata never orders versions, as in SemVer:

- Build metadata is ignored when matching a requirement.
- A unit's published versions must be unique ignoring build metadata (see
  [versioning](units.md#versioning--approved)). When an identity's published
  versions include two that differ only in build metadata, such as `1.0.0+a`
  and `1.0.0+b`, resolution never selects a version of that identity. It
  reports the versions as ambiguous.

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

The owner has also accepted these details for Alpha. The related entry stays
open until the owner settles it: **OC-16**.

- A **locked version is kept** when it satisfies every requirement and the
  dependencies of the locked versions can still be resolved. Otherwise it is
  replaced by the highest compatible version, as in a new resolution. A
  locked pre-release that no requirement names any more is therefore
  replaced.
- A candidate version is **passed over**, and the next one tried, when it
  conflicts with other requirements, depends on an identity that no
  registered source provides or whose published versions are ambiguous, or
  closes a dependency cycle. Resolution fails only when no combination of
  available versions avoids all of these.
- A locked version that still satisfies every requirement but is **not
  available locally** is never passed over. Resolution fails and reports that
  synchronization may be needed, because the lock takes precedence over fresh
  resolution.
- **Selection order.** Resolution is deterministic, but its result is not
  promised to be optimal by any measure. When raising one unit's version
  would force another's down, the unit decided first keeps the higher
  version. A unit is decided before the units its published versions depend
  on, and otherwise in identity order. Declaration order and the order in
  which the registry lists versions never affect the result. This rule may
  change while OC-16 is open.
- **Diagnostics** distinguish an identity that no registered source provides,
  a range that no locally available version satisfies (listing the available
  versions), an identity whose published versions are ambiguous, and a locked
  version that is not available locally. When resolution fails, the
  diagnostics explain why the **preferred** versions fail: the locked
  versions, or else the highest. Other versions were tried too, but the
  diagnostics do not list every reason each of them failed.

## Explicit updates — Approved

A dependency is never updated just because a newer version becomes available.
Updating is an explicit author action (see
[authoring-workflow.md](authoring-workflow.md#update--approved)). Consumer
projects keep the same principle. Their explicit update operations are not yet
defined (**OC-38**).
