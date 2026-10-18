# Registries and Discovery

A **registry** is a configured local source of units. Assemblr Alpha supports
two native local registry modalities. Remote registry services and marketplace
functionality are **outside Alpha**.

## Local Directory registry — Approved

- Reads unit sources from local **working directories**.
- Supports **uncommitted changes**, for development.
- Does not require publication.
- Useful for authoring, iteration, and local integration.

A directory source is mutable. When a composition has already resolved a
snapshot from it, later changes to the directory must **not silently change
that snapshot**.

## Local Git registry — Approved

- Reads **published** unit versions from local Git repositories.
- Resolves versions through **tags**.
- Reads versioned content **without switching or modifying the working
  tree**.
- Treats published content as **immutable**.

With one repository per unit, the tag for each version is `v<version>`:

```text
v1.0.0
v1.1.0
v2.0.0
```

A tag must match the version in the manifest it points to **exactly**. A
version tag whose manifest declares a different version is **invalid**.

Open:

- How invalid tags are reported, and how tags that are not of the form
  `v<version>` are treated. **OC-25.**
- Behavior when historical Git objects are unavailable. **OC-9.**

## Registry configuration — Approved

- A configured registry may point to **a single unit** or to **a directory
  grouping** separately maintained unit repositories.
- Assemblr supports **several** configured local registries.
- A **registry name** is a local identifier for a source. It is **not** a
  unit owner.
- A unit's **manifest** supplies its identity, not the registry.

Open:

- Where and how registries are configured, and their configuration syntax.
  **OC-23.**
- How units are discovered inside a grouping directory, for example search
  depth and nested repositories. **OC-24.**

## Source selection — Approved

When resolving a unit identity, including transitive dependencies:

1. An **explicit source selection** takes priority.
2. Otherwise, an explicit **owner-to-source mapping** may disambiguate which
   source provides an owner's units.
3. Otherwise, a unit found in **several sources is ambiguous**, and the
   ambiguity is reported.

Assemblr does **not** silently apply source precedence, for example
configuration order.

Open:

- The syntax and scope of explicit source selection and owner-to-source
  mappings. **OC-23.**

## Integrity and snapshots — Approved

- Resolved versions and content must be **identifiable and verifiable**.
- Local snapshots and lock information must support **deterministic reuse**
  and future **reconciliation**.
- A mutable directory source must not silently change an existing resolved
  snapshot.

Open:

- Packaging, cache, snapshot, and lock-state formats, including how content
  identity is computed. **OC-8.**

## Discovery and information — Approved

Assemblr will expose unit metadata that is useful to both humans and coding
agents.

Discovery is **metadata-based**. `search` matches against declared unit
metadata. It does not use semantic or AI-driven inference.

Conceptual commands (grammar not final, **OC-7**):

```text
assemblr search cloudflare
assemblr info @metamao/cloudflare-worker
assemblr info @metamao/cloudflare-worker --json
```

`info` describes:

- identity and version;
- the unit description;
- public parameters, with descriptions, types, requiredness, and defaults;
- allowed values;
- declared dependencies;
- dependency configuration requirements (`options`);
- relevant resolution and source information, where available.

Human-readable and machine-readable output are derived from the **same domain
information**. Neither is a separately maintained view.

Not in Alpha:

- agent-only prompt fields or instructions in manifests;
- semantic or AI-based discovery or recommendation.

Open:

- The exact matching rules for `search`: which metadata fields are matched,
  how, and how results are ordered. Ordering must be deterministic.
  **OC-29.**
