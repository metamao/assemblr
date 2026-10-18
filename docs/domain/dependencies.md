# Dependencies

## Declaration — Approved

- A unit declares its dependencies **explicitly** in its manifest.
- Each dependency is an **owner-qualified identity** (`@owner/unit-name`) with
  a **SemVer range**.
- Dependencies are never inferred from owner names, unit names, repository
  layout, or registry structure.

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

These are errors:

- **Dependency cycles.**
- **Missing dependencies**: no source provides the identity.
- **Incompatible versions**: no single version satisfies every declared range
  for an identity.
- **Ambiguous sources**: the same identity is found in several sources and no
  explicit selection or mapping applies (see
  [registries-and-discovery.md](registries-and-discovery.md#source-selection--approved)).

Conflicts are **reported, never silently resolved**. Resolution must not pick
a winner by source precedence, declaration order, or installation order.

Transitive dependencies follow the same resolution and source-selection rules
as direct ones.

Open:

- The policy for choosing among several versions that satisfy all ranges (for
  example, the highest compatible version). Whatever policy is chosen, it must
  be deterministic. **OC-16.**
- The SemVer range dialect, and pre-release and build metadata handling.
  **OC-15.**
- The lock-state file format and location. **OC-8.**

## Explicit updates — Approved

A dependency is never updated just because a newer version becomes available.
Updating is an explicit author action (see
[authoring-workflow.md](authoring-workflow.md#update--approved)).
