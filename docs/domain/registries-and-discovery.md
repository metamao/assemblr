# Registries and Discovery

Assemblr manages a **local registry**: a first-class subsystem, owned by the
CLI, that records which unit sources the user has registered, which versions
those sources have published, and the metadata needed to discover and resolve
units. Remote registry services and marketplace functionality are **outside
Alpha**.

This model replaces the earlier Local Directory and Local Git registry
modalities (see [Superseded model](#superseded-model-non-normative)). The
engineering mechanisms behind it are recorded in
[ADR 0003](../adr/0003-managed-local-registry.md),
[ADR 0004](../adr/0004-registry-validation-and-synchronization.md), and
[ADR 0005](../adr/0005-catalog-metadata-index.md).

## The local registry — Approved

- Assemblr manages **one local registry per user**, global to the user's
  Assemblr installation. It is not recreated for each consumer project or unit.
- Assemblr owns the registry's lifecycle and persistent state. Users register
  and administer sources through CLI commands. They do not create registry
  directories or edit the registry's internal files. (A catalog's
  `registry.jsonc` is not an internal registry file: it belongs to the
  catalog's own repository. See [Catalogs](#catalogs--approved).)
- The registry maintains a **validated local index** of known units, their
  published versions, and their sources.
- Registration, inspection, synchronization, removal, and diagnosis are
  **explicit, managed operations** (see
  [Registry commands](#registry-commands--approved)).
- Assemblr initializes registry state **lazily, when first needed**. No setup
  command or installation script is required, so installation through Cargo,
  standalone binaries, or any other method behaves the same.
- Project lock state is **independent of registry synchronization** (see
  [Integrity and snapshots](#integrity-and-snapshots--approved)).
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

- Published versions are recognized **only** through tags of the form
  `v<version>`, where `<version>` is a valid SemVer version. Tags outside this
  convention, such as `release-1` or `latest`, are not published versions.
- The tag version must match the exact SemVer `version` declared in the
  manifest **at the tagged commit**. A version tag whose manifest declares a
  different version is **invalid**.
- An invalid version never enters the usable version index, and it is reported
  explicitly (see [Registration](#registration--approved)).
- Assemblr reads versioned content **without switching or modifying** the
  author's working tree.

## Identity, versions, integrity, and sources — Approved

Four concepts are distinct:

| Concept | Meaning |
|---------|---------|
| **Unit identity** | `@owner/unit-name`, declared by the unit's manifest. |
| **Published version** | An exact SemVer version, associated with a `v<version>` Git tag. |
| **Artifact integrity** | The verifiable content of a published version. |
| **Source** | The Git repository reference from which the unit is obtained. |

A published unit version is therefore identified by its identity, its exact
version, its Git provenance (the source and the tagged commit), and an
immutable content identity.

Validation establishes **consistency and integrity**. It does **not**
authenticate ownership of an `@owner` namespace. Publisher authentication and
trust policy are not defined, and are not part of Alpha (see
[units.md](units.md#identity--approved)).

Diagnostics:

- Every excluded version tag is reported, with the reason, as a diagnostic
  (stderr). Tags that do not follow the `v<version>` convention, including
  tags that resemble it such as `v1.0`, are ignored and not mentioned.

Open:

- How integrity conflicts and other rejections are presented beyond that;
  and whether publication rejects a version that differs from a published one
  only in build metadata. **OC-25.**
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
assemblr registry add git@github.com:metamao/cloudflare-catalog.git
```

Every incoming unit repository or catalog is **untrusted until validated**.
`registry add` identifies the source as a unit repository or a catalog and
validates it **before** incorporating it into the active registry. The checks
include:

- the catalog's schema and identity;
- the declared source references;
- Git repository validity and accessibility;
- consistency of the unit identity with its historical manifests at published
  tags, and with the identity a catalog declares for it;
- agreement between published Git tags and manifest versions;
- the integrity of published artifacts;
- conflicts with sources that are already registered;
- internal inconsistencies within a catalog, including its
  [metadata index](#catalog-metadata-index--approved).

For catalog-managed units, these checks apply to the catalog's generated
metadata to the extent described in
[Metadata ingestion and trust](#metadata-ingestion-and-trust--approved).

Rules:

- Registration is **atomic**. If a required validation fails, the operation is
  rejected and the active registry is left unchanged. It is never partially
  altered.
- Validation **never executes** unit application code or any other code
  supplied by a source.
- Registration does **not** assume that the dependencies of the units it
  registers are already available. Dependency resolution is a separate
  responsibility (see [dependencies.md](dependencies.md#resolution--approved)).
- Registering a source that is already registered is **idempotent**: adding a
  reference that is already registered changes nothing. Newly published
  versions are indexed by [synchronization](#synchronization--approved), not by
  registering again.
- A unit with **no published tags** may still be registered, for discovery. It
  has no published versions until it is published.
- When a unit repository has **no valid published version**, its identity is
  taken from `manifest.jsonc` **as committed at `HEAD`**. This does not make
  `HEAD` a published version, and uncommitted or staged working-tree content is
  never read. If `HEAD` has no commit, or the committed manifest is missing or
  invalid, the registration fails explicitly and changes nothing.
- Once registered, a source's identity is **fixed**. Every version found later
  must declare it, and synchronization never changes it, silently or
  otherwise. Renaming a unit requires removing its registration and
  registering it again.
- Registration **never publishes** a unit. It never turns uncommitted or
  untagged working-tree content into a published version. A request for a
  published version always obtains tagged content, never the current working
  tree.

### Local repository references — Approved

A local unit repository is registered by its path, and recorded as a
reference:

- The path is made **absolute** against the caller's working directory and
  **normalized lexically**: `.` and `..` components are resolved by name,
  without consulting the file system. Symbolic links are **not** resolved.
- Stored references are compared **exactly**. No file-system identity, implicit
  symbolic-link resolution, or case folding decides that two references are the
  same. Two paths to the same repository, for example through a symbolic link
  or in a different letter case on a case-insensitive file system, are
  different references, and if they declare the same identity they are a
  [source conflict](#source-conflicts--approved).
- Portability: a local reference is meaningful only on the machine and in the
  directory layout where it was registered. Moving or renaming the repository
  makes the source unreadable until it is removed and registered again.
  Assemblr does not relocate a registered source automatically; whether an
  explicit mechanism exists beyond removing and correcting a source is
  **OC-23**. Where
  `..` crosses a symbolic link, the lexical result can differ from the path the
  operating system would resolve. How lock state records local sources across
  machines is **OC-49**.
### Invalid versions and failed sources — Approved

Validation distinguishes two kinds of failure:

- **An invalid version** affects only that version. Example: a `v<version>`
  tag whose manifest declares a different version. The version is excluded:
  it never enters the usable version index, and it is reported explicitly.
  The rest of the operation continues.
- **A source-level failure** invalidates the source, and with it the
  operation. Examples: a repository that is not a valid or accessible Git
  repository, an invalid catalog manifest, and a source conflict. The
  registration or synchronization is rejected under the atomicity guarantees
  above, and the active registry is left unchanged.

For unit repositories, the checks are classified as follows:

- **Version-level**, for a newly discovered version tag: a tag that does not
  point at a commit; a commit or root tree larger than Assemblr reads; a
  missing, oversized, non-UTF-8, or invalid manifest; a manifest declaring a
  different version; and invalid `src/` content, such as a symbolic link, a
  submodule, an invalid path, or files larger than Assemblr reads. The tag is
  excluded and reported, and the operation continues. Content is validated
  when a version is first indexed, so an indexed version is a valid published
  artifact, not only a valid manifest.
- **Version-level, until published amendments are composed:** a newly
  discovered version whose manifest declares amendments, or whose tree holds
  `amendments/`. Its amendment content is not validated yet, and a composition
  cannot apply it, so it is excluded and reported rather than indexed as a
  valid published artifact. The increment that composes published amendments
  replaces this rule with the validation of amendments (OC-33).
- **Source-level**: a repository that cannot be read; valid versions that
  declare different unit identities, or an identity other than the one the
  source is registered for; a source conflict; two or more published versions
  of **equal SemVer precedence**, including versions that differ only in build
  metadata, which no metadata-based preference resolves; and any integrity
  violation of a version already indexed (below).
- An excluded tag is not recorded. If it later publishes a valid version, for
  example after it is moved to a valid commit, a later synchronization indexes
  it as newly discovered. Integrity protection applies to versions that have
  been indexed.
- A version **already indexed** is never dropped silently. If its tag
  disappears, moves to another commit, or no longer publishes a valid version,
  that is an **integrity conflict**: the synchronization is rejected, across
  the entire registry and not only for the affected source, and the index is
  left unchanged.

Open:

- The classification of checks not covered above, such as those of catalogs
  and their metadata indexes. **OC-33.**

## Source conflicts — Approved

- A repository never becomes available just because it is located somewhere
  Assemblr can see. Every unit repository or catalog must be registered
  explicitly.
- **Different** source references that declare the same unit identity are
  **conflicting sources**.
- **Identical** source references that declare the same unit identity may be
  deduplicated.
- Assemblr never infers that two different references denote the same
  repository: no URL normalization, shared Git history, common commits, or
  similar heuristics. No global repository identifier or other additional
  identity mechanism exists.
- Matching content for a version does **not** establish that two repositories
  are the same source.
- **Different content** for the same identity and published version is an
  **integrity conflict**.
- Changing the source reference of an identity that is already registered is a
  conflict that requires user intervention. It is never accepted silently as a
  source migration.
- Assemblr never resolves a conflict automatically or by source priority,
  whether registration order, configuration order, or any other precedence. It
  reports an actionable diagnostic, and the user resolves the conflict
  explicitly, for example by removing or correcting a source.
- The unit's **manifest** supplies its identity. Neither a catalog nor a
  repository location does.

Open:

- Whether any explicit resolution mechanism exists beyond removing or
  correcting a source. The earlier contract allowed an explicit source
  selection and owner-to-source mappings. Whether those remain part of this
  model needs owner confirmation. **OC-23.**

## Catalogs — Approved

A catalog is a **Git repository** containing a declarative `registry.jsonc`
that references independent unit repositories. It is an **inventory of
sources**, not a package repository, and it has no authority over unit
contents.

```jsonc
{
  "name": "@metamao/cloudflare",
  "units": {
    "@metamao/cloudflare-worker": "git@github.com:metamao/worker.git",
    "@metamao/node": "../assemblr-node"
  }
}
```

- `name` is the **catalog identity**. It uses the same `@owner/name`
  convention and syntax rules as units (see
  [units.md](units.md#identity--approved)). Catalog identities and unit identities occupy
  **separate namespaces**.
- `units` maps each expected unit identity **directly** to a Git repository
  reference. A reference is a **plain string**, not an object.
- HTTPS, SSH, and local Git repository references are permitted.
- A relative local reference resolves against the **catalog's root**, never
  against the caller's working directory. Local references may not be
  portable between environments.
- A referenced unit's manifest must declare **exactly** the identity the
  catalog declares for it. The manifest remains authoritative, and a catalog
  never overrides it.
- `registry.jsonc` does **not** duplicate unit versions, dependencies,
  parameters, or content hashes, and it does not pin unit versions. The
  metadata consumers need is published separately, in a generated
  [metadata index](#catalog-metadata-index--approved). Indexing adds no fields
  to `registry.jsonc`, and its references stay plain strings.
- A catalog does not contain the unit repositories or embed their source
  trees, and it does not require Git submodules.
- Each unit keeps its own repository, manifest, Git tags, and publication
  lifecycle. Unit repositories may be private.
- A unit does **not** need to belong to a catalog. It may be registered
  individually, referenced by one catalog, or referenced by several. Catalogs
  are an organization and discovery mechanism, not a prerequisite for
  authoring or publishing a unit.
- A catalog needs no SemVer version and no unit-style release publication.
- A catalog can be shared or cloned onto other machines.
- `registry create` initializes a new catalog. Entries may be edited directly
  in `registry.jsonc`, then validated through registration or synchronization.

Open:

- Whether `registry.jsonc` may contain fields beyond `name` and `units`; how
  conflicting catalog identities are handled; which revision synchronization
  reads for a local catalog, and how uncommitted local catalog changes are
  treated; and how identical references are recognized when relative
  references come from different catalogs. **OC-32.**

## Catalog metadata index — Approved

A catalog publishes an **Assemblr-generated metadata index**, derived from the
units its `registry.jsonc` declares and from their published manifests. It
gives consumers and tooling the information they need without the complete
unit sources.

For each published version of each declared unit, the index describes:

- the unit identity and description;
- the exact version;
- its published Git provenance;
- its dependency requirements;
- its parameters, with types, descriptions, allowed values, and defaults;
- the information required for version resolution, validation, and integrity
  checks.

Rules:

- The unit's historical `manifest.jsonc` remains **authoritative**. The index is
  a generated, derived artifact, and it is not edited by hand.
- The index is regenerated **deterministically**: the same declared sources and
  published history produce the same index.
- Units outside any catalog need no index. Assemblr obtains their metadata
  directly (see [Metadata ingestion and trust](#metadata-ingestion-and-trust--approved)).

### Catalog refresh — Approved

Conceptual command (grammar not final, **OC-7**):

```text
assemblr catalog refresh
```

Catalog refresh is a **catalog maintenance** operation, not part of consumer
composition. It:

1. inspects the Git sources the catalog declares;
2. discovers their published unit versions;
3. retrieves and validates their manifests;
4. regenerates the catalog metadata index deterministically;
5. reports invalid sources and inconsistent declarations.

The catalog maintainer invokes refresh and publishes the refreshed index.
Refresh is suitable for automation, but Assemblr operates **no** mandatory
central service. A maintainer may refresh manually, from CI after changing
`registry.jsonc`, on a schedule, or when notified by a unit's publication
workflow. The maintainer chooses the mechanism.

- Publishing a new unit version does not modify the catalogs that reference
  the unit. Nothing runs in a catalog's repository merely because another
  repository published a tag.
- No webhooks, back-references from units to catalogs, or central coordination
  service are required. No provider-specific workflow is required in Alpha.

Open:

- The index's file name, serialization format, storage location, and hash
  representation; whether a catalog without a published index can be
  registered; and how refresh reports partial failures. **OC-40.**

## Synchronization — Approved

Conceptual command (grammar not final, **OC-7**):

```text
assemblr registry sync
```

`registry sync` updates **both catalogs and units**. It:

1. obtains changes from the registered Git sources;
2. inspects catalog changes, including added, removed, or modified unit
   references, and the catalog's published metadata index;
3. discovers newly published unit versions: through Git tags for
   individually registered units, and through the catalog's metadata index for
   catalog-managed units;
4. validates identities, manifests, tags, content integrity, and source
   conflicts (see
   [Metadata ingestion and trust](#metadata-ingestion-and-trust--approved));
5. commits the resulting local registry index **atomically**, only after that
   validation succeeds.

A single synchronization discovers newly added catalog entries **and** the
published versions of the units they reference. A newly published version of a
catalog-managed unit becomes discoverable through the catalog once the
catalog's metadata index has been refreshed and published (see
[Catalog refresh](#catalog-refresh--approved)).

Rules:

- The **complete** proposed index is validated before it replaces the active
  index. If a source-level validation fails, the previous valid state is kept
  and the problem is reported. A synchronization result is never partially
  accepted. An invalid version is excluded and reported, and does not reject
  the synchronization (see
  [Invalid versions and failed sources](#invalid-versions-and-failed-sources--approved)).
- A unit reference removed from a catalog is removed from the active discovery
  state.
- Changing the repository reference of an existing identity is a conflict that
  requires user intervention (see
  [Source conflicts](#source-conflicts--approved)).
- For a remote catalog, synchronization reads the catalog's **Git default
  branch**. The index records the **exact catalog commit** it was built from.
- Historical snapshots required by existing project locks and for
  reconciliation are preserved.
- Synchronization **never modifies** consumer projects, dependency selections,
  or lock files.
- Synchronization never rewrites a version that has already been recorded (see
  [Integrity and snapshots](#integrity-and-snapshots--approved)).
- Registration and synchronization are distinct operations.
- `registry sync` discovers changes made **outside** the local publishing flow,
  for example tags created or pushed elsewhere.

- A synchronization reads every registered source. A source that cannot be
  read, or any source-level failure or integrity conflict in any source,
  rejects the whole synchronization, and every problem found is reported (see
  [Invalid versions and failed sources](#invalid-versions-and-failed-sources--approved)).

Open:

- The classification of checks not covered by
  [Invalid versions and failed sources](#invalid-versions-and-failed-sources--approved);
  and whether synchronization can target individual sources. **OC-33.**
- Which revision is read for a local catalog, and how uncommitted local
  catalog changes are treated. **OC-32.**
- How the index update of `unit publish` and the discovery of individually
  registered units interact with versions a catalog's index does not yet list.
  **OC-33.**

## Metadata ingestion and trust — Approved

`registry add` and `registry sync` build the local index from:

- the generated metadata indexes of registered catalogs; and
- the metadata of individually registered units, which Assemblr obtains
  directly from their repositories.

Both produce the **same metadata model**, whichever way a unit was registered.

Catalog-generated metadata is **untrusted input**. It is not trusted merely
because it exists. Assemblr validates its structure, identities, provenance,
internal consistency, and correspondence with the authoritative published unit
data, to the extent the source-trust guarantees of this document require. The
source-conflict and validation policies above apply unchanged.

Efficient metadata-only ingestion and strong independent verification against
remote Git sources are in tension. Neither extreme is acceptable: Assemblr
neither trusts arbitrary catalog metadata without verification, nor downloads
the full sources of every unit during each ordinary registry operation.

Open:

- The verification and caching strategy that meets these guarantees. **OC-41.**

## Metadata and content — Approved

Assemblr distinguishes three levels of information:

1. **Source declarations**: catalog `registry.jsonc` entries and registered
   unit references.
2. **Validated unit metadata**: the local registry index.
3. **Complete unit content**: the files of a published version.

- Discovery and inspection work primarily from metadata.
- Complete unit content is retrieved and cached when actual composition
  requires it. It is not downloaded indiscriminately when a catalog is
  registered.
- Metadata is reused incrementally. Queries do not require repeatedly cloning
  complete repositories.

Open:

- Which operations retrieve unit content on demand, and how that retrieval
  relates to the rule that ordinary reads and authoring operations never fetch
  implicitly; and the Git partial-fetch and caching mechanisms. **OC-42.**

## Index freshness and fetching — Approved

The registry index is read **offline**. Remote changes are obtained
explicitly, through registration and synchronization.
[Catalog refresh](#catalog-refresh--approved) also reads remote sources, to
regenerate a catalog's metadata index.

- `unit publish` updates the local registry index automatically when the unit
  is **already registered**. It never registers a previously unknown unit, and
  it never pushes commits or tags to a remote (see
  [authoring-workflow.md](authoring-workflow.md#publication--approved)).
- If publication succeeds but the index update fails, Assemblr reports that
  partial outcome explicitly. `registry sync` recovers.
- `search` and `info` operate on indexed metadata. They never fetch remote
  changes implicitly.
- `unit check`, `unit build`, and `unit update` never fetch remote changes
  implicitly.
- If a required published version is not available locally, Assemblr reports
  that synchronization is needed.
- When the local index is missing or may be stale for a request, diagnostics
  state that limitation and direct the user to synchronization where
  appropriate.

## Integrity and snapshots — Approved

Assemblr distinguishes these kinds of state, which serve different purposes:

| State | What it is | Character |
|-------|------------|-----------|
| **Working tree** | The author's editable checkout of a unit repository | Mutable. Used by authoring operations such as `unit build`. Never a published version. |
| **Published version** | A `v<version>` tag in the unit repository | Immutable once published. |
| **Registry index** | Validated, discoverable metadata about registered sources, their units, and their published versions, including the exact catalog commit it was built from | Changed only by explicit registry operations and by the index update of `unit publish`. Never partially updated by a failed registration or synchronization. |
| **Snapshot** | A fixed, identifiable copy of a unit's content at a resolved version, with its Git provenance | Immutable historical state, needed for composition, amendments, and reconciliation. |
| **Lock state** | The exact resolved versions, sources, and content identities of a composition. For a consumer project, also the identity of its reconciliation base (see [consumer-projects.md](consumer-projects.md#lock-and-reconciliation-state--approved)). | Independent of registry synchronization. Preserves a resolution until it is changed explicitly. |

**Catalog revision tracking** and **historical artifact preservation** are
different things. The recorded catalog commit explains which inventory of
sources the current index was built from. It pins no unit versions. Snapshots
preserve the content of unit versions that locks and amendments depend on, and
they are kept even when a catalog later drops or changes a reference.

Rules:

- A published version stays associated with **exactly** the content first
  recorded for it. Moving a tag to another commit is not a valid way to update
  a version. Different content for the same identity and version is an
  integrity conflict, never trusted silently.
- A mutable working tree is never substituted for a published version.
- The registry never silently replaces historical content.
- Resolved versions and content are **identifiable and verifiable**.
- A published version's **content identity** is the Git tree of its tagged
  commit, and the commit is its provenance. The tree covers the entire
  repository snapshot at that commit, not only the files that contribute to a
  composition, so a change to any file in the repository, such as a README,
  gives a different identity (see
  [ADR 0006](../adr/0006-reading-git-repositories.md)).
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
  how lock state and snapshots record and verify content identity and how
  long historical snapshots are retained. **OC-8.**

## Removal — Approved

- `registry remove` removes a registered source's **registry relationship**
  only.
- It never deletes the user's Git repositories or working trees.
- It never modifies consumer projects or lock files. Historical snapshots
  required by existing locks and for reconciliation are not discarded.
- It intentionally discards the registration's recorded provenance.
  Registering the source again establishes a new baseline, so changes made in
  between, such as a moved tag, cannot be detected from the discarded record.
  This never makes changing a published tag valid: published versions stay
  immutable, and lock state verifies its own recorded content identity
  independently of the registry (**OC-8**).

## Registry commands — Approved

The user-facing registry command surface is deliberately minimal:

| Command | Responsibility |
|---------|----------------|
| `registry create` | Initialize a new Git catalog. |
| `registry add` | Register and validate a source. |
| `registry remove` | Remove a registered source. |
| `registry list` | List registered sources. |
| `registry info` | Inspect indexed information. |
| `registry sync` | Synchronize registered catalogs and units. |
| `registry doctor` | Diagnose registry inconsistencies. |

The table fixes **responsibilities**, not arguments or flags. There are no
commands solely for adding or removing individual catalog entries: entries are
edited in `registry.jsonc` and validated through registration or
synchronization.

Catalog maintenance adds one conceptual operation outside this table,
[catalog refresh](#catalog-refresh--approved).

### Implemented grammar — Approved

For local unit repositories:

```text
assemblr registry add <path>
assemblr registry list
assemblr registry list --json
assemblr registry remove <identity>
assemblr registry sync
```

- `add` takes the path of a local repository. Remote references are not
  supported by this grammar yet: one fails explicitly and changes nothing.
- `remove` takes the unit identity, which is unique in the registry, so a
  source can be removed even when its repository no longer exists.
- Data goes to stdout and diagnostics to stderr. A failed operation exits
  non-zero and leaves the registry index unchanged.
- `list --json` prints one JSON object, derived from the same index as the
  text listing:

  ```json
  {
    "units": [
      {
        "identity": "@acme/node",
        "source": "/home/me/units/node",
        "versions": [
          { "commit": "<commit id>", "tree": "<tree id>", "version": "1.0.0" }
        ]
      }
    ]
  }
  ```

  Units are ordered by identity, and versions in SemVer order. `source` is the
  stored [reference](#local-repository-references--approved). `commit` is the
  version's provenance and `tree` its content identity, as lowercase
  hexadecimal Git object ids. The order of keys within an object is not
  significant. Fields may be added, and existing fields keep their meaning. A
  backward-incompatible change needs a versioned output format or an explicit
  compatibility decision.
- Diagnostics keep excluded tags apart from indexed versions: each excluded
  tag is reported with its reason on stderr, and the result names the indexed
  versions and, separately, the tags excluded and not indexed.

Open:

- Exact grammar, arguments, and flags beyond the implemented grammar above,
  including for catalogs and remote sources; how `registry info` relates to the
  top-level `info` command; what `registry doctor` does beyond diagnosis; and
  how catalog operations (`registry create`, catalog refresh) are grouped.
  **OC-7.**

## Discovery and information — Approved

Assemblr exposes unit metadata that is useful to both humans and coding
agents. The registry makes this metadata discoverable, so a consumer does not
have to inspect source repositories by hand.

Discovery is **metadata-based** and reads the local index **offline**.
`search` matches against declared unit metadata in the index. It does not use
semantic or AI-driven inference.

Conceptual commands (grammar not final, **OC-7**):

```text
assemblr registry list
assemblr search cloudflare
assemblr info @metamao/cloudflare-worker
assemblr info @metamao/cloudflare-worker --json
```

`info` describes:

- identity and version, and the unit's published versions;
- the unit description;
- public parameters, with descriptions, types, requiredness, and defaults;
- allowed values;
- declared dependencies;
- dependency configuration requirements (`options`);
- relevant resolution and source information, where available.

Human-readable and machine-readable output are derived from the **same domain
information**. Neither is a separately maintained view.

The same indexed metadata supports agent-facing validation and diagnostics,
for example when a consumer composition is missing a required parameter (see
[consumer-projects.md](consumer-projects.md#parameters-and-non-interactive-operation--approved)),
and editing assistance for `assemblr.jsonc` (see
[consumer-projects.md](consumer-projects.md#editing-assistance--approved-direction-details-open)).

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
  Registry operations are safely repeatable when their inputs and intended
  state are unchanged. Expensive checks need a specific correctness or
  integrity reason.
- **Git.** Use Git's existing object and tag infrastructure rather than a
  parallel versioning mechanism. Consider annotated release tags (**OC-26**).
  Authentication to private repositories is delegated to the user's existing
  Git and SSH configuration. Assemblr has no credential manager in Alpha.
- **Snapshots and caches.** Snapshots are immutable and content-addressed, with
  explicit Git provenance. Recoverable caches are kept distinct from persistent
  historical state. Historical snapshots are not garbage-collected
  automatically in Alpha.
- **Version selection.** See
  [dependencies.md](dependencies.md#version-selection--approved).

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
from other units' uncommitted working trees. That capability is **not**
carried forward: in Alpha, dependencies resolve only from published versions
(see [dependencies.md](dependencies.md#resolution--approved)). This settles
**OC-30**.

Earlier drafts of this model also used an object form for catalog entries (for
example `{ "git": "…" }`), a non-owner catalog name, a `registry unit add`
command, and a recommendation to avoid manual catalog edits. All four are
superseded by the [catalog contract](#catalogs--approved) above.
