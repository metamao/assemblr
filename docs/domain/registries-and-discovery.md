# Registries and Discovery

Assemblr manages a **local registry**: a first-class subsystem, owned by the
CLI, that records which unit sources the user has registered, which versions
those sources have published, and the metadata needed to discover and resolve
units. Remote registry services and marketplace functionality are **outside
Alpha**.

This model replaces the earlier Local Directory and Local Git registry
modalities (see [Superseded model](#superseded-model-non-normative)). The
engineering mechanisms behind it are recorded in
[ADR 0003](../adr/0003-managed-local-registry.md) and
[ADR 0004](../adr/0004-registry-validation-and-synchronization.md).

## The local registry — Approved

- Assemblr manages **one local registry per user**, global to the user's
  Assemblr installation. It is not recreated for each consumer project or unit.
- Assemblr owns the registry's lifecycle and persistent state. Users register
  and administer sources through CLI commands. They do not create registry
  directories or edit the registry's internal files.
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

- Every unit lives in a Git repository: **its own**, with `manifest.jsonc`
  at the repository root, or a **catalog**, with `catalog.jsonc` at the root
  and the unit in `units/<name>` (see
  [units.md](units.md#authoring-repository-convention--approved) and
  [catalogs](units.md#catalogs--approved)). Git is a requirement, not a
  preferred convention.
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
  manifest **at the tagged commit**, or, in a catalog, in `catalog.jsonc`. A
  version tag whose manifest or catalog declares a different version is
  **invalid**. A catalog's tag publishes every unit in it, and is invalid as
  a whole when any unit in it is.
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
**explicitly registered and validated**. A registered source is **a unit
repository**: the Git repository of one unit, or a catalog, whose units are
registered from it together. There are no separate registry types or
operating modes.

The `registry.jsonc` catalogs that once declared sets of unit repositories
were removed by the owner's decision of 2026-10-27
([#129](https://github.com/metamao/assemblr/issues/129)).

## Registration — Approved

Conceptual commands (grammar not final, **OC-7**):

```text
assemblr registry add ./assemblr-cloudflare-worker
assemblr registry add git@github.com:metamao/cloudflare-worker.git
```

Every incoming unit repository is **untrusted until validated**. `registry
add` validates it **before** incorporating it into the active registry. The
checks include:

- Git repository validity and accessibility;
- consistency of the unit identity with its historical manifests at published
  tags;
- agreement between published Git tags and manifest versions;
- the integrity of published artifacts;
- conflicts with sources that are already registered.

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
- `registry add` of a catalog registers every unit any of its valid versions
  publishes, or, with none, every unit committed at `HEAD`. Its units are
  registered and removed together, and synchronization registers a unit
  that a new version of the catalog publishes for the first time
  (**Provisional (D-54)**). A unit's identity in a catalog stays fixed as
  for any source; a later version that no longer publishes an indexed unit
  leaves that unit's earlier versions as they are.
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
  repository and a source conflict. The
  registration or synchronization is rejected under the atomicity guarantees
  above, and the active registry is left unchanged.

For unit repositories, the checks are classified as follows:

- **Version-level**, for a newly discovered version tag: a tag that does not
  point at a commit; a commit or root tree larger than Assemblr reads; a
  missing, oversized, non-UTF-8, or invalid manifest; a manifest declaring a
  different version; and invalid `src/` content, such as a symbolic link, a
  submodule, an invalid path, or too many entries. The tag is excluded and
  reported, and the operation continues. The structure of the content is
  validated when a version is first indexed, so an indexed version is a
  valid published artifact, not only a valid manifest. File sizes are not:
  registration does not fetch file contents, so a file or directory larger
  than Assemblr reads fails the first operation that reads the version
  (**Decided by the owner (2026-10-27, in session, #126)**, see
  [ADR 0037](../adr/0037-partial-fetch.md)).
- **Version-level, for a newly discovered version tag with amendments:** the
  properties the version shows on its own. Its `amendments/` is read by the
  rules of `src/`; every copy is declared and every declaration has a copy;
  each declaration names a declared dependency (manifest validation); and
  the version carries a unit lockfile that reads as one and locks each
  amended provider at the declared base (see
  [amendments-and-composition.md](amendments-and-composition.md#base-identity-and-verification)).
  A version that fails any of these is excluded and reported, like invalid
  `src/` content. Whether the provider provides the amended path at the base,
  and whether the base's tree is the one this registry records,
  depend on other sources and on the order they are synchronized in, so they
  are checked by the build that composes the amendment, not at index time.
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

- The classification of checks not covered above. **OC-33.**

## Source conflicts — Approved

- A repository never becomes available just because it is located somewhere
  Assemblr can see. Every unit repository must be registered explicitly.
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
- The unit's **manifest** supplies its identity. A repository location does
  not.

Open:

- Whether any explicit resolution mechanism exists beyond removing or
  correcting a source. The earlier contract allowed an explicit source
  selection and owner-to-source mappings. Whether those remain part of this
  model needs owner confirmation. **OC-23.**

## Synchronization — Approved

Conceptual command (grammar not final, **OC-7**):

```text
assemblr registry sync
```

`registry sync` updates the registered units. It:

1. obtains changes from the registered Git sources;
2. discovers newly published unit versions through their Git tags;
3. validates identities, manifests, tags, content integrity, and source
   conflicts;
4. commits the resulting local registry index **atomically**, only after that
   validation succeeds.

Rules:

- The **complete** proposed index is validated before it replaces the active
  index. If a source-level validation fails, the previous valid state is kept
  and the problem is reported. A synchronization result is never partially
  accepted. An invalid version is excluded and reported, and does not reject
  the synchronization (see
  [Invalid versions and failed sources](#invalid-versions-and-failed-sources--approved)).
- Changing the repository reference of an existing identity is a conflict that
  requires user intervention (see
  [Source conflicts](#source-conflicts--approved)).
- A catalog's indexed version that no longer publishes one of its indexed
  units is an integrity conflict, as a moved tag is.
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

## Metadata ingestion — Approved

`registry add` and `registry sync` build the local index from the metadata of
registered units, which Assemblr obtains directly from their repositories: the
manifest and lockfile at each version tag. No generated metadata index is
trusted in their place.

## Metadata and content — Approved

Assemblr distinguishes three levels of information:

1. **Source declarations**: registered unit references.
2. **Validated unit metadata**: the local registry index.
3. **Complete unit content**: the files of a published version.

- Discovery and inspection work primarily from metadata.
- Complete unit content is retrieved and cached when actual composition
  requires it. It is not downloaded indiscriminately when a source is
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

- `unit publish` updates the local registry index automatically when the unit
  is **already registered**: when the registry records the publishing
  repository's reference as the source of the unit's identity, it reads that
  source again and indexes the new version under the synchronization rules.
  A unit registered from another reference becomes discoverable when that
  source is synchronized. `unit publish` never registers a previously
  unknown unit, and it never pushes commits or tags to a remote (see
  [authoring-workflow.md](authoring-workflow.md#publication-details--approved-d-3-d-4)).
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
| **Registry index** | Validated, discoverable metadata about registered sources, their units, and their published versions | Changed only by explicit registry operations and by the index update of `unit publish`. Never partially updated by a failed registration or synchronization. |
| **Snapshot** | A fixed, identifiable copy of a unit's content at a resolved version, with its Git provenance | Immutable historical state, needed for composition, amendments, and reconciliation. |
| **Lock state** | The exact resolved versions, sources, and content identities of a composition. For a consumer project, also the identity of its reconciliation base (see [consumer-projects.md](consumer-projects.md#lock-and-reconciliation-state--approved)). | Independent of registry synchronization. Preserves a resolution until it is changed explicitly. |

Snapshots preserve the content of unit versions that locks and amendments
depend on, and they are kept even when a source's registration is later
removed.

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
  gives a different identity. This holds for every unit of a catalog too,
  which shares the root tree of the catalog's tag (see
  [ADR 0006](../adr/0006-reading-git-repositories.md)).
- Lock state is verified against the registry by content identity: a locked
  version agrees with the registry's record when both name the same tree.
  The locked commit is kept as provenance; a different commit with the same
  tree, as a pull request's merge commit gives, is the same content and
  leaves the lock unchanged (owner decision on #123). Moving a version's tag
  is still an integrity conflict at registration and synchronization,
  whatever tree the new commit holds.
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
  long historical snapshots are retained. **OC-8.** Decided by the owner for
  consumer projects (#41): the lockfile, the base's identity, and the stored
  base are in
  [consumer-projects.md](consumer-projects.md#the-lockfile-and-the-stored-base--approved).

## Removal — Approved

- `registry remove` removes a registered source's **registry relationship**
  only. For a unit of a catalog, that is every unit of the catalog, which
  are registered together (**Provisional (D-54)**).
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
| `registry add` | Register and validate a source. |
| `registry remove` | Remove a registered source. |
| `registry list` | List registered sources. |
| `registry info` | Inspect indexed information. |
| `registry sync` | Synchronize registered units. |
| `registry doctor` | Diagnose registry inconsistencies. |

The table fixes **responsibilities**, not arguments or flags.

### Implemented grammar — Approved

For unit repositories:

```text
assemblr registry add <path or URL>
assemblr registry list
assemblr registry list --json
assemblr registry remove <identity>
assemblr registry sync
```

- `add` takes the path of a local repository, or the reference of a remote
  one (**Provisional (D-35)**): an `https://`, `ssh://`, or `file://` URL, or
  the scp-like `user@host:path`. A remote reference is stored exactly as
  given. Other transports, such as `http://`, `git://`, and Git's
  `<transport>::` helpers, and an `https://` URL that carries credentials,
  fail explicitly before anything is fetched. `name:path` that also names a
  local path is refused as ambiguous; `./name:path` registers the local one.
- A remote repository is fetched with the user's own Git and SSH
  configuration, which supplies any credentials, into a copy that Assemblr
  keeps (**Provisional (D-35)**, see
  [ADR 0029](../adr/0029-remote-sources.md)). `add` fetches it, and `sync`
  fetches every remote source again before reading it. They fetch commits,
  trees, and the metadata files registration reads, not the other files of
  a version, which are fetched when the version is first read (see below and
  [ADR 0037](../adr/0037-partial-fetch.md)); nothing else fetches. `info`
  and `search` read the copy, offline, and so does every operation on a
  version already fetched. A
  remote that cannot be fetched is a source-level failure: the registration
  or synchronization is rejected and the index is unchanged. A tag that
  moved or disappeared on the remote is an integrity conflict, like a local
  one.
- `remove` takes the unit identity, which is unique in the registry, so a
  source can be removed even when its repository no longer exists. A
  catalog's units are removed together, and the output lists them.
- `add` of a catalog lists every unit it registered, each with its
  versions. A catalog's excluded tag is reported for the catalog, since it
  publishes all its units at once.
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

  Units are ordered by identity, and versions in SemVer order. The
  `direct` and `catalogs` fields, and the top-level `catalogs`, which
  described the `registry.jsonc` catalogs, were removed with them (#129,
  **Provisional (D-51)**).
  `source` is the stored [reference](#local-repository-references--approved). `commit` is the
  version's provenance and `tree` its content identity, as lowercase
  hexadecimal Git object ids. The order of keys within an object is not
  significant. Fields may be added, and existing fields keep their meaning. A
  backward-incompatible change needs a versioned output format or an explicit
  compatibility decision.
- Diagnostics keep excluded tags apart from indexed versions: each excluded
  tag is reported with its reason on stderr, and the result names the indexed
  versions and, separately, the tags excluded and not indexed.

- A remote unit's content is retrieved into its copy the first time an
  operation reads a version the copy does not hold: `add`, `compose`, and
  `update` in a project, and `unit build`, `unit check`, `unit update`, and
  `unit amend` for a dependency. This retrieves exactly the
  commit the index records, which must still exist and hold the indexed
  tree and manifest, so it never changes what is indexed or resolved
  (**Provisional (D-39)**, under OC-42).

For a registered unit, the top-level `info` command (**Provisional (D-19)**):

```text
assemblr info <identity>
assemblr info <identity>@<version>
assemblr info <identity> --json
```

- `info` reads the local index and the unit's repository, offline. It
  describes the newest stable version (the newest one when every version is
  a pre-release), or the exact version named after a second `@`. A version
  requirement is not accepted. The manifest is read at the commit the index
  records, so a version whose content no longer matches the index is an
  error, never described from what the repository holds now.
- The text lists the identity and version, description, source, the
  published versions in SemVer order, the commit and tree, the dependencies,
  the placeholders to replace, with their descriptions, the ignored tokens,
  and the amendments. A section with nothing to list says
  `none`. Names are in sorted order, so the output depends only on the
  registry and the repository.
- `--json` prints one object from the same data: `identity`, `version`,
  `description`, `source`, `versions`, `commit`, `tree`, `dependencies`
  (identity to requirement), `placeholders` (`replace`, placeholder to
  description, and `ignore`, an array of tokens), and `amendments` (path to `unit` and `base`). The
  order of keys is not significant, and fields may be added.
- An unknown unit or version, a unit without a published version, an
  unreadable repository, and a version that no longer matches the index each
  fail with exit status 1 and a hint.

For registered units, the top-level `search` command (**Provisional
(D-32)**):

```text
assemblr search <term>...
assemblr search <term>... --json
```

- `search` reads the local index and the units' repositories, offline, as
  `info` does. A unit matches when every whitespace-separated term occurs,
  ignoring case, in its identity or in the description of its newest stable
  version (the version `info` describes without a version). `search
  cloudflare worker` and `search "cloudflare worker"` are the same query. A
  query with no term fails with exit status 1.
- Each match is one line, in identity order: `<identity> <version>:
  <description>`. A unit with no published version is matched on its identity
  alone and shown as `(no published version)`.
- A unit whose version cannot be described, for example because its
  repository was moved, is matched on its identity alone, and a warning on
  stderr says so, since a match on its description may be missing. The other
  units are still searched, and the command succeeds.
- No match prints nothing on stdout, a note on stderr, and succeeds.
- `--json` prints one object from the same data: `units`, in identity order,
  each with `identity`, `source`, `version`, and `description`; the last two
  are `null` when the unit could not be described. The order of keys is not
  significant, and fields may be added.

Open:

- Exact grammar, arguments, and flags beyond the implemented grammar above,
  including for remote sources; how `registry info` relates to the
  top-level `info` command; and what `registry doctor` does beyond
  diagnosis. **OC-7.**

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
- the placeholders its files hold, with their descriptions;
- declared dependencies;
- relevant resolution and source information, where available.

Human-readable and machine-readable output are derived from the **same domain
information**. Neither is a separately maintained view.

The same indexed metadata supports agent-facing validation and diagnostics,
for example the [placeholder report](placeholders.md#consumer-report--approved),
and editing assistance for `assemblr.jsonc` (see
[consumer-projects.md](consumer-projects.md#editing-assistance--approved-direction-details-open)).

Not in Alpha:

- agent-only prompt fields or instructions in manifests;
- semantic or AI-based discovery or recommendation.

Open:

- The exact matching rules for `search`: which metadata fields are matched,
  how, and how results are ordered. Ordering must be deterministic.
  **OC-29.** **Provisional (D-32)**: see the [implemented
  grammar](#implemented-grammar--approved).

## Agent guide — Approved

Decided by the owner on 2026-10-23 (#105).

```text
assemblr guide
```

`guide` prints how a coding agent uses Assemblr: when to look for a unit
before writing code by hand, the project and unit workflows, and
what is never edited by hand.

- The guide describes the **installed version**. Its text changes with each
  release, and it is not a contract: an agent reads it, a program does not
  parse it.
- The command's **name is stable**. Instructions installed outside Assemblr,
  such as an agent's skill or a project's `AGENTS.md`, only say to run
  `assemblr guide`, so every later version keeps the command, and they never
  need updating when Assemblr changes.
- The guide belongs to the CLI, not to units. Manifests still carry no
  agent-only fields or instructions (see
  [above](#discovery-and-information--approved)); a unit documents itself with
  its own files.

```text
assemblr skill install <agent>
```

Decided by the owner on 2026-10-23 (#111). `skill install` writes the
agent's user-level skill, which says when Assemblr helps and to run
`assemblr guide`. The user runs it once per machine.

- `<agent>` names the coding agent. `claude` (Claude Code) is the only one
  supported; other agents may be added as values.
- It writes only when the user runs it, and only the skill's file. Running it
  again updates a skill that Assemblr wrote. A file that Assemblr did not
  write is kept unless the user asks to replace it.
- The skill names no command but `assemblr guide`, so it stays true when
  Assemblr changes. Assemblr recognizes a skill it wrote by a closing comment
  that names no command either, and every later version still recognizes it.

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
  version discovery, publication, and synchronization.
  Registry operations are safely repeatable when their inputs and intended
  state are unchanged. Expensive checks need a specific correctness or
  integrity reason.
- **Git.** Use Git's existing object and tag infrastructure rather than a
  parallel versioning mechanism. Release tags are annotated (**OC-26**, resolved).
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
- explicit registration of unit repositories replaces configured
  locations and the discovery of units inside grouping directories;
- CLI-managed state replaces user-maintained registry configuration;
- one Git repository per unit is mandatory, not preferred, unless the unit
  is published with others in a catalog.

The Local Directory registry let a unit under development resolve dependencies
from other units' uncommitted working trees. That capability is **not**
carried forward: in Alpha, dependencies resolve only from published versions
(see [dependencies.md](dependencies.md#resolution--approved)). This settles
**OC-30**.

Until #129, a **catalog** was a Git repository whose `registry.jsonc` mapped
unit identities to repository references, with a generated
`registry-index.json` of their published versions, maintained with `catalog
init`, `add`, `remove`, and `refresh`, and registered with `registry add`.
The owner removed it on 2026-10-27: each machine registers unit repositories
directly. ADRs 0005, 0030, 0031, and 0033 record the removed design.
