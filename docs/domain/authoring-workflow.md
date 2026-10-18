# Unit Authoring Workflow

This document describes the complete approved lifecycle for developing,
maintaining, validating, and publishing a unit.

## Goal — Approved

An author should be able to:

1. develop an opinionated implementation as a unit;
2. compose and inspect it locally;
3. validate its contracts;
4. publish a reproducible, versioned unit;
5. make it available through the local registry, by registering its
   repository directly or through a catalog.

Reusing and evolving units in consumer projects, without needlessly destroying
consumer modifications, is the larger goal. That workflow is reserved
(**OC-1**).

## Commands — Conceptual

The intended authoring commands are:

```text
assemblr unit init @metamao/cloudflare-angular-ssr
assemblr unit amend -u @metamao/cloudflare-worker -f package.json
assemblr unit check
assemblr unit build
assemblr unit update @metamao/cloudflare-worker
assemblr unit publish
```

The **operations and their intent** are approved. The exact command grammar,
flags, arguments, and edge-case behavior are **Open (OC-7)**. Registry and
catalog operations are described in
[registries-and-discovery.md](registries-and-discovery.md).

## Typical flow — Conceptual

```text
init ──> edit src/, manifest ──> amend (as needed) ──> check ──> build
  ──> verify build/ with stack-specific tools ──> commit ──> publish

later:  update <dependency> ──> check ──> build ──> verify ──> publish

once:   registry add <unit repository>, directly or through a catalog
```

## Initialization — Approved

