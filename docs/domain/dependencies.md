# Dependencies

## Declaration — Approved

- A unit declares its dependencies **explicitly** in its manifest.
- Each dependency is an **owner-qualified identity** (`@owner/unit-name`) with
  a **SemVer range**.
- Dependencies are never inferred from owner names, unit names, repository
  layout, catalog membership, or registry structure.

The serialization of `dependencies` is not final (see
[manifest.md](manifest.md#vocabulary--approved)).

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
- Lock state records the **exact** resolved versions.
- Existing lock state **takes precedence** over fresh resolution. A composition
  that is already resolved does not change just because a newer version
  appears in the registry.
- A resolved published version is obtained from its tagged content, never from
  a working tree.

These are errors:

- **Dependency cycles.**
- **Missing dependencies**: no registered source provides the identity.
- **Incompatible versions**: no single version satisfies every declared range
  for an identity.
- **Source conflicts**: several sources claim the same identity (see
  [registries-and-discovery.md](registries-and-discovery.md#source-conflicts--approved)).

Conflicts are **reported, never silently resolved**. Resolution must not pick
a winner by source precedence, declaration order, or installation order.

Transitive dependencies follow the same resolution and source-conflict rules
as direct ones.

Open:

- The policy for choosing among several versions that satisfy all ranges (for
  example, the highest compatible version). Whatever policy is chosen, it must
  be deterministic. **OC-16.**
- The SemVer range dialect, and pre-release and build metadata handling.
  **OC-15.**
- The lock-state file format and location. **OC-8.**
- Whether a unit under development may resolve a dependency to an unpublished
  working tree. **OC-30.**

## Version selection — Recommended (provisional)

Until the resolver contract is finalized (**OC-15**, **OC-16**), the owner has
accepted this default policy for Alpha:

- When existing locked resolutions satisfy every requirement, they are
  preserved.
- A new resolution considers the **full transitive constraint set**.
- Among the versions that satisfy it, the **highest compatible stable
  published version** is preferred.
- Cargo-like SemVer requirement semantics are the starting point for the range
  dialect.

The approved rules above still apply: unsatisfiable constraints are reported,
and an incompatible version is never chosen silently.

## Explicit updates — Approved

A dependency is never updated just because a newer version becomes available.
Updating is an explicit author action (see
[authoring-workflow.md](authoring-workflow.md#update--approved)).
