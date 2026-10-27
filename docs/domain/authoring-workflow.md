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
   repository.

Reusing and evolving units in consumer projects, without needlessly destroying
consumer modifications, is the larger goal. That workflow is described in
[consumer-projects.md](consumer-projects.md).

## Commands — Conceptual

The intended authoring commands are:

```text
assemblr unit init @metamao/cloudflare-angular-ssr
assemblr unit add @metamao/cloudflare-worker
assemblr unit amend -u @metamao/cloudflare-worker -f package.json
assemblr unit check
assemblr unit build
assemblr unit update @metamao/cloudflare-worker
assemblr unit publish
```

The **operations and their intent** are approved. The exact command grammar,
flags, arguments, and edge-case behavior are **Open (OC-7)**, except where a
details subsection below records a decision. Registry operations are
described in [registries-and-discovery.md](registries-and-discovery.md).

## Typical flow — Conceptual

```text
init ──> edit src/, add <dependency> ──> amend (as needed) ──> check ──> build
  ──> verify build/ with stack-specific tools ──> commit ──> publish

later:  update <dependency> ──> check ──> build ──> verify ──> publish

once:   registry add <unit repository>
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
- creates an empty `src/` directory, since most units need one and an
  author who does not can delete it (owner decision of 2026-10-25, in the
  session for [#120](https://github.com/metamao/assemblr/issues/120));
- avoids other unnecessary scaffolding, for example an `amendments/`
  directory or an empty marker file in `src/`.

Assemblr's own validation remains authoritative over the schema.

Open:

- How `init` establishes the Git repository: for example, whether it
  initializes a new repository or adopts an existing one, how it treats a
  target inside another repository, and whether it creates an initial commit.
  Part of **OC-7.** **Provisional (D-5, D-33)**; see
  [initialization details](#initialization-details--provisional-d-5-d-33).

### Initialization details — Provisional (D-5, D-33)

Decided for the first `unit init` under AGENTS.md §1.1 (decision log D-2 and
D-5) and merged with PR #37. The owner approved the schema (D-2, OC-17) in
the [owner review of 2026-10-23](https://github.com/metamao/assemblr/issues/34#issuecomment-6358234799); the rest is not approved explicitly.

**Command.** `assemblr unit init <identity> [<directory>]
[--description <text>]`. The directory is created, parents included, when it
does not exist. **Provisional (D-33):** without a directory, the unit gets a
new one named after it in the current directory, `web` for `@acme/web`. That
directory is used when it does not exist, is empty, or already holds a
`manifest.jsonc`; otherwise the command refuses, changing nothing, so a unit
never lands among unrelated files by default. A name that cannot be a
directory on every platform, such as `con`, is refused and needs a directory.
`.` sets up the unit in the current directory, for example to adopt an
existing repository. The description is the
manifest's; without one, it is "The `<identity>` unit." The result says what
was created, replaced, and already in place, and what to do next.

**The repository.** When no Git repository encloses the directory, `unit
init` runs `git init` there, with the author's own Git configuration, so
that settings such as the default branch name are the author's. When the
directory is the top level of an existing repository, that repository is
adopted. When it lies inside a repository whose top level holds
`manifest.jsonc`, the command refuses, changing nothing: a repository holds
exactly one unit. Inside any other repository, such as a home directory
kept in Git, the unit gets its own repository, nested in the enclosing one,
and the result says so. A bare repository and a linked worktree, whose
`.git` is a file, are refused: a unit's repository has a working tree with
`.git` at its top level. `unit init` never commits; the author commits, and
`unit publish` requires the result to be committed.

**The files.** `manifest.jsonc` is written with `$schema`, `name`,
`version` `0.1.0`, and `description`, in that order, and is read back by
Assemblr's own reader before it is written. An existing manifest is never
overwritten: one declaring the same identity is kept as it is, whatever its
version and content, and one declaring another identity, or one Assemblr
cannot read, refuses the command. The manifest's JSON Schema is written as
`manifest.schema.json` at the repository's top level, next to the manifest,
tracked in Git so that a fresh clone is editable without running Assemblr,
and referenced as `"$schema": "./manifest.schema.json"`. It is a fixed
document embedded in Assemblr, created when missing and replaced when it
differs; no other command reads or compares it, since Assemblr's validation
is authoritative, and running `unit init` again after an upgrade refreshes
it, which the schema's own description says. `.gitignore` is created with
`/build/`, or `/build/` is appended when no line of it already ignores
`build/`, keeping the file's line endings. A `README.md` and an `AGENTS.md`
are created when nothing is at their paths, and never read or changed
otherwise: the README with the unit's identity, its description, and
sections for the author to fill in, and `AGENTS.md` pointing coding agents
to `assemblr guide`, with a section of the same kind; the result asks the
author to fill them in. They sit at the repository's top level, outside
`src/`, so no project receives them. Owner decision of 2026-10-23
([#115](https://github.com/metamao/assemblr/issues/115)). An empty `src/` is
created when nothing is at that path, and nothing is put in it, since every
file there is delivered to consumers; Git does not track an empty directory,
so a clone has no `src/` until a file is added. No `amendments/` is
created. A file that is replaced is written beside its destination, as
`.assemblr-manifest.schema.json.new` or `.assemblr-gitignore.new`, reserved
names, and renamed into place; a new file is created directly. Running the
command again with the same identity completes whatever is missing and
changes nothing else.

## Add — Approved

`unit add` declares a dependency and builds the unit, so that `unit amend`
works right after it. Decided by the owner on 2026-10-23 (#99): the author
runs one command instead of editing `manifest.jsonc` by hand and building,
and `unit amend` keeps requiring a lockfile that records the current
resolution.

### Add details — Approved

**Command.** `assemblr unit add <unit>[@<requirement>]`, run at the
repository's top level as `unit build` is.
It edits `dependencies` in `manifest.jsonc` and then builds exactly as
`unit build` does, so the lockfile is still written only by `unit build`
and `unit update`. One unit per call. The result goes to stdout, and the
lockfile's changes to stderr, as a build reports them.

**Requirement.** As the consumer's `add` records it (see
[consumer-projects.md](consumer-projects.md#add-and-remove-details--approved-d-17)):
without one, a caret requirement on the version the resolution picks under
the lockfile, which is the locked version when the unit is a transitive
dependency already, so that promoting it changes no locked version; a bare
version is recorded as a caret requirement on it; any other requirement is
recorded as written. A unit declared already keeps its requirement unless
another is given.

**Placeholders.** The added unit's placeholders stay its own: nothing is
copied into the unit's manifest, and the build summary lists the
placeholders the composed result holds (see
[placeholders.md](placeholders.md#author-checks--approved)).

**Writing and recovery.** Everything is planned on the manifest's text and
validated as a build validates the working tree before anything is written:
a build that would be rejected leaves the manifest, the lockfile, and
`build/` as they were. The edit keeps the manifest's comments, formatting,
and line endings, and the edited text is read back and must describe exactly
the intended manifest, as `unit amend` checks its edit. The manifest is
replaced first, through the temporary file `.assemblr-manifest.jsonc.new`
and a rename, and the build is published afterwards; a build that fails to
write after the manifest was replaced is redone by `unit build`.

**Inside a unit.** `assemblr add` run in a directory that holds
`manifest.jsonc` says that it is a unit's repository and points to
`unit add`.

## Amend — Approved

`unit amend` creates an amendment from a dependency's file. It:

- locates the dependency's file **recursively by filename**, searching the
  `src/` trees of **every unit in the unit lock state**, direct or
  transitive, each at its locked version (owner decision on #100);
- **fails** if no file matches;
- uses the file when **exactly one** matches;
- **fails on ambiguity** and reports every matching path;
- accepts an unambiguous **relative path** when a filename is not enough;
- creates an **editable full-file copy** at `amendments/<path>`, holding the
  file's **layered base**: the provider's file as the units below compose it,
  which is the file as it stands in `build/` (see
  [amendments-and-composition.md](amendments-and-composition.md#the-base));
- records the copy's **exact base provenance** in the manifest: the
  dependency's identity and the locked version (see
  [amendments-and-composition.md](amendments-and-composition.md#layout-and-declaration));
- never **silently overwrites** an existing amendment.

### Amend details — Approved

These were decided by the owner for the first amendments increment.

**Command.** `assemblr unit amend <file> [--unit <identity>]`, run at the
repository's top level as `unit build` is. `<file>` without a `/` is a file
name, matched against the name of every file in the `src/` trees searched;
with a `/` it is a path in that `src/`, matched exactly. The copy keeps the
provider's path. `--unit` restricts the search to one locked dependency,
direct or transitive; without it, every locked unit is searched. Several
matches are reported, each by its path and provider, and never resolved by
preference. The result goes to stdout, naming the units below whose
amendments the copy is composed from, when there are any; diagnostics go
to stderr. A copy whose base holds a conflict marker between siblings is
created as it is, and the result says so: the unit resolves the conflict in
its copy.

**The lockfile.** `unit amend` requires a unit lockfile that records the
current resolution: it resolves the dependencies under the lockfile as a
build does, and fails when there is no lockfile or the resolution would
change it, saying to run `unit build` first. It never writes the lockfile.
It takes the same lock as a build, so a concurrent build sees the manifest
either before or after the amendment.

**What it refuses, changing nothing:** a `--unit` that is not a locked
dependency; a path the unit's own `src/` holds, which would collide; a path
the manifest already declares; a file whose base cannot be composed, for the
reasons a build reports; and a file at `amendments/<path>` whose content
differs from the composed base, or that is not a regular file. A file that a
unit below already amends is amended on top of its amendment, whatever the
format; a file that siblings amend is amended on top of their combination,
with their conflicts marked. While an update of the unit is interrupted,
it refuses to run at all, until `unit update` completes it (see
[update details](#update-details--approved)). A file of another provider
with the same name is a different file, and delivered-path collisions are
found by the build as usual.

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
the copy, as long as its content is still the composed base.

## Check — Approved

`unit check` performs Assemblr-native verification:

- manifest validity;
- identity and version validity;
- placeholder declarations and the tokens of the unit's files (see
  [placeholders.md](placeholders.md#author-checks--approved));
- dependency resolution;
- amendment validity: each amendment targets a locked unit, direct or
  transitive, that provides the path, its declared base version is the
  locked version, the lockfile identifies the content of every version the
  base is composed of, the historical base is available, and no copy holds a
  conflict marker;
- merge validity;
- an **effective, non-redundant contribution**.

A unit that contributes nothing is **invalid for publication**. Contributions
may come from files, amendments, or the aggregation of dependencies into a
reusable composition. This check is not a superficial "`src/` must contain
files" rule. A unit with no `src/` files can still contribute through
amendments or the units it composes.

The effective-contribution algorithm is approved as the [check
details](#check-details--approved-d-1-d-3) describe (OC-11, resolved).

### Check details — Approved (D-1, D-3)

Decided for the first `unit check` under AGENTS.md §1.1 (decision log D-1 and
D-3), merged with PR #36, and approved by the owner in the [owner review of 2026-10-23](https://github.com/metamao/assemblr/issues/34#issuecomment-6358234799).

**Command.** `assemblr unit check`, with no arguments, run at the
repository's top level as `unit build` is. It reads the unit as a build does,
runs every validation that gates a build, with the same diagnostics, and then
the checks that gate publication. It reports every problem it finds, exits
non-zero on any, and writes nothing: no `build/`, no lockfile, nothing in the
working tree. It takes the same lock as a build, so it never reads a manifest
and a lockfile that an update is replacing. While an update of the unit is
interrupted, it refuses to run, as a build does.

**The lockfile.** A published version carries its lockfile, so `unit check`
requires one that records the current resolution: a missing lockfile, and a
resolution that would add, change, or retire an entry, are reported with the
hint to run `unit build`, which writes it. `unit check` never creates it.

**Effective contribution** (OC-11, resolved; D-1). A unit is effective
when it delivers at least one `src/` file or declares at least one dependency.
Aggregation is a contribution: a unit whose purpose is to compose other units
into one reusable dependency contributes that composition, whether it
aggregates one unit or many. Since amendments presuppose a dependency, a
unit is ineffective exactly when it has no file and no dependency, and that
is reported. An amendment whose copy is the provider's file byte for byte at
the declared base is a **redundant** contribution and is reported by path:
it changes nothing and claims the file for no reason. The check gates
`unit check` and `unit publish`, never a build (OC-28).

**What a merge would not carry.** For each of the unit's own copies of a
JSON or JSONC file, `unit check` and `unit build` report, as warnings that
fail nothing, the comments the copy adds and the member orders it changes.
A copy alone takes the file's place whole, so nothing is lost today; a second
unit that amends the same file would make the merge drop them (see
[the merge engine](amendments-and-composition.md#the-merge-engine--approved)),
and the author hears of it before a consumer does.

**Placeholders.** A token of the placeholder form that neither the unit nor
its resolved graph declares or ignores, which a build only warns about, is a
problem `unit check` and `unit publish` report (see
[placeholders.md](placeholders.md#author-checks--approved)).

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
- Every file is delivered byte for byte: a build renders and substitutes
  nothing, and placeholders stay in `build/` as they are in the files (see
  [placeholders.md](placeholders.md)).

### Build details — Approved

These were decided by the owner for the first `unit build`.

**Command.** `assemblr unit build`, with no arguments. It builds the unit
whose repository's top level is the current directory, which must hold
`manifest.jsonc`; parent directories are not searched. The result goes to
stdout. Diagnostics, and changes to the unit lockfile, go to stderr.

**Validations that gate a build** (OC-28):

- manifest validity, identity, and version;
- dependency resolution, under the [unit lock state](#unit-lock-state--approved);
- `src/` path validity and path collisions (see
  [units.md](units.md#src-unit-files));
- amendment validity: each declared provider is a locked unit, direct or
  transitive, that provides the path in its `src/` at the locked version, the
  declared base is the locked version, the copy exists, holds no conflict
  marker, and no copy is undeclared (see below); for the amendments that
  resolved dependencies publish, each base is verified against the registry
  record, and siblings that conflict in a file the unit does not amend put
  the build in the pending state;
- the unit's placeholder declarations, after a resolution succeeds, since
  they are checked against the resolved graph (see
  [placeholders.md](placeholders.md#author-checks--approved)). A token the
  unit's files hold that nothing declares or ignores is a warning, not a
  problem that gates the build.

The effective-contribution check (OC-11) does not gate a build; it gates
publication. Neither does a clean or committed working tree. A build also
refuses to run while an update of the unit is interrupted, until
`unit update` completes it (see [update details](#update-details--approved)).
A failed build reports every problem it can discover independently. A problem in a later
step that cannot run after an earlier failure, such as composition after a
failed resolution, is not reported.

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

**Placeholders.** The result lists, after the summary, each placeholder the
composed files hold and in how many files, in name order. The warning about
undeclared tokens lists each token with its locations, one `path:line` per
row, and offers both fixes (see
[placeholders.md](placeholders.md#author-checks--approved)).

**Amendments.** A build applies the unit's own amendments and those its
resolved dependencies publish. The unit's `amendments/` is read by the same
rules as `src/`, including the path rules and the rejection of symbolic
links; an empty `amendments/`, or one holding only ignored files, is no
amendment. A published version's `amendments/` and lockfile are read from
Git at the indexed commit, by the same rules. Each declared copy takes the
place of the provider's file: it is delivered at the provider's delivered
path with the provider's executable bit. Every amendment that cannot be applied is reported with the others: a
declared base other than the locked version (see
[unit lock state](#unit-lock-state--approved)), a provider that is not a
locked unit, a path the provider does not have at that version, a
declaration without a copy, and a copy without a declaration. For a
published amender, its lockfile's identity of each version its base is
composed of is compared with the registry record, and a disagreement is an
integrity conflict; a provider or a unit below resolved to a version other
than the amender locked is not a problem for a published amender, whose
changes are moved onto the version resolved (see
[amendments-and-composition.md](amendments-and-composition.md#across-provider-versions--approved)),
and is for the unit's own amendment, whose base is its locked state. The
amenders of one file are composed in layers (see
[amendments-and-composition.md](amendments-and-composition.md#the-base)):
a unit's copy on top of the composition of the units below it, whatever the
format, and siblings side by side, by the merge engine for a JSON or JSONC
file and by line for text; what a copy carries beyond values is reported as
a warning, and insertions siblings make at one position are all kept in the
order of the units' identities, and reported. YAML, TOML, and INI files are
text here (owner decision on #92). An XML file that siblings amend, or a
file or copy that is not UTF-8 text, is not amended by any of them, and the
build is rejected (see
[amendments-and-composition.md](amendments-and-composition.md#the-merge-engine--approved)).
The unit's own `src/` file at an amended path is a path collision.

**Pending state** (owner decision on #100: the unit flow behaves as the
consumer's pending resolution does). When siblings conflict in a file the
unit does not amend yet, the conflict is the unit's to resolve, and the build
creates the amendment itself: the copy at `amendments/<path>`, holding the
sources layered with each conflict between markers, the declaration in
`manifest.jsonc`, and the lockfile, since the resolution is sound and the
conflict is a matter of content (**Provisional (D-42)**). Nothing is written
under `build/`. The build reports each file with its conflicts, says what was
written, and exits non-zero, saying to edit the copy, remove every marker,
and build again. While any copy under `amendments/` holds a marker, `unit
build`, `unit check`, and `unit publish` refuse, naming the files, and change
nothing; `unit amend` of another file still works. A conflict in a file the
unit does amend is reported as a build error: its copy resolved what it saw,
and the rest is a change below (see
[update details](#update-details--approved)).

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
- `unit update` rewrites it with the versions the author updates, and, for
  an amendment it carries over, rewrites the declared base in the same
  operation (see [update details](#update-details--approved)).
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
  base version disagrees with the lockfile, as after an edit of the
  declaration or an update whose record was deleted, which reverting the
  declaration to the locked version fixes.

`unit amend` reads the lockfile and never writes it (see
[amend details](#amend-details--approved)). `unit publish` never writes it
either: it requires the lockfile to be committed and to record the current
resolution (see [publication details](#publication-details--approved-d-3-d-4)), so
the lockfile's writers are `unit build` and `unit update`.

`unit publish` never writing the lockfile is approved (D-4, OC-46 resolved).

## Update — Approved

Updating a dependency is an **explicit** author action. It never happens just
because a new version becomes available.

`unit update` changes a dependency's resolved version in the
[unit lock state](#unit-lock-state--approved), and reconciles the unit's
amendments from their historical base to the newer dependency version.

- Reconciliation works on the files as they are: the base at the old
  version, the amendment copy, and the base at the new version. The result is
  a new editable copy, placeholders included. The declared base version and
  the lock change together with it.
- Changing the **resolved version** of a dependency and changing its declared
  SemVer **range** are separate concerns.
- Assemblr **never widens a declared range automatically**. Changing a range is
  a separate, explicit change by the author.
- Conflicts must not leave a **silent partial update**.
- After updating, the author is responsible for rebuilding and checking the
  result.
- A newly discovered Git tag, for example after a registry synchronization,
  never updates a dependency automatically.
- When the declared range allows several versions, the [resolution
  policy](dependencies.md#resolution-policy--approved) chooses (**OC-16**,
  resolved).

Open:

- The command surface for changing a range. **OC-7**, **OC-27.**

### Update details — Approved

These were decided by the owner for the first `unit update`, and extended on
#59 to rebase a copy when the provider's file changed.

**Command.** `assemblr unit update [<identity>...]`, run at the repository's
top level as `unit build` is. Without arguments it updates every locked
dependency. With identities it updates those, direct or transitive; an
identity that is neither locked nor declared as a dependency by the unit or
by a locked version is an error. It requires a unit lockfile, and fails
without one, saying to run `unit build` first, which creates it. It never
changes a declared range, never fetches, and never builds: after it, the
author rebuilds and checks the result. It takes the same lock as a build.
The result, every version that changed and every amendment carried over,
goes to stdout; diagnostics go to stderr.

**Resolution.** The dependencies are resolved as a build resolves them,
with the [unit lock state](#unit-lock-state--approved) minus the updated
units, so that the other locked versions keep precedence and change only
when the updated units' requirements force it (see
[dependencies.md](dependencies.md#resolution-policy--approved)). What changed is
reported against the complete previous lock state, and every kept version,
named or not, must still be published with the content (tree) it was
locked to; otherwise the update fails, as a build does. A run that changes nothing
reports that and touches no file.

**Amendments** (owner decision on #100, replacing the earlier statement
that no marker-based resolution is offered for a unit's amendments). For
each of the unit's own amendments, the update composes its **layered base**
twice: before the update, at the lock state, and
after it, at the new resolution (see
[amendments-and-composition.md](amendments-and-composition.md#the-base)).

- when the two bases are the same bytes, the amendment is **carried over**:
  the copy is untouched, and when the provider moves the declaration's `base`
  becomes the new version and the lock records it, in one operation;
- when they differ, because the provider moved or a unit below did, and the
  file is JSON or JSONC, or UTF-8 text outside the structured formats without
  a merge (XML; YAML, TOML, and INI are text, owner decision on #92), the
  amendment is **rebased**: the base before, the copy, and the base after are
  merged three ways, by value for a JSON or JSONC file and by line otherwise,
  with neither side taking precedence, so the copy keeps its own layout. A change below that the
  copy did not touch flows through. Insertions the base and the copy make at
  the same position are both kept, the base's first, as a composition applies
  them across versions (see
  [across provider versions](amendments-and-composition.md#across-provider-versions--approved)).
  What both the copy and the update change is **not applied silently**: it is
  left in the copy between `<<<<<<< current (your copy)`, `||||||| base
  (before the update)`, `=======`, and `>>>>>>> incoming (the base after the
  update)` lines for text, and as the marker object the layered base uses for
  a JSON value, naming each side as `current` and `incoming`, and each is
  reported. The copy is rewritten, the declaration's `base` follows the lock,
  and the lock records the new versions, in one operation. A conflict marker
  between siblings that the base after the update holds, in lines the copy
  did not change, flows into the copy the same way, and is reported. The
  update then exits non-zero, saying that it was applied and that the author
  edits the copy, removes every marker, and builds;
- otherwise the update fails and changes nothing, naming the amendment, the
  provider, the old base, the new version, and the cause: the base changed
  and the file has no merge or is not text, the copy cannot be moved onto
  the new base, or the new version no longer provides the path. The author
  resolves it by removing the declaration and the copy and amending the file
  again at the new version.

A declaration whose base is not the locked version was edited by hand, and
fails the update too.

**Sibling conflicts.** When the dependencies after the update conflict in a
file the unit does not amend, the update creates the amendment as a build
does in the [pending state](#build-details--approved): the marked copy, its
declaration, and the lock, in the same operation as the update, and reports
it the same way.

**Published amendments.** An amendment published by a dependency, whose
provider resolves to a version other than its base, does not block the
update: composition applies it to the version resolved (see
[amendments-and-composition.md](amendments-and-composition.md#across-provider-versions--approved)).
The declared requirement on the provider still decides which versions the
resolution may choose.

**Writing and recovery.** Everything is decided before anything is written.
An update replaces `assemblr-lock.json` and, when an amendment is carried
over or created, `manifest.jsonc`, whose declarations are rewritten in place
or added so that comments and formatting survive, as `unit amend` edits it,
and the copies it rebased or created under `amendments/`. Before any file is
touched, the update records
every file's contents before and after it in `assemblr-update.json` in the
repository's Git directory. It then replaces the copies, in path order, each
through a temporary file beside it and a rename, then the manifest through the
temporary file `.assemblr-manifest.jsonc.new` and a rename, then the lockfile
through `.assemblr-lock.json.new` and a rename, and removes the record last. A
failure after the record is written says which files were replaced. While
the record exists, `unit build` and `unit amend` refuse to run and point at
`unit update`, which completes the interrupted update before anything else:
a file that still has its recorded content from before the update is
replaced with the planned content, a file that already has the planned
content is left as it is, and the record is removed. A file with any other
content, changed since the update started, is never overwritten: the update
stops, keeps the record, and says to restore the file to either recorded
content or to delete the record to abandon the update. A record that
Assemblr cannot read is reported and kept. Nothing under `src/` is ever
written by an update, and under `amendments/` only the copies it rebased or
created.

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
3. verifies the unit's placeholder declarations, and that every token of
   the placeholder form in its files is declared or ignored by the unit or
   its resolved graph (see
   [placeholders.md](placeholders.md#author-checks--approved));
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

### Publication details — Approved (D-3, D-4)

Decided for the first `unit publish` under AGENTS.md §1.1 (decision log D-3
and D-4), merged with PR #36, and approved by the owner in the [owner review of 2026-10-23](https://github.com/metamao/assemblr/issues/34#issuecomment-6358234799) (OC-26 and
OC-46 resolved).

**Command.** `assemblr unit publish`, with no arguments, run at the
repository's top level as `unit build` is. The version is the one the
committed manifest declares. The result, the tag and what happened to the
registry index, goes to stdout; diagnostics go to stderr. It takes the same
lock as a build, and refuses to run while an update of the unit is
interrupted.

**Committed, clean state.** Committing everything must change nothing: a
staged or unstaged change to a tracked file, a path in conflict, and an
untracked file that no `.gitignore` ignores are each reported by path, and
the publication is refused. The rule is the one the build reads `src/` with,
so a file the build composed is never left out of the publication as
"ignored": `.git/info/exclude` and the user's global excludes do not apply,
and `build/` is ignored by the unit's own `.gitignore`. A repository with no
commit cannot publish.

**What is validated.** The tree of the commit at `HEAD`, read with the
readers the registry and dependents use for a published version, never the
working tree. With a clean tree the two agree; validating the tree makes "the
tag references exactly the validated content" a check rather than an
assumption. Every check of `unit check` runs on it, including the lockfile:
`assemblr-lock.json` must be in the commit and record exactly the current
resolution, with the content identity of every locked version, which the
amendment bases are verified against. A missing or stale lockfile is
reported with the hint to run `unit build` and commit.

**The tag.** `v<version>`, annotated, with the message `<identity>
<version>`, created at the commit's id with the author's own Git
configuration and identity, so that the tagger is the author and signing
follows the author's settings; a missing identity is reported with Git's
explanation. A tag of the version that already points at `HEAD` means the
version is already published: nothing is created, the command exits zero,
and the index update below still runs, which is how a failed one is
retried. A tag of the version at another commit refuses the publication, as
does a published version of equal SemVer precedence, which differs only in
build metadata (OC-25). A new version need not be greater than earlier
published versions; only uniqueness is required, so a backport such as
`1.2.5` after `2.0.0` is legitimate.

**The registry.** When the local registry records this repository's
reference as the source of the manifest's identity, the new tag is checked
against the registry's synchronization rules before it is created, so a tag
the registry would reject, for example because this repository is registered
under another identity or an indexed tag of it was moved, is refused rather
than created and left unindexable. After the tag is created, the registry
reads this one source again and indexes the version under the registry lock,
with the validation and integrity rules of synchronization; the other
registered sources are not read. When the identity is registered from
another reference, or not registered at all, the result says so and that the
version is not discoverable until that source is synchronized or the
repository is registered; nothing is registered, and this is not a failure.
When the index update fails after the tag was created, the command reports
the partial outcome, exits non-zero, and says that the tag stands and that
`registry sync` indexes the version.

**Ordering.** Everything is decided before the tag is created. After it,
only the index update can fail. Nothing is pushed or fetched.

**Pushing.** Alpha provides no push operation (owner decision, OC-13
resolved). Other machines see a version once the commit and its tag reach
the remote that they register, so after a new publication the result
suggests `git push --follow-tags`, which pushes the commit and its annotated
tag but no other local tag.

## Authoring a catalog — Approved

A [catalog](units.md#catalogs--approved) is authored with the same
commands, run at its top level for every unit or in a unit's directory,
`units/<name>`, for that unit (owner decision of 2026-10-27, adopting the
catalog design; #127). The details are **Provisional (D-53)** unless noted.

- **Siblings come from the working tree.** A build or a check reads every
  unit of the catalog from its working tree, and resolves each sibling a
  unit requires as `"catalog"` at the catalog's version with that content,
  never from the registry. A problem in one unit's files fails the build or
  check of the others, since they resolve together; at the top level, each
  problem names its unit as `units/<name>`.
- **One lock.** The catalog's units resolve together into one lock, which
  is the catalog's one lockfile, `assemblr-lock.json` at its top level, and
  which records what they depend on in other repositories, never the units
  themselves. Each unit is then composed under that lock. A unit's build
  and a build of every unit write the same lockfile.
- **`unit build`** at the top level builds every unit into its
  `units/<name>/build/`, in the order of their directories, and writes
  nothing unless every unit builds. The builds are then written one unit at
  a time, the first with the lockfile; a failure while writing names the
  units already built, and building again completes it. At the top level,
  a conflict between amenders of a file a unit does not amend is reported
  with the unit; the unit's own build creates the pending state in its
  directory, as for a unit outside a catalog.
- **`unit check`** at the top level checks every unit, and reports a
  missing or stale lockfile once, for the catalog.
- **`unit publish`** runs at the top level only: in a unit's directory it is
  refused, since a catalog's units are published together. It validates
  every unit of the commit at `HEAD`, each with its committed siblings,
  as a single unit's publication is validated, and creates one annotated
  tag, `v<version>` of `catalog.jsonc`, with the message `<catalog name>
  <version>`. The registry part is a single unit's, with every unit of the
  catalog.
- **`unit init`** at the top level, or with a directory in `units/`,
  creates the unit in `units/<name>`, in the catalog's repository: no Git
  repository of its own, a manifest without `version`, and the rest of the
  scaffold in the unit's directory. Its identity must be the one the
  catalog gives the directory. `unit init` never writes a manifest beside
  `catalog.jsonc`.
- **`unit add`, `unit amend`, and `unit update`** refuse to run in a
  catalog for now, with a hint to edit the unit's manifest or copy and
  build (**Deferred**, decision log D-56).