A unit is a Git repository (see
[units.md](units.md#authoring-repository-convention--approved)), so Git is a
structural prerequisite of authoring. `unit init` establishes a valid unit
scaffold on that Git-based foundation, consistently for every unit. It:

- establishes the unit's Git repository;
- creates `manifest.jsonc`;
- provides a **local, offline JSON Schema** for manifest completion and
  diagnostics, and references it from the manifest through `$schema`;
- creates `.gitignore` containing `/build/`;
- avoids unnecessary mandatory scaffolding, for example empty `src/` or
  `amendments/` directories.

Assemblr's own validation remains authoritative over the schema.

Open:

- The schema's name, location, and tracking. **OC-17.**
- How `init` establishes the Git repository: for example, whether it
  initializes a new repository or adopts an existing one, how it treats a
  target inside another repository, and whether it creates an initial commit.
  Part of **OC-7.**

## Amend — Approved

`unit amend` creates an amendment from a dependency's file. It:

- locates the dependency's file **recursively by filename**;
- **fails** if no file matches;
- uses the file when **exactly one** matches;
- **fails on ambiguity** and reports every matching path;
- accepts an unambiguous **relative path** when a filename is not enough;
- creates an **editable full-file copy**;
- records the copy's **exact base provenance**;
- never **silently overwrites** an existing amendment.

Open:

- Which file tree the lookup searches: the dependency's own files, or its
  composed output including files from its own dependencies. **OC-20.**
- Amendment layout and declaration syntax. **OC-2**, **OC-3.**

## Check — Approved

`unit check` performs Assemblr-native verification:

- manifest validity;
- identity and version validity;
- parameter and options consistency;
- dependency resolution;
- amendment validity and availability of each historical base;
- merge validity;
- an **effective, non-redundant contribution**.

A unit that contributes nothing beyond its dependencies is **invalid for
publication**. Contributions may come from files, amendments, or meaningful
configuration of dependencies. This check is not a superficial "`src/` must
contain files" rule. A unit with no `src/` files can still contribute through
amendments or options.

Open:

- The exact no-op / effective-contribution algorithm. **OC-11.**

## Build — Approved

`unit build` composes the unit with its dependencies into a fixed local
directory:

```text
build/
```

Its purpose is to give the author a real, inspectable, consumer-like result.

`unit build` is an **authoring** operation. It composes the author's editable
working tree, so the author can develop and build without publishing every
change. It is not consumption of a published version. The unit's
**dependencies**, however, always resolve to **published versions** available
through the registry, never to another unit's working tree (see
[dependencies.md](dependencies.md#resolution--approved)).

`unit check`, `unit build`, and `unit update` never fetch remote changes
implicitly. If a required published version is not available locally,
Assemblr reports that synchronization is needed (see
[registries-and-discovery.md](registries-and-discovery.md#index-freshness-and-fetching--approved)).

Rules:

- `unit build` performs the **applicable Assemblr-native validations before
  generating** any output. A validation failure is reported, and no build is
  generated from an invalid unit.
- The output directory is always `build/`. There is no configuration for an
  arbitrary output directory.
- `build/` is **owned by Assemblr**. It is generated, disposable, and
  conceptually read-only for the author.
- `build/` is ignored by Git.
- Every build recreates `build/` **cleanly**. Assemblr may replace or delete its
  contents without prompting, and it needs no special detection of manually
  modified or foreign contents.
- `build/` needs no history or reconciliation mechanism. It is always
  rebuilt from scratch.
- All build operations stay **contained within `build/`**, with protection
  against path traversal and symlink escapes.
- A **failed build never damages authoring files**, such as the manifest,
  `src/`, or `amendments/`.

Open:

- How development input values are supplied for required parameters without
  defaults. The values must be supplied explicitly. **OC-6.**
- Exactly which `check` validations apply to `build`, for example whether the
  effective-contribution check, which gates publication, also gates build.
  **OC-28.**

## Update — Approved

Updating a dependency is an **explicit** author action. It never happens just
because a new version becomes available.

`unit update` reconciles the unit's amendments from their historical base to
the newer dependency version.

- Changing the **resolved version** of a dependency and changing its declared
  SemVer **range** are separate concerns.
- Assemblr **never widens a declared range automatically**. Changing a range is
  a separate, explicit change by the author.
- Conflicts must not leave a **silent partial update**.
- After updating, the author is responsible for rebuilding and checking the
  result.
- A newly discovered Git tag, for example after a registry synchronization,
  never updates a dependency automatically.

Open:

- Which version is chosen when the declared range allows several. This
  follows the resolution policy, which is not yet decided. **OC-16.**
- The command surface for updating a version and for changing a range.
  **OC-7**, **OC-27.**

## Testing: native validation vs functional correctness — Approved

Assemblr verifies **its own** domain invariants, resolution, and composition
contracts. The author verifies that the generated software **works**.

Assemblr cannot judge the semantic correctness of Angular, Cloudflare,
PostgreSQL, or any other ecosystem. An Angular SSR author, for example, must
confirm that the build compiles and runs in the intended runtime.

For Alpha:

- Native validations are mandatory.
- There is **no custom testing DSL** and no separate unit-testing framework.
- `tests/*.jsonc` is **not** an approved authoring contract.
- Authors run their existing stack-specific tools against `build/`.
- Publication does **not** certify functional correctness.

## Publication — Approved

Publication always goes through Assemblr's CLI, for consistency. A published
version is a Git tag, `v<manifest.version>`, in the unit's own repository. The
tag version must match the manifest version at the tagged commit exactly (see
[registries-and-discovery.md](registries-and-discovery.md#units-are-git-repositories--approved)).

`unit publish`:

1. validates the manifest and identity;
2. resolves and verifies dependencies and amendments;
3. runs **all** mandatory Assemblr-native checks;
4. rejects effectively empty or redundant units;
5. requires a **committed, clean** Git state;
6. verifies the declared SemVer version;
7. rejects a version that is **already published**;
8. creates or validates the corresponding Git tag, `v<manifest.version>`;
9. ensures the tag references **exactly the validated content**;
10. preserves the immutability and traceability of published versions.

The author must verify the functional build before publishing. Assemblr does
not infer or certify that verification. No approval token or extra
functional-test declaration is required.

Publication and the registry:

- If the unit is **already registered**, `unit publish` updates the local
  registry index automatically, so the new version is discoverable without a
  `registry sync`.
- Publication never registers a previously unknown unit. Registration is a
  separate registry operation, and registering never publishes.
- If publication succeeds but the index update fails, Assemblr reports that
  partial outcome explicitly. The published tag stands, and `registry sync`
  recovers the index.

Publishing locally is distinct from **pushing** to a remote Git server.
`unit publish` establishes the published version locally and **never pushes**
commits or tags as a side effect. Pushing is an explicit, separate operation.

Open:

- Whether Alpha provides a push operation, and its command or flag.
  **OC-13.**
- Edge cases, including untracked files, an existing tag that already points
  at the validated commit, annotated vs lightweight tags (annotated tags are
  recommended for consideration), and whether a new version must be greater
  than earlier published versions. **OC-26.**
