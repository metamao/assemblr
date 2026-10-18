# Registries and Discovery

Assemblr manages a **local registry**: a first-class subsystem, owned by the
CLI, that records which unit sources the user has registered, which versions
those sources have published, and the metadata needed to discover and resolve
units. Remote registry services and marketplace functionality are **outside
Alpha**.

This model replaces the earlier Local Directory and Local Git registry
modalities (see [Superseded model](#superseded-model-non-normative)). The engineering
mechanisms behind it are recorded in
[ADR 0003](../adr/0003-managed-local-registry.md).

## The local registry — Approved

- Assemblr owns the registry's lifecycle and persistent state. Users work with
  it through CLI commands. They do not create registry directories or edit the
  registry's internal files.
- Registry state is **global to the user's Assemblr installation**. It is not
  recreated for each consumer project or unit.
- Registration, inspection, synchronization, and removal are **explicit,
  managed operations**.
- Assemblr initializes registry state **lazily, on first use**. No setup
  command or installation script is required, so installation through Cargo,
  standalone binaries, or any other method behaves the same.
- The registry's internal configuration and storage formats are implementation
  details, not an authoring contract.

## Units are Git repositories — Approved

- Every unit lives in **its own Git repository**, with `manifest.jsonc` at the
  repository root (see
  [units.md](units.md#authoring-repository-convention--approved)). Git is a
  requirement, not a preferred convention.
- Git provides the unit's version **history**, not merely a transport.
- Each published version is a Git tag of the form `v<version>`:

  ```text
  v1.0.0
  v1.1.0
  v2.0.0
  ```

- The tag version must match the exact SemVer `version` declared in the
  manifest **at the tagged commit**. A version tag whose manifest declares a
  different version is **invalid**. That version fails validation. Other
  versions of the unit are not affected.
- Assemblr reads versioned content **without switching or modifying** the
  author's working tree.

A published unit version is identified by:

- its canonical identity, `@owner/unit-name`;
- its exact version;
- its Git provenance: the repository and the tagged commit;
- an immutable content identity.

Open:

- How invalid and moved tags are reported, what happens to a version after a
  moved tag is detected, and how tags that are not of the form `v<version>` are
  treated. **OC-25.**
- Behavior when historical Git objects or registered sources are unavailable.
  **OC-9.**

## Registered sources — Approved

A directory is not a registry, and a directory that contains Git repositories
is not automatically a source. Assemblr knows only the sources that have been
**explicitly registered and validated**. A registered source is one of:

1. **A unit repository**: the Git repository of one unit.
2. **A catalog**: a Git repository that declares a set of unit repositories
   (see [Catalogs](#catalogs--approved)).

The same registry manages both. There are no separate registry types or
operating modes.

## Registration — Approved

Conceptual commands (grammar not final, **OC-7**):

```text
assemblr registry add ./assemblr-cloudflare-worker
assemblr registry add ./metamao-cloudflare
```

Registration identifies the source as a unit repository or a catalog, validates
it, and only then adds it to the registry.

- A missing or invalid Git repository is an error.
- A missing or invalid unit manifest is an error.
- An invalid canonical identity is an error.
- Registering a source that is already registered is **idempotent**.
- A different repository claiming an identity that a registered source already
  provides is an explicit **collision** error (see
  [Source conflicts](#source-conflicts--approved)).
- A version whose tag is inconsistent with its manifest fails validation. Only
  the affected versions fail.
- A unit with **no published tags** may still be registered, for development
  and discovery. It has no published versions until it is published.

Registration **never publishes** a unit. It never turns uncommitted or
untagged working-tree content into a published version. A request for a
published version always obtains tagged content, never the current working
tree.

Open:

- How Assemblr decides that two references denote the same source, for
  example a local path and a remote URL of the same repository. **OC-23.**
- How a catalog's unit references are validated during registration. **OC-32.**

## Source conflicts — Approved

- A repository never becomes available just because it is located somewhere
  Assemblr can see. Every unit repository or catalog must be registered
  explicitly.
- When competing sources claim the same canonical identity, Assemblr **never
  silently selects one**, whether by registration order, configuration order,
  or any other precedence. It reports an actionable diagnostic, and the
  conflict requires an explicit resolution.
- The unit's **manifest** supplies its identity. Neither a catalog nor a
  repository location does.

Open:

- The mechanism for explicitly resolving a conflict. The earlier contract
  allowed an explicit source selection and owner-to-source mappings to
  disambiguate. Whether those mechanisms remain part of this model needs owner
  confirmation. **OC-23.**

## Catalogs — Approved

A catalog organizes references to independently maintained unit repositories.

Conceptual commands (grammar not final, **OC-7**):

```text
assemblr registry create metamao-cloudflare
assemblr registry unit add ../assemblr-cloudflare-worker
assemblr registry add ./metamao-cloudflare
```

- A catalog is itself a **Git repository**, created and managed through the
  Assemblr CLI.
- A catalog has an explicit **catalog identity**. The catalog identity names
  the catalog. It is not a unit owner, and it does not determine the identities
  of the units it lists.
- A catalog contains a formal, **declarative manifest** of its units: each
  unit's identity and Git source.
- A catalog does not contain the unit repositories or embed their source
  trees, and it does not require Git submodules.
- A catalog does not pin a single version of each unit. Each unit keeps its
  independent SemVer history and publication lifecycle.
- Unit repositories referenced by a catalog may be private.
- A catalog can be shared or cloned onto other machines.
- Assemblr validates a catalog's manifest and its unit references. A catalog
  entry does not confer identity: the referenced unit's manifest remains
  authoritative, and a catalog never overrides it.

### Catalog manifest — Recommended (provisional)

The catalog manifest is a declarative JSONC file, provisionally named
`registry.jsonc`. Illustrative content (**Conceptual**, not a frozen schema):

```jsonc
{
  "name": "metamao-cloudflare",
  "units": {
    "@metamao/cloudflare-worker": {
      "git": "git@github.com:metamao/assemblr-cloudflare-worker.git"
    },
    "@metamao/cloudflare-angular-ssr": {
      "git": "git@github.com:metamao/assemblr-cloudflare-angular-ssr.git"
    }
  }
}
```

The manifest identifies the catalog, declares its unit references, preserves
their source provenance, permits portable Git references, and supports
validation by the CLI. Catalog management through the CLI should avoid
requiring manual edits of the manifest. Explicit local paths may be permitted
for development, but they are not portable to other machines.

Open:

- The catalog manifest's schema and file name, the syntax of catalog
  identities, whether local paths are permitted, how a mismatch between a
  catalog entry and the referenced manifest is reported, and whether a catalog
  may reference other catalogs. **OC-32.**

## Synchronization — Approved

Conceptual command (grammar not final, **OC-7**):

```text
assemblr registry sync
```

- Synchronization updates the registry's knowledge of the registered sources
  and their published versions.
- Registration and synchronization are **distinct** operations.
- Discovery commands (`registry list`, `search`, `info`) **never fetch** remote
  changes implicitly.
- Synchronization **validates** updated metadata before making it
  authoritative.
- A failed synchronization **preserves the previously valid registry state**.
  It never leaves a partially updated index.
- A source that has not changed causes no update.
- Synchronization never rewrites a version that has already been recorded (see
  [Integrity and snapshots](#integrity-and-snapshots--approved)).

Open:

- The granularity of synchronization: whether it can target individual
  sources, whether one failing source blocks the others, how per-source
  failures are reported, and how a unit removed from a catalog is reflected.
  **OC-33.**
- Whether newly published tags of a registered local repository become
  visible without synchronization, and whether resolution may fetch.
  **OC-31.**

## Integrity and snapshots — Approved

Assemblr distinguishes five kinds of state, which serve different purposes:

| State | What it is | Character |
|-------|------------|-----------|
| **Working tree** | The author's editable checkout of a unit repository | Mutable. Used by authoring operations such as `unit build`. Never a published version. |
| **Published version** | A `v<version>` tag in the unit repository | Immutable once published. |
| **Registry index** | Discoverable metadata about registered sources and their published versions | Refreshed explicitly by synchronization. |
| **Snapshot** | A fixed, identifiable copy of a unit's content at a resolved version, with its Git provenance | Immutable historical state, needed for composition and amendments. |
| **Lock state** | The exact resolved versions, sources, and content identities of a composition | Preserves a resolution until it is changed explicitly. |

Rules:

- A published version stays associated with **exactly** the content first
  recorded for it. Moving a tag to another commit is not a valid way to update
  a version.
- Assemblr **detects** historical identity or content mismatches, such as a
  moved tag, and reports them. It never silently trusts a moved tag.
- A mutable working tree is never substituted for a published version.
- The registry never silently replaces historical content.
- Resolved versions and content are **identifiable and verifiable**.
- Snapshots and provenance preserve what is needed to reconstruct the exact
  historical files that amendments are based on (see
  [amendments-and-composition.md](amendments-and-composition.md#amendments--approved)).
- Snapshots and lock state support **deterministic reuse** and future
  **reconciliation**.
- A composition that is already resolved does not change just because a newer
  version appears in the registry. Existing lock state takes precedence over
  fresh resolution (see
  [dependencies.md](dependencies.md#resolution--approved)).

Open:

- Packaging, cache, snapshot, and lock-state formats and locations, including
  how content identity is computed and how long historical snapshots are
  retained. **OC-8.**

## Removal — Approved

- Unregistering a unit repository or a catalog removes its **registry
  relationship** only.
- It never deletes the user's Git repositories or working trees.
- Historical artifacts that may still be needed for reconciliation are not
  discarded arbitrarily.

Open:

- How removal affects existing lock state and compositions that reference the
  removed source. **OC-33.**

## Discovery and information — Approved

Assemblr exposes unit metadata that is useful to both humans and coding
agents. The registry makes this metadata discoverable, so a consumer does not
have to inspect source repositories by hand.

Discovery is **metadata-based**. `search` matches against declared unit
metadata in the registry. It does not use semantic or AI-driven inference.

Conceptual commands (grammar not final, **OC-7**):

```text
assemblr registry list
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

## Recommended mechanisms — Recommended (provisional)

The owner has accepted the following as default mechanisms and policies for
Alpha. They are **not** verified guarantees: no prototype of this registry has
been evaluated yet. They may change when technical evidence shows a better
mechanism. A change that affects approved user-visible semantics needs owner
review. [ADR 0003](../adr/0003-managed-local-registry.md) records the
engineering side.

- **Storage locations.** Platform-appropriate application directories hold
  configuration, persistent registry metadata, managed sources, historical
  artifacts, and disposable cache: XDG-compatible paths on Linux, native
  conventions on Windows and macOS. There is no hardcoded `~/.assemblr` path.
  Directory names and internal layout are implementation details.
- **Persistence.** A small, file-based store managed by the CLI, with
  validated, crash-conscious writes. No database unless experiments show the
  simpler approach is insufficient.
- **Validation.** One common validation model across unit registration,
  catalog registration, version discovery, publication, and synchronization.
  Registry mutations are validated before they are committed. Registry
  operations are safely repeatable when their inputs and intended state are
  unchanged. Expensive checks need a specific correctness or integrity reason.
- **Git.** Use Git's existing object and tag infrastructure rather than a
  parallel versioning mechanism. Consider annotated release tags (**OC-26**).
  Authentication to private repositories is delegated to the user's existing
  Git and SSH configuration. Assemblr has no credential manager in Alpha.
- **Snapshots and caches.** Snapshots are immutable and content-addressed, with
  explicit Git provenance. Recoverable caches are kept distinct from persistent
  historical state. Historical snapshots are not garbage-collected
  automatically in Alpha.
- **Version selection.** See
  [dependencies.md](dependencies.md#version-selection--recommended-provisional).
- **Publication and push.** See
  [authoring-workflow.md](authoring-workflow.md#publication--approved).

## Superseded model (non-normative)

Earlier contracts defined two registry modalities: a **Local Directory
registry**, which read units from working directories including uncommitted
changes, and a **Local Git registry**, which read published versions through
tags. Registries were configured locations, possibly directories grouping
several unit repositories, maintained by the user. That model is retired:

- one managed local registry replaces the two modalities;
- explicit registration of unit repositories and catalogs replaces configured
  locations and the discovery of units inside grouping directories;
- CLI-managed state replaces user-maintained registry configuration;
- one Git repository per unit is mandatory, not preferred.

The Local Directory registry let a unit under development resolve dependencies
from other units' uncommitted working trees. This model does not carry that
capability forward by default. Whether and how it is supported is **OC-30**.
