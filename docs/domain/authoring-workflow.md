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
consumer modifications, is the larger goal. That workflow is described in
[consumer-projects.md](consumer-projects.md).

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

- locates the dependency's file **recursively by filename**, searching the
  `src/` trees of the unit's **direct dependencies**, each at its version in
  the [unit lock state](#unit-lock-state--approved);
- **fails** if no file matches;
- uses the file when **exactly one** matches;
- **fails on ambiguity** and reports every matching path;
- accepts an unambiguous **relative path** when a filename is not enough;
- creates an **editable full-file copy** at `amendments/<path>`;
- records the copy's **exact base provenance** in the manifest: the
  dependency's identity and the locked version (see
  [amendments-and-composition.md](amendments-and-composition.md#layout-and-declaration));
- never **silently overwrites** an existing amendment.

### Amend details — Approved

These were decided by the owner for the first amendments increment.

**Command.** `assemblr unit amend <file> [--unit <identity>]`, run at the
repository's top level as `unit build` is. `<file>` without a `/` is a file
name, matched against the name of every file in the `src/` trees searched;
with a `/` it is a path in that `src/`, matched exactly. Either form matches
a template by its source name or by its delivered name, without
`.assemblr.tmpl`: `config/package.json` and `package.json` both find
`config/package.json.assemblr.tmpl`. The copy always keeps the source path,
suffix included. `--unit` restricts the search to one direct dependency;
without it, every direct dependency is searched. Several matches are
reported, each by its path and provider, and never resolved by preference.
The result goes to stdout; diagnostics go to stderr.

**The lockfile.** `unit amend` requires a unit lockfile that records the
current resolution: it resolves the dependencies under the lockfile as a
build does, and fails when there is no lockfile or the resolution would
change it, saying to run `unit build` first. It never writes the lockfile.
It takes the same lock as a build, so a concurrent build sees the manifest
either before or after the amendment.

**What it refuses, changing nothing:** a `--unit` that is not a direct
dependency; a path the unit's own `src/` holds, which would collide; a path
the manifest already declares; and a file at `amendments/<path>` whose
content differs from the provider's file, or that is not a regular file.

**Writing.** Everything is decided before anything is written. The copy is
created first, as a new file, and the manifest is replaced afterwards: the
declaration is inserted into the manifest's text, so comments and formatting
survive, and the edited text is read back and must describe exactly the
manifest plus the declaration before it replaces `manifest.jsonc` through the
temporary file `.assemblr-manifest.jsonc.new` beside it, a reserved name, and
a rename. The edit keeps the manifest's line endings. When the manifest
cannot be edited safely, the
command fails before writing and prints the member to add by hand. When the
manifest cannot be written, the copy is removed again. An interruption
between the two steps leaves a copy that is not declared: `unit build`
reports it, and running the same `unit amend` again completes it, keeping
the copy, as long as its content is still the provider's file.

## Check — Approved

`unit check` performs Assemblr-native verification:

- manifest validity;
- identity and version validity;
- parameter and options consistency, including that the required parameters
  of every dependency are satisfied through the dependency chain (see
  [parameters-and-options.md](parameters-and-options.md#satisfying-dependency-parameters--approved));
- dependency resolution;
- amendment validity: each amendment targets a direct dependency that
  provides the path, its declared base version is the locked version, the
  lockfile identifies that version's content, and the historical base is
  available;
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
through the registry, never to another unit's working tree, following the
[unit lock state](#unit-lock-state--approved) (see
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
- Development input values configure only the unit being built. Its
  dependencies receive configuration only through the unit's own `options`;
  a build has no other way to configure them.

### Build details — Approved

These were decided by the owner for the first `unit build`.

**Command.** `assemblr unit build`, with no arguments. It builds the unit
whose repository's top level is the current directory, which must hold
`manifest.jsonc`; parent directories are not searched. The result goes to
stdout. Diagnostics, and changes to the unit lockfile, go to stderr.

**Validations that gate a build** (OC-28):

- manifest validity, identity, and version;
- parameter and options consistency, including that the required
  parameters of every dependency are satisfied through the dependency chain;
- dependency resolution, under the [unit lock state](#unit-lock-state--approved);
- `src/` path validity and path collisions, on delivered paths (see
  [units.md](units.md#src-unit-files));
- amendment validity: each declared provider is a direct dependency that
  provides the path in its `src/` at the locked version, the declared base is
  the locked version, the copy exists, and no copy is undeclared (see
  below);
- rendering of the templates of the unit and of its dependencies, amended
  copies included (see
  [parameters-and-options.md](parameters-and-options.md#rendering--approved)).

The effective-contribution check (OC-11) does not gate a build; it gates
publication. Neither does a clean or committed working tree. A failed build
reports every problem it can discover independently. A problem in a later
step that cannot run after an earlier failure, such as composition after a
failed resolution, is not reported.

**Development values** (OC-6 stays open). Until a mechanism for supplying
development values exists, a unit that declares required parameters without
defaults cannot be built: the build fails, names each such parameter, and
says that supplying development values is not supported yet. This concerns
only the unit being built. Its dependencies' required parameters satisfied
through its `options` build as usual.

**The working tree's `src/`.** A build reads the files that committing
everything would publish, with their current working-tree contents:

- every tracked file, with its working-tree contents, not the staged ones;
  this includes a tracked file that an ignore rule matches;
- every untracked file that no ignore rule matches;
- not a tracked file deleted from the working tree.

The ignore rules are those of the `.gitignore` files in the working tree,
and only those. Rules specific to one clone (`.git/info/exclude`) or to a
machine or user (`core.excludesFile`, the global Git configuration) never
apply. Patterns match as Git matches them in that clone, which follows its
`core.ignoreCase` setting, usually set on case-insensitive file systems.

Symbolic links, including tracked ones checked out as plain files,
submodules, nested repositories, and other entries a unit file cannot be are
rejected and reported. On Unix a file is executable when its owner may
execute it. Windows file systems record no such bit, so there a tracked
file is executable when the Git index records it as executable, and an
untracked file is not.

**Templates.** The `src/` files ending in `.assemblr.tmpl`, of the unit and
of its dependencies, are rendered with the resolved parameter values of the
unit that provides each one, and delivered without the suffix (see
[units.md](units.md#templates) and
[parameters-and-options.md](parameters-and-options.md#rendering--approved)).
Every rendering problem is reported with its file and position, and a build
with any is rejected before `build/` or the lockfile changes.

**Amendments.** A build applies the unit's own amendments. `amendments/` is
read by the same rules as `src/`, including the path rules and the rejection
of symbolic links; an empty `amendments/`, or one holding only ignored
files, is no amendment. Each declared copy takes the place of the provider's
file: it is delivered at the provider's delivered path with the provider's
executable bit, and it is rendered exactly when the provider's file is, with
the provider's values and the unit's own as `own` (see
[amendments-and-composition.md](amendments-and-composition.md#template-contexts--approved)).
Every amendment that cannot be applied is reported with the others: a
declared base other than the locked version (see
[unit lock state](#unit-lock-state--approved)), a path the provider does
not have at that version, naming the provider's template or plain file when
only the suffix differs, a declaration without a copy, and a copy without a
declaration. The unit's own `src/` file at an amended path is a path
collision. A dependency whose published version declares amendments is
refused: composing published amendments is not supported yet.

**`build/`.**

- When validation fails, the previous `build/` is preserved, and the error
  says that `build/` and the unit lockfile were not changed.
- Only a real directory at `build/` is treated as disposable output. A
  symbolic link or a regular file there is rejected and left unmodified.
- A build publishes `build/` and the unit lockfile so that an interrupted or
  failed build never leaves a `build/` that looks complete but does not
  match the lockfile: there is either the previous build with the previous
  lockfile, no `build/`, or the new build with the new lockfile. A failure
  says which state remains.
- While it publishes, a build uses `.assemblr-build.new/`,
  `.assemblr-build.old/`, and `.assemblr-lock.json.new` beside `build/`.
  These names are reserved. Before writing there, a build records that it
  has started, and it clears the record once nothing it wrote there
  remains. The next build deletes what is at those paths only when that
  record shows an earlier build was interrupted. Otherwise anything there
  is someone else's content: it is reported and left unmodified.
- Builds of the same unit repository run one at a time. A build waits for
  any other build of the same repository to finish before it reads the
  unit's files, its lockfile, or what an interrupted build left, and keeps
  the others waiting until it has published. Concurrent builds therefore
  keep the guarantee above.

The mechanisms and their limits are in [ADR 0008](../adr/0008-unit-build.md).

Open:

- How development input values are supplied for required parameters without
  defaults. The values must be supplied explicitly. **OC-6.**

## Unit lock state — Approved

A unit repository has its own **CLI-managed lockfile**, the **unit lockfile**.
It is conceptually distinct from a consumer project's lockfile (see
[consumer-projects.md](consumer-projects.md#lock-and-reconciliation-state--approved)).

- It records the **exact resolved dependency versions** used while the unit is
  developed.
- `unit check`, `unit build`, and `unit update` use this state consistently.
  Existing locked versions take precedence over fresh resolution (see
  [dependencies.md](dependencies.md#resolution--approved)).
- A new version appearing in the registry, for example after a
  synchronization, never silently changes a locked version. Changing it is an
  explicit [update](#update--approved).
- It is intended to be **tracked** in the unit repository.
- It does **not** determine which versions a consumer project resolves. A
  consumer resolves the unit's dependencies within the ranges the unit
  declares, under the consumer's own lock state.

Decided by the owner for the first `unit build` (OC-46):

- The unit lockfile is `assemblr-lock.json`, at the unit repository's top
  level.
- It is versioned JSON, written by the CLI with sorted keys, so that it
  diffs cleanly. It is not JSONC: it is rewritten whole, which would lose
  comments.
- It records every resolved unit, direct and transitive, with its exact
  version, and the commit and root tree it was published with (see
  [registries-and-discovery.md](registries-and-discovery.md#identity-versions-integrity-and-sources--approved)).
  It records no source references (see OC-49).
- `unit build` creates it, and rewrites it when resolution changes it, for
  example when a dependency is added to or removed from the manifest, or a
  range is changed so that a locked version no longer satisfies it. Each
  change is reported. When nothing changes, the file is not touched. A
  change to the locked version of a unit the author amends is the exception:
  the build fails instead (below).
- A locked version that the registry now records with other content than
  the lockfile does is rejected, never used or silently relocked.
- Deleting the lockfile is not an integrity-preserving repair. It discards
  every locked version, and the next build resolves every dependency
  afresh, possibly to other versions.
- A lockfile that is not one Assemblr wrote, for example one with
  unresolved merge conflicts, is reported and left as it is.

The lockfile and amendments:

- The lockfile entry for a unit that the author amends is the **content
  identity of the amendment's base**: the declared base version, with the
  commit and tree it was published with. The lockfile is tracked, so it is
  published with the unit, and dependents and consumers verify bases against
  it (see
  [amendments-and-composition.md](amendments-and-composition.md#base-identity-and-verification)).
- Changing the locked version of a unit that an amendment is based on is an
  explicit [update](#update--approved), never a side effect of a build. When
  the resolution of `unit build` would change that version, for example after
  a range edit, the build fails and changes nothing, naming the amendments
  concerned. The diagnostic distinguishes the two causes: the dependency's
  resolved version changed, which `unit update` reconciles, or the declared
  base version was edited by hand, which reverting the edit fixes.

`unit amend` reads the lockfile and never writes it (see
[amend details](#amend-details--approved)).

Open:

- Which other operations write the lockfile. **OC-46.**

## Update — Approved

Updating a dependency is an **explicit** author action. It never happens just
because a new version becomes available.

`unit update` changes a dependency's resolved version in the
[unit lock state](#unit-lock-state--approved), and reconciles the unit's
amendments from their historical base to the newer dependency version.

- Reconciliation works on the **source templates**: the base at the old
  version, the amendment copy, and the base at the new version. The result is
  a new editable copy that keeps its template expressions. The declared base
  version and the lock change together with it (see
  [amendments-and-composition.md](amendments-and-composition.md#template-contexts--approved)).
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
- How reconciliation conflicts are presented to the author, and how the
  update is completed or abandoned. **OC-53.**

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
2. resolves and verifies dependencies and amendments, including that the
   unit lockfile the published version will carry identifies the content of
   every amendment base;
3. verifies that the required parameters of its direct dependencies are
   satisfied by defaults, its `options`, or mappings from its own parameters,
   and that those of transitive dependencies are satisfied through the
   dependency chain (see
   [parameters-and-options.md](parameters-and-options.md#satisfying-dependency-parameters--approved));
4. runs **all** mandatory Assemblr-native checks;
5. rejects effectively empty or redundant units;
6. requires a **committed, clean** Git state;
7. verifies the declared SemVer version;
8. rejects a version that is **already published**;
9. creates or validates the corresponding Git tag, `v<manifest.version>`;
10. ensures the tag references **exactly the validated content**;
11. preserves the immutability and traceability of published versions.

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
