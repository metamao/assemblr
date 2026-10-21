# Consumer Projects

A **consumer project** is a software project composed from units. It is the
final consumer of units. It selects units, supplies values for their
parameters, and owns the application that results. This document defines how a
consumer project is initialized, how it declares its composition, and how
Assemblr composes and reconciles it.

Consumer composition is **file composition**. Assemblr resolves units and
composes and reconciles their file contributions. Technology-specific setup
and execution remain the responsibility of the consumer or their coding agent
(see [No external commands](#no-external-commands--approved)).

## Initialization — Approved

Conceptual commands (grammar not final, **OC-7**):

```text
assemblr init .
assemblr init my-prod-app
```

- `assemblr init` is explicit, and it is required before a project consumes
  units.
- `init` creates only the minimal Assemblr project configuration: the project
  configuration file, `assemblr.jsonc`.
- `init` does **not** generate application code, resolve, retrieve, or
  materialize units, run package managers or other external programs,
  initialize technology-specific frameworks, or modify existing application
  files.
- A project may begin with no units. It acquires its composition later,
  declaratively or incrementally.
- Lock and reconciliation state may be created lazily, when first needed.

Open:

- The initial contents and full schema of `assemblr.jsonc`. **OC-36.** The
  initial contents are decided provisionally (decision log D-9); see
  [initialization details](#initialization-details--approved).

### Initialization details — Approved

Decided for the first `assemblr init` under AGENTS.md §1.1 (decision log
D-9) and accepted by the owner with PR #42.

**Command.** `assemblr init [<directory>]`. The directory defaults to the
current one and is created, parents included, when it does not exist. It
writes `assemblr.jsonc` with empty `dependencies` and `options`:

```jsonc
{
  "dependencies": {},
  "options": {}
}
```

It creates nothing else and runs nothing: no Git repository, no files, no
lock. An existing declaration is never overwritten: a readable one means
the project is initialized already and the command changes nothing; one
Assemblr cannot read is an error. A directory holding `manifest.jsonc` is a
unit's repository, not a consumer project, and is refused.

## Project configuration — Approved

`assemblr.jsonc` is the editable declaration of the consumer's **desired
composition**. It is JSONC, and it is intentionally editable by humans and
coding agents. It records **current intent**, not a history of commands.

Its API deliberately resembles a unit's [`manifest.jsonc`](manifest.md):

| Field | Meaning |
|-------|---------|
| `dependencies` | The units the consumer **explicitly selects**, each with a SemVer requirement. |
| `options` | Parameter values the consumer supplies to its explicitly selected units. |

`dependencies` and `options` use the manifest's approved serialization (see
[manifest.md](manifest.md#serialization-of-dependencies-options-params-and-amendments--approved)).
`assemblr.jsonc` has **no** `params` field. Example (versions are
illustrative):

```jsonc
{
  "dependencies": {
    "@metamao/angular-i18n": "^1.2.0",
    "@metamao/neonpg-drizzle": "^1.0.0"
  },
  "options": {
    "@metamao/angular-i18n": {
      "defaultLocale": "es"
    }
  }
}
```

Rules:

- Only **explicit selections** belong in `dependencies`. Transitive
  dependencies are resolved, never declared redundantly.
- `options` supplies values only to explicitly selected units, and only for
  parameters those units expose in their `params`.
- `dependencies` and `options` have the same semantics as in a unit manifest
  wherever they apply: owner-qualified identities, the same SemVer requirement
  dialect, and the same validation, compatibility, and conflict rules for
  option values (see
  [dependencies.md](dependencies.md#declaration--approved) and
  [parameters-and-options.md](parameters-and-options.md#consumer-values--approved)).
  A consumer value never overrides an incompatible requirement through
  implicit precedence. Assemblr does not maintain two unnecessarily different
  declarative languages.
- A consumer project declares no public `params` contract. It is not a
  reusable unit.
- Editing `assemblr.jsonc` does **not** change application files by itself.
  The consumer runs `compose` to reconcile the project with its declaration.
- Assemblr's own validation is authoritative. Schemas only assist editing (see
  [Editing assistance](#editing-assistance--approved-direction-details-open)).

Open:

- The full schema of `assemblr.jsonc`, including its initial contents and
  whether it has fields beyond `$schema`, `dependencies`, and `options`.
  **OC-36.**

## Composition commands — Approved

Three operations change or apply a consumer composition. The operations and
their intent are approved. The grammar is **Conceptual** (**OC-7**).

```text
assemblr add @owner/unit
assemblr remove @owner/unit
assemblr compose
```

**`add`**:

- accepts **one** explicitly selected unit per invocation;
- adds or updates that unit's declaration in `assemblr.jsonc`. A newly added
  direct dependency gets a caret-compatible SemVer requirement (`^`) by
  default;
- resolves the unit's complete transitive dependency graph (see
  [Version resolution](#version-resolution--approved));
- validates and applies the resulting composition directly. No separate
  `compose` is needed afterward.

**`remove`**:

- removes the unit's explicit declaration;
- recalculates the effective dependency graph;
- reconciles the project's files with the new composition (see
  [Removal](#removal--approved));
- retires transitive units that no explicit selection still requires.

**`compose`**:

- reads the current `assemblr.jsonc`;
- resolves and validates the desired composition;
- materializes and reconciles its file contributions;
- updates the corresponding lock and reconciliation state;
- never changes the consumer's explicit dependency or option choices.

`add` and `remove` change the declaration and then compose it with the same
semantics as `compose`. There is no mandatory two-step "install, then compose"
workflow.

### Workflows — Conceptual

Declarative:

```text
assemblr init my-prod-app
cd my-prod-app
# edit assemblr.jsonc
assemblr compose
```

Incremental:

```text
assemblr init my-prod-app
cd my-prod-app
assemblr add @metamao/angular-i18n
assemblr add @metamao/neonpg-drizzle
```

Both workflows produce the same composition from the same declaration.

### Composition details — Approved

Decided for the first `assemblr compose` under AGENTS.md §1.1 (decision log
D-10 to D-12) and accepted by the owner with PR #42. They cover a project
composed for the first time; a project composed before is reconciled as
[reconciliation details](#reconciliation-details--approved) describes.

**Command.** `assemblr compose`, with no arguments, run at the project's top
level, which holds `assemblr.jsonc`. The result goes to stdout; diagnostics,
and what the composition could not carry or store, go to stderr. It never
fetches, runs no external program, and makes no commit.

**What it does.** It reads the declaration, resolves the explicit
selections from the local registry under the [lock](#lock-and-reconciliation-state--approved),
and validates and composes the result as a unit build does, with the
project as the root and its `options` as the root's values: parameters,
option conflicts, path collisions, amendments including several of one
JSON file, and rendering. Then it writes, in this order, the
reconciliation base to `.assemblr/`, a journal of the files it is about to
create, the files themselves, each created new in directories it creates,
with its executable bit, and the lock; and removes the journal. Every
version locked and every file written is reported.

**What it refuses, writing nothing:** a problem a unit build would report,
with the same wording; a conflict between units' contributions, which is a
composition error and not a pending-resolution state (**OC-47** stays
open); a delivered path that already holds content Assemblr did not
materialize, or whose directory on the way is a file or a symbolic link,
naming every such path (**OC-44** stays open; composing over existing
content is not supported yet); a delivered file that only the consumer's own
Git rules ignore and that the base would store for the first time (see
below); a delivered path Assemblr reserves for
itself: `assemblr.jsonc`, `assemblr-lock.json`, and anything under
`.assemblr/` (part of **OC-48**); a stored base that does not verify
against the lock; a lockfile that is a unit's, without reconciliation
state; a directory holding `manifest.jsonc`.

**Ignored files.** Before writing, the composition asks the project's Git,
read-only, which delivered paths it ignores, and which rule decides. Only
the project's own rules, its `.gitignore` files, decide what is unbased,
because the lock records it and every clone of the project must lock the
same way (decision log D-16, which revises D-11 and D-15). A file a
`.gitignore` ignores is written to the project, but its content is not
stored in the tracked base: the lock lists it as unbased, and a warning says
a later composition cannot reconcile edits to it. A `!` pattern in a
`.gitignore` re-includes a file over any rule of the machine. A project in
no repository ignores nothing. Whether a file is unbased is decided at every
composition that writes, from the project's rules in force then: a file the
project ignores since is no longer stored, and one it no longer ignores is
stored from then on, which a note announces.

The consumer's own rules, `.git/info/exclude` and the user's global
excludes, belong to one machine. They never decide the lock. They are the
consumer's safety preference, so a composition does not override them
either: a delivered file that only they ignore, and whose content the base
would store for the first time, is refused before anything is written,
naming the file, the pattern, and where it is written. The way out is
stated in the diagnostic: the project's `.gitignore` can ignore the file
(unbased for everyone) or re-include it with a `!` pattern (stored for
everyone), or the consumer removes their own rule. A file the base already
stores stays stored when a local rule appears later, since nothing new
enters the store. When `.assemblr/` or one of its directories is ignored by
any rule, a warning says the base will not travel with the project, and an
edited `.assemblr/.gitattributes` is restored and reported. Nothing is
staged, and content is never scanned for secrets.

**Repeating `compose`.** On a project composed before, the stored base is
verified against the lock first; a missing, corrupted, or unverifiable base
is an explicit failure that writes nothing and says to restore `.assemblr/`
from version control. Then the declaration is composed and compared with
the base by identity: the same composition means nothing to compose, and
nothing is written, whatever the consumer did to the files since; a
different one is reconciled with the project (see
[reconciliation details](#reconciliation-details--approved)).

**Interrupted compositions.** While the journal exists, a composition was
interrupted. The next `compose` recovers from it before anything else. The
lock is written after every file, so a lock that records the journaled
composition means it was complete except for the journal's removal: the
journal is removed and the files and the lock stand. Otherwise the
composition is reverted, file by file: one it created whose content is
still what was written is removed, one it replaced or removed is put back
to its previous content when the path holds what was written, or nothing,
and one whose content is neither is kept and named. The directories it
created are removed when empty, the journal and the previous content are
removed, and the composition continues with a fresh plan, in which a kept
file counts as the project's content.

Open:

- Exact grammar, including how `add` accepts a version requirement or
  parameter values, the exact requirement `add` records, how `remove` treats a
  unit that is not an explicit selection, and what happens to a removed unit's
  `options` entry. **OC-7.** The grammar of `init` and `compose` is decided
  provisionally above.
- Explicit update operations for a consumer project. **OC-38.**

## Explicit and transitive dependencies — Approved

Assemblr distinguishes what the consumer **explicitly wants** from what the
dependency graph **requires**.

- An **explicit selection** is a unit declared in `assemblr.jsonc`
  `dependencies`.
- A **transitive dependency** is a unit present only because a selected unit
  requires it, directly or indirectly.
- When the consumer runs `add` for a unit that is already present
  transitively, Assemblr **promotes** it to an explicit selection. It does not
  install or materialize a duplicate.
- Removing another unit never removes an explicit selection.
- The distinction governs **lifecycle**: which units are retained when others
  are removed. It does not change what an otherwise equivalent effective
  composition produces.

Conceptual example (hypothetical units). Given these declared dependencies:

```text
docker-bun       -> docker
angular-bun      -> docker-bun
cloudflare-bun   -> docker-bun
angular-ssr-trpc -> angular-bun, cloudflare-bun
angular-i18n     -> angular-ssr-trpc
neonpg-drizzle   -> cloudflare-bun
```

selecting only `@metamao/angular-i18n` and `@metamao/neonpg-drizzle`
composes all seven units, with `docker-bun` and `cloudflare-bun` each resolved
once and shared.

### Convergence — Approved

Given equivalent effective units, exact versions, parameter values, and
applicable consumer state, composition produces **equivalent materialized
results regardless of the order** in which units were originally added.
Installation order is never used as merge or option precedence (see
[dependencies.md](dependencies.md#resolution--approved) and
[parameters-and-options.md](parameters-and-options.md#rules--approved)).

## Version resolution — Approved

Consumer operations resolve versions as described in
[dependencies.md](dependencies.md#resolution--approved), following the policy
of **minimal necessary updates** defined in
[version selection](dependencies.md#version-selection--approved): `add` changes
locked versions only as far as the new constraints require, and explains every
version it changed. The lock records the exact resolved versions. Explicit
update operations are distinct from these minimal changes (**OC-38**).

## Parameters and non-interactive operation — Approved

Assemblr's CLI is **non-interactive by default**. It never blocks waiting for
a prompt or a confirmation.

- Valid parameter defaults are applied automatically.
- The consumer supplies explicit values through `options` in `assemblr.jsonc`
  or through command-line arguments (syntax **OC-7**).
- A missing required value or an invalid value fails clearly. A composition
  whose required configuration cannot be resolved is never partially applied.
- Diagnostics give an agent or a human enough to correct the declaration or
  the command: the relevant unit metadata, comparable to `info` (description,
  required and optional parameters, types, allowed values, and defaults),
  together with the validation errors.
- The consumer supplies values only to its **explicit selections**. It is
  never asked for the required parameters of transitive units: each published
  unit is responsible for satisfying its dependencies' required parameters
  (see
  [parameters-and-options.md](parameters-and-options.md#satisfying-dependency-parameters--approved)).
- Parameters customize file contents only. They never change dependency
  topology or which files exist (see
  [parameters-and-options.md](parameters-and-options.md#what-parameters-must-not-do--approved)).
- An option value is not a one-time initialization input. Changing it is a
  composition change, reconciled like a change to units or versions (see
  [Reconciliation](#reconciliation--approved)).

## No external commands — Approved

`add`, `remove`, and `compose` are file-composition operations. They never
run, implicitly or otherwise:

- package-manager commands, such as `bun install` or `npm install`;
- code generators, such as `wrangler types`;
- database migrations;
- framework build commands;
- container commands, such as Docker;
- scripts supplied by units.

The consumer or their coding agent performs technology-specific setup and
execution after composition. This rule concerns consumer composition. It does
not change how unit authors verify their builds with stack-specific tools (see
[authoring-workflow.md](authoring-workflow.md#testing-native-validation-vs-functional-correctness--approved)).

## File contributions — Approved

The files of a consumer project result from four distinct things:

| | Meaning |
|-|---------|
| **Initial delivery** | The files a unit introduces in its `src/`, rendered with its parameter values and materialized when the unit enters the composition. |
| **Amendments** | Modifications that units make to files their dependencies provide (see [amendments-and-composition.md](amendments-and-composition.md#amendments--approved)). |
| **Effective composition** | The deterministic result of composing every contribution for the current declaration. The previously materialized one is the **reconciliation base**, and the newly desired one is **incoming**. |
| **Consumer modifications** | Changes the consumer makes to the project's files after they are materialized. |

Rules:

- A file delivered from a unit's `src/` is a starting point. The consumer may
  modify it freely.
- That file keeps receiving changes through the effective composition, for
  example from the amendments of a unit added later, from a new version of a
  unit, or from changed option values. Consumer modification does not make a
  file immune to later contributions.
- Every contributed file follows the **same lifecycle**. Whether a change is
  applied depends only on whether it can be reconciled safely against the
  reconciliation base and the consumer's current content (see
  [Reconciliation](#reconciliation--approved)). A change that cannot be is a
  conflict, resolved through
  [conflict resolution](#conflict-resolution--approved).
- There are no file classes or ownership categories. Units declare none, and
  Assemblr neither determines nor records one for each file. The unit
  authoring contract is unchanged (see [units.md](units.md#src-unit-files)).
- Two units that provide a file at the same path through their `src/`
  directories make the composition fail with a **path collision** error,
  reported before anything is applied (see
  [units.md](units.md#src-unit-files)).

Open:

- How a composition treats a path that already holds content Assemblr did not
  materialize, for example in an existing project. **OC-44.** The first
  `compose` refuses it (decision log D-10); see
  [composition details](#composition-details--approved).
- File-system semantics of contributed files, including executable bits,
  symbolic links, binary content, line endings, paths that differ only in
  case, and paths reserved for Assemblr's own files such as `assemblr.jsonc`
  and the lockfile. **OC-48.** The first `compose` writes bytes and the
  executable bit as composed, never through a symbolic link, and reserves
  `assemblr.jsonc`, `assemblr-lock.json`, and `.assemblr/` (decision log
  D-10). A project file that differs from its base only by CRLF line
  endings counts as unchanged (decision log D-15).

## Reconciliation — Approved

Composition reconciles three inputs:

| Input | Meaning |
|-------|---------|
| **Reconciliation base** | The previously materialized composition. |
| **Current** | The consumer's actual files, including their own changes. |
| **Incoming** | The newly desired composition. |

(The reconciliation base is distinct from an amendment's
[base](amendments-and-composition.md#amendments--approved).)

- Independent changes are reconciled automatically. Incompatible changes are
  reported, never resolved by silently overwriting either side.
- The [merge semantics](amendments-and-composition.md#merge-semantics--approved)
  apply, including that downstream consumer edits are never arbitrarily
  overridden by incoming changes.
- Changes to units, versions, `options`, and amendments all go through the
  same reconciliation, which applies to every contributed file alike.
- Repeating `compose` without relevant changes does not rewrite the project.

The reconciliation strategy is decided provisionally in
[reconciliation details](#reconciliation-details--approved); Git takes no
part in it. **OC-10** stays open until the owner settles it.

### Reconciliation details — Approved

Decided for the first reconciliation under AGENTS.md §1.1 (decision log
D-13 to D-15) and implemented by `compose` on a project composed before.
They decide the consumer side of OC-10, which the merge-engine decision
left to the consumer lifecycle; the entry stays open.

**Per path, three-way, without Git.** Every path of the reconciliation
base and of the incoming composition is decided on its own from three
inputs: the base, what the project holds there now, and the incoming file,
each as a mode and a content identity.

- A side that did not change the file yields to the side that did: an
  incoming change to a file the consumer did not touch replaces it; a
  consumer's edit to a file the composition does not change stays; the same
  change on both sides is one change.
- A file that leaves the composition is removed when the project holds the
  base's content, and its directories are removed when they become empty.
  One the consumer edited is a conflict.
- A file the consumer deleted stays deleted when the composition does not
  change it, which is reported; one the composition changes is a conflict.
- A new file of the composition is created; content already at its path is
  refused as existing content (OC-44).
- A file both sides changed is merged when it is JSON or JSONC and its base
  is stored: the changes each side made against the base are derived with
  the [merge engine](amendments-and-composition.md#the-merge-engine--approved),
  the changes that collide are conflicts, and the composition's own changes
  are edited into the project's text, whose layout and comments stay. Any
  other file both sides changed, a text file, a binary file, or one whose
  base is not stored, is a conflict.
- The executable bit is reconciled like content.
- A managed path that is not a regular file is a conflict.

**Conflicts are refused.** A composition with any conflict is refused
before anything is written, naming every conflicting path and why; the
project, its lock, and its base stay as they were (standing default D2).
Resolving conflicts in the project, the pending-resolution state of
[conflict resolution](#conflict-resolution--approved), is a later
increment. A conflict between units' contributions remains a composition
error (OC-47).

**Line endings.** A project file that differs from its base, or from the
incoming file, only by CRLF line endings throughout counts as that content:
a checkout that converts line endings is not an edit. What is written to
such a file keeps CRLF line endings, and a note says so.

**What is written.** The new base is stored, the previous content of every
file to replace or remove is kept under `.assemblr/previous/`, the journal
is written, the files are created, replaced, and removed, each written
beside its path and renamed into place so that a path holds either its
previous or its new content, the lock is written with the new base,
managed, and unbased sets, and the journal and the previous content are
removed. `.assemblr/.gitignore`, which the store writes and restores like
`.gitattributes`, keeps the journal and the previous content out of version
control: a clone of an interrupted project has no journal, and its files
are the project's content. The report names every path created, modified,
merged, and removed, and every locked version that changed. What the
composition changed in a merged JSON or JSONC file beyond values, a member
order or a comment, is not edited into the project's text, which keeps its own
layout; a note names the file and what was not carried.

### Removal — Approved

Removing a unit is reconciliation, not indiscriminate deletion. Assemblr:

- retires obsolete contributions that can be safely retired;
- preserves contributions that other units still require;
- retains consumer-authored modifications where possible;
- reports a conflict when a removal would otherwise discard consumer
  modifications;
- recalculates dependencies and file contributions consistently.

A consumer-modified file is never deleted merely because the unit that
contributed it is no longer part of the effective graph.

### Preparation and failure — Approved

- Changes are prepared and validated **before** they are applied.
- Invalid resolution, incompatible configuration, or unresolved planning
  errors leave the project **unmodified**.
- If applying changes to the filesystem fails, Assemblr preserves enough state
  for safe recovery and reports what was and was not applied.
- Consumer changes are never lost silently.

Strict multi-file atomicity is **not** promised. The requirement is safe,
explicit, and recoverable behavior.

## Conflict resolution — Approved

Conflicts are resolved **directly in the consumer project**, in a Git-like
way. When composition finds unresolved conflicts:

- Assemblr reports the affected files and conflicts;
- the project enters an explicit **pending-resolution state**;
- conflicting files are editable at their normal project locations;
- humans and coding agents resolve them with their existing tools. There is no
  mandatory interactive editor, UI, or prompt;
- Assemblr provides access to the relevant reconciliation base, current, and
  incoming context.

Conceptual commands (grammar not final, **OC-7**):

```text
assemblr compose --continue
assemblr compose --abort
```

- **Continue** validates the resolutions and finalizes the composition.
- **Abort** restores the project to its state before the operation, including
  the consumer modifications that existed then.

Rules:

- Assemblr never creates commits or modifies the consumer's Git history, and
  it requires no particular branching or commit workflow.
- A pending conflict is the one deliberate exception to the expectation that
  operations leave no partially materialized files. Recovery and rollback
  remain required.
- Preparation may happen in isolation internally. Resolution happens in the
  consumer's project.
- No specific mechanism, such as Git worktrees or a particular conflict-file
  layout, is part of this contract.

Open:

- Conflict presentation and the mechanisms behind the pending-resolution
  state. **OC-37.**
- Whether conflicts **between units' contributions**, for example an amendment
  that cannot be applied to the dependency version the consumer resolved, are
  resolved in the pending-resolution state like conflicts with consumer
  modifications, or are composition errors; and, if they are resolved in
  place, what the incoming composition and the next reconciliation base are.
  **OC-47.**

## Lock and reconciliation state — Approved

Assemblr maintains a **versioned, CLI-managed lockfile** for the consumer
project, distinct from the editable `assemblr.jsonc` and from the
[unit lockfile](authoring-workflow.md#unit-lock-state--approved) of a unit
repository.

- It records the exact resolved unit versions and their provenance.
- It includes a **verifiable identity of the reconciliation base**.
- It is suitable for inclusion in the consumer's version control.
- The historical reconciliation base stays **recoverable or verifiably
  reconstructible**, without depending on the consumer project's Git history.
  A reconstructed base is checked against its recorded identity.
- The lock does not replace the state required for safe reconciliation and
  recovery.
- Lock state is independent of registry synchronization (see
  [registries-and-discovery.md](registries-and-discovery.md#integrity-and-snapshots--approved)).

### The lockfile and the stored base — Approved

Decided by the owner on #41 (OC-8) for consumer projects, and implemented
by the first `compose`:

- **The lockfile** is `assemblr-lock.json` at the project's top level. It
  is the unit lockfile's format, `format: 1` with `units` recording each
  resolved unit's `version`, `commit`, and `tree`, plus a `project` member
  holding the **reconciliation state**: its own `format` (1), `base`, the
  identity of the reconciliation base, `managed`, every path the
  composition manages, sorted, and `unbased`, the managed paths whose base
  is not stored. A unit's lockfile never has `project`, and a consumer's
  always does, so the two are unambiguous and one reader serves both. The
  lock records no source references (OC-49).
- **The base's identity** is SHA-256 over a canonical, versioned listing of
  the materialized files: the line `assemblr-base-v1`, then one line per
  file, `<mode> <sha256 of the content> <path>`, in path order, modes
  `100644` or `100755`, `\n` line ends. The identity is written
  `sha256-v1:<hex>`, so that a later algorithm or listing format is
  distinguishable. It depends on the files' bytes, modes, and paths only,
  never on the consumer's Git configuration, operating system, or checkout.
- **The base is stored in the project**, under `.assemblr/`, as tracked,
  content-addressed state: each file's bytes once under `objects/`, named
  by their SHA-256, and each base's listing under `bases/`, named by its
  identity's digits. Modes live in the listing, not in the file system, and
  `.assemblr/.gitattributes` marks everything there binary (`* -text`), so
  that a checkout cannot convert line endings or drop bits. The store is
  written before the project's files, is never garbage-collected, and is
  verified, listing and blobs, against the lock before any later
  composition uses it.
- **Recovery.** The stored base is authoritative; a base that is missing or
  does not verify is an explicit failure, never replaced by regeneration
  or by the project's current content. Regeneration from the locked inputs
  may be offered later only when its identity matches the recorded one.

Open:

- Regeneration of a missing base from the lock as a verified fallback, and
  how long bases are kept. **OC-8.**
- The pending-resolution state, continue, and abort. **OC-37.** The
  journal and the recovery of an interrupted composition are described in
  [composition details](#composition-details--approved) and
  [reconciliation details](#reconciliation-details--approved).
- Portability of a lockfile between machines: what happens when the source a
  lock records for a unit differs from the source registered for that identity
  on another machine, and what a lock records for a unit obtained through a
  relative catalog reference. Unavailable sources are **OC-9**. **OC-49.**

## Editing assistance — Approved direction, details Open

Locally indexed registry metadata improves editing `assemblr.jsonc`:

- unit-name discovery and completion;
- available versions;
- option-key completion;
- parameter descriptions and types;
- allowed-value hints;
- validation of the declaration.

Recommended (provisional) initial mechanism: a static JSON Schema for the
general document structure, plus a locally generated, project-aware schema
informed by the registry index, used through existing JSON and JSONC editor
tooling. No custom language server and no editor extension is required in
Alpha.

The CLI remains authoritative for dependency resolution, SemVer semantics, and
complete composition validation.

Open:

- Schema file names and locations, how the project-aware schema is generated,
  and how it is refreshed. **OC-36.**
